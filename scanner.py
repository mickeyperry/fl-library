"""Index every .flp / FL .zip found by Everything into library.db. Read-only on the projects."""
import glob
import hashlib
import json
import mmap
import ntpath
import os
import re
import sqlite3
import sys
import threading
import time
import urllib.parse
import urllib.request
import zipfile
from concurrent.futures import FIRST_COMPLETED, ProcessPoolExecutor, ThreadPoolExecutor, wait

import flparse

HERE = os.path.dirname(os.path.abspath(__file__))
# Installed by FL-Library-Setup.exe (Program Files is read-only for normal programs): keep data
# in %LOCALAPPDATA%\FL Library. Running from a git checkout: keep it next to the scripts.
if os.path.exists(os.path.join(HERE, 'installed.flag')):
    DATA_DIR = os.path.join(os.environ.get('LOCALAPPDATA') or os.path.expanduser('~'), 'FL Library')
    os.makedirs(DATA_DIR, exist_ok=True)
else:
    DATA_DIR = HERE
DB_PATH = os.path.join(DATA_DIR, 'library.db')
CONFIG_PATH = os.path.join(DATA_DIR, 'config.json')

DEFAULT_CONFIG = {
    'everything_url': 'http://127.0.0.1:8666/',
    # network mirrors of this PC's own drives - same files, different root
    'exclude_prefixes': [],  # e.g. ["\\\\nas\\mirror\\"] for shares that duplicate local drives
    'exclude_contains': ['\\$RECYCLE.BIN\\'],
    'port': 8777,
    'fl_exe': None,
}

AUDIO_EXTS = ('wav', 'mp3', 'ogg', 'flac', 'aif', 'aiff', 'wv', 'm4a')
RENDER_EXTS = ('.mp3', '.wav', '.flac', '.ogg', '.m4a')
MAX_ZIP_MEMBER = 300 * 1024 * 1024
MAX_FLPS_PER_ZIP = 50

SCHEMA = """
CREATE TABLE IF NOT EXISTS files(
  path TEXT PRIMARY KEY, container TEXT, kind TEXT, name TEXT, dir TEXT,
  size INTEGER, mtime INTEGER, hash TEXT, song_key TEXT, song_name TEXT,
  is_factory INTEGER, is_autosave INTEGER, render TEXT);
CREATE INDEX IF NOT EXISTS files_song ON files(song_key);
CREATE INDEX IF NOT EXISTS files_hash ON files(hash);
CREATE INDEX IF NOT EXISTS files_path_nc ON files(path COLLATE NOCASE);
CREATE TABLE IF NOT EXISTS projects(
  hash TEXT PRIMARY KEY, version TEXT, major INTEGER, bpm REAL, title TEXT, genre TEXT,
  author TEXT, comment TEXT, channels INTEGER, patterns INTEGER, created TEXT, hours REAL,
  plugins TEXT, samples TEXT, n_missing INTEGER, n_moved INTEGER, error TEXT);
CREATE TABLE IF NOT EXISTS zipcache(path TEXT PRIMARY KEY, size INTEGER, mtime INTEGER, members TEXT);
CREATE TABLE IF NOT EXISTS meta(
  song_key TEXT PRIMARY KEY, status TEXT, tags TEXT, rating INTEGER, notes TEXT, updated INTEGER);
CREATE TABLE IF NOT EXISTS state(k TEXT PRIMARY KEY, v TEXT);
"""

AUTOSAVE_RE = re.compile(r'\s*\((autosaved|overwritten)[^)]*\)\s*$', re.I)
VERSION_SUFFIX_RE = re.compile(r'([ _\-.]*(\(\d+\)|(v|ver|version)?[ _\-.]*\d+))+$', re.I)
GENERIC_DIRS = {'backup', 'flp', 'flps', 'projects', 'project', 'project data', 'fl studio', 'fl'}
# FL's own content. Anchored on an "Image-Line" or "FL Studio <ver>" folder, so backup copies of the
# program folder (…\program files\FL Studio 12\Data\…) count too; your own Data\Projects\<song> stay.
FACTORY_RE = re.compile(
    r'(\\image-line[^\\]*\\(.*\\)?|\\fl studio[^\\]*\\)'
    r'(data\\(patches|templates|system|demo projects)|plugins|'
    r'data\\projects\\(cool stuff|demo songs|demo projects|tutorial|tutorials|templates|newstuff|soundfonts|flm|mobile'
    r'|visual|visualizer|performance mode|remix performance|performance|song contests|product demos))\\'
    r'|\\il groove loops\\',
    re.I)


