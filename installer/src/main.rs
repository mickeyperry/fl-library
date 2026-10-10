//! FL-Library-Setup.exe: one-file installer / uninstaller for FL Library.
//!
//! Everything the app needs (panel, Explorer extension, scripts, a private embedded Python) is
//! packed into `payload.zip` and compiled in. The UI is fully custom-painted in the panel's FL
//! style: a 16-step sequencer doubles as the progress bar, and finishing plays a little beat.
//!
//! `FL-Library-Setup.exe`              install / update
//! `FL-Library-Setup.exe /uninstall`   remove (this is what Apps & features runs)
#![windows_subsystem = "windows"]

use std::ffi::c_void;
use std::io::{Cursor, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::Duration;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Input::KeyboardAndMouse::ReleaseCapture;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Shell::PropertiesSystem::{PSRegisterPropertySchema, PSUnregisterPropertySchema};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

static PAYLOAD: &[u8] = include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/payload.zip"));
const VERSION: &str = env!("CARGO_PKG_VERSION");

const CLSID_PROPS: &str = "{7C0B4E7A-52F1-4B8E-9D7C-2F1A6B0C4D11}";
const CLSID_OLD_PANEL: &str = "{3E9A1D52-8C47-4B6F-A1E3-5D7F2B9C6A21}";
const KEY_APP: &str = "SOFTWARE\\FLLibrary";
const KEY_UNINSTALL: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\FLLibrary";
const KEY_RUN: &str = "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Run";
const KEY_ASSOC: &str = "SOFTWARE\\Classes\\SystemFileAssociations\\.flp";
const TASK: &str = "FL Library scan";
const TASK_SYNC: &str = "FL Library sync";
const NO_WINDOW: u32 = 0x0800_0000;

// ---------- look ----------

const fn rgb(r: u32, g: u32, b: u32) -> u32 {
    r | (g << 8) | (b << 16)
}
const C_BG: u32 = rgb(30, 31, 35);
const C_PANEL: u32 = rgb(40, 42, 48);
const C_PAD_A: u32 = rgb(58, 60, 68);
const C_PAD_B: u32 = rgb(74, 77, 87);
const C_TEXT: u32 = rgb(232, 232, 236);
const C_DIM: u32 = rgb(150, 152, 162);
const C_ACCENT: u32 = rgb(255, 140, 0);
const C_ACCENT_HI: u32 = rgb(255, 170, 60);
const C_GREEN: u32 = rgb(80, 205, 120);
const C_RED: u32 = rgb(255, 95, 95);
const PALETTE: [u32; 8] = [
    rgb(255, 140, 0), rgb(255, 200, 60), rgb(80, 205, 120), rgb(84, 150, 255),
    rgb(240, 160, 255), rgb(130, 230, 230), rgb(255, 150, 170), rgb(190, 255, 140),
];
const W: i32 = 760;
const H: i32 = 500;
const WM_PROGRESS: u32 = WM_APP + 1;
const WM_EVERYTHING: u32 = WM_APP + 2;
const TIMER_ANIM: usize = 1;

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Welcome,
    Working,
    Done,
    Failed,
    UninstallAsk,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Act {
    Close,
    Install,
    Toggle(usize),
    GetEverything,
    Recheck,
    OpenExplorer,
    Uninstall,
}

struct Confetti {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    color: u32,
    size: f32,
}

struct App {
    page: Page,
    uninstall: bool,
    dpi: i32,
    tick: u32,
    options: [bool; 3], // start with Windows, Explorer columns + menu, index now  (uninstall: [delete library, -, -])
    everything: Option<std::result::Result<u64, String>>,
    fl_versions: Vec<u32>,
    progress: f32,
    status: String,
    detail: String,
    error: String,
    hits: Vec<(RECT, Act)>,
    hover: POINT,
    confetti: Vec<Confetti>,
    rng: u32,
}

impl App {
    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        (self.rng % 10_000) as f32 / 10_000.0
    }
}

// ---------- small helpers ----------

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn install_dir() -> PathBuf {
    let pf = std::env::var("ProgramW6432").or_else(|_| std::env::var("ProgramFiles")).unwrap_or_else(|_| "C:\\Program Files".into());
    PathBuf::from(pf).join("FL Library")
}

fn data_dir() -> PathBuf {
    let base = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| std::env::var("USERPROFILE").unwrap_or_default());
    PathBuf::from(base).join("FL Library")
}

fn hidden(cmd: &mut Command) -> &mut Command {
    cmd.creation_flags(NO_WINDOW)
}

fn reg_set(root: HKEY, path: &str, name: &str, value: &str) -> std::result::Result<(), String> {
    unsafe {
        let mut key = HKEY::default();
        let p = wide(path);
        RegCreateKeyExW(root, PCWSTR(p.as_ptr()), 0, None, REG_OPTION_NON_VOLATILE, KEY_WRITE | KEY_WOW64_64KEY, None, &mut key, None)
            .ok()
            .map_err(|e| format!("registry {path}: {e}"))?;
        let n = wide(name);
        let v = wide(value);
        let bytes = std::slice::from_raw_parts(v.as_ptr() as *const u8, v.len() * 2);
        let name_ptr = if name.is_empty() { PCWSTR::null() } else { PCWSTR(n.as_ptr()) };
        let r = RegSetValueExW(key, name_ptr, 0, REG_SZ, Some(bytes));
        let _ = RegCloseKey(key);
        r.ok().map_err(|e| format!("registry {path}\\{name}: {e}"))
    }
}

fn reg_dword(root: HKEY, path: &str, name: &str, value: u32) -> std::result::Result<(), String> {
    unsafe {
        let mut key = HKEY::default();
        let p = wide(path);
        RegCreateKeyExW(root, PCWSTR(p.as_ptr()), 0, None, REG_OPTION_NON_VOLATILE, KEY_WRITE | KEY_WOW64_64KEY, None, &mut key, None)
            .ok()
            .map_err(|e| format!("registry {path}: {e}"))?;
        let n = wide(name);
        let r = RegSetValueExW(key, PCWSTR(n.as_ptr()), 0, REG_DWORD, Some(&value.to_le_bytes()));
        let _ = RegCloseKey(key);
        r.ok().map_err(|e| format!("registry {path}\\{name}: {e}"))
    }
}

