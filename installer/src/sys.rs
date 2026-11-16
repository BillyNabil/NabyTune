//! The Windows pieces: known folders, the uninstall registry key, .lnk shortcuts (COM, with
//! hand-written vtables since windows-sys has no interfaces), and NabyTune's running processes.

use std::ffi::{c_void, OsStr, OsString};
use std::os::windows::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};
use std::ptr::{null, null_mut};
use std::time::{Duration, Instant};

use windows_sys::core::{GUID, HRESULT, PCWSTR, PWSTR};
use windows_sys::Win32::Foundation::{CloseHandle, BOOL, ERROR_SUCCESS, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, WAIT_TIMEOUT};
use windows_sys::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
use windows_sys::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegGetValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_DWORD, REG_OPTION_NON_VOLATILE, REG_SZ, RRF_RT_REG_SZ,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};
use windows_sys::Win32::UI::Shell::{ShellExecuteW, SHGetKnownFolderPath, ShellLink, FOLDERID_Desktop, FOLDERID_Programs, FOLDERID_UserProgramFiles, KF_FLAG_CREATE};
use windows_sys::Win32::UI::WindowsAndMessaging::{EnumWindows, GetWindowThreadProcessId, IsWindowVisible, PostMessageW, SW_SHOWNORMAL, WM_CLOSE};

pub const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\NabyTune";
pub const APP_ID: &str = "NabyTune.Player";

pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(Some(0)).collect()
}

fn from_wide_ptr(p: *const u16) -> OsString {
    unsafe {
        let len = (0..).take_while(|&i| *p.add(i) != 0).count();
        OsString::from_wide(std::slice::from_raw_parts(p, len))
    }
}

fn known_folder(id: &GUID, flags: i32) -> Option<PathBuf> {
    unsafe {
        let mut p: PWSTR = null_mut();
        let hr = SHGetKnownFolderPath(id, flags as u32, null_mut(), &mut p);
        let out = (hr >= 0 && !p.is_null()).then(|| PathBuf::from(from_wide_ptr(p)));
        if !p.is_null() {
            CoTaskMemFree(p as *const c_void);
        }
        out
    }
}

/// %LOCALAPPDATA%\Programs\NabyTune (FOLDERID_UserProgramFiles), the per-user install dir.
pub fn default_install_dir() -> PathBuf {
    known_folder(&FOLDERID_UserProgramFiles, KF_FLAG_CREATE)
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Programs")))
        .unwrap_or_else(|| PathBuf::from(r"C:\NabyTune"))
        .join("NabyTune")
}

pub fn start_menu_shortcut() -> Option<PathBuf> {
    known_folder(&FOLDERID_Programs, 0).map(|d| d.join("NabyTune.lnk"))
}

pub fn desktop_shortcut() -> Option<PathBuf> {
    known_folder(&FOLDERID_Desktop, 0).map(|d| d.join("NabyTune.lnk"))
}

/// NabyTune's own data: what the app keeps (%APPDATA%\NabyTune) and the sign-in window's
/// WebView2 profile (%LOCALAPPDATA%\NabyTune). Read from the environment, as the app does.
pub fn data_dirs() -> Vec<PathBuf> {
    ["APPDATA", "LOCALAPPDATA"]
        .iter()
        .filter_map(|v| std::env::var_os(v))
        .map(|d| PathBuf::from(d).join("NabyTune"))
        .collect()
}

// --- registry ----------------------------------------------------------------------------------

pub struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        unsafe { RegCloseKey(self.0) };
    }
}

impl Key {
    pub fn create_uninstall() -> Result<Key, String> {
        let mut hkey: HKEY = null_mut();
        let err = unsafe {
            RegCreateKeyExW(HKEY_CURRENT_USER, wide(UNINSTALL_KEY).as_ptr(), 0, null(), REG_OPTION_NON_VOLATILE, KEY_SET_VALUE, null(), &mut hkey, null_mut())
        };
        if err != ERROR_SUCCESS {
            return Err(format!("could not create HKCU\\{UNINSTALL_KEY} (error {err})"));
        }
        Ok(Key(hkey))
    }

