use crate::{invalid, Version};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::File,
    io::{self, Cursor, Read, Write},
    path::Path,
    str::FromStr,
};
use zip::ZipArchive;

pub const LATEST_URL: &str = "https://api.github.com/repos/QeeDy777/svyaz/releases/latest";
pub const LIMIT: usize = 160 * 1024 * 1024;
// The 0.13.3 stable launcher hashes these exact names, in this order, even after an upgrade.
pub const RUNTIME_FILES: [&str; 6] = [
    "Связь.exe",
    "D3DCompiler_47_cor3.dll",
    "PenImc_cor3.dll",
    "PresentationNative_cor3.dll",
    "vcruntime140_cor3.dll",
    "wpfgfx_cor3.dll",
];
pub(crate) const LICENSES: [&str; 4] = [
    "DOTNET-LICENSE.txt",
    "DOTNET-THIRD-PARTY-NOTICES.txt",
    "WINDOWS-DESKTOP-LICENSE.txt",
    "TG-WS-PROXY-LICENSE.txt",
];

#[derive(Debug)]
pub struct Release {
    pub version: Version,
    pub url: String,
    pub size: usize,
    digest: String,
}

impl Release {
    pub fn parse(json: &[u8]) -> io::Result<Self> {
        #[derive(Deserialize)]
        struct Metadata {
            draft: bool,
            prerelease: bool,
            tag_name: String,
            assets: Vec<Asset>,
        }
        #[derive(Deserialize)]
        struct Asset {
            name: String,
            size: usize,
            digest: Option<String>,
            browser_download_url: String,
        }
        let data: Metadata = serde_json::from_slice(json)?;
        let version = Version::from_str(
            data.tag_name
                .strip_prefix('v')
                .ok_or_else(|| invalid("Неверный тег релиза."))?,
        )?;
        if data.draft || data.prerelease {
            return Err(invalid("Ожидался стабильный релиз."));
        }
        for variant in ["signed", "unsigned"] {
            let name = format!("Svyaz-{version}-win-x64-{variant}.zip");
            let mut matches = data.assets.iter().filter(|a| a.name == name);
            let Some(asset) = matches.next() else {
                continue;
            };
            let url =
                format!("https://github.com/QeeDy777/svyaz/releases/download/v{version}/{name}");
            let digest = asset
                .digest
                .as_deref()
                .and_then(|s| s.strip_prefix("sha256:"))
                .unwrap_or("");
            if matches.next().is_some()
                || asset.size == 0
                || asset.size > LIMIT
                || asset.browser_download_url != url
                || digest.len() != 64
                || !digest.bytes().all(|b| b.is_ascii_hexdigit())
            {
                return Err(invalid("Неверные данные Windows-пакета."));
            }
            return Ok(Self {
                version,
                url,
                size: asset.size,
                digest: digest.to_ascii_lowercase(),
            });
        }
        Err(invalid("Нет архива для Windows x64."))
    }
}

// zip indexes entries by name. Compare with the actual ZIP directory before trusting its index.
fn entry_count(bytes: &[u8]) -> io::Result<usize> {
    for offset in (bytes.len().saturating_sub(65557)..=bytes.len().saturating_sub(22)).rev() {
        let Some(end) = bytes.get(offset..offset + 22) else {
            break;
        };
        if &end[..4] == b"PK\x05\x06"
            && offset + 22 + usize::from(u16::from_le_bytes([end[20], end[21]])) == bytes.len()
        {
            if end[4..8] != [0, 0, 0, 0] || end[8..10] != end[10..12] {
                break;
            }
            return Ok(usize::from(u16::from_le_bytes([end[10], end[11]])));
        }
    }
    Err(invalid("Неверный каталог архива."))
}

pub(crate) fn extract(
    release: &Release,
    bytes: &[u8],
    out: &Path,
    cancel: &dyn Fn() -> io::Result<()>,
) -> io::Result<()> {
    cancel()?;
    if bytes.len() != release.size
        || bytes.len() > LIMIT
        || format!("{:x}", Sha256::digest(bytes)) != release.digest
    {
        return Err(invalid("Размер или SHA-256 обновления не совпадает."));
    }
    let count = entry_count(bytes)?;
    if !(RUNTIME_FILES.len()..=RUNTIME_FILES.len() + LICENSES.len() + 3).contains(&count) {
        return Err(invalid("Недопустимое число файлов."));
    }
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(invalid)?;
    if archive.len() != count {
        return Err(invalid("Повторяющиеся файлы ZIP."));
    }
    let mut names = HashSet::new();
    let mut total = 0;
    for i in 0..archive.len() {
        let f = archive.by_index(i).map_err(invalid)?;
        let name = f.name();
        let allowed = RUNTIME_FILES.contains(&name)
            || LICENSES.contains(&name)
            || ["README.txt", "SIGNING.txt", "VALIDATION.txt"].contains(&name);
        let kind = f.unix_mode().unwrap_or(0) & 0o170000;
        if !allowed
            || !names.insert(name.to_owned())
            || (kind != 0 && kind != 0o100000)
            || f.size() == 0
            || f.size() > LIMIT as u64
        {
            return Err(invalid("Недопустимый файл в обновлении."));
        }
        total += f.size();
    }
    if total > 256 * 1024 * 1024 || !RUNTIME_FILES.iter().all(|n| names.contains(*n)) {
        return Err(invalid("Неполный или чрезмерно большой пакет."));
    }
    let mut buf = [0u8; 65536];
    for i in 0..archive.len() {
        let mut input = archive.by_index(i).map_err(invalid)?;
        let mut output = File::create_new(out.join(input.name()))?;
        let mut copied = 0u64;
        loop {
            cancel()?;
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            copied += n as u64;
            if copied > input.size() {
                return Err(invalid("Превышен размер файла в ZIP."));
            }
            output.write_all(&buf[..n])?;
        }
        if copied != input.size() {
            return Err(invalid("Неполный файл в ZIP."));
        }
    }
    Ok(())
}
