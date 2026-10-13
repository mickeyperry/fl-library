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


PE_X64, PE_X86 = 0x8664, 0x14C


def pe_machine(path):
    """0x8664 (64-bit) / 0x14c (32-bit) / None."""
    try:
        with open(path, 'rb') as f:
            head = f.read(4096)
        pe = int.from_bytes(head[0x3C:0x40], 'little')
        return int.from_bytes(head[pe + 4:pe + 6], 'little') if head[pe:pe + 4] == b'PE\0\0' else None
    except (OSError, IndexError, ValueError):
        return None


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


def fl_known_plugins():
    """Plugin DLL paths FL's own plugin database lists as installed on this PC."""
    paths = set()
    base = os.path.join(os.path.expanduser('~'), 'Documents', 'Image-Line', 'FL Studio', 'Presets', 'Plugin database', 'Installed')
    for root, _dirs, files in os.walk(base):
        for f in files:
            if f.lower().endswith('.nfo'):
                try:
                    for line in open(os.path.join(root, f), encoding='utf-8', errors='replace'):
                        if line.startswith('ps_file_filename_'):
                            paths.add(line.split('=', 1)[1].strip().lower())
                except OSError:
                    pass
    return paths


def missing_plugins(db, flp):
    """VST plugins a project refers to whose DLL is not on this PC: [(name, stored path)]."""
    row = db.execute('SELECT p.plugins FROM files f JOIN projects p ON p.hash = f.hash '
                     'WHERE f.path = ? COLLATE NOCASE', (flp,)).fetchone()
    plugins = json.loads(row[0]) if row and row[0] else []
    if not plugins or not any('path' in p for p in plugins):
        info = scanner.flparse.parse_file(flp)  # not indexed yet (or indexed before paths were stored)
        plugins = info['plugins']
    known = None
    out = []
    for p in plugins:
        path = p.get('path')
        if not path or path.startswith('\\\\'):
            continue
        if os.path.exists(path):
            continue
        if known is None:
            known = fl_known_plugins()
            known_names = {os.path.basename(k) for k in known}
        # FL finds a plugin by name after its own scan, so the same file in another plugin folder counts
        if path.lower() in known or os.path.basename(path.lower()) in known_names:
            continue
        out.append((p['name'], path))
    return out


def plugin_sources(cfg, name, path):
    """Where a copy of this plugin's file can be fetched from, best first: [(source path, note)]."""
    import re
    base = os.path.basename(path)
    stem, ext = os.path.splitext(base)
    cands = []
    here = cfg['everything_url']
    for full, size in everything_hits(here, f'file: whole:"{base}"'):
        cands.append((full, size, 'network share' if full.startswith('\\\\') else 'this PC'))
    for peer in cfg.get('peers') or []:
        for full, size in everything_hits(peer['everything'], f'file: whole:"{base}"'):
            if re.match(r'^[A-Za-z]:\\', full) and peer.get('share'):
                unc = peer['share'].replace('{drive}', full[0]) + full[2:]
                cands.append((unc, size, peer['everything']))
    want64 = ext.lower() in ('.dll', '.vst3')
    scored = []
    for full, size, where in cands:
        low = full.lower()
        if low == path.lower() or '$recycle.bin' in low or '\\temp\\' in low:
            continue
        if size and size < 32 * 1024 and '.64.' in low:
            continue  # a 32->64-bit bridge stub, not the plugin
        score = 0
        if low.endswith(os.path.dirname(path).lower().split('\\')[-1] + '\\' + base.lower()):
            score += 3  # same folder name as the project expects
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


def copy_plugin(src, dest):
    """Copy the plugin file to where the project expects it, plus the support files next to it."""
    import shutil
    os.makedirs(os.path.dirname(dest), exist_ok=True)
    shutil.copy2(src, dest)
    sdir, ddir = os.path.dirname(src), os.path.dirname(dest)
    stem = os.path.splitext(os.path.basename(src))[0].lower()
    folder_is_plugins = os.path.basename(sdir).lower() in ('vstplugins', 'vstplugins64', 'vst', 'vst2', 'vst3', 'plugins', 'steinberg')
    for entry in os.listdir(sdir):
        s = os.path.join(sdir, entry)
        if os.path.normcase(s) == os.path.normcase(src):
            continue
        same_stem = os.path.splitext(entry)[0].lower().startswith(stem)
        if same_stem or not folder_is_plugins:  # a plugin in its own folder: bring the whole folder
            d = os.path.join(ddir, entry)
            try:
                if os.path.isdir(s):
                    shutil.copytree(s, d, dirs_exist_ok=True)
                elif not os.path.exists(d):
                    shutil.copy2(s, d)
            except OSError:
                pass


def fetch_plugins(flp, elevated=False):
    """Find each missing plugin on your other PCs / drives and copy it to the path the project
    expects, so FL loads it with no rescan. Re-runs itself elevated when Program Files is involved."""
    flp = os.path.abspath(flp)
    cfg = scanner.load_config()
    db = scanner.connect()
    missing = missing_plugins(db, flp)
    db.close()
    if not missing:
        ctypes.windll.user32.MessageBoxW(None, 'All plugins this project uses are on this PC.', 'FL Library', 0x40)
        return
    plan = []
    for name, path in missing:
        sources = plugin_sources(cfg, name, path)
        plan.append((name, path, sources))
    needs_admin = any(not os.access(nearest_existing_dir(path), os.W_OK) for _n, path, srcs in plan if srcs)
    if needs_admin and not elevated:
        params = f'"{os.path.abspath(__file__)}" fetchplugins-elevated "{flp}"'
        if ctypes.windll.shell32.ShellExecuteW(None, 'runas', sys.executable, params, None, 1) > 32:
            return
        ctypes.windll.user32.MessageBoxW(None, 'Admin permission is needed to copy into Program Files.', 'FL Library', 0x30)
        return
    report = []
    for name, path, sources in plan:
        done = False
        for src, where in sources:
            try:
                copy_plugin(src, path)
                report.append(f'OK  {name}\n      from {src}')
                done = True
                break
            except OSError as e:
                report.append(f'--  {name}: could not copy from {src} ({e})')
        if not done:
            inst = installer_candidates(cfg, name)
            if inst:
                report.append(f'??  {name}: no copy found; installer candidates:\n      ' + '\n      '.join(inst))
            else:
                report.append(f'XX  {name}: not found on any drive or PC\n      expected at {path}')
    ok = sum(r.startswith('OK') for r in report)
    text = f'{ok} of {len(plan)} missing plugin(s) copied.\n\n' + '\n'.join(report)
    text += '\n\nPlugins that came with an installer (licensed / with data folders) may still need to be installed properly.'
    ctypes.windll.user32.MessageBoxW(None, text, 'FL Library - fetch plugins', 0x40 if ok == len(plan) else 0x30)


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