    pub fn set_str(&self, name: &str, value: impl AsRef<OsStr>) -> Result<(), String> {
        let data = wide(value);
        let err = unsafe { RegSetValueExW(self.0, wide(name).as_ptr(), 0, REG_SZ, data.as_ptr() as *const u8, (data.len() * 2) as u32) };
        if err != ERROR_SUCCESS {
            return Err(format!("could not write registry value {name} (error {err})"));
        }
        Ok(())
    }

    pub fn set_dword(&self, name: &str, value: u32) -> Result<(), String> {
        let err = unsafe { RegSetValueExW(self.0, wide(name).as_ptr(), 0, REG_DWORD, &value as *const u32 as *const u8, 4) };
        if err != ERROR_SUCCESS {
            return Err(format!("could not write registry value {name} (error {err})"));
        }
        Ok(())
    }
}

pub fn read_uninstall_value(name: &str) -> Option<String> {
    unsafe {
        let key = wide(UNINSTALL_KEY);
        let value = wide(name);
        let mut size = 0u32;
        if RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, null_mut(), null_mut(), &mut size) != ERROR_SUCCESS {
            return None;
        }
        let mut buf = vec![0u16; size as usize / 2 + 1];
        let mut size = (buf.len() * 2) as u32;
        if RegGetValueW(HKEY_CURRENT_USER, key.as_ptr(), value.as_ptr(), RRF_RT_REG_SZ, null_mut(), buf.as_mut_ptr() as *mut c_void, &mut size) != ERROR_SUCCESS {
            return None;
        }
        Some(from_wide_ptr(buf.as_ptr()).to_string_lossy().into_owned())
    }
}

pub fn delete_uninstall_key() -> Result<(), String> {
    let err = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, wide(UNINSTALL_KEY).as_ptr()) };
    // 2 = ERROR_FILE_NOT_FOUND: nothing to remove.
    if err != ERROR_SUCCESS && err != 2 {
        return Err(format!("could not delete HKCU\\{UNINSTALL_KEY} (error {err})"));
    }
    Ok(())
}

// --- shortcuts ---------------------------------------------------------------------------------

type Fn0 = unsafe extern "system" fn(*mut c_void) -> u32;
type FnStr = unsafe extern "system" fn(*mut c_void, PCWSTR) -> HRESULT;

#[repr(C)]
struct IUnknownVtbl {
    query_interface: unsafe extern "system" fn(*mut c_void, *const GUID, *mut *mut c_void) -> HRESULT,
    add_ref: Fn0,
    release: Fn0,
}

#[repr(C)]
struct IShellLinkWVtbl {
    base: IUnknownVtbl,
    get_path: usize,
    get_id_list: usize,
    set_id_list: usize,
    get_description: usize,
    set_description: FnStr,
    get_working_directory: usize,
    set_working_directory: FnStr,
    get_arguments: usize,
    set_arguments: FnStr,
    get_hotkey: usize,
    set_hotkey: usize,
    get_show_cmd: usize,
    set_show_cmd: usize,
    get_icon_location: usize,
    set_icon_location: unsafe extern "system" fn(*mut c_void, PCWSTR, i32) -> HRESULT,
    set_relative_path: usize,
    resolve: usize,
    set_path: FnStr,
}

#[repr(C)]
struct IPersistFileVtbl {
    base: IUnknownVtbl,
    get_class_id: usize,
    is_dirty: usize,
    load: usize,
    save: unsafe extern "system" fn(*mut c_void, PCWSTR, BOOL) -> HRESULT,
    save_completed: usize,
    get_cur_file: usize,
}

#[repr(C)]
struct PropertyKey {
    fmtid: GUID,
    pid: u32,
}

/// PROPVARIANT holding a VT_LPWSTR (24 bytes on x64, 16 on x86).
#[repr(C)]
struct PropVariantStr {
    vt: u16,
    reserved: [u16; 3],
    value: *const u16,
    pad: usize,
}

