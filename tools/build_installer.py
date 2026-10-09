"""Build dist/FL-Library-Setup.exe: one self-contained installer.

Steps: build the Rust panel + Explorer extension, generate icons, fetch an embedded Python (cached),
assemble everything into installer/payload.zip, then compile the installer around it.

    python tools/build_installer.py
"""
import io
import os
import shutil
import subprocess
import sys
import urllib.request
import zipfile

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BUILD = os.path.join(ROOT, 'build')
PY_VERSION = '3.12.8'
PY_URL = f'https://www.python.org/ftp/python/{PY_VERSION}/python-{PY_VERSION}-embed-amd64.zip'
APP_FILES = ['flparse.py', 'scanner.py', 'flctl.py']


def run(cmd, cwd):
    print('>', ' '.join(cmd), flush=True)
    subprocess.run(cmd, cwd=cwd, check=True)


def embedded_python():
    os.makedirs(BUILD, exist_ok=True)
    cached = os.path.join(BUILD, os.path.basename(PY_URL))
    if not os.path.exists(cached):
        print('downloading', PY_URL, flush=True)
        with urllib.request.urlopen(PY_URL) as r, open(cached + '.part', 'wb') as f:
            shutil.copyfileobj(r, f)
        os.replace(cached + '.part', cached)
    return cached


def main():
    run([sys.executable, 'make_icons.py'], ROOT)
    run(['cargo', 'build', '--release'], os.path.join(ROOT, 'shellext'))
    rel = os.path.join(ROOT, 'shellext', 'target', 'release')

    payload = io.BytesIO()
    with zipfile.ZipFile(payload, 'w', zipfile.ZIP_DEFLATED, compresslevel=9) as z:
        z.write(os.path.join(rel, 'flpanel.exe'), 'flpanel.exe')
        z.write(os.path.join(rel, 'flprops.dll'), 'flprops.dll')
        z.write(os.path.join(ROOT, 'shellext', 'flprops.propdesc'), 'flprops.propdesc')
        for ico in ('fl_on.ico', 'fl_off.ico'):
            z.write(os.path.join(ROOT, 'icons', ico), ico)
        for f in APP_FILES:
            z.write(os.path.join(ROOT, f), f'app/{f}')
        for f in sorted(os.listdir(os.path.join(ROOT, 'icons'))):
            if f.startswith('folder_'):
                z.write(os.path.join(ROOT, 'icons', f), f'app/icons/{f}')
        # private Python: the stock embeddable build, told to also look in ..\app
        with zipfile.ZipFile(embedded_python()) as py:
            pth = f'python{PY_VERSION.replace(".", "")[:3]}._pth'
            for name in py.namelist():
                data = py.read(name)
                if name == pth:
                    data = data.replace(b'.\r\n', b'.\r\n..\\app\r\n', 1)
                    if b'..\\app' not in data:
                        data += b'\r\n..\\app\r\n'
                z.writestr(f'python/{name}', data)
    with open(os.path.join(ROOT, 'installer', 'payload.zip'), 'wb') as f:
        f.write(payload.getvalue())
    print(f'payload: {len(payload.getvalue()) / 1e6:.1f} MB', flush=True)

    run(['cargo', 'build', '--release'], os.path.join(ROOT, 'installer'))
    os.makedirs(os.path.join(ROOT, 'dist'), exist_ok=True)
    out = os.path.join(ROOT, 'dist', 'FL-Library-Setup.exe')
    shutil.copy(os.path.join(ROOT, 'installer', 'target', 'release', 'FL-Library-Setup.exe'), out)
    print('built', out, f'({os.path.getsize(out) / 1e6:.1f} MB)')


if __name__ == '__main__':
    main()