fn reg_get(root: HKEY, path: &str, name: &str) -> Option<String> {
    unsafe {
        let (p, n) = (wide(path), wide(name));
        let mut buf = [0u16; 2048];
        let mut size = (buf.len() * 2) as u32;
        RegGetValueW(root, PCWSTR(p.as_ptr()), PCWSTR(n.as_ptr()), RRF_RT_REG_SZ | RRF_SUBKEY_WOW6464KEY, None,
                     Some(buf.as_mut_ptr() as *mut c_void), Some(&mut size)).ok().ok()?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

fn reg_delete_tree(root: HKEY, path: &str) {
    unsafe {
        let p = wide(path);
        let _ = RegDeleteTreeW(root, PCWSTR(p.as_ptr()));
        let _ = RegDeleteKeyExW(root, PCWSTR(p.as_ptr()), KEY_WOW64_64KEY.0, 0);
    }
}

fn reg_delete_value(root: HKEY, path: &str, name: &str) {
    unsafe {
        let mut key = HKEY::default();
        let p = wide(path);
        if RegOpenKeyExW(root, PCWSTR(p.as_ptr()), 0, KEY_SET_VALUE | KEY_WOW64_64KEY, &mut key).is_ok() {
            let n = wide(name);
            let _ = RegDeleteValueW(key, PCWSTR(n.as_ptr()));
            let _ = RegCloseKey(key);
        }
    }
}

/// Product version of an exe, e.g. FL64.exe -> (25, 2, 5, 5319).
fn file_version(path: &Path) -> Option<(u32, u32, u32, u32)> {
    unsafe {
        let p = wide(&path.to_string_lossy());
        let size = GetFileVersionInfoSizeW(PCWSTR(p.as_ptr()), None);
        if size == 0 {
            return None;
        }
        let mut buf = vec![0u8; size as usize];
        GetFileVersionInfoW(PCWSTR(p.as_ptr()), 0, size, buf.as_mut_ptr() as *mut c_void).ok()?;
        let mut info: *mut c_void = std::ptr::null_mut();
        let mut len = 0u32;
        if !VerQueryValueW(buf.as_ptr() as *const c_void, w!("\\"), &mut info, &mut len).as_bool() || info.is_null() {
            return None;
        }
        let fi = &*(info as *const VS_FIXEDFILEINFO);
        Some((fi.dwFileVersionMS >> 16, fi.dwFileVersionMS & 0xFFFF, fi.dwFileVersionLS >> 16, fi.dwFileVersionLS & 0xFFFF))
    }
}

/// Every installed FL Studio by major version: (major, exe), newest build of each major.
fn find_fl() -> Vec<(u32, PathBuf)> {
    let mut best: std::collections::BTreeMap<u32, ((u32, u32, u32, u32, bool), PathBuf)> = Default::default();
    for root in ["C:\\Program Files\\Image-Line", "C:\\Program Files (x86)\\Image-Line"] {
        let Ok(dirs) = std::fs::read_dir(root) else { continue };
        for d in dirs.flatten() {
            let dname = d.file_name().to_string_lossy().to_string();
            if !dname.to_lowercase().starts_with("fl studio") {
                continue;
            }
            for exe in ["FL64.exe", "FL.exe"] {
                let path = d.path().join(exe);
                let Some(mut v) = file_version(&path) else { continue };
                if v.0 < 2 {
                    // FL 11 / 12 report 1.1.x: take the major from the folder name
                    if let Some(m) = dname.split_whitespace().nth(2).and_then(|s| s.split('.').next()).and_then(|s| s.parse().ok()) {
                        v = (m, 0, 0, 0);
                    }
                }
                let rank = (v.0, v.1, v.2, v.3, exe == "FL64.exe");
                if best.get(&v.0).map(|(r, _)| rank > *r).unwrap_or(true) {
                    best.insert(v.0, (rank, path));
                }
            }
        }
    }
    best.into_iter().map(|(m, (_, p))| (m, p)).collect()
}

/// Ask Everything how many FL projects it can see.
fn check_everything() -> std::result::Result<u64, String> {
    let addr: SocketAddr = "127.0.0.1:8666".parse().unwrap();
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_millis(800))
        .map_err(|_| "not reachable".to_string())?;
    s.set_read_timeout(Some(Duration::from_secs(4))).ok();
    write!(s, "GET /?json=1&count=0&search=ext%3Aflp HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n").map_err(|e| e.to_string())?;
    let mut raw = String::new();
    s.read_to_string(&mut raw).map_err(|e| e.to_string())?;
    let i = raw.find("\"totalResults\"").ok_or("unexpected answer")?;
    let digits: String = raw[i..].chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().map_err(|_| "unexpected answer".to_string())
}

// ---------- sound: a tiny drum machine rendered into a WAV in memory ----------

fn render_beat(pattern: &str, step: f32) -> Vec<u8> {
    const RATE: u32 = 22_050;
    let len = (step * pattern.len() as f32 + 0.6) * RATE as f32;
    let mut mix = vec![0f32; len as usize];
    let mut seed = 0x1234_5678u32;
    let mut noise = || {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed as f32 / u32::MAX as f32) * 2.0 - 1.0
    };
    for (i, hit) in pattern.chars().enumerate() {
        let start = (i as f32 * step * RATE as f32) as usize;
        let n = (0.45 * RATE as f32) as usize;
        let mut phase = 0f32;
        let mut last = 0f32;
        for k in 0..n {
            let t = k as f32 / RATE as f32;
            let v = match hit {
                'K' => {
                    let f = 48.0 + 120.0 * (-t * 28.0).exp();
                    phase += f / RATE as f32;
                    (phase * std::f32::consts::TAU).sin() * (-t * 8.0).exp() * 0.9
                }
                'S' => {
                    phase += 185.0 / RATE as f32;
                    noise() * (-t * 18.0).exp() * 0.45 + (phase * std::f32::consts::TAU).sin() * (-t * 22.0).exp() * 0.35
                }
                'H' => {
                    let x = noise();
                    let hp = x - last; // crude high-pass for a crisp hat
                    last = x;
                    hp * (-t * 55.0).exp() * 0.28
                }
                _ => 0.0,
            };
            if let Some(m) = mix.get_mut(start + k) {
                *m += v;
            }
        }
    }
    let mut wav = Vec::with_capacity(44 + mix.len() * 2);
    let data_len = (mix.len() * 2) as u32;
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&RATE.to_le_bytes());
    wav.extend_from_slice(&(RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for s in mix {
        wav.extend_from_slice(&((s.clamp(-1.0, 1.0) * 30_000.0) as i16).to_le_bytes());
    }
    wav
}

fn play(which: &str) {
    static KICK: OnceLock<Vec<u8>> = OnceLock::new();
    static OUTRO: OnceLock<Vec<u8>> = OnceLock::new();
    let wav = match which {
        "kick" => KICK.get_or_init(|| render_beat("K", 0.2)),
        _ => OUTRO.get_or_init(|| render_beat("KHSHKKSHK", 0.13)),
    };
    unsafe {
        let _ = PlaySoundW(PCWSTR(wav.as_ptr() as *const u16), None, SND_MEMORY | SND_ASYNC | SND_NODEFAULT);
    }
}

// ---------- install / uninstall (worker thread) ----------

struct Progress(isize);

impl Progress {
    fn send(&self, pct: f32, status: &str, detail: &str) {
        log(&format!("{:>3.0}% {status} {detail}", pct * 100.0));
        if self.0 == 0 {
            return; // silent mode: no window to update
        }
        let msg = Box::into_raw(Box::new((status.to_string(), detail.to_string())));
        unsafe {
            if PostMessageW(HWND(self.0 as *mut c_void), WM_PROGRESS, WPARAM((pct * 1000.0) as usize), LPARAM(msg as isize)).is_err() {
                drop(Box::from_raw(msg));
            }
        }
    }
    fn fail(&self, err: String) {
        log(&format!("FAILED: {err}"));
        let msg = Box::into_raw(Box::new(("!".to_string(), err)));
        unsafe {
            if PostMessageW(HWND(self.0 as *mut c_void), WM_PROGRESS, WPARAM(usize::MAX), LPARAM(msg as isize)).is_err() {
                drop(Box::from_raw(msg));
            }
        }
    }
}

fn stop_panel() {
    let _ = hidden(Command::new("taskkill").args(["/F", "/IM", "flpanel.exe"])).output();
    std::thread::sleep(Duration::from_millis(400));
}

fn fnv(data: &[u8]) -> u32 {
    let mut h: u32 = 0x811C9DC5;
    for b in data {
        h = (h ^ *b as u32).wrapping_mul(0x01000193);
    }
    h
}

fn quoted(p: &Path) -> String {
    format!("\"{}\"", p.display())
}

/// Start a program as the signed-in user rather than elevated (Explorer launches it for us).
fn start_unelevated(exe: &Path) {
    let _ = Command::new("explorer.exe").arg(exe).spawn();
}

/// The property-schema and shell calls are COM: the worker thread has to join COM first.
fn com_init() {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_APARTMENTTHREADED);
    }
}

