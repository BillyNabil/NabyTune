//! NabyTune's icon in the Windows notification area (the system tray).
//!
//! The icon lives on its own thread, with a hidden top-level window and a message loop, so the
//! shell's callbacks never wait on GPUI. A hidden window rather than a message-only one: only
//! top-level windows receive the "TaskbarCreated" broadcast that Explorer sends when it restarts,
//! which is when the icon has to be added again.
//!
//! The app reads clicks with [`Tray::events`] (non-blocking) and pushes state with
//! [`Tray::set_tooltip`] and [`Tray::set_playing`]; both are safe to call from any thread.
//! Dropping the [`Tray`] removes the icon and ends the thread.
//!
//! windows-sys features used: Win32_Foundation, Win32_Graphics_Gdi, Win32_System_LibraryLoader,
//! Win32_System_Registry, Win32_UI_HiDpi, Win32_UI_Shell, Win32_UI_WindowsAndMessaging.

use std::cell::Cell;
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{
    GetLastError, BOOL, ERROR_CLASS_ALREADY_EXISTS, HWND, LPARAM, LRESULT, WPARAM,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ};
use windows_sys::Win32::UI::HiDpi::{
    GetDpiForWindow, GetSystemMetricsForDpi, SetThreadDpiAwarenessContext,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NIM_SETVERSION, NIN_SELECT, NOTIFYICONDATAW, NOTIFYICON_VERSION_4,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, ChangeWindowMessageFilterEx, CreateIconFromResourceEx, CreatePopupMenu,
    CreateWindowExW, DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW,
    EndMenu, FindWindowW, GetMessageW, GetSystemMetrics, GetWindowLongPtrW, IsWindow,
    LookupIconIdFromDirectoryEx, PostMessageW, PostQuitMessage, RegisterClassExW,
    RegisterWindowMessageW, SetForegroundWindow, SetMenuDefaultItem, SetWindowLongPtrW,
    TrackPopupMenu, TranslateMessage, CREATESTRUCTW, GWLP_USERDATA, HICON, HMENU, LR_DEFAULTCOLOR,
    MF_SEPARATOR, MF_STRING, MSG, MSGFLT_ALLOW, SM_CXSMICON, SM_MENUDROPALIGNMENT, TPM_BOTTOMALIGN,
    TPM_LEFTALIGN, TPM_NONOTIFY, TPM_RETURNCMD, TPM_RIGHTALIGN, TPM_RIGHTBUTTON, WM_APP,
    WM_CONTEXTMENU, WM_DESTROY, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_LBUTTONDBLCLK, WM_NCCREATE,
    WM_NCDESTROY, WM_NULL, WM_SETTINGCHANGE, WNDCLASSEXW, WS_EX_TOOLWINDOW, WS_POPUP,
};

use crate::i18n::t;

/// What the user asked for from the tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayEvent {
    /// Left click, double-click, Enter on the icon, or "Show NabyTune" in the menu.
    Show,
    PlayPause,
    Next,
    Previous,
    Quit,
}

/// The tray icon. Dropping it removes the icon.
pub struct Tray {
    /// The hidden window, as an integer so `Tray` is `Send` and `Sync`.
    hwnd: isize,
    shared: Arc<Shared>,
    events: Mutex<Receiver<TrayEvent>>,
    /// Disconnects when the tray thread has finished.
    finished: Mutex<Receiver<()>>,
    thread: Option<JoinHandle<()>>,
}

/// State the app's threads write and the tray thread reads.
struct Shared {
    playing: AtomicBool,
    /// The tooltip, NUL-terminated, at most 127 UTF-16 units before the NUL.
    tooltip: Mutex<Vec<u16>>,
}

// The window's own messages.
const WM_TRAY_CALLBACK: u32 = WM_APP + 1; // from the shell: clicks on the icon
const WM_TRAY_TOOLTIP: u32 = WM_APP + 2; // from `set_tooltip`
const WM_TRAY_QUIT: u32 = WM_APP + 3; // from `Drop`

