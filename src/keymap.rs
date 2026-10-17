//! Keyboard shortcuts: the editor's commands, their default keys, and `keymap.ini`
//! in the settings folder, which overrides them.

use std::path::PathBuf;
use std::time::SystemTime;

use windows::Win32::UI::Input::KeyboardAndMouse::*;

use crate::palette::Command;
use crate::settings;
use crate::table::TableCmd;

/// The file name in the settings folder.
pub const FILE: &str = "keymap.ini";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Palette,
    QuickOpen,
    SearchFiles,
    RecentFolders,
    ToggleExplorer,
    ExplorerFocus,
    FocusExplorer,
    ToggleOutline,
    NewWindow,
    NewFile,
    OpenFile,
    Save,
    SaveAs,
    CloseFile,
    CloseWindow,
    Back,
    Forward,
    Undo,
    Redo,
    SelectAll,
    Copy,
    Cut,
    Paste,
    Find,
    Replace,
    FindNext,
    FindPrevious,
    GoToLine,
    SelectNextMatch,
    SelectLine,
    Bold,
    Italic,
    Code,
    Strikethrough,
    Highlight,
    Link,
    Heading(u8),
    BulletList,
    NumberedList,
    TaskList,
    Quote,
    ToggleTask,
    ToggleComment,
    Indent,
    Outdent,
    InsertLineBelow,
    InsertLineAbove,
    MoveLinesUp,
    MoveLinesDown,
    CopyLinesUp,
    CopyLinesDown,
    DeleteLines,
    ToggleSource,
    ToggleTheme,
    ZoomIn,
    ZoomOut,
    ZoomReset,
    /// A palette command with no action of its own.
    Run(Command),
}

