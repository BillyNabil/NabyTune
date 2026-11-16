//! NabyTune-Setup.exe: a per-user installer (no admin) for NabyTune, with the app embedded.
//!
//! NabyTune-Setup.exe [--silent] [--desktop-shortcut | --no-desktop-shortcut] [--launch]
//! NabyTune-Setup.exe --uninstall [--silent] [--remove-data]
//! Either can first --wait-pid <pid>: wait (up to 30 s) for that process to exit. The app's
//! updater runs `--silent --launch --wait-pid <its pid>`, then quits.
//! Testing only: --prefix <dir> (install there instead), --no-shortcuts, --no-register.
//!
//! Installs into %LOCALAPPDATA%\Programs\NabyTune (or wherever an earlier install is), adds a
//! Start menu shortcut (and optionally a desktop one), and registers itself under
//! HKCU\...\Uninstall\NabyTune with a copy of this exe as uninstall.exe. Uninstalling keeps
//! the user's data (%APPDATA%\NabyTune, %LOCALAPPDATA%\NabyTune) unless asked not to.

#![windows_subsystem = "windows"]

mod sys;
mod ui;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use ui::{Icon, Page};

mod payload {
    include!(concat!(env!("OUT_DIR"), "/payload.rs"));
}

const APP_EXE: &str = "NabyTune.exe";
const ICON_FILE: &str = "nabytune.ico";
const UNINSTALLER: &str = "uninstall.exe";
/// How long --wait-pid waits for the app to quit.
const WAIT_LIMIT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Default)]
struct Args {
    uninstall: bool,
    silent: bool,
    desktop: Option<bool>,
    launch: bool,
    remove_data: bool,
    prefix: Option<PathBuf>,
    no_shortcuts: bool,
    no_register: bool,
    wait_pid: Option<u32>,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args::default();
    // Run as ...\uninstall.exe with no arguments (e.g. by hand): uninstall.
    let me = std::env::current_exe().ok();
    if me.as_ref().and_then(|p| p.file_name()).is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(UNINSTALLER)) {
        a.uninstall = true;
    }
    let mut it = std::env::args_os().skip(1);
    while let Some(arg) = it.next() {
        match arg.to_string_lossy().to_lowercase().as_str() {
            "--uninstall" | "/uninstall" => a.uninstall = true,
            "--silent" | "/s" | "/silent" | "/quiet" | "-s" => a.silent = true,
            "--desktop-shortcut" => a.desktop = Some(true),
            "--no-desktop-shortcut" => a.desktop = Some(false),
            "--launch" => a.launch = true,
            "--remove-data" => a.remove_data = true,
            "--prefix" => a.prefix = Some(PathBuf::from(it.next().ok_or("--prefix needs a folder")?)),
            "--no-shortcuts" => a.no_shortcuts = true,
            "--no-register" => a.no_register = true,
            "--wait-pid" => {
                let pid = it.next().ok_or("--wait-pid needs a process id")?;
                let pid = pid.to_string_lossy();
                a.wait_pid = Some(pid.trim().parse().map_err(|_| format!("--wait-pid: not a process id: {pid}"))?);
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    if let Some(p) = &a.prefix {
        if p.is_relative() {
            a.prefix = Some(std::env::current_dir().map_err(|e| e.to_string())?.join(p));
        }
    }
    Ok(a)
}

fn main() {
    // Silent runs report to the console they were started from, if any.
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS);
    }
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            let usage = "Usage: NabyTune-Setup.exe [--silent] [--desktop-shortcut | --no-desktop-shortcut] [--launch] [--wait-pid <pid>]\n       NabyTune-Setup.exe --uninstall [--silent] [--remove-data] [--wait-pid <pid>]";
            eprintln!("{e}\n{usage}");
            let silent = std::env::args().any(|a| matches!(a.to_lowercase().as_str(), "--silent" | "/s" | "/silent" | "/quiet" | "-s"));
            if !silent {
                ui::message(&format!("{e}\n\n{usage}"), true);
            }
            std::process::exit(2);
        }
    };
    // Never hold the install folder open as the working directory.
    let _ = std::env::set_current_dir(std::env::temp_dir());
    // An update started by the app: let it quit on its own before anything is touched. If it
    // is still running after that, closing NabyTune's processes (as always) ends it.
    if let Some(pid) = args.wait_pid {
        if !sys::wait_for_exit(pid, WAIT_LIMIT) {
            eprintln!("NabyTune setup: process {pid} is still running after {} s; closing it", WAIT_LIMIT.as_secs());
        }
    }
    let code = if args.uninstall { uninstall(&args) } else { install(&args) };
    let _ = std::io::stderr().flush();
    std::process::exit(code);
}

