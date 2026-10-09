//! Explorer property handler for .flp: FL version, BPM, title... read from the file header,
//! plus status / tags / rating / missing samples looked up in the FL Library database.
#![allow(non_snake_case)]

pub mod panel;

use std::cell::RefCell;
use std::ffi::{c_char, c_int, c_void, CString};
use std::io::Read;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::OnceLock;

use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Shell::PropertiesSystem::*;

const CLSID_FLPROPS: GUID = GUID::from_u128(0x7C0B4E7A_52F1_4B8E_9D7C_2F1A6B0C4D11);
const FMTID_FL: GUID = GUID::from_u128(0xB5F0A2C4_1E6D_4F3A_8B7E_9C2D4A6E8F10);
const FMTID_SUMMARY: GUID = GUID::from_u128(0xF29F85E0_4FF9_1068_AB91_08002B27B3D9);
const FMTID_MEDIA: GUID = GUID::from_u128(0x64440492_4C8B_11D1_8B70_080036B11A03);
const FMTID_MUSIC: GUID = GUID::from_u128(0x56A3372E_CE9C_11D2_9F0E_006097C686F6);

const fn pkey(fmtid: GUID, pid: u32) -> PROPERTYKEY {
    PROPERTYKEY { fmtid, pid }
}
// custom (flprops.propdesc)
const PK_FLVERSION: PROPERTYKEY = pkey(FMTID_FL, 2);
const PK_STATUS: PROPERTYKEY = pkey(FMTID_FL, 3);
const PK_MISSING: PROPERTYKEY = pkey(FMTID_FL, 4);
const PK_HOURS: PROPERTYKEY = pkey(FMTID_FL, 5);
const PK_VERSIONS: PROPERTYKEY = pkey(FMTID_FL, 6);
const PK_CHANNELS: PROPERTYKEY = pkey(FMTID_FL, 7);
// built-in
const PK_TITLE: PROPERTYKEY = pkey(FMTID_SUMMARY, 2);
const PK_AUTHOR: PROPERTYKEY = pkey(FMTID_SUMMARY, 4);
const PK_KEYWORDS: PROPERTYKEY = pkey(FMTID_SUMMARY, 5);
const PK_COMMENT: PROPERTYKEY = pkey(FMTID_SUMMARY, 6);
const PK_RATING: PROPERTYKEY = pkey(FMTID_MEDIA, 9);
const PK_GENRE: PROPERTYKEY = pkey(FMTID_MUSIC, 11);
const PK_BPM: PROPERTYKEY = pkey(FMTID_MUSIC, 35);

pub(crate) const HEAD_BYTES: u64 = 64 * 1024; // project-level events all come before the first channel

// ---------- FLP header ----------

#[derive(Default)]
pub(crate) struct Head {
    pub(crate) version: String,
    pub(crate) bpm: Option<f64>,
    pub(crate) title: String,
    pub(crate) author: String,
    pub(crate) genre: String,
    pub(crate) comment: String,
    pub(crate) hours: Option<f64>,
    pub(crate) channels: u32,
}

fn text(data: &[u8], utf16: bool) -> String {
    let s = if utf16 {
        let units: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        // pre-11.5 projects are ANSI; map the Hebrew block of cp1255, rest as Latin-1
        data.iter()
            .map(|&b| match b {
                0xE0..=0xFA => char::from_u32(0x05D0 + (b as u32 - 0xE0)).unwrap_or('?'),
                _ => b as char,
            })
            .collect()
    };
    s.split('\0').next().unwrap_or("").trim().to_string()
}

