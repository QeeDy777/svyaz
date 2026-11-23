use super::*;
use std::io::{Read, Seek, SeekFrom};
use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};

#[derive(Clone, Copy)]
enum Indicator {
    Idle,
    Checking,
    Passed,
    Failed,
    Unconfirmed,
}
impl Indicator {
    fn appearance(self) -> (&'static str, Color32) {
        match self {
            Self::Idle => ("Не проверено", MUTED),
            Self::Checking => ("Проверяем…", AMBER),
            Self::Passed => ("Отвечает", GREEN),
            Self::Failed => ("Не отвечает", RED),
            Self::Unconfirmed => ("Без подтверждения", Color32::from_rgb(245, 197, 66)),
        }
    }
}

fn caption(ui: &mut Ui, text: impl Into<String>, size: f32) {
    ui.label(RichText::new(text).size(size).color(MUTED));
}

fn group_heading(ui: &mut Ui, heading: &str, right: &str, endpoint: bool) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(heading).font(semibold(21.)));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let text = RichText::new(right).color(MUTED);
            ui.label(if endpoint {
                text.monospace().size(13.)
            } else {
                text.size(12.)
            });
        });
    });
}

fn check_row(ui: &mut Ui, name: &str, kind: Indicator, detail: &str) {
    let (status, color) = kind.appearance();
    let row = ui.allocate_ui_with_layout(
        Vec2::new(ui.available_width(), 36.),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(RichText::new(name).size(14.));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.;
                ui.label(RichText::new(status).color(color).font(semibold(12.)));
                let (dot, _) = ui.allocate_exact_size(Vec2::splat(6.), egui::Sense::hover());
                ui.painter().circle_filled(dot.center(), 3., color);
            });
        },
    );
    let rect = row.response.rect;
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0_f32, LINE),
    );
    row.response.on_hover_text(detail);
}

fn check_detail(check: &CheckResult) -> String {
    match check {
        CheckResult::Success { elapsed_ms } => format!("Ответ за {elapsed_ms} мс"),
        CheckResult::Failed(reason) => reason.clone(),
        CheckResult::Skipped => "Проверка пропущена".into(),
    }
}

fn service_row(
    ui: &mut Ui,
    name: &str,
    description: &str,
    check: Option<&CheckResult>,
    checking: bool,
) {
    let state = match check {
        Some(CheckResult::Success { .. }) => Indicator::Passed,
        Some(CheckResult::Failed(_)) => Indicator::Failed,
        None if checking => Indicator::Checking,
        _ => Indicator::Idle,
    };
    let detail = check.map_or_else(
        || description.to_string(),
        |c| format!("{description}\n{}", check_detail(c)),
    );
    check_row(ui, name, state, &detail);
}

