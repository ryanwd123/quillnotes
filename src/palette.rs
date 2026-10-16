//! Command palette state: the query, term filtering, and selection.

use std::path::{Path, PathBuf};

use crate::doc;
use crate::options::{Opt, RenderOptions};
use crate::render::PALETTE_ROWS;
use crate::table::TableCmd;
use crate::workspace::{self, Entry, FolderOpt, FolderSettings};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    QuickOpen,
    OpenFolder,
    OpenRecent,
    CloseFolder,
    NewFile,
    NewFolder,
    ToggleExplorer,
    ToggleOutline,
    FolderSettings,
    ChangeFont,
    ChangeCodeFont,
    CodeFontSize,
    UiFontSize,
    RenderingOptions,
    ToggleSource,
    ToggleTheme,
    Help,
    Table(TableCmd),
}

/// Every command, in the order shown for an empty query: (label, shortcut hint, command).
const COMMANDS: &[(&str, &str, Command)] = &[
    ("Go to File\u{2026}", "Ctrl+P", Command::QuickOpen),
    ("File: Open Folder\u{2026}", "", Command::OpenFolder),
    (
        "File: Open Recent Folder\u{2026}",
        "Ctrl+R",
        Command::OpenRecent,
    ),
    ("File: Close Folder", "", Command::CloseFolder),
    ("File: New File in Folder\u{2026}", "", Command::NewFile),
    ("File: New Folder\u{2026}", "", Command::NewFolder),
    (
        "View: Toggle Explorer",
        "Ctrl+Shift+E",
        Command::ToggleExplorer,
    ),
    (
        "View: Toggle Outline",
        "Ctrl+Shift+O",
        Command::ToggleOutline,
    ),
    ("Preferences: Folder Settings", "", Command::FolderSettings),
    ("Font: Change Editor Font", "", Command::ChangeFont),
    ("Font: Change Code Font", "", Command::ChangeCodeFont),
    ("Font: Code Font Size", "", Command::CodeFontSize),
    ("Preferences: UI Font Size", "", Command::UiFontSize),
    ("View: Rendering Options", "", Command::RenderingOptions),
    (
        "View: Toggle Source Mode",
        "Ctrl+Shift+V",
        Command::ToggleSource,
    ),
    (
        "Preferences: Toggle Light/Dark Theme",
        "Ctrl+Shift+T",
        Command::ToggleTheme,
    ),
    ("Help: Keyboard Shortcuts", "", Command::Help),
];

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Target {
    Command(Command),
    Font(String),
    Option(Opt),
    ResetOptions,
    OpenFile(PathBuf),
    SwitchFolder(PathBuf),
    OpenFolderDialog,
    FolderOpt(FolderOpt),
}

pub struct Item {
    pub label: String,
    pub detail: String,
    /// Extra text matched by the search but not highlighted (e.g. a file's folder).
    pub keywords: String,
    pub target: Target,
    /// For fonts: whether to draw the label in the font itself.
    pub preview: bool,
}

