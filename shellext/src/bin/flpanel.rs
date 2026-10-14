//! FL Library companion panel: a window that sits over Explorer's preview-pane area and follows
//! the folder you are in and the item you select. Unlike a preview handler it stays up when
//! nothing is selected. Turn the pane on/off with Alt+P as usual.
#![windows_subsystem = "windows"]

use std::ffi::c_void;

use flprops::{panel, query};
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::LibraryLoader::GetModuleFileNameW;
use windows::Win32::System::Registry::*;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Accessibility::{SetWinEventHook, HWINEVENTHOOK};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;

const TICK: usize = 1;
const TICK_MS: u32 = 300;
const WM_TRAY: u32 = WM_APP + 2;
const MENU_TOGGLE: usize = 1;
const MENU_EXIT: usize = 2;
const MENU_UPDATE: usize = 3;
const UPDATE_TIMER: usize = 2;
const UPDATE_MS: u32 = 24 * 3600 * 1000; // daily; flctl itself prompts only when a newer release exists
const WM_REPLACE: u32 = WM_APP + 1; // Explorer moved / came to front: re-place now
static HOST: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);
const MIN_PANE: i32 = 180; // narrower than this means the preview pane is off

struct State {
    shell: IShellWindows,
    browser: Option<IShellBrowser>, // cached for `target`, so a tick costs a few calls, not a full enumeration
    target: HWND,            // Explorer window we are attached to
    content: Option<String>, // path the panel currently shows
    child: HWND,
    rect: RECT,
    checked: (String, bool), // last folder tested for projects
    zip_checked: (String, bool), // last zip tested for a project inside
    enabled: bool,           // tray toggle; remembered in HKCU\Software\FLLibrary\Enabled
    taskbar_created: u32,    // Explorer restarted: the tray icon must be added again
}

struct View {
    folder: String,
    selected: Option<String>,
    count: i32,
}

unsafe fn class_of(h: HWND) -> String {
    let mut buf = [0u16; 64];
    let n = GetClassNameW(h, &mut buf).max(0) as usize;
    String::from_utf16_lossy(&buf[..n])
}

/// The Explorer browser object behind window `target`.
unsafe fn find_browser(shell: &IShellWindows, target: HWND) -> Option<IShellBrowser> {
    for i in 0..shell.Count().ok()? {
        let Ok(disp) = shell.Item(&VARIANT::from(i)) else { continue };
        let Ok(sp) = disp.cast::<IServiceProvider>() else { continue };
        let Ok(browser) = sp.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) else { continue };
        if browser.GetWindow().map(|h| GetAncestor(h, GA_ROOT) == target).unwrap_or(false) {
            return Some(browser);
        }
    }
    None
}

/// Current folder and selection shown by `browser`.
unsafe fn view_of(browser: &IShellBrowser) -> Option<View> {
    let fv: IFolderView = browser.QueryActiveShellView().ok()?.cast().ok()?;
    let mut folder = String::new();
    if let Ok(pf) = fv.GetFolder::<IPersistFolder2>() {
        if let Ok(pidl) = pf.GetCurFolder() {
            let mut buf = vec![0u16; 2048];
            if SHGetPathFromIDListEx(pidl, &mut buf, GPFIDL_DEFAULT).as_bool() {
                let n = buf.iter().position(|&c| c == 0).unwrap_or(0);
                folder = String::from_utf16_lossy(&buf[..n]);
            }
            CoTaskMemFree(Some(pidl as *const c_void));
        }
    }
    let count = fv.ItemCount(SVGIO_SELECTION).unwrap_or(0);
    let mut selected = None;
    if count == 1 {
        if let Ok(item) = fv.Items::<IShellItemArray>(SVGIO_SELECTION).and_then(|a| a.GetItemAt(0)) {
            if let Ok(name) = item.GetDisplayName(SIGDN_FILESYSPATH) {
                selected = name.to_string().ok();
                CoTaskMemFree(Some(name.0 as *const c_void));
            }
        }
    }
    Some(View { folder, selected, count })
}

struct Found {
    defview: HWND,
    dui: HWND,
}

unsafe extern "system" fn find_cb(h: HWND, lp: LPARAM) -> BOOL {
    let f = &mut *(lp.0 as *mut Found);
    if IsWindowVisible(h).as_bool() {
        match class_of(h).as_str() {
            "SHELLDLL_DefView" if f.defview.is_invalid() => f.defview = h,
            "DUIViewWndClassName" if f.dui.is_invalid() => f.dui = h,
            _ => {}
        }
    }
    TRUE
}

