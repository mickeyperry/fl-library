"""Explorer right-click actions: flctl.py status|rating|tags|notes|rescan ..."""
import ctypes
import json
import os
import subprocess
import sys
import time

import scanner

HERE = os.path.dirname(os.path.abspath(__file__))
SHCNE_UPDATEITEM, SHCNF_PATHW = 0x00002000, 0x0005
FIELDS = {'status': 0, 'tags': 1, 'rating': 2, 'notes': 3, 'bookmark': 4}


def refresh(paths):
    for p in paths:
        ctypes.windll.shell32.SHChangeNotify(SHCNE_UPDATEITEM, SHCNF_PATHW, ctypes.c_wchar_p(p), None)


def song_for(db, path):
    sql = 'SELECT song_key FROM files WHERE path = ? COLLATE NOCASE OR container = ? COLLATE NOCASE'
    row = db.execute(sql, (path, path)).fetchone()
    if not row:
        scanner.index_one(db, path)
        row = db.execute(sql, (path, path)).fetchone()
    return row[0]


def ask(title, prompt, initial):
    import tkinter
    from tkinter import simpledialog
    root = tkinter.Tk()
    root.withdraw()
    root.attributes('-topmost', True)
    value = simpledialog.askstring(title, prompt, initialvalue=initial, parent=root)
    root.destroy()
    return value


def set_field(field, value, path):
    if '|' not in path:
        path = os.path.abspath(path)
    db = scanner.connect()
    key = song_for(db, path)
    cur = list(db.execute('SELECT status, tags, rating, notes, bookmark FROM meta WHERE song_key = ?',
                          (key,)).fetchone() or ('', '', 0, '', 0))
    if value is None:  # tags / notes: ask
        name = db.execute('SELECT song_name FROM files WHERE song_key = ? LIMIT 1', (key,)).fetchone()[0]
        value = ask('FL Library', f'{field.capitalize()} for "{name}":', cur[FIELDS[field]] or '')
        if value is None:
            return
    if field == 'bookmark' and value == 'toggle':
        value = 0 if cur[FIELDS[field]] else 1
    cur[FIELDS[field]] = int(value) if field in ('rating', 'bookmark') else value.strip()
    db.execute(
        'INSERT INTO meta(song_key, status, tags, rating, notes, bookmark, updated) VALUES(?,?,?,?,?,?,?) '
        'ON CONFLICT(song_key) DO UPDATE SET status = excluded.status, tags = excluded.tags, '
        'rating = excluded.rating, notes = excluded.notes, bookmark = excluded.bookmark, updated = excluded.updated',
        (key, *cur, int(time.time())))
    db.commit()
    paths = [r[0] for r in db.execute("SELECT path FROM files WHERE song_key = ? AND kind = 'flp'", (key,))]
    db.close()
    refresh(paths)
    db = scanner.connect()
    scanner.sync_meta(db)  # push the change to the other PCs right away
    db.close()


def show_samples(path):
    """Window listing the project's samples: missing ones first, with where Everything found them."""
    import json
    import tkinter
    from tkinter import scrolledtext
    path = os.path.abspath(path)
    db = scanner.connect()
    song_for(db, path)
    row = db.execute('SELECT p.samples FROM files f JOIN projects p ON p.hash = f.hash '
                     'WHERE f.path = ? COLLATE NOCASE', (path,)).fetchone()
    samples = json.loads(row[0] or '[]') if row else []
    order = {'missing': 0, 'moved': 1, 'unchecked': 2, 'ok': 3, 'inzip': 4, 'factory': 5}
    samples.sort(key=lambda s: (order.get(s['status'], 9), s['path'].lower()))
    counts = {}
    for s in samples:
        counts[s['status']] = counts.get(s['status'], 0) + 1
    root = tkinter.Tk()
    root.title(f'Samples - {os.path.basename(path)}')
    root.geometry('980x560')
    summary = ', '.join(f'{n} {k}' for k, n in sorted(counts.items(), key=lambda kv: order.get(kv[0], 9)))
    tkinter.Label(root, text=summary or 'no samples', anchor='w', padx=8, pady=6).pack(fill='x')
    box = scrolledtext.ScrolledText(root, wrap='none', font=('Consolas', 9))
    box.pack(fill='both', expand=True)
    for tag, color in (('missing', '#c62828'), ('moved', '#b26a00'), ('factory', '#888888'), ('inzip', '#888888')):
        box.tag_config(tag, foreground=color)
    for s in samples:
        box.insert('end', f"[{s['status']:<9}] {s['path']}\n", s['status'])
        if s.get('found'):
            box.insert('end', f"{'':12}found at: {s['found']}\n", 'moved')
    box.config(state='disabled')
    root.attributes('-topmost', True)
    root.after(300, lambda: root.attributes('-topmost', False))
    root.mainloop()


