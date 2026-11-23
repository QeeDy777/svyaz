use crate::{CheckResult, Runner, SelectionError, StartError, VoiceEndpoint, probe_control};
use chrono::{DateTime, Duration as CalendarDuration, SecondsFormat, Utc};
use regex::Regex;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, Metadata},
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{
        LazyLock,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant, SystemTime},
};
use svyaz_settings::AppState;

static VOICE_HOST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i-u)[a-z0-9][a-z0-9-]{0,62}\.discord\.media(?::[0-9]{1,5})?").unwrap()
});

fn endpoint_in(line: &str) -> Option<VoiceEndpoint> {
    for location in VOICE_HOST.find_iter(line) {
        let boundary = |ch: char| ch.is_alphanumeric() || matches!(ch, '_' | '.' | '-');
        if line[..location.start()]
            .chars()
            .next_back()
            .is_some_and(boundary)
            || line[location.end()..]
                .chars()
                .next()
                .is_some_and(|ch| boundary(ch) || ch == ':')
        {
            continue;
        }
        if let Ok(endpoint) = location.as_str().parse() {
            return Some(endpoint);
        }
    }
    None
}

fn redirect(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn log_files(dir: &Path, depth: u8, cutoff: SystemTime, files: &mut Vec<(SystemTime, PathBuf)>) {
    if !fs::symlink_metadata(dir).is_ok_and(|m| m.is_dir() && !redirect(&m)) {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if redirect(&meta) {
            continue;
        }
        if meta.is_dir() {
            if depth < 2 {
                log_files(&path, depth + 1, cutoff, files);
            }
        } else if meta.is_file() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if !name.contains(".log") && !name.ends_with(".txt") {
                continue;
            }
            if let Ok(time) = meta.modified() {
                if time > cutoff {
                    files.push((time, path));
                }
            }
        }
    }
}

fn discover(roaming: &Path, now: DateTime<Utc>) -> Option<(VoiceEndpoint, String, DateTime<Utc>)> {
    let mut files = Vec::new();
    let cutoff = (now - CalendarDuration::days(7)).into();
    for app in ["discord", "discordptb", "discordcanary"] {
        let dir = roaming.join(app);
        if fs::symlink_metadata(&dir).is_ok_and(|m| !redirect(&m)) {
            log_files(&dir.join("logs"), 0, cutoff, &mut files);
        }
    }
    files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    const TAIL: u64 = 2 * 1024 * 1024;
    for (time, path) in files.into_iter().take(12) {
        let Ok(mut file) = File::open(path) else {
            continue;
        };
        let Ok(meta) = file.metadata() else {
            continue;
        };
        if file
            .seek(SeekFrom::Start(meta.len().saturating_sub(TAIL)))
            .is_err()
        {
            continue;
        }
        let mut bytes = Vec::new();
        if file.take(TAIL).read_to_end(&mut bytes).is_err() {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for line in text
            .trim_start_matches('\u{feff}')
            .rsplit('\n')
            .filter(|s| !s.contains("(stream)"))
        {
            if let Some(endpoint) = endpoint_in(line) {
                return Some((
                    endpoint,
                    format!("{:X}", Sha256::digest(line.as_bytes())),
                    time.into(),
                ));
            }
        }
    }
    None
}

fn remember(
    state: &mut AppState,
    endpoint: &VoiceEndpoint,
    observed: DateTime<Utc>,
    fingerprint: Option<String>,
) {
    // VoiceEndpoint always normalizes to wss://authority/?v=8.
    state.voice_server = Some(
        endpoint.as_str()[6..]
            .split('/')
            .next()
            .unwrap_or_default()
            .into(),
    );
    state.voice_observed_at = observed.to_rfc3339_opts(SecondsFormat::Millis, false);
    state.voice_log_fingerprint = fingerprint;
}

pub fn resolve_voice(
    manual: &str,
    roaming: &Path,
    state: &mut AppState,
) -> Result<Option<VoiceEndpoint>, &'static str> {
    let now = Utc::now();
    if !manual.trim().is_empty() {
        let endpoint = manual.parse()?;
        remember(state, &endpoint, now, None);
        return Ok(Some(endpoint));
    }
    let found = discover(roaming, now).map(|(endpoint, fingerprint, time)| {
        if Some(&fingerprint) != state.voice_log_fingerprint.as_ref() {
            remember(state, &endpoint, time, Some(fingerprint));
        }
        endpoint
    });
    let fresh = DateTime::parse_from_rfc3339(&state.voice_observed_at)
        .is_ok_and(|time| time > now - CalendarDuration::days(7));
    if !fresh {
        return Ok(None);
    }
    if let Some(endpoint) = found {
        return Ok(Some(endpoint));
    }
    state.voice_server.as_deref().map(str::parse).transpose()
}