#[repr(C)]
struct IPropertyStoreVtbl {
    base: IUnknownVtbl,
    get_count: usize,
    get_at: usize,
    get_value: usize,
    set_value: unsafe extern "system" fn(*mut c_void, *const PropertyKey, *const PropVariantStr) -> HRESULT,
    commit: unsafe extern "system" fn(*mut c_void) -> HRESULT,
}

const IID_ISHELLLINKW: GUID = GUID::from_u128(0x000214f9_0000_0000_c000_000000000046);
const IID_IPERSISTFILE: GUID = GUID::from_u128(0x0000010b_0000_0000_c000_000000000046);
const IID_IPROPERTYSTORE: GUID = GUID::from_u128(0x886d8eeb_8cf2_4446_8d02_cdba1dbdcf99);
/// PKEY_AppUserModel_ID
const PKEY_APP_USER_MODEL_ID: PropertyKey = PropertyKey { fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3), pid: 5 };

/// A COM pointer, released on drop.
struct Com<V>(*mut *const V);

impl<V> Com<V> {
    unsafe fn vtbl(&self) -> &V {
        &**self.0
    }
    fn this(&self) -> *mut c_void {
        self.0 as *mut c_void
    }
    unsafe fn cast<W>(&self, iid: &GUID) -> Option<Com<W>> {
        let unk = &*(*self.0 as *const IUnknownVtbl);
        let mut out = null_mut();
        ((unk.query_interface)(self.this(), iid, &mut out) >= 0 && !out.is_null()).then(|| Com(out as *mut *const W))
    }
}

impl<V> Drop for Com<V> {
    fn drop(&mut self) {
        unsafe {
            let unk = &*(*self.0 as *const IUnknownVtbl);
            (unk.release)(self.this());
        }
    }
}

fn check(hr: HRESULT, what: &str) -> Result<(), String> {
    if hr < 0 {
        Err(format!("{what} failed (0x{:08X})", hr as u32))
    } else {
        Ok(())
    }
}

/// Writes a .lnk at `lnk` launching `target`, with `icon` and, optionally, an AppUserModelID.
pub fn create_shortcut(lnk: &Path, target: &Path, workdir: &Path, icon: (&Path, i32), description: &str, app_id: Option<&str>) -> Result<(), String> {
    unsafe {
        let init = CoInitializeEx(null(), COINIT_APARTMENTTHREADED as u32);
        let result = (|| {
            let mut raw = null_mut();
            check(CoCreateInstance(&ShellLink, null_mut(), CLSCTX_INPROC_SERVER, &IID_ISHELLLINKW, &mut raw), "creating a shell link")?;
            let link: Com<IShellLinkWVtbl> = Com(raw as *mut *const IShellLinkWVtbl);
            let v = link.vtbl();
            check((v.set_path)(link.this(), wide(target).as_ptr()), "IShellLinkW::SetPath")?;
            check((v.set_working_directory)(link.this(), wide(workdir).as_ptr()), "IShellLinkW::SetWorkingDirectory")?;
            check((v.set_arguments)(link.this(), wide("").as_ptr()), "IShellLinkW::SetArguments")?;
            check((v.set_description)(link.this(), wide(description).as_ptr()), "IShellLinkW::SetDescription")?;
            check((v.set_icon_location)(link.this(), wide(icon.0).as_ptr(), icon.1), "IShellLinkW::SetIconLocation")?;
            if let Some(id) = app_id {
                if let Some(store) = link.cast::<IPropertyStoreVtbl>(&IID_IPROPERTYSTORE) {
                    let id = wide(id);
                    let pv = PropVariantStr { vt: 31 /* VT_LPWSTR */, reserved: [0; 3], value: id.as_ptr(), pad: 0 };
                    let sv = store.vtbl();
                    check((sv.set_value)(store.this(), &PKEY_APP_USER_MODEL_ID, &pv), "IPropertyStore::SetValue")?;
                    check((sv.commit)(store.this()), "IPropertyStore::Commit")?;
                }
            }
            let file: Com<IPersistFileVtbl> = link.cast(&IID_IPERSISTFILE).ok_or("no IPersistFile on the shell link")?;
            if let Some(parent) = lnk.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            check((file.vtbl().save)(file.this(), wide(lnk).as_ptr(), 1), "saving the shortcut")?;
            Ok(())
        })();
        if init >= 0 {
            CoUninitialize();
        }
        result.map_err(|e: String| format!("{}: {e}", lnk.display()))
    }
}