fn install(opts: [bool; 3], p: &Progress) -> std::result::Result<(), String> {
    com_init();
    let dir = install_dir();
    let app = dir.join("app");
    let pythonw = dir.join("python").join("pythonw.exe");
    let hklm = HKEY_LOCAL_MACHINE;

    p.send(0.03, "Getting ready", "Stopping a running panel, if any");
    stop_panel();

    // 1. files
    let mut zip = zip::ZipArchive::new(Cursor::new(PAYLOAD)).map_err(|e| format!("payload: {e}"))?;
    let total = zip.len().max(1);
    let mut dll_name = String::new();
    std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| format!("payload: {e}"))?;
        let Some(rel) = f.enclosed_name() else { continue };
        let mut target = dir.join(&rel);
        if f.is_dir() {
            std::fs::create_dir_all(&target).ok();
            continue;
        }
        let mut data = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut data).map_err(|e| format!("payload: {e}"))?;
        if rel.as_os_str() == "flprops.dll" {
            // Explorer keeps a loaded copy locked, so every build gets its own file name
            dll_name = format!("flprops-{:08X}.dll", fnv(&data));
            target = dir.join(&dll_name);
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(&target, &data).map_err(|e| format!("{}: {e}", target.display()))?;
        if i % 25 == 0 {
            p.send(0.05 + 0.55 * i as f32 / total as f32, "Copying files", &rel.to_string_lossy());
        }
    }
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("flprops-") && n.ends_with(".dll") && n != dll_name {
                let _ = std::fs::remove_file(e.path()); // older builds; locked ones go next time
            }
        }
    }
    std::fs::write(app.join("installed.flag"), b"data lives in %LOCALAPPDATA%\\FL Library\n").ok();
    if let Ok(me) = std::env::current_exe() {
        let _ = std::fs::copy(me, dir.join("uninstall.exe"));
    }
    let data = data_dir();
    std::fs::create_dir_all(&data).ok();
    // coming from an earlier / developer install: bring its library (tags, notes, bookmarks) along
    let db = data.join("library.db");
    if !db.exists() {
        if let Some(old) = reg_get(hklm, KEY_APP, "Db").map(PathBuf::from).filter(|p| p.exists() && *p != db) {
            p.send(0.62, "Bringing your library along", &old.to_string_lossy());
            for ext in ["", "-wal", "-shm"] {
                let from = PathBuf::from(format!("{}{ext}", old.display()));
                if from.exists() {
                    let _ = std::fs::copy(&from, format!("{}{ext}", db.display()));
                }
            }
            let cfg = old.with_file_name("config.json");
            if cfg.exists() && !data.join("config.json").exists() {
                let _ = std::fs::copy(cfg, data.join("config.json"));
            }
        }
    }

    // 2. settings the panel and Explorer extension read
    p.send(0.64, "Finding FL Studio", "Looking for every installed version");
    let fl = find_fl();
    reg_set(hklm, KEY_APP, "Db", &data.join("library.db").to_string_lossy())?;
    reg_set(hklm, KEY_APP, "Ctl", &app.join("flctl.py").to_string_lossy())?;
    reg_set(hklm, KEY_APP, "Pythonw", &pythonw.to_string_lossy())?;
    reg_set(hklm, KEY_APP, "Everything", "127.0.0.1:8666")?;
    reg_set(hklm, KEY_APP, "SearchExclude", "")?;
    reg_set(hklm, KEY_APP, "FlExe", &fl.last().map(|(_, p)| p.to_string_lossy().to_string()).unwrap_or_default())?;
    reg_set(hklm, KEY_APP, "FlMajors", &fl.iter().map(|(m, _)| m.to_string()).collect::<Vec<_>>().join(","))?;
    for (m, path) in &fl {
        reg_set(hklm, KEY_APP, &format!("Fl_{m}"), &path.to_string_lossy())?;
    }
    // leftovers of the early preview-handler version
    reg_delete_tree(hklm, &format!("SOFTWARE\\Classes\\CLSID\\{CLSID_OLD_PANEL}"));
    reg_delete_tree(hklm, "SOFTWARE\\Classes\\.flp\\shellex\\{8895b1c6-b41f-4c1c-a562-0d564250836f}");
    reg_delete_tree(hklm, "SOFTWARE\\Classes\\Directory\\shellex\\{8895b1c6-b41f-4c1c-a562-0d564250836f}");

    // 3. Explorer columns + right-click menu
    if opts[1] {
        p.send(0.72, "Teaching Explorer about .flp", "Columns: FL version, BPM, status, rating...");
        let schema = dir.join("flprops.propdesc");
        let sw = wide(&schema.to_string_lossy());
        unsafe {
            let _ = PSUnregisterPropertySchema(PCWSTR(sw.as_ptr()));
            PSRegisterPropertySchema(PCWSTR(sw.as_ptr())).map_err(|e| format!("property schema: {e}"))?;
        }
        let clsid = format!("SOFTWARE\\Classes\\CLSID\\{CLSID_PROPS}");
        reg_set(hklm, &clsid, "", "FL Library property handler")?;
        reg_set(hklm, &format!("{clsid}\\InprocServer32"), "", &dir.join(&dll_name).to_string_lossy())?;
        reg_set(hklm, &format!("{clsid}\\InprocServer32"), "ThreadingModel", "Both")?;
        reg_set(hklm, "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\PropertySystem\\PropertyHandlers\\.flp", "", CLSID_PROPS)?;
        let all = "FLLibrary.FLVersion;System.Music.BeatsPerMinute;FLLibrary.Status;System.Rating;System.Keywords;FLLibrary.HoursWorked;FLLibrary.MissingSamples;FLLibrary.Versions;FLLibrary.Channels;System.Title;System.Author;System.Music.Genre;System.Comment";
        reg_set(hklm, KEY_ASSOC, "FullDetails", &format!("prop:System.PropGroup.Description;{all};System.PropGroup.FileSystem;System.ItemNameDisplay;System.ItemFolderPathDisplay;System.Size;System.DateCreated;System.DateModified"))?;
        reg_set(hklm, KEY_ASSOC, "PreviewDetails", &format!("prop:{all};System.DateModified;System.Size"))?;
        reg_set(hklm, KEY_ASSOC, "InfoTip", "prop:FLLibrary.FLVersion;System.Music.BeatsPerMinute;FLLibrary.Status;System.Rating;System.Keywords;FLLibrary.HoursWorked;FLLibrary.MissingSamples;System.DateModified;System.Size")?;
        reg_set(hklm, KEY_ASSOC, "ExtendedTileInfo", "prop:FLLibrary.FLVersion;System.Music.BeatsPerMinute;FLLibrary.Status")?;

        let menu = format!("{KEY_ASSOC}\\shell\\FLLibrary");
        reg_delete_tree(hklm, &menu);
        reg_set(hklm, &menu, "MUIVerb", "FL Library")?;
        reg_set(hklm, &menu, "SubCommands", "")?;
        let ctl = format!("{} {}", quoted(&pythonw), quoted(&app.join("flctl.py")));
        let items: [(&str, &str, &str, u32); 14] = [
            ("02versions", "Find all versions in Everything", "versions \"%1\"", 0),
            ("10idea", "Status: idea", "status idea \"%1\"", 0x20),
            ("11wip", "Status: WIP", "status wip \"%1\"", 0),
            ("12mixing", "Status: mixing", "status mixing \"%1\"", 0),
            ("13done", "Status: done", "status done \"%1\"", 0),
            ("14dead", "Status: dead", "status dead \"%1\"", 0),
            ("15none", "Status: clear", "status none \"%1\"", 0),
            ("20r5", "Rating: 5 stars", "rating 5 \"%1\"", 0x20),
            ("21r4", "Rating: 4 stars", "rating 4 \"%1\"", 0),
            ("22r3", "Rating: 3 stars", "rating 3 \"%1\"", 0),
            ("23r2", "Rating: 2 stars", "rating 2 \"%1\"", 0),
            ("24r1", "Rating: 1 star", "rating 1 \"%1\"", 0),
            ("25r0", "Rating: clear", "rating 0 \"%1\"", 0),
            ("40rescan", "Rescan library now", "rescan", 0x20),
        ];
        for (key, label, args, flags) in items {
            let k = format!("{menu}\\shell\\{key}");
            reg_set(hklm, &k, "MUIVerb", label)?;
            if flags != 0 {
                reg_dword(hklm, &k, "CommandFlags", flags)?;
            }
            reg_set(hklm, &format!("{k}\\command"), "", &format!("{ctl} {args}"))?;
        }
    }

    // 4. background re-index every 30 minutes
    p.send(0.82, "Scheduling re-indexing", "Every 30 minutes, only new or changed projects");
    let tr = format!("{} {}", quoted(&pythonw), quoted(&app.join("scanner.py")));
    let out = hidden(Command::new("schtasks").args(["/Create", "/F", "/TN", TASK, "/SC", "MINUTE", "/MO", "30", "/TR", &tr]))
        .output()
        .map_err(|e| format!("schtasks: {e}"))?;
    if !out.status.success() {
        return Err(format!("could not create the scan task: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    // tags / bookmarks shared with other PCs through config.json "sync_dir": pull every 5 minutes
    let sync = format!("{} {} sync", quoted(&pythonw), quoted(&app.join("flctl.py")));
    let _ = hidden(Command::new("schtasks").args(["/Create", "/F", "/TN", TASK_SYNC, "/SC", "MINUTE", "/MO", "5", "/TR", &sync])).output();

    // 5. Apps & features entry, startup, launch
    p.send(0.9, "Almost there", "Adding FL Library to Apps & features");
    let panel = dir.join("flpanel.exe");
    reg_set(hklm, KEY_UNINSTALL, "DisplayName", "FL Library")?;
    reg_set(hklm, KEY_UNINSTALL, "DisplayVersion", VERSION)?;
    reg_set(hklm, KEY_UNINSTALL, "Publisher", "FL Library")?;
    reg_set(hklm, KEY_UNINSTALL, "DisplayIcon", &panel.to_string_lossy())?;
    reg_set(hklm, KEY_UNINSTALL, "InstallLocation", &dir.to_string_lossy())?;
    reg_set(hklm, KEY_UNINSTALL, "UninstallString", &format!("{} /uninstall", quoted(&dir.join("uninstall.exe"))))?;
    reg_dword(hklm, KEY_UNINSTALL, "NoModify", 1)?;
    reg_dword(hklm, KEY_UNINSTALL, "NoRepair", 1)?;
    reg_dword(hklm, KEY_UNINSTALL, "EstimatedSize", (PAYLOAD.len() as u32 / 1024) * 3)?;
    if opts[0] {
        reg_set(hklm, KEY_RUN, "FLLibraryPanel", &quoted(&panel))?;
    } else {
        reg_delete_value(hklm, KEY_RUN, "FLLibraryPanel");
    }
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }
    start_unelevated(&panel);
    if opts[2] {
        p.send(0.97, "Starting the first scan", "It keeps running in the background");
        let _ = hidden(Command::new("schtasks").args(["/Run", "/TN", TASK])).output();
    }
    p.send(1.0, "Done", "");
    Ok(())
}

fn uninstall(delete_library: bool, p: &Progress) -> std::result::Result<(), String> {
    com_init();
    let dir = install_dir();
    let hklm = HKEY_LOCAL_MACHINE;
    p.send(0.1, "Stopping FL Library", "");
    stop_panel();
    let _ = hidden(Command::new("schtasks").args(["/Delete", "/F", "/TN", TASK])).output();
    let _ = hidden(Command::new("schtasks").args(["/Delete", "/F", "/TN", TASK_SYNC])).output();

    p.send(0.3, "Removing it from Explorer", "");
    let sw = wide(&dir.join("flprops.propdesc").to_string_lossy());
    unsafe {
        let _ = PSUnregisterPropertySchema(PCWSTR(sw.as_ptr()));
    }
    for key in [
        "SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\PropertySystem\\PropertyHandlers\\.flp".to_string(),
        format!("SOFTWARE\\Classes\\CLSID\\{CLSID_PROPS}"),
        format!("SOFTWARE\\Classes\\CLSID\\{CLSID_OLD_PANEL}"),
        format!("{KEY_ASSOC}\\shell\\FLLibrary"),
        KEY_APP.to_string(),
        KEY_UNINSTALL.to_string(),
    ] {
        reg_delete_tree(hklm, &key);
    }
    for name in ["FullDetails", "PreviewDetails", "InfoTip", "ExtendedTileInfo"] {
        reg_delete_value(hklm, KEY_ASSOC, name);
    }
    reg_delete_value(hklm, KEY_RUN, "FLLibraryPanel");
    unsafe {
        SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None);
    }

    p.send(0.6, "Deleting files", &dir.to_string_lossy());
    remove_tree(&dir);
    if delete_library {
        p.send(0.85, "Deleting your library", "Tags, notes, bookmarks and the index");
        remove_tree(&data_dir());
    }
    p.send(1.0, "Done", "");
    Ok(())
}