/// Screen rectangle of the preview / details pane: the strip right of the file list.
unsafe fn pane_rect(target: HWND) -> Option<RECT> {
    let mut f = Found { defview: HWND::default(), dui: HWND::default() };
    let _ = EnumChildWindows(target, Some(find_cb), LPARAM(&mut f as *mut Found as isize));
    if f.defview.is_invalid() || f.dui.is_invalid() {
        return None;
    }
    let (mut list, mut area) = (RECT::default(), RECT::default());
    GetWindowRect(f.defview, &mut list).ok()?;
    GetWindowRect(f.dui, &mut area).ok()?;
    let gap = 8 * GetDpiForWindow(target).max(96) as i32 / 96;
    let rc = RECT { left: list.right + gap, top: list.top, right: area.right, bottom: area.bottom };
    (rc.right - rc.left >= MIN_PANE * GetDpiForWindow(target).max(96) as i32 / 96 && rc.bottom > rc.top).then_some(rc)
}

fn has_projects(st: &mut State, dir: &str) -> bool {
    if dir.is_empty() {
        return false;
    }
    if st.checked.0 != dir {
        let found = !query(
            "SELECT 1 FROM files WHERE is_autosave = 0 AND (dir = ?1 COLLATE NOCASE \
             OR substr(dir, 1, length(?1) + 1) = ?1 || char(92) COLLATE NOCASE) LIMIT 1",
            dir.trim_end_matches('\\'),
        )
        .is_empty();
        st.checked = (dir.to_string(), found);
    }
    st.checked.1
}

/// A zip the library found an FL project in (zipped projects are indexed like loose .flp files).
fn is_fl_zip(st: &mut State, zip: &str) -> bool {
    if st.zip_checked.0 != zip {
        let found = !query("SELECT 1 FROM files WHERE container = ?1 COLLATE NOCASE LIMIT 1", zip).is_empty();
        st.zip_checked = (zip.to_string(), found);
    }
    st.zip_checked.1
}

/// What the panel should show for this Explorer view; None hides it (normal preview shows).
fn wanted(st: &mut State, v: &View) -> Option<String> {
    if let Some(sel) = &v.selected {
        let low = sel.to_lowercase();
        if low.ends_with(".flp") || (low.ends_with(".zip") && is_fl_zip(st, sel)) {
            return Some(sel.clone());
        }
        let is_dir = std::fs::metadata(sel).map(|m| m.is_dir()).unwrap_or(false);
        if is_dir && has_projects(st, sel) {
            return Some(sel.clone());
        }
    }
    if has_projects(st, &v.folder) {
        return Some(v.folder.clone());
    }
    Some(panel::LIBRARY.to_string()) // nothing FL here: show the whole library instead
}

/// Keep the panel directly above its Explorer window in the z-order.
///
/// It is deliberately NOT an owned window of Explorer: a cross-process owner relationship makes
/// Windows attach the two input queues, so whenever the panel was busy, Explorer froze with it.
unsafe fn restack(host: HWND, target: HWND) {
    let above = GetWindow(target, GW_HWNDPREV).unwrap_or_default();
    if above == host {
        return;
    }
    let topmost = !above.is_invalid() && (GetWindowLongW(above, GWL_EXSTYLE) as u32 & WS_EX_TOPMOST.0) != 0;
    let after = if above.is_invalid() || topmost { HWND_TOP } else { above };
    let _ = SetWindowPos(host, after, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOOWNERZORDER);
}

unsafe fn tick(host: HWND, st: &mut State) {
    if !st.enabled {
        if IsWindowVisible(host).as_bool() {
            let _ = ShowWindow(host, SW_HIDE);
        }
        return;
    }
    let fg = GetForegroundWindow();
    if fg != host && !fg.is_invalid() && class_of(fg) == "CabinetWClass" && fg != st.target {
        st.target = fg;
        st.browser = None;
        let _ = SetPropW(host, w!("FLLibraryTarget"), HANDLE(fg.0));
    }
    let hide = |st: &mut State| {
        let _ = ShowWindow(host, SW_HIDE);
        st.rect = RECT::default();
    };
    if st.target.is_invalid() || !IsWindow(st.target).as_bool() {
        st.target = HWND::default();
        st.browser = None;
        return hide(st);
    }
    if IsIconic(st.target).as_bool() || !IsWindowVisible(st.target).as_bool() {
        return hide(st);
    }
    let Some(rc) = pane_rect(st.target) else { return hide(st) };
    if st.browser.is_none() {
        st.browser = find_browser(&st.shell, st.target);
    }
    let view = st.browser.as_ref().and_then(|b| view_of(b));
    let Some(view) = view else {
        st.browser = None; // window navigated / closed: look it up again next tick
        return hide(st);
    };
    // keep whatever is shown while the user is working inside the panel (typing a search...)
    let content = if fg == host && st.content.is_some() { st.content.clone() } else { wanted(st, &view) };
    let Some(path) = content else { return hide(st) };
    let _ = view.count;

    if rc != st.rect {
        st.rect = rc;
        let _ = SetWindowPos(host, None, rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top,
                             SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER);
    }
    if st.content.as_deref() != Some(path.as_str()) {
        if !st.child.is_invalid() {
            let _ = DestroyWindow(st.child);
        }
        let mut client = RECT::default();
        let _ = GetClientRect(host, &mut client);
        st.child = panel::create(host, client, &path);
        st.content = Some(path);
    }
    if !IsWindowVisible(host).as_bool() {
        let _ = ShowWindow(host, SW_SHOWNOACTIVATE);
    }
    restack(host, st.target);
}

