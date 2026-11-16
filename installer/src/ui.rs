//! One task dialog that walks confirm -> working (marquee) -> done, navigating in place.
//! TaskDialogIndirect lives only in comctl32 v6 (the embedded manifest selects it), so it is
//! looked up at run time; without it the same pages become plain message boxes.

use std::ptr::{null, null_mut};
use std::thread::JoinHandle;

use windows_sys::core::{HRESULT, PCWSTR};
use windows_sys::Win32::Foundation::{BOOL, HINSTANCE, HWND, LPARAM, S_FALSE, S_OK, WPARAM};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress, LoadLibraryW};
use windows_sys::Win32::UI::Controls::*;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MessageBoxW, SendMessageW, IDCANCEL, IDOK, IDYES, MB_ICONERROR, MB_ICONINFORMATION, MB_OKCANCEL, MB_YESNO, MB_OK, MESSAGEBOX_STYLE,
};
#[cfg(naby_icon)]
use windows_sys::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, LoadImageW, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, SM_CXICON, SM_CXSMICON, WM_SETICON};

use crate::sys::wide;

pub const TITLE: &str = "NabyTune Setup";
const ID_GO: i32 = 100;

#[derive(Clone, Copy, PartialEq)]
pub enum Icon {
    App,
    Error,
}

pub struct Page {
    pub icon: Icon,
    pub heading: String,
    pub content: String,
    /// The checkbox under the content, and whether it starts ticked.
    pub check: Option<(String, bool)>,
    /// The confirm page's go button ("Install"); other pages get Close (or a disabled Cancel).
    pub go: Option<String>,
}

pub type Work = Box<dyn FnOnce(bool) -> Result<Page, String> + Send>;

pub struct Outcome {
    /// The work ran and succeeded.
    pub done: bool,
    /// The final page's checkbox.
    pub checked: bool,
}

type TaskDialogIndirectFn = unsafe extern "system" fn(*const TASKDIALOGCONFIG, *mut i32, *mut i32, *mut BOOL) -> HRESULT;

fn task_dialog_indirect() -> Option<TaskDialogIndirectFn> {
    unsafe {
        let lib = LoadLibraryW(wide("comctl32.dll").as_ptr());
        if lib.is_null() {
            return None;
        }
        GetProcAddress(lib, c"TaskDialogIndirect".as_ptr() as *const u8).map(|f| std::mem::transmute(f))
    }
}

pub fn error_page(e: &str) -> Page {
    Page { icon: Icon::Error, heading: "Setup could not finish".into(), content: e.into(), check: None, go: None }
}

/// Shows `confirm`; if accepted, runs `work` (with the confirm page's checkbox) behind
/// `working`, then shows the page the work returns. None when the user cancelled.
pub fn run(confirm: Page, working: Page, work: Work) -> Option<Outcome> {
    match task_dialog_indirect() {
        Some(f) => run_task_dialog(f, confirm, working, work),
        None => run_message_boxes(confirm, work),
    }
}

/// A plain message (for errors before the wizard starts, or silent-mode fallbacks).
pub fn message(text: &str, error: bool) {
    let style = if error { MB_ICONERROR } else { MB_ICONINFORMATION };
    unsafe { MessageBoxW(null_mut(), wide(text).as_ptr(), wide(TITLE).as_ptr(), MB_OK | style) };
}

// --- message box fallback ----------------------------------------------------------------------

fn mb(text: &str, style: MESSAGEBOX_STYLE) -> i32 {
    unsafe { MessageBoxW(null_mut(), wide(text).as_ptr(), wide(TITLE).as_ptr(), style) }
}

fn run_message_boxes(confirm: Page, work: Work) -> Option<Outcome> {
    let text = format!("{}\n\n{}", confirm.heading, confirm.content);
    if mb(&text, MB_OKCANCEL | MB_ICONINFORMATION) != IDOK {
        return None;
    }
    let flag = match &confirm.check {
        Some((q, _)) => mb(&format!("{q}?"), MB_YESNO | MB_ICONINFORMATION) == IDYES,
        None => false,
    };
    match work(flag) {
        Ok(page) => {
            let text = format!("{}\n\n{}", page.heading, page.content);
            let checked = match &page.check {
                Some((q, _)) => mb(&format!("{text}\n\n{q}?"), MB_YESNO | MB_ICONINFORMATION) == IDYES,
                None => {
                    mb(&text, MB_OK | MB_ICONINFORMATION);
                    false
                }
            };
            Some(Outcome { done: true, checked })
        }
        Err(e) => {
            mb(&format!("Setup could not finish\n\n{e}"), MB_OK | MB_ICONERROR);
            Some(Outcome { done: false, checked: false })
        }
    }
}

