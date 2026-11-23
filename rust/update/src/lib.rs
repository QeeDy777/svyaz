//! Update archives and state compatible with the installed Windows launcher.
mod http;
pub use http::fetch_github;
mod package;
mod runtime;
pub use package::{Release, LATEST_URL, RUNTIME_FILES};
pub use runtime::{file_version, identity, install};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    str::FromStr,
};
use tempfile::NamedTempFile;

fn invalid(s: impl fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, s.to_string())
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);
impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}
impl FromStr for Version {
    type Err = io::Error;
    fn from_str(s: &str) -> io::Result<Self> {
        let mut parts = s.split('.');
        let parse = |v: Option<&str>| {
            v.filter(|x| {
                !x.is_empty()
                    && x.bytes().all(|c| c.is_ascii_digit())
                    && (*x == "0" || !x.starts_with('0'))
            })
            .and_then(|x| x.parse::<i32>().ok())
            .map(|v| v as u32)
            .ok_or_else(|| invalid("Неверная версия «Связи»."))
        };
        let version = Self(
            parse(parts.next())?,
            parse(parts.next())?,
            parse(parts.next())?,
        );
        if s.len() > 32 || parts.next().is_some() {
            return Err(invalid("Неверная версия «Связи»."));
        }
        Ok(version)
    }
}
mod version_json {
    use super::*;
    pub fn serialize<S: serde::Serializer>(v: &Version, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(v)
    }
    pub fn deserialize<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Version, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct InstalledApp {
    #[serde(with = "version_json")]
    pub version: Version,
    pub id: String,
}

fn valid_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

#[derive(Clone, Copy)]
pub enum State {
    Pending,
    Current,
    Rejected,
    Rollback,
}
impl State {
    fn file(self) -> &'static str {
        match self {
            Self::Pending => "pending.json",
            Self::Current => "current.json",
            Self::Rejected => "rejected.json",
            Self::Rollback => "rollback.json",
        }
    }
}

