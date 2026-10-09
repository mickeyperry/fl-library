"""Minimal, tolerant FLP reader: pulls metadata without loading plugin states."""
import mmap
import struct
import sys
from datetime import datetime, timedelta

# Event ids (FL event stream: id byte, then 1/2/4 bytes or varint-length blob)
EV_CHAN_TYPE = 21
EV_NEW_CHAN = 64
EV_NEW_PAT = 65
EV_TEMPO_OLD = 66
EV_TEMPO_FINE = 93
EV_TEMPO = 156
EV_BUILD = 159
EV_TITLE = 194
EV_COMMENT = 195
EV_SAMPLE_PATH = 196
EV_VERSION = 199
EV_PLUGIN_INTERNAL = 201
EV_URL = 202
EV_PLUGIN_NAME = 203
EV_GENRE = 206
EV_AUTHOR = 207
EV_PLUGIN_PARAMS = 213
EV_TIMESTAMP = 237

WRAPPER = 'fruity wrapper'
# Fruity Wrapper sub-chunk ids
VST_FOURCC, VST_NAME, VST_PATH, VST_VENDOR = 51, 54, 55, 56

DELPHI_EPOCH = datetime(1899, 12, 30)


class FLPError(Exception):
    pass


def _version_tuple(s):
    out = []
    for part in s.split('.'):
        digits = ''.join(c for c in part if c.isdigit())
        out.append(int(digits) if digits else 0)
    return tuple(out)


def _text(data, utf16):
    if utf16:
        s = data.decode('utf-16-le', errors='replace')
    else:
        # pre-11.5 projects are ANSI; Hebrew codepage is the likely one here
        s = data.decode('cp1255', errors='replace')
    return s.split('\0', 1)[0].strip()


def _parse_wrapper(data):
    """Return (name, vendor, path) from a Fruity Wrapper state blob, if chunked."""
    name = vendor = path = None
    if len(data) < 4:
        return name, vendor, path
    kind = struct.unpack_from('<I', data, 0)[0]
    if kind not in (8, 9, 10, 11, 12):
        return name, vendor, path
    pos, n = 4, len(data)
    while pos + 12 <= n:
        cid, size = struct.unpack_from('<IQ', data, pos)
        pos += 12
        if size > n - pos:
            break
        if cid in (VST_NAME, VST_VENDOR, VST_PATH):
            val = bytes(data[pos:pos + size]).decode('utf-8', errors='replace').strip('\0').strip()
            if cid == VST_NAME:
                name = val
            elif cid == VST_VENDOR:
                vendor = val
            else:
                path = val
        pos += size
    return name, vendor, path