impl Item {
    fn new(label: impl Into<String>, detail: impl Into<String>, target: Target) -> Self {
        Item {
            label: label.into(),
            detail: detail.into(),
            keywords: String::new(),
            target,
            preview: false,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Commands,
    Fonts,
    CodeFonts,
    Options,
    Files,
    Folders,
    FolderSettings,
}

pub struct Palette {
    pub mode: Mode,
    pub query: String,
    /// Caret byte offset in `query`.
    pub cur: usize,
    items: Vec<Item>,
    /// Matching item indices with the matched byte offsets of each label.
    filtered: Vec<(usize, Vec<usize>)>,
    /// Index into `filtered`.
    selected: usize,
    /// First visible index into `filtered`.
    first: usize,
    /// Font to restore when the font picker is cancelled.
    pub restore_font: Option<String>,
}

/// `path` shortened from the left to about `max` characters, e.g. `…\work\notes`.
fn short_path(path: &str, max: usize) -> String {
    let n = path.chars().count();
    if n <= max {
        return path.to_string();
    }
    let tail: String = path.chars().skip(n - (max - 1)).collect();
    // Start at a separator when there is one, so no folder name is cut in half.
    let tail = tail.find('\\').map_or(tail.as_str(), |i| &tail[i..]);
    format!("\u{2026}{tail}")
}

fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// Splits `query` into whitespace-separated terms; `label` matches when it contains
/// every term (case-insensitive, in any order). Returns a score (higher is better)
/// and the byte offsets of the matched characters in `label`.
pub fn term_match(query: &str, label: &str) -> Option<(i32, Vec<usize>)> {
    let chars: Vec<(usize, char)> = label.char_indices().collect();
    let folded: Vec<char> = chars.iter().map(|&(_, c)| fold(c)).collect();
    let word_start = |k: usize| {
        k == 0 || {
            let (p, c) = (chars[k - 1].1, chars[k].1);
            !p.is_alphanumeric() || (p.is_lowercase() && c.is_uppercase())
        }
    };
    let mut score = 0i32;
    let mut hits = Vec::new();
    for term in query.split_whitespace() {
        let t: Vec<char> = term.chars().map(fold).collect();
        if t.len() > folded.len() {
            return None;
        }
        let found: Vec<usize> = (0..=folded.len() - t.len())
            .filter(|&k| folded[k..k + t.len()] == t[..])
            .collect();
        // Highlight and score the word-start occurrence when there is one.
        let &k = found.iter().find(|&&k| word_start(k)).or(found.first())?;
        score += if word_start(k) { 10 } else { 0 } + if k == 0 { 10 } else { 0 };
        score -= (k as i32).min(20) / 4;
        hits.extend(chars[k..k + t.len()].iter().map(|&(b, _)| b));
    }
    hits.sort_unstable();
    hits.dedup();
    Some((score, hits))
}

impl Palette {
    fn new(mode: Mode, items: Vec<Item>) -> Self {
        let mut p = Palette {
            mode,
            query: String::new(),
            cur: 0,
            items,
            filtered: Vec::new(),
            selected: 0,
            first: 0,
            restore_font: None,
        };
        p.refilter();
        p
    }

    pub fn commands() -> Self {
        let mut items: Vec<Item> = COMMANDS
            .iter()
            .map(|&(label, detail, cmd)| Item::new(label, detail, Target::Command(cmd)))
            .collect();
        items.extend(TableCmd::ALL.iter().map(|&(cmd, label)| {
            let hint = if cmd == TableCmd::RowBelow {
                "Ctrl+Enter"
            } else {
                ""
            };
            Item::new(
                format!("Table: {label}"),
                hint,
                Target::Command(Command::Table(cmd)),
            )
        }));
        Palette::new(Mode::Commands, items)
    }

    /// A picker over `families` (name, can draw its own name, monospaced), starting
    /// on `current`. The code font picker lists monospaced fonts first.
    pub fn fonts(mut families: Vec<(String, bool, bool)>, current: &str, code: bool) -> Self {
        if code {
            families.sort_by_key(|f| !f.2);
        }
        let items = families
            .into_iter()
            .map(|(name, preview, mono)| {
                let detail = match (name == current, preview) {
                    (true, _) => "Current",
                    (false, false) => "Symbols",
                    _ if code && mono => "Monospace",
                    _ => "",
                };
                Item {
                    preview,
                    ..Item::new(name.clone(), detail, Target::Font(name))
                }
            })
            .collect();
        let mode = if code { Mode::CodeFonts } else { Mode::Fonts };
        let mut p = Palette::new(mode, items);
        p.restore_font = Some(current.to_string());
        if let Some(i) = p
            .filtered
            .iter()
            .position(|(i, _)| p.items[*i].label == current)
        {
            p.select(i);
        }
        p
    }

    /// The rendering options list, with values from `o`, filtered by `query`.
    pub fn options(o: &RenderOptions, query: &str) -> Self {
        let mut items: Vec<Item> = Opt::all()
            .into_iter()
            .map(|opt| Item::new(opt.label(), opt.value_text(o), Target::Option(opt)))
            .collect();
        items.push(Item::new(
            "Reset rendering options to defaults",
            "",
            Target::ResetOptions,
        ));
        let mut p = Palette::new(Mode::Options, items);
        p.insert(query);
        p
    }

    /// Quick open: the Penmark-readable files under `root`, by name, with their
    /// folder shown and searchable.
    pub fn files(root: &Path, entries: &[Entry], current: Option<&Path>) -> Self {
        let mut items: Vec<Item> = entries
            .iter()
            .filter(|e| !e.is_dir && workspace::is_text(&e.path))
            .map(|e| {
                let folder = e
                    .path
                    .parent()
                    .map(|p| workspace::relative(root, p))
                    .unwrap_or_default();
                let detail = if current == Some(e.path.as_path()) {
                    if folder.is_empty() {
                        "open".to_string()
                    } else {
                        format!("{folder}  \u{b7}  open")
                    }
                } else {
                    folder.clone()
                };
                Item {
                    keywords: folder,
                    ..Item::new(e.name.clone(), detail, Target::OpenFile(e.path.clone()))
                }
            })
            .collect();
        items.sort_by(|a, b| {
            a.label
                .to_lowercase()
                .cmp(&b.label.to_lowercase())
                .then(a.keywords.cmp(&b.keywords))
        });
        Palette::new(Mode::Files, items)
    }

    /// Recent folders, with "Open Folder..." first.
    pub fn folders(recent: &[PathBuf], current: Option<&Path>) -> Self {
        let same = |a: &Path, b: &Path| workspace::contains(a, b) && workspace::contains(b, a);
        let mut items = vec![Item::new(
            "Open Folder\u{2026}",
            "",
            Target::OpenFolderDialog,
        )];
        for path in recent {
            let name = path.file_name().map_or_else(
                || path.to_string_lossy().into_owned(),
                |n| n.to_string_lossy().into_owned(),
            );
            let parent = path
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let shown = short_path(&parent, 40);
            let detail = if current.is_some_and(|c| same(c, path)) {
                format!("{shown}  \u{b7}  current")
            } else {
                shown
            };
            items.push(Item {
                keywords: parent,
                ..Item::new(name, detail, Target::SwitchFolder(path.clone()))
            });
        }
        let mut p = Palette::new(Mode::Folders, items);
        // Start on the most recent folder other than the current one.
        let first_is_current = recent
            .first()
            .is_some_and(|f| current.is_some_and(|c| same(c, f)));
        let start = if first_is_current { 2 } else { 1 };
        p.select(start.min(p.filtered.len().saturating_sub(1)));
        p
    }

    /// The open folder's settings.
    pub fn folder_settings(f: &FolderSettings) -> Self {
        let items = FolderOpt::ALL
            .iter()
            .map(|&opt| Item::new(opt.label(), opt.value_text(f), Target::FolderOpt(opt)))
            .collect();
        Palette::new(Mode::FolderSettings, items)
    }

    pub fn refresh_folder_values(&mut self, f: &FolderSettings) {
        for item in &mut self.items {
            if let Target::FolderOpt(opt) = item.target {
                item.detail = opt.value_text(f);
            }
        }
    }

    /// Refreshes option values after a change, keeping the query and selection.
    pub fn refresh_values(&mut self, o: &RenderOptions) {
        for item in &mut self.items {
            if let Target::Option(opt) = item.target {
                item.detail = opt.value_text(o);
            }
        }
    }

    pub fn placeholder(&self) -> &'static str {
        match self.mode {
            Mode::Commands => "Type a command",
            Mode::Fonts => "Choose the editor font",
            Mode::CodeFonts => "Choose the font for code",
            Mode::Options => "Filter options  ·  ←/→ adjust (Shift for more)  ·  Enter toggles",
            Mode::Files => "Go to file by name (add a folder name to narrow it down)",
            Mode::Folders => "Open a recent folder",
            Mode::FolderSettings => "Folder settings  ·  ←/→ adjust  ·  Enter toggles",
        }
    }

    pub fn empty_text(&self) -> &'static str {
        match self.mode {
            Mode::Commands => "No matching commands",
            Mode::Fonts | Mode::CodeFonts => "No matching fonts",
            Mode::Options => "No matching options",
            Mode::Files => "No matching files",
            Mode::Folders => "No matching folders",
            Mode::FolderSettings => "No matching settings",
        }
    }

