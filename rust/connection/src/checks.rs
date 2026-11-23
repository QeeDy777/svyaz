use std::{
    future::Future,
    str::FromStr,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

use futures_util::StreamExt;
use reqwest::{Client, Url, redirect::Policy};
use serde_json::Value;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};

const TIMEOUT: Duration = Duration::from_secs(7);
const API: &str = "https://discord.com/api/v10/gateway";
const GATEWAY: &str = "wss://gateway.discord.gg/?v=10&encoding=json";
const YOUTUBE: &str = "https://www.youtube.com/generate_204";
const GOOGLE_VIDEO: &str = "https://redirector.googlevideo.com/generate_204";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CheckResult {
    Success { elapsed_ms: u64 },
    Failed(String),
    Skipped,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProbeResult {
    pub api: CheckResult,
    pub gateway: CheckResult,
    pub voice: CheckResult,
    pub youtube: CheckResult,
    pub google_video: CheckResult,
}

impl ProbeResult {
    fn checks(&self) -> [&CheckResult; 5] {
        [
            &self.api,
            &self.gateway,
            &self.voice,
            &self.youtube,
            &self.google_video,
        ]
    }

    pub fn success(&self) -> bool {
        let checks = self.checks();
        !checks.iter().any(|s| matches!(s, CheckResult::Failed(_)))
            && checks
                .iter()
                .any(|s| matches!(s, CheckResult::Success { .. }))
    }

    pub fn sample_ms(&self) -> Option<u64> {
        if !self.success() {
            return None;
        }
        self.checks()
            .iter()
            .filter_map(|s| match s {
                CheckResult::Success { elapsed_ms } => Some(*elapsed_ms),
                _ => None,
            })
            .max()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoiceEndpoint(String);

impl VoiceEndpoint {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl FromStr for VoiceEndpoint {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        const INVALID: &str = "Нужен адрес голосового сервера вида имя.discord.media:порт.";
        let input = value.trim();
        let raw = if input.contains("://") {
            input.to_owned()
        } else {
            format!("wss://{input}")
        };
        // Reject userinfo, path folding and non-ASCII aliases before URL normalization.
        let authority = raw
            .split_once("://")
            .ok_or(INVALID)?
            .1
            .split(['/', '?', '#'])
            .next()
            .ok_or(INVALID)?;
        if !authority.is_ascii()
            || authority.contains(['@', '%', '\\'])
            || raw.chars().any(char::is_control)
        {
            return Err(INVALID);
        }
        let mut url = Url::parse(&raw).map_err(|_| INVALID)?;
        let host = url.host_str().ok_or(INVALID)?;
        let label = host.strip_suffix(".discord.media").ok_or(INVALID)?;
        if url.scheme() != "wss"
            || url.path() != "/"
            || url.fragment().is_some()
            || url.port() == Some(0)
            || label.is_empty()
            || label.len() > 63
            || label.starts_with('-')
            || label.ends_with('-')
            || !label
                .bytes()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == b'-')
        {
            return Err(INVALID);
        }
        url.set_query(Some("v=8"));
        Ok(Self(url.to_string()))
    }
}

fn valid_gateway(bytes: &[u8]) -> bool {
    serde_json::from_slice::<Value>(bytes)
        .ok()
        .and_then(|v| {
            v.get("url")
                .and_then(Value::as_str)
                .map(|s| s == "wss://gateway.discord.gg")
        })
        .unwrap_or(false)
}

fn valid_hello(text: &str, opcode: i64) -> bool {
    let Ok(value) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    value.get("op").and_then(Value::as_i64) == Some(opcode)
        && value
            .pointer("/d/heartbeat_interval")
            .and_then(Value::as_f64)
            .is_some_and(|interval| interval > 0.0 && interval <= 300_000.0)
}

type NetworkResult = Result<bool, String>;

async fn hello(endpoint: &str, opcode: i64) -> NetworkResult {
    let config = WebSocketConfig::default()
        .max_frame_size(Some(16_384))
        .max_message_size(Some(16_384));
    let (mut ws, _) = connect_async_with_config(endpoint, Some(config), false)
        .await
        .map_err(|_| "Не удалось подключиться по WebSocket".to_owned())?;
    while let Some(frame) = ws.next().await {
        match frame.map_err(|_| "Ошибка чтения WebSocket".to_owned())? {
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Text(text) => return Ok(valid_hello(text.as_str(), opcode)),
            _ => return Ok(false),
        }
    }
    Ok(false)
}

async fn discord_api(client: &Client) -> NetworkResult {
    let mut response = client
        .get(API)
        .header("User-Agent", "DiscordRoute/0.1")
        .header("Cache-Control", "no-cache")
        .send()
        .await
        .map_err(|_| "Ошибка подключения к API Discord".to_owned())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status().as_u16()));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Ошибка чтения API Discord".to_owned())?
    {
        if chunk.len() > 8192 - bytes.len() {
            return Ok(false);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(valid_gateway(&bytes))
}

async fn generate_204(client: &Client, endpoint: &str) -> NetworkResult {
    let response = client
        .get(endpoint)
        .header("Cache-Control", "no-cache, no-store")
        .send()
        .await
        .map_err(|_| "Ошибка подключения HTTPS".to_owned())?;
    if response.status().as_u16() != 204 {
        return Err(format!(
            "Ожидался HTTP 204, получен {}",
            response.status().as_u16()
        ));
    }
    Ok(true)
}

async fn measure(enabled: bool, future: impl Future<Output = NetworkResult>) -> CheckResult {
    if !enabled {
        return CheckResult::Skipped;
    }
    let start = Instant::now();
    match tokio::time::timeout(TIMEOUT, future).await {
        Err(_) => CheckResult::Failed("Тайм-аут 7 с".into()),
        Ok(Err(reason)) => CheckResult::Failed(reason),
        Ok(Ok(false)) => CheckResult::Failed("Нет корректного ответа".into()),
        Ok(Ok(true)) => CheckResult::Success {
            elapsed_ms: start.elapsed().as_millis() as u64,
        },
    }
}

async fn cancelled(cancel: &AtomicBool) {
    while !cancel.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Run on the connection worker. A fresh client per pass prevents connection reuse across strategies.
pub fn probe(
    discord: bool,
    youtube: bool,
    endpoint: Option<&VoiceEndpoint>,
    cancel: &AtomicBool,
) -> ProbeResult {
    run(discord, discord, youtube, endpoint, cancel)
}

pub fn probe_control(cancel: &AtomicBool) -> ProbeResult {
    run(true, false, false, None, cancel)
}

fn run(
    discord: bool,
    voice: bool,
    youtube: bool,
    endpoint: Option<&VoiceEndpoint>,
    cancel: &AtomicBool,
) -> ProbeResult {
    let failed = |error: &str| {
        let one = |enabled| {
            if enabled {
                CheckResult::Failed(error.to_owned())
            } else {
                CheckResult::Skipped
            }
        };
        ProbeResult {
            api: one(discord),
            gateway: one(discord),
            voice: one(voice),
            youtube: one(youtube),
            google_video: one(youtube),
        }
    };
    if cancel.load(Ordering::Acquire) {
        return failed("Отменено");
    }
    let Ok(rt) = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    else {
        return failed("Не удалось начать проверку");
    };
    rt.block_on(async {
        let Ok(client) = Client::builder().no_proxy().redirect(Policy::none()).build() else { return failed("Не удалось настроить HTTPS"); };
        let all = async {
            let (api, gateway, voice, youtube, google_video) = tokio::join!(
                measure(discord, discord_api(&client)),
                measure(discord, hello(GATEWAY, 10)),
                measure(voice, async {
                    let address = endpoint.ok_or_else(|| "Нет адреса голосового сервера.".to_owned())?;
                    hello(address.as_str(), 8).await
                }),
                measure(youtube, generate_204(&client, YOUTUBE)),
                measure(youtube, generate_204(&client, GOOGLE_VIDEO)),
            );
            ProbeResult { api, gateway, voice, youtube, google_video }
        };
        tokio::select! { biased; _ = cancelled(cancel) => failed("Отменено"), result = all => result }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn different_protocols_and_block_pages() {
        let voice = r#"{"op":8,"d":{"heartbeat_interval":45000}}"#;
        assert!(valid_hello(voice, 8));
        assert!(!valid_hello(voice, 10));
        for invalid in [
            r#"{"op":8,"d":{"heartbeat_interval":0}}"#,
            r#"{"op":8,"d":{"heartbeat_interval":300001}}"#,
            "<html>",
        ] {
            assert!(!valid_hello(invalid, 8));
        }
        assert!(valid_gateway(br#"{"url":"wss://gateway.discord.gg"}"#));
        assert!(!valid_gateway(
            br#"{"url":"wss://gateway.discord.gg.example.org"}"#
        ));
    }
    #[test]
    fn endpoint_allowlist_and_privacy() {
        assert_eq!(
            " HOST.discord.media:8443/?token=secret "
                .parse::<VoiceEndpoint>()
                .unwrap()
                .as_str(),
            "wss://host.discord.media:8443/?v=8"
        );
        for input in [
            "discord.media",
            "https://x.discord.media",
            "wss://x.discord.media@bad.xyz",
            "user@x.discord.media",
            "x.discord.media:0",
            "a.b.discord.media",
            "x.discord.media/path",
            "x.discord.media/#fragment",
            "s.discord.media.bad",
        ] {
            assert!(input.parse::<VoiceEndpoint>().is_err(), "{input}");
        }
    }
    #[test]
    fn cancel_and_skips_cannot_confirm_connectivity() {
        let cancel = AtomicBool::new(true);
        let result = probe(true, false, None, &cancel);
        assert!(!result.success());
        assert_eq!(result.youtube, CheckResult::Skipped);
        assert!(!probe(false, false, None, &cancel).success());
    }
}
