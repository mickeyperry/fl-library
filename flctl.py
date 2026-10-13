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
        # not indexed yet, or indexed before plugin paths were stored: parse it now and store the
        # result so the panel shows the check from here on
        try:
            scanner.index_one(db, flp)
            row = db.execute('SELECT p.plugins FROM files f JOIN projects p ON p.hash = f.hash '
                             'WHERE f.path = ? COLLATE NOCASE', (flp,)).fetchone()
            plugins = json.loads(row[0]) if row and row[0] else []
        except Exception:
            plugins = scanner.flparse.parse_file(flp)['plugins']
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
        if '\\program files' in low and '\\bu\\' not in low and 'backup' not in low and '\\old ' not in low:
            score += 2  # a real install, not a backup copy
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


def copy_batch(cfg, db, batch, emit):
    """Copy each plugin from the best readable source; one event per step. batch: [(name, stored
    path, [(src, where)])]. Returns (ok count, report lines)."""
    report, ok = [], 0
    for name, path, sources in batch:
        done, errors = False, []
        for src, _where in sources:
            dest = plugin_dest(cfg, src, path)
            emit(plugin=name, status='copying', detail=f'{src}  ->  {dest}')
            try:
                copy_plugin(src, dest)
                db.execute('INSERT OR REPLACE INTO plugin_files VALUES(?,?,?)',
                           (os.path.basename(dest).lower(), dest, pe_machine(dest)))
                db.commit()
                lic = licensed_note(src)
                emit(plugin=name, status='licensed' if lic else 'ok', detail=f'{dest}   (from {src})')
                report.append(f'OK  {name}\n      -> {dest}\n      from {src}'
                              + ('\n      (licensed plugin: a copied file may not run, use its installer)' if lic else ''))
                ok += 1
                done = True
                break
            except OSError as e:
                errors.append((src, e))
        if not done:
            unc = [(s, e) for s, e in errors if s.startswith('\\\\')]
            offline = sorted({s.split('\\')[2] for s, e in unc if e.errno in (2, 53, 64, 67, 1231)})
            if errors and len(unc) == len(errors) and offline:
                emit(plugin=name, status='offline', detail=f"copies exist on {', '.join(offline)} but it is offline right now")
                report.append(f"XX  {name}: copies exist on {', '.join(offline)} but it is offline right now")
            else:
                src, e = errors[0] if errors else ('?', 'no readable source')
                emit(plugin=name, status='error', detail=f'could not copy from {src}: {e}')
                report.append(f'XX  {name}: could not copy from {src} ({e})')
    return ok, report


def fetch_plugins(flp, json_out=False):
    """Find each plugin this project uses that is not installed here, on this PC's other drives,
    network shares and the other PCs, and copy it into this PC's VST folder.

    With json_out, progress goes to stdout as one JSON object per line (the panel's fetch window
    shows it live); otherwise a message box sums it up at the end. Copying into Program Files is
    done by a second, elevated copy of this script (one UAC prompt) that reports back via a file."""
    import tempfile

    def emit(**ev):
        if json_out:
            sys.stdout.write(json.dumps(ev) + '\n')
            sys.stdout.flush()

    def say(text, flags=0x40):
        if not json_out:
            ctypes.windll.user32.MessageBoxW(None, text, 'FL Library - fetch plugins', flags)

    flp = os.path.abspath(flp)
    cfg = scanner.load_config()
    db = scanner.connect()
    try:
        missing = missing_plugins(db, flp, cfg)
    except Exception as e:
        emit(plugin='(check)', status='error', detail=str(e))
        emit(summary=f'Could not check the plugins: {e}')
        say(f'Could not check the plugins:\n{e}', 0x10)
        db.close()
        return
    if not missing:
        emit(none=True)
        say('All plugins this project uses are installed on this PC.')
        db.close()
        return
    emit(plugins=[name for name, _p, _l in missing])
    plan = []
    for name, path, local in missing:
        emit(plugin=name, status='searching', detail=os.path.basename(path))
        sources = plugin_sources(cfg, name, path, local)
        if sources:
            emit(plugin=name, status='found', detail=f'{len(sources)} cop{"y" if len(sources) == 1 else "ies"}, best: {sources[0][0]}',
                 sources=[s for s, _w in sources[:20]])
            plan.append((name, path, sources))
        else:
            inst = installer_candidates(cfg, name)
            if inst:
                emit(plugin=name, status='installer', detail='no copy on any drive or PC; installer: ' + inst[0])
            else:
                emit(plugin=name, status='notfound', detail=f'no copy of {os.path.basename(path)} on any drive or PC')
    report = []
    for name, path, _l in missing:
        if not any(n == name for n, _p, _s in plan):
            report.append(f'XX  {name}: no copy found on any drive or PC')
    ok, lines = copy_plan(cfg, db, plan, emit)
    report += lines
    db.close()
    n = len(missing)
    summary = f"{ok} of {n} missing plugin(s) copied into this PC's VST folders."
    if ok:
        summary += ' Now run Options > Manage plugins > Find more plugins in FL once, then reopen the project.'
    if n - ok:
        summary += ' Right-click a row to copy from a different source.'
    emit(summary=summary, ok=ok, total=n)
    text = summary + '\n\n' + '\n'.join(report)
    if ok:
        text += '\n\nPlugins that came with an installer (licensed / with data folders) may still need to be installed properly.'
    say(text, 0x40 if ok == n else 0x30)


def can_write(directory):
    """Really try: os.access() says yes for Program Files although UAC says no."""
    import tempfile
    try:
        with tempfile.TemporaryFile(dir=nearest_existing_dir(os.path.join(directory, 'x'))):
            return True
    except OSError:
        return False


