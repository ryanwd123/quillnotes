//! User settings persisted as `key=value` lines in `%APPDATA%\Penmark\settings.ini`.

use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Default)]
pub struct Settings {
    values: BTreeMap<String, String>,
}

/// The settings folder, `%APPDATA%\Penmark`.
#[cfg(not(test))]
pub fn dir() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join(crate::app::APP_NAME))
}

/// Tests get a per-run temp folder so they never write to the user's real
/// settings, keymap or theme files.
#[cfg(test)]
pub fn dir() -> Option<PathBuf> {
    Some(std::env::temp_dir().join(format!("penmark-settings-{}", std::process::id())))
}

fn path() -> Option<PathBuf> {
    dir().map(|d| d.join("settings.ini"))
}

impl Settings {
    pub fn load() -> Self {
        let text = path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        Settings {
            values: parse(&text),
        }
    }

    /// Applies `f` to the settings on disk and saves them. Reloading first keeps
    /// changes made by other Penmark windows since this one started.
    pub fn update(f: impl FnOnce(&mut Settings)) -> std::io::Result<()> {
        let mut s = Settings::load();
        f(&mut s);
        s.save()
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    /// Stores `value`; call [`save`](Self::save) to write the file.
    pub fn set(&mut self, key: &str, value: &str) {
        self.values.insert(key.to_string(), value.to_string());
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = path().ok_or(std::io::ErrorKind::NotFound)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, serialize(&self.values))
    }
}

fn parse(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with(['#', ';']))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .collect()
}

fn serialize(values: &BTreeMap<String, String>) -> String {
    values.iter().map(|(k, v)| format!("{k}={v}\n")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut m = parse("# comment\nfont = Georgia \nbad line\n");
        assert_eq!(m.get("font").map(String::as_str), Some("Georgia"));
        assert_eq!(m.len(), 1);
        m.insert("theme".into(), "dark".into());
        assert_eq!(parse(&serialize(&m)), m);
    }
}