def parse_buffer(buf):
    n = len(buf)
    if n < 22 or bytes(buf[0:4]) != b'FLhd':
        raise FLPError('not an FLP (no FLhd header)')
    hlen = struct.unpack_from('<I', buf, 4)[0]
    fmt, nchan, ppq = struct.unpack_from('<hHH', buf, 8)
    pos = 8 + hlen
    if bytes(buf[pos:pos + 4]) != b'FLdt':
        raise FLPError('no FLdt chunk')
    dlen = struct.unpack_from('<I', buf, pos + 4)[0]
    pos += 8
    end = min(n, pos + dlen)

    info = {
        'format': fmt, 'ppq': ppq, 'channels': 0, 'patterns': 0,
        'version': None, 'build': None, 'bpm': None,
        'title': None, 'comment': None, 'genre': None, 'author': None, 'url': None,
        'created': None, 'hours_worked': None,
        'plugins': [], 'samples': [], 'truncated': False,
    }
    utf16 = False
    tempo_old = tempo_fine = None
    chan_ctx = 0  # events left in which a 201 still belongs to the channel just opened
    pending = None  # plugin being described by 201/203/213
    plugins = {}
    samples = {}

    def flush():
        nonlocal pending
        if not pending:
            return
        p, pending = pending, None
        name = p['name']
        if p['wrapper']:
            name = p['vst'] or p['alt']
            if not name:
                return
        key = (name.lower(), p['kind'])
        if key in plugins:
            plugins[key]['count'] += 1
        else:
            plugins[key] = {'name': name, 'vendor': p['vendor'], 'kind': p['kind'],
                            'vst': p['wrapper'], 'count': 1}

    while pos < end:
        eid = buf[pos]
        pos += 1
        if eid < 64:
            size = 1
        elif eid < 128:
            size = 2
        elif eid < 192:
            size = 4
            # FL 25+ writes event 172 with a 3-byte payload, directly followed by a text event (192)
            if eid == 172 and pos + 3 < end and buf[pos + 3] == 192:
                size = 3
        else:
            size = shift = 0
            while True:
                if pos >= end:
                    info['truncated'] = True
                    break
                b = buf[pos]
                pos += 1
                size |= (b & 0x7F) << shift
                shift += 7
                if not b & 0x80:
                    break
        if pos + size > end:
            info['truncated'] = True
            break
        start = pos
        pos += size
        if chan_ctx and eid != EV_NEW_CHAN:
            chan_ctx -= 1

        if eid == EV_VERSION:
            v = bytes(buf[start:pos]).decode('ascii', errors='replace').strip('\0').strip()
            info['version'] = v
            utf16 = _version_tuple(v)[:2] >= (11, 5)
        elif eid == EV_BUILD:
            info['build'] = struct.unpack_from('<I', buf, start)[0]
        elif eid == EV_TEMPO:
            info['bpm'] = struct.unpack_from('<I', buf, start)[0] / 1000.0
        elif eid == EV_TEMPO_OLD:
            tempo_old = struct.unpack_from('<H', buf, start)[0]
        elif eid == EV_TEMPO_FINE:
            tempo_fine = struct.unpack_from('<H', buf, start)[0]
        elif eid == EV_NEW_CHAN:
            info['channels'] += 1
            chan_ctx = 4
        elif eid == EV_NEW_PAT:
            num = struct.unpack_from('<H', buf, start)[0]
            if num > info['patterns']:
                info['patterns'] = num
        elif eid == EV_TITLE:
            info['title'] = _text(buf[start:pos], utf16) or None
        elif eid == EV_COMMENT:
            c = _text(buf[start:pos], utf16)
            if c and not c.startswith('{\\rtf'):
                info['comment'] = c[:2000]
        elif eid == EV_GENRE:
            info['genre'] = _text(buf[start:pos], utf16) or None
        elif eid == EV_AUTHOR:
            info['author'] = _text(buf[start:pos], utf16) or None
        elif eid == EV_URL:
            info['url'] = _text(buf[start:pos], utf16) or None
        elif eid == EV_TIMESTAMP and size >= 16:
            created, worked = struct.unpack_from('<dd', buf, start)
            try:
                if 20000 < created < 80000:
                    info['created'] = (DELPHI_EPOCH + timedelta(days=created)).isoformat(timespec='seconds')
                if 0 <= worked < 3650:
                    info['hours_worked'] = round(worked * 24, 2)
            except (OverflowError, ValueError):
                pass
        elif eid == EV_SAMPLE_PATH:
            sp = _text(buf[start:pos], utf16)
            if sp:
                samples.setdefault(sp.lower(), sp)
        elif eid == EV_PLUGIN_INTERNAL:
            flush()
            name = _text(buf[start:pos], utf16)
            kind = 'gen' if chan_ctx else 'fx'
            chan_ctx = 0
            if name:
                pending = {'name': name, 'wrapper': name.lower() == WRAPPER, 'kind': kind,
                           'vst': None, 'vendor': None, 'alt': None}
        elif eid == EV_PLUGIN_NAME:
            if pending and pending['wrapper'] and not pending['alt']:
                pending['alt'] = _text(buf[start:pos], utf16) or None
        elif eid == EV_PLUGIN_PARAMS:
            if pending and pending['wrapper'] and not pending['vst']:
                vname, vendor, _ = _parse_wrapper(buf[start:pos])
                pending['vst'], pending['vendor'] = vname, vendor
    flush()

    if info['bpm'] is None and tempo_old:
        info['bpm'] = tempo_old + (tempo_fine or 0) / 1000.0
    info['channels'] = info['channels'] or nchan
    info['plugins'] = sorted(plugins.values(), key=lambda p: (p['kind'] != 'gen', p['name'].lower()))
    info['samples'] = list(samples.values())
    return info


def parse_file(path):
    with open(path, 'rb') as f:
        try:
            mm = mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ)
        except ValueError:
            raise FLPError('empty file')
        try:
            return parse_buffer(mm)
        finally:
            mm.close()


if __name__ == '__main__':
    import json
    for p in sys.argv[1:]:
        try:
            print(json.dumps(parse_file(p), indent=1, ensure_ascii=False))
        except Exception as e:  # noqa: BLE001
            print(p, 'ERROR', e)
