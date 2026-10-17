//! Command palette state: the query, term filtering, and selection. Also hosts
//! "Search in Files", which matches the query against the lines of Markdown files.

use std::path::{Path, PathBuf};

use crate::doc;
use crate::keymap::{Action, Keymap};
use crate::options::{Opt, RenderOptions};
use crate::render::PALETTE_ROWS;
use crate::table::TableCmd;
use crate::workspace::{self, Entry, FolderOpt, FolderSettings};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    QuickOpen,
    GoToHeading,
    SearchFiles,
    OpenFolder,
    OpenRecent,
    CloseFolder,
    NewFile,
    NewFolder,
    DailyNote,
    PreviousDailyNote,
    NextDailyNote,
    ToggleExplorer,
    ToggleOutline,
    ToggleSpellCheck,
    FoldHeading,
    UnfoldHeading,
    UnfoldAll,
    FolderSettings,
    ChangeFont,
    ChangeCodeFont,
    CodeFontSize,
    UiFontSize,
    RenderingOptions,
    ToggleSource,
    CycleViewMode,
    ToggleTheme,
    Help,
    OpenSettingsFolder,
    WriteKeymap,
    WriteTheme,
    Quit,
    Table(TableCmd),
}

/// Every command, in the order shown for an empty query: (label, the action whose
/// key is shown beside it, command).
const COMMANDS: &[(&str, Option<Action>, Command)] = &[
    (
        "Go to File\u{2026}",
        Some(Action::QuickOpen),
        Command::QuickOpen,
    ),
    (
        "Go to Heading\u{2026}",
        Some(Action::Run(Command::GoToHeading)),
        Command::GoToHeading,
    ),
    (
        "Search: Find in Files",
        Some(Action::SearchFiles),
        Command::SearchFiles,
    ),
    ("File: Open Folder\u{2026}", None, Command::OpenFolder),
    (
        "File: Open Recent Folder\u{2026}",
        Some(Action::RecentFolders),
        Command::OpenRecent,
    ),
    ("File: Close Folder", None, Command::CloseFolder),
    ("File: New File in Folder\u{2026}", None, Command::NewFile),
    ("File: New Folder\u{2026}", None, Command::NewFolder),
    (
        "File: Open Today's Daily Note",
        Some(Action::Run(Command::DailyNote)),
        Command::DailyNote,
    ),
    (
        "File: Previous Daily Note",
        Some(Action::Run(Command::PreviousDailyNote)),
        Command::PreviousDailyNote,
    ),
    (
        "File: Next Daily Note",
        Some(Action::Run(Command::NextDailyNote)),
        Command::NextDailyNote,
    ),
    ("File: Quit (Close All Windows)", None, Command::Quit),
    (
        "View: Toggle Explorer",
        Some(Action::ToggleExplorer),
        Command::ToggleExplorer,
    ),
    (
        "View: Toggle Outline",
        Some(Action::ToggleOutline),
        Command::ToggleOutline,
    ),
    (
        "View: Fold Heading",
        Some(Action::Run(Command::FoldHeading)),
        Command::FoldHeading,
    ),
    (
        "View: Unfold Heading",
        Some(Action::Run(Command::UnfoldHeading)),
        Command::UnfoldHeading,
    ),
    ("View: Unfold All Headings", None, Command::UnfoldAll),
    (
        "View: Toggle Spell Check",
        Some(Action::Run(Command::ToggleSpellCheck)),
        Command::ToggleSpellCheck,
    ),
    (
        "Preferences: Folder Settings",
        None,
        Command::FolderSettings,
    ),
    ("Font: Change Editor Font", None, Command::ChangeFont),
    ("Font: Change Code Font", None, Command::ChangeCodeFont),
    ("Font: Code Font Size", None, Command::CodeFontSize),
    ("Preferences: UI Font Size", None, Command::UiFontSize),
    ("View: Rendering Options", None, Command::RenderingOptions),
    (
        "View: Toggle Source Mode",
        Some(Action::ToggleSource),
        Command::ToggleSource,
    ),
    (
        "View: Cycle View Mode (Live, Live 2, View, Source)",
        None,
        Command::CycleViewMode,
    ),
    (
        "Preferences: Toggle Light/Dark Theme",
        Some(Action::ToggleTheme),
        Command::ToggleTheme,
    ),
    (
        "Preferences: Open Settings Folder",
        None,
        Command::OpenSettingsFolder,
    ),
    (
        "Preferences: Write keymap.ini (Edit Keyboard Shortcuts)",
        None,
        Command::WriteKeymap,
    ),
    (
        "Preferences: Write theme.ini (Edit Colors)",
        None,
        Command::WriteTheme,
    ),
    ("Help: Keyboard Shortcuts", None, Command::Help),
];

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Target {
    Command(Command),
    Font(String),
    Option(Opt),
    ResetOptions,
    OpenFile(PathBuf),
    /// A search result: the file and the byte range of the match on `line`.
    OpenMatch {
        path: PathBuf,
        line: usize,
        col: usize,
        len: usize,
    },
    SwitchFolder(PathBuf),
    OpenFolderDialog,
    FolderOpt(FolderOpt),
    /// A heading in the open note, by line.
    Heading(usize),
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
    Search,
    Headings,
}

