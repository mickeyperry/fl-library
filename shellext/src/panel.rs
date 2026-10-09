//! The FL Library panel (dark, FL-flavoured). For an .flp: everything about that project with
//! one-click status / rating / bookmark, tags, notes, samples, versions. For a folder: the projects
//! inside it. Always: an Everything-powered search box, a Bookmarks list and a Library tree of every
//! indexed project by year / month / song.
//! Reads library.db; writes go through flctl.py. Hosted by flpanel.exe.

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::ffi::c_void;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::windows::process::CommandExt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::process::Command;
use std::time::Duration;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, IServiceProvider, CLSCTX_LOCAL_SERVER};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Controls::{InitCommonControlsEx, SetWindowTheme, ICC_LISTVIEW_CLASSES, ICC_TREEVIEW_CLASSES, INITCOMMONCONTROLSEX};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, GetAsyncKeyState, GetFocus, SetFocus};
use windows::Win32::UI::Shell::PropertiesSystem::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use crate::{parse_head, query, reg_str, Db, HEAD_BYTES};

/// Pseudo-path: open the panel straight on the Library tree (folders without projects).
pub const LIBRARY: &str = "::library";

pub const CLSID_PANEL: GUID = GUID::from_u128(0x3E9A1D52_8C47_4B6F_A1E3_5D7F2B9C6A21);

// ---------- theme (FL Studio-ish: charcoal + orange) ----------

const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | (g << 8) | (b << 16)
}
const C_BG: u32 = rgb(32, 33, 37);
const C_FIELD: u32 = rgb(44, 46, 52);
const C_BTN: u32 = rgb(60, 62, 70);
const C_BTN_DOWN: u32 = rgb(78, 80, 90);
const C_BORDER: u32 = rgb(86, 88, 98);
const C_TEXT: u32 = rgb(228, 228, 232);
const C_DIM: u32 = rgb(152, 154, 162);
const C_ACCENT: u32 = rgb(255, 140, 0);
const C_ON_ACCENT: u32 = rgb(24, 24, 24);
const STATUS_COLORS: [u32; 6] = [rgb(84, 150, 255), rgb(255, 190, 60), C_ACCENT, rgb(80, 205, 120), rgb(128, 128, 136), C_BTN_DOWN];

const STATUSES: [&str; 6] = ["idea", "wip", "mixing", "done", "dead", "none"];
const RATINGS: [&str; 6] = ["1 \u{2605}", "2 \u{2605}", "3 \u{2605}", "4 \u{2605}", "5 \u{2605}", "clear"];
const SEP: char = 0x5C as char; // backslash
const DOT: &str = "   \u{00B7}   ";
const STAR: &str = "\u{2605}";
const HINT: &str = "double-click: open   \u{00B7}   Shift: original FL   \u{00B7}   Ctrl: show in folder   \u{00B7}   \u{2605}: bookmark";

const ID_TITLE: i32 = 1;
const ID_INFO: i32 = 2;
const ID_PLUGINS: i32 = 3;
const ID_TAGS_LABEL: i32 = 4;
const ID_STATUS: i32 = 100; // ..=105
const ID_RATING: i32 = 110; // ..=115
const ID_TAGS: i32 = 120;
const ID_NOTES: i32 = 121;
const ID_SAVE: i32 = 122;
const ID_OPEN: i32 = 130;
const ID_PLAY: i32 = 131;
const ID_FIND: i32 = 132;
const ID_FOLDER: i32 = 133;
const ID_OPEN_ORIG: i32 = 134;
const ID_BOOKMARK: i32 = 135;
const ID_SAMPLES_LABEL: i32 = 140;
const ID_SAMPLES: i32 = 141;
const ID_VERSIONS_LABEL: i32 = 150;
const ID_VERSIONS: i32 = 151;
const ID_SEARCH_LABEL: i32 = 160;
const ID_SEARCH: i32 = 161;
const ID_RESULTS_LABEL: i32 = 162;
const ID_RESULTS: i32 = 163;
const ID_BOOKMARKS: i32 = 164;
const ID_LIBRARY: i32 = 165;
const ID_TREE: i32 = 166;
const ID_AUTOSHOW: i32 = 167;
const ID_HIDEDEMOS: i32 = 168;
const ID_COLORIZE: i32 = 169;
const ID_SEL_OPEN: i32 = 170; // act on the row selected in the results table / library tree
const ID_SEL_ORIG: i32 = 171;
const ID_SEL_FOLDER: i32 = 172;
const LVN_ITEMCHANGED: i32 = -101;
const TVN_SELCHANGEDW: i32 = -451;
const LVM_GETNEXTITEM: u32 = 0x100C;
const LVNI_SELECTED: isize = 2;
const SEARCH_TIMER: usize = 1;
const SELECT_TIMER: usize = 2;
const WM_SEARCH_DONE: u32 = WM_APP + 10;

// window styles / messages not worth a typed import
const BS_OWNERDRAW: u32 = 0x000B;
const ES_MULTILINE: u32 = 0x0004;
const ES_AUTOVSCROLL: u32 = 0x0040;
const ES_AUTOHSCROLL: u32 = 0x0080;
const ES_WANTRETURN: u32 = 0x1000;
const EN_CHANGE: u32 = 0x0300;
const LBS_NOINTEGRALHEIGHT: u32 = 0x0100;
const SS_NOPREFIX: u32 = 0x0080;
const SS_ENDELLIPSIS: u32 = 0x4000;
const LB_ADDSTRING: u32 = 0x0180;
const LB_SETHORIZONTALEXTENT: u32 = 0x0194;
const LVS_REPORT: u32 = 0x0001;
const LVS_SINGLESEL: u32 = 0x0004;
const LVS_SHOWSELALWAYS: u32 = 0x0008;
const LVM_SETBKCOLOR: u32 = 0x1001;
const LVM_DELETEALLITEMS: u32 = 0x1009;
const LVM_GETHEADER: u32 = 0x101F;
const LVM_SETTEXTCOLOR: u32 = 0x1024;
const LVM_SETTEXTBKCOLOR: u32 = 0x1026;
const LVM_SETEXTENDEDLISTVIEWSTYLE: u32 = 0x1036;
const LVM_INSERTITEMW: u32 = 0x104D;
const LVM_INSERTCOLUMNW: u32 = 0x1061;
const LVM_SETITEMTEXTW: u32 = 0x1074;
const HDM_GETITEMW: u32 = 0x120B;
const TVS_HASBUTTONS: u32 = 0x0001;
const TVS_HASLINES: u32 = 0x0002;
const TVS_LINESATROOT: u32 = 0x0004;
const TVS_SHOWSELALWAYS: u32 = 0x0020;
const TVM_DELETEITEM: u32 = 0x1101;
const TVM_GETNEXTITEM: u32 = 0x110A;
const TVM_SETBKCOLOR: u32 = 0x111D;
const TVM_SETTEXTCOLOR: u32 = 0x111E;
const TVM_SETLINECOLOR: u32 = 0x1128;
const TVM_INSERTITEMW: u32 = 0x1132;
const TVM_GETITEMW: u32 = 0x113E;
const TVGN_CARET: usize = 9;
const TVI_ROOT: isize = -0x10000;
const TVI_LAST: isize = -0x0FFFE;
const NM_CLICK: i32 = -2;
const NM_DBLCLK: i32 = -3;
const NM_CUSTOMDRAW: i32 = -12;
const LVN_COLUMNCLICK: i32 = -108;
const CDDS_PREPAINT: u32 = 1;
const CDDS_ITEMPREPAINT: u32 = 0x10001;
const CDRF_DODEFAULT: isize = 0;
const CDRF_SKIPDEFAULT: isize = 4;
const CDRF_NOTIFYITEMDRAW: isize = 0x20;
const ODS_SELECTED: u32 = 1;
const ODS_DISABLED: u32 = 4;
const VK_SHIFT: i32 = 0x10;
const VK_CONTROL: i32 = 0x11;

#[repr(C)]
struct LvColumn {
    mask: u32,
    fmt: i32,
    cx: i32,
    text: *mut u16,
    cch: i32,
    sub: i32,
    image: i32,
    order: i32,
    cx_min: i32,
    cx_default: i32,
    cx_ideal: i32,
}

#[repr(C)]
struct LvItem {
    mask: u32,
    item: i32,
    sub: i32,
    state: u32,
    state_mask: u32,
    text: *mut u16,
    cch: i32,
    image: i32,
    lparam: isize,
    indent: i32,
    group_id: i32,
    c_columns: u32,
    pu_columns: *mut u32,
    pi_col_fmt: *mut i32,
    group: i32,
}

#[repr(C)]
struct HdItem {
    mask: u32,
    cxy: i32,
    text: *mut u16,
    hbm: isize,
    cch: i32,
    fmt: i32,
    lparam: isize,
    image: i32,
    order: i32,
    kind: u32,
    filter: *mut c_void,
    state: u32,
}

#[repr(C)]
struct TvItem {
    mask: u32,
    item: isize,
    state: u32,
    state_mask: u32,
    text: *mut u16,
    cch: i32,
    image: i32,
    selected_image: i32,
    children: i32,
    lparam: isize,
    integral: i32,
    state_ex: u32,
    hwnd: isize,
    expanded_image: i32,
    reserved: i32,
}

#[repr(C)]
struct TvInsert {
    parent: isize,
    after: isize,
    item: TvItem,
}

#[repr(C)]
struct NmHdr {
    from: *mut c_void,
    id: usize,
    code: i32,
}

#[repr(C)]
struct NmListView {
    hdr: NmHdr,
    item: i32,
    sub: i32,
}

#[repr(C)]
struct NmCustomDraw {
    hdr: NmHdr,
    stage: u32,
    hdc: *mut c_void,
    rc: RECT,
    item: usize,
    state: u32,
    lparam: isize,
}

/// Prefix shared by NMLVCUSTOMDRAW and NMTVCUSTOMDRAW: the colours a row is drawn with.
#[repr(C)]
struct NmColorDraw {
    cd: NmCustomDraw,
    clr_text: u32,
    clr_text_bk: u32,
}

#[repr(C)]
struct DrawItem {
    ctl_type: u32,
    ctl_id: u32,
    item_id: u32,
    action: u32,
    state: u32,
    hwnd: *mut c_void,
    hdc: *mut c_void,
    rc: RECT,
    data: usize,
}

/// (header, width at 96 dpi, right-aligned)
const COLUMNS: [(&str, i32, bool); 8] = [
    (STAR, 26, false),
    ("Name", 200, false),
    ("FL version", 86, false),
    ("BPM", 44, true),
    ("Status", 56, false),
    ("Created", 80, false),
    ("Modified", 80, false),
    ("Folder", 420, false),
];

