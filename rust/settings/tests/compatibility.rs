use serde_json::{Value, json};
use std::{fs, io::ErrorKind};
use svyaz_settings::{AppState, ConvenienceState, FIRST_TIMESTAMP, ServiceSelection};
use tempfile::tempdir;

#[test]
fn legacy_files_survive_edit_and_restart() {
    let dir = tempdir().unwrap();
    let settings = dir.path().join("settings.json");
    let prefs = dir.path().join("preferences.json");
    let before = json!({"AutoUpdate":false,"VoiceObservedAt":"2026-11-14T13:15:16.1234567+07:00", "LastStrategy":"ALT 2", "ActivePackage":"abc", "Telegram":{"Port":1444,"StartWithApp":true,"Secret":"00112233445566778899aabbccddeeff","Future":{"ключ":1}},"GameFilter":{"Mode":2,"TcpPorts":"443,1024-65535","UdpPorts":"12","Future":false},"Unknown":[1,null]});
    fs::write(&settings, format!("\u{feff}{before}")).unwrap();
    fs::write(&prefs,r#"{"NetworkStrategies":{"z":"старая","a":"новая"},"QuietNotifications":false,"LastSeenAppVersion":"0.13.3","Future":12}"#).unwrap();
    let mut state = AppState::load(&settings).unwrap();
    let mut preferences = ConvenienceState::load(&prefs).unwrap();
    assert!(preferences.connection_sounds);
    preferences.connection_sounds = false;
    assert!(preferences.effective_services(&state).telegram);
    assert!(!state.auto_update);
    state.last_strategy = Some("ALT 3".into());
    state.save(&settings).unwrap();
    let mut after: Value = serde_json::from_slice(&fs::read(&settings).unwrap()).unwrap();
    assert_eq!(after["LastStrategy"], "ALT 3");
    after["LastStrategy"] = before["LastStrategy"].clone();
    for (key, value) in before.as_object().unwrap() {
        assert_eq!(&after[key], value);
    }
    preferences.services = Some(ServiceSelection {
        discord: false,
        youtube: false,
        telegram: true,
        ..Default::default()
    });
    preferences.save(&prefs).unwrap();
    let saved = fs::read_to_string(&prefs).unwrap();
    assert!(saved.find("старая").unwrap() < saved.find("новая").unwrap());
    let val: Value = serde_json::from_str(&saved).unwrap();
    assert_eq!(
        val["Services"],
        json!({"Discord":false,"YouTube":false,"Telegram":true,"UsesZapret":false})
    );
    assert_eq!(val["Future"], 12);
    assert!(!ConvenienceState::load(&prefs).unwrap().connection_sounds);
    assert_eq!(
        ConvenienceState::load(&prefs)
            .unwrap()
            .last_seen_app_version
            .as_deref(),
        Some("0.13.3")
    );
}

#[test]
fn missing_null_and_invalid_are_distinguished_without_writing() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let state = AppState::load(&path).unwrap();
    assert!(state.auto_update);
    assert_eq!(state.voice_observed_at, FIRST_TIMESTAMP);
    assert!(!path.exists());
    fs::write(&path, "null").unwrap();
    assert_eq!(
        AppState::load(&path).err().unwrap().kind(),
        ErrorKind::InvalidData
    );
    assert!(ConvenienceState::load(&path).unwrap().quiet_notifications);
    fs::write(&path, r#"{"NetworkStrategies":null}"#).unwrap();
    assert!(
        ConvenienceState::load(&path)
            .unwrap()
            .network_strategies
            .is_empty()
    );
    fs::write(&path, "{broken").unwrap();
    assert!(ConvenienceState::load(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"{broken");
}

#[test]
fn read_only_getter_is_recomputed_and_extra_service_fields_survive() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("preferences.json");
    fs::write(&path,r#"{"Services":{"Discord":false,"YouTube":false,"Telegram":true,"UsesZapret":true,"Future":9}}"#).unwrap();
    let p = ConvenienceState::load(&path).unwrap();
    p.save(&path).unwrap();
    let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(value["Services"]["UsesZapret"], false);
    assert_eq!(value["Services"]["Future"], 9);
}
