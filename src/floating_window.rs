use std::time::Duration;

use gpui::{point, px, size, App, AsyncApp, Bounds, Context, DisplayId, Pixels, Point, Size, Window};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows_sys::Win32::Foundation::{POINT, RECT};
use windows_sys::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, GetWindowRect, SetWindowPos, SM_SWAPBUTTON, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
};

pub(crate) fn hwnd(window: &Window) -> isize {
    match HasWindowHandle::window_handle(window).map(|handle| handle.as_raw()) {
        Ok(RawWindowHandle::Win32(handle)) => handle.hwnd.get(),
        _ => 0,
    }
}

pub(crate) fn initial_bounds(
    cx: &App,
    key: &str,
    requested: Size<Pixels>,
    default_origin: impl FnOnce(Bounds<Pixels>, Size<Pixels>) -> Point<Pixels>,
) -> (Bounds<Pixels>, Option<DisplayId>) {
    let saved = crate::settings::get(key).and_then(|value| {
        let (x, y) = value.split_once(',')?;
        let (x, y) = (x.parse::<f32>().ok()?, y.parse::<f32>().ok()?);
        (x.is_finite() && y.is_finite()).then(|| point(px(x), px(y)))
    });
    let display = saved
        .and_then(|at| {
            let center = Bounds::new(at, requested).center();
            cx.displays()
                .into_iter()
                .find(|d| d.bounds().contains(&center) || d.bounds().contains(&at))
        })
        .or_else(|| cx.primary_display());
    let display_id = display.as_ref().map(|d| d.id());
    let screen = display
        .map(|d| d.visible_bounds())
        .unwrap_or(Bounds::new(Point::default(), size(px(1920.0), px(1080.0))));
    let dimensions = size(requested.width.min(screen.size.width), requested.height.min(screen.size.height));
    let at = saved.unwrap_or_else(|| default_origin(screen, dimensions));
    let origin = point(
        at.x.max(screen.origin.x).min(screen.origin.x + screen.size.width - dimensions.width),
        at.y.max(screen.origin.y).min(screen.origin.y + screen.size.height - dimensions.height),
    );
    (Bounds::new(origin, dimensions), display_id)
}

pub(crate) fn rect(hwnd: isize) -> Option<RECT> {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    (unsafe { GetWindowRect(hwnd as _, &mut rect) } != 0).then_some(rect)
}

pub(crate) fn cursor() -> Option<POINT> {
    let mut cursor = POINT { x: 0, y: 0 };
    (unsafe { GetCursorPos(&mut cursor) } != 0).then_some(cursor)
}

pub(crate) fn work_area(at: POINT) -> Option<RECT> {
    let mut info = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        rcMonitor: RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        rcWork: RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        },
        dwFlags: 0,
    };
    let monitor = unsafe { MonitorFromPoint(at, MONITOR_DEFAULTTONEAREST) };
    (unsafe { GetMonitorInfoW(monitor, &mut info) } != 0).then_some(info.rcWork)
}

pub(crate) fn center(rect: RECT) -> POINT {
    POINT {
        x: rect.left + (rect.right - rect.left) / 2,
        y: rect.top + (rect.bottom - rect.top) / 2,
    }
}

pub(crate) fn place(hwnd: isize, current: RECT, left: i32, top: i32, work: RECT) {
    let current_width = current.right - current.left;
    let width = current_width.min(work.right - work.left);
    let sizing = width != current_width;
    let height = current.bottom - current.top;
    let left = left.clamp(work.left, (work.right - width).max(work.left));
    let top = top.clamp(work.top, (work.bottom - height).max(work.top));
    if sizing || (left, top) != (current.left, current.top) {
        unsafe {
            SetWindowPos(
                hwnd as _,
                std::ptr::null_mut(),
                left,
                top,
                width,
                height,
                SWP_NOZORDER | SWP_NOACTIVATE | if sizing { 0 } else { SWP_NOSIZE },
            );
        }
    }
}

pub(crate) struct DragPosition {
    pub(crate) rect: RECT,
    pub(crate) wanted: POINT,
    pub(crate) work: RECT,
}

/// Follow the pointer without entering Windows' title-bar snap loop.
pub(crate) fn track_drag<T: 'static>(
    window: &Window,
    cx: &mut Context<T>,
    mut moved: impl FnMut(DragPosition, &mut AsyncApp) + 'static,
    done: impl FnOnce(&mut T, &mut Context<T>) + 'static,
) -> bool {
    let hwnd = hwnd(window);
    let (Some(start), Some(original)) = (cursor(), rect(hwnd)) else {
        return false;
    };
    let button = if unsafe { GetSystemMetrics(SM_SWAPBUTTON) } == 0 {
        VK_LBUTTON
    } else {
        VK_RBUTTON
    };
    cx.spawn(async move |this, cx| {
        let mut previous = start;
        loop {
            cx.background_executor().timer(Duration::from_millis(12)).await;
            if this.upgrade().is_none() {
                return;
            }
            let released = unsafe { GetAsyncKeyState(button as i32) } >= 0;
            let (Some(at), Some(rect)) = (cursor(), rect(hwnd)) else { break };
            if (at.x, at.y) != (previous.x, previous.y) {
                if let Some(work) = work_area(at) {
                    let wanted = POINT {
                        x: original.left + at.x - start.x,
                        y: original.top + at.y - start.y,
                    };
                    moved(DragPosition { rect, wanted, work }, cx);
                    previous = at;
                }
            }
            if released {
                break;
            }
        }
        let _ = this.update(cx, done);
    })
    .detach();
    true
}

pub(crate) fn drag<T: 'static>(window: &Window, cx: &mut Context<T>) {
    let hwnd = hwnd(window);
    track_drag(
        window,
        cx,
        move |position, _| {
            place(hwnd, position.rect, position.wanted.x, position.wanted.y, position.work);
        },
        |_, _| {},
    );
}