/// Delete a folder; whatever is locked (the running uninstaller, a DLL Explorer still holds) goes at next restart.
fn remove_tree(path: &Path) {
    let Ok(entries) = std::fs::read_dir(path) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            remove_tree(&p);
        } else if std::fs::remove_file(&p).is_err() {
            let w = wide(&p.to_string_lossy());
            unsafe {
                let _ = MoveFileExW(PCWSTR(w.as_ptr()), PCWSTR::null(), MOVEFILE_DELAY_UNTIL_REBOOT);
            }
        }
    }
    if std::fs::remove_dir(path).is_err() {
        let w = wide(&path.to_string_lossy());
        unsafe {
            let _ = MoveFileExW(PCWSTR(w.as_ptr()), PCWSTR::null(), MOVEFILE_DELAY_UNTIL_REBOOT);
        }
    }
}

// ---------- painting ----------

struct Paint {
    hdc: HDC,
    dpi: i32,
}

impl Paint {
    fn px(&self, v: i32) -> i32 {
        v * self.dpi / 96
    }
    fn r(&self, x: i32, y: i32, w: i32, h: i32) -> RECT {
        RECT { left: self.px(x), top: self.px(y), right: self.px(x + w), bottom: self.px(y + h) }
    }
    unsafe fn fill(&self, rc: RECT, color: u32) {
        let b = CreateSolidBrush(COLORREF(color));
        FillRect(self.hdc, &rc, b);
        let _ = DeleteObject(b);
    }
    unsafe fn round(&self, rc: RECT, radius: i32, color: u32) {
        let b = CreateSolidBrush(COLORREF(color));
        let pen = CreatePen(PS_SOLID, 1, COLORREF(color));
        let ob = SelectObject(self.hdc, b);
        let op = SelectObject(self.hdc, pen);
        let rr = self.px(radius);
        let _ = RoundRect(self.hdc, rc.left, rc.top, rc.right, rc.bottom, rr, rr);
        SelectObject(self.hdc, ob);
        SelectObject(self.hdc, op);
        let _ = DeleteObject(b);
        let _ = DeleteObject(pen);
    }
    unsafe fn text(&self, rc: RECT, s: &str, size: i32, weight: i32, color: u32, flags: DRAW_TEXT_FORMAT) {
        // an empty slice is a dangling pointer; DrawTextW's ellipsis handling dereferences it
        if s.is_empty() {
            return;
        }
        let font = CreateFontW(-self.px(size), 0, 0, 0, weight, 0, 0, 0, 1, 0, 0, 5, 0, w!("Segoe UI"));
        let of = SelectObject(self.hdc, font);
        SetBkMode(self.hdc, TRANSPARENT);
        SetTextColor(self.hdc, COLORREF(color));
        let mut buf: Vec<u16> = s.encode_utf16().collect();
        let mut rc = rc;
        DrawTextW(self.hdc, &mut buf, &mut rc, flags | DT_NOPREFIX);
        SelectObject(self.hdc, of);
        let _ = DeleteObject(font);
    }
}