// --- processes ---------------------------------------------------------------------------------

fn norm(p: &Path) -> String {
    p.to_string_lossy().trim_end_matches('\\').to_lowercase()
}

/// Processes (other than this one) running an exe that lives directly in `dir`.
pub fn processes_in(dir: &Path) -> Vec<(u32, PathBuf)> {
    let dir = norm(dir);
    let me = unsafe { GetCurrentProcessId() };
    let mut found = Vec::new();
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if snap == INVALID_HANDLE_VALUE {
            return found;
        }
        let mut entry: PROCESSENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
        let mut ok = Process32FirstW(snap, &mut entry);
        while ok != 0 {
            let pid = entry.th32ProcessID;
            if pid != me && pid != 0 {
                if let Some(path) = process_path(pid) {
                    if path.parent().map(norm).as_deref() == Some(dir.as_str()) {
                        found.push((pid, path));
                    }
                }
            }
            ok = Process32NextW(snap, &mut entry);
        }
        CloseHandle(snap);
    }
    found
}

fn process_path(pid: u32) -> Option<PathBuf> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len);
        CloseHandle(h);
        (ok != 0).then(|| PathBuf::from(OsString::from_wide(&buf[..len as usize])))
    }
}

unsafe extern "system" fn close_windows_of(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let pids = &*(lparam as *const Vec<u32>);
    let mut pid = 0u32;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pids.contains(&pid) && IsWindowVisible(hwnd) != 0 {
        PostMessageW(hwnd, WM_CLOSE, 0, 0);
    }
    1
}

/// Asks each process's windows to close, then ends whatever is still running after a few seconds.
pub fn close_processes(procs: &[(u32, PathBuf)]) -> Result<(), String> {
    if procs.is_empty() {
        return Ok(());
    }
    let pids: Vec<u32> = procs.iter().map(|(p, _)| *p).collect();
    unsafe {
        let handles: Vec<HANDLE> = pids.iter().map(|&pid| OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_TERMINATE, 0, pid)).filter(|h| !h.is_null()).collect();
        EnumWindows(Some(close_windows_of), &pids as *const Vec<u32> as LPARAM);
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut failed = None;
        for &h in &handles {
            let left = deadline.saturating_duration_since(Instant::now()).as_millis() as u32;
            if WaitForSingleObject(h, left) == WAIT_TIMEOUT {
                TerminateProcess(h, 1);
                if WaitForSingleObject(h, 5000) == WAIT_TIMEOUT {
                    failed = Some("NabyTune is still running and could not be closed");
                }
            }
            CloseHandle(h);
        }
        match failed {
            Some(e) => Err(e.into()),
            None => Ok(()),
        }
    }
}

/// Waits up to `limit` for process `pid` to exit. True once it has, or if it cannot be opened
/// (it is gone already); false if it is still running.
pub fn wait_for_exit(pid: u32, limit: Duration) -> bool {
    unsafe {
        if pid == 0 || pid == GetCurrentProcessId() {
            return true;
        }
        let h = OpenProcess(PROCESS_SYNCHRONIZE, 0, pid);
        if h.is_null() {
            return true;
        }
        let ms = limit.as_millis().min(u32::MAX as u128 - 1) as u32;
        let r = WaitForSingleObject(h, ms);
        CloseHandle(h);
        r != WAIT_TIMEOUT
    }
}

pub fn launch(exe: &Path) {
    let dir = exe.parent().map(wide).unwrap_or_else(|| vec![0]);
    unsafe {
        ShellExecuteW(null_mut(), wide("open").as_ptr(), wide(exe).as_ptr(), null(), dir.as_ptr(), SW_SHOWNORMAL);
    }
}