#[derive(Clone, Default)]
struct Row {
    bookmark: bool,
    name: String,
    version: String,
    bpm: f64,
    status: String,
    created: String,
    modified: i64,
    dir: String,
    path: String,   // library id (file path, or "zip|member")
    target: String, // what to hand to FL / Explorer
    song: String,
    song_name: String,
    demo: bool,
    color: (u32, u32), // (text, background) from the project folder's hue
}

/// Light hues that read well on the dark field; one per project folder.
const PALETTE: [(u32, u32, u32); 10] = [
    (255, 179, 128), (128, 222, 160), (140, 190, 255), (240, 160, 255), (255, 230, 120),
    (130, 230, 230), (255, 150, 170), (190, 255, 140), (255, 200, 90), (180, 170, 255),
];
const GENERIC_DIRS: [&str; 8] = ["backup", "flp", "flps", "projects", "project", "project data", "fl studio", "fl"];

/// The folder that identifies a project: the last path component that is not a generic name.
fn color_key(dir: &str) -> &str {
    dir.split(SEP).filter(|p| !p.is_empty()).rev().find(|p| !GENERIC_DIRS.contains(&p.to_lowercase().as_str())).unwrap_or("")
}

fn color_for(key: &str) -> (u32, u32) {
    if key.is_empty() {
        return (C_TEXT, C_FIELD);
    }
    let mut h: u32 = 0x811C9DC5; // FNV-1a
    for b in key.to_lowercase().bytes() {
        h = (h ^ b as u32).wrapping_mul(0x01000193);
    }
    let (r, g, b) = PALETTE[(h % PALETTE.len() as u32) as usize];
    let field = (C_FIELD & 0xFF, (C_FIELD >> 8) & 0xFF, (C_FIELD >> 16) & 0xFF);
    let mix = |c: u32, f: u32| (f * 78 + c * 22) / 100; // 22% tint: a hint of colour, the text carries the hue
    (rgb(r, g, b), rgb(mix(r, field.0), mix(g, field.1), mix(b, field.2)))
}

impl Row {
    fn cell(&self, col: usize) -> String {
        match col {
            0 => if self.bookmark { STAR.to_string() } else { String::new() },
            1 => self.name.clone(),
            2 => self.version.clone(),
            3 => if self.bpm > 0.0 { format!("{}", self.bpm.round() as u32) } else { String::new() },
            4 => if self.status.is_empty() && self.demo { "demo".to_string() } else { self.status.clone() },
            5 => self.created.clone(),
            6 => if self.modified > 0 { date(self.modified) } else { String::new() },
            _ => self.dir.clone(),
        }
    }
}

fn version_key(v: &str) -> Vec<u32> {
    v.split('.').map(|p| p.parse().unwrap_or(0)).collect()
}

fn sort_rows(rows: &mut [Row], col: i32, asc: bool) {
    rows.sort_by(|a, b| {
        let o = match col {
            0 => b.bookmark.cmp(&a.bookmark),
            2 => version_key(&a.version).cmp(&version_key(&b.version)),
            3 => a.bpm.partial_cmp(&b.bpm).unwrap_or(Ordering::Equal),
            6 => a.modified.cmp(&b.modified),
            c => a.cell(c as usize).to_lowercase().cmp(&b.cell(c as usize).to_lowercase()),
        };
        if asc { o } else { o.reverse() }
    });
}

#[derive(PartialEq, Clone, Copy)]
enum Mode {
    Inside,
    Search,
    Bookmarks,
    Library,
}

struct Song {
    path: String,
    folder: bool,
    name: String,
    info: String,
    plugins: String,
    version: String,
    status: String,
    rating: u32,
    bookmark: bool,
    tags: String,
    notes: String,
    samples: Vec<String>,
    sample_summary: String,
    versions: Vec<Row>,
    inside: Vec<Row>,  // folder mode: projects under the folder
    results: Vec<Row>, // what the results table currently shows
    library: Vec<Row>, // rows behind the library tree (lparam = index + 1)
    tree_loaded: bool,
    mode: Mode,
    search_gen: u64, // newest search; older results arriving late are dropped
    want_bookmarks: bool,
    want_library: bool,
    autoshow: bool,
    hide_demos: bool,
    sort_results: (i32, bool),
    sort_versions: (i32, bool),
    render: Option<String>,
    pending: Option<(String, u32)>, // file to select once Explorer has arrived in its folder
    dpi: i32,
    font: HFONT,
    bold: HFONT,
    small: HFONT,
    brush_bg: HBRUSH,
    brush_field: HBRUSH,
}

fn date(unix: i64) -> String {
    // civil-from-days (Howard Hinnant)
    let z = unix.div_euclid(86400) + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Columns every row query selects, in the order `row_from` expects.
const ROW_COLS: &str = "f.name, IFNULL(f.container, f.path), f.mtime, IFNULL(p.version,''), IFNULL(p.bpm,0), \
    IFNULL(m.status,''), IFNULL(m.bookmark,0), IFNULL(substr(p.created,1,10),''), f.dir, f.path, f.song_key, f.song_name, \
    f.is_factory";
const ROW_JOIN: &str = "FROM files f LEFT JOIN projects p ON p.hash = f.hash LEFT JOIN meta m ON m.song_key = f.song_key";

fn row_from(r: &[String]) -> Row {
    Row {
        name: r[0].clone(),
        target: r[1].clone(),
        modified: r[2].parse().unwrap_or(0),
        version: r[3].clone(),
        bpm: r[4].parse().unwrap_or(0.0),
        status: r[5].clone(),
        bookmark: r[6] == "1",
        created: r[7].clone(),
        dir: r[8].clone(),
        path: r[9].clone(),
        song: r[10].clone(),
        song_name: r[11].clone(),
        demo: r[12] == "1",
        color: color_for(if color_key(&r[8]).is_empty() { &r[11] } else { color_key(&r[8]) }),
    }
}

fn load_folder(s: &mut Song) {
    let rows = query(
        &format!(
            "SELECT {ROW_COLS} {ROW_JOIN} WHERE f.is_autosave = 0 AND (f.dir = ?1 COLLATE NOCASE \
             OR substr(f.dir, 1, length(?1) + 1) = ?1 || char(92) COLLATE NOCASE) ORDER BY f.mtime DESC LIMIT 2000"
        ),
        &s.path,
    );
    let mut songs = std::collections::HashSet::new();
    for r in &rows {
        let mut row = row_from(r);
        songs.insert(row.song.clone());
        row.dir = row.dir.get(s.path.len()..).unwrap_or("").trim_start_matches(SEP).to_string();
        // colour by the first subfolder, so everything under one project folder matches
        let key = row.dir.split(SEP).next().unwrap_or("").to_string();
        row.color = color_for(if key.is_empty() { &row.song_name } else { &key });
        s.inside.push(row);
    }
    s.info = if rows.is_empty() {
        "No FL projects indexed in this folder".to_string()
    } else {
        format!("{} project file{} in this folder{DOT}{} song{}", rows.len(), if rows.len() == 1 { "" } else { "s" },
                songs.len(), if songs.len() == 1 { "" } else { "s" })
    };
}

fn load_project(s: &mut Song, file_name: &str) {
    let path = s.path.clone();
    let mut buf = Vec::new();
    if let Ok(f) = std::fs::File::open(&path) {
        let _ = f.take(HEAD_BYTES).read_to_end(&mut buf);
    }
    let mut parts: Vec<String> = Vec::new();
    let mut line2: Vec<String> = Vec::new();
    if let Some(h) = parse_head(&buf) {
        parts.push(format!("FL {}", h.version));
        s.version = h.version.clone();
        if let Some(bpm) = h.bpm {
            parts.push(if bpm.fract() == 0.0 { format!("{} BPM", bpm as u32) } else { format!("{bpm:.1} BPM") });
        }
        if h.channels > 0 {
            parts.push(format!("{} channels", h.channels));
        }
        if let Some(hours) = h.hours {
            parts.push(format!("{hours:.1} h worked"));
        }
        for (label, v) in [("Title", &h.title), ("Author", &h.author), ("Genre", &h.genre)] {
            if !v.is_empty() {
                line2.push(format!("{label}: {v}"));
            }
        }
    } else {
        parts.push("not a readable FL project".into());
    }

    let rows = query(
        "SELECT f.song_key, f.song_name, IFNULL(p.samples,'[]'), IFNULL(p.plugins,'[]'), IFNULL(m.status,''), \
         IFNULL(m.tags,''), IFNULL(m.rating,0), IFNULL(m.notes,''), IFNULL(m.bookmark,0), IFNULL(substr(p.created,1,10),'') \
         FROM files f LEFT JOIN projects p ON p.hash = f.hash LEFT JOIN meta m ON m.song_key = f.song_key \
         WHERE f.path = ?1 COLLATE NOCASE LIMIT 1",
        &path,
    );
    if let Some(r) = rows.first() {
        let key = &r[0];
        s.name = format!("{}  \u{2014}  {}", r[1], file_name);
        s.status = r[4].clone();
        s.tags = r[5].clone();
        s.rating = r[6].parse().unwrap_or(0);
        s.notes = r[7].replace('\n', "\r\n").replace("\r\r\n", "\r\n");
        s.bookmark = r[8] == "1";
        if !r[9].is_empty() {
            parts.push(format!("created {}", r[9]));
        }

        if let Ok(serde_json::Value::Array(items)) = serde_json::from_str(&r[3]) {
            let names: Vec<&str> = items.iter().filter_map(|p| p["name"].as_str()).collect();
            if !names.is_empty() {
                s.plugins = format!("Plugins ({}): {}", names.len(), names.join(", "));
            }
        }
        if let Ok(serde_json::Value::Array(items)) = serde_json::from_str(&r[2]) {
            let order = |st: &str| ["missing", "moved", "unchecked", "ok", "inzip", "factory"].iter().position(|x| *x == st).unwrap_or(9);
            let mut list: Vec<(usize, String)> = Vec::new();
            let mut counts = [0usize; 10];
            for it in &items {
                let st = it["status"].as_str().unwrap_or("?");
                let p = it["path"].as_str().unwrap_or("");
                counts[order(st)] += 1;
                let file = p.rsplit(SEP).next().unwrap_or(p);
                let line = match it["found"].as_str() {
                    Some(found) => format!("[{st}] {file}   \u{2192}   {found}"),
                    None => format!("[{st}] {file}   \u{2014}   {p}"),
                };
                list.push((order(st), line));
            }
            list.sort();
            s.samples = list.into_iter().map(|(_, l)| l).collect();
            s.sample_summary = format!(
                "SAMPLES ({}){DOT}{} missing, {} moved, {} ok, {} factory",
                items.len(), counts[0], counts[1], counts[3] + counts[4], counts[5]
            );
        }

        // one row per distinct content; copies on other drives are counted, not listed
        for v in query(
            &format!(
                "SELECT {ROW_COLS}, COUNT(*), MAX(f.mtime) {ROW_JOIN} WHERE f.song_key = ?1 \
                 GROUP BY f.hash ORDER BY f.is_autosave, MAX(f.mtime) DESC LIMIT 400"
            ),
            key,
        ) {
            let mut row = row_from(&v);
            row.modified = v[14].parse().unwrap_or(row.modified);
            let copies: u32 = v[13].parse().unwrap_or(1);
            if copies > 1 {
                row.name.push_str(&format!("   ({copies} copies)"));
            }
            s.versions.push(row);
        }
        parts.push(format!("{} version{}", s.versions.len(), if s.versions.len() == 1 { "" } else { "s" }));
        s.render = query(
            "SELECT render FROM files WHERE song_key = ?1 AND render IS NOT NULL ORDER BY mtime DESC LIMIT 1",
            key,
        )
        .first()
        .map(|r| r[0].clone());
    }
    s.info = parts.join(DOT);
    if !line2.is_empty() {
        s.info.push_str("\r\n");
        s.info.push_str(&line2.join(DOT));
    }
}

fn load(path: &str) -> Song {
    let file_name = path.trim_end_matches(SEP).rsplit(SEP).next().unwrap_or(path).to_string();
    let mut s = Song {
        path: path.trim_end_matches(SEP).to_string(),
        folder: std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false),
        name: file_name.clone(),
        info: String::new(),
        plugins: String::new(),
        version: String::new(),
        status: String::new(),
        rating: 0,
        bookmark: false,
        tags: String::new(),
        notes: String::new(),
        samples: Vec::new(),
        sample_summary: "SAMPLES: not indexed yet (rescan runs every 30 min)".into(),
        versions: Vec::new(),
        inside: Vec::new(),
        results: Vec::new(),
        library: Vec::new(),
        tree_loaded: false,
        mode: Mode::Inside,
        search_gen: 0,
        want_bookmarks: false,
        want_library: false,
        autoshow: autoshow_enabled(),
        hide_demos: hide_demos_enabled(),
        sort_results: (6, false),
        sort_versions: (6, false),
        render: None,
        pending: None,
        dpi: 96,
        font: HFONT::default(),
        bold: HFONT::default(),
        small: HFONT::default(),
        brush_bg: HBRUSH::default(),
        brush_field: HBRUSH::default(),
    };
    if path == LIBRARY {
        s.folder = true;
        s.path.clear();
        s.name = "FL Library".to_string();
        s.info = "No FL projects in this folder \u{2014} here is everything you have".to_string();
        s.want_library = true;
    } else if s.folder {
        load_folder(&mut s);
    } else {
        load_project(&mut s, &file_name);
    }
    s
}