fn report(args: &Args, result: Result<(), String>) -> i32 {
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("NabyTune setup: {e}");
            if !args.silent {
                ui::message(&e, true);
            }
            1
        }
    }
}

// --- install -----------------------------------------------------------------------------------

fn install_dir(args: &Args) -> PathBuf {
    if let Some(p) = &args.prefix {
        return p.clone();
    }
    if !args.no_register {
        if let Some(loc) = sys::read_uninstall_value("InstallLocation").filter(|l| !l.trim().is_empty()) {
            return PathBuf::from(loc);
        }
    }
    sys::default_install_dir()
}

struct Plan {
    dir: PathBuf,
    shortcuts: bool,
    register: bool,
}

fn install(args: &Args) -> i32 {
    let plan = Plan { dir: install_dir(args), shortcuts: !args.no_shortcuts, register: !args.no_register };
    let previous = if plan.register { sys::read_uninstall_value("DisplayVersion") } else { None };
    let previous = previous.filter(|_| plan.dir.join(APP_EXE).is_file());
    let had_desktop = plan.shortcuts && sys::desktop_shortcut().is_some_and(|p| p.is_file());

    if args.silent {
        let desktop = args.desktop.unwrap_or(had_desktop);
        let r = do_install(&plan, desktop).map(|_| ());
        // After a failed update, bring back the version that is still there rather than none.
        if args.launch && (r.is_ok() || plan.dir.join(APP_EXE).is_file()) {
            launch(&plan);
        }
        // An update the app started has no console to report to: say what went wrong.
        if let (Err(e), Some(_)) = (&r, args.wait_pid) {
            ui::message(&format!("NabyTune could not be updated.\n\n{e}"), true);
        }
        return report(args, r);
    }

    let running = !sys::processes_in(&plan.dir).is_empty();
    let v = payload::VERSION;
    let (heading, go) = match previous.as_deref() {
        Some(p) if p == v => (format!("Reinstall NabyTune {v}"), "Reinstall"),
        Some(p) => (format!("Update NabyTune {p} to {v}"), "Update"),
        None => (format!("Install NabyTune {v}"), "Install"),
    };
    let mut content = format!("NabyTune will be installed for your account only, in\n{}\n\nNo administrator rights are needed.", plan.dir.display());
    if running {
        content += "\n\nNabyTune is running; setup will close it first.";
    }
    let confirm = Page {
        icon: Icon::App,
        heading,
        content,
        check: plan.shortcuts.then(|| ("Create a desktop shortcut".to_string(), args.desktop.unwrap_or(had_desktop))),
        go: Some(go.into()),
    };
    let working = Page { icon: Icon::App, heading: "Installing NabyTune\u{2026}".into(), content: "Copying files. This takes a few seconds.".into(), check: None, go: None };
    let plan2 = Plan { dir: plan.dir.clone(), shortcuts: plan.shortcuts, register: plan.register };
    let work: ui::Work = Box::new(move |desktop| {
        do_install(&plan2, desktop)?;
        let where_ = if plan2.shortcuts { "You'll find it in the Start menu." } else { "" };
        Ok(Page {
            icon: Icon::App,
            heading: "NabyTune is ready".into(),
            content: format!("NabyTune {} is installed. {where_}", payload::VERSION),
            check: Some(("Launch NabyTune".into(), true)),
            go: None,
        })
    });
    match ui::run(confirm, working, work) {
        None => 1, // cancelled
        Some(o) => {
            if o.done && o.checked {
                launch(&plan);
            }
            if o.done { 0 } else { 1 }
        }
    }
}

fn launch(plan: &Plan) {
    // Through the Start menu shortcut when there is one, so the app gets its AppUserModelID.
    match sys::start_menu_shortcut().filter(|l| plan.shortcuts && l.is_file()) {
        Some(lnk) => sys::launch(&lnk),
        None => sys::launch(&plan.dir.join(APP_EXE)),
    }
}

/// The embedded files: (name, bytes) pairs from the inflated archive.
fn unpack() -> Result<Vec<(String, Vec<u8>)>, String> {
    let raw = miniz_oxide::inflate::decompress_to_vec_with_limit(payload::PACKED, payload::RAW_LEN).map_err(|e| format!("the setup file is damaged ({e:?})"))?;
    let bad = || "the setup file is damaged (bad archive)".to_string();
    let mut files = Vec::new();
    let mut at = 0usize;
    let mut take = |n: usize| -> Result<&[u8], String> {
        let s = raw.get(at..at + n).ok_or_else(bad)?;
        at += n;
        Ok(s)
    };
    let count = u32::from_le_bytes(take(4)?.try_into().unwrap());
    for _ in 0..count {
        let nlen = u16::from_le_bytes(take(2)?.try_into().unwrap()) as usize;
        let name = String::from_utf8(take(nlen)?.to_vec()).map_err(|_| bad())?;
        let len = u64::from_le_bytes(take(8)?.try_into().unwrap()) as usize;
        files.push((name, take(len)?.to_vec()));
    }
    Ok(files)
}