pub struct Updater {
    install_root: PathBuf,
    state_dir: PathBuf,
    pub running: Version,
}
impl Updater {
    pub fn new(
        install_root: impl Into<PathBuf>,
        data_root: impl AsRef<Path>,
        running_version: Version,
    ) -> Self {
        Self {
            install_root: install_root.into(),
            state_dir: data_root.as_ref().join("AppUpdate"),
            running: running_version,
        }
    }
    pub fn read(&self, state: State) -> io::Result<Option<InstalledApp>> {
        let path = self.state_dir.join(state.file());
        match fs::symlink_metadata(&path) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
            other => {
                other?;
            }
        }
        runtime::ensure_plain(&self.state_dir, true)?;
        runtime::ensure_plain(&path, false)?;
        let mut v = Vec::new();
        File::open(path)?.take(4097).read_to_end(&mut v)?;
        if v.len() > 4096 {
            return Err(invalid("Данные обновления слишком велики."));
        }
        let value: InstalledApp =
            serde_json::from_slice(v.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&v))?;
        if !valid_id(&value.id) {
            return Err(invalid("Неверный каталог обновления."));
        }
        Ok(Some(value))
    }
    fn lock(&self) -> io::Result<File> {
        fs::create_dir_all(&self.state_dir)?;
        runtime::ensure_plain(&self.state_dir, true)?;
        let file = File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.state_dir.join("update.lock"))?;
        file.lock()?;
        Ok(file)
    }
    fn write(&self, state: State, value: &InstalledApp) -> io::Result<()> {
        if !valid_id(&value.id) {
            return Err(invalid("Неверный каталог обновления."));
        }
        let mut temp = NamedTempFile::new_in(&self.state_dir)?;
        temp.write_all(&serde_json::to_vec(value)?)?;
        temp.as_file().sync_all()?;
        temp.persist(self.state_dir.join(state.file()))
            .map_err(|e| e.error)?;
        Ok(())
    }
    fn remove(&self, state: State) -> io::Result<()> {
        match fs::remove_file(self.state_dir.join(state.file())) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }
    pub fn executable(&self, app: &InstalledApp) -> io::Result<PathBuf> {
        if !valid_id(&app.id) {
            return Err(invalid("Неверный каталог обновления."));
        }
        let versions = self.install_root.join("Versions");
        runtime::ensure_plain(&versions, true)?;
        let root = versions.join(&app.id);
        if identity(&root)? != app.id {
            return Err(invalid("Установленная версия повреждена."));
        }
        let exe = root.join(RUNTIME_FILES[0]);
        if file_version(&exe)? != app.version {
            return Err(invalid("Версия EXE не совпадает с релизом."));
        }
        Ok(exe)
    }
    pub fn prepare(
        &self,
        force: bool,
        cancel: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<Option<InstalledApp>> {
        let pending = self.read(State::Pending)?;
        let mut minimum = self.running;
        if !force {
            for item in [pending.as_ref(), self.read(State::Rejected)?.as_ref()]
                .into_iter()
                .flatten()
            {
                minimum = minimum.max(item.version);
            }
        }
        match http::download(minimum, cancel)? {
            Some((release, bytes)) => self.stage(&release, &bytes, cancel).map(Some),
            None => Ok(pending.filter(|x| x.version > self.running)),
        }
    }
    pub fn stage(
        &self,
        release: &Release,
        bytes: &[u8],
        cancel: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<InstalledApp> {
        let _lock = self.lock()?;
        let stage = tempfile::Builder::new()
            .prefix("stage-")
            .tempdir_in(&self.state_dir)?;
        package::extract(release, bytes, stage.path(), cancel)?;
        let source = stage.path().join(RUNTIME_FILES[0]);
        if file_version(&source)? != release.version {
            return Err(invalid("Версия EXE не совпадает с релизом."));
        }
        cancel()?;
        let exe = install(&source, &self.install_root)?;
        let item = InstalledApp {
            version: release.version,
            id: exe
                .parent()
                .and_then(Path::file_name)
                .and_then(|s| s.to_str())
                .ok_or_else(|| invalid("Неверный путь установленной версии."))?
                .into(),
        };
        self.executable(&item)?;
        cancel()?;
        self.write(State::Pending, &item)?;
        Ok(item)
    }
    pub fn select_for_launch(&self, apply_pending: bool) -> io::Result<Option<InstalledApp>> {
        let _lock = self.lock()?;
        let rollback = self.read(State::Rollback)?;
        if apply_pending && rollback.is_some() {
            self.remove(State::Rollback)?;
        }
        let current = self.read(State::Current)?;
        if !apply_pending
            && rollback == current
            && rollback.as_ref().is_some_and(|x| x.version < self.running)
        {
            return Ok(rollback);
        }
        let pending = if apply_pending {
            self.read(State::Pending)?
        } else {
            None
        };
        Ok([current, pending]
            .into_iter()
            .flatten()
            .filter(|x| x.version > self.running)
            .max_by_key(|x| x.version))
    }
    pub fn installed_versions(&self) -> io::Result<Vec<InstalledApp>> {
        let path = self.install_root.join("Versions");
        if !path.try_exists()? {
            return Ok(Vec::new());
        }
        runtime::ensure_plain(&path, true)?;
        let mut items = Vec::new();
        for e in fs::read_dir(path)? {
            let e = e?;
            let id = e.file_name().to_string_lossy().into_owned();
            if !valid_id(&id) || !e.file_type()?.is_dir() {
                continue;
            }
            if let Ok(version) = file_version(&e.path().join(RUNTIME_FILES[0])) {
                let item = InstalledApp { version, id };
                if version >= Version(0, 5, 0) && self.executable(&item).is_ok() {
                    items.push(item);
                }
            }
        }
        items.sort_by_key(|x| std::cmp::Reverse(x.version));
        Ok(items)
    }
    pub fn previous_version(&self) -> io::Result<Option<InstalledApp>> {
        Ok(self
            .installed_versions()?
            .into_iter()
            .find(|x| x.version < self.running))
    }
    pub fn promote(&self, app: &InstalledApp, rollback: bool) -> io::Result<()> {
        let _lock = self.lock()?;
        self.executable(app)?;
        if rollback && app.version >= self.running {
            return Err(invalid("Откат доступен только к предыдущей версии."));
        }
        let pending = self.read(State::Pending)?;
        if rollback {
            self.write(State::Rollback, app)?;
        }
        self.write(State::Current, app)?;
        // current.json is the commit point; cleanup is best effort after it.
        if !rollback {
            let _ = self.remove(State::Rollback);
        }
        if rollback || pending.as_ref() == Some(app) {
            let _ = self.remove(State::Pending);
        }
        Ok(())
    }
    pub fn reject(&self, app: &InstalledApp) -> io::Result<()> {
        let _lock = self.lock()?;
        self.write(State::Rejected, app)?;
        for state in [State::Pending, State::Current] {
            if self.read(state)?.as_ref() == Some(app) {
                self.remove(state)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use std::io::Cursor;
    use zip::{write::SimpleFileOptions, ZipWriter};
    fn pack(names: &[&str]) -> Vec<u8> {
        let mut z = ZipWriter::new(Cursor::new(Vec::new()));
        for name in names {
            z.start_file(*name, SimpleFileOptions::default()).unwrap();
            z.write_all(name.as_bytes()).unwrap();
        }
        z.finish().unwrap().into_inner()
    }
    fn metadata(bytes: &[u8]) -> serde_json::Value {
        let name = "Svyaz-0.13.3-win-x64-unsigned.zip";
        json!({"draft":false,"prerelease":false,"tag_name":"v0.13.3","assets":[{"name":name,"size":bytes.len(),"digest":format!("sha256:{:x}",Sha256::digest(bytes)),"browser_download_url":format!("https://github.com/QeeDy777/svyaz/releases/download/v0.13.3/{name}")}]})
    }
    #[test]
    fn validates_release_and_sha() {
        let bytes = pack(&RUNTIME_FILES);
        let release = |j: serde_json::Value| Release::parse(&serde_json::to_vec(&j).unwrap());
        let j = metadata(&bytes);
        for tag in [
            "v00.13.3",
            "0.13.3",
            "v2147483648.1.1",
            "v0.13.3-beta",
            "v1.2",
        ] {
            let mut bad = j.clone();
            bad["tag_name"] = tag.into();
            assert!(release(bad).is_err());
        }
        let mut duplicate = j.clone();
        duplicate["assets"]
            .as_array_mut()
            .unwrap()
            .push(j["assets"][0].clone());
        assert!(release(duplicate).is_err());
        let mut bad = j.clone();
        bad["assets"][0]["browser_download_url"] = "https://example.org/file.zip".into();
        assert!(release(bad).is_err());
        let r = release(j).unwrap();
        let dir = tempfile::tempdir().unwrap();
        assert!(
            package::extract(&r, &bytes, dir.path(), &|| Err(io::Error::from(
                io::ErrorKind::Interrupted
            )))
            .is_err()
        );
        let mut altered = bytes.clone();
        altered[0] ^= 1;
        assert!(package::extract(&r, &altered, dir.path(), &|| Ok(())).is_err());
        package::extract(&r, &bytes, dir.path(), &|| Ok(())).unwrap();
        assert!(package::extract(&r, &bytes, dir.path(), &|| Ok(())).is_err());
        let install_root = tempfile::tempdir().unwrap();
        let a = install(&dir.path().join(RUNTIME_FILES[0]), install_root.path()).unwrap();
        assert_eq!(
            a,
            install(&dir.path().join(RUNTIME_FILES[0]), install_root.path()).unwrap()
        );
        let expected = format!(
            "{:x}",
            Sha256::digest(
                RUNTIME_FILES
                    .iter()
                    .flat_map(|f| Sha256::digest(f.as_bytes()))
                    .collect::<Vec<_>>()
            )
        );
        assert_eq!(
            a.parent().unwrap().file_name().unwrap().to_str().unwrap(),
            expected
        );
        fs::write(a, b"changed").unwrap();
        assert!(install(&dir.path().join(RUNTIME_FILES[0]), install_root.path()).is_err());
    }
    #[test]
    fn rejects_unknown_and_missing_entries() {
        for names in [
            RUNTIME_FILES[..5].to_vec(),
            RUNTIME_FILES.into_iter().chain(["../file"]).collect(),
        ] {
            let bytes = pack(&names);
            let r = Release::parse(&serde_json::to_vec(&metadata(&bytes)).unwrap()).unwrap();
            let dst = tempfile::tempdir().unwrap();
            assert!(package::extract(&r, &bytes, dst.path(), &|| Ok(())).is_err());
            assert_eq!(fs::read_dir(dst.path()).unwrap().count(), 0);
        }
    }
    #[test]
    fn preserves_old_pointer_format_and_selection() {
        let root = tempfile::tempdir().unwrap();
        let updater = Updater::new(root.path(), root.path().join("Data"), Version(0, 13, 3));
        let old = InstalledApp {
            version: Version(0, 6, 1),
            id: "a".repeat(64),
        };
        let new = InstalledApp {
            version: Version(1, 0, 0),
            id: "b".repeat(64),
        };
        fs::create_dir_all(&updater.state_dir).unwrap();
        let legacy = format!(r#"{{"Version":"0.6.1","Id":"{}"}}"#, old.id);
        fs::write(updater.state_dir.join("current.json"), legacy).unwrap();
        assert_eq!(updater.read(State::Current).unwrap(), Some(old.clone()));
        updater.write(State::Rollback, &old).unwrap();
        updater.write(State::Pending, &new).unwrap();
        assert_eq!(updater.select_for_launch(false).unwrap(), Some(old));
        assert_eq!(updater.select_for_launch(true).unwrap(), Some(new.clone()));
        assert!(updater.read(State::Rollback).unwrap().is_none());
        updater.reject(&new).unwrap();
        assert!(updater.read(State::Pending).unwrap().is_none());
        assert_eq!(updater.read(State::Rejected).unwrap(), Some(new));
        fs::write(
            updater.state_dir.join("current.json"),
            r#"{"Version":"0.6.1","Id":"../other"}"#,
        )
        .unwrap();
        assert!(updater.read(State::Current).is_err());
    }
}

#[cfg(all(test, windows))]
mod windows_tests {
    use super::*;
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemDirectoryW(buffer: *mut u16, size: u32) -> u32;
    }
    #[test]
    fn confirms_installed_pe_version_and_rollback() {
        let mut buffer = vec![0u16; 32768];
        let n = unsafe { GetSystemDirectoryW(buffer.as_mut_ptr(), buffer.len() as u32) } as usize;
        assert!(n > 0 && n < buffer.len());
        let source = tempfile::tempdir().unwrap();
        let exe = source.path().join(RUNTIME_FILES[0]);
        fs::copy(
            PathBuf::from(OsString::from_wide(&buffer[..n])).join("cmd.exe"),
            &exe,
        )
        .unwrap();
        for name in RUNTIME_FILES.iter().skip(1) {
            fs::write(source.path().join(name), name.as_bytes()).unwrap();
        }
        let version = file_version(&exe).unwrap();
        let root = tempfile::tempdir().unwrap();
        let data = root.path().join("Data");
        let installed = install(&exe, root.path()).unwrap();
        let id = identity(installed.parent().unwrap()).unwrap();
        let item = InstalledApp { version, id };
        let updater = Updater::new(root.path(), &data, Version(version.0 + 1, 0, 0));
        let different = InstalledApp {
            version: Version(0, 0, 0),
            id: item.id.clone(),
        };
        assert!(updater.executable(&different).is_err());
        assert_eq!(updater.executable(&item).unwrap(), installed);
        assert_eq!(updater.previous_version().unwrap(), Some(item.clone()));
        updater.promote(&item, true).unwrap();
        assert_eq!(
            updater.select_for_launch(false).unwrap(),
            Some(item.clone())
        );
        assert_eq!(updater.read(State::Current).unwrap(), Some(item.clone()));
        updater.select_for_launch(true).unwrap();
        assert_eq!(updater.read(State::Rollback).unwrap(), None);
        fs::write(
            installed.parent().unwrap().join(RUNTIME_FILES[1]),
            b"corrupt",
        )
        .unwrap();
        assert!(updater.promote(&item, false).is_err());
        assert_eq!(updater.read(State::Current).unwrap(), Some(item));
    }
}