fn bookmarks() -> Vec<Row> {
    // newest file of every bookmarked song (bare columns follow MAX in SQLite)
    query(
        &format!(
            "SELECT {ROW_COLS}, MAX(f.mtime) {ROW_JOIN} WHERE m.bookmark = 1 AND f.is_autosave = 0 AND ?1 = '' \
             GROUP BY f.song_key ORDER BY MAX(f.mtime) DESC LIMIT 2000"
        ),
        "",
    )
    .iter()
    .map(|r| row_from(r))
    .collect()
}

// ---------- Explorer "preview pane on by default" ----------

const PANE_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Explorer\\Modules\\GlobalSettings\\DetailsContainer");

/// Whether new Explorer windows open with the preview pane (where this panel lives).
fn autoshow_enabled() -> bool {
    unsafe {
        let mut data = [0u8; 8];
        let mut size = 8u32;
        RegGetValueW(HKEY_CURRENT_USER, PANE_KEY, w!("DetailsContainer"), RRF_RT_REG_BINARY, None,
                     Some(data.as_mut_ptr() as *mut c_void), Some(&mut size)).is_ok()
            && size == 8 && data[0] == 2 && data[4] == 1
    }
}

fn set_autoshow(on: bool) {
    unsafe {
        let mut key = HKEY::default();
        if RegCreateKeyExW(HKEY_CURRENT_USER, PANE_KEY, 0, None, REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut key, None).is_ok() {
            let data = [2u8, 0, 0, 0, on as u8, 0, 0, 0]; // preview pane, visible flag
            let _ = RegSetValueExW(key, w!("DetailsContainer"), 0, REG_BINARY, Some(&data));
            let _ = RegCloseKey(key);
        }
    }
}

const OPT_KEY: PCWSTR = w!("Software\\FLLibrary");

/// Hide FL's own demo / template / tutorial projects (default on).
fn hide_demos_enabled() -> bool {
    unsafe {
        let mut data = 1u32;
        let mut size = 4u32;
        let _ = RegGetValueW(HKEY_CURRENT_USER, OPT_KEY, w!("HideDemos"), RRF_RT_REG_DWORD, None,
                             Some(&mut data as *mut u32 as *mut c_void), Some(&mut size));
        data != 0
    }
}

fn set_hide_demos(on: bool) {
    unsafe {
        let mut key = HKEY::default();
        if RegCreateKeyExW(HKEY_CURRENT_USER, OPT_KEY, 0, None, REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut key, None).is_ok() {
            let _ = RegSetValueExW(key, w!("HideDemos"), 0, REG_DWORD, Some(&(on as u32).to_le_bytes()));
            let _ = RegCloseKey(key);
        }
    }
}

// ---------- Everything search ----------

fn everything(search: &str) -> Option<serde_json::Value> {
    let addr: SocketAddr = reg_str(w!("Everything"))?.parse().ok()?;
    let mut encoded = String::new();
    for b in search.bytes() {
        if b.is_ascii_alphanumeric() {
            encoded.push(b as char);
        } else {
            encoded.push_str(&format!("%{b:02X}"));
        }
    }
    let mut stream = TcpStream::connect_timeout(&addr, Duration::from_millis(400)).ok()?;
    stream.set_read_timeout(Some(Duration::from_millis(2500))).ok()?;
    write!(
        stream,
        "GET /?json=1&path_column=1&date_modified_column=1&sort=date_modified&ascending=0&count=150&search={encoded} \
         HTTP/1.0\r\nHost: {addr}\r\n\r\n"
    )
    .ok()?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).ok()?;
    let body = raw.windows(4).position(|w| w == b"\r\n\r\n")? + 4;
    serde_json::from_slice(&raw[body..]).ok()
}

/// Projects on any drive whose name matches `text`, newest first, with what the library knows.
fn search(text: &str) -> (String, Vec<Row>) {
    let exclude = reg_str(w!("SearchExclude")).unwrap_or_default();
    let Some(json) = everything(&format!("file: ext:flp;zip {text} !\"(autosaved\" !\"(overwritten\" {exclude}")) else {
        return ("Everything is not answering".into(), Vec::new());
    };
    let total = json["totalResults"].as_u64().unwrap_or(0);
    let db = Db::open();
    let sql = format!("SELECT {ROW_COLS} {ROW_JOIN} WHERE f.path = ?1 COLLATE NOCASE OR f.container = ?1 COLLATE NOCASE LIMIT 1");
    let mut rows = Vec::new();
    for r in json["results"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        let (Some(name), Some(dir)) = (r["name"].as_str(), r["path"].as_str()) else { continue };
        let full = format!("{dir}{SEP}{name}");
        let known = db.as_ref().and_then(|db| db.rows(&sql, &full).into_iter().next());
        if known.is_none() && name.to_lowercase().ends_with(".zip") {
            continue; // a zip the library has not found a project in
        }
        let mut row = known.map(|k| row_from(&k)).unwrap_or_default();
        row.name = name.to_string();
        row.dir = dir.to_string();
        row.target = full.clone();
        if row.path.is_empty() {
            row.path = full;
        }
        row.modified = r["date_modified"].as_str().and_then(|t| t.parse::<i64>().ok()).map(|ft| ft / 10_000_000 - 11_644_473_600).unwrap_or(0);
        rows.push(row);
    }
    let label = if total as usize > rows.len() {
        format!("{} newest of {} matches", rows.len(), total)
    } else {
        format!("{} match{}", rows.len(), if rows.len() == 1 { "" } else { "es" })
    };
    (label, rows)
}

// ---------- actions ----------

fn flctl(args: &[&str]) {
    let (Some(py), Some(ctl)) = (reg_str(w!("Pythonw")), reg_str(w!("Ctl"))) else { return };
    let _ = Command::new(py).arg(ctl).args(args).creation_flags(0x0800_0000).spawn();
}

/// Installed FL Studio closest to `version`: the same major if present, else the next newer one.
fn fl_for(version: &str) -> Option<(u32, String)> {
    let major: u32 = version.split('.').next()?.parse().ok()?;
    let mut majors: Vec<u32> = reg_str(w!("FlMajors"))?.split(',').filter_map(|m| m.trim().parse().ok()).collect();
    majors.sort();
    let pick = majors.iter().copied().find(|m| *m >= major).or(majors.last().copied())?;
    let name = HSTRING::from(format!("Fl_{pick}"));
    Some((pick, reg_str(PCWSTR(name.as_ptr()))?))
}

fn open_in_fl(target: &str) {
    if let Some(exe) = reg_str(w!("FlExe")).filter(|e| !e.is_empty()) {
        if Command::new(exe).arg(target).spawn().is_ok() {
            return;
        }
    }
    shell_open(target);
}

fn open_original(target: &str, version: &str) {
    match fl_for(version) {
        Some((_, exe)) if Command::new(&exe).arg(target).spawn().is_ok() => {}
        _ => open_in_fl(target),
    }
}