fn windows_version() -> Option<String> {
    use winreg::{RegKey, enums::HKEY_LOCAL_MACHINE};
    let key = RegKey::predef(HKEY_LOCAL_MACHINE)
        .open_subkey(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion")
        .ok()?;
    let major: u32 = key.get_value("CurrentMajorVersionNumber").ok()?;
    let minor: u32 = key.get_value("CurrentMinorVersionNumber").ok()?;
    let build: String = key.get_value("CurrentBuildNumber").ok()?;
    Some(format!("{major}.{minor}.{build}"))
}

fn quality_summary(samples: &VecDeque<Option<u64>>) -> Option<String> {
    if samples.is_empty() {
        return None;
    }
    let mut times = samples.iter().filter_map(|p| *p).collect::<Vec<_>>();
    times.sort_unstable();
    let count = format!("Успешно {}/{}", times.len(), samples.len());
    let Some(&first) = times.first() else {
        return Some(format!("{count} · время не измерено"));
    };
    let low = times[(times.len() - 1) / 2];
    let median = low + (times[times.len() / 2] - low) / 2;
    let spread = if times.len() == 1 {
        "нужен ещё замер".into()
    } else {
        format!("разброс {} мс", times[times.len() - 1] - first)
    };
    Some(format!("Отклик {median} мс · {spread} · {count}"))
}

fn labels(s: &ServiceSelection) -> String {
    let mut names = Vec::new();
    if s.discord {
        names.push("Discord");
    }
    if s.youtube {
        names.push("YouTube");
    }
    if s.telegram {
        names.push("Telegram");
    }
    names.join(" · ")
}
fn launch_proxy(url: &str) -> io::Result<()> {
    // Only the validated loopback URI produced by svyaz_telegram::proxy_url reaches ShellExecute.
    let wide: Vec<_> = url.encode_utf16().chain(Some(0)).collect();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            std::ptr::null(),
            wide.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as usize <= 32 {
        Err(io::Error::other(
            "Установите Telegram или скопируйте ссылку прокси.",
        ))
    } else {
        Ok(())
    }
}

impl Desktop {
    pub(super) fn update_banner(&mut self, ui: &mut Ui) {
        if let Some((id, version)) = self
            .snap
            .as_ref()
            .and_then(|s| s.pending.as_ref())
            .filter(|a| self.dismissed.as_ref() != Some(&a.id))
            .map(|a| (a.id.clone(), a.version.to_string()))
        {
            card().show(ui, |ui| {
                ui.strong(format!("Версия {version} готова"));
                ui.add_enabled_ui(!self.busy(), |ui| {
                    if action(ui, "Обновить и перезапустить", true) {
                        self.send(Command::Apply);
                    }
                    if ui.button("Позже").clicked() {
                        self.dismissed = Some(id);
                    }
                });
            });
        }
    }
    fn telegram_check(&self) -> (Indicator, String) {
        let active = self.snap.as_ref().is_some_and(|s| s.telegram);
        if self.tg_checking {
            (
                Indicator::Checking,
                "Проверяем ответ MTProto через прокси".into(),
            )
        } else if !active {
            if self.snap.as_ref().is_some_and(|s| s.telegram_wanted) && !self.error.is_empty() {
                (Indicator::Failed, self.error.clone())
            } else {
                (Indicator::Idle, "Ответ MTProto через прокси".into())
            }
        } else {
            match self.tg_quality.back() {
                Some(Some(_)) => (Indicator::Passed, self.tg_status.clone()),
                Some(None) => (Indicator::Unconfirmed, self.tg_status.clone()),
                None => (Indicator::Idle, "Сервер ещё не проверен.".into()),
            }
        }
    }

    fn telegram_description(&self) -> String {
        let active = self.snap.as_ref().is_some_and(|s| s.telegram);
        if active && self.network_notice_visible() {
            "Сеть изменилась. Локальный прокси запущен.".into()
        } else if active {
            match self.tg_quality.back() {
                Some(Some(_)) => "Прокси запущен на этом компьютере.".into(),
                Some(None) => format!("Прокси запущен. {}", self.tg_status),
                None => "Прокси запущен.".into(),
            }
        } else if self.snap.as_ref().is_some_and(|s| s.telegram_wanted) {
            "Ожидаем восстановления подключения.".into()
        } else {
            "Встроенный прокси выключен.".into()
        }
    }

    pub(super) fn connection(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 0.;
        let selected = self
            .snap
            .as_ref()
            .map(|s| s.prefs.effective_services(&s.state))
            .unwrap_or_default();
        let connected = self.connected();
        let waiting = self.snap.as_ref().is_some_and(|s| s.telegram_wanted);
        let probing = self.connection_busy || (selected.telegram && self.tg_busy);
        hero(ui, true, |ui| {
            let heading = if probing {
                "Проверяем подключение"
            } else if !self.error.is_empty() {
                "Нужно ваше внимание"
            } else if connected {
                "Связь включена"
            } else if waiting {
                "Ожидаем подключения"
            } else {
                "Связь выключена"
            };
            ui.add_space(6.);
            title(ui, heading);
            ui.add_space(4.);
            let status =
                if self.network_notice_visible() && self.snap.as_ref().is_some_and(|s| s.discord) {
                    "Сеть изменилась. Текущий альт запущен.".into()
                } else if selected.telegram && !selected.uses_zapret() {
                    self.telegram_description()
                } else if self.status.is_empty() {
                    labels(&selected)
                } else {
                    self.status.clone()
                };
            hint(ui, status);
            if let Some(alt) = self
                .snap
                .as_ref()
                .filter(|s| s.discord)
                .and_then(|s| s.state.last_strategy.as_deref())
            {
                ui.add_space(4.);
                caption(ui, format!("Текущий альт · {alt}"), 12.);
            }
            ui.add_space(20.);
            let label = if probing {
                "Отменить и отключить"
            } else if connected || waiting {
                "Отключить"
            } else {
                "Подключить"
            };
            ui.add_enabled_ui(probing || !self.busy(), |ui| {
                if action(ui, label, true) {
                    if probing || connected || waiting {
                        self.send(Command::Stop);
                    } else {
                        self.connect(false);
                    }
                }
            });
        });
        ui.add_space(22.);
        group_heading(ui, "Сервисы", "", false);
        ui.add_space(6.);
        let result = self.report.as_ref();
        let status = |checks: &[&CheckResult]| {
            if self.connection_busy {
                Indicator::Checking
            } else if checks.iter().any(|c| matches!(c, CheckResult::Failed(_))) {
                Indicator::Failed
            } else if !checks.is_empty()
                && checks
                    .iter()
                    .all(|c| matches!(c, CheckResult::Success { .. }))
            {
                Indicator::Passed
            } else {
                Indicator::Idle
            }
        };
        if selected.discord {
            let state =
                result.map_or_else(|| status(&[]), |r| status(&[&r.api, &r.gateway, &r.voice]));
            check_row(ui, "Discord", state, "API, Gateway и голосовой сервер");
        }
        if selected.youtube {
            let state =
                result.map_or_else(|| status(&[]), |r| status(&[&r.youtube, &r.google_video]));
            check_row(ui, "YouTube", state, "Сайт и Googlevideo");
        }
        if selected.telegram {
            let (state, detail) = self.telegram_check();
            check_row(ui, "Telegram", state, &detail);
        }
        ui.add_space(18.);
        disclosure(ui, "Диагностика подключения", |ui| {
            let result = self.report.as_ref();
            if selected.discord {
                for (name, help, item) in [
                    ("API", "Доступ к Discord", result.map(|r| &r.api)),
                    (
                        "Gateway",
                        "Соединение с Discord",
                        result.map(|r| &r.gateway),
                    ),
                    (
                        "Голосовой сервер",
                        "Ответ сервера канала",
                        result.map(|r| &r.voice),
                    ),
                ] {
                    service_row(ui, name, help, item, self.connection_busy);
                }
            }
            if selected.youtube {
                let (state, detail) = match result {
                    Some(r) => {
                        let kind = if matches!(r.youtube, CheckResult::Failed(_))
                            || matches!(r.google_video, CheckResult::Failed(_))
                        {
                            Indicator::Failed
                        } else if matches!(r.youtube, CheckResult::Success { .. })
                            && matches!(r.google_video, CheckResult::Success { .. })
                        {
                            Indicator::Passed
                        } else {
                            Indicator::Idle
                        };
                        (
                            kind,
                            format!(
                                "Сайт YouTube · {}\nGooglevideo · {}\nВоспроизведение видео не проверяется.",
                                check_detail(&r.youtube),
                                check_detail(&r.google_video)
                            ),
                        )
                    }
                    None => (
                        if self.connection_busy {
                            Indicator::Checking
                        } else {
                            Indicator::Idle
                        },
                        "Сайт и Googlevideo. Воспроизведение видео не проверяется.".into(),
                    ),
                };
                check_row(ui, "YouTube · HTTPS", state, &detail);
            }
            if selected.telegram {
                let (state, detail) = self.telegram_check();
                check_row(ui, "Сервер Telegram", state, &detail);
            }
            ui.add_space(10.);
            if let Some(text) = quality_summary(&self.quality) {
                Frame::default()
                    .fill(Color32::from_rgb(37, 41, 44))
                    .corner_radius(12)
                    .inner_margin(10)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new(text).size(12.));
                        ui.add_space(4.);
                        caption(ui, "Время ответа сервисов за последние пять проверок.", 11.);
                    });
                ui.add_space(12.);
            }
            if selected.youtube {
                caption(
                    ui,
                    "Для YouTube проверяется ответ сайта и Googlevideo.",
                    11.,
                );
                ui.add_space(12.);
            }
            if selected.uses_zapret() {
                ui.add_enabled_ui(!self.busy(), |ui| {
                    if action(ui, "Полный подбор", false) {
                        self.connect(true);
                    }
                });
                ui.add_space(6.);
                caption(
                    ui,
                    "Проверяет все альты для этой сети. Во время подбора связь может прерываться.",
                    11.,
                );
                ui.add_space(14.);
                if !self.error.is_empty() && !probing {
                    if action(ui, "Повторить проверку", false) {
                        self.manual = None;
                        self.connect(false);
                    }
                    ui.add_space(12.);
                }
            }
            if let Some(duration) = self.snap.as_ref().and_then(|s| s.uptime) {
                ui.vertical_centered(|ui| {
                    caption(
                        ui,
                        format!("Подключено {} мин", duration.as_secs() / 60),
                        12.,
                    )
                });
            }
            if let Some(stamp) = self
                .last_verified
                .as_ref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            {
                ui.add_space(4.);
                ui.vertical_centered(|ui| {
                    caption(
                        ui,
                        format!("Успешная проверка · {}", stamp.format("%d.%m %H:%M")),
                        12.,
                    )
                });
                ui.add_space(12.);
            }
        });
        disclosure(ui, "История проверок", |ui| {
            ui.add_space(12.);
            if self.history.is_empty() {
                hint(ui, "После подключения здесь появятся результаты подбора.");
            }
            for (name, outcome, result) in self.history.iter().rev() {
                ui.strong(name);
                ui.add_space(4.);
                ui.label(outcome);
                if let Some(r) = result {
                    ui.add_space(8.);
                    for (service, c) in [
                        ("API", &r.api),
                        ("Gateway", &r.gateway),
                        ("Голос", &r.voice),
                        ("YouTube", &r.youtube),
                        ("Googlevideo", &r.google_video),
                    ] {
                        if !matches!(c, CheckResult::Skipped) {
                            caption(ui, format!("{service} · {}", check_detail(c)), 12.);
                        }
                    }
                }
                ui.add_space(10.);
                ui.separator();
                ui.add_space(10.);
            }
        });
    }

    pub(super) fn telegram(&mut self, ui: &mut Ui) {
        ui.spacing_mut().item_spacing.y = 0.;
        let active = self.snap.as_ref().is_some_and(|s| s.telegram);
        let waiting = !active && self.snap.as_ref().is_some_and(|s| s.telegram_wanted);
        hero(ui, false, |ui| {
            title(ui, "Прокси Telegram");
            ui.add_space(4.);
            hint(ui, self.telegram_description());
            ui.add_space(14.);
            let label = if self.tg_busy {
                "Отменить"
            } else if active {
                "Остановить прокси"
            } else if waiting {
                "Отменить повторы"
            } else {
                "Запустить прокси"
            };
            ui.add_enabled_ui(!self.busy() || self.tg_busy, |ui| {
                if action(ui, label, !active) {
                    if self.tg_busy {
                        self.cancel_telegram();
                    } else {
                        self.send(Command::Telegram);
                    }
                }
            });
        });
        ui.add_space(16.);
        if active {
            let port = self.snap.as_ref().unwrap().state.telegram.port;
            group_heading(ui, "Адрес прокси", &format!("127.0.0.1:{port}"), true);
            ui.add_space(8.);
            let url = self.proxy_url.clone();
            ui.add_enabled_ui(url.is_some(), |ui| {
                ui.columns(2, |cols| {
                    if action(&mut cols[0], "Открыть в Telegram", true) {
                        if let Some(url) = url.as_deref() {
                            if let Err(error) = launch_proxy(url) {
                                self.error = error.to_string();
                            }
                        }
                    }
                    if action(&mut cols[1], "Скопировать ссылку", false) {
                        if let Some(url) = url {
                            cols[1].ctx().copy_text(url);
                            self.feedback = "Ссылка скопирована.".into();
                        }
                    }
                });
            });
            if self.proxy_url.is_none() {
                ui.colored_label(RED, "Не удалось подготовить ссылку локального прокси.");
            }
            ui.add_space(16.);
        }
        let time = if active {
            self.tg_started.map_or_else(
                || "Прокси запущен".into(),
                |s| format!("Прокси работает {} мин", s.elapsed().as_secs() / 60),
            )
        } else {
            String::new()
        };
        group_heading(ui, "Проверка", &time, false);
        ui.add_space(4.);
        let (state, detail) = self.telegram_check();
        check_row(ui, "Сервер Telegram", state, &detail);
        if active {
            if let Some(summary) = quality_summary(&self.tg_quality) {
                ui.add_space(6.);
                caption(ui, summary, 12.);
            }
            ui.add_space(8.);
            ui.add_enabled_ui(!self.busy(), |ui| {
                if action(ui, "Проверить сервер", false) {
                    self.send(Command::ProbeTelegram);
                }
            });
        }
        ui.add_space(16.);
        disclosure(ui, "Настройки Telegram", |ui| {
            ui.add_space(12.);
            ui.add_enabled_ui(!self.busy() && !active && !waiting, |ui| {
                ui.label("Локальный порт");
                ui.add_space(6.);
                ui.add_sized(
                    [ui.available_width(), 38.],
                    egui::TextEdit::singleline(&mut self.port).char_limit(5),
                );
                ui.add_space(16.);
                ui.checkbox(&mut self.tg_start, "Запускать вместе со «Связью»");
                ui.add_space(16.);
                if ui
                    .add(egui::Button::new("Сохранить настройки").min_size(Vec2::new(150., 38.)))
                    .clicked()
                {
                    match self.port.trim().parse::<u16>().ok().filter(|p| *p != 0) {
                        Some(port) => self.send(Command::SaveTelegram {
                            port: i32::from(port),
                            start_with_app: self.tg_start,
                        }),
                        None => self.error = "Порт должен быть от 1 до 65535.".into(),
                    }
                }
            });
            ui.add_space(12.);
            caption(
                ui,
                format!("Встроенный движок Telegram {}", svyaz_telegram::VERSION),
                12.,
            );
            ui.add_space(8.);
            caption(
                ui,
                "Перед изменением настроек остановите прокси. Движок обновляется вместе со «Связью».",
                12.,
            );
            ui.add_space(8.);
        });
    }

    pub(super) fn settings(&mut self, ui: &mut Ui) {
        title(ui, "Настройки");
        let show_notes = std::mem::take(&mut self.notes);
        if show_notes {
            ui.data_mut(|data| {
                for name in ["Запуск и обновления", "Что нового"] {
                    data.insert_temp(disclosure_id(name), true);
                }
            });
        }
        card().inner_margin(16).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.;
            ui.label(RichText::new("Сервисы подключения").font(semibold(21.)));
            ui.add_space(4.);
            hint(ui, "Что запускать кнопкой «Подключить»");
            ui.add_space(4.);
            let editable = !self.busy() && !self.connected();
            ui.add_enabled_ui(editable, |ui| {
                switch_row(ui, "Discord", &mut self.services.discord);
                dividing_line(ui);
                switch_row(ui, "YouTube", &mut self.services.youtube);
                dividing_line(ui);
                switch_row(ui, "Telegram", &mut self.services.telegram);
                dividing_line(ui);
                switch_row(ui, "Сообщать о длительном сбое", &mut self.quiet);
                ui.add_space(6.);
                let changed = self.snap.as_ref().is_some_and(|s| {
                    self.services != s.prefs.effective_services(&s.state)
                        || self.quiet != s.prefs.quiet_notifications
                });
                if changed && action(ui, "Сохранить выбор", true) {
                    self.send(Command::SaveServices(self.services.clone(), self.quiet));
                }
            });
            if self.connected() {
                ui.add_space(8.);
                hint(ui, "Отключите связь, чтобы изменить выбор.");
            }
        });
        if let Some(mut enabled) = self.snap.as_ref().map(|s| s.prefs.connection_sounds) {
            ui.add_enabled_ui(!self.busy(), |ui| {
                if switch_row(ui, "Звуки подключения и сбоев", &mut enabled) {
                    self.send(Command::ConnectionSounds(enabled));
                }
            });
        }
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 0.;
            disclosure(ui, "Запуск и обновления", |ui| {
                let mut autostart = self.snap.as_ref().is_some_and(|s| s.autostart);
                let mut update = self.snap.as_ref().is_none_or(|s| s.state.auto_update);
                ui.add_enabled_ui(!self.busy(), |ui| {
                    if switch_row(ui, "Подключать при входе в Windows", &mut autostart) {
                        self.send(Command::Autostart(autostart));
                    }
                    hint(ui, "Через 15 секунд после входа «Связь» запустится в трее.");
                    if switch_row(ui, "Обновлять «Связь» и компоненты", &mut update) {
                        self.send(Command::AutoUpdate(update));
                    }
                    hint(ui, "Проверка каждые 6 часов. Загруженная версия установится при следующем запуске.");
                    if action(ui, "Проверить новую версию", false) {
                        self.send(Command::CheckApp);
                    }
                    let pending = self.snap.as_ref().is_some_and(|s| s.pending.is_some());
                    ui.add_enabled_ui(pending, |ui| {
                        if action(ui, "Обновить и перезапустить", true) {
                            self.send(Command::Apply);
                        }
                    });
                    hint(ui, "Перезапуск ненадолго прервёт подключение. Предыдущая версия и настройки сохранятся.");
                });
                disclosure(ui, "Что нового", |ui| {
                    let heading = ui.strong(format!("Связь {}", running()));
                    if show_notes { heading.scroll_to_me(Some(egui::Align::Center)); }
                    hint(ui, "Во время перемещения окна Windows использует уже отрисованный кадр. После отпускания мыши «Связь» сразу показывает актуальное состояние.");
                    disclosure(ui, "Предыдущие выпуски", |ui| hint(ui, include_str!("release_history.txt").trim_end()));
                });
                if let Some(version) = self.snap.as_ref().and_then(|s| s.previous.as_ref()).map(|s| s.version.to_string()) {
                    disclosure(ui, "Откат версии", |ui| {
                        hint(ui, "Откат перезапустит приложение и выключит автообновление. Настройки сохранятся.");
                        ui.add_enabled_ui(!self.busy(), |ui| {
                            if action(ui, &format!("Вернуться к версии {version}"), false) { self.send(Command::Rollback); }
                        });
                    });
                }
                ui.add_enabled_ui(!self.busy(), |ui| {
                    if action(ui, "Выйти из «Связи»", false) { self.send(Command::Quit); }
                });
            });
            disclosure(ui, "Параметры подключения", |ui| {
                ui.label("Стратегия");
                let strategies = self.snap.as_ref().map(|s| s.strategies.clone()).unwrap_or_default();
                egui::ComboBox::from_id_salt("strategy")
                    .width(ui.available_width())
                    .selected_text(self.manual.as_deref().unwrap_or("Автоматически"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.manual, None, "Автоматически");
                        for s in strategies { ui.selectable_value(&mut self.manual, Some(s.clone()), s); }
                    });
                hint(ui, "Проверка проходит при подключении. После сна и смены сети рабочий альт сохраняется.");
                ui.label("Голосовой сервер Discord · необязательно");
                ui.add_sized([ui.available_width(), 38.], egui::TextEdit::singleline(&mut self.voice).char_limit(255));
                hint(ui, "Оставьте поле пустым для поиска в журнале Discord. Если адрес ещё не появился, войдите в голосовой канал. Формат адреса — имя.discord.media:порт.");
            });
            disclosure(ui, "Игры", |ui| {
                const MODES: [&str; 4] = ["Выключен", "TCP и UDP", "Только TCP", "Только UDP"];
                ui.add_enabled_ui(!self.busy() && !self.snap.as_ref().is_some_and(|s|s.discord), |ui| {
                    ui.label("Режим");
                    egui::ComboBox::from_id_salt("filter").width(ui.available_width())
                        .selected_text(MODES.get(self.game.mode as usize).copied().unwrap_or("Неизвестно"))
                        .show_ui(ui, |ui| {
                            for (i,t) in MODES.iter().enumerate() {
                                ui.selectable_value(&mut self.game.mode, i as i32, *t);
                            }
                        });
                    disclosure(ui, "Диапазоны портов", |ui| {
                        ui.label("TCP");
                        ui.add_sized([ui.available_width(),38.], egui::TextEdit::singleline(&mut self.game.tcp_ports).char_limit(2048));
                        ui.label("UDP");
                        ui.add_sized([ui.available_width(),38.], egui::TextEdit::singleline(&mut self.game.udp_ports).char_limit(2048));
                        hint(ui, "По умолчанию 1024–65535. Перечислите порты и диапазоны через запятую.");
                    });
                    hint(ui, "Фильтр обрабатывает выбранные порты для сетей из IPSet Flowseal. Проверьте подключение в самой игре.");
                    if action(ui, "Сохранить Game Filter", false) { self.send(Command::SaveGame(self.game.clone())); }
                });
            });
            disclosure(ui, "Диагностика", |ui| {
                hint(ui, "В отчёт входят версия, состояние сервисов и результаты проверок.");
                if action(ui, "Сохранить отчёт", false) { self.diagnostics(); }
            });
            disclosure(ui, "Обслуживание zapret", |ui| {
                if let Some(name) = self.snap.as_ref().and_then(|s| s.state.last_strategy.as_deref()) {
                    hint(ui, format!("Последний выбранный альт · {name}"));
                } else if self.snap.as_ref().is_none_or(|s| s.state.active_package.is_none()) {
                    hint(ui, "Пакет ещё не загружен");
                }
                ui.add_enabled_ui(!self.busy(), |ui| {
                    if action(ui, "Обновить альты", false) { self.send(Command::CheckPackage); }
                    ui.add_space(6.);
                    card().show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.label(RichText::new("Полный сброс zapret").font(semibold(18.)));
                        hint(ui, "Сброс выгрузит WinDivert и удалит его службу, пакеты и списки zapret. Затем «Связь» скачает свежую копию и начнёт подбор.");
                        let reset = egui::Button::new(RichText::new("Сбросить кэш zapret").color(RED).strong()).corner_radius(12);
                        if ui.add_sized([ui.available_width(),40.],reset).clicked() { self.send(Command::InspectReset); }
                    });
                });
            });
        });
    }

    fn diagnostics(&mut self) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter("JSON", &["json"])
            .set_file_name("svyaz-diagnostics.json")
            .save_file()
        else {
            return;
        };
        let one = |c: Option<&CheckResult>| match c {
            Some(CheckResult::Success { .. }) => Some(true),
            Some(CheckResult::Failed(_)) => Some(false),
            _ => None,
        };
        let r = self.last_probe.as_ref();
        let report = serde_json::json!({
            "AppVersion":running().to_string(),"WindowsVersion":windows_version(),
            "LastDiscordError":null, "LastTelegramError":null,
            "DiscordProcessRunning":self.snap.as_ref().is_some_and(|s|s.discord),
            "DiscordVerified":self.snap.as_ref().is_some_and(|s|s.verified),
            "TelegramProcessRunning":self.snap.as_ref().is_some_and(|s|s.telegram),
            "ApiPassed":one(r.map(|r|&r.api)),"GatewayPassed":one(r.map(|r|&r.gateway)),"VoicePassed":one(r.map(|r|&r.voice)),
            "YouTubeSitePassed":one(r.map(|r|&r.youtube)),"YouTubeGoogleVideoPassed":one(r.map(|r|&r.google_video)),
            "LastSuccessfulCheck":self.last_verified,"TestedStrategies":self.history.len()
        });
        let saved = serde_json::to_vec_pretty(&report)
            .map_err(io::Error::other)
            .and_then(|b| fs::write(path, b));
        match saved {
            Ok(()) => self.feedback = "Отчёт сохранён.".into(),
            Err(e) => self.error = e.to_string(),
        }
    }
    pub(super) fn journal(&mut self, ui: &mut Ui) {
        title(ui, "Журнал");
        let now = std::time::Instant::now();
        if self
            .journal_last_read
            .is_none_or(|last| now.duration_since(last) >= std::time::Duration::from_millis(500))
        {
            self.journal_last_read = Some(now);
            self.refresh_journal();
        }
        if self.journal.trim().is_empty() {
            card().inner_margin(16).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    ui.spacing_mut().item_spacing.x = 12.;
                    let (icon, _) =
                        ui.allocate_exact_size(egui::vec2(19., 23.), egui::Sense::hover());
                    let glyph = egui::Rect::from_min_max(
                        icon.min + egui::vec2(1., 3.),
                        icon.max - egui::vec2(1., 0.),
                    );
                    let stroke = Stroke::new(1.5_f32, AMBER);
                    ui.painter()
                        .rect_stroke(glyph, 0, stroke, egui::StrokeKind::Inside);
                    for y in [glyph.top() + 6.5, glyph.top() + 12.] {
                        ui.painter().line_segment(
                            [
                                egui::pos2(glyph.left() + 4., y),
                                egui::pos2(glyph.right() - 4., y),
                            ],
                            stroke,
                        );
                    }
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 4.;
                        ui.label(RichText::new("Записей пока нет").font(semibold(21.)));
                        hint(
                            ui,
                            "Здесь появятся результаты проверок, обновления и сообщения zapret.",
                        );
                    });
                });
            });
        } else {
            hint(ui, "Выделите записи и нажмите Ctrl+C, чтобы скопировать.");
            let h = ui.available_height();
            card()
                .inner_margin(egui::Margin {
                    left: 12,
                    right: 4,
                    top: 8,
                    bottom: 8,
                })
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical()
                        .id_salt("journal-entries")
                        .max_height((h - 18.).max(1.))
                        .auto_shrink([false, false])
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            let mut text = self.journal.as_str();
                            let entries = egui::TextEdit::multiline(&mut text)
                                .font(egui::FontId::monospace(13.))
                                .text_color(INK)
                                .frame(false)
                                .desired_rows(1)
                                .desired_width(f32::INFINITY)
                                .margin(egui::Margin::symmetric(10, 5));
                            ui.add(entries).context_menu(|ui| {
                                if ui.button("Скопировать журнал").clicked() {
                                    ui.ctx().copy_text(self.journal.clone());
                                    ui.close_menu();
                                }
                            });
                        });
                });
        }
    }

    fn refresh_journal(&mut self) {
        let log = self.data.join("app.log");
        if let Ok(meta) = fs::metadata(&log) {
            let current = (meta.len(), meta.modified().ok());
            if self.journal_meta != Some(current) {
                if let Ok(mut file) = fs::File::open(&log) {
                    let mut bytes = Vec::new();
                    if file
                        .seek(SeekFrom::Start(meta.len().saturating_sub(64 * 1024)))
                        .is_ok()
                        && file.take(64 * 1024).read_to_end(&mut bytes).is_ok()
                    {
                        let mut text = String::from_utf8_lossy(&bytes).into_owned();
                        if let Some(secret) = self
                            .snap
                            .as_ref()
                            .map(|s| s.state.telegram.secret.as_str())
                            .filter(|s| !s.is_empty())
                        {
                            text = text.replace(secret, "•••");
                        }
                        self.journal = text;
                        self.journal_meta = Some(current);
                    }
                }
            }
        }
    }

    pub(super) fn confirm_reset(&mut self, ctx: &egui::Context) {
        let Some(approved) = self.reset_confirm.clone() else {
            return;
        };
        let mut command = None;
        egui::Modal::new(egui::Id::new("reset-confirm")).show(ctx,|ui|{
            ui.set_max_width(340.);title(ui,"Сбросить кэш zapret?");
            ui.label("Текущее подключение прервётся. Будут удалены драйвер WinDivert «Связи», скачанные пакеты, пользовательские списки и сохранённые альты.");
            if !approved.is_empty(){ui.label("Также будут удалены службы старого zapret:");for(name,path)in &approved{ui.strong(name);hint(ui,path);}}
            if ui.button("Отмена").clicked(){command=Some(false);}
            if ui.button(RichText::new("Сбросить и загрузить заново").color(RED)).clicked(){command=Some(true);}
        });
        if let Some(confirmed) = command {
            self.reset_confirm = None;
            if confirmed {
                self.manual = None;
                self.voice.clear();
                self.quality.clear();
                self.history.clear();
                self.report = None;
                self.last_probe = None;
                self.last_verified = None;
                self.send(Command::Reset(approved));
            }
        }
    }
}

