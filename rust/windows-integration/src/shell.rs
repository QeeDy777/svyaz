use std::{
    ffi::OsString,
    fs, io,
    os::windows::{ffi::OsStringExt, fs::MetadataExt},
    path::{Component, Path, PathBuf, Prefix},
};
use svyaz_update::RUNTIME_FILES;
use windows::{
    core::{Interface, GUID, HSTRING, PCWSTR},
    Win32::{
        Storage::FileSystem::{ReplaceFileW, REPLACE_FILE_FLAGS},
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, IPersistFile,
            CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, STGM_READ,
        },
        UI::Shell::{
            FOLDERID_CommonPrograms, FOLDERID_ProgramFiles, FOLDERID_PublicDesktop, IShellLinkW,
            SHChangeNotify, SHGetKnownFolderPath, SHGetPathFromIDListEx, ShellLink, GPFIDL_DEFAULT,
            KF_FLAG_DONT_VERIFY, SHCNE_UPDATEITEM, SHCNF_PATHW,
        },
    },
};

pub(crate) struct Apartment(bool);
impl Apartment {
    pub(crate) fn enter() -> io::Result<Self> {
        let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        // An already initialized MTA may use task scheduler and shell COM interfaces.
        if hr.0 as u32 == 0x80010106 {
            return Ok(Self(false));
        }
        hr.ok()?;
        Ok(Self(true))
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() }
        }
    }
}

fn folder(guid: &GUID) -> io::Result<PathBuf> {
    unsafe {
        let ptr = SHGetKnownFolderPath(guid, KF_FLAG_DONT_VERIFY, None)?;
        let value = PathBuf::from(OsString::from_wide(ptr.as_wide()));
        CoTaskMemFree(Some(ptr.0.cast()));
        Ok(value)
    }
}
pub fn install_root() -> io::Result<PathBuf> {
    Ok(folder(&FOLDERID_ProgramFiles)?.join("DiscordRoute"))
}
fn same(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}
fn local(p: &Path) -> bool {
    matches!(p.components().next(),Some(Component::Prefix(prefix)) if matches!(prefix.kind(),Prefix::Disk(_)))
        && p.has_root()
        && !p
            .to_string_lossy()
            .split(['\\', '/'])
            .any(|part| part == "." || part == "..")
}
fn plain_chain(p: &Path) -> bool {
    p.ancestors()
        .all(|p| fs::symlink_metadata(p).is_ok_and(|a| a.file_attributes() & 0x400 == 0))
}
pub(crate) fn executable_under_versions(root: &Path, exe: &Path) -> bool {
    let Some(dir) = exe.parent() else {
        return false;
    };
    let Some(id) = dir.file_name() else {
        return false;
    };
    local(exe) && same(exe, &root.join("Versions").join(id).join(RUNTIME_FILES[0]))
}
fn ours(root: &Path, p: &Path) -> bool {
    same(p, &root.join("Application").join(RUNTIME_FILES[0]))
        || executable_under_versions(root, p)
            && p.parent()
                .and_then(Path::file_name)
                .and_then(|s| s.to_str())
                .is_some_and(|id| id.len() == 64 && id.bytes().all(|c| c.is_ascii_hexdigit()))
}
fn from_buf(buf: &[u16]) -> PathBuf {
    PathBuf::from(OsString::from_wide(
        &buf[..buf.iter().position(|&x| x == 0).unwrap_or(buf.len())],
    ))
}

