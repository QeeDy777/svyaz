use super::{
    Restart, running,
    tray::{Tray, TrayAction},
    worker::{Command, Event, Snapshot, Worker},
};
use eframe::egui::{self, Color32, Frame, Margin, RichText, Stroke, Ui, Vec2, ViewportCommand};
use std::{
    collections::VecDeque,
    fs, io,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use svyaz_connection::{CheckResult, Outcome, ProbeResult};
use svyaz_settings::{GameFilterSettings, ServiceSelection};
use svyaz_update::{State, Updater};

#[path = "desktop_panes.rs"]
mod panes;

const BG: Color32 = Color32::from_rgb(18, 20, 22);
const CARD: Color32 = Color32::from_rgb(29, 32, 34);
const HERO: Color32 = Color32::from_rgb(41, 36, 32);
const LINE: Color32 = Color32::from_rgb(56, 60, 62);
const INK: Color32 = Color32::from_rgb(244, 244, 240);
const MUTED: Color32 = Color32::from_rgb(174, 180, 179);
const AMBER: Color32 = Color32::from_rgb(255, 183, 112);
const GREEN: Color32 = Color32::from_rgb(166, 222, 192);
const RED: Color32 = Color32::from_rgb(255, 137, 124);

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum Tab {
    Connection,
    Telegram,
    Settings,
    Journal,
}

fn brand() -> io::Result<egui::IconData> {
    let image = image::load_from_memory(include_bytes!(
        "../../../src/DiscordRoute.App/Assets/icon-source.png"
    ))
    .map_err(io::Error::other)?
    .to_rgba8();
    let (width, height) = image.dimensions();
    Ok(egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    })
}
fn configure(ctx: &egui::Context) {
    let mut s = (*ctx.style()).clone();
    s.visuals = egui::Visuals::dark();
    s.visuals.override_text_color = Some(INK);
    s.visuals.window_fill = CARD;
    s.visuals.panel_fill = BG;
    s.visuals.extreme_bg_color = BG;
    s.visuals.widgets.inactive.bg_fill = CARD;
    s.visuals.widgets.inactive.weak_bg_fill = CARD;
    s.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0_f32, LINE);
    s.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0_f32, LINE);
    s.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0_f32, AMBER);
    s.visuals.selection.bg_fill = Color32::from_rgb(103, 73, 47);
    s.visuals.selection.stroke = Stroke::new(1.0_f32, AMBER);
    s.visuals.hyperlink_color = AMBER;
    s.spacing.item_spacing = Vec2::new(8.0, 8.0);
    s.spacing.button_padding = Vec2::new(12.0, 7.0);
    s.spacing.scroll.bar_inner_margin = 12.0;
    s.text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
    s.text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
    ctx.set_style(s);
}
fn card() -> Frame {
    Frame::default()
        .fill(CARD)
        .stroke(Stroke::new(1.0_f32, LINE))
        .corner_radius(12)
        .inner_margin(12)
}
fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(24.0).strong());
}
fn hint(ui: &mut Ui, text: impl Into<String>) {
    ui.label(RichText::new(text).size(13.0).color(MUTED));
}
fn action(ui: &mut Ui, text: &str, primary: bool) -> bool {
    let mut button = egui::Button::new(
        RichText::new(text)
            .color(if primary { BG } else { INK })
            .strong(),
    )
    .corner_radius(8);
    if primary {
        button = button.fill(AMBER).stroke(Stroke::NONE);
    }
    ui.add_sized([ui.available_width(), 38.0], button).clicked()
}

pub(super) fn show(
    install: PathBuf,
    data: PathBuf,
    args: Vec<String>,
    token: Option<String>,
    legacy_parent: bool,
) -> io::Result<Option<Restart>> {
    let restart = Arc::new(Mutex::new(None));
    let written = restart.clone();
    let icon = brand()?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Связь")
            .with_inner_size([440., 680.])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_icon(icon.clone()),
        persist_window: false,
        ..Default::default()
    };
    eframe::run_native(
        "Связь",
        options,
        Box::new(move |cc| {
            let ctx = &cc.egui_ctx;
            configure(ctx);
            let texture = ctx.load_texture(
                "ASF Control",
                egui::ColorImage::from_rgba_unmultiplied(
                    [icon.width as usize, icon.height as usize],
                    &icon.rgba,
                ),
                egui::TextureOptions::LINEAR,
            );
            let tray = Tray::new(ctx)?;
            let worker = Worker::spawn(install.clone(), data.clone(), ctx.clone());
            Ok(Box::new(Desktop {
                worker,
                tray,
                logo: texture,
                install,
                data,
                tab: Tab::Connection,
                startup: Some(args),
                token,
                legacy_parent,
                rendered: false,
                visible: true,
                exiting: false,
                restart: written,
                inflight: 0,
                snap: None,
                status: String::new(),
                error: String::new(),
                tg_status: String::new(),
                proxy_url: None,
                report: None,
                history: VecDeque::new(),
                scan_count: 0,
                last_verified: None,
                reset_confirm: None,
                port: "1443".into(),
                tg_start: false,
                services: Default::default(),
                quiet: false,
                game: Default::default(),
                manual: None,
                voice: String::new(),
                journal: String::new(),
                journal_meta: None,
                notes: false,
            }))
        }),
    )
    .map_err(|e| io::Error::other(e.to_string()))?;
    let answer = restart
        .lock()
        .map_err(|_| io::Error::other("Ошибка завершения обновления."))?
        .take();
    Ok(answer)
}

