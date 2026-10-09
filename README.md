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
- Python 3.10+ with tkinter (the standard python.org installer), used by the scanner and the menu
  actions.
- Rust (stable, MSVC toolchain), to build the panel and the Explorer extension.
- Pillow (`pip install pillow`), to generate the icons.

## Install

```powershell
git clone https://github.com/<you>/fl-library
cd fl-library
copy config.example.json config.json      # edit if your Everything port differs
pip install pillow
python make_icons.py
cd shellext; cargo build --release; cd ..
python scanner.py                         # first full index; can take a while on big drives
# from an *elevated, 64-bit* PowerShell:
powershell -ExecutionPolicy Bypass -File install.ps1
```

The installer does the following:

- registers the Explorer columns and the right-click menu
- starts the panel and adds it to Windows startup
- adds a scheduled task that re-indexes every 30 minutes (only new or changed files are parsed)

To remove everything (your `library.db` is kept):

```powershell
powershell -ExecutionPolicy Bypass -File uninstall.ps1
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