/// Every action: (group, name in keymap.ini, action, default keys).
const ACTIONS: &[(&str, &str, Action, &str)] = &[
    (
        "Files and folders",
        "palette",
        Action::Palette,
        "F1, Ctrl+Shift+P",
    ),
    (
        "Files and folders",
        "quick_open",
        Action::QuickOpen,
        "Ctrl+P",
    ),
    (
        "Files and folders",
        "search_files",
        Action::SearchFiles,
        "Ctrl+Shift+F",
    ),
    (
        "Files and folders",
        "recent_folders",
        Action::RecentFolders,
        "Ctrl+R",
    ),
    (
        "Files and folders",
        "open_folder",
        Action::Run(Command::OpenFolder),
        "",
    ),
    (
        "Files and folders",
        "close_folder",
        Action::Run(Command::CloseFolder),
        "",
    ),
    (
        "Files and folders",
        "new_file_in_folder",
        Action::Run(Command::NewFile),
        "",
    ),
    (
        "Files and folders",
        "new_folder",
        Action::Run(Command::NewFolder),
        "",
    ),
    (
        "Files and folders",
        "toggle_explorer",
        Action::ToggleExplorer,
        "Ctrl+B",
    ),
    (
        "Files and folders",
        "explorer_focus",
        Action::ExplorerFocus,
        "Ctrl+Shift+E",
    ),
    (
        "Files and folders",
        "focus_explorer",
        Action::FocusExplorer,
        "Ctrl+0",
    ),
    (
        "Files and folders",
        "toggle_outline",
        Action::ToggleOutline,
        "Ctrl+Shift+O",
    ),
    (
        "Files and folders",
        "new_window",
        Action::NewWindow,
        "Ctrl+Shift+N",
    ),
    ("Files and folders", "new_file", Action::NewFile, "Ctrl+N"),
    ("Files and folders", "open_file", Action::OpenFile, "Ctrl+O"),
    ("Files and folders", "save", Action::Save, "Ctrl+S"),
    (
        "Files and folders",
        "save_as",
        Action::SaveAs,
        "Ctrl+Shift+S",
    ),
    (
        "Files and folders",
        "close_file",
        Action::CloseFile,
        "Ctrl+W",
    ),
    (
        "Files and folders",
        "daily_note",
        Action::Run(Command::DailyNote),
        "Alt+D",
    ),
    (
        "Files and folders",
        "previous_daily_note",
        Action::Run(Command::PreviousDailyNote),
        "",
    ),
    (
        "Files and folders",
        "next_daily_note",
        Action::Run(Command::NextDailyNote),
        "",
    ),
    (
        "Files and folders",
        "close_window",
        Action::CloseWindow,
        "Ctrl+Shift+W",
    ),
    (
        "Files and folders",
        "quit",
        Action::Run(Command::Quit),
        "Ctrl+Q",
    ),
    ("Files and folders", "back", Action::Back, "Alt+Left"),
    ("Files and folders", "forward", Action::Forward, "Alt+Right"),
    ("Editing", "undo", Action::Undo, "Ctrl+Z"),
    ("Editing", "redo", Action::Redo, "Ctrl+Y, Ctrl+Shift+Z"),
    ("Editing", "select_all", Action::SelectAll, "Ctrl+A"),
    ("Editing", "copy", Action::Copy, "Ctrl+C"),
    ("Editing", "cut", Action::Cut, "Ctrl+X"),
    ("Editing", "paste", Action::Paste, "Ctrl+V"),
    ("Editing", "find", Action::Find, "Ctrl+F"),
    ("Editing", "replace", Action::Replace, "Ctrl+H"),
    ("Editing", "find_next", Action::FindNext, "F3"),
    ("Editing", "find_previous", Action::FindPrevious, "Shift+F3"),
    ("Editing", "go_to_line", Action::GoToLine, "Ctrl+G"),
    (
        "Editing",
        "select_next_match",
        Action::SelectNextMatch,
        "Ctrl+D",
    ),
    ("Editing", "select_line", Action::SelectLine, "Ctrl+L"),
    ("Editing", "indent", Action::Indent, "Ctrl+]"),
    ("Editing", "outdent", Action::Outdent, "Ctrl+["),
    (
        "Editing",
        "insert_line_below",
        Action::InsertLineBelow,
        "Ctrl+Enter",
    ),
    (
        "Editing",
        "insert_line_above",
        Action::InsertLineAbove,
        "Ctrl+Shift+Enter",
    ),
    ("Editing", "move_lines_up", Action::MoveLinesUp, "Alt+Up"),
    (
        "Editing",
        "move_lines_down",
        Action::MoveLinesDown,
        "Alt+Down",
    ),
    (
        "Editing",
        "copy_lines_up",
        Action::CopyLinesUp,
        "Shift+Alt+Up",
    ),
    (
        "Editing",
        "copy_lines_down",
        Action::CopyLinesDown,
        "Shift+Alt+Down",
    ),
    (
        "Editing",
        "delete_lines",
        Action::DeleteLines,
        "Ctrl+Shift+K",
    ),
    ("Editing", "toggle_comment", Action::ToggleComment, "Ctrl+/"),
    ("Formatting", "bold", Action::Bold, "Ctrl+Shift+B"),
    ("Formatting", "italic", Action::Italic, "Ctrl+I"),
    ("Formatting", "code", Action::Code, "Ctrl+E"),
    (
        "Formatting",
        "strikethrough",
        Action::Strikethrough,
        "Ctrl+Shift+X",
    ),
    ("Formatting", "highlight", Action::Highlight, "Ctrl+Shift+H"),
    ("Formatting", "link", Action::Link, "Ctrl+K"),
    ("Formatting", "heading_1", Action::Heading(1), "Ctrl+1"),
    ("Formatting", "heading_2", Action::Heading(2), "Ctrl+2"),
    ("Formatting", "heading_3", Action::Heading(3), "Ctrl+3"),
    ("Formatting", "heading_4", Action::Heading(4), "Ctrl+4"),
    ("Formatting", "heading_5", Action::Heading(5), "Ctrl+5"),
    ("Formatting", "heading_6", Action::Heading(6), "Ctrl+6"),
    (
        "Formatting",
        "numbered_list",
        Action::NumberedList,
        "Ctrl+Shift+7",
    ),
    (
        "Formatting",
        "bullet_list",
        Action::BulletList,
        "Ctrl+Shift+8",
    ),
    ("Formatting", "task_list", Action::TaskList, "Ctrl+Shift+9"),
    ("Formatting", "quote", Action::Quote, "Ctrl+Shift+."),
    ("Formatting", "toggle_task", Action::ToggleTask, "Alt+C"),
    (
        "View",
        "toggle_source",
        Action::ToggleSource,
        "Ctrl+Shift+V",
    ),
    ("View", "toggle_theme", Action::ToggleTheme, "Ctrl+Shift+T"),
    (
        "View",
        "zoom_in",
        Action::ZoomIn,
        "Ctrl+=, Ctrl+Shift+=, Ctrl+Num+",
    ),
    (
        "View",
        "zoom_out",
        Action::ZoomOut,
        "Ctrl+-, Ctrl+Shift+-, Ctrl+Num-",
    ),
    ("View", "zoom_reset", Action::ZoomReset, "Ctrl+Num0"),
    (
        "Tables",
        "table_align_left",
        Action::Run(Command::Table(TableCmd::AlignLeft)),
        "",
    ),
    (
        "Tables",
        "table_align_center",
        Action::Run(Command::Table(TableCmd::AlignCenter)),
        "",
    ),
    (
        "Tables",
        "table_align_right",
        Action::Run(Command::Table(TableCmd::AlignRight)),
        "",
    ),
    (
        "Tables",
        "table_row_above",
        Action::Run(Command::Table(TableCmd::RowAbove)),
        "",
    ),
    (
        "Tables",
        "table_row_below",
        Action::Run(Command::Table(TableCmd::RowBelow)),
        "",
    ),
    (
        "Tables",
        "table_delete_row",
        Action::Run(Command::Table(TableCmd::DeleteRow)),
        "",
    ),
    (
        "Tables",
        "table_column_left",
        Action::Run(Command::Table(TableCmd::ColumnLeft)),
        "",
    ),
    (
        "Tables",
        "table_column_right",
        Action::Run(Command::Table(TableCmd::ColumnRight)),
        "",
    ),
    (
        "Tables",
        "table_delete_column",
        Action::Run(Command::Table(TableCmd::DeleteColumn)),
        "",
    ),
    (
        "Tables",
        "table_delete",
        Action::Run(Command::Table(TableCmd::DeleteTable)),
        "",
    ),
    (
        "Preferences",
        "folder_settings",
        Action::Run(Command::FolderSettings),
        "",
    ),
    (
        "Preferences",
        "change_font",
        Action::Run(Command::ChangeFont),
        "",
    ),
    (
        "Preferences",
        "change_code_font",
        Action::Run(Command::ChangeCodeFont),
        "",
    ),
    (
        "Preferences",
        "code_font_size",
        Action::Run(Command::CodeFontSize),
        "",
    ),
    (
        "Preferences",
        "ui_font_size",
        Action::Run(Command::UiFontSize),
        "",
    ),
    (
        "Preferences",
        "rendering_options",
        Action::Run(Command::RenderingOptions),
        "",
    ),
    (
        "Preferences",
        "open_settings_folder",
        Action::Run(Command::OpenSettingsFolder),
        "",
    ),
    (
        "Preferences",
        "write_keymap",
        Action::Run(Command::WriteKeymap),
        "",
    ),
    ("Preferences", "help", Action::Run(Command::Help), ""),
];