const ICON_ID: u32 = 1;
const NIN_KEYSELECT: u32 = NIN_SELECT | 1; // NINF_KEY
const CLASS_NAME: &str = "NabyTuneTrayWindow";
const DEFAULT_TOOLTIP: &str = "NabyTune";

const CMD_SHOW: u32 = 1;
const CMD_PLAY_PAUSE: u32 = 2;
const CMD_NEXT: u32 = 3;
const CMD_PREVIOUS: u32 = 4;
const CMD_QUIT: u32 = 5;

/// A double-click also arrives as a click (NIN_SELECT); one `Show` is enough for both.
const SHOW_DEBOUNCE: Duration = Duration::from_millis(600);

impl Tray {
    /// Adds the icon to the notification area. `icon_ico` is a whole .ico file (several sizes);
    /// the size the tray wants at the current DPI is picked from it. Returns `None` if the window
    /// or the icon could not be created, or if the taskbar refused the icon.
    pub fn new(icon_ico: &'static [u8]) -> Option<Tray> {
        let shared = Arc::new(Shared {
            playing: AtomicBool::new(false),
            tooltip: Mutex::new(tooltip_units(DEFAULT_TOOLTIP)),
        });
        let (event_tx, event_rx) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel::<Option<isize>>();
        let (finished_tx, finished_rx) = mpsc::channel::<()>();
        let thread_shared = Arc::clone(&shared);
        let thread = std::thread::Builder::new()
            .name("nabytune-tray".into())
            .spawn(move || {
                // Dropped when this thread ends, however it ends.
                let _finished = finished_tx;
                let _ = catch_unwind(AssertUnwindSafe(|| {
                    run(icon_ico, thread_shared, event_tx, ready_tx);
                }));
            })
            .ok()?;
        match ready_rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Some(hwnd)) => Some(Tray {
                hwnd,
                shared,
                events: Mutex::new(event_rx),
                finished: Mutex::new(finished_rx),
                thread: Some(thread),
            }),
            Ok(None) | Err(RecvTimeoutError::Disconnected) => {
                let _ = thread.join();
                None
            }
            // Still stuck in the shell after 10 s: give up on it and let the thread go.
            Err(RecvTimeoutError::Timeout) => None,
        }
    }

    /// Everything the user did since the last call, oldest first. Never blocks.
    pub fn events(&self) -> Vec<TrayEvent> {
        match self.events.lock() {
            Ok(rx) => rx.try_iter().collect(),
            Err(poisoned) => poisoned.into_inner().try_iter().collect(),
        }
    }

    /// The text shown when hovering the icon, e.g. "Song — Artist". Cut to the 127 UTF-16 units
    /// the shell allows. An empty string shows "NabyTune".
    pub fn set_tooltip(&self, text: &str) {
        let text = if text.trim().is_empty() { DEFAULT_TOOLTIP } else { text };
        let units = tooltip_units(text);
        {
            let mut tip = match self.shared.tooltip.lock() {
                Ok(tip) => tip,
                Err(poisoned) => poisoned.into_inner(),
            };
            if *tip == units {
                return;
            }
            *tip = units;
        }
        unsafe {
            PostMessageW(self.hwnd as HWND, WM_TRAY_TOOLTIP, 0, 0);
        }
    }

    /// Whether the menu offers "Pause" (playing) or "Play".
    pub fn set_playing(&self, playing: bool) {
        self.shared.playing.store(playing, Ordering::Relaxed);
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        unsafe {
            PostMessageW(self.hwnd as HWND, WM_TRAY_QUIT, 0, 0);
        }
        let finished = match self.finished.lock() {
            Ok(rx) => rx.recv_timeout(Duration::from_secs(3)),
            Err(poisoned) => poisoned.into_inner().recv_timeout(Duration::from_secs(3)),
        };
        if let Some(thread) = self.thread.take() {
            // Join only a thread that has ended; one stuck in the shell is let go rather than
            // hanging the app's exit.
            if !matches!(finished, Err(RecvTimeoutError::Timeout)) {
                let _ = thread.join();
            }
        }
    }
}