/// Writes `data` beside `dest` and renames it over, retrying while the old file is still locked
/// (an exe that has only just exited, a virus scanner reading it). If it stays locked for some
/// 10 s, the old file is renamed aside to `.old` (Windows allows that even for a running exe),
/// which the next install or the uninstaller removes.
fn put_file(dest: &Path, write: impl Fn(&Path) -> std::io::Result<()>) -> Result<(), String> {
    let tmp = dest.with_extension("new");
    if let Err(e) = write(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("could not write {}: {e}", tmp.display()));
    }
    let mut last = None;
    for attempt in 0..30 {
        match std::fs::rename(&tmp, dest) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(std::time::Duration::from_millis(if attempt < 10 { 200 } else { 400 }));
            }
        }
    }
    let old = dest.with_extension("old");
    let _ = std::fs::remove_file(&old);
    if std::fs::rename(dest, &old).is_ok() {
        if std::fs::rename(&tmp, dest).is_ok() {
            return Ok(());
        }
        let _ = std::fs::rename(&old, dest);
    }
    let _ = std::fs::remove_file(&tmp);
    let why = last.map(|e| e.to_string()).unwrap_or_default();
    Err(format!("could not replace {}: {why}", dest.display()))
}

fn do_install(plan: &Plan, desktop: bool) -> Result<u64, String> {
    let dir = &plan.dir;
    let files = unpack()?;
    sys::close_processes(&sys::processes_in(dir))?;
    std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    // Files an earlier install had to move aside while they were in use.
    for name in files.iter().map(|(n, _)| n.as_str()).chain([UNINSTALLER]) {
        let _ = std::fs::remove_file(dir.join(name).with_extension("old"));
    }

    let mut total = 0u64;
    for (name, data) in &files {
        put_file(&dir.join(name), |p| std::fs::write(p, data))?;
        total += data.len() as u64;
    }
    // Ourselves, as the uninstaller (unless this already is that copy).
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let uninstaller = dir.join(UNINSTALLER);
    if !same_file(&me, &uninstaller) {
        put_file(&uninstaller, |p| std::fs::copy(&me, p).map(|_| ()))?;
    }
    total += std::fs::metadata(&uninstaller).map(|m| m.len()).unwrap_or(0);

    let exe = dir.join(APP_EXE);
    let has_ico = files.iter().any(|(n, _)| n.eq_ignore_ascii_case(ICON_FILE));
    // The app exe carries no icon of its own yet: point at the .ico, else at the setup's.
    let icon: (PathBuf, i32) = if has_ico {
        (dir.join(ICON_FILE), 0)
    } else if cfg!(naby_icon) {
        (uninstaller.clone(), 0)
    } else {
        (exe.clone(), 0)
    };

    if plan.shortcuts {
        let desc = "NabyTune: a YouTube Music player";
        if let Some(lnk) = sys::start_menu_shortcut() {
            sys::create_shortcut(&lnk, &exe, dir, (&icon.0, icon.1), desc, Some(sys::APP_ID))?;
        }
        if let Some(lnk) = sys::desktop_shortcut() {
            if desktop {
                sys::create_shortcut(&lnk, &exe, dir, (&icon.0, icon.1), desc, Some(sys::APP_ID))?;
            } else if lnk.is_file() {
                let _ = std::fs::remove_file(&lnk);
            }
        }
    }

    if plan.register {
        let key = sys::Key::create_uninstall()?;
        let un = format!("\"{}\" --uninstall", uninstaller.display());
        key.set_str("DisplayName", "NabyTune")?;
        key.set_str("DisplayVersion", payload::VERSION)?;
        key.set_str("Publisher", "NabyTune")?;
        key.set_str("DisplayIcon", format!("{},{}", icon.0.display(), icon.1))?;
        key.set_str("InstallLocation", dir)?;
        key.set_str("UninstallString", &un)?;
        key.set_str("QuietUninstallString", format!("{un} --silent"))?;
        key.set_str("InstallDate", today())?;
        key.set_dword("EstimatedSize", (total / 1024).min(u32::MAX as u64) as u32)?;
        key.set_dword("NoModify", 1)?;
        key.set_dword("NoRepair", 1)?;
    }
    Ok(total)
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn today() -> String {
    let mut t = unsafe { std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>() };
    unsafe { windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t) };
    format!("{:04}{:02}{:02}", t.wYear, t.wMonth, t.wDay)
}

// --- uninstall ---------------------------------------------------------------------------------

