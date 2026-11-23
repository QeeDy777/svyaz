//! settings.json and preferences.json as written by «Связь» 0.13.3.
use std::{fs, io, path::Path};

use indexmap::IndexMap;
use serde::de::DeserializeOwned;
use serde::ser::SerializeMap;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};
use tempfile::NamedTempFile;

pub const SETTINGS_FILE: &str = "settings.json";
pub const PREFERENCES_FILE: &str = "preferences.json";
pub const FIRST_TIMESTAMP: &str = "0001-01-01T00:00:00+00:00";

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct AppState {
    pub zapret_reset_pending: bool,
    pub active_package: Option<String>,
    pub pending_package: Option<String>,
    pub previous_package: Option<String>,
    pub last_strategy: Option<String>,
    pub voice_server: Option<String>,
    pub voice_log_fingerprint: Option<String>,
    // Retain .NET's fractional precision and offset when unrelated settings change.
    pub voice_observed_at: String,
    pub auto_update: bool,
    pub game_filter: GameFilterSettings,
    pub telegram: TelegramSettings,
    pub telegram_active_package: Option<String>,
    pub telegram_pending_package: Option<String>,
    pub autostart_configured: bool,
    pub last_update_check: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            zapret_reset_pending: false,
            active_package: None,
            pending_package: None,
            previous_package: None,
            last_strategy: None,
            voice_server: None,
            voice_log_fingerprint: None,
            voice_observed_at: FIRST_TIMESTAMP.into(),
            auto_update: true,
            game_filter: GameFilterSettings::default(),
            telegram: TelegramSettings::default(),
            telegram_active_package: None,
            telegram_pending_package: None,
            autostart_configured: false,
            last_update_check: FIRST_TIMESTAMP.into(),
            extra: Map::new(),
        }
    }
}

impl AppState {
    pub fn load(path: &Path) -> io::Result<Self> {
        Ok(read(path)?.unwrap_or_default())
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        write(path, self)
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct GameFilterSettings {
    pub mode: i32,
    pub tcp_ports: String,
    pub udp_ports: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for GameFilterSettings {
    fn default() -> Self {
        Self {
            mode: 0,
            tcp_ports: "1024-65535".into(),
            udp_ports: "1024-65535".into(),
            extra: Map::new(),
        }
    }
}

impl GameFilterSettings {
    /// Values for the legacy, TCP and UDP placeholders used by the existing package format.
    pub fn ranges(&self) -> io::Result<[String; 3]> {
        let tcp = normalize_ports(&self.tcp_ports)?;
        let udp = normalize_ports(&self.udp_ports)?;
        let disabled = || String::from("12");
        match self.mode {
            0 => Ok([disabled(), disabled(), disabled()]),
            1 => Ok([tcp.clone(), tcp, udp]),
            2 => Ok([tcp.clone(), tcp, disabled()]),
            3 => Ok([udp.clone(), disabled(), udp]),
            _ => Err(invalid("Неизвестный режим Game Filter.")),
        }
    }
}

pub fn normalize_ports(text: &str) -> io::Result<String> {
    let error = || invalid("Порты: числа или диапазоны от 1 до 65535 через запятую.");
    if text.encode_utf16().count() > 2048 {
        return Err(error());
    }
    let ports = text.replace(' ', "");
    let parse = |n: &str| -> io::Result<u16> {
        if n.starts_with('0') || !n.bytes().all(|x| x.is_ascii_digit()) {
            return Err(error());
        }
        n.parse().map_err(|_| error())
    };
    for item in ports.split(',') {
        let (lower, upper) = item.split_once('-').unwrap_or((item, item));
        if parse(lower)? > parse(upper)? {
            return Err(error());
        }
    }
    Ok(ports)
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct TelegramSettings {
    pub port: i32,
    pub secret: String,
    pub start_with_app: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for TelegramSettings {
    fn default() -> Self {
        Self {
            port: 1443,
            secret: String::new(),
            start_with_app: false,
            extra: Map::new(),
        }
    }
}

#[derive(PartialEq, Eq, Clone, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ServiceSelection {
    pub discord: bool,
    #[serde(rename = "YouTube")]
    pub youtube: bool,
    pub telegram: bool,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for ServiceSelection {
    fn default() -> Self {
        Self {
            discord: true,
            youtube: true,
            telegram: false,
            extra: Map::new(),
        }
    }
}

impl ServiceSelection {
    pub fn uses_zapret(&self) -> bool {
        self.discord || self.youtube
    }

    pub fn validate(&self) -> io::Result<()> {
        if self.uses_zapret() || self.telegram {
            Ok(())
        } else {
            Err(invalid("Выберите хотя бы один сервис."))
        }
    }
}

impl Serialize for ServiceSelection {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("Discord", &self.discord)?;
        map.serialize_entry("YouTube", &self.youtube)?;
        map.serialize_entry("Telegram", &self.telegram)?;
        // The previous client serialized this getter and ignored it on read.
        map.serialize_entry("UsesZapret", &self.uses_zapret())?;
        for (key, value) in &self.extra {
            if key != "UsesZapret" {
                map.serialize_entry(key, value)?;
            }
        }
        map.end()
    }
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct ConvenienceState {
    // Legacy profiles evict the oldest insertion when adding a 33rd network.
    #[serde(deserialize_with = "read_networks")]
    pub network_strategies: IndexMap<String, String>,
    pub services: Option<ServiceSelection>,
    pub quiet_notifications: bool,
    pub connection_sounds: bool,
    pub last_seen_app_version: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

fn read_networks<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<IndexMap<String, String>, D::Error> {
    Ok(Option::deserialize(deserializer)?.unwrap_or_default())
}

impl Default for ConvenienceState {
    fn default() -> Self {
        Self {
            network_strategies: IndexMap::new(),
            services: None,
            quiet_notifications: true,
            connection_sounds: true,
            last_seen_app_version: None,
            extra: Map::new(),
        }
    }
}

impl ConvenienceState {
    pub fn load(path: &Path) -> io::Result<Self> {
        // The 0.13.3 preferences reader also accepted a literal null.
        Ok(read::<Option<Self>>(path)?.flatten().unwrap_or_default())
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        write(path, self)
    }

    pub fn effective_services(&self, state: &AppState) -> ServiceSelection {
        self.services.clone().unwrap_or_else(|| ServiceSelection {
            telegram: state.telegram.start_with_app,
            ..ServiceSelection::default()
        })
    }

    pub fn remember_network(
        &mut self,
        started: Option<&str>,
        current: Option<&str>,
        strategy: &str,
    ) -> bool {
        let Some(id) = started.filter(|_| started == current) else {
            return false;
        };
        if !self.network_strategies.contains_key(id) && self.network_strategies.len() >= 32 {
            self.network_strategies.shift_remove_index(0);
        }
        self.network_strategies
            .insert(id.to_owned(), strategy.to_owned());
        true
    }
}

fn read<T: DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    let contents = match fs::read(path) {
        Ok(value) => value,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    // File.ReadAllText consumed the UTF-8 BOM on older installations.
    let bytes = contents.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&contents);
    serde_json::from_slice(bytes)
        .map(Some)
        .map_err(|e| invalid(&e.to_string()))
}

fn write(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let mut temp = NamedTempFile::new_in(dir)?;
    serde_json::to_writer_pretty(temp.as_file_mut(), value)?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    Ok(())
}
