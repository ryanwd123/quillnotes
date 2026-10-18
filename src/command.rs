//! Command links: `[label](<run:program args>)` launches a program directly and
//! `[label](<shell:...>)` hands the text to `cmd`. Options go in the link title as
//! `key=value` pairs separated by `;`, e.g. `"cwd=%USERPROFILE%\proj; window=keep"`.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// Launch the program directly, after expanding `%VARS%`.
    Run,
    /// Run the text with `cmd /c`, which expands `%VARS%` itself.
    Shell,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Window {
    /// No console window.
    Hidden,
    /// A console window that closes when the command ends.
    Show,
    /// Run through `cmd /k`, so the console stays open with the output.
    Keep,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Command {
    pub mode: Mode,
    /// The command text after the `run:` / `shell:` prefix.
    pub text: String,
    /// Working directory option, unexpanded.
    pub cwd: Option<String>,
    pub window: Window,
    /// Track the process and report its exit code.
    pub wait: bool,
    /// Always ask before running, even once trusted.
    pub confirm: bool,
}

/// The glyph a command link shows before its label.
pub fn glyph(target: &str) -> Option<&'static str> {
    if target.starts_with("run:") {
        Some("\u{25b6} ")
    } else if target.starts_with("shell:") {
        Some("$ ")
    } else {
        None
    }
}

/// Parses a command link's target and title, or None for other links.
pub fn parse(target: &str, title: &str) -> Option<Command> {
    let (mode, text) = if let Some(t) = target.strip_prefix("run:") {
        (Mode::Run, t)
    } else {
        (Mode::Shell, target.strip_prefix("shell:")?)
    };
    let mut c = Command {
        mode,
        text: text.trim().to_string(),
        cwd: None,
        // A shell command usually runs a script or `start`s a program: no console.
        window: if mode == Mode::Run {
            Window::Show
        } else {
            Window::Hidden
        },
        wait: false,
        confirm: false,
    };
    for opt in title.split(';').map(str::trim).filter(|o| !o.is_empty()) {
        let (key, value) = opt.split_once('=').unwrap_or((opt, ""));
        match (key.trim().to_ascii_lowercase().as_str(), value.trim()) {
            ("cwd", v) if !v.is_empty() => c.cwd = Some(v.to_string()),
            ("window", "hidden") => c.window = Window::Hidden,
            ("window", "show") => c.window = Window::Show,
            ("window", "keep") => c.window = Window::Keep,
            ("wait", _) => c.wait = true,
            ("detach", _) => c.wait = false,
            ("confirm", _) => c.confirm = true,
            _ => {}
        }
    }
    Some(c)
}

/// Splits off the program: (program, byte offset where the arguments start). Follows
/// the rules for a command line's first token: quoted up to the next quote, else up
/// to whitespace.
fn split_program(cmd: &str) -> (&str, usize) {
    if let Some(rest) = cmd.strip_prefix('"') {
        let end = rest.find('"').map_or(cmd.len(), |i| i + 1);
        (&cmd[1..end], (end + 1).min(cmd.len()))
    } else {
        let end = cmd.find(char::is_whitespace).unwrap_or(cmd.len());
        (&cmd[..end], end)
    }
}

/// A relative program path resolved against `cwd`, when such a file exists there
/// (with or without `.exe`). Bare names otherwise stay for the PATH search.
fn resolve_program(program: &str, cwd: &Path) -> Option<PathBuf> {
    let p = Path::new(program);
    if p.is_absolute() {
        return None;
    }
    [cwd.join(p), cwd.join(format!("{program}.exe"))]
        .into_iter()
        .find(|c| c.is_file())
}

/// The full command line to launch, from the command text (already expanded for
/// `run:`), running in `cwd`.
pub fn command_line(c: &Command, text: &str, cwd: &Path, comspec: &str) -> String {
    let line = match c.mode {
        Mode::Run => {
            let (program, args) = split_program(text);
            match resolve_program(program, cwd) {
                Some(p) => format!("\"{}\"{}", p.display(), &text[args..]),
                None => text.to_string(),
            }
        }
        Mode::Shell => text.to_string(),
    };
    let flag = if c.window == Window::Keep { "/k" } else { "/c" };
    match (c.mode, c.window) {
        (Mode::Run, Window::Keep) | (Mode::Shell, _) => {
            format!("\"{comspec}\" /d /s {flag} \"{line}\"")
        }
        (Mode::Run, _) => line,
    }
}

/// A stable key for a command as it will run, so a link that changes asks again.
pub fn trust_key(line: &str, cwd: &Path) -> String {
    // FNV-1a: std's hasher is not guaranteed stable across Rust versions.
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in line.bytes().chain([0]).chain(cwd.to_string_lossy().bytes()) {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_targets_and_options() {
        let c = parse(
            "run:python.exe x.py",
            "cwd=%USERPROFILE%\\proj; window=keep; confirm",
        )
        .unwrap();
        assert_eq!(c.mode, Mode::Run);
        assert_eq!(c.text, "python.exe x.py");
        assert_eq!(c.cwd.as_deref(), Some("%USERPROFILE%\\proj"));
        assert_eq!((c.window, c.wait, c.confirm), (Window::Keep, false, true));
        let c = parse("shell:make > log 2>&1", "wait").unwrap();
        assert_eq!(
            (c.mode, c.window, c.wait),
            (Mode::Shell, Window::Hidden, true)
        );
        assert!(parse("https://x", "").is_none());
        assert_eq!(glyph("shell:x"), Some("$ "));
        assert_eq!(glyph("x.md"), None);
    }

    #[test]
    fn builds_command_lines() {
        let dir = std::env::temp_dir().join(format!("quillnotes-cmd-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("bin")).unwrap();
        std::fs::write(dir.join("bin").join("tool.exe"), "").unwrap();
        let run = |text: &str, title: &str| {
            let c = parse(&format!("run:{text}"), title).unwrap();
            command_line(&c, &c.text, &dir, "C:\\cmd.exe")
        };
        // A program next to the note is found with or without `.exe`.
        let tool = dir.join("bin").join("tool.exe");
        assert_eq!(
            run("bin\\tool a b", ""),
            format!("\"{}\" a b", tool.display())
        );
        assert_eq!(
            run("\"bin\\tool.exe\" \"a b\"", ""),
            format!("\"{}\" \"a b\"", tool.display())
        );
        assert_eq!(run("notepad.exe x.txt", ""), "notepad.exe x.txt");
        assert_eq!(
            run("notepad.exe", "window=keep"),
            "\"C:\\cmd.exe\" /d /s /k \"notepad.exe\""
        );
        let c = parse("shell:start \"\" \"x\"", "").unwrap();
        assert_eq!(
            command_line(&c, &c.text, &dir, "cmd.exe"),
            "\"cmd.exe\" /d /s /c \"start \"\" \"x\"\""
        );
        assert_eq!(split_program("\"a b\\c.exe\" x"), ("a b\\c.exe", 11));
        assert_ne!(trust_key("a", &dir), trust_key("b", &dir));
        assert_eq!(trust_key("a", &dir), trust_key("a", &dir));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