pub fn refresh_shortcuts(root: &Path, exe: &Path) {
    let Ok(_apartment) = Apartment::enter() else {
        return;
    };
    for guid in [FOLDERID_CommonPrograms, FOLDERID_PublicDesktop] {
        if let Ok(folder) = folder(&guid) {
            let _ = refresh_link(root, exe, &folder.join("Связь.lnk"));
        }
    }
}
fn refresh_link(root: &Path, exe: &Path, shortcut: &Path) -> io::Result<()> {
    if !local(root)
        || !local(shortcut)
        || !local(exe)
        || !ours(root, exe)
        || !exe.is_file()
        || !shortcut.is_file()
        || !plain_chain(exe)
        || !plain_chain(shortcut)
        || root.components().next() != shortcut.components().next()
    {
        return Ok(());
    }
    let lnk = HSTRING::from(shortcut.as_os_str());
    let old_launcher = root.join("Application").join(RUNTIME_FILES[0]);
    let mut buf = vec![0u16; 32768];
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let file: IPersistFile = link.cast()?;
        file.Load(&lnk, STGM_READ)?;
        link.GetPath(&mut buf, std::ptr::null_mut(), 4)?;
        if buf[0] == 0 {
            let id = link.GetIDList()?;
            if id.is_null() {
                return Ok(());
            }
            let ok = SHGetPathFromIDListEx(id, &mut buf, GPFIDL_DEFAULT);
            CoTaskMemFree(Some(id.cast()));
            if !ok.as_bool() {
                return Ok(());
            }
        }
        if !same(&from_buf(&buf), &old_launcher) {
            return Ok(());
        }
        buf.fill(0);
        let mut index = 0;
        link.GetIconLocation(&mut buf, &mut index)?;
        let old = from_buf(&buf);
        if index != 0 || same(&old, exe) || !old.as_os_str().is_empty() && !ours(root, &old) {
            return Ok(());
        }
        let temp_dir = tempfile::Builder::new()
            .prefix(".shortcut-")
            .tempdir_in(root)?;
        let temp = HSTRING::from(temp_dir.path().join("Связь.lnk").as_os_str());
        link.SetIconLocation(&HSTRING::from(exe.as_os_str()), 0)?;
        file.Save(&temp, false)?;
        drop(file);
        drop(link);
        ReplaceFileW(
            &lnk,
            &temp,
            PCWSTR::null(),
            REPLACE_FILE_FLAGS(0),
            None,
            None,
        )?;
        SHChangeNotify(
            SHCNE_UPDATEITEM,
            SHCNF_PATHW,
            Some(lnk.as_ptr().cast()),
            None,
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::Com::CLSCTX_INPROC_SERVER;

    fn fixture(path: &Path, target: &Path, icon: Option<&Path>) -> io::Result<()> {
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(&HSTRING::from(target.as_os_str()))?;
            link.SetArguments(windows::core::w!("--startup"))?;
            link.SetDescription(windows::core::w!("Моя связь"))?;
            if let Some(icon) = icon {
                link.SetIconLocation(&HSTRING::from(icon.as_os_str()), 0)?;
            }
            link.cast::<IPersistFile>()?
                .Save(&HSTRING::from(path.as_os_str()), true)?;
            Ok(())
        }
    }
    #[test]
    fn refreshes_existing_unicode_links_but_keeps_custom_icons() -> io::Result<()> {
        let _apartment = Apartment::enter()?;
        let root = tempfile::tempdir()?;
        let launcher = root.path().join("Application/Связь.exe");
        fs::create_dir(launcher.parent().unwrap())?;
        fs::copy(std::env::current_exe()?, &launcher)?;
        let exe = root
            .path()
            .join("Versions")
            .join("b".repeat(64))
            .join("Связь.exe");
        fs::create_dir_all(exe.parent().unwrap())?;
        fs::copy(&launcher, &exe)?;
        let elsewhere = root.path().join("рабочий стол");
        fs::create_dir(&elsewhere)?;
        let lnk = elsewhere.join("Связь.lnk");
        fixture(&lnk, &launcher, None)?;
        refresh_link(root.path(), &exe, &lnk)?;
        unsafe {
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.cast::<IPersistFile>()?
                .Load(&HSTRING::from(lnk.as_os_str()), STGM_READ)?;
            let mut buf = vec![0u16; 32768];
            let mut index = 1;
            link.GetIconLocation(&mut buf, &mut index)?;
            assert_eq!(index, 0);
            assert!(same(&from_buf(&buf), &exe));
            link.GetPath(&mut buf, std::ptr::null_mut(), 0)?;
            assert!(same(&from_buf(&buf), &launcher));
            link.GetArguments(&mut buf)?;
            assert_eq!(from_buf(&buf), Path::new("--startup"));
        }
        let custom = root.path().join("выбранный.ico");
        fs::write(&custom, b"icon")?;
        fixture(&lnk, &launcher, Some(&custom))?;
        let before = fs::read(&lnk)?;
        refresh_link(root.path(), &exe, &lnk)?;
        assert_eq!(before, fs::read(&lnk)?);
        fs::remove_file(&lnk)?;
        refresh_link(root.path(), &exe, &lnk)?;
        assert!(!lnk.exists());
        Ok(())
    }
}
