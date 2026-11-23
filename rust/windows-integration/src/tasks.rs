use super::shell::{executable_under_versions, Apartment};
use std::{
    io,
    os::windows::io::{FromRawHandle, OwnedHandle},
    path::Path,
};
use windows::{
    core::{Interface, BSTR, PWSTR},
    Win32::{
        Foundation::{LocalFree, HANDLE, HLOCAL},
        Security::{
            Authorization::ConvertSidToStringSidW, GetTokenInformation, TokenUser, TOKEN_QUERY,
            TOKEN_USER,
        },
        System::{
            Com::{CoCreateInstance, CLSCTX_INPROC_SERVER},
            TaskScheduler::*,
            Threading::{GetCurrentProcess, OpenProcessToken},
            Variant::VARIANT,
        },
    },
};

fn sid() -> io::Result<String> {
    unsafe {
        let mut handle = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle)?;
        let _handle = OwnedHandle::from_raw_handle(handle.0);
        let mut len = 0;
        let _ = GetTokenInformation(handle, TokenUser, None, 0, &mut len);
        if len == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buffer = vec![0usize; (len as usize).div_ceil(size_of::<usize>())];
        GetTokenInformation(
            handle,
            TokenUser,
            Some(buffer.as_mut_ptr().cast()),
            len,
            &mut len,
        )?;
        let user = &*buffer.as_ptr().cast::<TOKEN_USER>();
        let mut ptr = PWSTR::null();
        ConvertSidToStringSidW(user.User.Sid, &mut ptr)?;
        let value = ptr.to_string();
        let _ = LocalFree(Some(HLOCAL(ptr.0.cast())));
        value.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }
}
fn folder() -> io::Result<(Apartment, ITaskFolder)> {
    let apartment = Apartment::enter()?;
    unsafe {
        let scheduler: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
        let empty = VARIANT::default();
        scheduler.Connect(&empty, &empty, &empty, &empty)?;
        Ok((apartment, scheduler.GetFolder(&BSTR::from("\\"))?))
    }
}
fn user_task(folder: &ITaskFolder, name: &BSTR) -> io::Result<Option<IRegisteredTask>> {
    match unsafe { folder.GetTask(name) } {
        Ok(t) => Ok(Some(t)),
        Err(e) if e.code().0 as u32 == 0x80070002 => Ok(None),
        Err(e) => Err(e.into()),
    }
}
pub fn autostart_enabled() -> io::Result<bool> {
    let (_apt, root) = folder()?;
    Ok(user_task(&root, &BSTR::from(format!("DiscordRoute-{}", sid()?)))?.is_some())
}
fn own_action(def: &ITaskDefinition, root: &Path) -> io::Result<Option<IExecAction>> {
    unsafe {
        let actions = def.Actions()?;
        let mut count = 0;
        actions.Count(&mut count)?;
        if count != 1 {
            return Ok(None);
        }
        let action = actions.get_Item(1)?;
        let Ok(exec) = action.cast::<IExecAction>() else {
            return Ok(None);
        };
        let mut path = BSTR::default();
        exec.Path(&mut path)?;
        Ok(executable_under_versions(root, Path::new(&path.to_string())).then_some(exec))
    }
}
pub fn set_autostart(enabled: bool, install_root: &Path, source_exe: &Path) -> io::Result<()> {
    let (_apartment, root) = folder()?;
    let sid = sid()?;
    let name = BSTR::from(format!("DiscordRoute-{sid}"));
    unsafe {
        if let Some(t) = user_task(&root, &name)? {
            if own_action(&t.Definition()?, install_root)?.is_none() {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "Имя задачи уже используется другой программой.",
                ));
            }
            if !enabled {
                root.DeleteTask(&name, 0)?;
                return Ok(());
            }
        } else if !enabled {
            return Ok(());
        }
        let exe = svyaz_update::install(source_exe, install_root)?;
        let exe = exe
            .to_str()
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "Недопустимый путь автозапуска.")
            })?
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;");
        let xml = format!(
            r#"<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task"><RegistrationInfo><Description>DiscordRoute: автоматический подбор zapret после входа в Windows.</Description></RegistrationInfo><Triggers><LogonTrigger><Enabled>true</Enabled><UserId>{sid}</UserId><Delay>PT15S</Delay></LogonTrigger></Triggers><Principals><Principal id="User"><UserId>{sid}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals><Settings><MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy><DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries><StopIfGoingOnBatteries>false</StopIfGoingOnBatteries><StartWhenAvailable>true</StartWhenAvailable><ExecutionTimeLimit>PT0S</ExecutionTimeLimit><Enabled>true</Enabled></Settings><Actions Context="User"><Exec><Command>{exe}</Command><Arguments>--startup</Arguments></Exec></Actions></Task>"#
        );
        let empty = VARIANT::default();
        root.RegisterTask(
            &name,
            &BSTR::from(xml),
            TASK_CREATE_OR_UPDATE.0,
            &VARIANT::from(sid.as_str()),
            &empty,
            TASK_LOGON_INTERACTIVE_TOKEN,
            &empty,
        )?;
        Ok(())
    }
}
pub(crate) fn visit_tasks(root: &Path, destination: Option<&Path>) -> io::Result<()> {
    let (_apartment, folder) = folder()?;
    unsafe {
        let tasks = folder.GetTasks(TASK_ENUM_HIDDEN.0)?;
        let empty = VARIANT::default();
        // Deleting from the end preserves the collection indices yet to visit.
        for index in (1..=tasks.Count()?).rev() {
            let task = tasks.get_Item(&VARIANT::from(index))?;
            let name = task.Name()?;
            if !name.to_string().starts_with("DiscordRoute-") {
                continue;
            }
            let def = task.Definition()?;
            let Some(action) = own_action(&def, root)? else {
                continue;
            };
            if let Some(path) = destination {
                action.SetPath(&BSTR::from(path.to_str().ok_or_else(|| {
                    io::Error::new(io::ErrorKind::InvalidData, "Недопустимый путь приложения.")
                })?))?;
                let principal = def.Principal()?;
                let mut id = BSTR::default();
                let mut logon = TASK_LOGON_TYPE::default();
                principal.UserId(&mut id)?;
                principal.LogonType(&mut logon)?;
                folder.RegisterTaskDefinition(
                    &name,
                    &def,
                    TASK_UPDATE.0,
                    &VARIANT::from(id),
                    &empty,
                    logon,
                    &empty,
                )?;
            } else {
                folder.DeleteTask(&name, 0)?;
            }
        }
    }
    Ok(())
}