pub(crate) fn parse_head(buf: &[u8]) -> Option<Head> {
    let n = buf.len();
    if n < 22 || &buf[0..4] != b"FLhd" {
        return None;
    }
    let hlen = u32::from_le_bytes(buf[4..8].try_into().ok()?) as usize;
    let mut h = Head { channels: u16::from_le_bytes([buf[10], buf[11]]) as u32, ..Default::default() };
    let mut pos = 8usize.checked_add(hlen)?;
    if buf.get(pos..pos + 4)? != b"FLdt" {
        return None;
    }
    pos += 8;
    let mut utf16 = false;
    let (mut tempo_old, mut tempo_fine) = (0u32, 0u32);
    while pos < n {
        let eid = buf[pos];
        pos += 1;
        let size = if eid < 64 {
            1
        } else if eid < 128 {
            2
        } else if eid < 192 {
            // FL 25+ writes event 172 with a 3-byte payload, directly followed by a text event
            if eid == 172 && pos + 3 < n && buf[pos + 3] == 192 { 3 } else { 4 }
        } else {
            let (mut size, mut shift) = (0usize, 0u32);
            loop {
                let b = *buf.get(pos)?;
                pos += 1;
                if shift < 56 {
                    size |= ((b & 0x7F) as usize) << shift;
                }
                shift += 7;
                if b & 0x80 == 0 {
                    break;
                }
            }
            size
        };
        let Some(d) = pos.checked_add(size).and_then(|end| buf.get(pos..end)) else { break };
        pos += size;
        match eid {
            64 => break, // first channel: the project-level block is over
            199 => {
                h.version = String::from_utf8_lossy(d).trim_matches('\0').trim().to_string();
                let mut it = h.version.split('.').map(|p| p.parse::<u32>().unwrap_or(0));
                let v = (it.next().unwrap_or(0), it.next().unwrap_or(0));
                utf16 = v >= (11, 5);
            }
            156 if size == 4 => h.bpm = Some(u32::from_le_bytes([d[0], d[1], d[2], d[3]]) as f64 / 1000.0),
            66 => tempo_old = u16::from_le_bytes([d[0], d[1]]) as u32,
            93 => tempo_fine = u16::from_le_bytes([d[0], d[1]]) as u32,
            194 => h.title = text(d, utf16),
            195 => {
                let c = text(d, utf16);
                if !c.starts_with("{\\rtf") {
                    h.comment = c.chars().take(500).collect();
                }
            }
            206 => h.genre = text(d, utf16),
            207 => h.author = text(d, utf16),
            237 if size >= 16 => {
                let worked = f64::from_le_bytes(d[8..16].try_into().ok()?);
                if (0.0..3650.0).contains(&worked) {
                    h.hours = Some((worked * 24.0 * 10.0).round() / 10.0);
                }
            }
            _ => {}
        }
    }
    if h.bpm.is_none() && tempo_old > 0 {
        h.bpm = Some(tempo_old as f64 + tempo_fine as f64 / 1000.0);
    }
    Some(h)
}

// ---------- library database (system winsqlite3) ----------

#[link(name = "winsqlite3", kind = "raw-dylib")]
extern "system" {
    fn sqlite3_open_v2(filename: *const c_char, db: *mut *mut c_void, flags: c_int, vfs: *const c_char) -> c_int;
    fn sqlite3_close(db: *mut c_void) -> c_int;
    fn sqlite3_busy_timeout(db: *mut c_void, ms: c_int) -> c_int;
    fn sqlite3_prepare_v2(db: *mut c_void, sql: *const c_char, n: c_int, stmt: *mut *mut c_void, tail: *mut *const c_char) -> c_int;
    fn sqlite3_bind_text(stmt: *mut c_void, idx: c_int, text: *const c_char, n: c_int, destructor: isize) -> c_int;
    fn sqlite3_step(stmt: *mut c_void) -> c_int;
    fn sqlite3_column_text(stmt: *mut c_void, col: c_int) -> *const c_char;
    fn sqlite3_column_int(stmt: *mut c_void, col: c_int) -> c_int;
    fn sqlite3_column_count(stmt: *mut c_void) -> c_int;
    fn sqlite3_finalize(stmt: *mut c_void) -> c_int;
}

const SQL: &[u8] = b"SELECT m.status, m.tags, m.rating, p.n_missing, \
    (SELECT COUNT(DISTINCT hash) FROM files f2 WHERE f2.song_key = f.song_key) \
    FROM files f LEFT JOIN meta m ON m.song_key = f.song_key LEFT JOIN projects p ON p.hash = f.hash \
    WHERE f.path = ?1 COLLATE NOCASE LIMIT 1\0";

#[derive(Default)]
struct LibRow {
    status: String,
    tags: String,
    rating: u32,
    missing: u32,
    versions: u32,
}

/// A string value from HKLM\SOFTWARE\FLLibrary (written by install.ps1).
pub(crate) fn reg_str(name: PCWSTR) -> Option<String> {
    unsafe {
        let mut buf = [0u16; 1024];
        let mut size = (buf.len() * 2) as u32;
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\FLLibrary"),
            name,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr() as *mut c_void),
            Some(&mut size),
        )
        .ok()
        .ok()?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(0);
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

fn db_path() -> Option<&'static CString> {
    static PATH: OnceLock<Option<CString>> = OnceLock::new();
    PATH.get_or_init(|| CString::new(reg_str(w!("Db"))?).ok()).as_ref()
}

/// Run a one-parameter query; every column comes back as text.
pub fn query(sql: &str, param: &str) -> Vec<Vec<String>> {
    Db::open().map(|db| db.rows(sql, param)).unwrap_or_default()
}