/// A key with its modifiers.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Chord {
    pub vk: u16,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

/// Keys with names other than their letter, digit or F-number. The first name for
/// a key is the one written; the others are accepted too.
const NAMED: &[(&str, VIRTUAL_KEY)] = &[
    ("Enter", VK_RETURN),
    ("Return", VK_RETURN),
    ("Tab", VK_TAB),
    ("Esc", VK_ESCAPE),
    ("Escape", VK_ESCAPE),
    ("Space", VK_SPACE),
    ("Up", VK_UP),
    ("Down", VK_DOWN),
    ("Left", VK_LEFT),
    ("Right", VK_RIGHT),
    ("Home", VK_HOME),
    ("End", VK_END),
    ("PageUp", VK_PRIOR),
    ("PageDown", VK_NEXT),
    ("Backspace", VK_BACK),
    ("Delete", VK_DELETE),
    ("Del", VK_DELETE),
    ("Insert", VK_INSERT),
    ("=", VK_OEM_PLUS),
    ("-", VK_OEM_MINUS),
    ("[", VK_OEM_4),
    ("]", VK_OEM_6),
    (";", VK_OEM_1),
    ("'", VK_OEM_7),
    (",", VK_OEM_COMMA),
    (".", VK_OEM_PERIOD),
    ("/", VK_OEM_2),
    ("\\", VK_OEM_5),
    ("`", VK_OEM_3),
    ("Num+", VK_ADD),
    ("Num-", VK_SUBTRACT),
    ("Num*", VK_MULTIPLY),
    ("Num/", VK_DIVIDE),
    ("Num.", VK_DECIMAL),
];

