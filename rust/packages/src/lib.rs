use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
};
pub mod game;
pub mod strategy;
fn invalid(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}
#[derive(Clone)]
pub struct Package {
    pub id: String,
    pub version: String,
    pub root: PathBuf,
}
const REPO: &str = "https://github.com/Flowseal/zapret-discord-youtube/releases/download/";
const API: &str = "https://api.github.com/repos/Flowseal/zapret-discord-youtube/releases/latest";
const MAX: usize = 64 * 1024 * 1024;

pub fn open(store: &Path, id: &str) -> io::Result<Package> {
    if id.len() != 32
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
    {
        return Err(invalid("Неверный идентификатор пакета."));
    }
    let dir = store.join(id);
    let meta: Value = serde_json::from_slice(&fs::read(dir.join("release.json"))?)?;
    let version = meta["version"]
        .as_str()
        .ok_or_else(|| invalid("Нет версии пакета."))?
        .to_owned();
    Ok(Package {
        id: id.to_owned(),
        version,
        root: find_root(&dir)?,
    })
}
fn find_root(dir: &Path) -> io::Result<PathBuf> {
    let mut all = vec![dir.to_owned()];
    for entry in fs::read_dir(dir)? {
        let e = entry?;
        if e.file_type()?.is_dir() {
            all.push(e.path());
        }
    }
    let mut candidates = all
        .into_iter()
        .filter(|p| p.join("bin/winws.exe").is_file());
    let root = candidates
        .next()
        .ok_or_else(|| invalid("Не найден winws.exe."))?;
    if candidates.next().is_some()
        || !root.join("lists").is_dir()
        || ["WinDivert.dll", "WinDivert64.sys", "cygwin1.dll"]
            .iter()
            .any(|n| !root.join("bin").join(n).is_file())
    {
        return Err(invalid("Неверная структура пакета zapret."));
    }
    Ok(root)
}
fn clean_component(s: &str) -> bool {
    let name = s.split('.').next().unwrap_or("").to_ascii_uppercase();
    !s.is_empty()
        && !s.ends_with(['.', ' '])
        && !s.chars().any(|c| c < ' ' || ":*?\"<>|".contains(c))
        && !matches!(
            name.as_str(),
            "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$"
        )
        && !((name.starts_with("COM") || name.starts_with("LPT"))
            && ["1", "2", "3", "4", "5", "6", "7", "8", "9", "¹", "²", "³"].contains(&&name[3..]))
}
fn extract(bytes: &[u8], stage: &Path, cancel: &dyn Fn() -> io::Result<()>) -> io::Result<()> {
    let mut archive = zip::ZipArchive::new(io::Cursor::new(bytes))?;
    if archive.len() > 4096 {
        return Err(invalid("Слишком много файлов в архиве."));
    }
    let mut seen = HashSet::new();
    let mut total = 0u64;
    for i in 0..archive.len() {
        cancel()?;
        let mut entry = archive.by_index(i)?;
        let name = entry.name().replace('\\', "/");
        let path = name.strip_suffix('/').unwrap_or(&name);
        if path.len() > 240
            || !path.split('/').all(clean_component)
            || !seen.insert(path.to_lowercase())
            || entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000)
        {
            return Err(invalid("Небезопасный путь в ZIP."));
        }
        let size = entry.size();
        total = total
            .checked_add(size)
            .ok_or_else(|| invalid("ZIP слишком велик."))?;
        if size > MAX as u64 || total > 256 * 1024 * 1024 {
            return Err(invalid("ZIP слишком велик."));
        }
        let dest = stage.join(path);
        if name.ends_with('/') {
            fs::create_dir_all(dest)?;
        } else {
            fs::create_dir_all(dest.parent().ok_or_else(|| invalid("Нет каталога."))?)?;
            let mut output = fs::File::create_new(dest)?;
            if io::copy(&mut (&mut entry).take(size + 1), &mut output)? != size {
                return Err(invalid("Неверный размер файла."));
            }
        }
    }
    Ok(())
}
pub fn prepare_lists(root: &Path, previous: Option<&Path>) -> io::Result<()> {
    let dest = root.join("lists");
    if let Some(prev) = previous {
        for ent in fs::read_dir(prev.join("lists"))? {
            let ent = ent?;
            if ent.file_type()?.is_file()
                && ent
                    .file_name()
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .ends_with("-user.txt")
            {
                fs::copy(ent.path(), dest.join(ent.file_name()))?;
            }
        }
    }
    for name in [
        "list-general-user.txt",
        "list-exclude-user.txt",
        "ipset-exclude-user.txt",
    ] {
        let path = dest.join(name);
        if !path.try_exists()? {
            fs::write(
                path,
                if name.starts_with("ipset") {
                    "203.0.113.113/32\n"
                } else {
                    "domain.example.abc\n"
                },
            )?;
        }
    }
    Ok(())
}
pub fn stage_latest(
    store: &Path,
    previous: Option<&Package>,
    current_version: Option<&str>,
    cancel: &dyn Fn() -> io::Result<()>,
) -> io::Result<Option<Package>> {
    use svyaz_update::fetch_github;
    let doc: Value = serde_json::from_slice(&fetch_github(API, 2 * 1024 * 1024, cancel)?)?;
    if doc["draft"] != false || doc["prerelease"] != false {
        return Err(invalid("Ожидался стабильный релиз."));
    }
    let tag = doc["tag_name"]
        .as_str()
        .ok_or_else(|| invalid("Нет версии релиза."))?;
    if Some(tag) == current_version {
        return Ok(None);
    }
    let assets = doc["assets"]
        .as_array()
        .ok_or_else(|| invalid("Нет файлов релиза."))?;
    let mut matches = assets.iter().filter(|a| {
        a["name"].as_str().is_some_and(|n| {
            n.starts_with("zapret-discord-youtube-") && n.to_ascii_lowercase().ends_with(".zip")
        })
    });
    let asset = matches.next().ok_or_else(|| invalid("Нет ZIP-пакета."))?;
    let url = asset["browser_download_url"].as_str().unwrap_or_default();
    let name = asset["name"].as_str().unwrap_or_default();
    let size = asset["size"].as_u64().unwrap_or(0);
    let hash = asset["digest"]
        .as_str()
        .and_then(|s| s.strip_prefix("sha256:"))
        .unwrap_or_default();
    if matches.next().is_some()
        || url != format!("{REPO}{tag}/{name}")
        || name.contains(['/', '\\'])
        || !(1..=MAX as u64).contains(&size)
        || hash.len() != 64
        || !hash.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Err(invalid("Неверные атрибуты пакета."));
    }
    let data = fetch_github(url, MAX, cancel)?;
    if data.len() as u64 != size
        || !format!("{:x}", Sha256::digest(&data)).eq_ignore_ascii_case(hash)
    {
        return Err(invalid("Не совпадает SHA-256 или размер."));
    }
    fs::create_dir_all(store)?;
    let tmp = tempfile::tempdir_in(store)?;
    extract(&data, tmp.path(), cancel)?;
    let root = find_root(tmp.path())?;
    prepare_lists(&root, previous.map(|p| p.root.as_path()))?;
    strategy::load(&root, |_| {})?;
    fs::write(
        tmp.path().join("release.json"),
        serde_json::to_vec(&serde_json::json!({"version":tag}))?,
    )?;
    cancel()?;
    let id = uuid::Uuid::new_v4().simple().to_string();
    fs::rename(tmp.path(), store.join(&id))?;
    open(store, &id).map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn disallow_special_paths() {
        for s in ["..", "NUL.txt", "COM1", "x:y", "a.", "", "a*b"] {
            assert!(!clean_component(s));
        }
    }
    #[test]
    fn load_preserves_the_previous_windows_scan_order() {
        let package = tempfile::tempdir().unwrap();
        for name in ["general.bat", "general (ALT).bat", "general (ALT2).bat"] {
            fs::write(
                package.path().join(name),
                r#"start "" "%BIN%winws.exe" --new"#,
            )
            .unwrap();
        }
        let strategies = strategy::load(package.path(), |_| {}).unwrap();
        let names: Vec<_> = strategies.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["general (ALT)", "general (ALT2)", "general"]);
    }
    #[test]
    fn game_ipset_keeps_its_location_when_the_runner_changes_directory() {
        let file = Path::new("GameFilter/ipset-all.txt");
        let filter = svyaz_settings::GameFilterSettings {
            mode: 1,
            ..Default::default()
        };
        let argv = strategy::arguments(
            r#"start "" "%BIN%winws.exe" --ipset="%LISTS%ipset-all.txt""#,
            Path::new("package"),
            &filter,
            Some(file),
        )
        .unwrap();
        assert_eq!(
            argv,
            [format!(
                "--ipset={}",
                std::env::current_dir().unwrap().join(file).display()
            )]
        );
    }
    #[test]
    fn ranges_and_quoted_paths() {
        assert!(svyaz_settings::normalize_ports("65536").is_err());
        assert_eq!(
            svyaz_settings::normalize_ports("1, 12-13").unwrap(),
            "1,12-13"
        );
        let a = strategy::arguments(
            "start cmd \"%BIN%winws.exe\" --list=\"%LISTS%my file\" ^\r\n --ports=%GameFilter%",
            Path::new("my folder"),
            &Default::default(),
            None,
        )
        .unwrap();
        assert_eq!(a.len(), 2);
        assert!(a[0].ends_with("my file"));
        assert_eq!(a[1], "--ports=12");
    }
}