fn shell_open(target: &str) {
    unsafe {
        ShellExecuteW(None, w!("open"), &HSTRING::from(target), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
    }
}

/// Show the file selected in an Explorer window (reuses a window already on that folder).
fn reveal(target: &str) {
    unsafe {
        let pidl = ILCreateFromPathW(&HSTRING::from(target));
        if !pidl.is_null() {
            let _ = AllowSetForegroundWindow(ASFW_ANY); // let Explorer come to the front
            let ok = SHOpenFolderAndSelectItems(pidl, None, 0).is_ok();
            ILFree(Some(pidl as *const _));
            if ok {
                return;
            }
        }
    }
    let _ = Command::new("explorer.exe").raw_arg(format!("/select,\"{target}\"")).spawn();
}

/// The Explorer window this panel belongs to.
unsafe fn browser_for(panel: HWND) -> Option<IShellBrowser> {
    // flpanel.exe records which Explorer window it is sitting on
    let root = HWND(GetPropW(GetAncestor(panel, GA_ROOT), w!("FLLibraryTarget")).0);
    if root.is_invalid() {
        return None;
    }
    let shell: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER).ok()?;
    for i in 0..shell.Count().ok()? {
        let Ok(disp) = shell.Item(&VARIANT::from(i)) else { continue };
        let Ok(sp) = disp.cast::<IServiceProvider>() else { continue };
        let Ok(browser) = sp.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) else { continue };
        if browser.GetWindow().map(|h| GetAncestor(h, GA_ROOT) == root).unwrap_or(false) {
            return Some(browser);
        }
    }
    None
}

fn parent_dir(target: &str) -> &str {
    target.rfind(SEP).map(|i| &target[..i]).unwrap_or(target)
}

/// Take the current Explorer window to the file's folder and select the file there.
unsafe fn go_to(hwnd: HWND, s: &mut Song, target: &str) {
    let folder = ILCreateFromPathW(&HSTRING::from(parent_dir(target)));
    let went = !folder.is_null()
        && browser_for(hwnd).map(|b| b.BrowseObject(folder, SBSP_SAMEBROWSER | SBSP_ABSOLUTE).is_ok()).unwrap_or(false);
    ILFree(Some(folder as *const _));
    if went {
        s.pending = Some((target.to_string(), 0));
        SetTimer(hwnd, SELECT_TIMER, 250, None);
    } else {
        reveal(target); // not attached to an Explorer window: open a new one
    }
}

/// Navigation is asynchronous: keep trying until the window shows the folder, then select.
unsafe fn select_pending(hwnd: HWND, s: &mut Song) {
    let Some((target, tries)) = s.pending.take() else {
        let _ = KillTimer(hwnd, SELECT_TIMER);
        return;
    };
    let mut done = tries >= 20;
    if let Some(view) = browser_for(hwnd).and_then(|b| b.QueryActiveShellView().ok()) {
        let folder = ILCreateFromPathW(&HSTRING::from(parent_dir(&target)));
        let item = ILCreateFromPathW(&HSTRING::from(target.as_str()));
        let here = view.cast::<IFolderView>().ok()
            .and_then(|fv| fv.GetFolder::<IPersistFolder2>().ok())
            .and_then(|pf| pf.GetCurFolder().ok());
        if let Some(cur) = here {
            if !folder.is_null() && !item.is_null() && ILIsEqual(cur, folder).as_bool() {
                let flags = (SVSI_SELECT.0 | SVSI_DESELECTOTHERS.0 | SVSI_ENSUREVISIBLE.0 | SVSI_FOCUSED.0) as u32;
                done = view.SelectItem(ILFindLastID(item), flags).is_ok() || done;
            }
            CoTaskMemFree(Some(cur as *const c_void));
        }
        ILFree(Some(folder as *const _));
        ILFree(Some(item as *const _));
    }
    if done {
        let _ = KillTimer(hwnd, SELECT_TIMER);
    } else {
        s.pending = Some((target, tries + 1));
    }
}

// ---------- window ----------

unsafe fn child(parent: HWND, class: PCWSTR, text: &str, style: u32, ex: u32, id: i32, font: HFONT) -> HWND {
    let h = CreateWindowExW(
        WINDOW_EX_STYLE(ex),
        class,
        &HSTRING::from(text),
        WINDOW_STYLE(style) | WS_CHILD | WS_VISIBLE,
        0, 0, 0, 0,
        parent,
        HMENU(id as isize as *mut c_void),
        HINSTANCE::default(),
        None,
    )
    .unwrap_or_default();
    SendMessageW(h, WM_SETFONT, WPARAM(font.0 as usize), LPARAM(1));
    h
}

unsafe fn dark_scrollbars(h: HWND) {
    let _ = SetWindowTheme(h, w!("DarkMode_Explorer"), PCWSTR::null());
}