fn key_name(vk: u16) -> Option<String> {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => Some(char::from(vk as u8).to_string()),
        _ if (VK_NUMPAD0.0..=VK_NUMPAD9.0).contains(&vk) => {
            Some(format!("Num{}", vk - VK_NUMPAD0.0))
        }
        _ if (VK_F1.0..=VK_F24.0).contains(&vk) => Some(format!("F{}", vk - VK_F1.0 + 1)),
        _ => NAMED
            .iter()
            .find(|(_, k)| k.0 == vk)
            .map(|(n, _)| n.to_string()),
    }
}

fn parse_key(name: &str) -> Option<u16> {
    let upper = name.to_ascii_uppercase();
    let b = upper.as_bytes();
    if b.len() == 1 && b[0].is_ascii_alphanumeric() {
        return Some(b[0] as u16);
    }
    let number = |prefix: &str, max: u16| {
        upper
            .strip_prefix(prefix)
            .and_then(|n| n.parse::<u16>().ok())
            .filter(|&n| n <= max)
    };
    if let Some(n) = number("NUM", 9) {
        return Some(VK_NUMPAD0.0 + n);
    }
    if let Some(n) = number("F", 24).filter(|&n| n >= 1) {
        return Some(VK_F1.0 + n - 1);
    }
    NAMED
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, k)| k.0)
}

impl Chord {
    /// Parses keys such as `Ctrl+Shift+B` (case-insensitive, modifiers in any order).
    pub fn parse(s: &str) -> Option<Chord> {
        let mut c = Chord {
            vk: 0,
            ctrl: false,
            shift: false,
            alt: false,
        };
        // No key name has a space, so `Ctrl + B` is read as `Ctrl+B`.
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        let mut rest = s.as_str();
        loop {
            let lower = rest.to_ascii_lowercase();
            let flag = if lower.starts_with("ctrl+") {
                &mut c.ctrl
            } else if lower.starts_with("shift+") {
                &mut c.shift
            } else if lower.starts_with("alt+") {
                &mut c.alt
            } else {
                break;
            };
            *flag = true;
            rest = &rest[lower.find('+').unwrap() + 1..];
        }
        c.vk = parse_key(rest)?;
        Some(c)
    }

    pub fn label(&self) -> String {
        let mut s = String::new();
        for (on, name) in [
            (self.ctrl, "Ctrl+"),
            (self.shift, "Shift+"),
            (self.alt, "Alt+"),
        ] {
            if on {
                s.push_str(name);
            }
        }
        s.push_str(&key_name(self.vk).unwrap_or_else(|| format!("0x{:02X}", self.vk)));
        s
    }

    /// Whether the key also types a character (so a shortcut on it must drop it).
    pub fn types_char(&self) -> bool {
        // Space, digits, letters, the number pad, and the punctuation keys.
        !self.ctrl
            && !self.alt
            && matches!(
                self.vk,
                0x20 | 0x30..=0x39 | 0x41..=0x5A | 0x60..=0x6F | 0xBA..=0xC0 | 0xDB..=0xDE
            )
    }
}

/// Parses a comma-separated list of keys; an empty value means no keys.
fn parse_keys(s: &str) -> Result<Vec<Chord>, String> {
    s.split(',')
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(|k| Chord::parse(k).ok_or_else(|| format!("unknown key \"{k}\"")))
        .collect()
}

pub fn path() -> Option<PathBuf> {
    settings::dir().map(|d| d.join(FILE))
}

fn modified() -> Option<SystemTime> {
    std::fs::metadata(path()?).ok()?.modified().ok()
}

pub struct Keymap {
    /// Keys of each action, in `ACTIONS` order.
    keys: Vec<Vec<Chord>>,
    /// keymap.ini's modification time when it was read.
    stamp: Option<SystemTime>,
}

impl Keymap {
    pub fn defaults() -> Self {
        Keymap {
            keys: ACTIONS
                .iter()
                .map(|a| parse_keys(a.3).expect("default keys parse"))
                .collect(),
            stamp: None,
        }
    }