def show_versions(path):
    """Open Everything listing every version / copy of this song."""
    path = os.path.abspath(path)
    db = scanner.connect()
    key = song_for(db, path)
    rows = db.execute('SELECT song_name, name, dir FROM files WHERE song_key = ?', (key,)).fetchall()
    song = rows[0][0]
    if any(song.lower() in r[1].lower() for r in rows):
        query = f'ext:flp;zip "{song}"'
    else:  # numbered files: the folder is the song
        query = f'ext:flp;zip path:"{song}"'
    exe = r'C:\Program Files\Everything\Everything.exe'
    subprocess.Popen([exe, '-s', query])


INI_MARK = '; FL Library folder colour'
PALETTE_SIZE = 10


def color_index(key):
    """Same FNV-1a hash as the panel, so folder icons match the panel's row colours."""
    h = 0x811C9DC5
    for b in key.lower().encode('utf-8'):
        h = ((h ^ b) * 0x01000193) & 0xFFFFFFFF
    return h % PALETTE_SIZE


def color_folder(folder, on):
    """Give `folder` a coloured folder icon (desktop.ini), or take ours away again."""
    ini = os.path.join(folder, 'desktop.ini')
    ours = os.path.exists(ini) and INI_MARK in open(ini, encoding='utf-8', errors='replace').read()
    if on:
        if os.path.exists(ini) and not ours:
            return False  # the folder already has its own desktop.ini; leave it alone
        icon = os.path.join(HERE, 'icons', f'folder_{color_index(os.path.basename(folder))}.ico')
        if os.path.exists(ini):
            subprocess.run(['attrib', '-H', '-S', ini], creationflags=subprocess.CREATE_NO_WINDOW)
        with open(ini, 'w', encoding='utf-8') as f:
            f.write(f'[.ShellClassInfo]\nIconResource={icon},0\n{INI_MARK}\n')
        subprocess.run(['attrib', '+H', '+S', ini], creationflags=subprocess.CREATE_NO_WINDOW)
        subprocess.run(['attrib', '+R', folder], creationflags=subprocess.CREATE_NO_WINDOW)  # makes Explorer read the ini
    elif ours:
        subprocess.run(['attrib', '-H', '-S', ini], creationflags=subprocess.CREATE_NO_WINDOW)
        os.remove(ini)
        subprocess.run(['attrib', '-R', folder], creationflags=subprocess.CREATE_NO_WINDOW)
    ctypes.windll.shell32.SHChangeNotify(0x00001000, SHCNF_PATHW, ctypes.c_wchar_p(folder), None)  # SHCNE_UPDATEDIR
    return True


def color_subfolders(root):
    """Toggle coloured icons on the project subfolders directly under `root`."""
    root = os.path.abspath(root)
    db = scanner.connect()
    prefix = root.rstrip('\\') + '\\'
    subs = sorted({r[0][len(prefix):].split('\\')[0]
                   for r in db.execute("SELECT dir FROM files WHERE is_autosave = 0 AND is_factory = 0 "
                                       "AND substr(dir, 1, ?) = ? COLLATE NOCASE", (len(prefix), prefix))
                   if len(r[0]) > len(prefix)})
    folders = [os.path.join(root, s) for s in subs if os.path.isdir(os.path.join(root, s))]
    already = any(os.path.exists(os.path.join(f, 'desktop.ini'))
                  and INI_MARK in open(os.path.join(f, 'desktop.ini'), encoding='utf-8', errors='replace').read()
                  for f in folders)
    done = sum(color_folder(f, not already) for f in folders)
    ctypes.windll.shell32.SHChangeNotify(0x00001000, SHCNF_PATHW, ctypes.c_wchar_p(root), None)
    return done, not already


PE_X64, PE_X86 = scanner.PE_X64, scanner.PE_X86
pe_machine = scanner.pe_machine

# folder names that are just "a plugins folder", not a plugin's own folder
GENERIC_PLUGIN_DIRS = {'vstplugins', 'vstplugins64', 'vst plugins', 'vst', 'vst2', 'vst3', 'plugins', 'steinberg',
                       'common files', 'program files', 'program files (x86)', 'x64', 'x86', '64', '32', '64-bit',
                       '32-bit', 'win64', 'win32', 'windows', 'vsts', 'bu', 'backup'}


