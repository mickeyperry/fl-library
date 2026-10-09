# FL Library

An FL Studio project organiser that lives inside Windows Explorer.

It indexes every `.flp` (and FL project `.zip`) on your drives through
[Everything](https://www.voidtools.com/), and adds a dark, FL-styled panel next to Explorer's file
list. It never moves or changes your project files.

## What you get

- **A panel beside Explorer's file list** (it sits where the preview pane is: Alt+P)
  - **Selected `.flp`:** FL version, BPM, channels, hours worked, creation date, plugins, samples
    (missing ones are flagged, with where Everything found moved copies), and every version of the
    song.
  - **Selected folder:** the projects inside it, as a sortable table.
  - **Folder with no projects:** the whole **Library**, a tree of every project grouped by
    creation year, month and song.
  - **Search box:** finds projects on every drive through Everything.
  - **Your own data:** status (idea / wip / mixing / done / dead), 1–5 star rating, bookmarks, tags
    and notes, stored per song.
  - **Opening a project:** double-click opens in the newest FL. Shift+double-click opens in the FL
    version the project was made with (the nearest installed one). Ctrl+double-click shows the file
    in the current Explorer window.
  - **Hide demos:** hides FL's own demo, template, tutorial and performance-mode projects.
  - **Colours:** each project folder gets its own colour.
- **Explorer columns for `.flp` files:** FL version, BPM, status, rating, hours worked, missing
  samples and versions. Right-click a column header → More…
- **Right-click menu** on `.flp` files: set status, rating, tags and notes, list samples, find all
  copies.
- **Tray icon:** click to turn the panel on or off; right-click → Exit.

## Requirements

- Windows 10 or 11, 64-bit.
- [Everything](https://www.voidtools.com/) with **Tools → Options → HTTP Server** enabled. The
  default port is 8666; change it in `config.json`.
- Nothing else to run the installer: it bundles its own Python. Building from source needs
  Python 3.10+ and Rust.

## Install

1. Install [Everything](https://www.voidtools.com/) and turn on **Tools → Options → HTTP Server**
   (port 8666).
2. Run **`FL-Library-Setup.exe`** (from Releases, or build it yourself: see below).

The setup shows whether Everything is reachable and which FL Studio versions it found, then
installs to `C:\Program Files\FL Library`. Nothing else is needed: Python is bundled. It does the
following:

- registers the Explorer columns and the right-click menu
- starts the panel and, if you choose, adds it to Windows startup
- adds a scheduled task that re-indexes every 30 minutes (only new or changed files are parsed)

Your library (index, tags, notes, bookmarks) lives in `%LOCALAPPDATA%\FL Library`.

To uninstall, use Windows **Settings → Apps → FL Library**. Removing your library as well is
optional.

### Build the installer

Needs Python 3.10+ (with Pillow: `pip install pillow`) and Rust (stable, MSVC toolchain).

```powershell
git clone https://github.com/<you>/fl-library
cd fl-library
python tools/build_installer.py        # -> dist\FL-Library-Setup.exe
```

The build script builds the panel and the Explorer extension, generates the icons, downloads the
official embeddable Python from python.org (cached in `build\`), and packs everything into one exe.

### Developer install (run from the checkout)

```powershell
copy config.example.json config.json
python make_icons.py
cd shellext; cargo build --release; cd ..
# from an elevated, 64-bit PowerShell:
powershell -ExecutionPolicy Bypass -File install.ps1     # uninstall.ps1 to remove
```

## Settings (`config.json`)

| key | meaning |
|---|---|
| `everything_url` | Everything's HTTP server address |
| `exclude_prefixes` | path roots to skip, e.g. network shares that mirror local drives |
| `exclude_contains` | skip any path containing these |
| `pythonw` | which `pythonw.exe` to use; defaults to the first one on PATH |
| `port` | port of the optional browser UI (`python server.py`) |

## How it works

| part | file |
|---|---|
| FLP parser (header events, plugins, sample paths; no third-party deps) | `flparse.py` |
| Indexer: Everything → parse → SQLite (`library.db`) | `scanner.py` |
| Menu and panel actions (status, rating, tags, bookmarks, folder colours) | `flctl.py` |
| Explorer property handler (columns) and panel UI, in Rust | `shellext/src/lib.rs`, `shellext/src/panel.rs` |
| Companion window that follows Explorer, plus the tray icon | `shellext/src/bin/flpanel.rs` |
| Optional browser UI | `server.py`, `index.html` |

The panel is a separate process, deliberately not attached to Explorer, so a slow network share or
a long search can never freeze Explorer. The column handler runs out of process for the same reason.

## Privacy

Everything stays on your machine. `library.db` (your index, tags, notes and bookmarks) and
`config.json` are gitignored and never leave your PC. The panel only talks to Everything on
`127.0.0.1`.