def reflag(db):
    """Re-apply the demo/factory rules (path, plus Image-Line as the author) to every indexed file."""
    for path, old in db.execute('SELECT path, is_factory FROM files').fetchall():
        new = int(bool(FACTORY_RE.search(path)))
        if new != old:
            db.execute('UPDATE files SET is_factory = ? WHERE path = ?', (new, path))
    db.execute("UPDATE files SET is_factory = 1 WHERE hash IN "
               "(SELECT hash FROM projects WHERE author LIKE 'FL%Studio%' OR author = 'Image-Line')")
    db.commit()
FACTORY_SAMPLE_RE = re.compile(r'\\image-line\\fl studio[^\\]*\\data\\patches\\', re.I)


def load_config():
    cfg = dict(DEFAULT_CONFIG)
    if os.path.exists(CONFIG_PATH):
        with open(CONFIG_PATH, encoding='utf-8') as f:
            cfg.update(json.load(f))
    else:
        with open(CONFIG_PATH, 'w', encoding='utf-8') as f:
            json.dump(cfg, f, indent=2)
    return cfg


def connect():
    db = sqlite3.connect(DB_PATH, timeout=30)
    db.execute('PRAGMA journal_mode=WAL')
    db.executescript(SCHEMA)
    if 'bookmark' not in [r[1] for r in db.execute('PRAGMA table_info(meta)')]:
        db.execute('ALTER TABLE meta ADD COLUMN bookmark INTEGER DEFAULT 0')
    db.execute('CREATE INDEX IF NOT EXISTS files_container_nc ON files(container COLLATE NOCASE)')
    return db


def find_fl_exe(cfg):
    if cfg.get('fl_exe') and os.path.exists(cfg['fl_exe']):
        return cfg['fl_exe']
    hits = glob.glob(r'C:\Program Files\Image-Line\FL Studio*\FL64.exe')
    return max(hits, key=os.path.getmtime) if hits else None


# ---------- Everything ----------

def ev_query(cfg, search, page=100000):
    offset = 0
    while True:
        qs = urllib.parse.urlencode({
            'json': 1, 'path_column': 1, 'size_column': 1, 'date_modified_column': 1,
            'count': page, 'offset': offset, 'search': search})
        with urllib.request.urlopen(cfg['everything_url'] + '?' + qs, timeout=300) as r:
            d = json.load(r)
        res = d.get('results', [])
        yield from res
        offset += len(res)
        if not res or offset >= d.get('totalResults', 0):
            break