def everything_hits(url, search, limit=50):
    """Full paths from an Everything HTTP server (ours or another PC's)."""
    import urllib.parse
    import urllib.request
    qs = urllib.parse.urlencode({'json': 1, 'path_column': 1, 'size_column': 1, 'count': limit, 'search': search})
    try:
        with urllib.request.urlopen(url.rstrip('/') + '/?' + qs, timeout=6) as r:
            data = json.load(r)
    except (OSError, ValueError):
        return []
    return [(x['path'] + '\\' + x['name'], int(x.get('size') or 0)) for x in data.get('results', []) if x.get('path')]


def project_plugins(db, flp):
    row = db.execute('SELECT p.plugins FROM files f JOIN projects p ON p.hash = f.hash '
                     'WHERE f.path = ? COLLATE NOCASE', (flp,)).fetchone()
    plugins = json.loads(row[0]) if row and row[0] else []
    if not plugins or not any('path' in p for p in plugins):
        info = scanner.flparse.parse_file(flp)  # not indexed yet (or indexed before paths were stored)
        plugins = info['plugins']
    return [p for p in plugins if p.get('path')]


def missing_plugins(db, flp, cfg):
    """VST plugins a project uses that are not installed on this PC.

    The path stored in the project is from whatever PC saved it, so only the file name counts: a
    plugin is present when FL's plugin database lists that file, or a copy sits in one of this PC's
    VST folders (checked live through Everything). Returns [(name, stored path, local copies
    elsewhere on this PC)]."""
    plugins = [p for p in project_plugins(db, flp) if not scanner.fl_own_plugin(p['path'])]
    if not plugins:
        return []
    dirs = scanner.vst_dirs(cfg)['all']
    known = {os.path.basename(k) for k in scanner.fl_known_plugins()}
    known |= {r[0] for r in db.execute('SELECT file FROM plugin_files')}
    known_stems = {scanner.plugin_stem(k) for k in known}
    bases = [os.path.basename(p['path']).lower() for p in plugins]
    hits = scanner.local_plugin_hits(cfg, bases + [scanner.plugin_stem(b) for b in bases])
    out = []
    for p, base in zip(plugins, bases):
        stem = scanner.plugin_stem(base)
        if base in known or stem in known_stems:
            continue
        local = hits.get(base, []) + [h for h in hits.get(stem, []) if stem != base]
        if any(scanner.in_vst_dirs(full, dirs) for full, _ in local):
            continue
        out.append((p['name'], p['path'], [full for full, _ in local]))
    return out


def plugin_sources(cfg, name, path, local_copies=()):
    """Where a copy of this plugin's file can be fetched from, best first: [(source path, where)]."""
    import re
    base = os.path.basename(path)
    _stem, ext = os.path.splitext(base)
    cands = [(full, os.path.getsize(full) if os.path.exists(full) else 0, 'this PC') for full in local_copies]
    here = cfg['everything_url']
    stem = scanner.plugin_stem(base)
    search = f'file: <wfn:"{base}"|wfn:"{stem}">' if stem != base.lower() else f'file: wfn:"{base}"'
    for full, size in everything_hits(here, search, 200):
        if full.startswith('\\\\'):
            cands.append((full, size, 'network share'))
        elif not local_copies:
            cands.append((full, size, 'this PC'))
    for peer in cfg.get('peers') or []:
        for full, size in everything_hits(peer['everything'], search, 200):
            if re.match(r'^[A-Za-z]:\\', full) and peer.get('share'):
                unc = peer['share'].replace('{drive}', full[0]) + full[2:]
                cands.append((unc, size, peer['everything']))
    want64 = ext.lower() in ('.dll', '.vst3')
    scored, seen = [], set()
    for full, size, where in cands:
        low = full.lower()
        if low in seen or '$recycle.bin' in low or '\\temp\\' in low:
            continue
        seen.add(low)
        if size and size < 32 * 1024 and '.64.' in low:
            continue  # a 32->64-bit bridge stub, not the plugin
        score = 0
        if low.endswith(os.path.dirname(path).lower().split('\\')[-1] + '\\' + base.lower()):
            score += 3  # same folder name as the project had
        machine = pe_machine(full) if want64 else None
        if machine == PE_X64:
            score += 2
        elif machine == PE_X86:
            score += 1
        if where == 'this PC':
            score += 1
        scored.append((score, full, where))
    scored.sort(key=lambda t: -t[0])
    return [(full, where) for _, full, where in scored]


