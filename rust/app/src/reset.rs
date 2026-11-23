use std::{
    fs, io,
    os::windows::fs::MetadataExt,
    path::{Component, Path, PathBuf, Prefix},
    ptr,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
use svyaz_settings::{
    AppState, ConvenienceState, FIRST_TIMESTAMP, PREFERENCES_FILE, SETTINGS_FILE,
};
use windows_sys::Win32::System::Services::{
    CloseServiceHandle, ControlService, DeleteService, OpenSCManagerW, OpenServiceW, SC_HANDLE,
    SC_MANAGER_CONNECT, SERVICE_CONTROL_STOP, SERVICE_STATUS, SERVICE_STOP,
};
use winreg::{
    RegKey,
    enums::{HKEY_LOCAL_MACHINE, KEY_READ, KEY_WRITE},
};

const SERVICES: &str = r"SYSTEM\CurrentControlSet\Services";
const EVENT_SOURCE: &str = r"SYSTEM\CurrentControlSet\Services\EventLog\System";
const DELETE_ACCESS: u32 = 0x10000;

fn cancel_check(cancel: &AtomicBool) -> io::Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(io::Error::new(io::ErrorKind::Interrupted, "Сброс отменён."))
    } else {
        Ok(())
    }
}
fn driver_name(name: &str) -> bool {
    name.get(..9)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("WinDivert"))
        && name.as_bytes()[9..]
            .iter()
            .all(|c| c.is_ascii_digit() || *c == b'.')
}
fn service_names() -> io::Result<Vec<String>> {
    let base = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(SERVICES)?;
    let mut names = Vec::new();
    for key in base.enum_keys() {
        let name = key?;
        if driver_name(&name) {
            names.push(name);
        }
    }
    for known in ["WinDivert", "WinDivert14"] {
        if !names.iter().any(|n| n.eq_ignore_ascii_case(known)) {
            names.push(known.into());
        }
    }
    Ok(names)
}