unsafe fn button(parent: HWND, text: &str, id: i32, font: HFONT) -> HWND {
    child(parent, w!("BUTTON"), text, BS_OWNERDRAW | WS_TABSTOP.0, 0, id, font)
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

unsafe fn table(hwnd: HWND, id: i32, s: &Song) {
    let lv = child(hwnd, w!("SysListView32"), "", LVS_REPORT | LVS_SINGLESEL | LVS_SHOWSELALWAYS | WS_TABSTOP.0, 0, id, s.font);
    // full-row select, header drag-drop, double buffering
    SendMessageW(lv, LVM_SETEXTENDEDLISTVIEWSTYLE, WPARAM(0), LPARAM(0x20 | 0x10 | 0x10000));
    SendMessageW(lv, LVM_SETBKCOLOR, WPARAM(0), LPARAM(C_FIELD as isize));
    SendMessageW(lv, LVM_SETTEXTBKCOLOR, WPARAM(0), LPARAM(C_FIELD as isize));
    SendMessageW(lv, LVM_SETTEXTCOLOR, WPARAM(0), LPARAM(C_TEXT as isize));
    dark_scrollbars(lv);
    let _ = SetWindowSubclass(lv, Some(table_proc), 1, 0);
    for (i, (title, width, right)) in COLUMNS.iter().enumerate() {
        let mut text = wide(title);
        let col = LvColumn {
            mask: 1 | 2 | 4 | 8,
            fmt: if *right { 1 } else { 0 },
            cx: width * s.dpi / 96,
            text: text.as_mut_ptr(),
            cch: 0, sub: i as i32, image: 0, order: 0, cx_min: 0, cx_default: 0, cx_ideal: 0,
        };
        SendMessageW(lv, LVM_INSERTCOLUMNW, WPARAM(i), LPARAM(&col as *const LvColumn as isize));
    }
}

/// Paints the table header dark (the common control otherwise insists on white).
unsafe extern "system" fn table_proc(lv: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    if msg == WM_NOTIFY && lp.0 != 0 {
        let cd = &*(lp.0 as *const NmCustomDraw);
        let header = SendMessageW(lv, LVM_GETHEADER, WPARAM(0), LPARAM(0)).0 as *mut c_void;
        if cd.hdr.code == NM_CUSTOMDRAW && cd.hdr.from == header {
            let hdc = HDC(cd.hdc);
            match cd.stage {
                CDDS_PREPAINT => {
                    let brush = CreateSolidBrush(COLORREF(C_BG));
                    FillRect(hdc, &cd.rc, brush);
                    let _ = DeleteObject(brush);
                    return LRESULT(CDRF_NOTIFYITEMDRAW);
                }
                CDDS_ITEMPREPAINT => {
                    let brush = CreateSolidBrush(COLORREF(C_BTN));
                    let mut rc = cd.rc;
                    rc.right -= 1;
                    FillRect(hdc, &rc, brush);
                    let _ = DeleteObject(brush);
                    let mut text = vec![0u16; 64];
                    let mut item = HdItem {
                        mask: 2 | 4, cxy: 0, text: text.as_mut_ptr(), hbm: 0, cch: 64, fmt: 0, lparam: 0,
                        image: 0, order: 0, kind: 0, filter: std::ptr::null_mut(), state: 0,
                    };
                    SendMessageW(HWND(header), HDM_GETITEMW, WPARAM(cd.item), LPARAM(&mut item as *mut HdItem as isize));
                    let n = text.iter().position(|&c| c == 0).unwrap_or(0);
                    SetBkMode(hdc, TRANSPARENT);
                    SetTextColor(hdc, COLORREF(C_DIM));
                    rc.left += 6;
                    rc.right -= 6;
                    let align = if item.fmt & 1 == 1 { DT_RIGHT } else { DT_LEFT };
                    DrawTextW(hdc, &mut text[..n], &mut rc, align | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
                    return LRESULT(CDRF_SKIPDEFAULT);
                }
                _ => {}
            }
        }
    }
    DefSubclassProc(lv, msg, wp, lp)
}

unsafe fn set_cell(lv: HWND, item: usize, col: usize, text: &str, insert: bool) {
    let mut text = wide(text);
    let it = LvItem {
        mask: 1, item: item as i32, sub: col as i32, state: 0, state_mask: 0, text: text.as_mut_ptr(), cch: 0,
        image: 0, lparam: 0, indent: 0, group_id: 0, c_columns: 0,
        pu_columns: std::ptr::null_mut(), pi_col_fmt: std::ptr::null_mut(), group: 0,
    };
    let lp = LPARAM(&it as *const LvItem as isize);
    if insert {
        SendMessageW(lv, LVM_INSERTITEMW, WPARAM(0), lp);
    } else {
        SendMessageW(lv, LVM_SETITEMTEXTW, WPARAM(item), lp);
    }
}

unsafe fn fill_table(hwnd: HWND, id: i32, rows: &[Row]) {
    let Ok(lv) = GetDlgItem(hwnd, id) else { return };
    SendMessageW(lv, WM_SETREDRAW, WPARAM(0), LPARAM(0));
    SendMessageW(lv, LVM_DELETEALLITEMS, WPARAM(0), LPARAM(0));
    for (i, row) in rows.iter().enumerate() {
        for col in 0..COLUMNS.len() {
            set_cell(lv, i, col, &row.cell(col), col == 0);
        }
    }
    SendMessageW(lv, WM_SETREDRAW, WPARAM(1), LPARAM(0));
    let _ = InvalidateRect(lv, None, true);
}

// ----- library tree: year / month / song / files -----

unsafe fn tree_insert(tv: HWND, parent: isize, text: &str, lparam: isize, has_children: bool) -> isize {
    let mut text = wide(text);
    let ins = TvInsert {
        parent,
        after: TVI_LAST,
        item: TvItem {
            mask: 1 | 4 | 0x40, // TVIF_TEXT, TVIF_PARAM, TVIF_CHILDREN
            item: 0, state: 0, state_mask: 0, text: text.as_mut_ptr(), cch: 0, image: 0, selected_image: 0,
            children: has_children as i32, lparam, integral: 0, state_ex: 0, hwnd: 0, expanded_image: 0, reserved: 0,
        },
    };
    SendMessageW(tv, TVM_INSERTITEMW, WPARAM(0), LPARAM(&ins as *const TvInsert as isize)).0
}

fn month_name(m: &str) -> &'static str {
    ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"]
        .get(m.parse::<usize>().unwrap_or(0).wrapping_sub(1))
        .copied()
        .unwrap_or("?")
}

/// Every indexed project, grouped by the song's creation date (file date when FL stored none).
unsafe fn fill_tree(hwnd: HWND, s: &mut Song) {
    let Ok(tv) = GetDlgItem(hwnd, ID_TREE) else { return };
    s.library = query(
        &format!("SELECT {ROW_COLS} {ROW_JOIN} WHERE f.is_autosave = 0 AND ?1 = '' ORDER BY f.mtime DESC"),
        "",
    )
    .iter()
    .map(|r| row_from(r))
    .filter(|r| !(s.hide_demos && r.demo))
    .collect();
    // song -> its date (earliest creation date, else earliest file date) and file indexes
    let mut songs: BTreeMap<String, (String, bool, Vec<usize>)> = BTreeMap::new();
    for (i, row) in s.library.iter().enumerate() {
        let e = songs.entry(row.song.clone()).or_insert_with(|| (String::new(), false, Vec::new()));
        let (d, real) = if row.created.is_empty() { (date(row.modified), false) } else { (row.created.clone(), true) };
        if (real && !e.1) || (real == e.1 && (e.0.is_empty() || d < e.0)) {
            e.0 = d;
            e.1 = real;
        }
        e.2.push(i);
    }
    // year -> month -> songs (newest first)
    let mut years: BTreeMap<String, BTreeMap<String, Vec<(String, String, bool, Vec<usize>)>>> = BTreeMap::new();
    for (key, (d, real, files)) in songs {
        let (y, m) = (d.get(0..4).unwrap_or("????").to_string(), d.get(5..7).unwrap_or("??").to_string());
        years.entry(y).or_default().entry(m).or_default().push((d, key, real, files));
    }
    SendMessageW(tv, WM_SETREDRAW, WPARAM(0), LPARAM(0));
    SendMessageW(tv, TVM_DELETEITEM, WPARAM(0), LPARAM(TVI_ROOT));
    for (y, months) in years.iter().rev() {
        let n: usize = months.values().map(|v| v.len()).sum();
        let hy = tree_insert(tv, TVI_ROOT, &format!("{y}      {n} songs"), 0, true);
        for (m, list) in months.iter().rev() {
            let hm = tree_insert(tv, hy, &format!("{}  {}      {} song{}", m, month_name(m), list.len(), if list.len() == 1 { "" } else { "s" }), 0, true);
            let mut list = list.clone();
            list.sort_by(|a, b| b.0.cmp(&a.0));
            for (d, _key, real, files) in &list {
                let newest = &s.library[files[0]];
                let folder = newest.dir.rsplit(SEP).next().unwrap_or("");
                let name = if newest.song_name.is_empty() { folder.to_string() } else { newest.song_name.clone() };
                let mark = if *real { "" } else { " (file date)" };
                let label = format!("{name}      {folder}      {d}{mark}      {} file{}", files.len(), if files.len() == 1 { "" } else { "s" });
                let hs = tree_insert(tv, hm, &label, -(files[0] as isize + 1), true); // negative: colour like its files
                for &i in files {
                    let r = &s.library[i];
                    let mut line = format!("{}      {}", date(r.modified), r.name);
                    if !r.version.is_empty() {
                        line.push_str(&format!("      FL {}", r.version));
                    }
                    if r.bpm > 0.0 {
                        line.push_str(&format!("      {} bpm", r.bpm.round() as u32));
                    }
                    if !r.status.is_empty() {
                        line.push_str(&format!("      [{}]", r.status));
                    }
                    if r.bookmark {
                        line.push_str("      \u{2605}");
                    }
                    line.push_str(&format!("      {}", r.dir));
                    tree_insert(tv, hs, &line, i as isize + 1, false);
                }
            }
        }
    }
    SendMessageW(tv, WM_SETREDRAW, WPARAM(1), LPARAM(0));
    let _ = InvalidateRect(tv, None, true);
    s.tree_loaded = true;
}

/// The row selected in whichever list is showing: the library tree, or the results / versions table.
unsafe fn selected_row(hwnd: HWND, s: &Song, versions: bool) -> Option<Row> {
    if !versions && s.mode == Mode::Library {
        return tree_row(hwnd, s).cloned();
    }
    let (id, rows) = if versions { (ID_VERSIONS, &s.versions) } else { (ID_RESULTS, &s.results) };
    let lv = GetDlgItem(hwnd, id).ok()?;
    let i = SendMessageW(lv, LVM_GETNEXTITEM, WPARAM(usize::MAX), LPARAM(LVNI_SELECTED)).0;
    usize::try_from(i).ok().and_then(|i| rows.get(i)).cloned()
}

/// Header text: the opened project / folder normally, the selected row in Library / Search / Bookmarks.
unsafe fn update_header(hwnd: HWND, s: &Song, row: Option<&Row>) {
    let (title, info) = match (s.mode, row) {
        (Mode::Inside, _) => (s.name.clone(), s.info.clone()),
        (_, Some(r)) => {
            let song = if r.song_name.is_empty() { r.name.clone() } else { format!("{}  \u{2014}  {}", r.song_name, r.name) };
            let mut parts = Vec::new();
            if !r.version.is_empty() {
                parts.push(format!("FL {}", r.version));
            }
            if r.bpm > 0.0 {
                parts.push(format!("{} BPM", r.bpm.round() as u32));
            }
            if !r.created.is_empty() {
                parts.push(format!("created {}", r.created));
            }
            if r.modified > 0 {
                parts.push(format!("modified {}", date(r.modified)));
            }
            if !r.status.is_empty() {
                parts.push(r.status.clone());
            }
            (song, format!("{}\r\n{}", parts.join(DOT), r.dir))
        }
        (mode, None) => {
            let what = match mode { Mode::Library => "Library", Mode::Bookmarks => "Bookmarks", _ => "Search" };
            (what.to_string(), "Select a project below to see it here".to_string())
        }
    };
    set_text(hwnd, ID_TITLE, &title);
    set_text(hwnd, ID_INFO, &info);
}

/// Enable the selection buttons and name the FL version the selected project would open in.
unsafe fn update_selection_buttons(hwnd: HWND, s: &Song) {
    let row = selected_row(hwnd, s, false);
    update_header(hwnd, s, if s.mode == Mode::Inside && s.folder { None } else { row.as_ref() });
    for id in [ID_SEL_OPEN, ID_SEL_FOLDER] {
        if let Ok(b) = GetDlgItem(hwnd, id) {
            let _ = EnableWindow(b, row.is_some());
        }
    }
    let orig = row.as_ref().and_then(|r| fl_for(&r.version));
    if let Ok(b) = GetDlgItem(hwnd, ID_SEL_ORIG) {
        let label = match &orig {
            Some((m, _)) => format!("Open in FL {m}"),
            None => "Open in original FL".to_string(),
        };
        let _ = SetWindowTextW(b, &HSTRING::from(label));
        let _ = EnableWindow(b, orig.is_some());
    }
}

unsafe fn tree_row<'a>(hwnd: HWND, s: &'a Song) -> Option<&'a Row> {
    let tv = GetDlgItem(hwnd, ID_TREE).ok()?;
    let h = SendMessageW(tv, TVM_GETNEXTITEM, WPARAM(TVGN_CARET), LPARAM(0)).0;
    if h == 0 {
        return None;
    }
    let mut it = TvItem {
        mask: 4 /* TVIF_PARAM */, item: h, state: 0, state_mask: 0, text: std::ptr::null_mut(), cch: 0, image: 0, selected_image: 0,
        children: 0, lparam: 0, integral: 0, state_ex: 0, hwnd: 0, expanded_image: 0, reserved: 0,
    };
    SendMessageW(tv, TVM_GETITEMW, WPARAM(0), LPARAM(&mut it as *mut TvItem as isize));
    // file lines store index + 1; song lines store -(index of their newest file + 1)
    let i = if it.lparam > 0 { it.lparam - 1 } else { -it.lparam - 1 };
    usize::try_from(i).ok().and_then(|i| s.library.get(i))
}

unsafe fn set_text(hwnd: HWND, id: i32, text: &str) {
    if let Ok(c) = GetDlgItem(hwnd, id) {
        let _ = SetWindowTextW(c, &HSTRING::from(text));
    }
}

unsafe fn redraw(hwnd: HWND, id: i32) {
    if let Ok(c) = GetDlgItem(hwnd, id) {
        let _ = InvalidateRect(c, None, true);
    }
}

