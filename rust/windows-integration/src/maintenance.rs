use super::{refresh_shortcuts, tasks::visit_tasks};
use std::{
    fs, io,
    path::{Path, PathBuf},
};
use svyaz_update::{
    file_version, identity, install, InstalledApp, State, Updater, Version, RUNTIME_FILES,
};

fn repair(source: &Path, root: &Path) -> io::Result<(InstalledApp, PathBuf)> {
    let from = source
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Нет каталога приложения."))?;
    let item = InstalledApp {
        version: file_version(source)?,
        id: identity(from)?,
    };
    let versions = root.join("Versions");
    let destination = versions.join(&item.id);
    let mut backup = None;
    if destination.try_exists()? && identity(&destination).ok().as_deref() != Some(item.id.as_str())
    {
        let directory = tempfile::Builder::new()
            .prefix(".repair-")
            .tempdir_in(&versions)?;
        let saved = directory.path().join("original");
        fs::rename(&destination, &saved)?;
        backup = Some((directory, saved));
    }
    match install(source, root) {
        Ok(path) => Ok((item, path)),
        Err(e) => {
            if let Some((dir, saved)) = backup {
                if let Err(restore) = fs::rename(&saved, &destination) {
                    let kept = dir.keep();
                    return Err(io::Error::new(
                        restore.kind(),
                        format!(
                            "Не удалось восстановить версию: {restore}. Резервная копия: {}",
                            kept.display()
                        ),
                    ));
                }
            }
            Err(e)
        }
    }
}
pub fn install_application(
    root: &Path,
    data: &Path,
    source: &Path,
    running: Version,
) -> io::Result<()> {
    let updater = Updater::new(root, data, running);
    let selected = match updater
        .read(State::Current)?
        .filter(|a| a.version > running)
    {
        Some(current) => updater.executable(&current)?,
        None => {
            if source.file_name().and_then(|s| s.to_str()) != Some(RUNTIME_FILES[0])
                || file_version(source)? != running
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "Версия установщика не соответствует приложению.",
                ));
            }
            let (item, path) = repair(source, root)?;
            updater.promote(&item, false)?;
            path
        }
    };
    visit_tasks(root, Some(&selected))?;
    refresh_shortcuts(root, &selected);
    Ok(())
}
pub fn uninstall_application(root: &Path) -> io::Result<()> {
    visit_tasks(root, None)
}
