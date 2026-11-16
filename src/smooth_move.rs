//! Dragging a window by its title bar, smoothly.
//!
//! GPUI redraws a window from scratch, every view and every element, each time Windows says
//! it moved (a refresh skips its view cache). While a window is dragged, Windows says so on
//! every mouse move, so NabyTune drew its whole page over and over just to stay where it was
//! inside the window, and the drag stuttered behind the mouse.
//!
//! The window's messages pass through here first: while Windows is moving the window (between
//! "enter size/move" and "exit size/move"), its "moved" messages wait, and only the last one is
//! passed on when the drag ends. The picture moves with the window as it is (Windows moves it,
//! it needs no redraw), and GPUI hears where the window ended up once.

use std::cell::RefCell;
use std::collections::HashMap;

use gpui::Window;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, SetWindowLongPtrW, GWLP_WNDPROC, WM_ENTERSIZEMOVE, WM_EXITSIZEMOVE, WM_MOVE, WM_NCDESTROY,
};

/// A window whose messages pass through here.
struct Hooked {
    /// GPUI's own handler, which gets every message in the end.
    original: isize,
    /// Windows is moving or sizing the window.
    moving: bool,
    /// The last "moved" held back during the drag.
    held: Option<LPARAM>,
}

thread_local! {
    // Windows' messages arrive on the thread that made the window: GPUI's main thread.
    static HOOKED: RefCell<HashMap<isize, Hooked>> = RefCell::new(HashMap::new());
}

/// Holds this window's "moved" messages while it is dragged (once per window).
pub(crate) fn smooth_moves(window: &Window) {
    let Ok(handle) = HasWindowHandle::window_handle(window) else { return };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else { return };
    let hwnd = handle.hwnd.get();
    if HOOKED.with(|h| h.borrow().contains_key(&hwnd)) {
        return;
    }
    let ours: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = proc;
    let original = unsafe { SetWindowLongPtrW(hwnd as HWND, GWLP_WNDPROC, ours as usize as isize) };
    if original == 0 {
        return;
    }
    HOOKED.with(|h| h.borrow_mut().insert(hwnd, Hooked { original, moving: false, held: None }));
}

unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let key = hwnd as isize;
    // What to do is settled with the table let go: GPUI's handler can send this window more
    // messages, which come back in here.
    enum Then {
        Pass(isize),
        Hold,
        Release(isize, Option<LPARAM>),
        Unhook(isize),
    }
    let then = HOOKED.with(|h| {
        let mut h = h.borrow_mut();
        let hooked = h.get_mut(&key)?;
        Some(match msg {
            WM_ENTERSIZEMOVE => {
                hooked.moving = true;
                Then::Pass(hooked.original)
            }
            WM_MOVE if hooked.moving => {
                hooked.held = Some(lparam);
                Then::Hold
            }
            WM_EXITSIZEMOVE => {
                hooked.moving = false;
                Then::Release(hooked.original, hooked.held.take())
            }
            WM_NCDESTROY => {
                let original = hooked.original;
                h.remove(&key);
                Then::Unhook(original)
            }
            _ => Then::Pass(hooked.original),
        })
    });
    let call = |original: isize, msg: u32, wparam: WPARAM, lparam: LPARAM| unsafe {
        let original: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT = std::mem::transmute(original);
        CallWindowProcW(Some(original), hwnd, msg, wparam, lparam)
    };
    match then {
        None => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
        Some(Then::Pass(original)) => call(original, msg, wparam, lparam),
        Some(Then::Hold) => 0,
        Some(Then::Release(original, held)) => {
            let result = call(original, msg, wparam, lparam);
            if let Some(at) = held {
                call(original, WM_MOVE, 0, at);
            }
            result
        }
        Some(Then::Unhook(original)) => unsafe {
            SetWindowLongPtrW(hwnd, GWLP_WNDPROC, original);
            call(original, msg, wparam, lparam)
        },
    }
}