/// The tray thread's state, reached from the window procedure through GWLP_USERDATA. Every
/// field is a `Cell` or read-only: the procedure is re-entered (by the menu's modal loop and
/// while `Shell_NotifyIconW` waits on Explorer), so it never holds a borrow.
struct State {
    hwnd: Cell<HWND>,
    icon_ico: &'static [u8],
    icon: Cell<HICON>,
    icon_size: Cell<i32>,
    added: Cell<bool>,
    in_menu: Cell<bool>,
    quit_pending: Cell<bool>,
    last_show: Cell<Option<Instant>>,
    taskbar_created: u32,
    shared: Arc<Shared>,
    events: Sender<TrayEvent>,
}

fn run(icon_ico: &'static [u8], shared: Arc<Shared>, events: Sender<TrayEvent>, ready: Sender<Option<isize>>) {
    unsafe {
        // Physical pixels: the icon is picked for the real DPI and the menu is drawn crisp, even
        // if this thread starts before the app has made the process DPI-aware.
        SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);

        let hinstance = GetModuleHandleW(std::ptr::null());
        let class_name = wide(CLASS_NAME);
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: hinstance,
            hIcon: std::ptr::null_mut(),
            hCursor: std::ptr::null_mut(),
            hbrBackground: std::ptr::null_mut(),
            lpszMenuName: std::ptr::null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: std::ptr::null_mut(),
        };
        if RegisterClassExW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
            let _ = ready.send(None);
            return;
        }

        let taskbar_created_name = wide("TaskbarCreated");
        let state = State {
            hwnd: Cell::new(std::ptr::null_mut()),
            icon_ico,
            icon: Cell::new(std::ptr::null_mut()),
            icon_size: Cell::new(0),
            added: Cell::new(false),
            in_menu: Cell::new(false),
            quit_pending: Cell::new(false),
            last_show: Cell::new(None),
            taskbar_created: RegisterWindowMessageW(taskbar_created_name.as_ptr()),
            shared,
            events,
        };

        let title = wide("NabyTune tray");
        // Never shown: WS_POPUP without WS_VISIBLE, and a tool window so it has no taskbar
        // button even if something shows it.
        let hwnd = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            class_name.as_ptr(),
            title.as_ptr(),
            WS_POPUP,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            hinstance,
            &state as *const State as *const c_void,
        );
        if hwnd.is_null() {
            let _ = ready.send(None);
            return;
        }
        state.hwnd.set(hwnd);
        if state.taskbar_created != 0 {
            // Lets the broadcast through when the app runs elevated.
            ChangeWindowMessageFilterEx(hwnd, state.taskbar_created, MSGFLT_ALLOW, std::ptr::null_mut());
        }
        allow_dark_menus(hwnd);

        load_icon(&state);
        if state.icon.get().is_null() {
            DestroyWindow(hwnd);
            let _ = ready.send(None);
            return;
        }

        // Explorer can be slow to answer right after login: retry a little before giving up.
        let mut added = add_icon(&state);
        for _ in 0..5 {
            if added {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
            added = add_icon(&state);
        }
        if !added && taskbar_exists() {
            DestroyWindow(hwnd);
            let _ = ready.send(None);
            return;
        }
        // With no taskbar at all (Explorer not running yet), keep waiting for TaskbarCreated.

        let _ = ready.send(Some(hwnd as isize));

        let mut msg: MSG = std::mem::zeroed();
        loop {
            let got = GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0);
            if got == 0 || got == -1 {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // The loop can only end through WM_DESTROY (or a GetMessage failure); make sure the
        // window, the icon and the shell entry are gone before `state` is.
        if IsWindow(hwnd) != 0 {
            DestroyWindow(hwnd);
        }
        if state.added.get() {
            delete_icon(&state);
        }
        let icon = state.icon.replace(std::ptr::null_mut());
        if !icon.is_null() {
            DestroyIcon(icon);
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = lparam as *const CREATESTRUCTW;
        if !create.is_null() {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, (*create).lpCreateParams as isize);
        }
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const State;
    if state.is_null() {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    if msg == WM_NCDESTROY {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    // A panic must not unwind into user32.
    match catch_unwind(AssertUnwindSafe(|| handle(&*state, hwnd, msg, wparam, lparam))) {
        Ok(Some(result)) => result,
        Ok(None) | Err(_) => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

/// Returns `None` to fall through to `DefWindowProcW`.
unsafe fn handle(state: &State, hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    match msg {
        WM_TRAY_CALLBACK => {
            // NOTIFYICON_VERSION_4: the event is in LOWORD(lParam), the anchor point in wParam.
            match (lparam as u32) & 0xFFFF {
                NIN_SELECT | NIN_KEYSELECT | WM_LBUTTONDBLCLK => show(state),
                WM_CONTEXTMENU => {
                    let x = (wparam & 0xFFFF) as u16 as i16 as i32;
                    let y = ((wparam >> 16) & 0xFFFF) as u16 as i16 as i32;
                    show_menu(state, hwnd, x, y);
                }
                _ => {}
            }
            Some(0)
        }
        WM_TRAY_TOOLTIP => {
            if state.added.get() {
                let mut nid = icon_data(state);
                nid.uFlags = NIF_TIP | NIF_SHOWTIP;
                Shell_NotifyIconW(NIM_MODIFY, &nid);
            }
            Some(0)
        }
        WM_TRAY_QUIT => {
            if state.in_menu.get() {
                // Close the menu first; `show_menu` finishes quitting once TrackPopupMenu returns.
                state.quit_pending.set(true);
                EndMenu();
            } else {
                quit(state, hwnd);
            }
            Some(0)
        }
        WM_DPICHANGED | WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
            refresh_icon(state);
            None
        }
        WM_DESTROY => {
            if state.added.get() {
                delete_icon(state);
            }
            PostQuitMessage(0);
            Some(0)
        }
        _ if state.taskbar_created != 0 && msg == state.taskbar_created => {
            // Explorer restarted (or started): the old icon is gone with it. The DPI may have
            // changed too, so pick the icon size again.
            state.added.set(false);
            let size = tray_icon_size(hwnd);
            if size != state.icon_size.get() {
                load_icon(state);
            }
            add_icon(state);
            Some(0)
        }
        _ => None,
    }
}

fn show(state: &State) {
    let now = Instant::now();
    if let Some(last) = state.last_show.get() {
        if now.duration_since(last) < SHOW_DEBOUNCE {
            return;
        }
    }
    state.last_show.set(Some(now));
    let _ = state.events.send(TrayEvent::Show);
}

unsafe fn show_menu(state: &State, hwnd: HWND, x: i32, y: i32) {
    if state.in_menu.get() {
        return;
    }
    let menu = CreatePopupMenu();
    if menu.is_null() {
        return;
    }
    let playing = state.shared.playing.load(Ordering::Relaxed);
    append(menu, CMD_SHOW, t("Show NabyTune"));
    append_separator(menu);
    append(menu, CMD_PLAY_PAUSE, if playing { t("Pause") } else { t("Play") });
    append(menu, CMD_NEXT, t("Next"));
    append(menu, CMD_PREVIOUS, t("Previous"));
    append_separator(menu);
    append(menu, CMD_QUIT, t("Quit NabyTune"));
    // Bold, like other tray menus: it is what clicking the icon does.
    SetMenuDefaultItem(menu, CMD_SHOW, 0);

    // The menu only closes on a click elsewhere if its owner is the foreground window
    // (KB135788). The shell lets the icon's owner take the foreground on a click.
    SetForegroundWindow(hwnd);
    let horizontal = if GetSystemMetrics(SM_MENUDROPALIGNMENT) != 0 { TPM_RIGHTALIGN } else { TPM_LEFTALIGN };
    state.in_menu.set(true);
    let command = TrackPopupMenu(
        menu,
        TPM_RIGHTBUTTON | TPM_RETURNCMD | TPM_NONOTIFY | TPM_BOTTOMALIGN | horizontal,
        x,
        y,
        0,
        hwnd,
        std::ptr::null(),
    );
    state.in_menu.set(false);
    // Also from KB135788: a message after the menu, so it closes properly next time.
    PostMessageW(hwnd, WM_NULL, 0, 0);
    DestroyMenu(menu);

    let event = match command as u32 {
        CMD_SHOW => Some(TrayEvent::Show),
        CMD_PLAY_PAUSE => Some(TrayEvent::PlayPause),
        CMD_NEXT => Some(TrayEvent::Next),
        CMD_PREVIOUS => Some(TrayEvent::Previous),
        CMD_QUIT => Some(TrayEvent::Quit),
        _ => None,
    };
    if let Some(event) = event {
        let _ = state.events.send(event);
    }
    if state.quit_pending.get() {
        quit(state, hwnd);
    }
}

unsafe fn append(menu: HMENU, id: u32, text: &str) {
    let text = wide(text);
    AppendMenuW(menu, MF_STRING, id as usize, text.as_ptr());
}

unsafe fn append_separator(menu: HMENU) {
    AppendMenuW(menu, MF_SEPARATOR, 0, std::ptr::null());
}

unsafe fn quit(state: &State, hwnd: HWND) {
    if state.added.get() {
        delete_icon(state);
    }
    // WM_DESTROY posts WM_QUIT, which ends the loop in `run`.
    DestroyWindow(hwnd);
}

/// NOTIFYICONDATAW for this icon, with every field the flags may name filled in.
fn icon_data(state: &State) -> NOTIFYICONDATAW {
    let mut nid: NOTIFYICONDATAW = unsafe { std::mem::zeroed() };
    nid.cbSize = std::mem::size_of::<NOTIFYICONDATAW>() as u32;
    nid.hWnd = state.hwnd.get();
    nid.uID = ICON_ID;
    nid.uCallbackMessage = WM_TRAY_CALLBACK;
    nid.hIcon = state.icon.get();
    let tip = match state.shared.tooltip.lock() {
        Ok(tip) => tip.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    };
    for (dst, src) in nid.szTip.iter_mut().zip(tip.iter().take(127)) {
        *dst = *src;
    }
    nid
}

unsafe fn add_icon(state: &State) -> bool {
    let mut nid = icon_data(state);
    nid.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP;
    if Shell_NotifyIconW(NIM_ADD, &nid) == 0 {
        // Already there (e.g. a TaskbarCreated that was not a restart): update it instead.
        if Shell_NotifyIconW(NIM_MODIFY, &nid) == 0 {
            return false;
        }
    }
    nid.Anonymous.uVersion = NOTIFYICON_VERSION_4;
    Shell_NotifyIconW(NIM_SETVERSION, &nid);
    state.added.set(true);
    true
}

unsafe fn delete_icon(state: &State) {
    let nid = icon_data(state);
    Shell_NotifyIconW(NIM_DELETE, &nid);
    state.added.set(false);
}

/// Loads the icon again if the size the tray wants has changed (a DPI or display change).
unsafe fn refresh_icon(state: &State) {
    let size = tray_icon_size(state.hwnd.get());
    if size == state.icon_size.get() {
        return;
    }
    let old = state.icon.get();
    load_icon(state);
    if state.icon.get() != old && state.added.get() {
        let mut nid = icon_data(state);
        nid.uFlags = NIF_ICON;
        Shell_NotifyIconW(NIM_MODIFY, &nid);
    }
}

/// The small-icon size at the window's DPI: 16 at 100 %, 20 at 125 %, 24 at 150 %, 32 at 200 %.
unsafe fn tray_icon_size(hwnd: HWND) -> i32 {
    let dpi = if hwnd.is_null() { 0 } else { GetDpiForWindow(hwnd) };
    let size = if dpi != 0 { GetSystemMetricsForDpi(SM_CXSMICON, dpi) } else { 0 };
    let size = if size > 0 { size } else { GetSystemMetrics(SM_CXSMICON) };
    if size > 0 {
        size
    } else {
        16
    }
}

/// Replaces `state.icon` with the .ico's image that best fits the tray at its current size.
/// Keeps the old icon if the new one cannot be made.
unsafe fn load_icon(state: &State) {
    let size = tray_icon_size(state.hwnd.get());
    let icon = icon_from_ico(state.icon_ico, size);
    if icon.is_null() {
        return;
    }
    let old = state.icon.replace(icon);
    state.icon_size.set(size);
    if !old.is_null() {
        // The shell keeps its own copy of whatever it was given, so ours can go.
        DestroyIcon(old);
    }
}

/// One image of a .ico file. `header` is its directory entry up to and including dwBytesInRes
/// (width, height, colour count, reserved, planes, bit count, size), shared with GRPICONDIRENTRY.
struct IcoEntry {
    header: [u8; 12],
    len: usize,
    offset: usize,
    width: u32,
}

fn parse_ico(bytes: &[u8]) -> Vec<IcoEntry> {
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let mut entries = Vec::new();
    if u16_at(0) != Some(0) || u16_at(2) != Some(1) {
        return entries;
    }
    let count = u16_at(4).unwrap_or(0) as usize;
    for i in 0..count {
        let at = 6 + 16 * i;
        let (Some(header), Some(len), Some(offset)) = (bytes.get(at..at + 12), u32_at(at + 8), u32_at(at + 12)) else {
            break;
        };
        let (len, offset) = (len as usize, offset as usize);
        if len == 0 || offset.checked_add(len).is_none_or(|end| end > bytes.len()) {
            continue;
        }
        let mut h = [0u8; 12];
        h.copy_from_slice(header);
        let width = if h[0] == 0 { 256 } else { h[0] as u32 };
        entries.push(IcoEntry { header: h, len, offset, width });
    }
    entries
}

/// An HICON of `size` px from a .ico file's bytes, made from the image that fits best.
unsafe fn icon_from_ico(ico: &[u8], size: i32) -> HICON {
    let entries = parse_ico(ico);
    let Some(entry) = entries.get(best_image(&entries, size)) else {
        return std::ptr::null_mut();
    };
    let Some(image) = ico.get(entry.offset..entry.offset + entry.len) else {
        return std::ptr::null_mut();
    };
    // An aligned copy: include_bytes! data has no alignment, the bitmap header is read as DWORDs.
    let image: Vec<u8> = image.to_vec();
    CreateIconFromResourceEx(image.as_ptr(), image.len() as u32, 1 as BOOL, 0x0003_0000, size, size, LR_DEFAULTCOLOR)
}

/// The index of the image Windows would pick for `size` px. LookupIconIdFromDirectoryEx wants a
/// resource directory (GRPICONDIR, entries end in a 16-bit id) rather than the file's own
/// directory (entries end in a 32-bit offset), so one is built with id = index + 1.
unsafe fn best_image(entries: &[IcoEntry], size: i32) -> usize {
    if entries.is_empty() {
        return 0;
    }
    let mut dir = Vec::with_capacity(6 + 14 * entries.len());
    dir.extend_from_slice(&0u16.to_le_bytes());
    dir.extend_from_slice(&1u16.to_le_bytes());
    dir.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (i, entry) in entries.iter().enumerate() {
        dir.extend_from_slice(&entry.header); // through dwBytesInRes
        dir.extend_from_slice(&((i + 1) as u16).to_le_bytes());
    }
    let id = LookupIconIdFromDirectoryEx(dir.as_ptr(), 1, size, size, LR_DEFAULTCOLOR);
    if id >= 1 && (id as usize) <= entries.len() {
        return id as usize - 1;
    }
    // The smallest image at least `size` wide, else the largest.
    let mut best = 0;
    for (i, e) in entries.iter().enumerate() {
        let b = &entries[best];
        let fits = e.width >= size as u32;
        let best_fits = b.width >= size as u32;
        if (fits && (!best_fits || e.width < b.width)) || (!fits && !best_fits && e.width > b.width) {
            best = i;
        }
    }
    best
}

fn taskbar_exists() -> bool {
    let class = wide("Shell_TrayWnd");
    unsafe { !FindWindowW(class.as_ptr(), std::ptr::null()).is_null() }
}

/// Lets the popup menu follow Windows' dark app theme. Uses uxtheme's unnamed exports
/// (ordinal 133 AllowDarkModeForWindow, 135 SetPreferredAppMode, 136 FlushMenuThemes), as
/// Explorer, Notepad++ and others do; their meaning is settled from Windows 10 1903 (build
/// 18362), so older builds are left alone. With "AllowDark" the menu stays light when Windows'
/// app mode is light.
unsafe fn allow_dark_menus(hwnd: HWND) {
    if windows_build() < 18362 {
        return;
    }
    let uxtheme = wide("uxtheme.dll");
    let module = LoadLibraryW(uxtheme.as_ptr());
    if module.is_null() {
        return;
    }
    if let Some(allow_for_window) = GetProcAddress(module, 133 as *const u8) {
        let allow_for_window: unsafe extern "system" fn(HWND, BOOL) -> BOOL = std::mem::transmute(allow_for_window);
        allow_for_window(hwnd, 1);
    }
    if let Some(set_mode) = GetProcAddress(module, 135 as *const u8) {
        let set_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(set_mode);
        set_mode(1); // AllowDark
    }
    if let Some(flush) = GetProcAddress(module, 136 as *const u8) {
        let flush: unsafe extern "system" fn() = std::mem::transmute(flush);
        flush();
    }
}

/// Windows' build number (e.g. 26100), or 0 if it cannot be read.
fn windows_build() -> u32 {
    let key = wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let value = wide("CurrentBuildNumber");
    let mut buf = [0u16; 32];
    let mut len = std::mem::size_of_val(&buf) as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr() as *mut c_void,
            &mut len,
        )
    };
    if status != 0 {
        return 0;
    }
    let text: String = String::from_utf16_lossy(&buf).chars().take_while(|c| c.is_ascii_digit()).collect();
    text.parse().unwrap_or(0)
}

/// `text` as NUL-terminated UTF-16, cut to 127 units without splitting a surrogate pair.
fn tooltip_units(text: &str) -> Vec<u16> {
    let mut units: Vec<u16> = text.encode_utf16().filter(|&u| u != 0).collect();
    if units.len() > 127 {
        units.truncate(127);
        if units.last().is_some_and(|&u| (0xD800..0xDC00).contains(&u)) {
            units.pop();
        }
    }
    units.push(0);
    units
}

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows_sys::Win32::Graphics::Gdi::{DeleteObject, GetObjectW, BITMAP};
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetIconInfo, ICONINFO};

    static ICO: &[u8] = include_bytes!("../assets/nabytune.ico");

    #[test]
    fn tooltip_is_cut_to_127_units() {
        let units = tooltip_units(&"x".repeat(300));
        assert_eq!(units.len(), 128);
        assert_eq!(units.last(), Some(&0));
        // A surrogate pair straddling the limit is dropped whole.
        let text = format!("{}🎵", "x".repeat(126));
        let units = tooltip_units(&text);
        assert_eq!(units.len(), 127);
        assert!(units[..126].iter().all(|&u| u == 'x' as u16));
    }

    #[test]
    fn picks_each_tray_size_from_the_ico() {
        let entries = parse_ico(ICO);
        let widths: Vec<u32> = entries.iter().map(|e| e.width).collect();
        assert_eq!(widths, [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]);
        // Each tray size gets its own hand-tuned image rather than a scaled neighbour.
        for (index, size) in [16, 20, 24, 32].into_iter().enumerate() {
            assert_eq!(unsafe { best_image(&entries, size) }, index);
        }
        for size in [16, 20, 24, 32, 40, 48] {
            unsafe {
                let icon = icon_from_ico(ICO, size);
                assert!(!icon.is_null(), "no icon at {size}");
                let mut info: ICONINFO = std::mem::zeroed();
                assert_ne!(GetIconInfo(icon, &mut info), 0);
                let mut bm: BITMAP = std::mem::zeroed();
                GetObjectW(info.hbmColor, std::mem::size_of::<BITMAP>() as i32, &mut bm as *mut _ as *mut c_void);
                assert_eq!((bm.bmWidth, bm.bmHeight), (size, size));
                DeleteObject(info.hbmColor);
                DeleteObject(info.hbmMask);
                DestroyIcon(icon);
            }
        }
        assert!(unsafe { icon_from_ico(b"not an icon", 16) }.is_null());
    }
}