    /// The defaults with the commands listed in `text` (keymap.ini) rebound, plus a
    /// message for each line that could not be used.
    pub fn parse(text: &str) -> (Self, Vec<String>) {
        let mut map = Keymap::defaults();
        let mut errors = Vec::new();
        let mut set = vec![false; ACTIONS.len()];
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with([';', '#']) {
                continue;
            }
            let at = format!("{FILE} line {}", n + 1);
            let Some((name, value)) = line.split_once('=') else {
                errors.push(format!("{at}: expected command = keys"));
                continue;
            };
            let name = name.trim();
            let Some(i) = ACTIONS.iter().position(|a| a.1 == name) else {
                errors.push(format!("{at}: unknown command \"{name}\""));
                continue;
            };
            match parse_keys(value) {
                Ok(keys) => {
                    map.keys[i] = keys;
                    set[i] = true;
                }
                Err(e) => errors.push(format!("{at}: {e}")),
            }
        }
        // A key given to a command in the file is taken from commands left at their
        // defaults; two commands given the same key in the file are reported.
        for i in 0..ACTIONS.len() {
            for j in 0..ACTIONS.len() {
                if i == j || !set[i] {
                    continue;
                }
                let taken = map.keys[i].clone();
                if !set[j] {
                    map.keys[j].retain(|k| !taken.contains(k));
                } else if i < j
                    && let Some(k) = taken.iter().find(|k| map.keys[j].contains(k))
                {
                    errors.push(format!(
                        "{FILE}: {} is used by both {} and {}",
                        k.label(),
                        ACTIONS[i].1,
                        ACTIONS[j].1
                    ));
                }
            }
        }
        (map, errors)
    }

    /// Reads keymap.ini; without one, the defaults.
    pub fn load() -> (Self, Vec<String>) {
        let stamp = modified();
        let text = path()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        let (mut map, errors) = Keymap::parse(&text);
        map.stamp = stamp;
        (map, errors)
    }

    /// Reloads keymap.ini when it changed (or appeared or went away) since it was
    /// read, returning the problems found in it.
    pub fn reload_if_changed(&mut self) -> Option<Vec<String>> {
        if modified() == self.stamp {
            return None;
        }
        let (map, errors) = Keymap::load();
        *self = map;
        Some(errors)
    }

    pub fn action(&self, chord: Chord) -> Option<Action> {
        self.keys
            .iter()
            .position(|keys| keys.contains(&chord))
            .map(|i| ACTIONS[i].2)
    }

    /// The first key of `action`, for showing beside it (empty when it has none).
    /// The name of `action` in keymap.ini.
    #[cfg(test)]
    pub fn name(action: Action) -> Option<&'static str> {
        ACTIONS.iter().find(|a| a.2 == action).map(|a| a.1)
    }

    pub fn hint(&self, action: Action) -> String {
        ACTIONS
            .iter()
            .position(|a| a.2 == action)
            .and_then(|i| self.keys[i].first())
            .map(Chord::label)
            .unwrap_or_default()
    }

    /// keymap.ini text for these keys, with instructions.
    pub fn to_ini(&self) -> String {
        let mut s = String::from(
            "; Penmark keyboard shortcuts.\n\
             ;\n\
             ; Each line is `command = keys`. Separate several keys with commas, and leave\n\
             ; the keys empty to give a command no shortcut. Keys are written like\n\
             ; Ctrl+Shift+B, with the modifiers Ctrl, Shift and Alt, and the keys A-Z, 0-9,\n\
             ; F1-F24, Enter, Tab, Esc, Space, Up, Down, Left, Right, Home, End, PageUp,\n\
             ; PageDown, Backspace, Delete, Insert, the punctuation keys = - [ ] ; ' , . / \\ `\n\
             ; and the number pad keys Num0-Num9, Num+, Num-, Num*, Num/, Num.\n\
             ;\n\
             ; Commands with nothing after the = have no shortcut; add keys to give them one.\n\
             ; Commands left out of this file keep their default keys, and a key given to a\n\
             ; command here is taken from any command that has it by default. Changes apply\n\
             ; when you switch back to Penmark, or save this file in it. Delete this file to\n\
             ; go back to the defaults.\n\
             ;\n\
             ; Moving the caret and selecting (arrows, Home, End, Page Up/Down, with Ctrl and\n\
             ; Shift), Backspace, Delete, Enter, Tab, and the keys inside the palette, the\n\
             ; explorer and tables are not listed here and cannot be changed. Ctrl+Alt keys\n\
             ; are left to typing (AltGr) and cannot be used.\n",
        );
        let mut group = "";
        for (i, &(g, name, _, _)) in ACTIONS.iter().enumerate() {
            if g != group {
                group = g;
                s.push_str(&format!("\n; {g}\n"));
            }
            let keys: Vec<String> = self.keys[i].iter().map(Chord::label).collect();
            s.push_str(&format!("{name:<18} = {}\n", keys.join(", ")).replace(" \n", "\n"));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chord(s: &str) -> Chord {
        Chord::parse(s).unwrap()
    }

    #[test]
    fn chords_parse_and_print() {
        for s in [
            "Ctrl+Shift+B",
            "F1",
            "Shift+F3",
            "Shift+Alt+Up",
            "Ctrl+=",
            "Ctrl+Num+",
            "Ctrl+Num0",
            "Ctrl+]",
            "Ctrl+\\",
            "Ctrl+Shift+Enter",
        ] {
            assert_eq!(chord(s).label(), s);
        }
        assert_eq!(chord(" shift + ctrl+b").label(), "Ctrl+Shift+B");
        assert_eq!(chord("ctrl+escape").label(), "Ctrl+Esc");
        assert_eq!(chord("Ctrl+Shift+B").vk, VK_B.0);
        assert!(Chord::parse("Ctrl+Foo").is_none());
        assert!(Chord::parse("F25").is_none());
        assert!(Chord::parse("Ctrl+").is_none());
        assert!(chord("Q").types_char() && chord("Shift+/").types_char());
        assert!(!chord("F2").types_char() && !chord("Ctrl+Q").types_char());
    }

    #[test]
    fn defaults_and_overrides() {
        let d = Keymap::defaults();
        assert_eq!(d.action(chord("Ctrl+B")), Some(Action::ToggleExplorer));
        assert_eq!(d.action(chord("Ctrl+Shift+Z")), Some(Action::Redo));
        assert_eq!(d.action(chord("Ctrl+Shift+I")), None);
        assert_eq!(d.action(chord("Ctrl+Q")), Some(Action::Run(Command::Quit)));
        assert_eq!(d.hint(Action::Palette), "F1");

        let (m, errors) = Keymap::parse(
            "; comment\n\nbold = Ctrl+B\nredo =\nitalic = Ctrl+Foo\nnope = F2\njunk\n",
        );
        // Ctrl+B moves from the explorer (left at its default) to bold.
        assert_eq!(m.action(chord("Ctrl+B")), Some(Action::Bold));
        assert_eq!(m.action(chord("Ctrl+Shift+B")), None);
        assert_eq!(m.hint(Action::ToggleExplorer), "");
        assert_eq!(m.action(chord("Ctrl+Y")), None);
        // A bad line leaves the command at its default.
        assert_eq!(m.action(chord("Ctrl+I")), Some(Action::Italic));
        assert_eq!(
            errors,
            [
                "keymap.ini line 5: unknown key \"Ctrl+Foo\"",
                "keymap.ini line 6: unknown command \"nope\"",
                "keymap.ini line 7: expected command = keys",
            ]
        );

        let (_, errors) = Keymap::parse("bold = Ctrl+Q\nitalic = Ctrl+Q\n");
        assert_eq!(
            errors,
            ["keymap.ini: Ctrl+Q is used by both bold and italic"]
        );
    }

    #[test]
    fn ini_round_trips() {
        let d = Keymap::defaults();
        let text = d.to_ini();
        assert!(text.contains("\nbold               = Ctrl+Shift+B\n"));
        assert!(text.contains("\nzoom_in            = Ctrl+=, Ctrl+Shift+=, Ctrl+Num+\n"));
        let (m, errors) = Keymap::parse(&text);
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(m.keys, d.keys);
        // A command without keys is written with an empty value.
        let (m, _) = Keymap::parse("bold =");
        assert!(m.to_ini().contains("\nbold               =\n"));
    }
}
