use super::{Restart, connect, reset, runner::WindowsRunner, running};
use eframe::egui;
use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    thread,
    time::{Duration, Instant},
};
use svyaz_connection::{Outcome, ProbeResult, Runner as _};
use svyaz_settings::{
    AppState, ConvenienceState, GameFilterSettings, PREFERENCES_FILE, SETTINGS_FILE,
    ServiceSelection,
};
use svyaz_update::{InstalledApp, State, Updater};
pub fn log(data: &Path, text: &str) {
    let path = data.join("app.log");
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(
            file,
            "{} {text}",
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
        );
    }
}
pub enum Command {
    Initialize(Vec<String>),
    Connect {
        full: bool,
        manual: Option<String>,
        voice: String,
    },
    Stop,
    Telegram,
    ProbeTelegram,
    SaveTelegram {
        port: i32,
        start_with_app: bool,
    },
    SaveServices(ServiceSelection, bool),
    SaveGame(GameFilterSettings),
    AutoUpdate(bool),
    Autostart(bool),
    MarkReleaseNotesSeen,
    CheckApp,
    CheckPackage,
    Rollback,
    Apply,
    InspectReset,
    Reset(Vec<(String, String)>),
    Quit,
}
pub enum Event {
    State(Box<Snapshot>),
    Status(String),
    Checks(String, Outcome),
    Telegram(String),
    Error(String),
    ResetConfirm(Vec<(String, String)>),
    Notice(String),
    Finished,
    Exit(Option<Restart>),
}
pub struct Snapshot {
    pub state: AppState,
    pub prefs: ConvenienceState,
    pub discord: bool,
    pub verified: bool,
    pub telegram: bool,
    pub telegram_wanted: bool,
    pub autostart: bool,
    pub pending: Option<InstalledApp>,
    pub previous: Option<InstalledApp>,
    pub strategies: Vec<String>,
    pub uptime: Option<Duration>,
}
pub struct Worker {
    tx: Sender<(Command, Arc<AtomicBool>)>,
    pub events: Receiver<Event>,
    pending_commands: Vec<Weak<AtomicBool>>,
    background: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}