def installer_candidates(cfg, name):
    """Setup programs on your drives that look like this plugin's installer."""
    import re
    stem = re.sub(r'[^a-z0-9]+', ' ', name.lower()).split()
    if not stem:
        return []
    word = max(stem, key=len)
    hits = everything_hits(cfg['everything_url'], f'file: ext:exe;msi {word} <setup|install|installer>', 20)
    return [full for full, _ in hits if not full.lower().startswith('c:\\program files')][:3]


def vst3_bundle(path):
    """The .vst3 bundle folder containing this file, if it is packaged as one."""
    d = os.path.dirname(path)
    while d and os.path.dirname(d) != d:
        if d.lower().endswith('.vst3') and os.path.isdir(d):
            return d
        d = os.path.dirname(d)
    return None


def plugin_dest(cfg, src, stored_path):
    """Where this copy belongs on this PC: FL's VST folder for its format and bitness, keeping the
    plugin's own sub-folder name when it has one (support files sit next to the dll)."""
    dirs = scanner.vst_dirs(cfg)
    ext = os.path.splitext(src)[1].lower()
    if ext == '.vst3' or vst3_bundle(src):
        root = dirs['vst3']
    elif pe_machine(src) == PE_X86:
        root = dirs['vst2_32']
    else:
        root = dirs['vst2_64']
    sub = ''
    for cand in (os.path.basename(os.path.dirname(src)), os.path.basename(os.path.dirname(stored_path))):
        if cand and cand.lower() not in GENERIC_PLUGIN_DIRS and not cand.lower().endswith('.vst3'):
            sub = cand
            break
    bundle = vst3_bundle(src)
    if bundle:
        return os.path.join(root, os.path.basename(bundle))
    return os.path.join(root, sub, os.path.basename(src)) if sub else os.path.join(root, os.path.basename(src))


def copy_plugin(src, dest):
    """Copy the plugin to its new home, plus the support files that live next to it."""
    import shutil
    bundle = vst3_bundle(src)
    if bundle:
        shutil.copytree(bundle, dest, dirs_exist_ok=True)
        return
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    shutil.copy2(src, dest)
    sdir, ddir = os.path.dirname(src), os.path.dirname(dest)
    stem = os.path.splitext(os.path.basename(src))[0].lower()
    folder_is_generic = os.path.basename(sdir).lower() in GENERIC_PLUGIN_DIRS
    for entry in os.listdir(sdir):
        s = os.path.join(sdir, entry)
        if os.path.normcase(s) == os.path.normcase(src):
            continue
        same_stem = os.path.splitext(entry)[0].lower().startswith(stem)
        if same_stem or not folder_is_generic:  # a plugin in its own folder: bring the whole folder
            d = os.path.join(ddir, entry)
            try:
                if os.path.isdir(s):
                    shutil.copytree(s, d, dirs_exist_ok=True)
                elif not os.path.exists(d):
                    shutil.copy2(s, d)
            except OSError:
                pass