// --- task dialog -------------------------------------------------------------------------------

/// A page's config with the UTF-16 strings and buttons it points into (heap buffers, so the
/// pointers stay put when this moves).
struct Built {
    _strings: Vec<Vec<u16>>,
    _buttons: Vec<TASKDIALOG_BUTTON>,
    config: TASKDIALOGCONFIG,
    checked: bool,
    working: bool,
}

#[derive(PartialEq)]
enum Phase {
    Confirm,
    Working,
    Finished,
}

struct State {
    phase: Phase,
    checked: bool,
    work: Option<Work>,
    worker: Option<JoinHandle<Result<Page, String>>>,
    working: Option<Page>,
    pages: Vec<Box<Built>>,
    done: bool,
}

fn build(page: &Page, kind: Phase, state: *mut State) -> Box<Built> {
    let mut strings = Vec::new();
    let mut keep = |s: &str| -> PCWSTR {
        let w = wide(s);
        let p = w.as_ptr();
        strings.push(w);
        p
    };
    let title = keep(TITLE);
    let heading = keep(&page.heading);
    let content = keep(&page.content);
    let check = page.check.as_ref().map(|(t, _)| keep(t)).unwrap_or(null());
    let mut buttons = Vec::new();
    if let Some(go) = &page.go {
        buttons.push(TASKDIALOG_BUTTON { nButtonID: ID_GO, pszButtonText: keep(go) });
    }

    let mut flags = TDF_POSITION_RELATIVE_TO_WINDOW;
    let common;
    match kind {
        Phase::Confirm => {
            flags |= TDF_ALLOW_DIALOG_CANCELLATION;
            common = TDCBF_CANCEL_BUTTON;
        }
        Phase::Working => {
            flags |= TDF_SHOW_MARQUEE_PROGRESS_BAR | TDF_CALLBACK_TIMER;
            common = TDCBF_CANCEL_BUTTON; // disabled once shown
        }
        Phase::Finished => {
            flags |= TDF_ALLOW_DIALOG_CANCELLATION;
            common = TDCBF_CLOSE_BUTTON;
        }
    }
    let checked = page.check.as_ref().is_some_and(|(_, c)| *c);
    if checked {
        flags |= TDF_VERIFICATION_FLAG_CHECKED;
    }

    let (hinstance, main_icon): (HINSTANCE, PCWSTR) = match page.icon {
        Icon::Error => (null_mut(), TD_ERROR_ICON),
        #[cfg(naby_icon)]
        Icon::App => (unsafe { GetModuleHandleW(null()) }, 1 as PCWSTR),
        #[cfg(not(naby_icon))]
        Icon::App => (unsafe { GetModuleHandleW(null()) }, null()),
    };

    let config = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: null_mut(),
        hInstance: hinstance,
        dwFlags: flags,
        dwCommonButtons: common,
        pszWindowTitle: title,
        Anonymous1: TASKDIALOGCONFIG_0 { pszMainIcon: main_icon },
        pszMainInstruction: heading,
        pszContent: content,
        cButtons: buttons.len() as u32,
        pButtons: if buttons.is_empty() { null() } else { buttons.as_ptr() },
        nDefaultButton: if buttons.is_empty() { 0 } else { ID_GO },
        cRadioButtons: 0,
        pRadioButtons: null(),
        nDefaultRadioButton: 0,
        pszVerificationText: check,
        pszExpandedInformation: null(),
        pszExpandedControlText: null(),
        pszCollapsedControlText: null(),
        Anonymous2: TASKDIALOGCONFIG_1 { pszFooterIcon: null() },
        pszFooter: null(),
        pfCallback: Some(callback),
        lpCallbackData: state as isize,
        cxWidth: 0,
    };
    Box::new(Built { _strings: strings, _buttons: buttons, config, checked, working: kind == Phase::Working })
}