/// Read-only connection to the library, for several queries in a row.
pub(crate) struct Db(*mut c_void);

impl Db {
    pub(crate) fn open() -> Option<Db> {
        let dbp = db_path()?;
        unsafe {
            let mut db = std::ptr::null_mut();
            if sqlite3_open_v2(dbp.as_ptr(), &mut db, 2 /* READWRITE: a WAL database needs it to create its -shm file; nothing is written */, std::ptr::null()) != 0 {
                sqlite3_close(db);
                return None;
            }
            sqlite3_busy_timeout(db, 300);
            Some(Db(db))
        }
    }

    pub(crate) fn rows(&self, sql: &str, param: &str) -> Vec<Vec<String>> {
        let mut rows = Vec::new();
        let (Ok(csql), Ok(cparam)) = (CString::new(sql), CString::new(param)) else { return rows };
        unsafe {
            let mut stmt = std::ptr::null_mut();
            if sqlite3_prepare_v2(self.0, csql.as_ptr(), -1, &mut stmt, std::ptr::null_mut()) == 0 {
                sqlite3_bind_text(stmt, 1, cparam.as_ptr(), -1, -1 /* TRANSIENT */);
                while sqlite3_step(stmt) == 100 {
                    let n = sqlite3_column_count(stmt);
                    rows.push((0..n).map(|i| col_text(stmt, i)).collect());
                }
            }
            sqlite3_finalize(stmt);
        }
        rows
    }
}

impl Drop for Db {
    fn drop(&mut self) {
        unsafe {
            sqlite3_close(self.0);
        }
    }
}

unsafe fn col_text(stmt: *mut c_void, col: c_int) -> String {
    let p = sqlite3_column_text(stmt, col);
    if p.is_null() {
        String::new()
    } else {
        std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
    }
}

fn lookup(path: &str) -> Option<LibRow> {
    let dbp = db_path()?;
    let cpath = CString::new(path).ok()?;
    unsafe {
        let mut db = std::ptr::null_mut();
        if sqlite3_open_v2(dbp.as_ptr(), &mut db, 2 /* READWRITE: a WAL database needs it to create its -shm file; nothing is written */, std::ptr::null()) != 0 {
            sqlite3_close(db);
            return None;
        }
        sqlite3_busy_timeout(db, 200);
        let mut stmt = std::ptr::null_mut();
        let mut row = None;
        if sqlite3_prepare_v2(db, SQL.as_ptr() as *const c_char, -1, &mut stmt, std::ptr::null_mut()) == 0 {
            sqlite3_bind_text(stmt, 1, cpath.as_ptr(), -1, -1 /* TRANSIENT */);
            if sqlite3_step(stmt) == 100 {
                row = Some(LibRow {
                    status: col_text(stmt, 0),
                    tags: col_text(stmt, 1),
                    rating: sqlite3_column_int(stmt, 2).max(0) as u32,
                    missing: sqlite3_column_int(stmt, 3).max(0) as u32,
                    versions: sqlite3_column_int(stmt, 4).max(0) as u32,
                });
            }
        }
        sqlite3_finalize(stmt);
        sqlite3_close(db);
        row
    }
}

// ---------- property store ----------

fn put(cache: &IPropertyStoreCache, key: &PROPERTYKEY, mut pv: PROPVARIANT) {
    unsafe {
        if PSCoerceToCanonicalValue(key, &mut pv).is_ok() {
            let _ = cache.SetValueAndState(key, &pv, PSC_NORMAL);
        }
    }
}

fn put_str(cache: &IPropertyStoreCache, key: &PROPERTYKEY, s: &str) {
    if !s.is_empty() {
        put(cache, key, PROPVARIANT::from(s));
    }
}