def fetch_plugins(flp, elevated=False):
    """Find each plugin this project uses that is not installed here, on this PC's other drives,
    network shares and the other PCs, and copy it into this PC's VST folder. Re-runs itself
    elevated when that folder is under Program Files."""
    flp = os.path.abspath(flp)
    cfg = scanner.load_config()
    db = scanner.connect()
    missing = missing_plugins(db, flp, cfg)
    if not missing:
        db.close()
        ctypes.windll.user32.MessageBoxW(None, 'All plugins this project uses are installed on this PC.', 'FL Library', 0x40)
        return
    plan = []
    for name, path, local in missing:
        sources = plugin_sources(cfg, name, path, local)
        dest = plugin_dest(cfg, sources[0][0], path) if sources else None
        plan.append((name, path, sources, dest))
    needs_admin = any(dest and not os.access(nearest_existing_dir(dest), os.W_OK) for _n, _p, _s, dest in plan)
    if needs_admin and not elevated:
        db.close()
        params = f'"{os.path.abspath(__file__)}" fetchplugins-elevated "{flp}"'
        if ctypes.windll.shell32.ShellExecuteW(None, 'runas', sys.executable, params, None, 1) > 32:
            return
        ctypes.windll.user32.MessageBoxW(None, 'Admin permission is needed to copy into Program Files.', 'FL Library', 0x30)
        return
    report = []
    for name, path, sources, dest in plan:
        done = False
        errors = []
        for src, where in sources:
            dest = plugin_dest(cfg, src, path)
            try:
                copy_plugin(src, dest)
                db.execute('INSERT OR REPLACE INTO plugin_files VALUES(?,?,?)',
                           (os.path.basename(dest).lower(), dest, pe_machine(dest)))
                db.commit()
                lic = '\n      (licensed plugin: a copied file may not run, use its installer)' if licensed_note(src) else ''
                report.append(f'OK  {name}\n      -> {dest}\n      from {src}{lic}')
                done = True
                break
            except OSError as e:
                errors.append((src, e))
        if not done:
            inst = installer_candidates(cfg, name)
            if sources:
                offline = sorted({s.split('\\')[2] for s, e in errors if s.startswith('\\\\') and e.errno in (2, 53, 64, 67, 1231)})
                if offline and len(offline) == len({s.split('\\')[2] for s, _ in errors if s.startswith('\\\\')}) \
                        and all(s.startswith('\\\\') for s, _ in errors):
                    report.append(f"XX  {name}: copies exist on {', '.join(offline)} but it is offline right now")
                else:
                    src, e = errors[0]
                    report.append(f'XX  {name}: could not copy from {src} ({e})')
            elif inst:
                report.append(f'??  {name}: no copy found; installer candidates:\n      ' + '\n      '.join(inst))
            else:
                report.append(f'XX  {name}: not found on any drive or PC  ({os.path.basename(path)})')
    db.close()
    ok = sum(r.startswith('OK') for r in report)
    text = f"{ok} of {len(plan)} missing plugin(s) copied into this PC's VST folders.\n\n" + '\n'.join(report)
    if ok:
        text += ('\n\nFL finds plugins by file name in its VST folders, so the project should load now. '
                 'To see them in the browser too: Options > Manage plugins > Find more plugins.'
                 '\nPlugins that came with an installer (licensed / with data folders) may still need to be installed properly.')
    ctypes.windll.user32.MessageBoxW(None, text, 'FL Library - fetch plugins', 0x40 if ok == len(plan) else 0x30)


LICENSED_RE = None


def licensed_note(path):
    """Plugins that will not run from a copied file: they need their installer / licence."""
    import re
    global LICENSED_RE
    if LICENSED_RE is None:
        LICENSED_RE = re.compile(r'waveshell|\\waves\\|\\uad\b|uad |universal audio|native instruments|kontakt|massive|'
                                 r'reaktor|guitar rig|izotope|ozone|nectar|spectrasonics|omnisphere|trilian|keyscape|'
                                 r'stylus|atmosphere|fabfilter|slate|soundtoys|arturia|ilok|pace\b|antares|auto-tune|'
                                 r'melodyne|celemony|xln|addictive|toontrack|superior drummer|ezdrummer|serum|xfer', re.I)
    return bool(LICENSED_RE.search(path))


FL_LEGACY_URL = 'https://forum.image-line.com/viewtopic.php?t=194258'  # "FL Studio Installer Archive" (login)
FL_VER_RE = None


def installer_version(path):
    """Version number an FL installer file (or its folder) is named with, as a tuple, or None."""
    import re
    global FL_VER_RE
    if FL_VER_RE is None:
        FL_VER_RE = re.compile(r'(?:flstudio|fl[ _]?studio|fruity ?loops)[^0-9]{0,40}?(\d+(?:[._]\d+){0,3})', re.I)
    for text in (os.path.basename(path), os.path.basename(os.path.dirname(path))):
        text = re.sub(r'win64|win32|x64|x86|64bit|32bit', '', text, flags=re.I)
        m = FL_VER_RE.search(text)
        if m:
            parts = tuple(int(x) for x in re.split(r'[._]', m.group(1)))
            if 1 <= parts[0] <= 99:
                return parts
    return None


