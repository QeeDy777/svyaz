use super::{
    Restart, running,
    tray::{Tray, TrayAction},
    worker::{Command, Event, Snapshot, Worker},
};
use eframe::egui::{self, Color32, Frame, Margin, RichText, Stroke, Ui, Vec2, ViewportCommand};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    collections::VecDeque,
    fs, io,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use svyaz_connection::{CheckResult, Outcome, ProbeResult};
use svyaz_settings::{GameFilterSettings, ServiceSelection};
use svyaz_update::{State, Updater};
use windows_sys::Win32::{
    Foundation::HWND,
    UI::WindowsAndMessaging::{IsWindowVisible, SW_HIDE, ShowWindow},
};

#[path = "desktop_panes.rs"]
mod panes;

const BG: Color32 = Color32::from_rgb(18, 20, 22);
const CARD: Color32 = Color32::from_rgb(28, 31, 34);
const LINE: Color32 = Color32::from_rgb(57, 61, 64);
const INK: Color32 = Color32::from_rgb(243, 243, 239);
const MUTED: Color32 = Color32::from_rgb(170, 176, 175);
const AMBER: Color32 = Color32::from_rgb(255, 183, 112);
const GREEN: Color32 = Color32::from_rgb(166, 222, 192);
const RED: Color32 = Color32::from_rgb(255, 107, 107);

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
    let mut fonts = egui::FontDefinitions::default();
    if let Some(directory) = std::env::var_os("WINDIR") {
        let file = PathBuf::from(directory).join("Fonts").join("segoeui.ttf");
        if let Ok(bytes) = fs::read(file) {
            fonts
                .font_data
                .insert("Segoe UI".into(), egui::FontData::from_owned(bytes).into());
            fonts
                .families
                .entry(egui::FontFamily::Proportional)
                .or_default()
                .insert(0, "Segoe UI".into());
        }
    }
    let family = egui::FontFamily::Name("Interface Semibold".into());
    let mut fallback = fonts.families[&egui::FontFamily::Proportional].clone();
    if let Some(bytes) = std::env::var_os("WINDIR")
        .and_then(|w| fs::read(PathBuf::from(w).join("Fonts/seguisb.ttf")).ok())
    {
        fonts.font_data.insert(
            "Segoe UI Semibold".into(),
            egui::FontData::from_owned(bytes).into(),
        );
        fallback.insert(0, "Segoe UI Semibold".to_owned());
    }
    fonts.families.insert(family, fallback);
    ctx.set_fonts(fonts);
    ctx.set_theme(egui::ThemePreference::Dark);
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
    s.spacing.scroll.floating = false;
    s.spacing.scroll.bar_width = 7.0;
    s.spacing.scroll.bar_inner_margin = 14.0;
    s.spacing.scroll.bar_outer_margin = 0.0;
    s.visuals.widgets.noninteractive.fg_stroke.color = INK;
    s.text_styles
        .insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
    s.text_styles
        .insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
    let s = Arc::new(s);
    ctx.set_style_of(egui::Theme::Dark, s.clone());
    ctx.set_style_of(egui::Theme::Light, s);
}
fn card() -> Frame {
    Frame::default()
        .fill(CARD)
        .stroke(Stroke::new(1.0_f32, LINE))
        .corner_radius(12)
        .inner_margin(12)
}
fn semibold(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("Interface Semibold".into()))
}
fn title(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).font(semibold(28.)).color(INK));
}
fn hint(ui: &mut Ui, text: impl Into<String>) {
    ui.label(RichText::new(text).size(13.).color(MUTED));
}