/// A Markdown file searched by "Search in Files".
pub struct SearchFile {
    pub path: PathBuf,
    /// Path shown with each result, relative to the folder.
    pub shown: String,
    pub lines: Vec<String>,
}

/// Stop listing search results after this many matching lines.
pub const MAX_RESULTS: usize = 2000;
/// Characters of context kept before a match that is far into its line.
const CONTEXT: usize = 24;
/// Longest result label, in characters.
const LABEL_MAX: usize = 160;

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
    /// Number of items visible at once.
    rows: usize,
    /// Font to restore when the font picker is cancelled.
    pub restore_font: Option<String>,
    /// Files searched in `Mode::Search`.
    search: Vec<SearchFile>,
    /// Search mode: matching lines and files, and whether results were cut off.
    pub found: (usize, usize, bool),
}

/// The label for a line matching `q` first at byte `col`: trimmed, starting shortly
/// before the match when it is far in, with the bytes of every match in the label.
fn result_label(line: &str, q: &str, col: usize) -> (String, Vec<usize>) {
    let indent = line.len() - line.trim_start().len();
    let mut start = indent.min(col);
    if line[start..col].chars().count() > CONTEXT {
        start = line[..col]
            .char_indices()
            .rev()
            .nth(CONTEXT - 1)
            .map_or(start, |(i, _)| i);
    }
    let body: String = line[start..].trim_end().chars().take(LABEL_MAX).collect();
    let prefix = if start > indent { "\u{2026}" } else { "" };
    let label = format!("{prefix}{body}");
    let mut hits = Vec::new();
    let mut from = prefix.len();
    while let Some(i) = doc::find_ci(&label, q, from) {
        hits.extend(label[i..i + q.len()].char_indices().map(|(b, _)| i + b));
        from = i + q.len();
    }
    (label, hits)
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
            rows: PALETTE_ROWS,
            restore_font: None,
            search: Vec::new(),
            found: (0, 0, false),
        };
        p.refilter();
        p
    }

    /// All commands, with their keys from `keys`.
    pub fn commands(keys: &Keymap) -> Self {
        let mut items: Vec<Item> = COMMANDS
            .iter()
            .map(|&(label, action, cmd)| {
                let hint = keys.hint(action.unwrap_or(Action::Run(cmd)));
                Item::new(label, hint, Target::Command(cmd))
            })
            .collect();
        items.extend(TableCmd::ALL.iter().map(|&(cmd, label)| {
            let mut hint = keys.hint(Action::Run(Command::Table(cmd)));
            if hint.is_empty() && cmd == TableCmd::RowBelow {
                // Fixed inside a table.
                hint = "Ctrl+Enter".to_string();
            }
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

    /// Search in Files over `files`, starting with `query`.
    pub fn search(files: Vec<SearchFile>, query: &str) -> Self {
        let mut p = Palette::new(Mode::Search, Vec::new());
        p.search = files;
        p.insert(query);
        p
    }

    /// Recent folders, with "Open Folder..." first.
    /// The open note's headings, (line, level, text), indented by level; starting on
    /// `current` (an index into `headings`).
    pub fn headings(headings: &[(usize, u8, String)], current: Option<usize>) -> Self {
        let min = headings.iter().map(|h| h.1).min().unwrap_or(1);
        let items = headings
            .iter()
            .map(|(line, level, text)| {
                let indent = "    ".repeat((level - min) as usize);
                let text = if text.is_empty() { "(untitled)" } else { text };
                Item::new(
                    format!("{indent}{text}"),
                    format!("H{level}  \u{b7}  line {}", line + 1),
                    Target::Heading(*line),
                )
            })
            .collect();
        let mut p = Palette::new(Mode::Headings, items);
        if let Some(i) = current {
            p.select(i.min(p.filtered.len().saturating_sub(1)));
        }
        p
    }

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
            Mode::FolderSettings => "Folder settings  ·  ←/→ adjust  ·  Enter toggles or edits",
            Mode::Search => "Search in Markdown files",
            Mode::Headings => "Go to a heading in this note",
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
            Mode::Search if self.query.trim().is_empty() => "Type to search the folder's notes",
            Mode::Search => "No results",
            Mode::Headings => "No matching headings",
        }
    }

    fn refilter(&mut self) {
        if self.mode == Mode::Search {
            return self.find_in_files();
        }
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

    /// Lists every line containing the query (case-insensitive), file by file.
    fn find_in_files(&mut self) {
        self.items.clear();
        self.filtered.clear();
        self.selected = 0;
        self.first = 0;
        self.found = (0, 0, false);
        let q = self.query.trim();
        if q.is_empty() {
            return;
        }
        for f in &self.search {
            let mut in_file = false;
            for (l, line) in f.lines.iter().enumerate() {
                let Some(col) = doc::find_ci(line, q, 0) else {
                    continue;
                };
                if self.items.len() == MAX_RESULTS {
                    self.found.2 = true;
                    return;
                }
                let (label, hits) = result_label(line, q, col);
                self.filtered.push((self.items.len(), hits));
                self.items.push(Item::new(
                    label,
                    format!("{}:{}", f.shown, l + 1),
                    Target::OpenMatch {
                        path: f.path.clone(),
                        line: l,
                        col,
                        len: q.len(),
                    },
                ));
                self.found.0 += 1;
                in_file = true;
            }
            self.found.1 += in_file as usize;
        }
    }

    fn select(&mut self, i: usize) {
        if self.filtered.is_empty() {
            return;
        }
        self.selected = i.min(self.filtered.len() - 1);
        if self.selected < self.first {
            self.first = self.selected;
        } else if self.selected >= self.first + self.rows {
            self.first = self.selected + 1 - self.rows;
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

    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Shows `rows` items at once, keeping the selection in view.
    pub fn set_rows(&mut self, rows: usize) {
        self.rows = rows.max(1);
        self.first = self
            .first
            .min(self.filtered.len().saturating_sub(self.rows));
        self.select(self.selected);
    }

    pub fn select_visible(&mut self, row: usize) {
        self.select(self.first + row);
    }

    pub fn scroll(&mut self, rows: isize) {
        let max_first = self.filtered.len().saturating_sub(self.rows) as isize;
        self.first = (self.first as isize + rows).clamp(0, max_first) as usize;
        self.selected = self.selected.clamp(
            self.first,
            (self.first + self.rows).min(self.filtered.len()).max(1) - 1,
        );
    }

    pub fn selected(&self) -> Option<&Item> {
        self.filtered
            .get(self.selected)
            .map(|(i, _)| &self.items[*i])
    }

    /// Visible items with their matched offsets, plus the selected row within them.
    pub fn visible(&self) -> (Vec<(&Item, &[usize])>, Option<usize>) {
        let end = (self.first + self.rows).min(self.filtered.len());
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

    pub fn clear(&mut self) {
        self.query.clear();
        self.cur = 0;
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
    fn headings_filter_and_start_on_current() {
        let h = |l: usize, n: u8, t: &str| (l, n, t.to_string());
        let mut p = Palette::headings(
            &[h(0, 1, "Plan"), h(3, 2, "Goals"), h(9, 2, "Risks")],
            Some(1),
        );
        assert_eq!(p.selected().map(|i| &i.target), Some(&Target::Heading(3)));
        assert_eq!(p.selected().map(|i| i.label.as_str()), Some("    Goals"));
        p.insert("ri");
        assert_eq!(p.selected().map(|i| &i.target), Some(&Target::Heading(9)));
    }

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
    fn search_in_files() {
        let file = |name: &str, text: &str| SearchFile {
            path: PathBuf::from(name),
            shown: name.to_string(),
            lines: text.split('\n').map(String::from).collect(),
        };
        let files = vec![
            file("a.md", "# Notes\n  - buy Milk and milk\nnothing"),
            file("b.md", "no match"),
            file("c.md", "MILK"),
        ];
        let mut p = Palette::search(files, "");
        assert!(p.selected().is_none());
        assert_eq!(p.empty_text(), "Type to search the folder's notes");
        p.insert("milk");
        assert_eq!(p.found, (2, 2, false));
        let (rows, _) = p.visible();
        // Indentation is trimmed and every match on the line is highlighted.
        assert_eq!(rows[0].0.label, "- buy Milk and milk");
        assert_eq!(rows[0].1, &[6, 7, 8, 9, 15, 16, 17, 18]);
        assert_eq!(rows[0].0.detail, "a.md:2");
        assert_eq!(
            rows[0].0.target,
            Target::OpenMatch {
                path: PathBuf::from("a.md"),
                line: 1,
                col: 8,
                len: 4
            }
        );
        assert_eq!(rows[1].0.detail, "c.md:1");
        p.insert("x");
        assert!(p.selected().is_none());
        assert_eq!(p.empty_text(), "No results");
    }

    #[test]
    fn result_labels_show_the_match() {
        let line = format!("{}needle end", "word ".repeat(20));
        let col = line.find("needle").unwrap();
        let (label, hits) = result_label(&line, "needle", col);
        assert!(label.starts_with('\u{2026}'));
        assert!(label.ends_with("needle end"));
        assert_eq!(label.chars().count(), 1 + CONTEXT + "needle end".len());
        let at = label.find("needle").unwrap();
        assert_eq!(hits, (at..at + 6).collect::<Vec<_>>());
        // Non-ASCII text before the match is cut on a character boundary.
        let line = format!("{}\u{e9}", "\u{e9}".repeat(40));
        let (label, _) = result_label(&line, "\u{e9}", line.len() - 2);
        assert!(label.starts_with('\u{2026}'));
    }

    #[test]
    fn every_command_is_in_the_keymap() {
        for item in Palette::commands(&Keymap::defaults()).items {
            let Target::Command(cmd) = item.target else {
                unreachable!()
            };
            let action = COMMANDS
                .iter()
                .find(|c| c.2 == cmd)
                .and_then(|c| c.1)
                .unwrap_or(Action::Run(cmd));
            assert!(
                Keymap::name(action).is_some(),
                "{cmd:?} is not in the keymap"
            );
        }
        let (keys, errors) = Keymap::parse("change_font = F2");
        assert!(errors.is_empty());
        let p = Palette::commands(&keys);
        let font = p
            .items
            .iter()
            .find(|i| i.label == "Font: Change Editor Font");
        assert_eq!(font.unwrap().detail, "F2");
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