struct Desktop {
    worker: Worker,
    tray: Tray,
    logo: egui::TextureHandle,
    install: PathBuf,
    data: PathBuf,
    tab: Tab,
    startup: Option<Vec<String>>,
    token: Option<String>,
    legacy_parent: bool,
    rendered: bool,
    visible: bool,
    exiting: bool,
    restart: Arc<Mutex<Option<Restart>>>,
    inflight: usize,
    snap: Option<Box<Snapshot>>,
    status: String,
    error: String,
    tg_status: String,
    proxy_url: Option<String>,
    report: Option<ProbeResult>,
    history: VecDeque<(String, String)>,
    scan_count: usize,
    last_verified: Option<String>,
    reset_confirm: Option<Vec<(String, String)>>,
    port: String,
    tg_start: bool,
    services: ServiceSelection,
    quiet: bool,
    game: GameFilterSettings,
    manual: Option<String>,
    voice: String,
    journal: String,
    journal_meta: Option<(u64, Option<std::time::SystemTime>)>,
    notes: bool,
}
impl Desktop {
    fn send(&mut self, cmd: Command) {
        self.inflight += 1;
        self.error.clear();
        self.worker.send(cmd);
    }
    fn busy(&self) -> bool {
        self.inflight != 0
    }
    fn connected(&self) -> bool {
        self.snap.as_ref().is_some_and(|s| s.discord || s.telegram)
    }
    fn connect(&mut self, full: bool) {
        if self.busy() {
            return;
        }
        self.report = None;
        self.last_verified = None;
        self.scan_count = 0;
        self.history.clear();
        self.status = "Подключаем выбранные сервисы…".into();
        self.send(Command::Connect {
            full,
            manual: self.manual.clone(),
            voice: self.voice.clone(),
        });
    }
    fn record(&mut self, name: String, event: Outcome) {
        let detail = match event {
            Outcome::Starting {
                position,
                total,
                confirmation,
            } => {
                let s = if confirmation {
                    "Подтверждение".into()
                } else {
                    self.scan_count += 1;
                    format!("Подбор {position}/{total}")
                };
                self.status = format!("{s} · {name}");
                return;
            }
            Outcome::StartFailed(reason) => format!("Ошибка запуска · {reason}"),
            Outcome::Checked {
                pass,
                passes,
                result,
                confirmed,
            } => {
                let text = if !result.success() {
                    "Не прошла".into()
                } else if let Some(q) = confirmed {
                    format!("Выбрана · {} мс", q.median_ms)
                } else {
                    format!("Проверка {pass}/{passes}")
                };
                self.report = Some(result);
                text
            }
        };
        self.history.push_back((name, detail));
        while self.history.len() > 256 {
            self.history.pop_front();
        }
    }
    fn show_window(&mut self, ctx: &egui::Context) {
        self.visible = true;
        ctx.send_viewport_cmd(ViewportCommand::Visible(true));
        ctx.send_viewport_cmd(ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(ViewportCommand::Focus);
    }
    fn hide(&mut self, ctx: &egui::Context) {
        self.visible = false;
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
    }
    fn on_tray(&mut self, ctx: &egui::Context) {
        while let Some(action) = self.tray.try_next() {
            match action {
                TrayAction::Open => self.show_window(ctx),
                TrayAction::Connect | TrayAction::Repair if !self.busy() => self.connect(false),
                TrayAction::Update if !self.busy() => self.send(Command::CheckApp),
                TrayAction::Disconnect => self.send(Command::Stop),
                TrayAction::ToggleTelegram if !self.busy() => self.send(Command::Telegram),
                TrayAction::Quit => {
                    self.send(Command::Quit);
                }
                _ => (),
            }
        }
    }
    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.worker.events.try_recv() {
            match event {
                Event::Status(s) => self.status = s,
                Event::Error(e) => {
                    self.error = e;
                }
                Event::Checks(name, outcome) => self.record(name, outcome),
                Event::Telegram(s) => self.tg_status = s,
                Event::ResetConfirm(approved) => {
                    self.tab = Tab::Settings;
                    self.reset_confirm = Some(approved);
                    self.show_window(ctx);
                }
                Event::Notice(s) => {
                    if !self.visible {
                        self.tray.notify("Связь", &s);
                    }
                    self.status = s;
                }
                Event::Finished => self.inflight = self.inflight.saturating_sub(1),
                Event::State(next) => {
                    self.proxy_url = if next.telegram {
                        svyaz_telegram::proxy_url_at(&self.data, &next.state).ok()
                    } else {
                        None
                    };
                    if self.snap.is_none() {
                        self.services = next.prefs.effective_services(&next.state);
                        self.quiet = next.prefs.quiet_notifications;
                        self.game = next.state.game_filter.clone();
                        self.port = next.state.telegram.port.to_string();
                        self.tg_start = next.state.telegram.start_with_app;
                    }
                    if next.verified && !self.snap.as_ref().is_some_and(|s| s.verified) {
                        self.last_verified = Some(chrono::Local::now().to_rfc3339());
                    }
                    if !next.discord && !self.busy() {
                        self.report = None;
                    }
                    self.tray.status(
                        next.state.last_strategy.as_deref().unwrap_or(""),
                        next.verified,
                        next.telegram,
                    );
                    self.snap = Some(next);
                }
                Event::Exit(restart) => {
                    if let Ok(mut target) = self.restart.lock() {
                        *target = restart;
                    }
                    self.exiting = true;
                    ctx.send_viewport_cmd(ViewportCommand::Close);
                }
            }
        }
    }
    fn initialized(&mut self, ctx: &egui::Context) {
        if !self.rendered {
            self.rendered = true;
            ctx.request_repaint();
            return;
        }
        let Some(args) = self.startup.take() else {
            return;
        };
        if let Some(token) = self.token.take() {
            if let Err(error) = svyaz_windows::signal_ready(&token) {
                super::worker::log(&self.data, &error.to_string());
                self.send(Command::Quit);
                return;
            }
            if self.legacy_parent {
                let install = self.install.clone();
                let data = self.data.clone();
                std::thread::spawn(move || {
                    if svyaz_windows::wait_for_legacy_parent(&token).is_err() {
                        return;
                    }
                    let u = Updater::new(&install, &data, running());
                    if let (Ok(Some(app)), Ok(exe)) =
                        (u.read(State::Current), std::env::current_exe())
                    {
                        if u.executable(&app).is_ok_and(|current| current == exe) {
                            svyaz_windows::refresh_shortcuts(&install, &exe);
                        }
                    }
                });
            }
        }
        if args.iter().any(|s| s == "--startup") {
            self.hide(ctx);
        }
        self.send(Command::Initialize(args));
    }
}

