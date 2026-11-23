use crate::invalid;
use regex::Regex;
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::LazyLock,
};
use svyaz_settings::GameFilterSettings;

pub fn arguments(
    script: &str,
    root: &Path,
    filter: &GameFilterSettings,
    ipset: Option<&Path>,
) -> io::Result<Vec<String>> {
    static START: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?im)^start[^\r\n]*?"%BIN%winws\.exe"[ \t]+"#).unwrap());
    static JOIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\^\s*\r?\n").unwrap());
    let start = START
        .find(script.trim_start_matches('\u{feff}'))
        .ok_or_else(|| invalid("В стратегии нет команды winws."))?;
    let command = JOIN
        .replace_all(&script.trim_start_matches('\u{feff}')[start.end()..], " ")
        .replace("^!", "!");
    let command = command.trim();
    if command.contains(['\r', '\n', '&', '|', '<', '>', '`', '^', '\0']) {
        return Err(invalid("Стратегия содержит неподдерживаемые команды."));
    }
    let [legacy, tcp, udp] = filter.ranges()?;
    let bin = format!(
        "{}{}",
        root.join("bin").display(),
        std::path::MAIN_SEPARATOR
    );
    let lists = format!(
        "{}{}",
        root.join("lists").display(),
        std::path::MAIN_SEPARATOR
    );
    let mut tokens = Vec::new();
    let mut token = String::new();
    let mut quoted = false;
    for c in command.chars() {
        if c == '"' {
            quoted = !quoted;
        } else if !quoted && c.is_whitespace() {
            if !token.is_empty() {
                tokens.push(std::mem::take(&mut token));
            }
        } else {
            token.push(c);
        }
    }
    if !token.is_empty() {
        tokens.push(token);
    }
    if quoted || tokens.is_empty() {
        return Err(invalid("Проверьте кавычки и параметры стратегии."));
    }
    tokens
        .into_iter()
        .map(|t| {
            if !t.starts_with("--") {
                return Err(invalid("Ожидался параметр winws."));
            }
            let mut rest = t.as_str();
            let mut resolved = String::new();
            while let Some((before, after)) = rest.split_once('%') {
                resolved.push_str(before);
                let (name, tail) = after
                    .split_once('%')
                    .ok_or_else(|| invalid("Некорректная переменная."))?;
                resolved.push_str(match name.to_ascii_uppercase().as_str() {
                    "BIN" => &bin,
                    "LISTS" => &lists,
                    "GAMEFILTER" => &legacy,
                    "GAMEFILTERTCP" => &tcp,
                    "GAMEFILTERUDP" => &udp,
                    _ => return Err(invalid("Неизвестная переменная стратегии.")),
                });
                rest = tail;
            }
            resolved.push_str(rest);
            if resolved.contains('%')
                || resolved
                    .split_once('=')
                    .is_some_and(|(_, value)| value.contains("../") || value.contains("..\\"))
            {
                return Err(invalid("Недопустимый путь в стратегии."));
            }
            if filter.mode != 0
                && resolved.eq_ignore_ascii_case(&format!("--ipset={lists}ipset-all.txt"))
            {
                if let Some(file) = ipset {
                    return Ok(format!("--ipset={}", std::path::absolute(file)?.display()));
                }
            }
            Ok(resolved)
        })
        .collect()
}

#[derive(Clone)]
pub struct Strategy {
    pub name: String,
    pub file: PathBuf,
}
pub fn load(root: &Path, mut log: impl FnMut(String)) -> io::Result<Vec<Strategy>> {
    let mut items = Vec::new();
    for ent in fs::read_dir(root)? {
        let ent = ent?;
        let file = ent.path();
        if !ent.file_type()?.is_file() {
            continue;
        }
        let name = ent.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        if !lower.starts_with("general") || !lower.ends_with(".bat") {
            continue;
        }
        match arguments(
            &String::from_utf8_lossy(&fs::read(&file)?),
            root,
            &Default::default(),
            None,
        ) {
            Ok(_) => items.push(Strategy {
                name: name[..name.len() - 4].to_owned(),
                file,
            }),
            Err(e) => log(format!("{name}: {e}")),
        }
    }
    // The Windows client compares filenames including ".bat", so general.bat follows general (ALT).bat.
    items.sort_by_cached_key(|s| format!("{}.bat", s.name.to_lowercase()));
    if items.is_empty() {
        return Err(invalid("В пакете нет совместимых стратегий."));
    }
    Ok(items)
}
