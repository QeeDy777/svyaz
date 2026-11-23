use super::*;
use std::io::{Read, Seek, SeekFrom};
use windows_sys::Win32::UI::{Shell::ShellExecuteW, WindowsAndMessaging::SW_SHOWNORMAL};

fn section(ui: &mut Ui, name: &str, body: impl FnOnce(&mut Ui)) {
    card().show(ui, |ui| {
        ui.set_min_width(ui.available_width());
        egui::CollapsingHeader::new(RichText::new(name).strong().size(15.)).show(ui, body);
    });
}
fn check_row(ui: &mut Ui, name: &str, check: Option<&CheckResult>, enabled: bool) {
    let (text, color) = match check {
        _ if !enabled => ("Не выбрано".into(), MUTED),
        Some(CheckResult::Success { elapsed_ms }) => (format!("✓ {elapsed_ms} мс"), GREEN),
        Some(CheckResult::Failed(_)) => ("× Нет ответа".into(), RED),
        Some(CheckResult::Skipped) => ("Не выбрано".into(), MUTED),
        None => ("○ Не проверено".into(), MUTED),
    };
    ui.horizontal(|ui| {
        ui.strong(name);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let reply = ui.label(RichText::new(text).size(13.).color(color));
            if let Some(CheckResult::Failed(reason)) = check {
                reply.on_hover_text(reason);
            }
        });
    });
    ui.separator();
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
        if let Some(version) = self
            .snap
            .as_ref()
            .and_then(|s| s.pending.as_ref().map(|a| a.version.to_string()))
        {
            card().show(ui, |ui| {
                ui.strong(format!("Версия {version} готова"));
                ui.add_enabled_ui(!self.busy(), |ui| {
                    if action(ui, "Обновить и перезапустить", true) {
                        self.send(Command::Apply);
                    }
                });
            });
        }
    }
    pub(super) fn connection(&mut self, ui: &mut Ui) {
        let selected = self
            .snap
            .as_ref()
            .map(|s| s.prefs.effective_services(&s.state))
            .unwrap_or_default();
        let connected = self.connected();
        let verified = self.snap.as_ref().is_some_and(|s| s.verified);
        let busy = self.busy();
        Frame::default()
            .fill(HERO)
            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(92, 73, 57)))
            .corner_radius(15)
            .inner_margin(16)
            .show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                title(
                    ui,
                    if busy {
                        "Выполняется операция"
                    } else if !self.error.is_empty() {
                        "Нужно ваше внимание"
                    } else if verified {
                        "Связь включена"
                    } else if connected {
                        "Соединение запущено"
                    } else {
                        "Связь выключена"
                    },
                );
                let text = if !self.status.is_empty() {
                    self.status.clone()
                } else {
                    "При подключении проверим выбранные сервисы.".into()
                };
                hint(ui, text);
                ui.add_space(4.);
                let label = if busy {
                    "Отменить"
                } else if connected {
                    "Отключить"
                } else {
                    "Подключить"
                };
                if action(ui, label, true) {
                    if busy || connected {
                        self.send(Command::Stop)
                    } else {
                        self.connect(false);
                    }
                }
            });
        ui.add_space(8.);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Проверки").size(21.).strong());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                hint(ui, labels(&selected));
            });
        });
        let r = self.report.as_ref();
        if selected.discord {
            check_row(ui, "API", r.map(|r| &r.api), true);
            check_row(ui, "Gateway", r.map(|r| &r.gateway), true);
            check_row(ui, "Голосовой сервер", r.map(|r| &r.voice), true);
        }
        if selected.youtube {
            check_row(ui, "YouTube · HTTPS", r.map(|r| &r.youtube), true);
            check_row(ui, "Googlevideo · HTTPS", r.map(|r| &r.google_video), true);
            hint(
                ui,
                "Проверяется HTTPS. Для видео откройте ролик на YouTube.",
            );
        }
        if selected.telegram {
            ui.horizontal(|ui| {
                ui.strong("Telegram");
                hint(
                    ui,
                    if self.snap.as_ref().is_some_and(|s| s.telegram) {
                        "Прокси запущен"
                    } else {
                        "Отключено"
                    },
                );
            });
        }
        if selected.uses_zapret() {
            ui.add_enabled_ui(!busy, |ui| {
                if action(ui, "Полный подбор", false) {
                    self.connect(true);
                }
            });
        }
        section(ui, "История проверок", |ui| {
            if self.history.is_empty() {
                hint(ui, "Здесь появятся результаты подбора.");
            }
            for (name, detail) in self.history.iter().rev() {
                ui.label(name);
                hint(ui, detail);
                ui.separator();
            }
        });
        if let Some(duration) = self.snap.as_ref().and_then(|s| s.uptime) {
            hint(ui, format!("Подключено {} мин", duration.as_secs() / 60));
        }
    }

    pub(super) fn telegram(&mut self, ui: &mut Ui) {
        let active = self.snap.as_ref().is_some_and(|s| s.telegram);
        let busy = self.busy();
        card().show(ui, |ui| {
            ui.set_min_width(ui.available_width());
            title(
                ui,
                if active {
                    "Telegram подключён"
                } else {
                    "Telegram выключен"
                },
            );
            hint(
                ui,
                if self.tg_status.is_empty() {
                    "Встроенный прокси для Telegram.".to_owned()
                } else {
                    self.tg_status.clone()
                },
            );
            ui.add_enabled_ui(!busy, |ui| {
                if action(
                    ui,
                    if active {
                        "Остановить прокси"
                    } else {
                        "Запустить прокси"
                    },
                    true,
                ) {
                    self.send(Command::Telegram);
                }
            });
        });
        if active {
            if let Some(t) = self.snap.as_ref().map(|s| &s.state.telegram) {
                hint(ui, format!("Локальный адрес · 127.0.0.1:{}", t.port));
                match self.proxy_url.clone() {
                    Some(url) => {
                        if action(ui, "Открыть в Telegram", false) {
                            if let Err(e) = launch_proxy(&url) {
                                self.error = e.to_string();
                            }
                        }
                        if ui.button("Скопировать ссылку").clicked() {
                            ui.ctx().copy_text(url);
                            self.tg_status = "Ссылка скопирована.".into();
                        }
                    }
                    None => {
                        ui.colored_label(RED, "Не удалось подготовить ссылку локального прокси.");
                    }
                }
            }
            title(ui, "Проверка");
            hint(
                ui,
                if self.tg_status.is_empty() {
                    "Сервер ещё не проверен."
                } else {
                    self.tg_status.as_str()
                },
            );
            ui.add_enabled_ui(!busy, |ui| {
                if action(ui, "Проверить сервер", false) {
                    self.send(Command::ProbeTelegram);
                }
            });
        }
        section(ui, "Настройки Telegram", |ui| {
            ui.add_enabled_ui(!busy && !active, |ui| {
                ui.label("Локальный порт");
                ui.add(egui::TextEdit::singleline(&mut self.port).char_limit(5));
                ui.checkbox(&mut self.tg_start, "Запускать вместе со «Связью»");
                if action(ui, "Сохранить настройки", false) {
                    match self.port.trim().parse::<u16>().ok().filter(|v| *v != 0) {
                        Some(port) => self.send(Command::SaveTelegram {
                            port: port as i32,
                            start_with_app: self.tg_start,
                        }),
                        None => self.error = "Порт должен быть от 1 до 65535.".into(),
                    }
                }
            });
            hint(
                ui,
                format!(
                    "Движок Telegram {} · встроен в приложение.",
                    svyaz_telegram::VERSION
                ),
            );
        });
    }

    pub(super) fn settings(&mut self, ui: &mut Ui) {
        title(ui, "Настройки");
        card().show(ui, |ui| {
            ui.strong("Сервисы");
            ui.add_enabled_ui(!self.busy() && !self.connected(), |ui| {
                ui.checkbox(&mut self.services.discord, "Discord");
                ui.checkbox(&mut self.services.youtube, "YouTube");
                ui.checkbox(&mut self.services.telegram, "Telegram");
                if ui.button("Сохранить выбор").clicked() {
                    self.send(Command::SaveServices(self.services.clone(), self.quiet));
                }
            });
        });
        section(ui, "Запуск и обновления", |ui| {
            let mut autostart = self.snap.as_ref().is_some_and(|s| s.autostart);
            let mut update = self.snap.as_ref().is_none_or(|s| s.state.auto_update);
            ui.add_enabled_ui(!self.busy(), |ui| {
                if ui
                    .checkbox(&mut autostart, "Запускать при входе в Windows")
                    .changed()
                {
                    self.send(Command::Autostart(autostart));
                }
                if ui.checkbox(&mut update, "Автообновление").changed() {
                    self.send(Command::AutoUpdate(update));
                }
                if ui.checkbox(&mut self.quiet, "Тихие уведомления").changed() {
                    let applied = self
                        .snap
                        .as_ref()
                        .map(|s| s.prefs.effective_services(&s.state))
                        .unwrap_or_default();
                    self.send(Command::SaveServices(applied, self.quiet));
                }
                if action(ui, "Проверить обновление «Связи»", false) {
                    self.send(Command::CheckApp);
                }
                if let Some(version) = self
                    .snap
                    .as_ref()
                    .and_then(|s| s.previous.as_ref().map(|p| p.version.to_string()))
                {
                    if action(ui, &format!("Вернуться к версии {version}"), false) {
                        self.send(Command::Rollback);
                    }
                }
            });
            if ui.button("Что нового").clicked() {
                self.notes = !self.notes;
            }
            if self.notes {
                hint(
                    ui,
                    format!(
                        "{} · Перенос приложения на Rust. Настройки и стандартные ярлыки совместимы с 0.13.3.",
                        running()
                    ),
                );
            }
        });
        section(ui, "Подключение", |ui| {
            ui.label("Стратегия zapret");
            let strategies = self
                .snap
                .as_ref()
                .map(|s| s.strategies.clone())
                .unwrap_or_default();
            egui::ComboBox::from_id_salt("strategy")
                .selected_text(self.manual.as_deref().unwrap_or("Автоматически"))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.manual, None, "Автоматически");
                    for s in strategies {
                        ui.selectable_value(&mut self.manual, Some(s.clone()), s);
                    }
                });
            ui.label("Голосовой сервер Discord");
            ui.add(
                egui::TextEdit::singleline(&mut self.voice)
                    .hint_text("имя.discord.media:порт")
                    .char_limit(255),
            );
            hint(ui, "Оставьте поле пустым для поиска в журнале Discord.");
        });
        section(ui, "Игры", |ui| {
            const MODES: [&str; 4] = ["Выключен", "TCP и UDP", "Только TCP", "Только UDP"];
            ui.add_enabled_ui(
                !self.busy() && !self.snap.as_ref().is_some_and(|s| s.discord),
                |ui| {
                    egui::ComboBox::from_id_salt("filter")
                        .selected_text(
                            MODES
                                .get(self.game.mode as usize)
                                .copied()
                                .unwrap_or("Неизвестно"),
                        )
                        .show_ui(ui, |ui| {
                            for (i, t) in MODES.iter().enumerate() {
                                ui.selectable_value(&mut self.game.mode, i as i32, *t);
                            }
                        });
                    ui.label("Порты TCP");
                    ui.add(egui::TextEdit::singleline(&mut self.game.tcp_ports).char_limit(2048));
                    ui.label("Порты UDP");
                    ui.add(egui::TextEdit::singleline(&mut self.game.udp_ports).char_limit(2048));
                    if action(ui, "Сохранить Game Filter", false) {
                        self.send(Command::SaveGame(self.game.clone()));
                    }
                },
            );
        });
        section(ui, "Диагностика", |ui| {
            hint(
                ui,
                "В отчёт входят версия, состояние сервисов и результаты проверок.",
            );
            if action(ui, "Сохранить отчёт", false) {
                self.diagnostics();
            }
        });
        section(ui, "Обслуживание zapret", |ui| {
            if let Some(name) = self
                .snap
                .as_ref()
                .and_then(|s| s.state.last_strategy.as_deref())
            {
                hint(ui, format!("Последний выбранный альт · {name}"));
            }
            ui.add_enabled_ui(!self.busy(), |ui| {
                if action(ui, "Обновить альты", false) {
                    self.send(Command::CheckPackage);
                }
                if action(ui, "Сбросить кэш zapret", false) {
                    self.send(Command::InspectReset);
                }
            });
        });
        if !self.busy() && ui.button("Выйти из «Связи»").clicked() {
            self.send(Command::Quit);
        }
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
        let r = self.report.as_ref();
        let report = serde_json::json!({
            "AppVersion":running().to_string(),"Platform":"Windows",
            "DiscordProcessRunning":self.snap.as_ref().is_some_and(|s|s.discord),
            "DiscordVerified":self.snap.as_ref().is_some_and(|s|s.verified),
            "TelegramProcessRunning":self.snap.as_ref().is_some_and(|s|s.telegram),
            "ApiPassed":one(r.map(|r|&r.api)),"GatewayPassed":one(r.map(|r|&r.gateway)),"VoicePassed":one(r.map(|r|&r.voice)),
            "YouTubeSitePassed":one(r.map(|r|&r.youtube)),"YouTubeGoogleVideoPassed":one(r.map(|r|&r.google_video)),
            "LastSuccessfulCheck":self.last_verified,"TestedStrategies":self.scan_count
        });
        let saved = serde_json::to_vec_pretty(&report)
            .map_err(io::Error::other)
            .and_then(|b| fs::write(path, b));
        match saved {
            Ok(()) => self.status = "Отчёт сохранён.".into(),
            Err(e) => self.error = e.to_string(),
        }
    }
    pub(super) fn journal(&mut self, ui: &mut Ui) {
        title(ui, "Журнал");
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
        if self.journal.is_empty() {
            hint(
                ui,
                "Здесь появятся результаты проверок и сообщения приложения.",
            );
        } else {
            if ui.button("Скопировать журнал").clicked() {
                ui.ctx().copy_text(self.journal.clone());
            }
            hint(ui, "Выделите записи и нажмите Ctrl+C, чтобы скопировать.");
            ui.add(
                egui::Label::new(RichText::new(&self.journal).monospace().size(12.))
                    .wrap()
                    .selectable(true),
            );
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
                self.send(Command::Reset(approved));
            }
        }
    }
}
