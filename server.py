"""Local web UI for the FL project library. Run: python server.py"""
import json
import os
import subprocess
import sys
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import scanner

HERE = os.path.dirname(os.path.abspath(__file__))
CFG = scanner.load_config()
FL_EXE = scanner.find_fl_exe(CFG)
AUDIO_TYPES = {'.mp3': 'audio/mpeg', '.wav': 'audio/wav', '.flac': 'audio/flac',
               '.ogg': 'audio/ogg', '.m4a': 'audio/mp4'}


def state(db):
    return dict(db.execute('SELECT k, v FROM state'))


def list_songs(db):
    rows = db.execute(
        'SELECT f.path, f.kind, f.name, f.dir, f.mtime, f.hash, f.song_key, f.song_name, '
        'f.is_factory, f.is_autosave, f.render, p.version, p.major, p.bpm, p.hours, '
        'p.n_missing, p.n_moved, p.plugins, p.error, p.title, p.channels '
        'FROM files f LEFT JOIN projects p ON p.hash = f.hash').fetchall()
    meta = {r[0]: r[1:] for r in db.execute('SELECT song_key, status, tags, rating, notes FROM meta')}
    groups = {}
    for r in rows:
        groups.setdefault(r[6], []).append(r)
    songs = []
    for key, files in groups.items():
        good = [f for f in files if not f[18]] or files
        latest = max(good, key=lambda f: (not f[9], f[4]))
        render = next((f[10] for f in sorted(files, key=lambda f: -f[4]) if f[10]), None)
        try:
            plugins = [p['name'] for p in json.loads(latest[17] or '[]')]
        except ValueError:
            plugins = []
        m = meta.get(key, (None, None, None, None))
        songs.append({
            'key': key, 'name': latest[7], 'dir': latest[3], 'path': latest[0], 'file': latest[2],
            'mtime': max(f[4] for f in files), 'version': latest[11], 'major': latest[12],
            'bpm': latest[13], 'hours': max((f[14] or 0) for f in files) or None,
            'missing': latest[15] or 0, 'moved': latest[16] or 0, 'plugins': plugins,
            'error': latest[18], 'title': latest[19], 'channels': latest[20],
            'versions': len({f[5] for f in files}), 'copies': len(files),
            'factory': all(f[8] for f in files), 'zip': any(f[1] == 'zip' for f in files),
            'render': render, 'status': m[0] or '', 'tags': m[1] or '', 'rating': m[2] or 0,
            'notes': m[3] or '',
        })
    return songs


def song_detail(db, key):
    rows = db.execute(
        'SELECT f.path, f.container, f.kind, f.name, f.size, f.mtime, f.hash, f.is_autosave, f.render, '
        'p.version, p.bpm, p.hours, p.n_missing, p.n_moved, p.plugins, p.samples, p.error, p.title, '
        'p.genre, p.author, p.comment, p.channels, p.patterns, p.created '
        'FROM files f LEFT JOIN projects p ON p.hash = f.hash WHERE f.song_key = ?', (key,)).fetchall()
    versions = {}
    for r in rows:
        v = versions.get(r[6])
        if not v:
            v = versions[r[6]] = {
                'hash': r[6], 'name': r[3], 'mtime': r[5], 'size': r[4], 'autosave': bool(r[7]),
                'version': r[9], 'bpm': r[10], 'hours': r[11], 'missing': r[12] or 0, 'moved': r[13] or 0,
                'plugins': json.loads(r[14] or '[]'), 'samples': json.loads(r[15] or '[]'),
                'error': r[16], 'title': r[17], 'genre': r[18], 'author': r[19], 'comment': r[20],
                'channels': r[21], 'patterns': r[22], 'created': r[23], 'files': []}
        v['mtime'] = max(v['mtime'], r[5])
        v['files'].append({'path': r[0], 'kind': r[2], 'render': r[8]})
    return sorted(versions.values(), key=lambda v: -v['mtime'])