fn guard(cancel: &AtomicBool) -> Result<(), SelectionError> {
    if cancel.load(Ordering::Acquire) {
        Err(SelectionError::Cancelled)
    } else {
        Ok(())
    }
}

/// Call after initial discovery found no address. A temporary strategy can obtain the
/// address, but only the subsequent full voice probe is allowed to confirm the connection.
pub fn bootstrap_voice<R: Runner>(
    runner: &mut R,
    strategies: &[String],
    preferred: Option<&str>,
    cancel: &AtomicBool,
    mut resolve: impl FnMut() -> Result<Option<VoiceEndpoint>, String>,
    mut log: impl FnMut(String),
) -> Result<VoiceEndpoint, SelectionError> {
    guard(cancel)?;
    let bootstrap = !runner.is_running();
    let result = (|| {
        if bootstrap {
            let mut ordered: Vec<_> = strategies.iter().map(String::as_str).collect();
            ordered.sort_by_key(|s| Some(*s) != preferred);
            let mut opened = false;
            for strategy in ordered {
                guard(cancel)?;
                runner.stop().map_err(SelectionError::Runner)?;
                match runner.start(strategy, cancel) {
                    Ok(()) => (),
                    Err(StartError::Candidate(message)) => {
                        log(message);
                        continue;
                    }
                    Err(StartError::Conflict(message)) => {
                        return Err(SelectionError::RequiresManualRetry(message));
                    }
                }
                let control = probe_control(cancel);
                guard(cancel)?;
                log(format!(
                    "{strategy} (получение адреса): API {:?}; Gateway {:?}",
                    control.api, control.gateway
                ));
                if matches!(control.api, CheckResult::Success { .. })
                    && matches!(control.gateway, CheckResult::Success { .. })
                {
                    opened = true;
                    break;
                }
            }
            if !opened {
                return Err(SelectionError::RequiresManualRetry("Подбор завершён. Ни одна стратегия не открыла API и Gateway Discord для получения голосового адреса.".into()));
            }
        }
        for _ in 0..20 {
            let start = Instant::now();
            while start.elapsed() < Duration::from_secs(3) {
                guard(cancel)?;
                thread::sleep(Duration::from_millis(50));
            }
            guard(cancel)?;
            if let Some(endpoint) = resolve().map_err(SelectionError::Runner)? {
                return Ok(endpoint);
            }
        }
        Err(SelectionError::RequiresManualRetry("Не найден адрес голосового сервера. Войдите в канал Discord или укажите адрес в настройках.".into()))
    })();
    if bootstrap {
        runner.stop().map_err(SelectionError::Runner)?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn selects_latest_non_stream_line_and_preserves_cache_expiry() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("discordcanary/logs/sub");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("rtc.log.1");
        let line = "connected to two.discord.media:443\r";
        fs::write(
            &file,
            format!("one.discord.media\n{line}\n(stream) three.discord.media\n"),
        )
        .unwrap();
        let mut state = AppState::default();
        let voice = resolve_voice("", root.path(), &mut state).unwrap().unwrap();
        assert_eq!(voice.as_str(), "wss://two.discord.media/?v=8");
        assert_eq!(
            state.voice_log_fingerprint,
            Some(format!("{:X}", Sha256::digest(line.as_bytes())))
        );
        state.voice_observed_at = "2001-01-01T00:00:00+00:00".into();
        assert!(
            resolve_voice("", root.path(), &mut state)
                .unwrap()
                .is_none()
        );
        fs::remove_file(file).unwrap();
        resolve_voice("manual.discord.media", root.path(), &mut state).unwrap();
        assert!(state.voice_log_fingerprint.is_none());
        assert_eq!(
            resolve_voice("", root.path(), &mut state)
                .unwrap()
                .unwrap()
                .as_str(),
            "wss://manual.discord.media/?v=8"
        );
    }
    #[test]
    fn malformed_labels_and_stream_hosts_are_never_substituted() {
        for line in [
            "xn.zz.discord.media",
            "abc.discord.media.evil",
            "a.discord.media:900999",
            "_a.discord.media",
            "éa.discord.media",
        ] {
            assert!(endpoint_in(line).is_none(), "{line}");
        }
    }
}