/// Cheap re-place between ticks, driven by WinEvents (move / resize / activation).
unsafe fn replace(host: HWND, st: &mut State) {
    if st.target.is_invalid() || !IsWindowVisible(host).as_bool() {
        return;
    }
    if let Some(rc) = pane_rect(st.target) {
        if rc != st.rect {
            st.rect = rc;
            let _ = SetWindowPos(host, None, rc.left, rc.top, rc.right - rc.left, rc.bottom - rc.top,
                                 SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER);
        }
    }
    restack(host, st.target);
}

unsafe extern "system" fn on_winevent(_h: HWINEVENTHOOK, _event: u32, hwnd: HWND, id_object: i32, _child: i32, _thread: u32, _time: u32) {
    if id_object != 0 || hwnd.is_invalid() {
        return; // OBJID_WINDOW only: skips the caret / cursor noise
    }
    let host = HWND(HOST.load(std::sync::atomic::Ordering::Relaxed) as *mut c_void);
    if !host.is_invalid() && hwnd != host && class_of(hwnd) == "CabinetWClass" {
        let _ = PostMessageW(host, WM_REPLACE, WPARAM(0), LPARAM(0));
    }
}

// ---------- tray icon: click to turn the panel on / off ----------

const OPT_KEY: PCWSTR = w!("Software\\FLLibrary");

fn read_enabled() -> bool {
    unsafe {
        let mut data = 1u32;
        let mut size = 4u32;
        let _ = RegGetValueW(HKEY_CURRENT_USER, OPT_KEY, w!("Enabled"), RRF_RT_REG_DWORD, None,
                             Some(&mut data as *mut u32 as *mut c_void), Some(&mut size));
        data != 0
    }
}

fn write_enabled(on: bool) {
    unsafe {
        let mut key = HKEY::default();
        if RegCreateKeyExW(HKEY_CURRENT_USER, OPT_KEY, 0, None, REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, None, &mut key, None).is_ok() {
            let _ = RegSetValueExW(key, w!("Enabled"), 0, REG_DWORD, Some(&(on as u32).to_le_bytes()));
            let _ = RegCloseKey(key);
        }
    }
}

/// Icons ship next to the exe: fl_on.ico / fl_off.ico.
unsafe fn tray_icon(on: bool) -> HICON {
    let mut exe = [0u16; 1024];
    let n = GetModuleFileNameW(None, &mut exe) as usize;
    let exe = String::from_utf16_lossy(&exe[..n]);
    let dir = exe.rsplit_once('\\').map(|(d, _)| d).unwrap_or(".");
    let file = HSTRING::from(format!("{dir}\\{}", if on { "fl_on.ico" } else { "fl_off.ico" }));
    LoadImageW(None, &file, IMAGE_ICON, 16, 16, LR_LOADFROMFILE)
        .map(|h| HICON(h.0))
        .unwrap_or_else(|_| LoadIconW(None, IDI_APPLICATION).unwrap_or_default())
}

unsafe fn tray(host: HWND, on: bool, op: NOTIFY_ICON_MESSAGE) {
    let mut nid = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: host,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAY,
        hIcon: tray_icon(on),
        ..Default::default()
    };
    let tip = if on { "FL Library: on (click to turn off)" } else { "FL Library: off (click to turn on)" };
    for (i, c) in tip.encode_utf16().take(127).enumerate() {
        nid.szTip[i] = c;
    }
    let _ = Shell_NotifyIconW(op, &nid);
}

unsafe fn set_enabled(host: HWND, st: &mut State, on: bool) {
    st.enabled = on;
    write_enabled(on);
    tray(host, on, NIM_MODIFY);
    if on {
        st.content = None; // rebuild for wherever Explorer is now
        tick(host, st);
    } else {
        let _ = ShowWindow(host, SW_HIDE);
    }
}