unsafe fn build(hwnd: HWND, s: &Song) {
    let tab = WS_TABSTOP.0;
    child(hwnd, w!("STATIC"), "SEARCH", 0, 0, ID_SEARCH_LABEL, s.small);
    let e = child(hwnd, w!("EDIT"), "", ES_AUTOHSCROLL | tab, 0, ID_SEARCH, s.font);
    dark_scrollbars(e);
    button(hwnd, "Library", ID_LIBRARY, s.font);
    button(hwnd, "\u{2605} Bookmarks", ID_BOOKMARKS, s.font);
    button(hwnd, "Auto-show", ID_AUTOSHOW, s.font);
    button(hwnd, "Hide demos", ID_HIDEDEMOS, s.font);
    for (id, label) in [(ID_SEL_OPEN, "Open selected"), (ID_SEL_ORIG, "Open in original FL"), (ID_SEL_FOLDER, "Show in folder")] {
        let b = button(hwnd, label, id, s.font);
        let _ = EnableWindow(b, false); // until a row is selected
    }
    child(hwnd, w!("STATIC"), &s.name, SS_NOPREFIX | SS_ENDELLIPSIS, 0, ID_TITLE, s.bold);
    child(hwnd, w!("STATIC"), &s.info, SS_NOPREFIX, 0, ID_INFO, s.font);
    child(hwnd, w!("STATIC"), "", SS_NOPREFIX | SS_ENDELLIPSIS, 0, ID_RESULTS_LABEL, s.small);
    table(hwnd, ID_RESULTS, s);
    let tv = child(hwnd, w!("SysTreeView32"), "", TVS_HASBUTTONS | TVS_HASLINES | TVS_LINESATROOT | TVS_SHOWSELALWAYS | tab, 0, ID_TREE, s.font);
    SendMessageW(tv, TVM_SETBKCOLOR, WPARAM(0), LPARAM(C_FIELD as isize));
    SendMessageW(tv, TVM_SETTEXTCOLOR, WPARAM(0), LPARAM(C_TEXT as isize));
    SendMessageW(tv, TVM_SETLINECOLOR, WPARAM(0), LPARAM(C_BORDER as isize));
    dark_scrollbars(tv);
    if s.folder {
        button(hwnd, "Colour folders", ID_COLORIZE, s.font);
        return;
    }
    child(hwnd, w!("STATIC"), &s.plugins, SS_NOPREFIX | SS_ENDELLIPSIS, 0, ID_PLUGINS, s.font);
    for (i, name) in STATUSES.iter().enumerate() {
        button(hwnd, name, ID_STATUS + i as i32, s.font);
    }
    for (i, name) in RATINGS.iter().enumerate() {
        button(hwnd, name, ID_RATING + i as i32, s.font);
    }
    child(hwnd, w!("STATIC"), "TAGS", 0, 0, ID_TAGS_LABEL, s.small);
    let e = child(hwnd, w!("EDIT"), &s.tags, ES_AUTOHSCROLL | tab, 0, ID_TAGS, s.font);
    dark_scrollbars(e);
    button(hwnd, "Save", ID_SAVE, s.font);
    let e = child(hwnd, w!("EDIT"), &s.notes, ES_MULTILINE | ES_AUTOVSCROLL | ES_WANTRETURN | WS_VSCROLL.0 | tab, 0, ID_NOTES, s.font);
    dark_scrollbars(e);
    button(hwnd, "Open (newest FL)", ID_OPEN, s.font);
    let orig = fl_for(&s.version);
    let label = match &orig {
        Some((m, _)) => format!("Open in FL {m}"),
        None => "Open in original".to_string(),
    };
    let b = button(hwnd, &label, ID_OPEN_ORIG, s.font);
    if orig.is_none() {
        let _ = EnableWindow(b, false);
    }
    let play = button(hwnd, "Play render", ID_PLAY, s.font);
    if s.render.is_none() {
        let _ = EnableWindow(play, false);
    }
    button(hwnd, "All copies", ID_FIND, s.font);
    button(hwnd, "Folder", ID_FOLDER, s.font);
    button(hwnd, "\u{2605} Bookmark", ID_BOOKMARK, s.font);
    child(hwnd, w!("STATIC"), &s.sample_summary, SS_NOPREFIX | SS_ENDELLIPSIS, 0, ID_SAMPLES_LABEL, s.small);
    let samples = child(hwnd, w!("LISTBOX"), "", LBS_NOINTEGRALHEIGHT | WS_VSCROLL.0 | WS_HSCROLL.0 | tab, 0, ID_SAMPLES, s.font);
    dark_scrollbars(samples);
    for line in &s.samples {
        SendMessageW(samples, LB_ADDSTRING, WPARAM(0), LPARAM(HSTRING::from(line.as_str()).as_ptr() as isize));
    }
    SendMessageW(samples, LB_SETHORIZONTALEXTENT, WPARAM(2000), LPARAM(0));
    let label = format!("VERSIONS ({}){DOT}{HINT}", s.versions.len());
    child(hwnd, w!("STATIC"), &label, SS_NOPREFIX | SS_ENDELLIPSIS, 0, ID_VERSIONS_LABEL, s.small);
    table(hwnd, ID_VERSIONS, s);
    fill_table(hwnd, ID_VERSIONS, &s.versions);
}

/// Put the right content in the results area: search hits, bookmarks, library tree, or the folder's projects.
unsafe fn show_results(hwnd: HWND, s: &mut Song) {
    let text = text_of(hwnd, ID_SEARCH).trim().to_string();
    let label;
    if !text.is_empty() {
        // Everything + per-hit library lookups can take seconds: run them on a worker thread
        s.mode = Mode::Search;
        s.search_gen += 1;
        let (gen, target) = (s.search_gen, hwnd.0 as isize);
        std::thread::spawn(move || {
            let found = Box::into_raw(Box::new((gen, search(&text))));
            if PostMessageW(HWND(target as *mut c_void), WM_SEARCH_DONE, WPARAM(0), LPARAM(found as isize)).is_err() {
                drop(Box::from_raw(found)); // panel closed meanwhile
            }
        });
        s.results.clear();
        fill_table(hwnd, ID_RESULTS, &s.results);
        update_selection_buttons(hwnd, s);
        set_text(hwnd, ID_RESULTS_LABEL, "SEARCHING EVERYTHING\u{2026}");
        layout(hwnd, s);
        return;
    } else if s.want_library {
        s.mode = Mode::Library;
        if !s.tree_loaded {
            fill_tree(hwnd, s);
        }
        label = format!("LIBRARY{DOT}{} files by creation year / month / song", s.library.len());
    } else if s.want_bookmarks {
        s.mode = Mode::Bookmarks;
        s.results = bookmarks();
        label = format!("BOOKMARKS{DOT}{} song{}", s.results.len(), if s.results.len() == 1 { "" } else { "s" });
    } else {
        s.mode = Mode::Inside;
        s.results = s.inside.iter().filter(|r| !(s.hide_demos && r.demo)).cloned().collect();
        let hidden = s.inside.len() - s.results.len();
        label = format!("IN THIS FOLDER{DOT}{} project file{}{}", s.results.len(), if s.results.len() == 1 { "" } else { "s" },
                        if hidden > 0 { format!(" ({hidden} demos hidden)") } else { String::new() });
    }
    if s.mode != Mode::Library {
        let (col, asc) = s.sort_results;
        sort_rows(&mut s.results, col, asc);
        fill_table(hwnd, ID_RESULTS, &s.results);
        update_selection_buttons(hwnd, s);
    }
    let hint = if s.mode == Mode::Library { HINT.rsplit_once(DOT).map(|(h, _)| h).unwrap_or(HINT) } else { HINT };
    set_text(hwnd, ID_RESULTS_LABEL, &format!("{label}{DOT}{hint}"));
    layout(hwnd, s);
    redraw(hwnd, ID_LIBRARY);
    redraw(hwnd, ID_BOOKMARKS);
    redraw(hwnd, ID_HIDEDEMOS);
    update_selection_buttons(hwnd, s);
}

/// Tint table rows and tree nodes by their project folder.
unsafe fn custom_draw(s: &Song, id: i32, cd: &mut NmColorDraw) -> LRESULT {
    if cd.cd.stage == CDDS_PREPAINT {
        return LRESULT(CDRF_NOTIFYITEMDRAW);
    }
    if cd.cd.stage != CDDS_ITEMPREPAINT {
        return LRESULT(CDRF_DODEFAULT);
    }
    let row = if id == ID_TREE {
        let lp = cd.cd.lparam;
        let i = if lp > 0 { lp - 1 } else { -lp - 1 };
        if lp == 0 { None } else { s.library.get(i as usize) }
    } else {
        let rows = if id == ID_VERSIONS { &s.versions } else { &s.results };
        rows.get(cd.cd.item)
    };
    if let Some(row) = row {
        cd.clr_text = row.color.0;
        cd.clr_text_bk = row.color.1;
    }
    LRESULT(CDRF_DODEFAULT)
}

unsafe fn place(hwnd: HWND, id: i32, x: i32, y: i32, w: i32, h: i32, show: bool) {
    if let Ok(c) = GetDlgItem(hwnd, id) {
        let _ = MoveWindow(c, x, y, w.max(0), h.max(0), true);
        let _ = ShowWindow(c, if show { SW_SHOW } else { SW_HIDE });
    }
}

unsafe fn layout(hwnd: HWND, s: &Song) {
    let px = |v: i32| v * s.dpi / 96;
    let mut rc = RECT::default();
    let _ = GetClientRect(hwnd, &mut rc);
    let m = px(12);
    let w = (rc.right - 2 * m).max(px(120));
    let gap = px(4);
    let mut y = m;
    let (bl, bb, ba, bd) = (px(66), px(96), px(80), px(88));
    let buttons = bl + bb + ba + bd + 3 * gap;
    // narrow pane: the toggle buttons get their own row instead of squeezing the search box out
    let narrow = w - px(54) - buttons - gap < px(160);
    place(hwnd, ID_SEARCH_LABEL, m, y + px(5), px(52), px(16), true);
    let search_w = if narrow { w - px(54) } else { w - px(54) - buttons - gap };
    place(hwnd, ID_SEARCH, m + px(54), y, search_w, px(24), true);
    let (mut x, by) = if narrow { (m, y + px(30)) } else { (m + w - buttons, y) };
    let scale = if narrow && buttons > w { w as f32 / buttons as f32 } else { 1.0 };
    for (id, bw) in [(ID_LIBRARY, bl), (ID_BOOKMARKS, bb), (ID_HIDEDEMOS, bd), (ID_AUTOSHOW, ba)] {
        let bw = (bw as f32 * scale) as i32;
        place(hwnd, id, x, by, bw, px(24), true);
        x += bw + gap;
    }
    y += px(if narrow { 66 } else { 36 });
    place(hwnd, ID_TITLE, m, y, w, px(26), true);
    y += px(30);
    // project controls only belong to the project this view was opened on; in Library / Search /
    // Bookmarks the header describes the selected row instead, and the controls step aside
    let details = !s.folder && s.mode == Mode::Inside;
    if s.folder {
        place(hwnd, ID_INFO, m, y + px(4), w - px(130), px(18), true);
        place(hwnd, ID_COLORIZE, m + w - px(124), y, px(124), px(24), s.mode == Mode::Inside);
        y += px(32);
    } else {
        place(hwnd, ID_INFO, m, y, w, px(34), true);
        y += px(40);
    }
    if !s.folder && !details {
        let ids = [ID_PLUGINS, ID_TAGS_LABEL, ID_TAGS, ID_SAVE, ID_NOTES, ID_OPEN, ID_OPEN_ORIG, ID_PLAY, ID_FIND, ID_FOLDER, ID_BOOKMARK];
        for id in ids.into_iter().chain(ID_STATUS..ID_STATUS + 6).chain(ID_RATING..ID_RATING + 6) {
            place(hwnd, id, 0, 0, 0, 0, false);
        }
    }
    if details {
        place(hwnd, ID_PLUGINS, m, y, w, px(18), true);
        y += px(26);
        let bw = (w - 5 * gap) / 6;
        for i in 0..6 {
            place(hwnd, ID_STATUS + i, m + i * (bw + gap), y, bw, px(26), true);
        }
        y += px(26) + gap;
        for i in 0..6 {
            place(hwnd, ID_RATING + i, m + i * (bw + gap), y, bw, px(24), true);
        }
        y += px(34);
        place(hwnd, ID_TAGS_LABEL, m, y + px(5), px(40), px(16), true);
        place(hwnd, ID_TAGS, m + px(44), y, w - px(44) - px(64), px(24), true);
        place(hwnd, ID_SAVE, m + w - px(60), y, px(60), px(24), true);
        y += px(30);
        place(hwnd, ID_NOTES, m, y, w, px(52), true);
        y += px(60);
        for (i, id) in [ID_OPEN, ID_OPEN_ORIG, ID_PLAY, ID_FIND, ID_FOLDER, ID_BOOKMARK].into_iter().enumerate() {
            place(hwnd, id, m + i as i32 * (bw + gap), y, bw, px(26), true);
        }
        y += px(36);
    }
    // bottom area: results table / library tree, or the project's samples + versions
    let results = s.folder || s.mode != Mode::Inside;
    let tree = results && s.mode == Mode::Library;
    let bottom = rc.bottom - m;
    place(hwnd, ID_RESULTS_LABEL, m, y, w, px(16), results);
    let bw3 = (w - 2 * gap) / 3;
    for (i, id) in [ID_SEL_OPEN, ID_SEL_ORIG, ID_SEL_FOLDER].into_iter().enumerate() {
        place(hwnd, id, m + i as i32 * (bw3 + gap), y + px(20), bw3, px(26), results);
    }
    let ly = y + px(20) + px(32);
    place(hwnd, ID_RESULTS, m, ly, w, bottom - ly, results && !tree);
    place(hwnd, ID_TREE, m, ly, w, bottom - ly, tree);
    if !s.folder {
        let rest = (bottom - y - 2 * px(20) - px(10)).max(px(80));
        let sh = rest * 35 / 100;
        place(hwnd, ID_SAMPLES_LABEL, m, y, w, px(16), !results);
        place(hwnd, ID_SAMPLES, m, y + px(20), w, sh, !results);
        let y2 = y + px(20) + sh + px(10);
        place(hwnd, ID_VERSIONS_LABEL, m, y2, w, px(16), !results);
        place(hwnd, ID_VERSIONS, m, y2 + px(20), w, rest - sh, !results);
    }
    let _ = InvalidateRect(hwnd, None, true); // moved controls leave stale paint behind otherwise
}