fn dividing_line(ui: &mut Ui) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.), egui::Sense::hover());
    ui.painter()
        .hline(rect.x_range(), rect.center().y, Stroke::new(1.0_f32, LINE));
}

fn switch_row(ui: &mut Ui, label: &str, value: &mut bool) -> bool {
    let (row, mut response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 30.), egui::Sense::click());
    if response.clicked() {
        *value = !*value;
        response.mark_changed();
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), *value, label)
    });
    ui.painter().text(
        row.left_center(),
        egui::Align2::LEFT_CENTER,
        label,
        semibold(14.),
        INK,
    );
    let track = egui::Rect::from_center_size(
        egui::pos2(row.right() - 19., row.center().y),
        egui::vec2(38., 22.),
    );
    ui.painter().rect_filled(
        track,
        11,
        if *value {
            GREEN
        } else {
            Color32::from_rgb(47, 52, 54)
        },
    );
    let focus = response.has_focus() || response.hovered();
    let outline = if focus {
        AMBER
    } else if *value {
        GREEN
    } else {
        MUTED
    };
    ui.painter().rect_stroke(
        track,
        11,
        Stroke::new(1.0_f32, outline),
        egui::StrokeKind::Inside,
    );
    let t = transition(ui, response.id.with("knob"), *value);
    let x = egui::lerp(track.left() + 10.5..=track.right() - 10.5, t);
    ui.painter().circle_filled(
        egui::pos2(x, track.center().y),
        7.,
        if *value { BG } else { MUTED },
    );
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .changed()
}