unsafe fn tray_menu(host: HWND, st: &mut State) {
    let Ok(menu) = CreatePopupMenu() else { return };
    let _ = AppendMenuW(menu, MF_STRING, MENU_TOGGLE, if st.enabled { w!("Turn panel off") } else { w!("Turn panel on") });
    let _ = AppendMenuW(menu, MF_STRING, MENU_UPDATE, w!("Check for updates"));
    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
    let _ = AppendMenuW(menu, MF_STRING, MENU_EXIT, w!("Exit FL Library panel"));
    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = SetForegroundWindow(host); // so the menu closes when you click elsewhere
    let pick = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON, pt.x, pt.y, 0, host, None);
    let _ = DestroyMenu(menu);
    match pick.0 as usize {
        MENU_TOGGLE => set_enabled(host, st, !st.enabled),
        MENU_UPDATE => panel::flctl(&["update"]),
        MENU_EXIT => {
            tray(host, st.enabled, NIM_DELETE);
            let _ = DestroyWindow(host);
        }
        _ => {}
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let st = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
    match msg {
        WM_TIMER if !st.is_null() && wp.0 == TICK => {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tick(hwnd, &mut *st)));
            LRESULT(0)
        }
        WM_TIMER if wp.0 == UPDATE_TIMER => {
            let _ = SetTimer(hwnd, UPDATE_TIMER, UPDATE_MS, None); // first fire is 2 min after start
            panel::flctl(&["update", "check"]);
            LRESULT(0)
        }
        WM_TRAY if !st.is_null() => {
            match (lp.0 & 0xFFFF) as u32 {
                WM_LBUTTONUP => set_enabled(hwnd, &mut *st, !(*st).enabled),
                WM_RBUTTONUP => tray_menu(hwnd, &mut *st),
                _ => {}
            }
            LRESULT(0)
        }
        _ if !st.is_null() && msg == (*st).taskbar_created && msg != 0 => {
            tray(hwnd, (*st).enabled, NIM_ADD);
            LRESULT(0)
        }
        WM_REPLACE if !st.is_null() => {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| replace(hwnd, &mut *st)));
            LRESULT(0)
        }
        WM_SIZE if !st.is_null() => {
            let child = (*st).child;
            if !child.is_invalid() {
                let _ = MoveWindow(child, 0, 0, (lp.0 & 0xFFFF) as i32, ((lp.0 >> 16) & 0xFFFF) as i32, true);
            }
            LRESULT(0)
        }
        WM_CLOSE => LRESULT(0), // lives as long as the session; Alt+P hides it
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

fn main() -> Result<()> {
    unsafe {
        let _mutex = CreateMutexW(None, true, w!("Local\\FLLibraryPanel"))?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return Ok(());
        }
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;
        let shell: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_LOCAL_SERVER)?;

        let class = w!("FLLibraryHost");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: CreateSolidBrush(COLORREF(0x00252120)),
            lpszClassName: class,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let host = CreateWindowExW(
            WS_EX_TOOLWINDOW | WS_EX_CONTROLPARENT,
            class,
            w!("FL Library"),
            WS_POPUP | WS_CLIPCHILDREN,
            0, 0, 0, 0,
            None, None, None, None,
        )?;
        let state = Box::new(State {
            shell,
            browser: None,
            target: HWND::default(),
            content: None,
            child: HWND::default(),
            rect: RECT::default(),
            checked: (String::new(), false),
            zip_checked: (String::new(), false),
            enabled: read_enabled(),
            taskbar_created: RegisterWindowMessageW(w!("TaskbarCreated")),
        });
        SetWindowLongPtrW(host, GWLP_USERDATA, Box::into_raw(state) as isize);
        tray(host, read_enabled(), NIM_ADD);
        SetTimer(host, TICK, TICK_MS, None);
        SetTimer(host, UPDATE_TIMER, 2 * 60 * 1000, None);
        HOST.store(host.0 as isize, std::sync::atomic::Ordering::Relaxed);
        // out-of-context hooks: no DLL injected into Explorer, no input-queue coupling
        const EVENT_SYSTEM_FOREGROUND: u32 = 0x0003;
        const EVENT_OBJECT_LOCATIONCHANGE: u32 = 0x800B;
        let _ = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(on_winevent), 0, 0, 0);
        let _ = SetWinEventHook(EVENT_OBJECT_LOCATIONCHANGE, EVENT_OBJECT_LOCATIONCHANGE, None, Some(on_winevent), 0, 0, 0);

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if !IsDialogMessageW(host, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
    }
    Ok(())
}