unsafe fn text_of(hwnd: HWND, id: i32) -> String {
    let Ok(c) = GetDlgItem(hwnd, id) else { return String::new() };
    let mut buf = vec![0u16; 8192];
    let n = GetWindowTextW(c, &mut buf).max(0) as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// Which owner-drawn buttons look "pressed in" (toggles and radio groups).
fn lit(s: &Song, id: i32) -> Option<u32> {
    match id {
        _ if (ID_STATUS..ID_STATUS + 5).contains(&id) => (STATUSES[(id - ID_STATUS) as usize] == s.status).then_some(STATUS_COLORS[(id - ID_STATUS) as usize]),
        _ if (ID_RATING..ID_RATING + 5).contains(&id) => (s.rating == (id - ID_RATING + 1) as u32).then_some(rgb(255, 200, 60)),
        ID_BOOKMARK => s.bookmark.then_some(C_ACCENT),
        ID_BOOKMARKS => s.want_bookmarks.then_some(C_ACCENT),
        ID_LIBRARY => s.want_library.then_some(C_ACCENT),
        ID_AUTOSHOW => s.autoshow.then_some(rgb(80, 205, 120)),
        ID_HIDEDEMOS => s.hide_demos.then_some(rgb(80, 205, 120)),
        _ => None,
    }
}

unsafe fn draw_button(s: &Song, di: &DrawItem) {
    let hdc = HDC(di.hdc);
    let id = di.ctl_id as i32;
    let lit = lit(s, id);
    let down = di.state & ODS_SELECTED != 0;
    let disabled = di.state & ODS_DISABLED != 0;
    let fill = lit.unwrap_or(if down { C_BTN_DOWN } else { C_BTN });
    let brush = CreateSolidBrush(COLORREF(fill));
    FillRect(hdc, &di.rc, brush);
    let _ = DeleteObject(brush);
    let border = CreateSolidBrush(COLORREF(if lit.is_some() { fill } else { C_BORDER }));
    FrameRect(hdc, &di.rc, border);
    let _ = DeleteObject(border);
    let mut text = vec![0u16; 64];
    let n = GetWindowTextW(HWND(di.hwnd), &mut text).max(0) as usize;
    SetBkMode(hdc, TRANSPARENT);
    SetTextColor(hdc, COLORREF(if lit.is_some() { C_ON_ACCENT } else if disabled { C_DIM } else { C_TEXT }));
    let mut rc = di.rc;
    DrawTextW(hdc, &mut text[..n], &mut rc, DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_END_ELLIPSIS);
}

unsafe fn command(hwnd: HWND, s: &mut Song, id: i32, code: u32) {
    match id {
        ID_SEARCH if code == EN_CHANGE => {
            SetTimer(hwnd, SEARCH_TIMER, 250, None); // wait for a pause in typing
        }
        ID_BOOKMARKS => {
            s.want_bookmarks = !s.want_bookmarks;
            s.want_library = false;
            show_results(hwnd, s);
        }
        ID_LIBRARY => {
            s.want_library = !s.want_library;
            s.want_bookmarks = false;
            show_results(hwnd, s);
        }
        ID_AUTOSHOW => {
            s.autoshow = !s.autoshow;
            set_autoshow(s.autoshow);
            redraw(hwnd, ID_AUTOSHOW);
        }
        ID_COLORIZE => flctl(&["colorfolders", &s.path]),
        ID_HIDEDEMOS => {
            s.hide_demos = !s.hide_demos;
            set_hide_demos(s.hide_demos);
            s.tree_loaded = false; // the library tree is built with the filter applied
            show_results(hwnd, s);
        }
        _ if (ID_STATUS..ID_STATUS + 6).contains(&id) => {
            let name = STATUSES[(id - ID_STATUS) as usize];
            s.status = if name == "none" { String::new() } else { name.to_string() };
            flctl(&["status", name, &s.path]);
            for i in 0..6 {
                redraw(hwnd, ID_STATUS + i);
            }
        }
        _ if (ID_RATING..ID_RATING + 6).contains(&id) => {
            s.rating = if id - ID_RATING == 5 { 0 } else { (id - ID_RATING + 1) as u32 };
            flctl(&["rating", &s.rating.to_string(), &s.path]);
            for i in 0..6 {
                redraw(hwnd, ID_RATING + i);
            }
        }
        ID_SAVE => flctl(&["setmeta", &s.path, &text_of(hwnd, ID_TAGS), &text_of(hwnd, ID_NOTES).replace("\r\n", "\n")]),
        ID_BOOKMARK => {
            s.bookmark = !s.bookmark;
            flctl(&["bookmark", if s.bookmark { "1" } else { "0" }, &s.path]);
            redraw(hwnd, ID_BOOKMARK);
        }
        ID_SEL_OPEN | ID_SEL_ORIG | ID_SEL_FOLDER => {
            if let Some(row) = selected_row(hwnd, s, false) {
                match id {
                    ID_SEL_OPEN => open_in_fl(&row.target),
                    ID_SEL_ORIG => open_original(&row.target, &row.version),
                    _ => reveal(&row.target),
                }
            }
        }
        // project buttons act on the version picked in the versions table, else this project
        ID_OPEN | ID_OPEN_ORIG | ID_FOLDER if selected_row(hwnd, s, true).is_some() => {
            let row = selected_row(hwnd, s, true).unwrap_or_default();
            match id {
                ID_OPEN => open_in_fl(&row.target),
                ID_OPEN_ORIG => open_original(&row.target, &row.version),
                _ => reveal(&row.target),
            }
        }
        ID_OPEN => open_in_fl(&s.path),
        ID_OPEN_ORIG => open_original(&s.path, &s.version),
        ID_PLAY => {
            if let Some(r) = &s.render {
                shell_open(r);
            }
        }
        ID_FIND => flctl(&["versions", &s.path]),
        ID_FOLDER => reveal(&s.path),
        _ => {}
    }
}

/// Open / jump to a row according to the modifier keys held.
unsafe fn activate(hwnd: HWND, s: &mut Song, target: &str, version: &str) {
    if GetAsyncKeyState(VK_CONTROL) < 0 {
        go_to(hwnd, s, target);
    } else if GetAsyncKeyState(VK_SHIFT) < 0 {
        open_original(target, version);
    } else {
        open_in_fl(target);
    }
}

unsafe fn notify(hwnd: HWND, s: &mut Song, nm: &NmListView) {
    let id = nm.hdr.id as i32;
    if (id == ID_TREE && nm.hdr.code == TVN_SELCHANGEDW) || (id == ID_RESULTS && nm.hdr.code == LVN_ITEMCHANGED) {
        update_selection_buttons(hwnd, s);
    }
    if id == ID_TREE {
        if nm.hdr.code == NM_DBLCLK {
            if let Some(row) = tree_row(hwnd, s) {
                let (target, version) = (row.target.clone(), row.version.clone());
                activate(hwnd, s, &target, &version);
            }
        }
        return;
    }
    let versions = id == ID_VERSIONS;
    if id != ID_RESULTS && !versions {
        return;
    }
    match nm.hdr.code {
        LVN_COLUMNCLICK => {
            let sort = if versions { &mut s.sort_versions } else { &mut s.sort_results };
            *sort = if sort.0 == nm.sub { (nm.sub, !sort.1) } else { (nm.sub, nm.sub != 6) };
            let (col, asc) = *sort;
            let rows = if versions { &mut s.versions } else { &mut s.results };
            sort_rows(rows, col, asc);
            fill_table(hwnd, id, rows);
        }
        NM_CLICK => {
            let rows = if versions { &mut s.versions } else { &mut s.results };
            let Some(row) = usize::try_from(nm.item).ok().and_then(|i| rows.get_mut(i)) else { return };
            if nm.sub == 0 {
                row.bookmark = !row.bookmark;
                flctl(&["bookmark", if row.bookmark { "1" } else { "0" }, &row.path]);
                if let Ok(lv) = GetDlgItem(hwnd, id) {
                    set_cell(lv, nm.item as usize, 0, &row.cell(0), false);
                }
            }
        }
        NM_DBLCLK => {
            let rows = if versions { &s.versions } else { &s.results };
            let Some(row) = usize::try_from(nm.item).ok().and_then(|i| rows.get(i)) else { return };
            let (target, version) = (row.target.clone(), row.version.clone());
            activate(hwnd, s, &target, &version);
        }
        _ => {}
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut Song;
    match msg {
        WM_SIZE if !state.is_null() => {
            layout(hwnd, &*state);
            LRESULT(0)
        }
        WM_COMMAND if !state.is_null() => {
            let (id, code) = ((wp.0 & 0xFFFF) as i32, ((wp.0 >> 16) & 0xFFFF) as u32);
            let _ = catch_unwind(AssertUnwindSafe(|| command(hwnd, &mut *state, id, code)));
            LRESULT(0)
        }
        WM_NOTIFY if !state.is_null() && lp.0 != 0 => {
            let hdr = &*(lp.0 as *const NmHdr);
            let id = hdr.id as i32;
            if hdr.code == NM_CUSTOMDRAW && (id == ID_RESULTS || id == ID_VERSIONS || id == ID_TREE) {
                return custom_draw(&*state, id, &mut *(lp.0 as *mut NmColorDraw));
            }
            let _ = catch_unwind(AssertUnwindSafe(|| notify(hwnd, &mut *state, &*(lp.0 as *const NmListView))));
            LRESULT(0)
        }
        WM_DRAWITEM if !state.is_null() && lp.0 != 0 => {
            draw_button(&*state, &*(lp.0 as *const DrawItem));
            LRESULT(1)
        }
        WM_SEARCH_DONE if lp.0 != 0 => {
            let found = Box::from_raw(lp.0 as *mut (u64, (String, Vec<Row>)));
            if !state.is_null() {
                let s = &mut *state;
                let (gen, (label, mut rows)) = *found;
                if gen == s.search_gen && s.mode == Mode::Search {
                    if s.hide_demos {
                        rows.retain(|r| !r.demo);
                    }
                    let (col, asc) = s.sort_results;
                    sort_rows(&mut rows, col, asc);
                    s.results = rows;
                    fill_table(hwnd, ID_RESULTS, &s.results);
                    update_selection_buttons(hwnd, s);
                    set_text(hwnd, ID_RESULTS_LABEL, &format!("SEARCH{DOT}{label}{DOT}{HINT}"));
                }
            }
            LRESULT(0)
        }
        WM_TIMER if !state.is_null() && wp.0 == SELECT_TIMER => {
            let _ = catch_unwind(AssertUnwindSafe(|| select_pending(hwnd, &mut *state)));
            LRESULT(0)
        }
        WM_TIMER if !state.is_null() && wp.0 == SEARCH_TIMER => {
            let _ = KillTimer(hwnd, SEARCH_TIMER);
            let _ = catch_unwind(AssertUnwindSafe(|| show_results(hwnd, &mut *state)));
            LRESULT(0)
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLORBTN if !state.is_null() => {
            let s = &*state;
            let hdc = HDC(wp.0 as *mut c_void);
            let id = GetDlgCtrlID(HWND(lp.0 as *mut c_void));
            let color = match id {
                ID_TITLE => C_TEXT,
                ID_SAMPLES_LABEL | ID_VERSIONS_LABEL | ID_RESULTS_LABEL | ID_SEARCH_LABEL | ID_TAGS_LABEL => C_ACCENT,
                _ => C_DIM,
            };
            SetBkColor(hdc, COLORREF(C_BG));
            SetTextColor(hdc, COLORREF(color));
            LRESULT(s.brush_bg.0 as isize)
        }
        WM_CTLCOLOREDIT | WM_CTLCOLORLISTBOX if !state.is_null() => {
            let s = &*state;
            let hdc = HDC(wp.0 as *mut c_void);
            SetBkColor(hdc, COLORREF(C_FIELD));
            SetTextColor(hdc, COLORREF(C_TEXT));
            LRESULT(s.brush_field.0 as isize)
        }
        WM_DESTROY => {
            if !state.is_null() {
                let _ = KillTimer(hwnd, SEARCH_TIMER);
                let _ = KillTimer(hwnd, SELECT_TIMER);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                let s = Box::from_raw(state);
                for obj in [s.font.0, s.bold.0, s.small.0, s.brush_bg.0, s.brush_field.0] {
                    let _ = DeleteObject(HGDIOBJ(obj));
                }
            }
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Build the panel as a child of `parent`.
///
/// # Safety
/// `parent` must be a live window owned by the calling thread.
pub unsafe fn create(parent: HWND, rc: RECT, path: &str) -> HWND {
    let hinst: HINSTANCE = GetModuleHandleW(None).map(Into::into).unwrap_or_default();
    let icc = INITCOMMONCONTROLSEX { dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32, dwICC: ICC_LISTVIEW_CLASSES | ICC_TREEVIEW_CLASSES };
    let _ = InitCommonControlsEx(&icc);
    let class = w!("FLLibraryPanel");
    let wc = WNDCLASSEXW {
        cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
        lpfnWndProc: Some(wndproc),
        hInstance: hinst,
        hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
        hbrBackground: CreateSolidBrush(COLORREF(C_BG)),
        lpszClassName: class,
        ..Default::default()
    };
    RegisterClassExW(&wc); // fails harmlessly when already registered
    let Ok(hwnd) = CreateWindowExW(
        WS_EX_CONTROLPARENT,
        class,
        PCWSTR::null(),
        WS_CHILD | WS_VISIBLE | WS_CLIPCHILDREN,
        rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top,
        parent,
        None,
        hinst,
        None,
    ) else {
        return HWND::default();
    };
    let mut song = Box::new(load(path));
    song.dpi = (GetDpiForWindow(hwnd) as i32).max(96);
    let px = |v: i32| v * song.dpi / 96;
    song.font = CreateFontW(-px(13), 0, 0, 0, 400, 0, 0, 0, 1, 0, 0, 5, 0, w!("Segoe UI"));
    song.bold = CreateFontW(-px(18), 0, 0, 0, 700, 0, 0, 0, 1, 0, 0, 5, 0, w!("Segoe UI"));
    song.small = CreateFontW(-px(11), 0, 0, 0, 700, 0, 0, 0, 1, 0, 0, 5, 0, w!("Segoe UI"));
    song.brush_bg = CreateSolidBrush(COLORREF(C_BG));
    song.brush_field = CreateSolidBrush(COLORREF(C_FIELD));
    // state must be reachable before the first paint, or controls come up in default colours
    let state = Box::into_raw(song);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
    build(hwnd, &*state);
    show_results(hwnd, &mut *state);
    hwnd
}

// ---------- COM (preview handler) ----------

#[implement(IPreviewHandler, IInitializeWithFile, IInitializeWithItem, IObjectWithSite, IOleWindow)]
pub struct Panel {
    path: RefCell<String>,
    parent: Cell<HWND>,
    rect: Cell<RECT>,
    hwnd: Cell<HWND>,
    site: RefCell<Option<IUnknown>>,
}

impl Panel {
    pub fn new() -> Self {
        Panel {
            path: RefCell::new(String::new()),
            parent: Cell::new(HWND::default()),
            rect: Cell::new(RECT::default()),
            hwnd: Cell::new(HWND::default()),
            site: RefCell::new(None),
        }
    }
}

impl Default for Panel {
    fn default() -> Self {
        Self::new()
    }
}

impl Panel_Impl {
    fn fit(&self) {
        let (h, rc) = (self.hwnd.get(), self.rect.get());
        if !h.is_invalid() {
            unsafe {
                let _ = MoveWindow(h, rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top, true);
            }
        }
    }
}

impl IInitializeWithFile_Impl for Panel_Impl {
    fn Initialize(&self, pszfilepath: &PCWSTR, _grfmode: u32) -> Result<()> {
        *self.path.borrow_mut() = unsafe { pszfilepath.to_string() }.map_err(|_| Error::from(E_INVALIDARG))?;
        Ok(())
    }
}

impl IInitializeWithItem_Impl for Panel_Impl {
    fn Initialize(&self, psi: Option<&IShellItem>, _grfmode: u32) -> Result<()> {
        let item = psi.ok_or_else(|| Error::from(E_INVALIDARG))?;
        unsafe {
            let name = item.GetDisplayName(SIGDN_FILESYSPATH)?;
            let path = name.to_string();
            CoTaskMemFree(Some(name.0 as *const c_void));
            *self.path.borrow_mut() = path.map_err(|_| Error::from(E_INVALIDARG))?;
        }
        Ok(())
    }
}

impl IPreviewHandler_Impl for Panel_Impl {
    fn SetWindow(&self, hwnd: HWND, prc: *const RECT) -> Result<()> {
        self.parent.set(hwnd);
        if !prc.is_null() {
            self.rect.set(unsafe { *prc });
        }
        let h = self.hwnd.get();
        if !h.is_invalid() {
            unsafe {
                let _ = SetParent(h, hwnd);
            }
            self.fit();
        }
        Ok(())
    }
    fn SetRect(&self, prc: *const RECT) -> Result<()> {
        if !prc.is_null() {
            self.rect.set(unsafe { *prc });
        }
        self.fit();
        Ok(())
    }
    fn DoPreview(&self) -> Result<()> {
        if !self.hwnd.get().is_invalid() || self.parent.get().is_invalid() {
            return Ok(());
        }
        let (parent, rc, path) = (self.parent.get(), self.rect.get(), self.path.borrow().clone());
        // a panic must never unwind into the preview host
        let hwnd = catch_unwind(AssertUnwindSafe(|| unsafe { create(parent, rc, &path) })).unwrap_or_default();
        self.hwnd.set(hwnd);
        if hwnd.is_invalid() { Err(Error::from(E_FAIL)) } else { Ok(()) }
    }
    fn Unload(&self) -> Result<()> {
        let h = self.hwnd.replace(HWND::default());
        if !h.is_invalid() {
            unsafe {
                let _ = DestroyWindow(h);
            }
        }
        Ok(())
    }
    fn SetFocus(&self) -> Result<()> {
        let h = self.hwnd.get();
        if !h.is_invalid() {
            unsafe {
                let _ = SetFocus(h);
            }
        }
        Ok(())
    }
    fn QueryFocus(&self) -> Result<HWND> {
        let h = unsafe { GetFocus() };
        if h.is_invalid() { Err(Error::from(E_FAIL)) } else { Ok(h) }
    }
    fn TranslateAccelerator(&self, _pmsg: *const MSG) -> Result<()> {
        Err(Error::from(S_FALSE))
    }
}

impl IObjectWithSite_Impl for Panel_Impl {
    fn SetSite(&self, punksite: Option<&IUnknown>) -> Result<()> {
        *self.site.borrow_mut() = punksite.cloned();
        Ok(())
    }
    fn GetSite(&self, riid: *const GUID, ppvsite: *mut *mut c_void) -> Result<()> {
        match self.site.borrow().as_ref() {
            Some(site) => unsafe { site.query(riid, ppvsite).ok() },
            None => Err(Error::from(E_FAIL)),
        }
    }
}

impl IOleWindow_Impl for Panel_Impl {
    fn GetWindow(&self) -> Result<HWND> {
        Ok(self.parent.get())
    }
    fn ContextSensitiveHelp(&self, _fentermode: BOOL) -> Result<()> {
        Err(Error::from(E_NOTIMPL))
    }
}