fn uninstall_dir(args: &Args) -> PathBuf {
    if let Some(p) = &args.prefix {
        return p.clone();
    }
    // The copy in the install folder uninstalls the folder it is in.
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) {
        if dir.join(APP_EXE).is_file() && dir.join(UNINSTALLER).is_file() {
            return dir;
        }
    }
    install_dir(args)
}

fn uninstall(args: &Args) -> i32 {
    let plan = Plan { dir: uninstall_dir(args), shortcuts: !args.no_shortcuts, register: !args.no_register };
    let me = std::env::current_exe().ok();
    let inside = me.as_ref().and_then(|m| m.parent()).is_some_and(|p| same_file(p, &plan.dir));

    let code = if args.silent {
        report(args, do_uninstall(&plan, args.remove_data))
    } else {
        let version = if plan.register { sys::read_uninstall_value("DisplayVersion") } else { None };
        let mut content = format!(
            "This removes NabyTune from\n{}\nalong with its shortcuts and its entry in Installed apps.\n\n\
             Your library, settings and YouTube Music sign-in (in %APPDATA%\\NabyTune and \
             %LOCALAPPDATA%\\NabyTune) are kept, ready if you install NabyTune again, unless you tick the box below.",
            plan.dir.display()
        );
        if !sys::processes_in(&plan.dir).is_empty() {
            content += "\n\nNabyTune is running; it will be closed first.";
        }
        let confirm = Page {
            icon: Icon::App,
            heading: match version {
                Some(v) => format!("Uninstall NabyTune {v}?"),
                None => "Uninstall NabyTune?".into(),
            },
            content,
            check: Some(("Also delete my library, settings and sign-in".into(), args.remove_data)),
            go: Some("Uninstall".into()),
        };
        let working = Page { icon: Icon::App, heading: "Uninstalling NabyTune\u{2026}".into(), content: "Removing files.".into(), check: None, go: None };
        let plan2 = Plan { dir: plan.dir.clone(), shortcuts: plan.shortcuts, register: plan.register };
        let work: ui::Work = Box::new(move |remove_data| {
            do_uninstall(&plan2, remove_data)?;
            Ok(Page {
                icon: Icon::App,
                heading: "NabyTune has been uninstalled".into(),
                content: if remove_data {
                    "Your NabyTune data was deleted too.".into()
                } else {
                    "Your library, settings and sign-in were kept.".into()
                },
                check: None,
                go: None,
            })
        });
        match ui::run(confirm, working, work) {
            None => return 1, // cancelled: leave everything, including ourselves
            Some(o) => if o.done { 0 } else { 1 },
        }
    };

    if inside {
        if let Some(me) = me {
            schedule_self_delete(&me, &plan.dir);
        }
    }
    code
}

fn do_uninstall(plan: &Plan, remove_data: bool) -> Result<(), String> {
    let dir = &plan.dir;
    sys::close_processes(&sys::processes_in(dir))?;

    if plan.shortcuts {
        for lnk in [sys::start_menu_shortcut(), sys::desktop_shortcut()].into_iter().flatten() {
            if lnk.is_file() {
                std::fs::remove_file(&lnk).map_err(|e| format!("could not remove {}: {e}", lnk.display()))?;
            }
        }
    }
    if plan.register {
        sys::delete_uninstall_key()?;
    }

    let me = std::env::current_exe().ok();
    let names = payload::FILES.iter().copied().chain([UNINSTALLER]);
    let mut errors = Vec::new();
    for name in names {
        let path = dir.join(name);
        if me.as_ref().is_some_and(|m| same_file(m, &path)) {
            continue; // deleted after we exit
        }
        for leftover in [path.with_extension("new"), path.with_extension("old"), path.clone()] {
            if leftover.exists() {
                if let Err(e) = std::fs::remove_file(&leftover) {
                    errors.push(format!("{}: {e}", leftover.display()));
                }
            }
        }
    }
    let _ = std::fs::remove_dir(dir); // only if empty; the uninstaller's own folder goes later

    if remove_data {
        for d in sys::data_dirs() {
            if d.exists() {
                if let Err(e) = std::fs::remove_dir_all(&d) {
                    errors.push(format!("{}: {e}", d.display()));
                }
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!("Some files could not be removed:\n{}", errors.join("\n")))
    }
}

/// A running exe cannot delete itself: a hidden cmd waits for us to exit, then deletes this
/// exe and the (by then empty) install folder.
fn schedule_self_delete(me: &Path, dir: &Path) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let system = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows")).join("System32");
    let script = format!(
        "/d /c ping -n 3 127.0.0.1 >nul & del /f /q \"{}\" & rmdir \"{}\"",
        me.display(),
        dir.display()
    );
    let _ = Command::new(system.join("cmd.exe"))
        .raw_arg(script)
        .current_dir(std::env::temp_dir())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}