fn fill(cache: &IPropertyStoreCache, path: &str) {
    let mut buf = Vec::new();
    if let Ok(f) = std::fs::File::open(path) {
        let _ = f.take(HEAD_BYTES).read_to_end(&mut buf);
    }
    if let Some(h) = parse_head(&buf) {
        put_str(cache, &PK_FLVERSION, &h.version);
        if let Some(bpm) = h.bpm {
            let s = if bpm.fract() == 0.0 { format!("{}", bpm as u32) } else { format!("{bpm:.1}") };
            put_str(cache, &PK_BPM, &s);
        }
        put_str(cache, &PK_TITLE, &h.title);
        put_str(cache, &PK_AUTHOR, &h.author);
        put_str(cache, &PK_GENRE, &h.genre);
        put_str(cache, &PK_COMMENT, &h.comment);
        if let Some(hours) = h.hours {
            put(cache, &PK_HOURS, PROPVARIANT::from(hours));
        }
        if h.channels > 0 {
            put(cache, &PK_CHANNELS, PROPVARIANT::from(h.channels));
        }
    }
    if let Some(r) = lookup(path) {
        put_str(cache, &PK_STATUS, &r.status);
        put_str(cache, &PK_KEYWORDS, &r.tags.replace(',', ";"));
        if (1..=5).contains(&r.rating) {
            put(cache, &PK_RATING, PROPVARIANT::from([1u32, 25, 50, 75, 99][r.rating as usize - 1]));
        }
        put(cache, &PK_MISSING, PROPVARIANT::from(r.missing));
        if r.versions > 0 {
            put(cache, &PK_VERSIONS, PROPVARIANT::from(r.versions));
        }
    }
}

#[implement(IInitializeWithFile, IPropertyStore, IPropertyStoreCapabilities)]
struct FlProps {
    cache: RefCell<Option<IPropertyStoreCache>>,
}

impl FlProps_Impl {
    fn store(&self) -> Result<IPropertyStoreCache> {
        self.cache.borrow().clone().ok_or_else(|| Error::from(E_UNEXPECTED))
    }
}

impl IInitializeWithFile_Impl for FlProps_Impl {
    fn Initialize(&self, pszfilepath: &PCWSTR, _grfmode: u32) -> Result<()> {
        if self.cache.borrow().is_some() {
            return Err(Error::from(HRESULT(0x800704DFu32 as i32))); // already initialized
        }
        let path = unsafe { pszfilepath.to_string() }.map_err(|_| Error::from(E_INVALIDARG))?;
        let mut raw = std::ptr::null_mut();
        let cache: IPropertyStoreCache = unsafe {
            PSCreateMemoryPropertyStore(&IPropertyStoreCache::IID, &mut raw)?;
            IPropertyStoreCache::from_raw(raw)
        };
        // a panic must never unwind into Explorer
        let _ = catch_unwind(AssertUnwindSafe(|| fill(&cache, &path)));
        *self.cache.borrow_mut() = Some(cache);
        Ok(())
    }
}

impl IPropertyStore_Impl for FlProps_Impl {
    fn GetCount(&self) -> Result<u32> {
        unsafe { self.store()?.GetCount() }
    }
    fn GetAt(&self, iprop: u32, pkey: *mut PROPERTYKEY) -> Result<()> {
        unsafe { self.store()?.GetAt(iprop, pkey) }
    }
    fn GetValue(&self, key: *const PROPERTYKEY) -> Result<PROPVARIANT> {
        unsafe { self.store()?.GetValue(key) }
    }
    fn SetValue(&self, _key: *const PROPERTYKEY, _propvar: *const PROPVARIANT) -> Result<()> {
        Err(Error::from(STG_E_ACCESSDENIED))
    }
    fn Commit(&self) -> Result<()> {
        Err(Error::from(STG_E_ACCESSDENIED))
    }
}

impl IPropertyStoreCapabilities_Impl for FlProps_Impl {
    fn IsPropertyWritable(&self, _key: *const PROPERTYKEY) -> Result<()> {
        Err(Error::from(S_FALSE)) // read-only: edits go through the right-click menu
    }
}

#[implement(IClassFactory)]
struct Factory {
    panel: bool,
}

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(&self, punkouter: Option<&IUnknown>, riid: *const GUID, ppvobject: *mut *mut c_void) -> Result<()> {
        if punkouter.is_some() {
            return Err(Error::from(CLASS_E_NOAGGREGATION));
        }
        let unk: IUnknown = if self.panel { panel::Panel::new().into() } else { FlProps { cache: RefCell::new(None) }.into() };
        unsafe { unk.query(riid, ppvobject).ok() }
    }
    fn LockServer(&self, _flock: BOOL) -> Result<()> {
        Ok(())
    }
}

#[no_mangle]
unsafe extern "system" fn DllGetClassObject(rclsid: *const GUID, riid: *const GUID, ppv: *mut *mut c_void) -> HRESULT {
    if ppv.is_null() || rclsid.is_null() || riid.is_null() {
        return E_POINTER;
    }
    *ppv = std::ptr::null_mut();
    let panel = *rclsid == panel::CLSID_PANEL;
    if !panel && *rclsid != CLSID_FLPROPS {
        return CLASS_E_CLASSNOTAVAILABLE;
    }
    let factory: IClassFactory = Factory { panel }.into();
    factory.query(riid, ppv)
}

#[no_mangle]
extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}
