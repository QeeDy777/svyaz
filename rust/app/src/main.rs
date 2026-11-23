#![cfg_attr(windows, windows_subsystem = "windows")]
#[cfg(windows)]
mod connect;
#[cfg(windows)]
mod desktop;
#[cfg(any(windows, test))]
mod recovery;
#[cfg(windows)]
mod reset;
#[cfg(windows)]
mod runner;
#[cfg(any(windows, test))]
mod sound;
#[cfg(windows)]
mod tray;
#[cfg(windows)]
mod window_chrome;
#[cfg(windows)]
mod window_icons;
#[cfg(windows)]
mod worker;

#[cfg(windows)]
use std::io;
#[cfg(windows)]
use svyaz_update::{InstalledApp, Updater, Version};
#[cfg(windows)]
fn running() -> Version {
    Version(
        env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap(),
        env!("CARGO_PKG_VERSION_MINOR").parse().unwrap(),
        env!("CARGO_PKG_VERSION_PATCH").parse().unwrap(),
    )
}

#[cfg(windows)]
struct Restart {
    app: InstalledApp,
    rollback: bool,
    args: Vec<String>,
}

#[cfg(windows)]
fn launch() -> io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |s: &str| args.iter().any(|a| a == s);
    let install = svyaz_windows::install_root()?;
    let data = install.join("Data");
    let exe = std::env::current_exe()?;
    let maintenance = args.as_slice() == ["--install"] || args.as_slice() == ["--uninstall"];
    let token = args
        .iter()
        .position(|a| a == "--update-ready")
        .map(|i| {
            args.get(i + 1)
                .cloned()
                .ok_or_else(|| io::Error::other("Нет подтверждения обновления."))
        })
        .transpose()?;
    let Some(instance) = svyaz_windows::Instance::acquire()? else {
        return Err(io::Error::other(
            "«Связь» уже запущена. Откройте приложение через значок в трее.",
        ));
    };
    if maintenance {
        return if has("--install") {
            svyaz_windows::install_application(&install, &data, &exe, running())
        } else {
            svyaz_windows::uninstall_application(&install)
        };
    }
    std::fs::create_dir_all(&data)?;
    let updater = Updater::new(&install, &data, running());
    let state = svyaz_settings::AppState::load(&data.join(svyaz_settings::SETTINGS_FILE))?;
    let resume: Vec<String> = args
        .iter()
        .filter(|a| {
            matches!(
                a.as_str(),
                "--startup" | "--resume-discord" | "--resume-telegram"
            )
        })
        .cloned()
        .collect();
    let mut restart = None;
    if token.is_none() && !has("--skip-self-update") {
        match updater.select_for_launch(state.auto_update) {
            Ok(Some(selected)) => match updater.executable(&selected) {
                Ok(_) => {
                    restart = Some(Restart {
                        rollback: selected.version < running(),
                        app: selected,
                        args: resume.clone(),
                    })
                }
                Err(e) => worker::log(&data, &e.to_string()),
            },
            Err(e) => worker::log(&data, &e.to_string()),
            _ => {}
        }
    }
    let legacy_parent = token.is_some() && !has(svyaz_windows::PARENT_REFRESHES_ICONS);
    if restart.is_none() {
        restart = desktop::show(install.clone(), data.clone(), resume, token, legacy_parent)?;
    }
    drop(instance);
    if let Some(target) = restart {
        let handoff = target.args.clone();
        let result = updater.executable(&target.app).and_then(|new_exe| {
            svyaz_windows::start_confirmed(
                &new_exe,
                &handoff,
                std::time::Duration::from_secs(30),
                || updater.promote(&target.app, target.rollback),
            )?;
            svyaz_windows::refresh_shortcuts(&install, &new_exe);
            Ok(())
        });
        if let Err(e) = result {
            let _ = updater.reject(&target.app);
            worker::log(&data, &format!("Ошибка запуска обновления: {e}"));
            std::process::Command::new(exe)
                .arg("--skip-self-update")
                .args(target.args)
                .spawn()?;
            return Err(e);
        }
    }
    Ok(())
}
#[cfg(windows)]
fn main() {
    if let Err(error) = launch() {
        if let Ok(root) = svyaz_windows::install_root() {
            let _ = std::fs::write(root.join("Data/startup-error.log"), error.to_string());
        }
        if !std::env::args()
            .any(|s| matches!(s.as_str(), "--install" | "--uninstall" | "--update-ready"))
        {
            use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MessageBoxW};
            let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
            unsafe {
                MessageBoxW(
                    std::ptr::null_mut(),
                    wide(&error.to_string()).as_ptr(),
                    wide("Связь").as_ptr(),
                    MB_ICONERROR,
                );
            }
        }
        std::process::exit(1);
    }
}
#[cfg(not(windows))]
fn main() {
    eprintln!("«Связь» предназначена для Windows x64.");
    std::process::exit(1);
}
