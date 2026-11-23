use std::io;
use svyaz_settings::TelegramSettings;

pub fn validate(settings: &TelegramSettings) -> io::Result<u16> {
    let port = u16::try_from(settings.port)
        .ok()
        .filter(|p| *p != 0)
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Порт Telegram должен быть от 1 до 65535.",
            )
        })?;
    if !settings.secret.is_empty()
        && (settings.secret.len() != 32 || !settings.secret.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Повреждён ключ локального прокси Telegram.",
        ));
    }
    Ok(port)
}

pub fn ensure_secret(settings: &mut TelegramSettings) -> io::Result<()> {
    validate(settings)?;
    if settings.secret.is_empty() {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes)
            .map_err(|_| io::Error::other("Не удалось создать ключ Telegram."))?;
        settings.secret = format!("{:032x}", u128::from_be_bytes(bytes));
    } else {
        settings.secret.make_ascii_lowercase();
    }
    Ok(())
}

pub fn proxy_url(settings: &TelegramSettings) -> io::Result<String> {
    let port = validate(settings)?;
    if settings.secret.is_empty() {
        return Err(io::Error::other("Сначала запустите прокси Telegram."));
    }
    Ok(format!(
        "tg://proxy?server=127.0.0.1&port={port}&secret=dd{}",
        settings.secret.to_ascii_lowercase()
    ))
}

pub(crate) fn configuration(
    root: &std::path::Path,
    state: &svyaz_settings::AppState,
) -> io::Result<String> {
    use serde_json::{Map, Value, json};
    use std::{
        fs,
        io::{Read, Write},
    };
    let settings = &state.telegram;
    let port = validate(settings)?;
    if settings.secret.is_empty() {
        return Err(io::Error::other("Не создан ключ Telegram."));
    }
    let config_path = |root: &std::path::Path| root.join("TgWsProxy_data").join("config.json");
    let path = config_path(root);
    let mut source = path.clone();
    if !source.try_exists()? {
        for id in [
            &state.telegram_active_package,
            &state.telegram_pending_package,
        ]
        .into_iter()
        .flatten()
        {
            if id.len() != 32
                || !id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Повреждён идентификатор прежнего компонента Telegram.",
                ));
            }
            let candidate = config_path(&root.join("Packages").join(id));
            if candidate.try_exists()? {
                source = candidate;
                break;
            }
        }
    }
    let mut values = match fs::File::open(&source) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(65537).read_to_end(&mut bytes)?;
            if bytes.len() > 65536 {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Настройки Telegram слишком велики.",
                ));
            }
            serde_json::from_slice::<Map<String, Value>>(
                bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&bytes),
            )
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Неверный формат настроек Telegram.",
                )
            })?
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Map::new(),
        Err(e) => return Err(e),
    };
    for (key, value) in [
        ("host", json!("127.0.0.1")),
        ("port", json!(port)),
        ("secret", json!(settings.secret.to_ascii_lowercase())),
        ("autostart", json!(false)),
        ("check_updates", json!(false)),
        ("language", json!("ru")),
    ] {
        values.insert(key.into(), value);
    }
    let serialized = serde_json::to_string(&values)?;
    let dir = path.parent().unwrap();
    fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    tmp.write_all(serialized.as_bytes())?;
    tmp.as_file().sync_all()?;
    tmp.persist(&path)?;
    fs::write(dir.join(".first_run_done_mtproto"), b"")?;
    Ok(serialized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use svyaz_settings::AppState;
    #[test]
    fn legacy_config_migrates_once_and_retains_user_fields() {
        let dir = tempfile::tempdir().unwrap();
        let id = "a".repeat(32);
        let old = dir.path().join("Packages").join(&id).join("TgWsProxy_data");
        fs::create_dir_all(&old).unwrap();
        fs::write(old.join("config.json"),r#"{"host":"0.0.0.0","port":1234,"cfproxy_user_domain":["example.org"],"pool_size":7,"autostart":true,"check_updates":true}"#).unwrap();
        let mut state = AppState {
            telegram_active_package: Some(id),
            ..Default::default()
        };
        ensure_secret(&mut state.telegram).unwrap();
        let url = proxy_url(&state.telegram).unwrap();
        ensure_secret(&mut state.telegram).unwrap();
        assert_eq!(url, proxy_url(&state.telegram).unwrap());
        assert_eq!(
            url.len(),
            "tg://proxy?server=127.0.0.1&port=1443&secret=dd".len() + 32
        );
        let value: serde_json::Value =
            serde_json::from_str(&configuration(dir.path(), &state).unwrap()).unwrap();
        assert_eq!(value["host"], "127.0.0.1");
        assert_eq!(value["pool_size"], 7);
        assert_eq!(value["port"], 1443);
        assert_eq!(value["autostart"], false);
        assert_eq!(value["check_updates"], false);
        assert!(
            dir.path()
                .join("TgWsProxy_data/.first_run_done_mtproto")
                .is_file()
        );
        fs::write(old.join("config.json"), "{}").unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&configuration(dir.path(), &state).unwrap())
                .unwrap(),
            value
        );
    }
    #[test]
    fn invalid_secret_port_and_source_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = AppState::default();
        assert!(proxy_url(&state.telegram).is_err());
        state.telegram.secret = "01234bad".into();
        assert!(ensure_secret(&mut state.telegram).is_err());
        state.telegram.secret = "A".repeat(32);
        state.telegram.port = 65536;
        assert!(ensure_secret(&mut state.telegram).is_err());
        state.telegram.port = 443;
        ensure_secret(&mut state.telegram).unwrap();
        assert_eq!(state.telegram.secret, "a".repeat(32));
        state.telegram_active_package = Some("../abc".into());
        assert!(configuration(dir.path(), &state).is_err());
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }
}

/// Include the Fake TLS domain in the Telegram URI only when enabled in the active on-disk config.
pub fn proxy_url_at(
    data_root: &std::path::Path,
    state: &svyaz_settings::AppState,
) -> io::Result<String> {
    use std::{fmt::Write as _, fs, io::Read};
    let base = proxy_url(&state.telegram)?;
    let file = match fs::File::open(data_root.join("Telegram/TgWsProxy_data/config.json")) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(base),
        Err(e) => return Err(e),
    };
    let mut bytes = Vec::new();
    file.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Настройки Telegram слишком велики.",
        ));
    }
    let value: serde_json::Value = serde_json::from_slice(&bytes)?;
    let Some(domain) = value
        .get("fake_tls_domain")
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
    else {
        return Ok(base);
    };
    if domain.len() > 253
        || !domain
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Неверный домен Fake TLS.",
        ));
    }
    let mut link = base.replace("secret=dd", "secret=ee");
    for b in domain.bytes() {
        write!(&mut link, "{b:02x}").unwrap();
    }
    Ok(link)
}