unsafe fn navigate(hwnd: HWND, st: &mut State, page: &Page, kind: Phase) {
    let built = build(page, kind, st as *mut State);
    st.checked = built.checked;
    let cfg = &built.config as *const TASKDIALOGCONFIG as LPARAM;
    st.pages.push(built);
    SendMessageW(hwnd, TDM_NAVIGATE_PAGE as u32, 0, cfg);
}

unsafe extern "system" fn callback(hwnd: HWND, msg: TASKDIALOG_NOTIFICATIONS, wparam: WPARAM, _lparam: LPARAM, data: isize) -> HRESULT {
    let st = &mut *(data as *mut State);
    match msg {
        TDN_CREATED => set_window_icon(hwnd),
        TDN_DIALOG_CONSTRUCTED | TDN_NAVIGATED => {
            if st.pages.last().is_some_and(|b| b.working) {
                SendMessageW(hwnd, TDM_SET_PROGRESS_BAR_MARQUEE as u32, 1, 30);
                SendMessageW(hwnd, TDM_ENABLE_BUTTON as u32, IDCANCEL as WPARAM, 0);
            }
        }
        TDN_VERIFICATION_CLICKED => st.checked = wparam != 0,
        TDN_BUTTON_CLICKED => match st.phase {
            Phase::Confirm if wparam as i32 == ID_GO => {
                st.phase = Phase::Working;
                let work = st.work.take().expect("work runs once");
                let flag = st.checked;
                st.worker = Some(std::thread::spawn(move || work(flag)));
                let working = st.working.take().expect("working page");
                navigate(hwnd, st, &working, Phase::Working);
                return S_FALSE;
            }
            Phase::Working => return S_FALSE, // nothing closes the dialog mid-install
            _ => {}
        },
        TDN_TIMER => {
            if st.phase == Phase::Working && st.worker.as_ref().is_some_and(|w| w.is_finished()) {
                let result = st.worker.take().unwrap().join().unwrap_or_else(|_| Err("setup stopped unexpectedly".into()));
                st.phase = Phase::Finished;
                st.done = result.is_ok();
                let page = result.unwrap_or_else(|e| error_page(&e));
                navigate(hwnd, st, &page, Phase::Finished);
            }
        }
        _ => {}
    }
    S_OK
}

#[cfg(naby_icon)]
unsafe fn set_window_icon(hwnd: HWND) {
    let hinst = GetModuleHandleW(null());
    for (which, metric) in [(ICON_SMALL, SM_CXSMICON), (ICON_BIG, SM_CXICON)] {
        let size = GetSystemMetrics(metric);
        let icon = LoadImageW(hinst, 1 as PCWSTR, IMAGE_ICON, size, size, LR_DEFAULTCOLOR);
        if !icon.is_null() {
            SendMessageW(hwnd, WM_SETICON, which as WPARAM, icon as LPARAM);
        }
    }
}

#[cfg(not(naby_icon))]
unsafe fn set_window_icon(_hwnd: HWND) {}

fn run_task_dialog(f: TaskDialogIndirectFn, confirm: Page, working: Page, work: Work) -> Option<Outcome> {
    let mut st = Box::new(State { phase: Phase::Confirm, checked: false, work: Some(work), worker: None, working: Some(working), pages: Vec::new(), done: false });
    let sp: *mut State = &mut *st;
    let first = build(&confirm, Phase::Confirm, sp);
    st.checked = first.checked;
    let cfg: *const TASKDIALOGCONFIG = &first.config;
    st.pages.push(first);
    let mut button = 0i32;
    let mut verified: BOOL = 0;
    let hr = unsafe { f(cfg, &mut button, null_mut(), &mut verified) };
    if let Some(w) = st.worker.take() {
        // Closed while working (should not happen): let the work finish before exiting.
        let r = w.join();
        st.done = matches!(r, Ok(Ok(_)));
        st.phase = Phase::Finished;
    }
    if hr < 0 {
        // The dialog itself failed; fall back to message boxes for the whole flow.
        return match st.work.take() {
            Some(work) => run_message_boxes(confirm, work),
            None => Some(Outcome { done: st.done, checked: false }),
        };
    }
    if st.phase == Phase::Confirm {
        return None;
    }
    Some(Outcome { done: st.done, checked: verified != 0 })
}
