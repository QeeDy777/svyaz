use std::{
    ffi::c_void,
    fs, io, mem,
    os::windows::{
        io::{AsRawHandle, FromRawHandle, OwnedHandle},
        process::CommandExt,
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    ptr,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};
use svyaz_connection::{Runner, StartError};
use svyaz_packages::{
    Package,
    strategy::{Strategy, arguments},
};
use svyaz_settings::GameFilterSettings;
use windows_sys::Win32::{
    Foundation::{ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE, WAIT_TIMEOUT},
    System::{
        Diagnostics::ToolHelp::*,
        JobObjects::*,
        Threading::{CREATE_NO_WINDOW, WaitForSingleObject},
    },
};

pub(super) fn external() -> io::Result<Option<String>> {
    unsafe {
        let raw = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        if raw == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        let snapshot = OwnedHandle::from_raw_handle(raw);
        let mut entry: PROCESSENTRY32W = mem::zeroed();
        entry.dwSize = mem::size_of_val(&entry) as _;
        let mut found = Process32FirstW(snapshot.as_raw_handle(), &mut entry);
        while found != 0 {
            let len = entry
                .szExeFile
                .iter()
                .position(|c| *c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]);
            if name.eq_ignore_ascii_case("winws.exe") || name.eq_ignore_ascii_case("winws2.exe") {
                return Ok(Some(format!("{name}, PID {}", entry.th32ProcessID)));
            }
            found = Process32NextW(snapshot.as_raw_handle(), &mut entry);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(ERROR_NO_MORE_FILES as i32) {
            return Err(error);
        }
        Ok(None)
    }
}

pub struct WindowsRunner {
    pub package: Package,
    pub strategies: Vec<Strategy>,
    pub filter: GameFilterSettings,
    pub ipset: PathBuf,
    pub logfile: PathBuf,
    child: Option<Child>,
    job: Option<OwnedHandle>,
}
impl WindowsRunner {
    pub fn new(
        package: Package,
        strategies: Vec<Strategy>,
        filter: GameFilterSettings,
        ipset: PathBuf,
        logfile: PathBuf,
    ) -> Self {
        Self {
            package,
            strategies,
            filter,
            ipset,
            logfile,
            child: None,
            job: None,
        }
    }
    fn launch(&mut self, name: &str) -> io::Result<()> {
        let strategy = self
            .strategies
            .iter()
            .find(|s| s.name == name)
            .ok_or_else(|| io::Error::other("Не найдена стратегия."))?;
        let args = arguments(
            &String::from_utf8_lossy(&fs::read(&strategy.file)?),
            &self.package.root,
            &self.filter,
            Some(&self.ipset),
        )?;
        let directory = self.package.root.join("bin");
        // Closing our job terminates only processes launched by this instance.
        let job = unsafe {
            let raw = CreateJobObjectW(ptr::null(), ptr::null());
            if raw.is_null() {
                return Err(io::Error::last_os_error());
            }
            let handle = OwnedHandle::from_raw_handle(raw);
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                raw,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const c_void,
                mem::size_of_val(&limits) as u32,
            ) == 0
            {
                return Err(io::Error::last_os_error());
            }
            handle
        };
        let output = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.logfile)?;
        let mut child = Command::new(directory.join("winws.exe"))
            .args(args)
            .current_dir(directory)
            .creation_flags(CREATE_NO_WINDOW)
            .stdin(Stdio::null())
            .stderr(output.try_clone()?)
            .stdout(output)
            .spawn()?;
        if unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) } == 0 {
            let error = io::Error::last_os_error();
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        self.job = Some(job);
        self.child = Some(child);
        Ok(())
    }
}
impl Runner for WindowsRunner {
    fn is_running(&self) -> bool {
        self.child
            .as_ref()
            .is_some_and(|p| unsafe { WaitForSingleObject(p.as_raw_handle(), 0) } == WAIT_TIMEOUT)
    }
    fn stop(&mut self) -> Result<(), String> {
        self.job.take();
        if let Some(child) = self.child.as_mut() {
            if unsafe { WaitForSingleObject(child.as_raw_handle(), 5000) } == WAIT_TIMEOUT {
                return Err("Не удалось остановить текущий процесс.".into());
            }
            child.wait().map_err(|e| e.to_string())?;
        }
        self.child = None;
        Ok(())
    }
    fn start(&mut self, name: &str, cancel: &AtomicBool) -> Result<(), StartError> {
        self.stop().map_err(StartError::Conflict)?;
        for attempt in 0..=8 {
            if cancel.load(Ordering::Acquire) {
                return Err(StartError::Candidate("Отменено".into()));
            }
            let Some(conflict) = external().map_err(|e| StartError::Conflict(e.to_string()))?
            else {
                break;
            };
            if attempt == 8 {
                return Err(StartError::Conflict(format!(
                    "Уже запущен другой zapret ({conflict})."
                )));
            }
            thread::sleep(Duration::from_millis(250));
        }
        self.launch(name)
            .map_err(|e| StartError::Candidate(e.to_string()))?;
        for _ in 0..12 {
            if cancel.load(Ordering::Acquire) || !self.is_running() {
                self.stop().map_err(StartError::Conflict)?;
                return Err(StartError::Candidate("Запуск winws прерван.".into()));
            }
            thread::sleep(Duration::from_millis(100));
        }
        if !self.is_running() {
            self.stop().map_err(StartError::Conflict)?;
            return Err(StartError::Candidate("winws завершился.".into()));
        }
        Ok(())
    }
}
impl Drop for WindowsRunner {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

pub(super) fn network_available() -> bool {
    use windows_sys::Win32::NetworkManagement::{
        IpHelper::{FreeMibTable, GetIfTable2, IF_TYPE_SOFTWARE_LOOPBACK, IF_TYPE_TUNNEL},
        Ndis::IfOperStatusUp,
    };
    unsafe {
        let mut table = std::ptr::null_mut();
        if GetIfTable2(&mut table) != 0 || table.is_null() {
            return false;
        }
        let rows =
            std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        let available = rows.iter().any(|r| {
            r.OperStatus == IfOperStatusUp
                && !matches!(r.Type, IF_TYPE_SOFTWARE_LOOPBACK | IF_TYPE_TUNNEL)
        });
        FreeMibTable(table.cast());
        available
    }
}