impl Worker {
    pub fn spawn(install: PathBuf, data: PathBuf, ctx: egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        let (events, out) = mpsc::channel();
        let background = Arc::new(AtomicBool::new(false));
        let tick_cancel = background.clone();
        let handle =
            thread::spawn(move || Engine::new(install, data, events, ctx).run(rx, tick_cancel));
        Self {
            tx,
            events: out,
            background,
            pending_commands: Vec::new(),
            handle: Some(handle),
        }
    }
    pub fn send(&mut self, cmd: Command) {
        self.background.store(true, Ordering::Release);
        self.pending_commands.retain(|weak| weak.strong_count() > 0);
        if matches!(cmd, Command::Stop | Command::Quit) {
            for token in self.pending_commands.iter().filter_map(Weak::upgrade) {
                token.store(true, Ordering::Release);
            }
        }
        let token = Arc::new(AtomicBool::new(false));
        self.pending_commands.push(Arc::downgrade(&token));
        let _ = self.tx.send((cmd, token));
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.send(Command::Quit);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}
struct Engine {
    install: PathBuf,
    data: PathBuf,
    tx: Sender<Event>,
    ctx: egui::Context,
    discord: Option<WindowsRunner>,
    tg: svyaz_telegram::Runner,
    started: Option<Instant>,
    next_updates: Instant,
    previous: Option<InstalledApp>,
    pending: Option<InstalledApp>,
    autostart: bool,
    last_result: Option<ProbeResult>,
    network: Option<String>,
    recovery: super::recovery::Recovery,
    tg_recovery: super::recovery::Recovery,
    last_tick: Instant,
}
impl Engine {
    fn new(install: PathBuf, data: PathBuf, tx: Sender<Event>, ctx: egui::Context) -> Self {
        Self {
            install,
            data,
            tx,
            ctx,
            discord: None,
            tg: Default::default(),
            started: None,
            next_updates: Instant::now(),
            previous: None,
            pending: None,
            autostart: false,
            last_result: None,
            network: None,
            recovery: Default::default(),
            tg_recovery: Default::default(),
            last_tick: Instant::now(),
        }
    }
    fn send(&self, e: Event) {
        let _ = self.tx.send(e);
        self.ctx.request_repaint();
    }
    fn status(&self, s: impl Into<String>) {
        let s = s.into();
        log(&self.data, &s);
        self.send(Event::Status(s));
    }
    fn state(&self) -> io::Result<AppState> {
        AppState::load(&self.data.join(SETTINGS_FILE))
    }
    fn prefs(&self) -> io::Result<ConvenienceState> {
        ConvenienceState::load(&self.data.join(PREFERENCES_FILE))
    }
    fn updater(&self) -> Updater {
        Updater::new(&self.install, &self.data, running())
    }
    fn snapshot(&self) -> io::Result<()> {
        let state = self.state()?;
        let prefs = self.prefs()?;
        let strategies = state
            .pending_package
            .as_ref()
            .or(state.active_package.as_ref())
            .and_then(|id| svyaz_packages::open(&self.data.join("Packages"), id).ok())
            .and_then(|p| svyaz_packages::strategy::load(&p.root, |_| {}).ok())
            .unwrap_or_default()
            .into_iter()
            .map(|s| s.name)
            .collect();
        self.send(Event::State(Box::new(Snapshot {
            state,
            prefs,
            autostart: self.autostart,
            discord: self.discord.as_ref().is_some_and(|r| r.is_running()),
            verified: self.discord.as_ref().is_some_and(|r| r.is_running())
                && self.last_result.as_ref().is_some_and(ProbeResult::success),
            telegram: self.tg.status()?.alive,
            telegram_wanted: self.tg_recovery.wanted,
            previous: self.previous.clone(),
            pending: self.pending.clone(),
            strategies,
            uptime: self.started.map(|s| s.elapsed()),
        })));
        Ok(())
    }
    fn app_updates(&mut self, force: bool, cancel: &AtomicBool) -> io::Result<()> {
        let next = self.updater().prepare(force, &|| guard(cancel))?;
        if let Some(p) = &next {
            if self.pending.as_ref() != Some(p) {
                self.send(Event::Notice(format!(
                    "Версия {} готова к установке",
                    p.version
                )));
            }
        }
        self.pending = next;
        if force {
            self.status(if self.pending.is_some() {
                "Обновление загружено."
            } else {
                "Установлена актуальная версия."
            });
        }
        Ok(())
    }
    fn packages(&self, force: bool, cancel: &AtomicBool) -> io::Result<()> {
        let mut s = self.state()?;
        let dir = self.data.join("Packages");
        let active = s
            .active_package
            .as_deref()
            .map(|id| svyaz_packages::open(&dir, id))
            .transpose()?;
        let pending = s
            .pending_package
            .as_deref()
            .map(|id| svyaz_packages::open(&dir, id))
            .transpose()?;
        let new = svyaz_packages::stage_latest(
            &dir,
            active.as_ref(),
            pending
                .as_ref()
                .or(active.as_ref())
                .map(|p| p.version.as_str()),
            &|| guard(cancel),
        )?;
        if let Some(new) = new {
            s.pending_package = Some(new.id);
            self.status(format!(
                "Пакет {} готов к следующему подключению.",
                new.version
            ));
        } else if force {
            self.status("Пакет уже актуален.");
        }
        s.last_update_check = chrono::Utc::now().to_rfc3339();
        s.save(&self.data.join(SETTINGS_FILE))
    }
    fn start_tg(&mut self, cancel: &AtomicBool) -> io::Result<()> {
        let mut s = self.state()?;
        svyaz_telegram::ensure_secret(&mut s.telegram)?;
        s.save(&self.data.join(SETTINGS_FILE))?;
        if !self.tg.status()?.alive {
            self.tg.start(&self.data, &s, cancel)?;
        }
        self.send(Event::Telegram("Локальный прокси запущен.".into()));
        self.tg_recovery.succeeded();
        self.check_tg(cancel)
    }
    fn check_tg(&mut self, cancel: &AtomicBool) -> io::Result<()> {
        let result = self.tg.probe(cancel)?;
        if !self.tg.status()?.alive {
            return Err(io::Error::other("Движок Telegram остановился."));
        }
        self.send(Event::Telegram(result.map_or(
            "Работает локально · сервер не подтвердил проверку".into(),
            |ms| format!("Сервер ответил за {ms} мс"),
        )));
        Ok(())
    }
    fn connect(
        &mut self,
        full: bool,
        manual: Option<&str>,
        voice: &str,
        cancel: &AtomicBool,
    ) -> io::Result<()> {
        // Keep the previously owned process alive until its state is recorded for restoration.
        let old_name = self.state()?.last_strategy;
        let old = self.discord.take();
        let was_running = old.as_ref().is_some_and(|r| r.is_running());
        drop(old);
        self.started = None;
        self.last_result = None;
        let tx = &self.tx;
        let ctx = &self.ctx;
        let data = &self.data;
        let mut last = None;
        let mut manual_retry = false;
        let result = connect::run(&self.data, manual, full, voice, cancel, |event| {
            let event = match event {
                connect::Event::ManualRetryRequired => {
                    manual_retry = true;
                    return;
                }
                connect::Event::Status(s) => {
                    log(data, &s);
                    Event::Status(s)
                }
                connect::Event::Check { strategy, outcome } => {
                    if let Outcome::Checked { result, .. } = &outcome {
                        last = Some(result.clone());
                    }
                    log(data, &format!("{} · {:?}", strategy, outcome));
                    Event::Checks(strategy, outcome)
                }
            };
            let _ = tx.send(event);
            ctx.request_repaint();
        });
        match result {
            Ok((runner, name)) => {
                self.discord = Some(runner);
                self.started = Some(Instant::now());
                self.last_result = last;
                self.recovery.succeeded();
                self.status(format!("{name} · проверки пройдены"));
                Ok(())
            }
            Err(e) => {
                if manual_retry {
                    self.recovery.stop();
                } else {
                    self.recovery.failed(Instant::now());
                }
                if was_running && !cancel.load(Ordering::Acquire) {
                    if let (Some(name), Some(id)) = (old_name, self.state()?.active_package) {
                        let restore = || -> io::Result<WindowsRunner> {
                            let p = svyaz_packages::open(&self.data.join("Packages"), &id)?;
                            let strategies =
                                svyaz_packages::strategy::load(&p.root, |s| log(&self.data, &s))?;
                            let mut runner = WindowsRunner::new(
                                p,
                                strategies,
                                self.state()?.game_filter,
                                self.data.join("GameFilter/ipset-all.txt"),
                                self.data.join("app.log"),
                            );
                            runner
                                .start(&name, cancel)
                                .map_err(|e| io::Error::other(format!("{e:?}")))?;
                            Ok(runner)
                        };
                        if let Ok(r) = restore() {
                            self.discord = Some(r);
                            self.started = Some(Instant::now());
                            self.status(
                                "Запущен предыдущий альт. Доступность пока не подтверждена.",
                            );
                        }
                    }
                }
                Err(io::Error::other(e))
            }
        }
    }
    fn run(mut self, rx: Receiver<(Command, Arc<AtomicBool>)>, background: Arc<AtomicBool>) {
        loop {
            background.store(false, Ordering::Release);
            let (cmd, cancel) = match rx.recv_timeout(Duration::from_secs(15)) {
                Ok(x) => x,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Err(e) = self.tick(&background) {
                        log(&self.data, &e.to_string());
                    }
                    continue;
                }
            };
            match self.handle(cmd, &cancel) {
                Ok(Some(restart)) => {
                    self.discord.take();
                    let _ = self.tg.stop();
                    self.send(Event::Exit(restart));
                    break;
                }
                Err(e) => {
                    log(&self.data, &e.to_string());
                    self.send(Event::Error(e.to_string()));
                }
                _ => {}
            }
            if let Err(e) = self.snapshot() {
                self.send(Event::Error(e.to_string()));
            }
            self.send(Event::Finished);
        }
    }
    fn tick(&mut self, cancel: &AtomicBool) -> io::Result<()> {
        let now = Instant::now();
        let net = svyaz_connection::network_identity();
        let resumed = now.duration_since(self.last_tick) > Duration::from_secs(60);
        self.last_tick = now;
        let online = super::runner::network_available();
        let tg_alive = self.tg.status()?.alive;
        if net != self.network || resumed {
            self.network = net;
            self.last_result = None;
            if self.discord.is_some() {
                self.status("Сеть изменилась. Проверим при следующем подключении.");
            } else {
                self.recovery.network_changed(now);
            }
            if !tg_alive {
                self.tg_recovery.network_changed(now);
            } else {
                self.send(Event::Telegram(
                    "Сеть изменилась · локальный прокси работает".into(),
                ));
            }
        }
        if self.discord.as_ref().is_some_and(|r| !r.is_running()) {
            self.discord.take();
            self.started = None;
            self.last_result = None;
            self.recovery.failed(now);
            self.status("Процесс остановился. Ожидаем повторного подключения.");
        }
        if !tg_alive && self.tg_recovery.wanted {
            self.tg_recovery.failed(now);
        }
        let quiet = self.prefs()?.quiet_notifications;
        if self.recovery.notify(now, quiet) {
            self.send(Event::Notice(
                "Соединение не восстановлено. Откройте журнал «Связи».".into(),
            ));
        }
        if self.tg_recovery.notify(now, quiet) {
            self.send(Event::Notice("Прокси Telegram не восстановлен.".into()));
        }
        if self.discord.is_none() && self.recovery.begin(now, online) {
            if let Err(e) = self.connect(false, None, "", cancel) {
                log(&self.data, &e.to_string());
            }
        }
        if !tg_alive && self.tg_recovery.begin(now, online) {
            if let Err(e) = self.start_tg(cancel) {
                log(&self.data, &e.to_string());
            }
        }
        self.snapshot()?;
        if now >= self.next_updates && self.state()?.auto_update {
            self.next_updates = now + Duration::from_secs(6 * 3600);
            for result in [
                self.app_updates(false, &cancel),
                self.packages(false, &cancel),
            ] {
                if let Err(e) = result {
                    log(&self.data, &e.to_string());
                }
            }
            self.snapshot()?;
        }
        Ok(())
    }
    fn handle(&mut self, cmd: Command, cancel: &AtomicBool) -> io::Result<Option<Option<Restart>>> {
        if matches!(cmd, Command::Quit) {
            return Ok(Some(None));
        }
        if matches!(cmd, Command::Stop) {
            self.recovery.stop();
            self.tg_recovery.stop();
            self.discord.take();
            self.started = None;
            self.last_result = None;
            self.tg.stop()?;
            self.status("Отключено");
            return Ok(None);
        }
        if cancel.load(Ordering::Acquire)
            && matches!(
                cmd,
                Command::Initialize(_)
                    | Command::Connect { .. }
                    | Command::Telegram
                    | Command::ProbeTelegram
                    | Command::Reset(_)
                    | Command::CheckPackage
                    | Command::CheckApp
                    | Command::Apply
                    | Command::Rollback
            )
        {
            return Ok(None);
        }
        let mut s = self.state()?;
        let mut prefs = self.prefs()?;
        let selected = prefs.effective_services(&s);
        match cmd {
            Command::Quit => return Ok(Some(None)),
            Command::Initialize(args) => {
                self.network = svyaz_connection::network_identity();
                let updater = self.updater();
                self.pending = updater
                    .read(State::Pending)?
                    .filter(|p| p.version > running());
                if let Some(p) = &self.pending {
                    self.send(Event::Notice(format!(
                        "Версия {} готова к установке",
                        p.version
                    )));
                }
                self.previous = updater.previous_version()?;
                if !s.autostart_configured
                    && svyaz_windows::set_autostart(true, &self.install, &std::env::current_exe()?)
                        .is_ok()
                {
                    s.autostart_configured = true;
                    s.save(&self.data.join(SETTINGS_FILE))?;
                }
                self.autostart = svyaz_windows::autostart_enabled().unwrap_or(false);
                if self.autostart {
                    if let Err(e) =
                        svyaz_windows::set_autostart(true, &self.install, &std::env::current_exe()?)
                    {
                        log(&self.data, &e.to_string());
                    }
                }
                self.snapshot()?;
                let has = |key: &str| args.iter().any(|s| s == key);
                if s.telegram.start_with_app
                    || has("--resume-telegram")
                    || has("--startup") && selected.telegram
                {
                    self.tg_recovery.enable();
                    if let Err(e) = self.start_tg(cancel) {
                        self.send(Event::Telegram(e.to_string()));
                        self.tg_recovery.failed(Instant::now());
                    }
                }
                if has("--resume-discord") || has("--startup") && selected.uses_zapret() {
                    self.recovery.enable();
                    self.connect(false, None, "", cancel)?;
                }
            }
            Command::Connect {
                full,
                manual,
                voice,
            } => {
                selected.validate()?;
                let tg = if selected.telegram {
                    self.tg_recovery.enable();
                    self.start_tg(cancel)
                } else {
                    Ok(())
                };
                if selected.uses_zapret() {
                    self.recovery.enable();

                    self.connect(full, manual.as_deref(), &voice, cancel)?;
                }
                tg?;
            }
            Command::Stop => unreachable!(),
            Command::Telegram => {
                if self.tg.status()?.alive || self.tg_recovery.wanted {
                    self.tg_recovery.stop();
                    self.tg.stop()?;
                    self.send(Event::Telegram("Отключено".into()));
                } else {
                    self.tg_recovery.enable();

                    self.start_tg(cancel)?;
                }
            }
            Command::ProbeTelegram => self.check_tg(cancel)?,
            Command::SaveTelegram {
                port,
                start_with_app,
            } => {
                if self.tg.status()?.alive {
                    return Err(io::Error::other("Сначала остановите прокси."));
                }
                s.telegram.port = port;
                s.telegram.start_with_app = start_with_app;
                svyaz_telegram::ensure_secret(&mut s.telegram)?;
                s.save(&self.data.join(SETTINGS_FILE))?;
                self.status("Порт сохранён.");
            }
            Command::SaveServices(services, quiet) => {
                services.validate()?;
                let current = prefs.services.get_or_insert_with(Default::default);
                current.discord = services.discord;
                current.youtube = services.youtube;
                current.telegram = services.telegram;
                prefs.quiet_notifications = quiet;
                prefs.save(&self.data.join(PREFERENCES_FILE))?;
                self.status("Выбор сервисов сохранён.");
            }
            Command::SaveGame(filter) => {
                if self.discord.is_some() {
                    return Err(io::Error::other("Сначала отключите связь."));
                }
                filter.ranges()?;
                s.game_filter.mode = filter.mode;
                s.game_filter.tcp_ports = filter.tcp_ports;
                s.game_filter.udp_ports = filter.udp_ports;
                s.save(&self.data.join(SETTINGS_FILE))?;
                self.status("Game Filter сохранён.");
            }
            Command::Autostart(enable) => {
                svyaz_windows::set_autostart(enable, &self.install, &std::env::current_exe()?)?;
                self.autostart = enable;
                s.autostart_configured = true;
                s.save(&self.data.join(SETTINGS_FILE))?;
            }
            Command::AutoUpdate(enable) => {
                s.auto_update = enable;
                s.save(&self.data.join(SETTINGS_FILE))?;
                self.next_updates = Instant::now();
            }
            Command::MarkReleaseNotesSeen => {
                prefs.last_seen_app_version = Some(running().to_string());
                prefs.save(&self.data.join(PREFERENCES_FILE))?;
            }
            Command::CheckApp => self.app_updates(true, cancel)?,
            Command::CheckPackage => self.packages(true, cancel)?,
            Command::Apply | Command::Rollback => {
                let rollback = matches!(cmd, Command::Rollback);
                let app = if rollback {
                    self.updater().previous_version()?
                } else {
                    self.updater().read(State::Pending)?
                }
                .ok_or_else(|| io::Error::other("Нет доступной версии."))?;
                self.updater().executable(&app)?;
                if rollback {
                    s.auto_update = false;
                    s.save(&self.data.join(SETTINGS_FILE))?;
                }
                let mut args = vec![];
                if self.discord.as_ref().is_some_and(|r| r.is_running()) {
                    args.push("--resume-discord".into());
                }
                if self.tg.status()?.alive {
                    args.push("--resume-telegram".into());
                }
                return Ok(Some(Some(Restart {
                    app,
                    rollback,
                    args,
                })));
            }
            Command::InspectReset => {
                let candidates = reset::inspect(&self.data)?;
                self.send(Event::ResetConfirm(candidates));
            }
            Command::Reset(approved) => {
                self.discord.take();
                self.started = None;
                self.last_result = None;
                reset::reset(&self.data, &approved, cancel)?;
                self.packages(true, cancel)?;
                if selected.uses_zapret() {
                    self.connect(true, None, "", cancel)?;
                }
            }
        }
        Ok(None)
    }
}
fn guard(c: &AtomicBool) -> io::Result<()> {
    if c.load(Ordering::Acquire) {
        Err(io::Error::new(io::ErrorKind::Interrupted, "Отменено"))
    } else {
        Ok(())
    }
}