fn blend(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| ((a >> s) & 0xFF) as f32 * (1.0 - t) + ((b >> s) & 0xFF) as f32 * t;
    rgb(ch(0) as u32, ch(8) as u32, ch(16) as u32)
}

fn inside(rc: &RECT, pt: POINT) -> bool {
    pt.x >= rc.left && pt.x < rc.right && pt.y >= rc.top && pt.y < rc.bottom
}

unsafe fn button(app: &mut App, g: &Paint, x: i32, y: i32, w: i32, label: &str, primary: bool, act: Act) {
    let rc = g.r(x, y, w, 42);
    let hot = inside(&rc, app.hover);
    let color = match (primary, hot) {
        (true, false) => C_ACCENT,
        (true, true) => C_ACCENT_HI,
        (false, false) => C_PAD_A,
        (false, true) => C_PAD_B,
    };
    g.round(rc, 10, color);
    g.text(rc, label, 15, 700, if primary { rgb(25, 25, 25) } else { C_TEXT }, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    app.hits.push((rc, act));
}

unsafe fn checkbox(app: &mut App, g: &Paint, x: i32, y: i32, i: usize, label: &str, hint: &str) {
    let on = app.options[i];
    let box_rc = g.r(x, y + 2, 22, 22);
    g.round(box_rc, 6, if on { C_ACCENT } else { C_PAD_A });
    if on {
        g.text(box_rc, "\u{2713}", 15, 900, rgb(25, 25, 25), DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    }
    g.text(g.r(x + 34, y, 600, 22), label, 15, 600, C_TEXT, DT_LEFT | DT_SINGLELINE);
    g.text(g.r(x + 34, y + 21, 640, 18), hint, 12, 400, C_DIM, DT_LEFT | DT_SINGLELINE);
    app.hits.push((g.r(x, y, 660, 40), Act::Toggle(i)));
}

/// The 16-step sequencer: idle groove on the welcome page, progress while working, party when done.
unsafe fn sequencer(app: &App, g: &Paint, y: i32) {
    let (x0, total) = (40, W - 80);
    let gap = 6;
    let pad = (total - gap * 15) / 16;
    let step = (app.tick / 3) as usize % 16;
    let groove = [1, 0, 0, 1, 0, 0, 1, 0, 1, 0, 0, 1, 0, 1, 0, 0];
    for i in 0..16 {
        let rc = g.r(x0 + i as i32 * (pad + gap), y, pad, 46);
        let base = if (i / 4) % 2 == 0 { C_PAD_A } else { C_PAD_B };
        let color = match app.page {
            Page::Working => {
                let lit = (app.progress * 16.0) > i as f32;
                if lit {
                    if i == step { C_ACCENT_HI } else { C_ACCENT }
                } else if i == step {
                    blend(base, C_ACCENT, 0.25)
                } else {
                    base
                }
            }
            Page::Done => PALETTE[(i + app.tick as usize / 4) % PALETTE.len()],
            Page::Failed => if i % 2 == 0 { blend(base, C_RED, 0.5) } else { base },
            _ => {
                let on = groove[i] == 1;
                match (on, i == step) {
                    (true, true) => C_ACCENT_HI,
                    (true, false) => blend(base, C_ACCENT, 0.55),
                    (false, true) => blend(base, C_TEXT, 0.25),
                    _ => base,
                }
            }
        };
        g.round(rc, 7, color);
    }
}

unsafe fn paint(hwnd: HWND, app: &mut App) {
    let mut ps = PAINTSTRUCT::default();
    let screen = BeginPaint(hwnd, &mut ps);
    let mut client = RECT::default();
    let _ = GetClientRect(hwnd, &mut client);
    let mem = CreateCompatibleDC(screen);
    let bmp = CreateCompatibleBitmap(screen, client.right, client.bottom);
    let old = SelectObject(mem, bmp);
    let g = Paint { hdc: mem, dpi: app.dpi };
    app.hits.clear();

    g.fill(client, C_BG);
    g.fill(g.r(0, 0, W, 4), C_ACCENT);
    // header
    g.text(g.r(40, 26, 500, 46), "FL Library", 34, 800, C_TEXT, DT_LEFT | DT_SINGLELINE);
    g.text(g.r(42, 74, 640, 22), "Your FL Studio projects, organised right inside Explorer", 14, 400, C_DIM, DT_LEFT | DT_SINGLELINE);
    g.text(g.r(W - 140, 34, 80, 20), &format!("v{VERSION}"), 12, 600, C_DIM, DT_RIGHT | DT_SINGLELINE);
    let close = g.r(W - 46, 14, 30, 30);
    if inside(&close, app.hover) {
        g.round(close, 8, C_PAD_B);
    }
    g.text(close, "\u{2715}", 14, 600, C_DIM, DT_CENTER | DT_VCENTER | DT_SINGLELINE);
    app.hits.push((close, Act::Close));

    sequencer(app, &g, 112);

    let y = 186;
    match app.page {
        Page::Welcome => {
            // status lines
            let (dot, line) = match &app.everything {
                None => (C_DIM, "Everything: checking...".to_string()),
                Some(Ok(n)) => (C_GREEN, format!("Everything is running and sees {} FL projects on your drives", group(*n))),
                Some(Err(_)) => (C_RED, "Everything isn't answering: install it and turn on Tools > Options > HTTP Server (port 8666)".to_string()),
            };
            g.round(g.r(40, y + 5, 10, 10), 10, dot);
            g.text(g.r(60, y, 660, 20), &line, 13, 500, C_TEXT, DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS);
            let (dot, line) = if app.fl_versions.is_empty() {
                (C_DIM, "FL Studio: not found (you can still browse projects)".to_string())
            } else {
                (C_GREEN, format!("FL Studio found: {}", app.fl_versions.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(", ")))
            };
            g.round(g.r(40, y + 29, 10, 10), 10, dot);
            g.text(g.r(60, y + 24, 660, 20), &line, 13, 500, C_TEXT, DT_LEFT | DT_SINGLELINE);

            g.round(g.r(40, y + 56, W - 80, 168), 12, C_PANEL);
            checkbox(app, &g, 58, y + 70, 0, "Start the panel with Windows", "Tray icon: click it any time to turn the panel on or off");
            checkbox(app, &g, 58, y + 120, 1, "Explorer columns + right-click menu for .flp files", "FL version, BPM, status, rating and missing samples as Explorer columns");
            checkbox(app, &g, 58, y + 170, 2, "Index my projects right after installing", "Runs in the background; the panel fills in as it goes");

            g.text(g.r(40, H - 66, 380, 18), &format!("Installs to {}", install_dir().display()), 11, 400, C_DIM, DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS);
            g.text(g.r(40, H - 48, 380, 18), "Your data stays on this PC", 11, 400, C_DIM, DT_LEFT | DT_SINGLELINE);
            if matches!(app.everything, Some(Err(_))) {
                button(app, &g, W - 470, H - 72, 140, "Get Everything", false, Act::GetEverything);
                button(app, &g, W - 320, H - 72, 100, "Recheck", false, Act::Recheck);
            }
            button(app, &g, W - 210, H - 72, 170, "Install", true, Act::Install);
        }
        Page::UninstallAsk => {
            g.text(g.r(40, y, 680, 30), "Remove FL Library?", 22, 700, C_TEXT, DT_LEFT | DT_SINGLELINE);
            g.text(g.r(40, y + 36, 680, 40), "The panel, Explorer columns, right-click menu and the background scan will be removed. Your FL projects are never touched.", 13, 400, C_DIM, DT_LEFT | DT_WORDBREAK);
            g.round(g.r(40, y + 92, W - 80, 64), 12, C_PANEL);
            checkbox(app, &g, 58, y + 104, 0, "Also delete my library", "Your tags, notes, ratings, bookmarks and the project index");
            button(app, &g, W - 380, H - 72, 150, "Keep it", false, Act::Close);
            button(app, &g, W - 210, H - 72, 170, "Uninstall", true, Act::Uninstall);
        }
        Page::Working => {
            g.text(g.r(40, y + 10, 680, 34), &app.status, 22, 700, C_TEXT, DT_LEFT | DT_SINGLELINE);
            g.text(g.r(40, y + 50, 680, 22), &app.detail, 13, 400, C_DIM, DT_LEFT | DT_SINGLELINE | DT_PATH_ELLIPSIS);
            let bar = g.r(40, y + 100, W - 80, 8);
            g.round(bar, 8, C_PAD_A);
            let mut fill = bar;
            fill.right = fill.left + ((bar.right - bar.left) as f32 * app.progress) as i32;
            g.round(fill, 8, C_ACCENT);
            g.text(g.r(40, y + 116, 680, 20), &format!("{}%", (app.progress * 100.0) as i32), 13, 600, C_DIM, DT_LEFT | DT_SINGLELINE);
        }
        Page::Done => {
            if app.uninstall {
                g.text(g.r(40, y + 10, 680, 40), "FL Library is gone.", 26, 800, C_TEXT, DT_LEFT | DT_SINGLELINE);
                g.text(g.r(40, y + 56, 680, 44), "Thanks for using it. Anything still in use is removed the next time Windows restarts.", 14, 400, C_DIM, DT_LEFT | DT_WORDBREAK);
            } else {
                g.text(g.r(40, y + 4, 680, 40), "You're all set!", 26, 800, C_TEXT, DT_LEFT | DT_SINGLELINE);
                let tips = [
                    "Open any folder in Explorer and press Alt+P: the FL panel appears on the right.",
                    "Folders without projects show your whole library by year, month and song.",
                    "Tray icon: click to turn the panel on or off, right-click to exit.",
                    "Double-click opens in FL; Shift = the FL version it was made in; Ctrl = show in folder.",
                ];
                for (i, t) in tips.iter().enumerate() {
                    let ty = y + 56 + i as i32 * 30;
                    g.round(g.r(40, ty + 6, 8, 8), 8, PALETTE[i % PALETTE.len()]);
                    g.text(g.r(58, ty, 680, 22), t, 13, 500, C_TEXT, DT_LEFT | DT_SINGLELINE | DT_END_ELLIPSIS);
                }
                button(app, &g, W - 390, H - 72, 160, "Open Explorer", false, Act::OpenExplorer);
            }
            button(app, &g, W - 210, H - 72, 170, "Finish", true, Act::Close);
        }
        Page::Failed => {
            g.text(g.r(40, y + 4, 680, 36), "Something went wrong", 24, 800, C_RED, DT_LEFT | DT_SINGLELINE);
            g.text(g.r(40, y + 50, 680, 120), &app.error, 13, 400, C_TEXT, DT_LEFT | DT_WORDBREAK);
            button(app, &g, W - 210, H - 72, 170, "Close", true, Act::Close);
        }
    }

    // confetti on top of everything
    for c in &app.confetti {
        let s = g.px(c.size as i32).max(2);
        let rc = RECT { left: g.px(c.x as i32), top: g.px(c.y as i32), right: g.px(c.x as i32) + s, bottom: g.px(c.y as i32) + s * 2 / 3 + 1 };
        g.fill(rc, c.color);
    }

    let _ = BitBlt(screen, 0, 0, client.right, client.bottom, mem, 0, 0, SRCCOPY);
    SelectObject(mem, old);
    let _ = DeleteObject(bmp);
    let _ = DeleteDC(mem);
    let _ = EndPaint(hwnd, &ps);
}

fn group(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

// ---------- window ----------

fn spawn_everything_check(hwnd: HWND) {
    let h = hwnd.0 as isize;
    std::thread::spawn(move || {
        let result = Box::into_raw(Box::new(check_everything()));
        unsafe {
            if PostMessageW(HWND(h as *mut c_void), WM_EVERYTHING, WPARAM(0), LPARAM(result as isize)).is_err() {
                drop(Box::from_raw(result));
            }
        }
    });
}

unsafe fn start_work(hwnd: HWND, app: &mut App) {
    app.page = Page::Working;
    app.progress = 0.0;
    app.status = if app.uninstall { "Uninstalling".into() } else { "Installing".into() };
    let opts = app.options;
    let uninstalling = app.uninstall;
    let p = Progress(hwnd.0 as isize);
    play("kick");
    std::thread::spawn(move || {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if uninstalling { uninstall(opts[0], &p) } else { install(opts, &p) }
        }))
        .unwrap_or_else(|e| {
            let what = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()));
            Err(format!("internal error: {}", what.unwrap_or_else(|| "unknown".into())))
        });
        log(&format!("result: {r:?}"));
        if let Err(e) = r {
            p.fail(e);
        }
    });
}

