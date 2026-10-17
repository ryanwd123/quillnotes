//! Colors: the built-in light and dark palettes, overridden by `theme.ini` in the
//! settings folder.

use std::path::PathBuf;
use std::time::SystemTime;

use crate::render::{DARK, INK_COUNT, INK_NAMES, LIGHT};
use crate::settings;

pub const FILE: &str = "theme.ini";

pub fn path() -> Option<PathBuf> {
    settings::dir().map(|d| d.join(FILE))
}

fn modified() -> Option<SystemTime> {
    std::fs::metadata(path()?).ok()?.modified().ok()
}

pub struct Theme {
    /// ARGB colors, in `Ink` order.
    pub light: [u32; INK_COUNT],
    pub dark: [u32; INK_COUNT],
    /// theme.ini's modification time when it was read.
    stamp: Option<SystemTime>,
}

/// `#RRGGBB` or `#AARRGGBB` as ARGB.
fn parse_color(v: &str) -> Option<u32> {
    let hex = v.strip_prefix('#')?;
    let n = u32::from_str_radix(hex, 16).ok()?;
    match hex.len() {
        6 => Some(0xFF00_0000 | n),
        8 => Some(n),
        _ => None,
    }
}

fn color_text(c: u32) -> String {
    if c >> 24 == 0xFF {
        format!("#{:06X}", c & 0xFF_FFFF)
    } else {
        format!("#{c:08X}")
    }
}

impl Theme {
    pub fn defaults() -> Self {
        Theme {
            light: LIGHT,
            dark: DARK,
            stamp: None,
        }
    }

    /// The defaults with the colors in `text` (theme.ini) changed, and the problems
    /// found in it.
    pub fn parse(text: &str) -> (Self, Vec<String>) {
        let mut t = Theme::defaults();
        let mut errors = Vec::new();
        let mut section: Option<bool> = None; // Some(dark)
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with(';') {
                continue;
            }
            let err = |msg: String| format!("{FILE} line {}: {msg}", n + 1);
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = match name.trim().to_ascii_lowercase().as_str() {
                    "light" => Some(false),
                    "dark" => Some(true),
                    other => {
                        errors.push(err(format!("unknown section [{other}]")));
                        None
                    }
                };
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                errors.push(err("expected name = #RRGGBB".into()));
                continue;
            };
            let key = key.trim().to_ascii_lowercase();
            let Some(i) = INK_NAMES.iter().position(|&k| k == key) else {
                errors.push(err(format!("unknown color \"{key}\"")));
                continue;
            };
            let Some(c) = parse_color(value.trim()) else {
                errors.push(err(format!(
                    "\"{}\" is not #RRGGBB or #AARRGGBB",
                    value.trim()
                )));
                continue;
            };
            match section {
                Some(true) => t.dark[i] = c,
                Some(false) => t.light[i] = c,
                None => errors.push(err("a color must be under [light] or [dark]".into())),
            }
        }
        (t, errors)
    }

    /// Reads theme.ini; without one, the defaults.
    pub fn load() -> (Self, Vec<String>) {
        let stamp = modified();
        let text = path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        let (mut t, errors) = Theme::parse(&text);
        t.stamp = stamp;
        (t, errors)
    }

    /// Reloads theme.ini when it changed (or appeared or went away) since it was read,
    /// returning the problems found in it.
    pub fn reload_if_changed(&mut self) -> Option<Vec<String>> {
        if modified() == self.stamp {
            return None;
        }
        let (t, errors) = Theme::load();
        *self = t;
        Some(errors)
    }

    /// theme.ini text for these colors, with instructions.
    pub fn to_ini(&self) -> String {
        let mut out = String::from(
            "; Penmark colors. Each line is name = #RRGGBB, or #AARRGGBB with an alpha\n\
             ; (00 transparent, FF opaque) first. Colors left out, or this file deleted,\n\
             ; go back to the defaults. Changes apply when you switch back to Penmark, or\n\
             ; when this file is saved in Penmark. Ctrl+Shift+T switches light and dark.\n",
        );
        for (title, colors) in [("light", &self.light), ("dark", &self.dark)] {
            out.push_str(&format!("\n[{title}]\n"));
            for (name, &c) in INK_NAMES.iter().zip(colors.iter()) {
                out.push_str(&format!("{name} = {}\n", color_text(c)));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_writes() {
        let (t, errors) = Theme::parse(
            "; comment\n[dark]\nbg = #102030\nselection = #80FF0000\n\
             [light]\nText = #000000\nnope = #000000\nbg = red\n[other]\n",
        );
        assert_eq!(t.dark[0], 0xFF10_2030);
        assert_eq!(t.dark[8], 0x80FF_0000);
        assert_eq!(t.light[1], 0xFF00_0000);
        assert_eq!(t.light[0], LIGHT[0]);
        assert_eq!(
            errors,
            [
                "theme.ini line 7: unknown color \"nope\"",
                "theme.ini line 8: \"red\" is not #RRGGBB or #AARRGGBB",
                "theme.ini line 9: unknown section [other]",
            ]
        );
        // Written colors read back the same.
        let (back, errors) = Theme::parse(&t.to_ini());
        assert!(errors.is_empty());
        assert_eq!((back.light, back.dark), (t.light, t.dark));
        assert!(t.to_ini().contains("\nselection = #80FF0000\n"));
    }
}
