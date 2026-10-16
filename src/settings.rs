//! User settings persisted as `key=value` lines in `%APPDATA%\Penmark\settings.ini`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::app::APP_NAME;

#[derive(Default)]
pub struct Settings {
    values: BTreeMap<String, String>,
}

fn path() -> Option<PathBuf> {
    let appdata = std::env::var_os("APPDATA")?;
    Some(PathBuf::from(appdata).join(APP_NAME).join("settings.ini"))
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

    pub fn get(&self, key: &str) -> Option<&str> {
        self.values.get(key).map(String::as_str)
    }

    /// Stores `value` and writes the file.
    pub fn set(&mut self, key: &str, value: &str) -> std::io::Result<()> {
        self.values.insert(key.to_string(), value.to_string());
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