def filetime_to_unix(ft):
    try:
        return max(0, (int(ft) - 116444736000000000) // 10_000_000)
    except (TypeError, ValueError):
        return 0


def root_of(path):
    if path.startswith('\\\\'):
        return '\\'.join(path.split('\\')[:4])
    return path[:2]


def make_filter(cfg):
    prefixes = tuple(p.lower() for p in cfg['exclude_prefixes'])
    contains = [c.lower() for c in cfg['exclude_contains']]

    def wanted(path):
        low = path.lower()
        return not low.startswith(prefixes) and not any(c in low for c in contains)
    return wanted


def reachable(root, timeout=6):
    ok = []
    t = threading.Thread(target=lambda: ok.append(os.path.isdir(root + '\\')), daemon=True)
    t.start()
    t.join(timeout)
    return bool(ok and ok[0])


def candidates(cfg, search, wanted):
    out = {}
    for x in ev_query(cfg, search):
        if x.get('type') != 'file' or not x.get('path'):
            continue
        p = x['path'] + '\\' + x['name']
        if wanted(p):
            out[p] = (int(x.get('size') or 0), filetime_to_unix(x.get('date_modified')))
    return out


class AudioIndex:
    def __init__(self, cfg, wanted, progress):
        self.paths = set()
        self.names = {}
        n = 0
        for x in ev_query(cfg, 'file: ext:' + ';'.join(AUDIO_EXTS)):
            if not x.get('path'):
                continue
            p = x['path'] + '\\' + x['name']
            if not wanted(p):
                continue
            self.paths.add(p.lower())
            hits = self.names.setdefault(x['name'].lower(), [])
            if not p.startswith('\\\\'):
                hits.insert(0, p)  # local copies first, so they survive the cap
                del hits[8:]
            elif len(hits) < 8:
                hits.append(p)
            n += 1
            if n % 200000 == 0:
                progress(f'audio index: {n:,} files')

    def best(self, name, wanted_path):
        hits = self.names.get(name)
        if not hits:
            return None
        want = wanted_path.lower().split('\\')[::-1]

        def score(h):
            parts = h.lower().split('\\')[::-1]
            s = 0
            for a, b in zip(parts, want):
                if a != b:
                    break
                s += 1
            return s
        return max(hits, key=lambda h: (score(h), not h.startswith('\\\\')))


# ---------- naming ----------

def song_identity(directory, name):
    stem = name[:-4] if name.lower().endswith('.flp') else name
    autosave = bool(AUTOSAVE_RE.search(stem)) or ntpath.basename(directory).lower() == 'backup'
    stem = AUTOSAVE_RE.sub('', stem)
    base = VERSION_SUFFIX_RE.sub('', stem).strip(' _-.')
    parts = [p for p in directory.split('\\') if p]
    while len(parts) > 1 and parts[-1].lower() in GENERIC_DIRS:
        parts.pop()
    folder = parts[-1] if parts else ''
    key = (folder + '/' + base).lower()
    return key, (base or folder or stem), autosave


def expand_sample(sp):
    low = sp.lower()
    if low.startswith('%flstudiouserdata%'):
        return os.path.join(os.path.expanduser('~'), 'Documents', 'Image-Line') + sp[len('%flstudiouserdata%'):]
    return os.path.expandvars(sp)


def resolve_samples(samples, audio, zip_names):
    out, missing, moved = [], 0, 0
    if audio is None:  # single-file index from the Explorer menu; the next scan resolves them
        return [{'path': sp, 'status': 'unchecked', 'found': None} for sp in samples], 0, 0
    for sp in samples:
        low = sp.lower()
        name = ntpath.basename(low)
        found = None
        if zip_names and name in zip_names:
            status = 'inzip'
        elif (low.startswith(('%flstudiofactorydata%', '%flstudiodata%', '\\patches\\', 'patches\\'))
              or FACTORY_SAMPLE_RE.search(low)):
            status = 'factory'
        else:
            p = expand_sample(sp)
            indexed = name.rsplit('.', 1)[-1] in AUDIO_EXTS
            if (p.lower() in audio.paths) if indexed else os.path.exists(p):
                status = 'ok'
            else:
                found = audio.best(name, p)
                status = 'moved' if found else 'missing'
        missing += status == 'missing'
        moved += status == 'moved'
        out.append({'path': sp, 'status': status, 'found': found})
    return out, missing, moved


def find_render(directory, name, audio):
    stem = name[:-4]
    for cand in (stem, AUTOSAVE_RE.sub('', stem)):
        for ext in RENDER_EXTS:
            p = directory + '\\' + cand + ext
            if p.lower() in audio.paths:
                return p
    return None


# ---------- workers (run in subprocesses) ----------

def quick_hash(buf):
    n = len(buf)
    h = hashlib.sha1(str(n).encode())
    h.update(bytes(buf[:65536]))
    if n > 65536:
        h.update(bytes(buf[max(65536, n - 65536):]))
    return h.hexdigest()


def work_flp(path):
    try:
        with open(path, 'rb') as f:
            mm = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
            try:
                return path, quick_hash(mm), flparse.parse_buffer(mm), None
            finally:
                mm.close()
    except Exception as e:  # noqa: BLE001 - one bad file must not stop the scan
        return path, None, None, f'{type(e).__name__}: {e}'


def work_zip(args):
    zpath, members = args
    out = []
    try:
        with zipfile.ZipFile(zpath) as z:
            names = sorted({ntpath.basename(n.replace('/', '\\')).lower() for n in z.namelist()})
            for m in members:
                try:
                    if z.getinfo(m).file_size > MAX_ZIP_MEMBER:
                        raise flparse.FLPError('flp inside zip too large')
                    data = z.read(m)
                    out.append((m, quick_hash(data), flparse.parse_buffer(data), None))
                except Exception as e:  # noqa: BLE001
                    out.append((m, None, None, f'{type(e).__name__}: {e}'))
    except Exception as e:  # noqa: BLE001
        return zpath, [], [(m, None, None, f'{type(e).__name__}: {e}') for m in members]
    return zpath, names, out


def peek_zip(path):
    try:
        with zipfile.ZipFile(path) as z:
            return path, [n for n in z.namelist() if n.lower().endswith('.flp')][:MAX_FLPS_PER_ZIP]
    except Exception:  # noqa: BLE001 - not a readable zip
        return path, []


def store(db, audio, path, container, kind, directory, name, size, mtime, h, info, err, zip_names):
    key, sname, autosave = song_identity(directory, name)
    if err or not info:
        h = 'err:' + hashlib.sha1(path.encode('utf-8', 'replace')).hexdigest()
        db.execute('INSERT OR REPLACE INTO projects(hash, error) VALUES(?,?)', (h, err or 'unknown'))
    elif audio is None and db.execute('SELECT 1 FROM projects WHERE hash = ?', (h,)).fetchone():
        pass  # same content already indexed with resolved samples
    else:
        samples, n_missing, n_moved = resolve_samples(info['samples'], audio, zip_names)
        ver = info['version'] or ''
        major = flparse._version_tuple(ver)[0] if ver else None
        db.execute(
            'INSERT OR REPLACE INTO projects VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)',
            (h, ver, major, info['bpm'], info['title'], info['genre'], info['author'],
             info['comment'], info['channels'], info['patterns'], info['created'],
             info['hours_worked'], json.dumps(info['plugins'], ensure_ascii=False),
             json.dumps(samples, ensure_ascii=False), n_missing, n_moved,
             'truncated file' if info['truncated'] else None))
    render = find_render(directory, name, audio) if kind == 'flp' and audio else None
    db.execute(
        'INSERT OR REPLACE INTO files VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?)',
        (path, container, kind, name, directory, size, mtime, h, key, sname,
         int(bool(FACTORY_RE.search(path))), int(autosave), render))


def index_one(db, path):
    """Index a single .flp right now (no sample check); size -1 makes the next scan redo it fully."""
    path = os.path.abspath(path)
    st = os.stat(path)
    _, h, info, err = work_flp(path)
    store(db, None, path, None, 'flp', ntpath.dirname(path), ntpath.basename(path), -1, int(st.st_mtime),
          h, info, err, None)
    db.commit()


def run_pool(db, fn, items, path_of, on_result, label, progress, stall=90):
    """Parse items in worker processes. A share that stops answering is dropped for this scan
    (its files are retried next time) instead of hanging everything."""
    workers = max(2, min(8, (os.cpu_count() or 4) - 1))
    total, done_n = len(items), 0
    while items:
        pool = ProcessPoolExecutor(workers)
        futs = {pool.submit(fn, it): it for it in items}
        waiting = set(futs)
        stalled = False
        while waiting:
            done, waiting = wait(waiting, timeout=stall, return_when=FIRST_COMPLETED)
            if not done:
                stalled = True
                break
            for f in done:
                on_result(futs[f], f.result())
                done_n += 1
                if done_n % 500 == 0:
                    db.commit()
                    progress(f'{label}: {done_n:,}/{total:,}')
        db.commit()
        if not stalled:
            pool.shutdown()
            break
        for proc in list(pool._processes.values()):
            proc.terminate()
        pool.shutdown(wait=False, cancel_futures=True)
        stuck = [futs[f] for f in waiting if f.running()]
        bad_roots = {root_of(path_of(it)) for it in stuck if path_of(it).startswith('\\\\')}
        if bad_roots:
            for r in sorted(bad_roots):
                progress(f'{r} stopped answering, skipping it this scan')
            items = [futs[f] for f in waiting if root_of(path_of(futs[f])) not in bad_roots]
        else:  # a local file that cannot be read: record it and move on
            for it in stuck:
                on_result(it, fn_timeout(fn, it))
            stuck_ids = {id(it) for it in stuck}
            items = [futs[f] for f in waiting if id(futs[f]) not in stuck_ids]
        total -= len(waiting) - len(items)


def fn_timeout(fn, item):
    if fn is work_flp:
        return item, None, None, 'timed out reading file'
    return item[0], [], [(m, None, None, 'timed out reading file') for m in item[1]]


# ---------- scan ----------

def scan(recheck=False):
    cfg = load_config()
    db = connect()
    wanted = make_filter(cfg)
    t0 = time.time()

    def progress(msg):
        print(msg, flush=True)
        db.execute("INSERT OR REPLACE INTO state VALUES('progress', ?)", (msg,))
        db.commit()

    st = dict(db.execute('SELECT k, v FROM state'))
    if st.get('scanning') == '1' and time.time() - int(st.get('scan_started') or 0) < 7200:
        print('another scan is running')
        return
    db.execute("INSERT OR REPLACE INTO state VALUES('scanning', '1')")
    db.execute("INSERT OR REPLACE INTO state VALUES('scan_started', ?)", (str(int(time.time())),))
    progress('querying Everything...')
    flps = candidates(cfg, 'file: ext:flp', wanted)
    zips = candidates(cfg, 'file: ext:zip', wanted)

    roots = {root_of(p) for p in list(flps) + list(zips)}
    live = {r for r in roots if reachable(r)}
    for r in sorted(roots - live):
        progress(f'unreachable, skipping: {r}')
    flps = {p: v for p, v in flps.items() if root_of(p) in live}
    zips = {p: v for p, v in zips.items() if root_of(p) in live}
    progress(f'{len(flps):,} flp, {len(zips):,} zip candidates')

    # which zips hold projects (cached per size+mtime)
    cache = {r[0]: r[1:] for r in db.execute('SELECT path, size, mtime, members FROM zipcache')}
    to_peek = [p for p, (s, m) in zips.items() if p not in cache or cache[p][:2] != (s, m)]
    if to_peek:
        with ThreadPoolExecutor(8) as ex:
            for i, (p, members) in enumerate(ex.map(peek_zip, to_peek), 1):
                s, m = zips[p]
                cache[p] = (s, m, json.dumps(members))
                db.execute('INSERT OR REPLACE INTO zipcache VALUES(?,?,?,?)', (p, s, m, cache[p][2]))
                if i % 2000 == 0:
                    progress(f'checking zips: {i:,}/{len(to_peek):,}')
        db.commit()
    zip_members = {}
    for p in zips:
        members = json.loads(cache[p][2])
        if members:
            zip_members[p] = members

    known = {r[0]: (r[1], r[2]) for r in db.execute('SELECT path, size, mtime FROM files')}
    new_flps = [p for p, v in flps.items() if recheck or known.get(p) != v]
    new_flps.sort(key=lambda p: p.startswith('\\\\'))  # local drives first, slow shares last
    new_zips = []
    for zp, members in zip_members.items():
        if recheck or any(known.get(zp + '|' + m) != zips[zp] for m in members):
            new_zips.append((zp, members))
    progress(f'{len(new_flps):,} flp + {len(new_zips):,} zip to parse')

    if new_flps or new_zips:
        progress('loading audio index from Everything...')
        audio = AudioIndex(cfg, wanted, progress)
        progress(f'audio index: {len(audio.paths):,} files')

        def on_flp(p, res):
            _, h, info, err = res
            s, m = flps[p]
            store(db, audio, p, None, 'flp', ntpath.dirname(p), ntpath.basename(p), s, m, h, info, err, None)

        def on_zip(item, res):
            zp, names, results = res
            s, m = zips[zp]
            names = set(names)
            for member, h, info, err in results:
                mname = ntpath.basename(member.replace('/', '\\'))
                store(db, audio, zp + '|' + member, zp, 'zip', ntpath.dirname(zp), mname, s, m, h, info, err, names)

        run_pool(db, work_flp, new_flps, lambda p: p, on_flp, 'parsing flp', progress)
        run_pool(db, work_zip, new_zips, lambda it: it[0], on_zip, 'parsing zips', progress)

    # drop rows whose file vanished from a reachable root
    seen = set(flps)
    for zp, members in zip_members.items():
        seen.update(zp + '|' + m for m in members)
    gone = [p for p in known if p not in seen and root_of(p) in live]
    gone += [p for p in known if not wanted(p)]
    db.executemany('DELETE FROM files WHERE path=?', [(p,) for p in gone])
    db.execute('DELETE FROM projects WHERE hash NOT IN (SELECT hash FROM files)')
    # song grouping rules may have changed since a row was written
    for path, container, name, old_key in db.execute('SELECT path, container, name, song_key FROM files').fetchall():
        key, sname, autosave = song_identity(ntpath.dirname(container or path), name)
        if key != old_key:
            db.execute('UPDATE files SET song_key = ?, song_name = ?, is_autosave = ? WHERE path = ?',
                       (key, sname, int(autosave), path))
    reflag(db)
    db.execute("INSERT OR REPLACE INTO state VALUES('last_scan', ?)", (str(int(time.time())),))
    db.execute("INSERT OR REPLACE INTO state VALUES('scanning', '0')")
    n_files = db.execute('SELECT COUNT(*) FROM files').fetchone()[0]
    n_proj = db.execute('SELECT COUNT(*) FROM projects').fetchone()[0]
    n_song = db.execute('SELECT COUNT(DISTINCT song_key) FROM files').fetchone()[0]
    progress(f'done in {time.time() - t0:.0f}s: {n_files:,} files, {n_proj:,} unique projects, '
             f'{n_song:,} songs, {len(gone):,} removed')
    db.close()


if __name__ == '__main__':
    try:
        scan(recheck='--recheck' in sys.argv)
    except Exception:
        db = connect()
        db.execute("INSERT OR REPLACE INTO state VALUES('scanning', '0')")
        db.commit()
        raise