fn driver_path(image: &str) -> Option<PathBuf> {
    let s = image.trim().trim_matches('"');
    let s = s
        .strip_prefix(r"\??\")
        .or_else(|| s.strip_prefix(r"\\?\"))
        .unwrap_or(s);
    let p = Path::new(s);
    if !p.is_absolute()
        || !matches!(p.components().next()?,Component::Prefix(v) if matches!(v.kind(),Prefix::Disk(_)))
    {
        return None;
    }
    if p.components().any(|c| c == Component::ParentDir) || s.get(2..)?.contains([':', '%', '"']) {
        return None;
    }
    let file = p.file_name()?.to_str()?;
    if !["WinDivert.sys", "WinDivert32.sys", "WinDivert64.sys"]
        .iter()
        .any(|n| file.eq_ignore_ascii_case(n))
    {
        return None;
    }
    Some(p.to_owned())
}
fn safe_ancestors(path: &Path) -> bool {
    for part in path.ancestors() {
        match fs::symlink_metadata(part) {
            Ok(m) if m.file_attributes() & 0x400 != 0 => return false,
            Err(e) if e.kind() != io::ErrorKind::NotFound => return false,
            _ => (),
        }
    }
    true
}
fn owned(path: &Path, packages: &Path) -> bool {
    let mut it = path.components();
    for expected in packages.components() {
        if !it
            .next()
            .is_some_and(|v| v.as_os_str().eq_ignore_ascii_case(expected.as_os_str()))
        {
            return false;
        }
    }
    it.next().is_some() && safe_ancestors(path)
}
fn legacy(path: &Path) -> bool {
    let Some(bin) = path.parent() else {
        return false;
    };
    let Some(root) = bin.parent() else {
        return false;
    };
    if !bin
        .file_name()
        .is_some_and(|s| s.eq_ignore_ascii_case("bin"))
        || !safe_ancestors(path)
    {
        return false;
    }
    if (bin.join("winws.exe").is_file() || bin.join("winws2.exe").is_file())
        && root.join("service.bat").is_file()
    {
        return true;
    }
    let name = root
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_ascii_lowercase();
    let known = name == "zapret"
        || name == "zapret-discord-youtube"
        || name.starts_with("zapret-discord-youtube-");
    known && fs::metadata(path).is_err_and(|e| e.kind() == io::ErrorKind::NotFound)
}
fn service_driver(name: &str) -> io::Result<Option<PathBuf>> {
    let sub = RegKey::predef(HKEY_LOCAL_MACHINE).open_subkey(format!(r"{SERVICES}\{name}"));
    let key = match sub {
        Ok(k) => k,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if key.get_value::<u32, _>("Type")? != 1 {
        return Ok(None);
    }
    let value: String = key.get_value("ImagePath")?;
    Ok(driver_path(&value))
}
pub fn inspect(data: &Path) -> io::Result<Vec<(String, String)>> {
    let packages = data.join("Packages");
    let mut found = Vec::new();
    for name in service_names()? {
        if let Some(path) = service_driver(&name)? {
            if !owned(&path, &packages) && legacy(&path) {
                found.push((name, path.to_string_lossy().into_owned()));
            }
        }
    }
    Ok(found)
}

struct Handle(SC_HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseServiceHandle(self.0);
        }
    }
}
fn ensure_no_external(cancel: &AtomicBool) -> io::Result<()> {
    for attempt in 0..=8 {
        cancel_check(cancel)?;
        let Some(conflict) = super::runner::external()? else {
            return Ok(());
        };
        if attempt == 8 {
            return Err(io::Error::other(format!(
                "Остановите запущенный zapret ({conflict}) перед сбросом."
            )));
        }
        thread::sleep(Duration::from_millis(250));
    }
    unreachable!()
}
fn remove_service(
    manager: &Handle,
    name: &str,
    packages: &Path,
    approved: &[(String, String)],
    cancel: &AtomicBool,
) -> io::Result<()> {
    let wide: Vec<_> = name.encode_utf16().chain(Some(0)).collect();
    let deadline = Instant::now() + Duration::from_secs(12);
    while Instant::now() < deadline {
        cancel_check(cancel)?;
        let raw = unsafe { OpenServiceW(manager.0, wide.as_ptr(), SERVICE_STOP | DELETE_ACCESS) };
        if raw.is_null() {
            let e = io::Error::last_os_error();
            match e.raw_os_error() {
                Some(1060) => return Ok(()),
                Some(1072) => (),
                _ => return Err(e),
            }
        } else {
            let service = Handle(raw);
            let allowed = service_driver(name)?.is_some_and(|path| {
                owned(&path, packages)
                    || legacy(&path)
                        && approved.iter().any(|(key, image)| {
                            key.eq_ignore_ascii_case(name)
                                && Path::new(image)
                                    .as_os_str()
                                    .eq_ignore_ascii_case(path.as_os_str())
                        })
            });
            if !allowed {
                return Err(io::Error::other(format!(
                    "Служба {name} вне пакетов «Связи». Проверьте путь при ручном сбросе. Для стороннего драйвера используйте установившую его программу."
                )));
            }
            let mut status: SERVICE_STATUS = unsafe { std::mem::zeroed() };
            let code =
                if unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &mut status) } == 0 {
                    Some(io::Error::last_os_error())
                } else {
                    None
                };
            match code.and_then(|e| e.raw_os_error()) {
                None | Some(1062) => {
                    if unsafe { DeleteService(service.0) } == 0 {
                        let e = io::Error::last_os_error();
                        if e.raw_os_error() != Some(1072) {
                            return Err(e);
                        }
                    }
                }
                Some(1061 | 1072) => (),
                Some(other) => return Err(io::Error::from_raw_os_error(other)),
            }
        }
        thread::sleep(Duration::from_millis(250));
    }
    Err(io::Error::other(format!(
        "Windows ещё не выгрузила {name}. Закройте использующую драйвер программу или перезагрузите ПК."
    )))
}
fn clear_event_source(packages: &Path, approved: &[(String, String)]) -> io::Result<()> {
    let sources = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey_with_flags(EVENT_SOURCE, KEY_READ | KEY_WRITE)?;
    let source = match sources.open_subkey("WinDivert") {
        Ok(key) => key,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let value: String = match source.get_value("EventMessageFile") {
        Ok(v) => v,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    drop(source);
    if let Some(path) = driver_path(&value) {
        if owned(&path, packages)
            || legacy(&path)
                && approved.iter().any(|(_, image)| {
                    Path::new(image)
                        .as_os_str()
                        .eq_ignore_ascii_case(path.as_os_str())
                })
        {
            sources.delete_subkey("WinDivert")?;
        }
    }
    Ok(())
}

pub fn reset(data: &Path, approved: &[(String, String)], cancel: &AtomicBool) -> io::Result<()> {
    cancel_check(cancel)?;
    ensure_no_external(cancel)?;
    if !data.is_absolute() || !safe_ancestors(data) {
        return Err(io::Error::other("Небезопасный путь к данным приложения."));
    }
    let settings = data.join(SETTINGS_FILE);
    let preferences = data.join(PREFERENCES_FILE);
    let packages = data.join("Packages");
    let mut state = AppState::load(&settings)?;
    let mut prefs = ConvenienceState::load(&preferences)?;
    state.zapret_reset_pending = true;
    state.active_package = None;
    state.pending_package = None;
    state.previous_package = None;
    state.last_strategy = None;
    state.voice_server = None;
    state.voice_log_fingerprint = None;
    state.voice_observed_at = FIRST_TIMESTAMP.into();
    state.last_update_check = FIRST_TIMESTAMP.into();
    state.save(&settings)?;
    prefs.network_strategies.clear();
    prefs.save(&preferences)?;
    ensure_no_external(cancel)?;
    let raw = unsafe { OpenSCManagerW(ptr::null(), ptr::null(), SC_MANAGER_CONNECT) };
    if raw.is_null() {
        return Err(io::Error::last_os_error());
    }
    let manager = Handle(raw);
    for name in service_names()? {
        remove_service(&manager, &name, &packages, approved, cancel)?;
    }
    cancel_check(cancel)?;
    clear_event_source(&packages, approved)?;
    for folder in [packages, data.join("GameFilter")] {
        cancel_check(cancel)?;
        if !safe_ancestors(&folder) {
            return Err(io::Error::other("Небезопасный путь к кэшу zapret."));
        }
        match fs::remove_dir_all(folder) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            other => other?,
        }
    }
    state.zapret_reset_pending = false;
    state.save(&settings)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn service_and_image_filters() {
        assert!(driver_name("WinDivert14"));
        assert!(!driver_name("WinDivertBackup"));
        assert_eq!(
            driver_path(r"\??\C:\zapret\bin\WinDivert64.sys").unwrap(),
            PathBuf::from(r"C:\zapret\bin\WinDivert64.sys")
        );
        for p in [
            r"C:\a\..\WinDivert.sys",
            r"\\host\share\WinDivert.sys",
            r"C:\other.sys",
            r"C:\x:ads\WinDivert.sys",
        ] {
            assert!(driver_path(p).is_none());
        }
    }
}