def copy_plan(cfg, db, plan, emit):
    """Copy every plugin in the plan, in this process when the VST folders are writable, else in
    one elevated copy of this script (a single UAC prompt) that reports back through a file.
    Returns (copied count, report lines)."""
    import tempfile
    if not plan:
        return 0, []
    is_admin = bool(ctypes.windll.shell32.IsUserAnAdmin())
    needs_admin = not is_admin and any(not can_write(os.path.dirname(plugin_dest(cfg, sources[0][0], path)))
                                       for _n, path, sources in plan)
    if not needs_admin:
        return copy_batch(cfg, db, plan, emit)
    ok, report = 0, []
    for name, _p, _s in plan:
        emit(plugin=name, status='admin', detail='copying into Program Files needs admin permission')
    fd, plan_file = tempfile.mkstemp(prefix='fl-library-fetch-', suffix='.json')
    with os.fdopen(fd, 'w', encoding='utf-8') as f:
        json.dump(plan, f)
    out_file = plan_file + '.out'
    params = f'"{os.path.abspath(__file__)}" fetchplugins-copy "{plan_file}"'
    if run_elevated_and_wait(sys.executable, params):
        try:
            for line in open(out_file, encoding='utf-8'):
                ev = json.loads(line)
                if 'plugin' in ev:
                    emit(**ev)
                    if ev['status'] in ('ok', 'licensed'):
                        ok += 1
                elif 'report' in ev:
                    report.append(ev['report'])
        except OSError:
            for name, _p, _s in plan:
                emit(plugin=name, status='error', detail='the elevated copy step did not report back')
    else:
        for name, _p, _s in plan:
            emit(plugin=name, status='error', detail='admin permission was not given')
        report.append('XX  copying into Program Files needs admin permission')
    for f in (plan_file, out_file):
        try:
            os.remove(f)
        except OSError:
            pass
    return ok, report


def fetch_one(flp, name, src, json_out=True):
    """Copy one plugin of the project from the source the user picked (fetch window, right-click)."""
    def emit(**ev):
        if json_out:
            sys.stdout.write(json.dumps(ev) + '\n')
            sys.stdout.flush()

    cfg = scanner.load_config()
    db = scanner.connect()
    path = next((p['path'] for p in project_plugins(db, os.path.abspath(flp)) if p['name'] == name), None)
    if not path:
        emit(plugin=name, status='error', detail='plugin not found in the project')
        db.close()
        return
    if not os.path.exists(src):
        emit(plugin=name, status='offline' if src.startswith('\\\\') else 'error',
             detail=f'{src} is not reachable right now')
        db.close()
        return
    ok, _report = copy_plan(cfg, db, [(name, path, [(src, 'chosen')])], emit)
    db.close()
    emit(summary=('Copied. Now run Options > Manage plugins > Find more plugins in FL once, then reopen the project.'
                  if ok else 'Not copied. Right-click the row to try another source.'), ok=ok, total=1)


def fetch_copy_elevated(plan_file):
    """The elevated half of fetch_plugins: copy everything in the plan, report to <plan>.out."""
    plan = json.load(open(plan_file, encoding='utf-8'))
    cfg = scanner.load_config()
    db = scanner.connect()
    with open(plan_file + '.out', 'w', encoding='utf-8') as out:
        def emit(**ev):
            out.write(json.dumps(ev) + '\n')
            out.flush()
        _ok, lines = copy_batch(cfg, db, [(n, p, [tuple(s) for s in srcs]) for n, p, srcs in plan], emit)
        for line in lines:
            emit(report=line)
    db.close()


def run_elevated_and_wait(exe, params):
    """Run exe elevated (UAC prompt) and wait for it. False if the prompt was refused."""
    import ctypes.wintypes as wt

    class SHELLEXECUTEINFOW(ctypes.Structure):
        _fields_ = [('cbSize', wt.DWORD), ('fMask', wt.ULONG), ('hwnd', wt.HWND), ('lpVerb', wt.LPCWSTR),
                    ('lpFile', wt.LPCWSTR), ('lpParameters', wt.LPCWSTR), ('lpDirectory', wt.LPCWSTR),
                    ('nShow', ctypes.c_int), ('hInstApp', wt.HINSTANCE), ('lpIDList', ctypes.c_void_p),
                    ('lpClass', wt.LPCWSTR), ('hkeyClass', wt.HKEY), ('dwHotKey', wt.DWORD),
                    ('hIcon', wt.HANDLE), ('hProcess', wt.HANDLE)]
    info = SHELLEXECUTEINFOW()
    info.cbSize = ctypes.sizeof(info)
    info.fMask = 0x40  # SEE_MASK_NOCLOSEPROCESS
    info.lpVerb = 'runas'
    info.lpFile = exe
    info.lpParameters = params
    info.nShow = 0
    if not ctypes.windll.shell32.ShellExecuteExW(ctypes.byref(info)) or not info.hProcess:
        return False
    ctypes.windll.kernel32.WaitForSingleObject(info.hProcess, 0xFFFFFFFF)
    ctypes.windll.kernel32.CloseHandle(info.hProcess)
    return True


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
    if cmd == 'fetchplugins':  # fetchplugins <flp> [--json]
        fetch_plugins(argv[1], json_out='--json' in argv[2:])
        return
    if cmd == 'fetchplugins-copy':  # the elevated copy step
        fetch_copy_elevated(argv[1])
        return
    if cmd == 'fetchplugins-one':  # fetchplugins-one <flp> <plugin name> <source path> [--json]
        fetch_one(argv[1], argv[2], argv[3], json_out='--json' in argv[4:])
        return
    if cmd == 'getfl':
        get_fl(argv[1])
        return
    if cmd == 'update':  # update [check]
        update(check_only=len(argv) > 1 and argv[1] == 'check')
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