// The radial stops and the five curves come from the 0.13.3 WPF theme.
fn hero(ui: &mut Ui, waves: bool, content: impl FnOnce(&mut Ui)) {
    hero_inner(ui, waves, Margin::same(17), content)
}
fn hero_inner(ui: &mut Ui, waves: bool, margin: Margin, content: impl FnOnce(&mut Ui)) {
    let texture_id = egui::Id::new("ASF hero gradient");
    let texture = ui
        .ctx()
        .data(|data| data.get_temp::<egui::TextureHandle>(texture_id))
        .unwrap_or_else(|| {
            let n = 96;
            let mut image = egui::ColorImage::new([n, n], Color32::BLACK);
            for y in 0..n {
                for x in 0..n {
                    let dx = (x as f32 / (n - 1) as f32 - 1.) / 0.85;
                    let dy = (y as f32 / (n - 1) as f32 - 1.2) / 1.15;
                    let distance = dx.hypot(dy).min(1.);
                    let (from, to, t) = if distance < 0.36 {
                        ([159., 101., 64.], [81., 59., 44.], distance / 0.36)
                    } else {
                        ([81., 59., 44.], [36., 35., 34.], (distance - 0.36) / 0.64)
                    };
                    let rgb: [u8; 3] =
                        std::array::from_fn(|i| (from[i] + (to[i] - from[i]) * t).round() as u8);
                    image[(x, y)] = Color32::from_rgb(rgb[0], rgb[1], rgb[2]);
                }
            }
            let tex = ui
                .ctx()
                .load_texture("ASF hero", image, egui::TextureOptions::LINEAR);
            ui.ctx()
                .data_mut(|d| d.insert_temp(texture_id, tex.clone()));
            tex
        });
    let background = ui.painter().add(egui::Shape::Noop);
    let rect = Frame::default()
        .inner_margin(margin)
        .show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            content(ui);
        })
        .response
        .rect;
    let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.));
    let mut shapes = vec![
        egui::Shape::Rect(
            egui::epaint::RectShape::filled(rect, 16, Color32::WHITE)
                .with_texture(texture.id(), uv),
        ),
        egui::Shape::rect_stroke(
            rect,
            16,
            (1., Color32::from_rgb(102, 83, 68)),
            egui::StrokeKind::Inside,
        ),
    ];
    if waves {
        let pt = |x: f32, y: f32| {
            egui::pos2(
                rect.right() - 12. - 104. + (x - 20.) * 104. / 117.,
                rect.top() + 10. + (y - 28.) * 55. / 80.,
            )
        };
        for (x, cx, cy, y) in [
            (20., 34., 57., 28.),
            (37., 50., 68., 43.),
            (55., 65., 80., 58.),
            (77., 88., 88., 74.),
            (101., 114., 98., 90.),
        ] {
            shapes.push(egui::Shape::QuadraticBezier(
                egui::epaint::QuadraticBezierShape::from_points_stroke(
                    [pt(x, 108.), pt(cx, cy), pt(137., y)],
                    false,
                    Color32::TRANSPARENT,
                    (1., Color32::from_rgba_unmultiplied(255, 183, 112, 33)),
                ),
            ));
        }
    }
    ui.painter().set(background, egui::Shape::Vec(shapes));
}
fn action(ui: &mut Ui, text: &str, primary: bool) -> bool {
    let power = matches!(
        text,
        "Подключить" | "Отключить" | "Запустить прокси" | "Остановить прокси"
    );
    let label = if power {
        format!("     {text}")
    } else {
        text.to_owned()
    };
    let fg = if primary {
        Color32::from_rgb(38, 27, 18)
    } else {
        INK
    };
    let fill = if primary {
        AMBER
    } else {
        Color32::from_rgb(48, 52, 55)
    };
    let button = egui::Button::new(RichText::new(label).font(semibold(14.)).color(fg))
        .corner_radius(12)
        .fill(fill)
        .stroke(Stroke::new(
            1.0_f32,
            if primary {
                Color32::from_rgb(227, 154, 89)
            } else {
                LINE
            },
        ));
    let response = ui.add_sized([ui.available_width(), 40.], button);
    if power {
        let width = ui
            .painter()
            .layout_no_wrap(text.to_string(), semibold(14.), fg)
            .size()
            .x;
        let center = egui::pos2(
            response.rect.center().x - width / 2. - 5.,
            response.rect.center().y,
        );
        let arc: Vec<_> = (0..=20)
            .map(|i| {
                let angle = (-50. + i as f32 * 14.).to_radians();
                center + egui::vec2(angle.cos(), angle.sin()) * 6.5
            })
            .collect();
        ui.painter().add(egui::Shape::line(arc, (1.8, fg)));
        ui.painter().line_segment(
            [center - egui::vec2(0., 8.), center - egui::vec2(0., 1.)],
            (1.8, fg),
        );
    }
    response.clicked()
}