unsafe fn act(hwnd: HWND, app: &mut App, a: Act) {
    log(&format!("click: {a:?}"));
    match a {
        Act::Close => {
            if app.page != Page::Working {
                let _ = DestroyWindow(hwnd);
            }
        }
        Act::Toggle(i) => app.options[i] = !app.options[i],
        Act::Install | Act::Uninstall => start_work(hwnd, app),
        Act::GetEverything => {
            ShellExecuteW(None, w!("open"), w!("https://www.voidtools.com/"), PCWSTR::null(), PCWSTR::null(), SW_SHOWNORMAL);
        }
        Act::Recheck => {
            app.everything = None;
            spawn_everything_check(hwnd);
        }
        Act::OpenExplorer => {
            let _ = Command::new("explorer.exe").spawn();
        }
    }
    let _ = InvalidateRect(hwnd, None, false);
}

unsafe fn celebrate(app: &mut App) {
    for _ in 0..160 {
        let c = Confetti {
            x: app.rand() * W as f32,
            y: -app.rand() * 200.0,
            vx: (app.rand() - 0.5) * 3.0,
            vy: 2.0 + app.rand() * 4.0,
            color: PALETTE[(app.rand() * PALETTE.len() as f32) as usize % PALETTE.len()],
            size: 5.0 + app.rand() * 6.0,
        };
        app.confetti.push(c);
    }
    play("outro");
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let app = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if app.is_null() {
        return DefWindowProcW(hwnd, msg, wp, lp);
    }
    let app = &mut *app;
    match msg {
        WM_PAINT => {
            paint(hwnd, app);
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        WM_DPICHANGED => {
            // dragged to a monitor with different scaling: redraw at the new size
            app.dpi = ((wp.0 & 0xFFFF) as i32).max(96);
            let rc = &*(lp.0 as *const RECT);
            let _ = SetWindowPos(hwnd, None, rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top, SWP_NOZORDER | SWP_NOACTIVATE);
            LRESULT(0)
        }
        WM_TIMER => {
            app.tick = app.tick.wrapping_add(1);
            for c in app.confetti.iter_mut() {
                c.x += c.vx;
                c.y += c.vy;
                c.vy += 0.08;
                c.vx *= 0.99;
            }
            app.confetti.retain(|c| c.y < H as f32 + 20.0);
            let _ = InvalidateRect(hwnd, None, false);
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            app.hover = POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            let pt = POINT { x: (lp.0 & 0xFFFF) as i16 as i32, y: ((lp.0 >> 16) & 0xFFFF) as i16 as i32 };
            if let Some((_, a)) = app.hits.iter().find(|(rc, _)| inside(rc, pt)).copied() {
                act(hwnd, app, a);
            } else {
                // drag the window by any empty spot
                let _ = ReleaseCapture();
                SendMessageW(hwnd, WM_NCLBUTTONDOWN, WPARAM(HTCAPTION as usize), LPARAM(0));
            }
            LRESULT(0)
        }
        WM_PROGRESS => {
            let msg = Box::from_raw(lp.0 as *mut (String, String));
            if wp.0 == usize::MAX {
                app.page = Page::Failed;
                app.error = format!("{}\n\nA log of every step is in {}", msg.1, log_path().display());
            } else {
                app.progress = wp.0 as f32 / 1000.0;
                app.status = msg.0;
                app.detail = msg.1;
                if wp.0 >= 1000 {
                    app.page = Page::Done;
                    celebrate(app);
                }
            }
            LRESULT(0)
        }
        WM_EVERYTHING => {
            let r = Box::from_raw(lp.0 as *mut std::result::Result<u64, String>);
            app.everything = Some(*r);
            LRESULT(0)
        }
        WM_CLOSE => {
            log("WM_CLOSE");
            if app.page != Page::Working {
                let _ = DestroyWindow(hwnd);
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            log("WM_DESTROY");
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// %TEMP%\FL-Library-Setup.log: every step and any crash, so a failed install can be diagnosed.
fn log_path() -> PathBuf {
    std::env::temp_dir().join("FL-Library-Setup.log")
}

fn log(line: &str) {
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_path()) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "[{t}] {line}");
    }
}

fn main() -> Result<()> {
    let uninstalling = std::env::args().any(|a| a.eq_ignore_ascii_case("/uninstall"));
    let silent = std::env::args().any(|a| a.eq_ignore_ascii_case("/silent"));
    let _ = std::fs::remove_file(log_path());
    log(&format!("FL Library setup {VERSION} start: uninstall={uninstalling} silent={silent} os={}", std::env::consts::ARCH));
    // never vanish silently: record the crash and tell the user where the log is
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("CRASH: {info}");
        log(&msg);
        let text = wide(&format!("FL Library setup hit a problem:\n\n{info}\n\nDetails were saved to:\n{}", log_path().display()));
        unsafe {
            MessageBoxW(None, PCWSTR(text.as_ptr()), w!("FL Library Setup"), MB_ICONERROR | MB_OK);
        }
    }));
    if silent {
        // unattended run (testing / deployment): no window, result in the log and the exit code
        let p = Progress(0);
        let r = if uninstalling { uninstall(false, &p) } else { install([true, true, false], &p) };
        log(&format!("silent result: {r:?}"));
        std::process::exit(if r.is_ok() { 0 } else { 1 });
    }
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let hinst: HINSTANCE = GetModuleHandleW(None)?.into();
        let class = w!("FLLibrarySetup");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DROPSHADOW,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hIcon: LoadIconW(hinst, PCWSTR(1 as *const u16)).unwrap_or_default(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            if uninstalling { w!("Uninstall FL Library") } else { w!("FL Library Setup") },
            WS_POPUP | WS_SYSMENU | WS_MINIMIZEBOX,
            0, 0, W, H,
            None, None, hinst, None,
        )?;
        let dpi = (GetDpiForWindow(hwnd) as i32).max(96);
        let (w, h) = (W * dpi / 96, H * dpi / 96);
        let (sw, sh) = (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN));
        let _ = SetWindowPos(hwnd, None, (sw - w) / 2, (sh - h) / 2, w, h, SWP_NOZORDER);

        let app = Box::new(App {
            page: if uninstalling { Page::UninstallAsk } else { Page::Welcome },
            uninstall: uninstalling,
            dpi,
            tick: 0,
            options: if uninstalling { [false, false, false] } else { [true, true, true] },
            everything: None,
            fl_versions: if uninstalling { Vec::new() } else { find_fl().into_iter().map(|(m, _)| m).collect() },
            progress: 0.0,
            status: String::new(),
            detail: String::new(),
            error: String::new(),
            hits: Vec::new(),
            hover: POINT::default(),
            confetti: Vec::new(),
            rng: 0x9E37_79B9 ^ GetTickCount_(),
        });
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(app) as isize);
        if !uninstalling {
            spawn_everything_check(hwnd);
        }
        SetTimer(hwnd, TIMER_ANIM, 40, None);
        let _ = ShowWindow(hwnd, SW_SHOW);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

#[allow(non_snake_case)]
fn GetTickCount_() -> u32 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(1) | 1
}
