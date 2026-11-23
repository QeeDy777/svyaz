use super::invalid;
use std::{
    fs,
    io::{self, Read, Write},
    net::IpAddr,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
const MAX: usize = 2 * 1024 * 1024;
const SOURCE: &str = "https://raw.githubusercontent.com/Flowseal/zapret-discord-youtube/refs/heads/main/.service/ipset-service.txt";

fn validate(input: &str) -> io::Result<String> {
    let mut output = String::new();
    let mut count = 0;
    if input.len() > MAX {
        return Err(invalid("IPSet слишком велик."));
    }
    for s in input
        .trim_start_matches('\u{feff}')
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with('#'))
    {
        let (ip, bits) = s
            .split_once('/')
            .ok_or_else(|| invalid("Некорректная сеть IPSet."))?;
        let ip: IpAddr = ip
            .parse()
            .map_err(|_| invalid("Некорректный адрес IPSet."))?;
        let bits: u8 = bits.parse().map_err(|_| invalid("Некорректная маска."))?;
        if s.len() > 80 || bits == 0 || bits > if ip.is_ipv4() { 32 } else { 128 } {
            return Err(invalid("Некорректная сеть IPSet."));
        }
        if s == "203.0.113.113/32" {
            continue;
        }
        count += 1;
        if count > 100_000 {
            return Err(invalid("Слишком много сетей IPSet."));
        }
        output.push_str(&format!("{ip}/{bits}\n"));
    }
    if count == 0 {
        return Err(invalid("Пустой IPSet."));
    }
    Ok(output)
}
pub fn ensure(path: &Path, cancel: &AtomicBool) -> io::Result<()> {
    let guard = || {
        if cancel.load(Ordering::Acquire) {
            Err(io::Error::new(io::ErrorKind::Interrupted, "Отменено"))
        } else {
            Ok(())
        }
    };
    guard()?;
    let cache = fs::metadata(path)
        .ok()
        .filter(|m| m.len() <= MAX as u64)
        .filter(|_| {
            fs::read_to_string(path)
                .ok()
                .is_some_and(|s| validate(&s).is_ok())
        });
    if cache
        .as_ref()
        .and_then(|m| m.modified().ok()?.elapsed().ok())
        .is_some_and(|t| t < Duration::from_secs(6 * 3600))
    {
        return Ok(());
    }
    let download = || -> io::Result<String> {
        let client = reqwest::blocking::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(io::Error::other)?;
        let mut response = client.get(SOURCE).send().map_err(io::Error::other)?;
        if !response.status().is_success() {
            return Err(io::Error::other("Не удалось получить IPSet."));
        }
        let mut text = String::new();
        (&mut response)
            .take(MAX as u64 + 1)
            .read_to_string(&mut text)?;
        validate(&text)
    };
    let data = download();
    guard()?;
    match data {
        Ok(text) => {
            let dir = path
                .parent()
                .ok_or_else(|| invalid("Нет каталога IPSet."))?;
            fs::create_dir_all(dir)?;
            let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
            tmp.write_all(text.as_bytes())?;
            tmp.as_file().sync_all()?;
            tmp.persist(path)?;
            Ok(())
        }
        Err(_) if cache.is_some() => Ok(()),
        Err(e) => Err(e),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_and_placeholder_ranges() {
        assert!(validate("203.0.113.113/32").is_err());
        assert!(validate("1.2.3.4/33").is_err());
        assert_eq!(
            validate("# hi\n203.0.113.113/32\n2001:db8::/32\n").unwrap(),
            "2001:db8::/32\n"
        );
    }
}
