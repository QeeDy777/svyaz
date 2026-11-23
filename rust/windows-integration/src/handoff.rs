use std::{
    ffi::c_void,
    io,
    marker::PhantomData,
    os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle},
    path::Path,
    process::{Child, Command},
    rc::Rc,
    time::{Duration, Instant},
};
use uuid::Uuid;

pub const PARENT_REFRESHES_ICONS: &str = "--parent-refreshes-shortcut-icons";
const SYNCHRONIZE: u32 = 0x100000;
const EVENT_MODIFY_STATE: u32 = 2;
const SIGNALED: u32 = 0;
const TIMEOUT: u32 = 258;
const ALREADY_EXISTS: i32 = 183;

#[link(name = "kernel32")]
extern "system" {
    fn CreateMutexW(attributes: *const c_void, owned: i32, name: *const u16) -> *mut c_void;
    fn ReleaseMutex(mutex: *mut c_void) -> i32;
    fn CreateEventW(
        attributes: *const c_void,
        manual: i32,
        initial: i32,
        name: *const u16,
    ) -> *mut c_void;
    fn OpenEventW(access: u32, inherit: i32, name: *const u16) -> *mut c_void;
    fn SetEvent(event: *mut c_void) -> i32;
    fn WaitForSingleObject(handle: *mut c_void, timeout: u32) -> u32;
    fn CreateJobObjectW(attributes: *const c_void, name: *const u16) -> *mut c_void;
    fn AssignProcessToJobObject(job: *mut c_void, process: *mut c_void) -> i32;
    fn TerminateJobObject(job: *mut c_void, code: u32) -> i32;
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn handle(raw: *mut c_void) -> io::Result<OwnedHandle> {
    if raw.is_null() {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { OwnedHandle::from_raw_handle(raw) })
    }
}
fn check(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn name(token: &str) -> io::Result<Vec<u16>> {
    if token.len() != 32 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Неверный токен обновления.",
        ));
    }
    Ok(wide(&format!("Local\\Svyaz-Update-{token}")))
}
// Windows mutex ownership is per thread: its guard must be dropped on the acquiring thread.
pub struct Instance {
    mutex: OwnedHandle,
    _thread: PhantomData<Rc<()>>,
}
impl Instance {
    pub fn acquire() -> io::Result<Option<Self>> {
        unsafe {
            let raw = CreateMutexW(std::ptr::null(), 1, wide("Global\\DiscordRoute").as_ptr());
            let status = io::Error::last_os_error();
            let mutex = handle(raw)?;
            if status.raw_os_error() == Some(ALREADY_EXISTS) {
                return Ok(None);
            }
            Ok(Some(Self {
                mutex,
                _thread: PhantomData,
            }))
        }
    }
}
impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            ReleaseMutex(self.mutex.as_raw_handle());
        }
    }
}

pub fn signal_ready(token: &str) -> io::Result<()> {
    unsafe {
        let event = handle(OpenEventW(EVENT_MODIFY_STATE, 0, name(token)?.as_ptr()))?;
        check(SetEvent(event.as_raw_handle()))
    }
}
/// Called off the UI thread after signalling an updater from before 0.13.1.
pub fn wait_for_legacy_parent(token: &str) -> io::Result<()> {
    let event = name(token)?;
    loop {
        match handle(unsafe { OpenEventW(SYNCHRONIZE, 0, event.as_ptr()) }) {
            Ok(h) => drop(h),
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

pub fn start_confirmed(
    exe: &Path,
    args: &[String],
    timeout: Duration,
    commit: impl FnOnce() -> io::Result<()>,
) -> io::Result<()> {
    let token = Uuid::new_v4().simple().to_string();
    let signal = handle(unsafe { CreateEventW(std::ptr::null(), 1, 0, name(&token)?.as_ptr()) })?;
    let job = handle(unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) })?;
    let mut child = Command::new(exe)
        .current_dir(
            exe.parent()
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Нет каталога EXE."))?,
        )
        .args(args)
        .args([PARENT_REFRESHES_ICONS, "--update-ready", &token])
        .spawn()?;
    if let Err(e) =
        check(unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) })
    {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e);
    }
    let wait = |child: &mut Child| -> io::Result<()> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if child.try_wait()?.is_some() {
                break;
            }
            match unsafe { WaitForSingleObject(signal.as_raw_handle(), 100) } {
                SIGNALED if child.try_wait()?.is_none() => return commit(),
                SIGNALED => break,
                TIMEOUT => continue,
                _ => return Err(io::Error::last_os_error()),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "Новая версия не подтвердила запуск.",
        ))
    };
    match wait(&mut child) {
        Ok(()) => Ok(()),
        Err(reason) => {
            check(unsafe { TerminateJobObject(job.as_raw_handle(), 1) })?;
            if unsafe { WaitForSingleObject(child.as_raw_handle(), 5000) } != SIGNALED {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "Не удалось остановить новую версию.",
                ));
            }
            Err(reason)
        }
    }
}