class Handler(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def log_message(self, *a):
        pass

    def send_json(self, obj, code=200):
        body = json.dumps(obj, ensure_ascii=False).encode('utf-8')
        self.send_response(code)
        self.send_header('Content-Type', 'application/json; charset=utf-8')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        url = urllib.parse.urlparse(self.path)
        q = urllib.parse.parse_qs(url.query)
        if url.path == '/':
            with open(os.path.join(HERE, 'index.html'), 'rb') as f:
                body = f.read()
            self.send_response(200)
            self.send_header('Content-Type', 'text/html; charset=utf-8')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        db = scanner.connect()
        try:
            if url.path == '/api/songs':
                self.send_json({'songs': list_songs(db), 'state': state(db), 'fl_exe': FL_EXE})
            elif url.path == '/api/song':
                self.send_json({'versions': song_detail(db, q.get('key', [''])[0])})
            elif url.path == '/api/status':
                self.send_json(state(db))
            elif url.path == '/api/audio':
                self.send_audio(db, q.get('path', [''])[0])
            else:
                self.send_json({'error': 'not found'}, 404)
        finally:
            db.close()

    def send_audio(self, db, path):
        ext = os.path.splitext(path)[1].lower()
        ok = db.execute('SELECT 1 FROM files WHERE render = ? LIMIT 1', (path,)).fetchone()
        if not ok or ext not in AUDIO_TYPES or not os.path.isfile(path):
            return self.send_json({'error': 'not found'}, 404)
        size = os.path.getsize(path)
        start, end = 0, size - 1
        rng = self.headers.get('Range')
        if rng and rng.startswith('bytes='):
            a, _, b = rng[6:].partition('-')
            try:
                if a:
                    start = int(a)
                    end = int(b) if b else end
                else:
                    start = max(0, size - int(b))
            except ValueError:
                pass
            end = min(end, size - 1)
        if start > end:
            self.send_response(416)
            self.send_header('Content-Range', f'bytes */{size}')
            self.send_header('Content-Length', '0')
            self.end_headers()
            return
        self.send_response(206 if rng else 200)
        self.send_header('Content-Type', AUDIO_TYPES[ext])
        self.send_header('Accept-Ranges', 'bytes')
        self.send_header('Content-Length', str(end - start + 1))
        if rng:
            self.send_header('Content-Range', f'bytes {start}-{end}/{size}')
        self.end_headers()
        try:
            with open(path, 'rb') as f:
                f.seek(start)
                left = end - start + 1
                while left > 0:
                    chunk = f.read(min(262144, left))
                    if not chunk:
                        break
                    self.wfile.write(chunk)
                    left -= len(chunk)
        except (ConnectionError, OSError):
            pass

    def do_POST(self):
        n = int(self.headers.get('Content-Length') or 0)
        try:
            data = json.loads(self.rfile.read(n) or b'{}')
        except ValueError:
            return self.send_json({'error': 'bad json'}, 400)
        db = scanner.connect()
        try:
            if self.path == '/api/meta':
                db.execute(
                    'INSERT INTO meta(song_key, status, tags, rating, notes, updated) VALUES(?,?,?,?,?,?) '
                    'ON CONFLICT(song_key) DO UPDATE SET status = excluded.status, tags = excluded.tags, '
                    'rating = excluded.rating, notes = excluded.notes, updated = excluded.updated',
                    (data['key'], data.get('status') or '', data.get('tags') or '',
                     int(data.get('rating') or 0), data.get('notes') or '', int(time.time())))
                db.commit()
                self.send_json({'ok': True})
            elif self.path in ('/api/open', '/api/reveal'):
                row = db.execute('SELECT container FROM files WHERE path = ?', (data.get('path'),)).fetchone()
                if not row:
                    return self.send_json({'error': 'unknown file'}, 404)
                target = row[0] or data['path']
                if not os.path.exists(target):
                    return self.send_json({'error': 'file is not reachable right now'}, 404)
                if self.path == '/api/reveal':
                    subprocess.Popen(f'explorer /select,"{target}"')
                elif FL_EXE:
                    subprocess.Popen([FL_EXE, target])
                else:
                    os.startfile(target)
                self.send_json({'ok': True})
            elif self.path == '/api/rescan':
                if state(db).get('scanning') == '1':
                    return self.send_json({'ok': True, 'already': True})
                db.execute("INSERT OR REPLACE INTO state VALUES('scanning', '1')")
                db.commit()
                args = [sys.executable, os.path.join(HERE, 'scanner.py')]
                if data.get('recheck'):
                    args.append('--recheck')
                subprocess.Popen(args, cwd=HERE, creationflags=subprocess.CREATE_NO_WINDOW)
                self.send_json({'ok': True})
            else:
                self.send_json({'error': 'not found'}, 404)
        except KeyError as e:
            self.send_json({'error': f'missing field {e}'}, 400)
        finally:
            db.close()


if __name__ == '__main__':
    port = int(CFG.get('port', 8777))
    print(f'FL Library on http://127.0.0.1:{port}  (FL: {FL_EXE or "file association"})', flush=True)
    ThreadingHTTPServer(('127.0.0.1', port), Handler).serve_forever()