impl eframe::App for Desktop {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.initialized(ctx);
        self.receive(ctx);
        self.on_tray(ctx);
        if self.reset_confirm.is_none() {
            for (key, tab) in [
                (egui::Key::Num1, Tab::Connection),
                (egui::Key::Num2, Tab::Telegram),
                (egui::Key::Num3, Tab::Settings),
                (egui::Key::Num4, Tab::Journal),
            ] {
                if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, key)) {
                    self.tab = tab;
                }
            }
        }
        if !self.exiting && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(ViewportCommand::CancelClose);
            self.hide(ctx);
        }
        if self.visible && ctx.input(|i| i.viewport().minimized == Some(true)) {
            self.hide(ctx);
        }
        egui::TopBottomPanel::top("brand")
            .frame(
                Frame::default()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(16, 8)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.image((self.logo.id(), Vec2::splat(30.0)));
                    ui.label(RichText::new("Связь").size(19.).strong());
                    ui.label(RichText::new(running().to_string()).color(MUTED).size(12.));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new(if self.connected() {
                                "● Подключено"
                            } else {
                                "○ Отключено"
                            })
                            .color(if self.connected() { GREEN } else { MUTED })
                            .size(12.),
                        );
                    });
                });
            });
        egui::TopBottomPanel::bottom("navigation")
            .frame(
                Frame::default()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(4, 6)),
            )
            .show(ctx, |ui| {
                let tabs = [
                    (Tab::Connection, "Подключение"),
                    (Tab::Telegram, "Telegram"),
                    (Tab::Settings, "Настройки"),
                    (Tab::Journal, "Журнал"),
                ];
                ui.columns(4, |cols| {
                    for (col, (id, label)) in cols.iter_mut().zip(tabs) {
                        let active = self.tab == id;
                        if col
                            .add_sized(
                                [col.available_width(), 38.],
                                egui::Button::new(
                                    RichText::new(label).size(12.).color(if active {
                                        AMBER
                                    } else {
                                        MUTED
                                    }),
                                )
                                .fill(if active { CARD } else { BG })
                                .stroke(Stroke::NONE),
                            )
                            .clicked()
                        {
                            self.tab = id;
                        }
                    }
                });
            });
        egui::CentralPanel::default()
            .frame(
                Frame::default()
                    .fill(BG)
                    .inner_margin(Margin::symmetric(16, 14)),
            )
            .show(ctx, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt(("page", self.tab as u8))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if !self.error.is_empty() {
                            ui.colored_label(RED, self.error.clone());
                        }
                        self.update_banner(ui);
                        match self.tab {
                            Tab::Connection => self.connection(ui),
                            Tab::Telegram => self.telegram(ui),
                            Tab::Settings => self.settings(ui),
                            Tab::Journal => self.journal(ui),
                        }
                    });
            });
        self.confirm_reset(ctx);
    }
}