fn navigation(ui: &mut Ui, tab: Tab, label: &str, selected: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 49.), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, label)
    });
    let p = ui.painter();
    let fg = if selected { AMBER } else { MUTED };
    if selected || response.hovered() || response.has_focus() {
        p.rect_filled(rect, 8., Color32::from_rgb(37, 41, 44));
    }
    if response.has_focus() {
        p.rect_stroke(
            rect,
            8.,
            Stroke::new(1.0_f32, AMBER),
            egui::StrokeKind::Inside,
        );
    }
    let xy = |x: f32, y: f32| egui::pos2(rect.center().x - 10. + x, rect.top() + 4. + y);
    let stroke = Stroke::new(1.5_f32, fg);
    let line = |x, y, xx, yy| {
        p.line_segment([xy(x, y), xy(xx, yy)], stroke);
    };
    let outline = |x, y, xx, yy| {
        p.rect_stroke(
            egui::Rect::from_min_max(xy(x, y), xy(xx, yy)),
            1.,
            stroke,
            egui::StrokeKind::Inside,
        );
    };
    match tab {
        Tab::Connection => {
            for x in [2., 12.] {
                for y in [2., 12.] {
                    outline(x, y, x + 6., y + 6.);
                }
            }
        }
        Tab::Telegram => {
            let pts = [
                (2., 9.),
                (19., 2.),
                (15., 18.),
                (9., 13.),
                (6., 16.),
                (6.5, 11.),
                (2., 9.),
            ];
            for w in pts.windows(2) {
                line(w[0].0, w[0].1, w[1].0, w[1].1);
            }
            line(6.5, 11., 15., 6.);
            line(15., 6., 9., 13.);
        }
        Tab::Settings => {
            line(2., 5., 5., 5.);
            line(9., 5., 18., 5.);
            outline(5., 2., 9., 8.);
            line(2., 14., 12., 14.);
            line(16., 14., 18., 14.);
            outline(12., 11., 16., 17.);
        }
        Tab::Journal => {
            outline(3., 2., 17., 18.);
            line(6., 7., 14., 7.);
            line(6., 11., 13., 11.);
        }
    }
    p.text(
        xy(10., 30.),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(12.),
        fg,
    );
    if selected {
        p.rect_filled(
            egui::Rect::from_center_size(
                egui::pos2(rect.center().x, rect.top() + 1.),
                Vec2::new(29., 2.),
            ),
            1.,
            AMBER,
        );
    }
    response
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
            .with_inner_size([424., 640.])
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
            let RawWindowHandle::Win32(handle) = cc.window_handle()?.as_raw() else {
                return Err(io::Error::other("Не найдено окно Windows.").into());
            };
            let window = handle.hwnd.get() as HWND;
            let tray = Tray::new(ctx, window)?;
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
                window,
                visible: true,
                exiting: false,
                restart: written,
                inflight: 0,
                operations: VecDeque::new(),
                connection_busy: false,
                tg_busy: false,
                tg_quality: VecDeque::new(),
                tg_started: None,
                snap: None,
                status: String::new(),
                error: String::new(),
                tg_status: String::new(),
                proxy_url: None,
                report: None,
                history: VecDeque::new(),
                last_probe: None,
                new_probe: false,
                quality: VecDeque::new(),
                dismissed: None,
                notes_shown: false,
                feedback: String::new(),
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
    window: HWND,
    visible: bool,
    exiting: bool,
    restart: Arc<Mutex<Option<Restart>>>,
    inflight: usize,
    operations: VecDeque<(bool, bool)>,
    connection_busy: bool,
    tg_busy: bool,
    tg_quality: VecDeque<Option<u64>>,
    tg_started: Option<std::time::Instant>,
    snap: Option<Box<Snapshot>>,
    status: String,
    error: String,
    tg_status: String,
    proxy_url: Option<String>,
    report: Option<ProbeResult>,
    history: VecDeque<(String, String, Option<ProbeResult>)>,
    last_probe: Option<ProbeResult>,
    new_probe: bool,
    quality: VecDeque<Option<u64>>,
    dismissed: Option<String>,
    notes_shown: bool,
    feedback: String,
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
        let connecting = matches!(cmd, Command::Connect { .. } | Command::Reset(_));
        let telegram = matches!(
            cmd,
            Command::Telegram | Command::ProbeTelegram | Command::StopTelegram
        );
        self.operations.push_back((connecting, telegram));
        self.connection_busy |= connecting;
        self.tg_busy |= telegram;
        if !matches!(cmd, Command::MarkReleaseNotesSeen) {
            self.feedback.clear();
            self.error.clear();
        }
        self.worker.send(cmd);
    }
    fn cancel_telegram(&mut self) {
        self.send(Command::StopTelegram);
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
        self.quality.clear();
        if full
            || self
                .snap
                .as_ref()
                .is_some_and(|s| s.state.last_strategy.as_deref().is_none_or(str::is_empty))
        {
            self.manual = None;
        }
        self.history.clear();
        self.status = "Подключаем выбранные сервисы…".into();
        self.send(Command::Connect {
            full,
            manual: self.manual.clone(),
            voice: self.voice.clone(),
        });
    }
    fn record(&mut self, name: String, event: Outcome) {
        let mut checks = None;
        let detail = match event {
            Outcome::Starting {
                position,
                total,
                confirmation,
            } => {
                let s = if confirmation {
                    "Подтверждение".into()
                } else {
                    format!("Подбор {position}/{total}")
                };
                self.quality.clear();
                self.report = None;
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
                self.quality.push_back(result.sample_ms());
                if self.quality.len() > 5 {
                    self.quality.pop_front();
                }
                checks = Some(result.clone());
                self.new_probe = true;
                self.last_probe = Some(result.clone());
                self.report = Some(result);
                text
            }
        };
        self.history.retain(|(previous, _, _)| previous != &name);
        self.history.push_back((name, detail, checks));
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
        // Apply the tray transition to the actual Windows window independently of frame scheduling.
        unsafe {
            ShowWindow(self.window, SW_HIDE);
        }
        ctx.send_viewport_cmd(ViewportCommand::Visible(false));
    }
    fn on_tray(&mut self, ctx: &egui::Context) {
        while let Some(action) = self.tray.try_next() {
            match action {
                TrayAction::Open => self.show_window(ctx),
                TrayAction::Connect if !self.busy() => self.connect(false),
                TrayAction::Repair if !self.busy() => {
                    self.manual = None;
                    self.connect(false);
                }
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
                Event::Status(s) => {
                    self.feedback.clone_from(&s);
                    self.status = s;
                }
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
                Event::TelegramProbe(result) => {
                    if self.tg_quality.len() == 5 {
                        self.tg_quality.pop_front();
                    }
                    self.tg_quality.push_back(result);
                }
                Event::Finished => {
                    self.operations.pop_front();
                    self.inflight = self.operations.len();
                    self.connection_busy = self.operations.iter().any(|x| x.0);
                    self.tg_busy = self.operations.iter().any(|x| x.1);
                }
                Event::State(next) => {
                    if next.telegram {
                        self.tg_started.get_or_insert_with(std::time::Instant::now);
                    } else {
                        self.tg_started = None;
                        self.tg_quality.clear();
                    }
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
                    if next.verified
                        && (self.new_probe || !self.snap.as_ref().is_some_and(|s| s.verified))
                    {
                        self.last_verified = Some(chrono::Local::now().to_rfc3339());
                    }
                    if self
                        .snap
                        .as_ref()
                        .is_some_and(|old| old.verified && !next.verified)
                        && self.report.as_ref().is_some_and(ProbeResult::success)
                    {
                        self.quality.clear();
                        self.report = None;
                    }
                    self.new_probe = false;
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
        // eframe shows the root HWND after its first presentation. update() may run
        // several layout passes before that point, including on a software adapter.
        if self.startup.is_some() && unsafe { IsWindowVisible(self.window) } == 0 {
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
        if self.visible && !self.notes_shown && self.snap.is_some() {
            self.notes_shown = true;
            let version = running().to_string();
            if self
                .snap
                .as_ref()
                .and_then(|s| s.prefs.last_seen_app_version.as_deref())
                != Some(version.as_str())
            {
                self.notes = true;
                self.tab = Tab::Settings;
                self.send(Command::MarkReleaseNotesSeen);
            }
        }
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
                    ui.label(RichText::new("Связь").font(semibold(19.)).color(INK));
                    ui.label(RichText::new(running().to_string()).color(MUTED).size(12.));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (label, color) = match self.snap.as_deref() {
                            Some(s) if s.verified => ("● Подключено", GREEN),
                            Some(s) if s.discord => ("○ Без проверки", AMBER),
                            Some(s) if s.telegram => ("● Telegram", GREEN),
                            _ => ("○ Отключено", MUTED),
                        };
                        ui.label(RichText::new(label).color(color).size(12.));
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
                        if navigation(col, id, label, active).clicked() {
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
                let tab = self.tab;
                let page = |ui: &mut Ui| {
                    if !self.error.is_empty() {
                        ui.colored_label(RED, self.error.clone());
                    }
                    if self.tab != Tab::Connection && !self.feedback.is_empty() {
                        hint(ui, &self.feedback);
                    }
                    self.update_banner(ui);
                    match self.tab {
                        Tab::Connection => self.connection(ui),
                        Tab::Telegram => self.telegram(ui),
                        Tab::Settings => self.settings(ui),
                        Tab::Journal => self.journal(ui),
                    }
                };
                if tab == Tab::Journal {
                    let mut page = page;
                    page(ui);
                } else {
                    egui::ScrollArea::vertical()
                        .id_salt(("page", tab as u8))
                        .auto_shrink([false, false])
                        .show(ui, page);
                }
            });
        self.confirm_reset(ctx);
    }
}