def fl_installers(cfg, version):
    """FL Studio installers on your drives, shares and other PCs, best match for `version` first:
    [(path, version tuple, where)]."""
    want = tuple(int(x) for x in version.split('.') if x.isdigit())
    if not want:
        return []
    search = 'file: ext:exe;msi;zip;rar;7z;iso size:>20mb <flstudio|"fl studio"|fl_studio|fruityloops|"fruity loops">'
    cands = []
    for full, size in everything_hits(cfg['everything_url'], search, 400):
        cands.append((full, size, 'network share' if full.startswith('\\\\') else 'this PC'))
    for peer in cfg.get('peers') or []:
        for full, size in everything_hits(peer['everything'], search, 400):
            if full[1:3] == ':\\' and peer.get('share'):
                cands.append((peer['share'].replace('{drive}', full[0]) + full[2:], size, peer['everything']))
    scored, seen = [], set()
    for full, size, where in cands:
        ver = installer_version(full)
        low = full.lower()
        if not ver or ver[0] != want[0] or low in seen or '$recycle.bin' in low:
            continue
        seen.add(low)
        match = 0
        for a, b in zip(ver, want):
            if a != b:
                break
            match += 1
        score = (match, ver[:2] <= want[:2], low.endswith('.exe'), where == 'this PC', ver)
        scored.append((score, full, ver, where))
    scored.sort(key=lambda t: t[0], reverse=True)
    return [(full, ver, where) for _, full, ver, where in scored]


def get_fl(flp):
    """Find the installer of the FL version a project was made with and show it, or point at
    Image-Line's legacy downloads."""
    flp = os.path.abspath(flp)
    cfg = scanner.load_config()
    db = scanner.connect()
    row = db.execute('SELECT p.version FROM files f JOIN projects p ON p.hash = f.hash '
                     'WHERE f.path = ? COLLATE NOCASE', (flp,)).fetchone()
    db.close()
    version = (row[0] if row else '') or ''
    if not version:
        version = scanner.flparse.parse_file(flp).get('version') or ''
    if not version:
        ctypes.windll.user32.MessageBoxW(None, 'This project does not say which FL version made it.', 'FL Library', 0x30)
        return
    found = fl_installers(cfg, version)
    if found:
        lines = [f"{'.'.join(map(str, v))}   {p}" + ('' if w == 'this PC' else f'   ({w})') for p, v, w in found[:8]]
        text = (f'Project made with FL Studio {version}.\n\nInstallers found (closest first):\n\n' + '\n'.join(lines)
                + '\n\nShow the first one in Explorer?')
        if ctypes.windll.user32.MessageBoxW(None, text, 'FL Library - get FL ' + version, 0x24) == 6:
            subprocess.Popen(['explorer.exe', '/select,' + found[0][0]])
        return
    text = (f'Project made with FL Studio {version}.\n\nNo FL {version.split(".")[0]} installer found on your drives, '
            'shares or the other PCs.\n\nOpen Image-Line\'s "FL Studio Installer Archive" forum thread '
            '(versions 1-12 and samples; needs your Image-Line login)?')
    if ctypes.windll.user32.MessageBoxW(None, text, 'FL Library - get FL ' + version, 0x24) == 6:
        os.startfile(FL_LEGACY_URL)


def nearest_existing_dir(path):
    d = os.path.dirname(path)
    while d and not os.path.isdir(d):
        parent = os.path.dirname(d)
        if parent == d:
            break
        d = parent
    return d or '.'


def main(argv):
    cmd = argv[0]
    if cmd == 'colorfolders':
        color_subfolders(argv[1])
        return
    if cmd in ('fetchplugins', 'fetchplugins-elevated'):
        fetch_plugins(argv[1], elevated=cmd.endswith('elevated'))
        return
    if cmd == 'getfl':
        get_fl(argv[1])
        return
    if cmd == 'samples':
        show_samples(argv[1])
    elif cmd == 'versions':
        show_versions(argv[1])
    elif cmd == 'sync':  # scheduled every few minutes: pull other PCs' changes
        db = scanner.connect()
        scanner.sync_meta(db)
        db.close()
    elif cmd == 'rescan':
        subprocess.Popen([sys.executable, os.path.join(HERE, 'scanner.py')], cwd=HERE,
                         creationflags=subprocess.CREATE_NO_WINDOW)
    elif cmd == 'bookmark':  # bookmark toggle|1|0 <path>
        set_field('bookmark', argv[1], argv[2])
    elif cmd in ('status', 'rating'):
        set_field(cmd, '' if argv[1] == 'none' else argv[1], argv[2])
    elif cmd in ('tags', 'notes'):
        set_field(cmd, None, argv[1])
    elif cmd == 'setmeta':  # from the Explorer panel: path, tags, notes
        set_field('tags', argv[2], argv[1])
        set_field('notes', argv[3], argv[1])


if __name__ == '__main__':
    try:
        main(sys.argv[1:])
    except Exception as e:  # noqa: BLE001 - no console under pythonw, so show it
        ctypes.windll.user32.MessageBoxW(None, f'{type(e).__name__}: {e}', 'FL Library', 0x10)