    fn refilter(&mut self) {
        let mut scored: Vec<(i32, usize, Vec<usize>)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| {
                // Keywords are searched too, but only label characters are highlighted.
                let (score, hits) = if it.keywords.is_empty() {
                    term_match(&self.query, &it.label)?
                } else {
                    term_match(&self.query, &format!("{} {}", it.label, it.keywords))?
                };
                Some((
                    score,
                    i,
                    hits.into_iter().filter(|&b| b < it.label.len()).collect(),
                ))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        self.filtered = scored.into_iter().map(|(_, i, m)| (i, m)).collect();
        self.selected = 0;
        self.first = 0;
    }

    fn select(&mut self, i: usize) {
        if self.filtered.is_empty() {
            return;
        }
        self.selected = i.min(self.filtered.len() - 1);
        if self.selected < self.first {
            self.first = self.selected;
        } else if self.selected >= self.first + PALETTE_ROWS {
            self.first = self.selected + 1 - PALETTE_ROWS;
        }
    }

    /// Moves the selection by `delta`, wrapping only for single steps.
    pub fn move_by(&mut self, delta: isize) {
        let n = self.filtered.len() as isize;
        if n == 0 {
            return;
        }
        let i = self.selected as isize + delta;
        let i = if delta.abs() == 1 {
            i.rem_euclid(n)
        } else {
            i.clamp(0, n - 1)
        };
        self.select(i as usize);
    }

    pub fn select_visible(&mut self, row: usize) {
        self.select(self.first + row);
    }

    pub fn scroll(&mut self, rows: isize) {
        let max_first = self.filtered.len().saturating_sub(PALETTE_ROWS) as isize;
        self.first = (self.first as isize + rows).clamp(0, max_first) as usize;
        self.selected = self.selected.clamp(
            self.first,
            (self.first + PALETTE_ROWS).min(self.filtered.len()).max(1) - 1,
        );
    }

    pub fn selected(&self) -> Option<&Item> {
        self.filtered
            .get(self.selected)
            .map(|(i, _)| &self.items[*i])
    }

    /// Visible items with their matched offsets, plus the selected row within them.
    pub fn visible(&self) -> (Vec<(&Item, &[usize])>, Option<usize>) {
        let end = (self.first + PALETTE_ROWS).min(self.filtered.len());
        let rows = self.filtered[self.first..end]
            .iter()
            .map(|(i, m)| (&self.items[*i], m.as_slice()))
            .collect();
        let sel = (!self.filtered.is_empty()).then(|| self.selected - self.first);
        (rows, sel)
    }

    // Query editing.

    pub fn insert(&mut self, s: &str) {
        let s: String = s.chars().filter(|c| !c.is_control()).collect();
        self.query.insert_str(self.cur, &s);
        self.cur += s.len();
        self.refilter();
    }

    pub fn backspace(&mut self, word: bool) {
        if self.cur == 0 {
            return;
        }
        let start = if word {
            doc::prev_word(&self.query, self.cur)
        } else {
            doc::prev_boundary(&self.query, self.cur)
        };
        self.query.replace_range(start..self.cur, "");
        self.cur = start;
        self.refilter();
    }

    pub fn delete(&mut self) {
        if self.cur < self.query.len() {
            let end = doc::next_boundary(&self.query, self.cur);
            self.query.replace_range(self.cur..end, "");
            self.refilter();
        }
    }

    pub fn left(&mut self) {
        self.cur = doc::prev_boundary(&self.query, self.cur);
    }

    pub fn right(&mut self) {
        self.cur = doc::next_boundary(&self.query, self.cur);
    }

    pub fn home(&mut self) {
        self.cur = 0;
    }

    pub fn end(&mut self) {
        self.cur = self.query.len();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortens_paths() {
        assert_eq!(short_path(r"C:\notes", 40), r"C:\notes");
        assert_eq!(
            short_path(r"C:\Users\someone\AppData\Local\Temp\work\scratchpad", 20),
            "\u{2026}\\work\\scratchpad"
        );
    }

    #[test]
    fn term_matching() {
        let label = "Font: Change Editor Font";
        // Every term must appear, in any order.
        assert!(term_match("change font", label).is_some());
        assert!(term_match("FONT  change", label).is_some());
        assert!(term_match("ed ang", label).is_some());
        assert!(term_match("change xyz", label).is_none());
        // Letters scattered across words no longer match.
        assert!(term_match("cef", label).is_none());
        // Matched characters, preferring word-start occurrences.
        let (_, hits) = term_match("ditor ch", label).unwrap();
        assert_eq!(hits, vec![6, 7, 14, 15, 16, 17, 18]);
        // Terms at the start of the label or a word rank higher.
        assert!(
            term_match("con", "Consolas").unwrap().0
                > term_match("con", "Cambria Condensed").unwrap().0
        );
        assert!(
            term_match("con", "Cambria Condensed").unwrap().0
                > term_match("ond", "Cambria Condensed").unwrap().0
        );
        assert_eq!(term_match("", "anything"), Some((0, Vec::new())));
        assert!(term_match("toolongterm", "short").is_none());
    }

    #[test]
    fn font_picker_filters_and_selects() {
        let names = ["Arial", "Cascadia Mono", "Consolas", "Georgia", "Segoe UI"]
            .iter()
            .map(|s| (s.to_string(), true, s.ends_with("Mono") || *s == "Consolas"))
            .collect();
        let mut p = Palette::fonts(names, "Georgia", false);
        assert_eq!(p.selected().unwrap().label, "Georgia");
        assert_eq!(p.selected().unwrap().detail, "Current");
        p.insert("on"); // in "Consolas" and "Cascadia Mono"
        assert_eq!(p.selected().unwrap().label, "Consolas");
        p.move_by(1);
        assert_eq!(
            p.selected().unwrap().target,
            Target::Font("Cascadia Mono".into())
        );
        p.backspace(false);
        p.backspace(false);
        assert_eq!(p.visible().0.len(), 5);
        p.insert("zzz");
        assert!(p.selected().is_none());
        assert_eq!(p.restore_font.as_deref(), Some("Georgia"));
    }

    #[test]
    fn code_font_picker() {
        let names = [("Arial", false), ("Consolas", true), ("Georgia", false)]
            .iter()
            .map(|&(n, mono)| (n.to_string(), true, mono))
            .collect();
        let p = Palette::fonts(names, "Consolas", true);
        assert_eq!(p.mode, Mode::CodeFonts);
        let (rows, sel) = p.visible();
        assert_eq!(rows[0].0.label, "Consolas");
        assert_eq!(sel, Some(0));
        let o = RenderOptions::default();
        let p = Palette::options(&o, "UI: Font size");
        assert_eq!(p.visible().0.len(), 1);
    }

    #[test]
    fn selection_scrolls() {
        let names = (0..30)
            .map(|i| (format!("Font {i:02}"), true, false))
            .collect();
        let mut p = Palette::fonts(names, "Font 00", false);
        p.move_by(15);
        let (rows, sel) = p.visible();
        assert_eq!(rows[sel.unwrap()].0.label, "Font 15");
        p.move_by(-1);
        p.move_by(100);
        assert_eq!(p.selected().unwrap().label, "Font 29");
        p.move_by(1);
        assert_eq!(p.selected().unwrap().label, "Font 00");
    }
}
