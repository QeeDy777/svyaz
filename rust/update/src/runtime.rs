use crate::{
    invalid,
    package::{LICENSES, RUNTIME_FILES},
    Version,
};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

pub(crate) fn ensure_plain(path: &Path, directory: bool) -> io::Result<()> {
    let m = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if m.file_attributes() & 0x400 != 0 {
            return Err(invalid("Недопустимая точка повторной обработки."));
        }
    }
    if m.file_type().is_symlink() || if directory { !m.is_dir() } else { !m.is_file() } {
        return Err(invalid("Недопустимый тип файла."));
    }
    Ok(())
}

pub fn identity(directory: &Path) -> io::Result<String> {
    ensure_plain(directory, true)?;
    let mut combined = Sha256::new();
    for name in RUNTIME_FILES {
        let path = directory.join(name);
        ensure_plain(&path, false)?;
        let mut hash = Sha256::new();
        io::copy(&mut File::open(path)?, &mut hash)?;
        combined.update(hash.finalize());
    }
    Ok(format!("{:x}", combined.finalize()))
}

pub fn install(source_exe: &Path, install_root: &Path) -> io::Result<PathBuf> {
    if source_exe.file_name().and_then(|x| x.to_str()) != Some(RUNTIME_FILES[0]) {
        return Err(invalid("Для установки нужен Связь.exe."));
    }
    let source = source_exe
        .parent()
        .ok_or_else(|| invalid("Нет каталога приложения."))?;
    let id = identity(source)?;
    let versions = install_root.join("Versions");
    fs::create_dir_all(&versions)?;
    ensure_plain(&versions, true)?;
    let destination = versions.join(&id);
    if !destination.try_exists()? {
        let tmp = tempfile::Builder::new()
            .prefix(".staging-")
            .tempdir_in(&versions)?;
        for name in RUNTIME_FILES.into_iter().chain(LICENSES) {
            let file = source.join(name);
            if LICENSES.contains(&name) && !file.try_exists()? {
                continue;
            }
            ensure_plain(&file, false)?;
            fs::copy(&file, tmp.path().join(name))?;
        }
        if identity(tmp.path())? != id {
            return Err(invalid("Файлы изменились при копировании."));
        }
        if let Err(e) = fs::rename(tmp.path(), &destination) {
            if !destination.try_exists()? {
                return Err(e);
            }
        }
    }
    if identity(&destination)? != id {
        return Err(invalid("Установленная версия повреждена."));
    }
    Ok(destination.join(RUNTIME_FILES[0]))
}

#[cfg(windows)]
pub fn file_version(path: &Path) -> io::Result<Version> {
    use std::{ffi::c_void, os::windows::ffi::OsStrExt, ptr};
    #[link(name = "version")]
    extern "system" {
        fn GetFileVersionInfoSizeW(filename: *const u16, handle: *mut u32) -> u32;
        fn GetFileVersionInfoW(
            filename: *const u16,
            handle: u32,
            len: u32,
            data: *mut c_void,
        ) -> i32;
        fn VerQueryValueW(
            block: *const c_void,
            sub: *const u16,
            buffer: *mut *mut c_void,
            len: *mut u32,
        ) -> i32;
    }
    let mut name: Vec<u16> = path.as_os_str().encode_wide().collect();
    if name.contains(&0) {
        return Err(invalid("Недопустимое имя файла."));
    }
    name.push(0);
    let mut handle = 0;
    // The version API fills our aligned allocation and returns a pointer into it.
    unsafe {
        let n = GetFileVersionInfoSizeW(name.as_ptr(), &mut handle);
        if n == 0 {
            return Err(io::Error::last_os_error());
        }
        if n > 16 * 1024 * 1024 {
            return Err(invalid("Слишком большой ресурс версии."));
        }
        let mut data = vec![0u32; (n as usize).div_ceil(4)];
        if GetFileVersionInfoW(name.as_ptr(), 0, n, data.as_mut_ptr().cast()) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut ptr: *mut c_void = ptr::null_mut();
        let mut length = 0;
        if VerQueryValueW(
            data.as_ptr().cast(),
            [92u16, 0].as_ptr(),
            &mut ptr,
            &mut length,
        ) == 0
            || length < 52
            || ptr.is_null()
        {
            return Err(invalid("Отсутствует версия EXE."));
        }
        let p = ptr.cast::<u32>();
        if p.read_unaligned() != 0xfeef04bd {
            return Err(invalid("Повреждён ресурс версии."));
        }
        let ms = p.add(2).read_unaligned();
        let ls = p.add(3).read_unaligned();
        Ok(Version(ms >> 16, ms & 65535, ls >> 16))
    }
}

#[cfg(not(windows))]
pub fn file_version(_: &Path) -> io::Result<Version> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "Для проверки версии EXE требуется Windows.",
    ))
}
