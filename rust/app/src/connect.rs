use super::{recovery::different_networks, runner::WindowsRunner};
use std::{
    env, io,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
use svyaz_connection::{
    Mode, Outcome, Runner, SelectionError, bootstrap_voice, network_identity, probe, resolve_voice,
    select,
};
use svyaz_packages::{open, prepare_lists, stage_latest, strategy};
use svyaz_settings::{AppState, ConvenienceState, PREFERENCES_FILE, SETTINGS_FILE};

pub enum Event {
    ManualRetryRequired,
    Status(String),
    Check { strategy: String, outcome: Outcome },
}

fn detail(error: SelectionError, report: &mut impl FnMut(Event)) -> String {
    match error {
        SelectionError::Runner(text) => text,
        SelectionError::RequiresManualRetry(text) => {
            report(Event::ManualRetryRequired);
            text
        }
        SelectionError::Cancelled => "Подключение отменено.".into(),
    }
}
fn check_cancel(cancel: &AtomicBool) -> io::Result<()> {
    if cancel.load(Ordering::Acquire) {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Подключение отменено.",
        ))
    } else {
        Ok(())
    }
}
fn message(error: io::Error) -> String {
    error.to_string()
}

/// Run on a dedicated worker; the caller must own/store the returned runner until disconnect.
pub fn run(
    data: &Path,
    manual_strategy: Option<&str>,
    full: bool,
    voice_address: &str,
    cancel: &AtomicBool,
    mut progress: impl FnMut(Event),
) -> Result<(WindowsRunner, String, Option<String>), String> {
    let settings_file = data.join(SETTINGS_FILE);
    let prefs_file = data.join(PREFERENCES_FILE);
    let package_dir = data.join("Packages");
    let mut state = AppState::load(&settings_file).map_err(message)?;
    if state.zapret_reset_pending {
        progress(Event::Status("Завершаем сброс кэша zapret…".into()));
        super::reset::reset(data, &[], cancel).map_err(message)?;
        state = AppState::load(&settings_file).map_err(message)?;
    }
    let mut prefs = ConvenienceState::load(&prefs_file).map_err(message)?;
    let services = prefs.effective_services(&state);
    services.validate().map_err(message)?;
    if !services.uses_zapret() {
        return Err("Выберите Discord или YouTube для подбора альта.".into());
    }
    check_cancel(cancel).map_err(message)?;
    if !super::runner::network_available() {
        return Err("Нет сетевого подключения. Подключите Wi-Fi или сетевой кабель.".into());
    }
    let network = network_identity();
    let preferred = network
        .as_ref()
        .and_then(|id| prefs.network_strategies.get(id))
        .or(state.last_strategy.as_ref())
        .cloned();
    let mode = Mode::for_connection(
        state.last_strategy.as_deref(),
        manual_strategy.is_some(),
        full,
    );
    let active = state
        .active_package
        .as_deref()
        .map(|id| open(&package_dir, id))
        .transpose()
        .map_err(message)?;
    let mut pending = state
        .pending_package
        .as_deref()
        .map(|id| open(&package_dir, id))
        .transpose()
        .map_err(message)?;
    if active.is_none() && pending.is_none() {
        progress(Event::Status("Загрузка пакета zapret…".into()));
        pending =
            stage_latest(&package_dir, None, None, &|| check_cancel(cancel)).map_err(message)?;
        let staged = pending
            .as_ref()
            .ok_or("Не удалось получить пакет zapret.")?;
        state.pending_package = Some(staged.id.clone());
        state.save(&settings_file).map_err(message)?;
    }
    if let (Some(p), Some(a)) = (&pending, &active) {
        if p.id != a.id {
            prepare_lists(&p.root, Some(&a.root)).map_err(message)?;
        }
    }
    let ipset = data.join("GameFilter/ipset-all.txt");
    if state.game_filter.mode != 0 {
        svyaz_packages::game::ensure(&ipset, cancel).map_err(message)?;
    }
    let roaming = env::var_os("APPDATA").map(std::path::PathBuf::from);
    let roaming = if services.discord {
        Some(roaming.ok_or("Не найдена папка профиля Windows.")?)
    } else {
        None
    };
    let mut endpoint = if let Some(path) = &roaming {
        resolve_voice(voice_address, path, &mut state)?
    } else {
        None
    };
    for pkg in pending.iter().chain(
        active
            .iter()
            .filter(|a| pending.as_ref().is_none_or(|p| p.id != a.id)),
    ) {
        check_cancel(cancel).map_err(message)?;
        progress(Event::Status(format!("Проверка пакета {}", pkg.version)));
        let loaded = strategy::load(&pkg.root, |s| progress(Event::Status(s))).map_err(message)?;
        let all_names: Vec<_> = loaded.iter().map(|s| s.name.clone()).collect();
        let mut runner = WindowsRunner::new(
            pkg.clone(),
            loaded,
            state.game_filter.clone(),
            ipset.clone(),
            data.join("app.log"),
        );
        if let Some(path) = roaming.as_ref().filter(|_| endpoint.is_none()) {
            progress(Event::Status(
                "Откройте голосовой канал Discord: ждём адрес сервера…".into(),
            ));
            endpoint = Some(
                bootstrap_voice(
                    &mut runner,
                    &all_names,
                    preferred.as_deref(),
                    cancel,
                    || resolve_voice(voice_address, path, &mut state).map_err(str::to_owned),
                    |s| progress(Event::Status(s)),
                )
                .map_err(|e| detail(e, &mut progress))?,
            );
        }
        let names: Vec<_> = all_names
            .into_iter()
            .filter(|name| mode != Mode::FirstPassing || manual_strategy == Some(name.as_str()))
            .collect();
        if names.is_empty() {
            continue;
        }
        let winner = select(
            &mut runner,
            &names,
            preferred.as_deref(),
            mode,
            cancel,
            || {
                if let Some(path) = &roaming {
                    if let Ok(Some(found)) = resolve_voice(voice_address, path, &mut state) {
                        endpoint = Some(found);
                    }
                }
                probe(
                    services.discord,
                    services.youtube,
                    endpoint.as_ref(),
                    cancel,
                )
            },
            |strategy, outcome| {
                progress(Event::Check {
                    strategy: strategy.into(),
                    outcome,
                })
            },
        )
        .map_err(|e| detail(e, &mut progress))?;
        let Some(winner) = winner else {
            continue;
        };
        check_cancel(cancel).map_err(message)?;
        let current = network_identity();
        if different_networks(network.as_deref(), current.as_deref()) {
            runner.stop()?;
            return Err("Сеть изменилась во время проверки. Переподключитесь, чтобы проверить новый маршрут.".into());
        }
        if state.active_package.as_ref() != Some(&pkg.id) {
            state.previous_package = state.active_package.clone();
        }
        state.active_package = Some(pkg.id.clone());
        state.pending_package = None;
        state.last_strategy = Some(winner.clone());
        state.save(&settings_file).map_err(message)?;
        if prefs.remember_network(network.as_deref(), current.as_deref(), &winner) {
            if let Err(error) = prefs.save(&prefs_file) {
                progress(Event::Status(format!(
                    "Не удалось сохранить профиль сети: {error}"
                )));
            }
        }
        if pending
            .as_ref()
            .is_some_and(|candidate| candidate.id != pkg.id)
        {
            progress(Event::Status(
                "Новый пакет не прошёл проверку. Используется предыдущий.".into(),
            ));
        }
        return Ok((runner, winner, current.or(network)));
    }
    check_cancel(cancel).map_err(message)?;
    if !super::runner::network_available() {
        return Err("Нет сетевого подключения. Ожидаем подключения.".into());
    }
    if different_networks(network.as_deref(), network_identity().as_deref()) {
        return Err("Сеть изменилась во время подбора. Ожидаем подключения.".into());
    }
    progress(Event::ManualRetryRequired);
    Err("Подбор завершён. Ни один альт не прошёл все проверки и подтверждение. Можно повторить подбор вручную.".into())
}