fn disclosure_id(label: &str) -> egui::Id {
    egui::Id::new(("svyaz-disclosure", label))
}

fn disclosure(ui: &mut Ui, name: &str, body: impl FnOnce(&mut Ui)) {
    let id = disclosure_id(name);
    let mut expanded = ui.data(|data| data.get_temp::<bool>(id)).unwrap_or(false);
    let (row, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 38.), egui::Sense::click());
    if response.clicked() {
        expanded = !expanded;
        ui.data_mut(|data| data.insert_temp(id, expanded));
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::CollapsingHeader,
            ui.is_enabled(),
            expanded,
            name,
        )
    });
    let focus = response.has_focus() || response.hovered();
    let color = if focus { AMBER } else { INK };
    ui.painter().text(
        row.left_center() + egui::vec2(2., 0.),
        egui::Align2::LEFT_CENTER,
        name,
        semibold(14.),
        color,
    );
    let c = egui::pos2(row.right() - 13., row.center().y);
    let t = transition(ui, id.with("arrow"), expanded);
    let rotation = egui::emath::Rot2::from_angle(t * std::f32::consts::FRAC_PI_2);
    let points = [(-2., -4.), (2., 0.), (-2., 4.)].map(|(x, y)| c + rotation * egui::vec2(x, y));
    ui.painter().add(egui::Shape::line(
        points.to_vec(),
        Stroke::new(1.5_f32, if focus { AMBER } else { MUTED }),
    ));
    response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if expanded {
        egui::Frame::default()
            .inner_margin(egui::Margin {
                left: 2,
                right: 2,
                top: 6,
                bottom: 14,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 8.;
                body(ui);
            });
    }
    dividing_line(ui);
}
