//! Editor state and input handling.

mod folder;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{InvalidateRect, MONITOR_DEFAULTTONULL, MonitorFromRect};
use windows::Win32::UI::Input::Ime::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR};

use crate::attach;
use crate::command::{self as cmd, Mode as CmdMode, Window as CmdWindow};
use crate::doc::{self, DiskChange, Document, EditKind, Pos, find_all, find_ci};
use crate::keymap::{self, Action, Chord, Keymap};
use crate::markdown::{self, Align, Kind, LineInfo};
use crate::options::{Prefs, RenderOptions};
use crate::palette::{Command, Mode, Palette, Target};
use crate::platform;
use crate::render::{
    HeaderView, OutlineView, PaletteItem, PaletteView, PopupView, SidebarButton, Status, TableUi,
    View, ViewMode,
};
use crate::settings::Settings;
use crate::slash::{self, SlashCmd};
use crate::spell;
use crate::table::{self, Grid, TableCmd};
use crate::theme::{self, Theme};
use crate::workspace::{self, Entry, FolderSettings, NoteLink};

pub const APP_NAME: &str = "Penmark";

/// Posted after files are dropped, to handle them once the drag has ended.
pub const WM_APP_DROP: u32 = WM_APP + 1;
/// Posted when a command run with `wait` ends: exit code, command id.
pub const WM_APP_COMMAND_DONE: u32 = WM_APP + 2;

/// Command links trusted to run without asking (their trust keys), at most this many.
const TRUSTED_MAX: usize = 500;

const ZOOM_STEP: f32 = 0.05;

/// Rounds to the nearest 5% step within 60%–300%.
fn clamp_zoom(z: f32) -> f32 {
    (z / ZOOM_STEP).round().clamp(12.0, 60.0) * ZOOM_STEP
}

#[derive(Clone, Copy, PartialEq)]
enum PromptKind {
    Find,
    /// Find with a replacement: `text` is the field being edited, `other` the other.
    Replace,
    GoTo,
    /// Rename (or move, with a relative path) the explorer entry `Prompt::path`.
    Rename,
    /// Create a file or folder inside the folder `Prompt::path`.
    NewFile,
    NewFolder,
    /// A text folder setting, such as the daily notes folder (`Prompt::path` is the
    /// open folder).
    FolderText(workspace::FolderOpt),
}

struct Prompt {
    kind: PromptKind,
    text: String,
    cur: usize,
    origin: (Pos, Pos, f32),
    /// The file or folder a rename / new-item prompt acts on.
    path: Option<PathBuf>,
    /// False while a find prompt stays open but the keyboard is in the
    /// document or explorer (after a click there).
    focused: bool,
    /// Replace prompt: the field not being edited, and whether `text` is the
    /// replacement.
    other: String,
    in_replace: bool,
}

impl PromptKind {
    fn finds(self) -> bool {
        matches!(self, PromptKind::Find | PromptKind::Replace)
    }
}

/// Everything the layout depends on: doc version, selected lines, table being edited,
/// source mode, width, zoom, DPI, missing wiki links, folds.
type LayoutKey = (
    u64,
    usize,
    usize,
    u32,
    ViewMode,
    Option<usize>,
    u32,
    u32,
    u32,
    u64,
    u64,
);

/// Where the caret goes when moving to a table cell.
#[derive(Clone, Copy)]
enum Place {
    Start,
    End,
    /// Select the cell's text (Tab / Shift+Tab), so typing replaces it.
    All,
}

/// A table cell located from a document position.
struct Cell {
    /// First table line and one past the last.
    start: usize,
    end: usize,
    qend: usize,
    /// Grid row (header 0, delimiter skipped) and column.
    row: usize,
    col: usize,
    /// Byte range of the cell's text on its line.
    range: std::ops::Range<usize>,
}

/// Cells selected down one table column (Shift+Up/Down), as grid rows.
#[derive(Clone, Copy)]
struct CellSel {
    /// First line of the table.
    start: usize,
    col: usize,
    anchor: usize,
    row: usize,
}

impl CellSel {
    fn rows(&self) -> std::ops::RangeInclusive<usize> {
        self.anchor.min(self.row)..=self.anchor.max(self.row)
    }
}

/// The open `/` menu, for the `/` at `line`, `col`; or the `[[` note menu, whose
/// `col` is just after the `[[` (or, listing a note's headings, after `[[note#`).
struct SlashMenu {
    line: usize,
    col: usize,
    query: String,
    /// Matching [`slash::ITEMS`] (or `notes`) indices with highlighted label offsets.
    matches: Vec<(usize, Vec<usize>)>,
    selected: usize,
    /// The notes offered, for the `[[` menu (or headings, as notes named after them).
    notes: Option<Vec<NoteLink>>,
    /// The `[[note#` menu: `notes` are the note's headings.
    headings: bool,
}

/// Rows in the `[[` note menu.
const NOTE_MENU_ROWS: usize = 10;

/// The lines each folded heading hides: (heading line, the lines after it up to the
/// next heading of the same or a higher level). `headings` are (line, level, text).
fn folded_sections(
    headings: &[(usize, u8, String)],
    folded: &HashSet<usize>,
    lines: usize,
) -> Vec<(usize, std::ops::Range<usize>)> {
    headings
        .iter()
        .enumerate()
        .filter(|(_, h)| folded.contains(&h.0))
        .map(|(k, &(line, level, _))| {
            let end = headings[k + 1..]
                .iter()
                .find(|h| h.1 <= level)
                .map_or(lines, |h| h.0);
            (line, line + 1..end)
        })
        .collect()
}

/// A place in a file, for going back and forward between files.
struct NavEntry {
    path: PathBuf,
    cursor: Pos,
    scroll: f32,
}

const NAV_MAX: usize = 50;

#[derive(Clone, Copy)]
enum Drag {
    None,
    Char,
    Word(Pos, Pos),
    Line(usize),
    Scrollbar(f32),
}

pub struct App {
    pub hwnd: HWND,
    pub doc: Document,
    infos: Vec<LineInfo>,
    infos_ver: Option<u64>,
    pub view: View,
    anchor: Pos,
    cursor: Pos,
    goal_x: Option<f32>,
    scroll: f32,
    mode: ViewMode,
    /// The mode Toggle Source Mode returns to.
    prev_mode: ViewMode,
    /// In Live 2, the image line double-clicked to show its source.
    reveal: Option<usize>,
    caret_on: bool,
    focused: bool,
    layout_key: Option<LayoutKey>,
    message: Option<(String, Instant)>,
    prompt: Option<Prompt>,
    find_query: String,
    replace_query: String,
    drag: Drag,
    last_click: (u32, i32, i32, u32),
    high_surrogate: Option<u16>,
    /// Drop the next WM_CHAR: its key already acted (e.g. `A` in the explorer).
    skip_char: bool,
    words: (u64, usize),
    last_title: String,
    /// Display name for a document without a path.
    untitled_name: &'static str,
    /// True while the saved placement is applied, so a DPI change keeps the saved size.
    pub restoring_placement: bool,
    palette: Option<Palette>,
    /// Folded headings of the open file (`.0`), each as its text and which heading
    /// with that text it is, so folds stay on their heading as lines move.
    folds: (Option<PathBuf>, Vec<(String, usize)>),
    /// Where the caret was when Go to Heading opened, to go back to on Escape.
    heading_origin: Option<(Pos, Pos, f32)>,
    /// Set while Search in Files is open (see app/folder.rs).
    search_origin: Option<folder::SearchOrigin>,
    slash: Option<SlashMenu>,
    cells: Option<CellSel>,
    /// Mouse position (client DIPs) while over the window, for table controls.
    mouse: Option<(f32, f32)>,
    /// Table-control hover state last painted: near the table, control, column.
    table_hover: (bool, Option<TableUi>, Option<usize>),
    tracking_leave: bool,
    // The open folder and its explorer panel (see app/folder.rs).
    folder: Option<PathBuf>,
    entries: Vec<Entry>,
    expanded: HashSet<PathBuf>,
    folder_settings: FolderSettings,
    /// Whether the explorer is shown while a folder is open.
    explorer: bool,
    tree_focus: bool,
    tree_sel: Option<PathBuf>,
    tree_scroll: f32,
    tree_cut: Option<PathBuf>,
    tree_drag: Option<folder::TreeDrag>,
    tree_hover: Option<usize>,
    tree_button: Option<SidebarButton>,
    /// Explorer hides `data` folders and files other than Markdown.
    notes_only: bool,
    /// For autosave: when the document last changed, and a version that failed to save.
    last_edit: Instant,
    seen_version: u64,
    autosave_failed: Option<u64>,
    /// The outline panel and its headings (line, level, text) for a doc version.
    outline: bool,
    outline_items: (u64, Vec<(usize, u8, String)>),
    outline_scroll: f32,
    outline_hover: Option<usize>,
    /// The heading last shown as current, to scroll it into view when it changes.
    outline_current: Option<usize>,
    /// Notes linking to the open one, for the outline panel, and the (path, folder
    /// scan) they were found for; each note's links are cached by its stamp.
    backlinks: (Option<(PathBuf, u64)>, Vec<workspace::Backlink>),
    link_cache: workspace::LinkCache,
    /// Bumped on each folder scan, so backlinks are looked for again.
    scan_gen: u64,
    /// For dimming `[[links]]` to missing notes: the (doc version, scan) last
    /// checked, and whether each name (lower case) resolved, for a (scan, folder).
    missing_key: Option<(u64, u64)>,
    resolved: ((u64, PathBuf), HashMap<String, bool>),
    /// Commands run with `wait` that have not ended: (id, label).
    running: Vec<(isize, String)>,
    next_command: isize,
    /// Files dropped and waiting for WM_APP_DROP, and where they go.
    pending_drop: (Vec<PathBuf>, Option<Pos>),
    /// Files to go back / forward to (most recent last).
    back: Vec<NavEntry>,
    forward: Vec<NavEntry>,
    keymap: Keymap,
    theme: Theme,
    /// General preferences ("Preferences: Open Settings").
    prefs: Prefs,
    /// Spell checking: on or off, Windows' checker (made when first needed; None
    /// inside when there is none for the language), and each line's misspellings by
    /// a hash of its text.
    spell_on: bool,
    speller: Option<Option<platform::SpellChecker>>,
    spell_cache: HashMap<u64, Vec<std::ops::Range<usize>>>,
}

fn ctrl_down() -> bool {
    unsafe { GetKeyState(VK_CONTROL.0 as i32) < 0 }
}
fn shift_down() -> bool {
    unsafe { GetKeyState(VK_SHIFT.0 as i32) < 0 }
}
fn alt_down() -> bool {
    unsafe { GetKeyState(VK_MENU.0 as i32) < 0 }
}

fn rfind_ci(hay: &str, needle: &str, before: usize) -> Option<usize> {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    if n.is_empty() || before < n.len() {
        return None;
    }
    (0..=before.min(h.len()) - n.len())
        .rev()
        .find(|&i| hay.is_char_boundary(i) && h[i..i + n.len()].eq_ignore_ascii_case(n))
}

fn slug(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .chars()
        .filter_map(|c| match c {
            ' ' | '-' => Some('-'),
            c if c.is_alphanumeric() || c == '_' => Some(c),
            _ => None,
        })
        .collect()
}

pub const HELP: &str = include_str!("../assets/help.md");

impl App {
    pub fn new(mut view: View) -> Self {
        let settings = Settings::load();
        if let Some(font) = settings.get("font").filter(|f| view.has_font(f)) {
            view.set_body_font(font);
        }
        if let Some(font) = settings.get("code_font").filter(|f| view.has_font(f)) {
            view.set_code_font(font);
        }
        view.set_options(RenderOptions::load(&settings));
        let explorer = settings.get("explorer") != Some("off");
        if let Some(pct) = settings.get("zoom").and_then(|z| z.parse::<f32>().ok()) {
            view.zoom = clamp_zoom(pct / 100.0);
        }
        view.outline = settings.get("outline") == Some("on");
        let mode = settings
            .get("view_mode")
            .and_then(ViewMode::from_key)
            .unwrap_or_default();
        let (keymap, keymap_errors) = Keymap::load();
        let (theme, theme_errors) = Theme::load();
        view.set_colors(theme.light, theme.dark);
        App {
            hwnd: HWND::default(),
            doc: Document::new(),
            infos: Vec::new(),
            infos_ver: None,
            view,
            anchor: Pos::default(),
            cursor: Pos::default(),
            goal_x: None,
            scroll: 0.0,
            mode,
            prev_mode: if mode == ViewMode::Source {
                ViewMode::Live
            } else {
                mode
            },
            reveal: None,
            caret_on: true,
            focused: true,
            layout_key: None,
            message: keymap_errors
                .first()
                .or(theme_errors.first())
                .map(|e| (e.clone(), Instant::now())),
            prompt: None,
            find_query: String::new(),
            replace_query: String::new(),
            drag: Drag::None,
            last_click: (0, 0, 0, 0),
            high_surrogate: None,
            skip_char: false,
            words: (u64::MAX, 0),
            last_title: String::new(),
            untitled_name: "Untitled",
            palette: None,
            heading_origin: None,
            folds: (None, Vec::new()),
            search_origin: None,
            slash: None,
            cells: None,
            mouse: None,
            table_hover: (false, None, None),
            tracking_leave: false,
            folder: None,
            entries: Vec::new(),
            expanded: HashSet::new(),
            folder_settings: FolderSettings::default(),
            explorer,
            tree_focus: false,
            tree_sel: None,
            tree_scroll: 0.0,
            tree_cut: None,
            tree_drag: None,
            tree_hover: None,
            tree_button: None,
            notes_only: settings.get("explorer.notes_only") == Some("on"),
            last_edit: Instant::now(),
            seen_version: 0,
            autosave_failed: None,
            outline: settings.get("outline") == Some("on"),
            outline_items: (0, Vec::new()),
            outline_scroll: 0.0,
            outline_hover: None,
            outline_current: None,
            backlinks: (None, Vec::new()),
            link_cache: workspace::LinkCache::new(),
            scan_gen: 0,
            missing_key: None,
            resolved: ((0, PathBuf::new()), HashMap::new()),
            pending_drop: (Vec::new(), None),
            running: Vec::new(),
            next_command: 0,
            back: Vec::new(),
            forward: Vec::new(),
            keymap,
            theme,
            prefs: Prefs::load(&settings),
            spell_on: settings.get("spellcheck") != Some("off"),
            speller: None,
            spell_cache: HashMap::new(),
            restoring_placement: false,
        }
    }

    // -----------------------------------------------------------------------
    // State helpers

    fn infos(&mut self) -> &[LineInfo] {
        if self.infos_ver != Some(self.doc.version) {
            self.infos = markdown::classify(&self.doc.lines);
            self.infos_ver = Some(self.doc.version);
        }
        &self.infos
    }

    fn info(&mut self, line: usize) -> LineInfo {
        self.infos()[line]
    }

    fn sel(&self) -> (Pos, Pos) {
        if self.anchor <= self.cursor {
            (self.anchor, self.cursor)
        } else {
            (self.cursor, self.anchor)
        }
    }

    fn doc_dir(&self) -> Option<PathBuf> {
        self.doc
            .path
            .as_ref()
            .and_then(|p| p.parent())
            .map(Path::to_path_buf)
    }

    pub fn ensure_layout(&mut self) {
        self.infos();
        self.update_missing_links();
        self.update_folds();
        let (a, b) = self.sel();
        let grid = self.grid_block();
        if self
            .reveal
            .is_some_and(|l| l != self.cursor.line || self.mode != ViewMode::Live2)
        {
            self.reveal = None;
        }
        let key = (
            self.doc.version,
            a.line,
            b.line,
            grid,
            self.mode,
            self.reveal,
            self.view.w.to_bits(),
            self.view.zoom.to_bits(),
            self.view.dpi.to_bits(),
            self.view.links_gen,
            self.view.fold_gen,
        );
        if self.layout_key != Some(key) {
            let dir = self.doc_dir();
            self.view.layout(
                self.hwnd,
                &self.doc.lines,
                &self.infos,
                (a.line, b.line),
                grid,
                self.mode,
                self.reveal,
                dir.as_deref(),
            );
            self.layout_key = Some(key);
        }
    }

    /// Each heading as its fold key: its text and how many headings before it have
    /// the same text.
    fn heading_keys(&mut self) -> Vec<(usize, u8, (String, usize))> {
        let mut seen: HashMap<String, usize> = HashMap::new();
        self.outline_items()
            .iter()
            .map(|(line, level, text)| {
                let n = seen.entry(text.clone()).or_insert(0);
                *n += 1;
                (*line, *level, (text.clone(), *n - 1))
            })
            .collect()
    }

    /// Finds the folded headings' lines for the view, unfolding the section the
    /// caret is in (after find, go to line, an edit, ...).
    fn update_folds(&mut self) {
        if self.folds.0 != self.doc.path {
            self.folds = (self.doc.path.clone(), Vec::new());
        }
        let mut lines = HashSet::new();
        if !self.folds.1.is_empty() {
            let keys = self.heading_keys();
            lines = keys
                .iter()
                .filter(|(_, _, k)| self.folds.1.contains(k))
                .map(|h| h.0)
                .collect();
            let headings = self.outline_items().to_vec();
            let n = self.doc.lines.len();
            for (heading, range) in folded_sections(&headings, &lines, n) {
                if range.contains(&self.cursor.line) {
                    lines.remove(&heading);
                    if let Some((_, _, k)) = keys.iter().find(|h| h.0 == heading) {
                        self.folds.1.retain(|f| f != k);
                    }
                }
            }
            // Folds whose heading is gone are dropped.
            self.folds.1.retain(|f| keys.iter().any(|h| &h.2 == f));
        }
        if lines != self.view.folds {
            self.view.folds = lines;
            self.view.fold_gen += 1;
        }
    }

    /// Folds (or unfolds) the section the caret is in, by its heading.
    fn fold(&mut self, fold: bool) {
        let line = self.cursor.line;
        let keys = self.heading_keys();
        let Some((hline, _, key)) = keys.into_iter().rfind(|h| h.0 <= line) else {
            return self.say("No heading here");
        };
        if fold {
            if !self.folds.1.contains(&key) {
                self.folds.1.push(key);
            }
            // The caret goes to the heading, which stays in view.
            if line != hline {
                self.infos();
                let p = Pos::new(hline, self.infos[hline].content);
                self.set_sel(p, p);
            }
        } else {
            self.folds.1.retain(|f| *f != key);
        }
        self.changed(false);
    }

    /// Finds the document's `[[links]]` to notes that don't exist, for the view to
    /// dim. Each name is resolved once per folder scan.
    fn update_missing_links(&mut self) {
        let key = (self.doc.version, self.scan_gen);
        if self.missing_key == Some(key) {
            return;
        }
        self.missing_key = Some(key);
        let mut missing = HashSet::new();
        if let Some(root) = self.folder.clone() {
            let dir = self.doc_dir().unwrap_or_else(|| root.clone());
            let ((scan, cached_dir), resolved) = &mut self.resolved;
            if *scan != self.scan_gen || *cached_dir != dir {
                *scan = self.scan_gen;
                *cached_dir = dir.clone();
                resolved.clear();
            }
            for (line, info) in self.doc.lines.iter().zip(&self.infos) {
                if !line.contains("[[") || matches!(info.kind, Kind::Code | Kind::Fence) {
                    continue;
                }
                let inline = markdown::parse_inline(line, info.content, line.len());
                for name in inline
                    .links
                    .iter()
                    .filter_map(|l| crate::render::wiki_name(&l.1))
                    // `[[#heading]]` is a link within this note.
                    .filter(|n| !n.is_empty())
                {
                    let k = name.to_lowercase();
                    let found = *resolved.entry(k.clone()).or_insert_with(|| {
                        workspace::resolve_note(Some(&root), &self.entries, &dir, name).is_some()
                    });
                    if !found {
                        missing.insert(k);
                    }
                }
            }
        }
        if missing != self.view.missing_links {
            self.view.missing_links = missing;
            self.view.links_gen += 1;
        }
    }

    pub fn invalidate(&self) {
        unsafe {
            let _ = InvalidateRect(Some(self.hwnd), None, false);
        }
    }

    pub fn relayout(&mut self) {
        self.layout_key = None;
        self.clamp_scroll();
        self.invalidate();
    }

    fn view_h(&self) -> f32 {
        self.view.view_h()
    }

    fn clamp_scroll(&mut self) {
        self.ensure_layout();
        self.scroll = self.scroll.clamp(0.0, self.view.max_scroll());
    }

    fn scroll_to_caret(&mut self) {
        self.ensure_layout();
        if let Some((_, y, h)) = self.view.caret_rect(self.cursor) {
            let margin = 40.0 * self.view.zoom;
            let vh = self.view_h();
            if y - margin < self.scroll {
                self.scroll = y - margin;
            } else if y + h + margin > self.scroll + vh {
                self.scroll = y + h + margin - vh;
            }
        }
        self.clamp_scroll();
    }

    fn update_ime(&self) {
        let Some((x, y, _)) = self.view.caret_rect(self.cursor) else {
            return;
        };
        let k = self.view.dpi / 96.0;
        unsafe {
            let himc = ImmGetContext(self.hwnd);
            if !himc.is_invalid() {
                let cf = COMPOSITIONFORM {
                    dwStyle: CFS_POINT,
                    ptCurrentPos: POINT {
                        x: (x * k) as i32,
                        y: ((y - self.scroll + self.view.header_h()) * k) as i32,
                    },
                    rcArea: RECT::default(),
                };
                let _ = ImmSetCompositionWindow(himc, &cf);
                let _ = ImmReleaseContext(self.hwnd, himc);
            }
        }
    }

    /// Call after the cursor moved or the text changed.
    fn changed(&mut self, keep_goal: bool) {
        if !keep_goal {
            self.goal_x = None;
        }
        self.caret_on = true;
        self.restart_blink();
        if self.doc.version != self.seen_version {
            self.seen_version = self.doc.version;
            self.last_edit = Instant::now();
            self.autosave_failed = None;
        }
        self.snap_to_cell();
        self.update_slash();
        self.scroll_to_caret();
        self.update_ime();
        self.update_title();
        self.invalidate();
    }

    fn restart_blink(&self) {
        unsafe {
            SetTimer(
                Some(self.hwnd),
                1,
                GetCaretBlinkTime().clamp(200, 1200),
                None,
            );
        }
    }

    /// Shows the window at its saved size and position. Returns false when nothing
    /// usable is saved (missing, too small, or on a monitor that is gone).
    pub fn restore_placement(&mut self) -> bool {
        let settings = Settings::load();
        let Some(saved) = settings.get("window") else {
            return false;
        };
        let n: Vec<i32> = saved
            .split(',')
            .filter_map(|x| x.trim().parse().ok())
            .collect();
        let [left, top, right, bottom, maximized] = n[..] else {
            return false;
        };
        let rc = RECT {
            left,
            top,
            right,
            bottom,
        };
        if right - left < 200
            || bottom - top < 150
            || unsafe { MonitorFromRect(&rc, MONITOR_DEFAULTTONULL) }.is_invalid()
        {
            return false;
        }
        let show = if maximized != 0 {
            SW_SHOWMAXIMIZED
        } else {
            SW_SHOWNORMAL
        };
        let wp = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            showCmd: show.0 as u32,
            rcNormalPosition: rc,
            ..Default::default()
        };
        self.restoring_placement = true;
        let ok = unsafe { SetWindowPlacement(self.hwnd, &wp) }.is_ok();
        self.restoring_placement = false;
        ok
    }

    /// Saves the window's normal (unmaximized) rectangle and whether it is maximized.
    pub fn save_placement(&self) {
        if self.untitled_name == "Help" {
            return;
        }
        let mut wp = WINDOWPLACEMENT {
            length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
            ..Default::default()
        };
        if unsafe { GetWindowPlacement(self.hwnd, &mut wp) }.is_err() {
            return;
        }
        let maximized = wp.showCmd == SW_SHOWMAXIMIZED.0 as u32
            || (wp.showCmd == SW_SHOWMINIMIZED.0 as u32
                && wp.flags.0 & WPF_RESTORETOMAXIMIZED.0 != 0);
        let r = wp.rcNormalPosition;
        let value = format!(
            "{},{},{},{},{}",
            r.left, r.top, r.right, r.bottom, maximized as i32
        );
        // Only called as the window closes, so there is nowhere to report a failure.
        let _ = Settings::update(|s| s.set("window", &value));
    }

    /// "folder — Penmark" while a folder is open, else "• file — Penmark".
    pub fn update_title(&mut self) {
        let folder = self.folder.as_ref().map(|f| {
            f.file_name()
                .unwrap_or(f.as_os_str())
                .to_string_lossy()
                .into_owned()
        });
        let title = match folder {
            Some(folder) => format!("{folder} — {APP_NAME}"),
            None => format!(
                "{}{} — {APP_NAME}",
                if self.doc.is_dirty() { "• " } else { "" },
                self.doc_name(),
            ),
        };
        if title != self.last_title {
            unsafe {
                let _ = SetWindowTextW(self.hwnd, &HSTRING::from(&title));
            }
            self.last_title = title;
        }
    }

    /// The open file's name, or "Untitled".
    fn doc_name(&self) -> String {
        self.doc
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.untitled_name.into())
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.message = Some((msg.into(), Instant::now()));
        self.restart_blink();
        self.invalidate();
    }

    /// Saves a setting, saying so when it could not be written (a read-only
    /// settings folder). Call it after any `say` it should not be hidden by.
    fn save_setting(&mut self, f: impl FnOnce(&mut Settings)) {
        if let Err(e) = Settings::update(f) {
            self.say(format!("Settings not saved: {e}"));
        }
    }

    fn set_sel(&mut self, anchor: Pos, cursor: Pos) {
        self.anchor = self.doc.clamp(anchor);
        self.cursor = self.doc.clamp(cursor);
    }

    fn move_to(&mut self, p: Pos, extend: bool) {
        let p = self.doc.clamp(p);
        self.cursor = p;
        if !extend {
            self.anchor = p;
        }
    }

    // -----------------------------------------------------------------------
    // Editing primitives

    fn begin(&mut self, kind: EditKind) {
        self.doc.begin(kind, (self.anchor, self.cursor));
    }

    fn commit(&mut self) {
        self.doc.commit((self.anchor, self.cursor));
        self.changed(false);
    }

    /// Replaces the selection with `text`, leaving the cursor after it.
    fn insert(&mut self, text: &str) {
        let (a, b) = self.sel();
        let end = self.doc.replace(a, b, text);
        self.anchor = end;
        self.cursor = end;
    }

    /// Single-line replacement that keeps the selection anchored to the text.
    fn replace_keep(&mut self, a: Pos, b: Pos, text: &str) {
        debug_assert!(a.line == b.line && !text.contains('\n'));
        self.doc.replace(a, b, text);
        for p in [&mut self.anchor, &mut self.cursor] {
            if p.line == a.line {
                if p.col >= b.col {
                    p.col = p.col - (b.col - a.col) + text.len();
                } else if p.col > a.col {
                    p.col = a.col + text.len().min(p.col - a.col);
                }
            }
        }
    }

    fn type_text(&mut self, text: &str) {
        let (a, b) = self.sel();
        // Typing a delimiter with a selection wraps it.
        if a != b && a.line == b.line && matches!(text, "*" | "_" | "`" | "~" | "=") {
            self.begin(EditKind::Other);
            self.replace_keep(b, b, text);
            self.replace_keep(a, a, text);
            self.set_sel(Pos::new(a.line, a.col + 1), Pos::new(b.line, b.col + 1));
            self.commit();
            return;
        }
        self.begin(EditKind::Typing);
        self.insert(text);
        self.commit();
    }

    // -----------------------------------------------------------------------
    // Keyboard

    pub fn on_char(&mut self, unit: u16) {
        if std::mem::take(&mut self.skip_char) {
            return;
        }
        if (0xD800..0xDC00).contains(&unit) {
            self.high_surrogate = Some(unit);
            return;
        }
        let s = if (0xDC00..0xE000).contains(&unit) {
            match self.high_surrogate.take() {
                Some(h) => String::from_utf16_lossy(&[h, unit]),
                None => return,
            }
        } else {
            match char::from_u32(unit as u32) {
                Some(c) if !c.is_control() => c.to_string(),
                _ => return,
            }
        };
        if ctrl_down() && !alt_down() {
            return;
        }
        self.cells = None;
        if let Some(p) = &mut self.palette {
            p.insert(&s);
            self.palette_changed();
            // A new query shows where the editor was until a result is picked again.
            self.show_search_origin();
            return;
        }
        if let Some(p) = &mut self.prompt
            && p.focused
        {
            p.text.insert_str(p.cur, &s);
            p.cur += s.len();
            self.prompt_changed();
            return;
        }
        if self.tree_focus || self.read_only() {
            return;
        }
        // A pipe typed in a table cell must not split the cell.
        let s = if s == "|" && self.grid_block() != 0 {
            r"\|".to_string()
        } else {
            s
        };
        // `#` in the `[[` menu takes the highlighted note, then lists its headings.
        if s == "#" {
            self.accept_note_name();
        }
        self.type_text(&s);
        if s == "/" {
            self.open_slash();
        } else if s == "[" {
            self.open_note_menu();
        } else if s == "#" {
            self.open_heading_menu();
        }
    }

    /// Returns true when the key was handled.
    pub fn on_key(&mut self, vk: u16) -> bool {
        let (ctrl, shift, alt) = (ctrl_down(), shift_down(), alt_down());
        let key = VIRTUAL_KEY(vk);
        let keeps_cells = matches!(key, VK_SHIFT | VK_CONTROL | VK_MENU)
            || (shift && !ctrl && matches!(key, VK_UP | VK_DOWN))
            || (!shift && !ctrl && matches!(key, VK_DELETE | VK_BACK))
            || (ctrl && matches!(key, VK_C | VK_X));
        if self.cells.is_some() && !keeps_cells {
            self.cells = None;
            self.invalidate();
        }
        if ctrl && alt {
            return false; // AltGr
        }
        if self.palette.is_some() {
            if alt {
                return false;
            }
            self.palette_key(VIRTUAL_KEY(vk), ctrl, shift);
            return true;
        }
        if self.prompt_focused() {
            if self.prompt_key(vk, ctrl, shift) {
                return true;
            }
            // A file-name prompt owns the keyboard, even when opened from the
            // explorer: its keys must not reach the explorer (where A would start
            // another new file) or the document behind it.
            if self.prompt.as_ref().is_some_and(|p| p.path.is_some())
                && !alt
                && !self.opens_palette(vk, ctrl, shift)
            {
                return true;
            }
        }
        if self.tree_focus && !self.prompt_focused() && !alt && self.tree_key(key, ctrl) {
            return true;
        }
        // Escape closes a find prompt left open behind the document.
        if key == VK_ESCAPE && !ctrl && !alt && self.slash.is_none() && self.prompt.is_some() {
            self.prompt = None;
            self.invalidate();
            return true;
        }
        // The slash menu takes selection keys; everything else edits as usual.
        if let Some(m) = &mut self.slash
            && !ctrl
            && !alt
        {
            let n = m.matches.len();
            match key {
                VK_UP => m.selected = (m.selected + n - 1) % n,
                VK_DOWN => m.selected = (m.selected + 1) % n,
                VK_RETURN | VK_TAB => self.slash_accept(),
                VK_ESCAPE => self.slash = None,
                _ => return self.editor_key(key, ctrl, shift, alt),
            }
            self.invalidate();
            return true;
        }
        self.editor_key(key, ctrl, shift, alt)
    }

    /// The editor's own key bindings, after overlays have had their turn.
    fn editor_key(&mut self, key: VIRTUAL_KEY, ctrl: bool, shift: bool, alt: bool) -> bool {
        if !alt && self.grid_block() != 0 && self.table_key(key, ctrl, shift) {
            return true;
        }
        let chord = Chord {
            vk: key.0,
            ctrl,
            shift,
            alt,
        };
        if let Some(action) = self.keymap.action(chord) {
            self.skip_char = chord.types_char();
            self.run_action(action);
            return true;
        }
        // Keys that keymap.ini cannot change.
        if ctrl {
            match key {
                VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN | VK_HOME | VK_END | VK_BACK | VK_DELETE => {
                    self.nav_key(key, true, shift)
                }
                _ => return false,
            }
            return true;
        }
        if alt {
            return false;
        }
        match key {
            VK_ESCAPE => {
                if self.anchor != self.cursor {
                    self.anchor = self.cursor;
                    self.changed(false);
                }
            }
            VK_RETURN | VK_TAB if self.read_only() => {}
            VK_RETURN if shift => {
                self.begin(EditKind::Other);
                self.insert("\n");
                self.commit();
            }
            VK_RETURN => self.enter(),
            VK_TAB => self.indent(!shift),
            _ => {
                if matches!(
                    key,
                    VK_LEFT
                        | VK_RIGHT
                        | VK_UP
                        | VK_DOWN
                        | VK_HOME
                        | VK_END
                        | VK_PRIOR
                        | VK_NEXT
                        | VK_BACK
                        | VK_DELETE
                ) {
                    self.nav_key(key, false, shift);
                } else {
                    return false;
                }
            }
        }
        true
    }

    /// Runs a command bound in the keymap.
    fn run_action(&mut self, action: Action) {
        let edits = matches!(
            action,
            Action::Undo
                | Action::Redo
                | Action::Cut
                | Action::Paste
                | Action::Bold
                | Action::Italic
                | Action::Code
                | Action::Strikethrough
                | Action::Highlight
                | Action::Link
                | Action::Heading(_)
                | Action::BulletList
                | Action::NumberedList
                | Action::TaskList
                | Action::Quote
                | Action::ToggleTask
                | Action::ToggleComment
                | Action::Indent
                | Action::Outdent
                | Action::InsertLineBelow
                | Action::InsertLineAbove
                | Action::MoveLinesUp
                | Action::MoveLinesDown
                | Action::CopyLinesUp
                | Action::CopyLinesDown
                | Action::DeleteLines
        );
        if edits && self.read_only() {
            return;
        }
        match action {
            Action::Palette => self.open_palette(Palette::commands(&self.keymap)),
            Action::QuickOpen => self.quick_open(),
            Action::SearchFiles => self.search_files(),
            Action::RecentFolders => self.recent_folders(),
            Action::ToggleExplorer => self.toggle_explorer(),
            Action::ExplorerFocus => self.toggle_explorer_focus(),
            Action::FocusExplorer => self.focus_explorer(),
            Action::ToggleOutline => self.toggle_outline(),
            Action::NewWindow => self.new_window(),
            Action::NewFile | Action::CloseFile => self.new_doc(),
            Action::OpenFile => self.open(),
            Action::Save => {
                self.save();
            }
            Action::SaveAs => {
                self.save_as();
            }
            Action::CloseWindow => unsafe {
                let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
            },
            Action::Back => self.navigate(true),
            Action::Forward => self.navigate(false),
            Action::Undo => self.undo(true),
            Action::Redo => self.undo(false),
            Action::SelectAll => {
                self.set_sel(Pos::default(), self.doc.end());
                self.changed(false);
            }
            Action::Copy => self.copy(false),
            Action::Cut => self.copy(true),
            Action::Paste => self.paste(),
            Action::Find => self.open_prompt(PromptKind::Find),
            Action::Replace => self.open_prompt(PromptKind::Replace),
            Action::FindNext => self.find_next(true),
            Action::FindPrevious => self.find_next(false),
            Action::GoToLine => self.open_prompt(PromptKind::GoTo),
            Action::SelectNextMatch => self.select_next_match(),
            Action::SelectLine => self.select_line(),
            Action::Bold => self.toggle_wrap("**"),
            Action::Italic => self.toggle_wrap("*"),
            Action::Code => self.toggle_wrap("`"),
            Action::Strikethrough => self.toggle_wrap("~~"),
            Action::Highlight => self.toggle_wrap("=="),
            Action::Link => self.make_link(),
            Action::Heading(level) => self.set_heading(level),
            Action::BulletList => self.set_list(ListStyle::Bullet),
            Action::NumberedList => self.set_list(ListStyle::Ordered),
            Action::TaskList => self.set_list(ListStyle::Task),
            Action::Quote => self.toggle_quote(),
            Action::ToggleTask => self.toggle_task(self.cursor.line),
            Action::ToggleComment => self.toggle_comment(),
            Action::Indent => self.indent(true),
            Action::Outdent => self.indent(false),
            Action::InsertLineBelow => self.open_line(true),
            Action::InsertLineAbove => self.open_line(false),
            Action::MoveLinesUp => self.move_lines(false),
            Action::MoveLinesDown => self.move_lines(true),
            Action::CopyLinesUp => self.duplicate_lines(false),
            Action::CopyLinesDown => self.duplicate_lines(true),
            Action::DeleteLines => self.delete_lines(),
            Action::ToggleSource => self.toggle_source(),
            Action::ToggleTheme => self.toggle_theme(),
            Action::ZoomIn => self.set_zoom(self.view.zoom + ZOOM_STEP),
            Action::ZoomOut => self.set_zoom(self.view.zoom - ZOOM_STEP),
            Action::ZoomReset => self.set_zoom(1.0),
            Action::Run(cmd) => self.palette_command(cmd),
        }
    }

    // -----------------------------------------------------------------------
    // Spell checking

    fn toggle_spell_check(&mut self) {
        self.spell_on = !self.spell_on;
        let on = self.spell_on;
        self.say(if on {
            "Spell check on"
        } else {
            "Spell check off"
        });
        self.save_setting(|s| s.set("spellcheck", if on { "on" } else { "off" }));
        self.invalidate();
    }

    /// The checker, made the first time it is needed.
    fn speller(&mut self) -> Option<&platform::SpellChecker> {
        self.speller
            .get_or_insert_with(platform::SpellChecker::new)
            .as_ref()
    }

    /// Checks the lines in view (each line's result is cached by its text), for the
    /// view to underline. The word being typed is not flagged.
    fn update_spelling(&mut self) {
        let mut spelling = HashMap::new();
        // The checker is made by the timer, after the window first shows.
        let ready = matches!(self.speller, Some(Some(_)));
        if self.spell_on && !self.source() && ready {
            let (top, bottom) = (self.scroll, self.scroll + self.view.view_h());
            let first = self.view.rows.partition_point(|r| r.y + r.h < top);
            let caret_line = self.cursor.line;
            for l in first..self.view.rows.len() {
                if self.view.rows[l].y > bottom {
                    break;
                }
                let line = &self.doc.lines[l];
                let mut h = std::collections::hash_map::DefaultHasher::new();
                std::hash::Hash::hash(&(line, self.infos[l].kind), &mut h);
                let key = std::hash::Hasher::finish(&h);
                if !self.spell_cache.contains_key(&key) {
                    let errors = self.check_line(l);
                    if self.spell_cache.len() > 20_000 {
                        self.spell_cache.clear();
                    }
                    self.spell_cache.insert(key, errors);
                }
                let errors = &self.spell_cache[&key];
                let typing = (l == caret_line && self.focused).then_some(self.cursor.col);
                let errors = spell::without_caret_word(errors, typing);
                if !errors.is_empty() {
                    spelling.insert(l, errors);
                }
            }
        }
        self.view.spelling = spelling;
    }

    fn check_line(&mut self, l: usize) -> Vec<std::ops::Range<usize>> {
        let Some(text) = spell::prose(&self.doc.lines[l], &self.infos[l]) else {
            return Vec::new();
        };
        let (wide, map) = spell::utf16(&text);
        let Some(checker) = self.speller() else {
            return Vec::new();
        };
        checker
            .check(&wide)
            .into_iter()
            .filter_map(|(s, n)| Some(*map.get(s as usize)?..*map.get((s + n) as usize)?))
            .collect()
    }

    /// Right-click on an underlined word: suggestions, Add to Dictionary, Ignore.
    /// Returns false when `p` is not on one.
    fn spell_menu(&mut self, p: Pos, x: i32, y: i32) -> bool {
        let Some(r) = self
            .view
            .spelling
            .get(&p.line)
            .and_then(|e| e.iter().find(|r| r.start <= p.col && p.col <= r.end))
            .cloned()
        else {
            return false;
        };
        let word = self.doc.lines[p.line][r.clone()].to_string();
        let suggestions = self
            .speller()
            .map(|c| c.suggest(&word, 6))
            .unwrap_or_default();
        let mut items: Vec<platform::MenuItem> = suggestions
            .iter()
            .enumerate()
            .map(|(i, s)| platform::MenuItem::Item {
                id: i as u32 + 1,
                label: s,
                checked: false,
                enabled: true,
            })
            .collect();
        if items.is_empty() {
            items.push(platform::MenuItem::Item {
                id: 0,
                label: "(no suggestions)",
                checked: false,
                enabled: false,
            });
        }
        items.push(platform::MenuItem::Separator);
        items.push(platform::MenuItem::Item {
            id: 100,
            label: "Add to Dictionary",
            checked: false,
            enabled: true,
        });
        items.push(platform::MenuItem::Item {
            id: 101,
            label: "Ignore",
            checked: false,
            enabled: true,
        });
        match platform::popup_menu(self.hwnd, &items, x, y) {
            Some(100) => {
                if let Some(c) = self.speller() {
                    c.add(&word);
                }
                self.spell_cache.clear();
                self.say(format!("Added \"{word}\" to the dictionary"));
            }
            Some(101) => {
                if let Some(c) = self.speller() {
                    c.ignore(&word);
                }
                self.spell_cache.clear();
            }
            Some(i) if i >= 1 && (i as usize) <= suggestions.len() => {
                let fix = suggestions[i as usize - 1].clone();
                self.begin(EditKind::Other);
                self.doc
                    .replace(Pos::new(p.line, r.start), Pos::new(p.line, r.end), &fix);
                let end = Pos::new(p.line, r.start + fix.len());
                self.set_sel(end, end);
                self.commit();
            }
            _ => {}
        }
        self.invalidate();
        true
    }

    /// Picks up changes to theme.ini, reporting the first problem in it.
    fn reload_theme(&mut self) {
        if let Some(errors) = self.theme.reload_if_changed() {
            self.view.set_colors(self.theme.light, self.theme.dark);
            self.invalidate();
            match errors.first() {
                Some(e) => self.say(e.clone()),
                None => self.say("Colors reloaded"),
            }
        }
    }

    /// Writes theme.ini with every color as it is now, and opens it.
    fn write_theme(&mut self) {
        let Some(path) = theme::path() else {
            return self.say("No settings folder (APPDATA is not set)");
        };
        self.reload_theme();
        if path.exists()
            && platform::message_box(
                self.hwnd,
                &format!(
                    "{} already exists.\n\nReplace it with every color? Colors you changed \
                     are kept; comments are not.",
                    path.display()
                ),
                APP_NAME,
                MB_YESNO | MB_ICONQUESTION,
            ) != IDYES
        {
            return;
        }
        let text = self.theme.to_ini().replace('\n', "\r\n");
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, text));
        match written {
            Ok(()) => {
                self.reload_theme();
                self.say(format!("Wrote {}", path.display()));
                platform::shell_open(self.hwnd, &path.to_string_lossy());
            }
            Err(e) => self.say(format!("Could not write {}: {e}", path.display())),
        }
    }

    /// Picks up changes to keymap.ini, reporting the first problem in it.
    fn reload_keymap(&mut self) {
        if let Some(errors) = self.keymap.reload_if_changed() {
            match errors.first() {
                Some(e) => self.say(e.clone()),
                None => self.say("Keyboard shortcuts reloaded"),
            }
        }
    }

    /// Opens the settings folder in Explorer.
    fn open_settings_folder(&mut self) {
        let Some(dir) = crate::settings::dir() else {
            return self.say("No settings folder (APPDATA is not set)");
        };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            return self.say(format!("Could not create {}: {e}", dir.display()));
        }
        platform::shell_open(self.hwnd, &dir.to_string_lossy());
    }

    /// Writes keymap.ini with every command and its current keys, and opens it.
    fn write_keymap(&mut self) {
        let Some(path) = keymap::path() else {
            return self.say("No settings folder (APPDATA is not set)");
        };
        self.reload_keymap();
        if path.exists()
            && platform::message_box(
                self.hwnd,
                &format!(
                    "{} already exists.\n\nReplace it with every command and its current \
                     keys? Keys you changed are kept; comments are not.",
                    path.display()
                ),
                APP_NAME,
                MB_YESNO | MB_ICONQUESTION,
            ) != IDYES
        {
            return;
        }
        let text = self.keymap.to_ini().replace('\n', "\r\n");
        let written = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, text));
        match written {
            Ok(()) => {
                self.reload_keymap();
                self.say(format!("Wrote {}", path.display()));
                platform::shell_open(self.hwnd, &path.to_string_lossy());
            }
            Err(e) => self.say(format!("Could not write {}: {e}", path.display())),
        }
    }

    fn nav_key(&mut self, key: VIRTUAL_KEY, ctrl: bool, shift: bool) {
        let c = self.cursor;
        let line = self.doc.lines[c.line].clone();
        let (a, b) = self.sel();
        match key {
            VK_LEFT => {
                let p = if !shift && a != b {
                    a
                } else if c.col > 0 && self.image_unit(c.line) {
                    Pos::new(c.line, 0)
                } else if c.col == 0 {
                    if c.line == 0 {
                        c
                    } else {
                        Pos::new(c.line - 1, self.doc.line_len(c.line - 1))
                    }
                } else if ctrl {
                    Pos::new(c.line, doc::prev_word(&line, c.col))
                } else {
                    Pos::new(c.line, doc::prev_boundary(&line, c.col))
                };
                self.move_to(p, shift);
            }
            VK_RIGHT => {
                let p = if !shift && a != b {
                    b
                } else if c.col < line.len() && self.image_unit(c.line) {
                    Pos::new(c.line, line.len())
                } else if c.col >= line.len() {
                    if c.line + 1 >= self.doc.lines.len() {
                        c
                    } else {
                        Pos::new(c.line + 1, 0)
                    }
                } else if ctrl {
                    Pos::new(c.line, doc::next_word(&line, c.col))
                } else {
                    Pos::new(c.line, doc::next_boundary(&line, c.col))
                };
                self.move_to(p, shift);
            }
            VK_UP | VK_DOWN if ctrl => {
                let dy = 3.0 * 1.6 * self.view.zoom * 16.0 * if key == VK_UP { -1.0 } else { 1.0 };
                self.scroll = (self.scroll + dy).clamp(0.0, self.view.max_scroll());
                self.invalidate();
                return;
            }
            VK_UP | VK_DOWN => {
                self.vertical(key == VK_DOWN, shift);
                return;
            }
            VK_PRIOR | VK_NEXT => {
                self.page(key == VK_NEXT, shift);
                return;
            }
            VK_HOME => {
                let p = if ctrl {
                    Pos::default()
                } else {
                    let content = self.info(c.line).content.min(line.len());
                    Pos::new(
                        c.line,
                        if c.col == content || content >= line.len() {
                            0
                        } else {
                            content
                        },
                    )
                };
                self.move_to(p, shift);
            }
            VK_END => {
                let p = if ctrl {
                    self.doc.end()
                } else {
                    Pos::new(c.line, line.len())
                };
                self.move_to(p, shift);
            }
            VK_BACK => {
                if !self.read_only() {
                    self.backspace(ctrl);
                }
                return;
            }
            VK_DELETE => {
                if !self.read_only() {
                    self.delete_forward(ctrl);
                }
                return;
            }
            _ => {}
        }
        self.changed(false);
    }

    fn vertical(&mut self, down: bool, extend: bool) {
        self.ensure_layout();
        let Some((cx, _, _)) = self.view.caret_rect(self.cursor) else {
            return;
        };
        let gx = self.goal_x.unwrap_or(cx);
        let y = if let Some(y) = self.view.adjacent_line_y(self.cursor, down) {
            y
        } else {
            let mut i = self.cursor.line as isize;
            loop {
                i += if down { 1 } else { -1 };
                if i < 0 || i as usize >= self.view.rows.len() {
                    let p = if down { self.doc.end() } else { Pos::default() };
                    self.move_to(p, extend);
                    self.changed(false);
                    return;
                }
                if self.view.rows[i as usize].h > 0.0 {
                    break;
                }
            }
            self.view.row_probe_y(i as usize, !down)
        };
        let infos = self.infos.clone();
        let hit = self.view.hit(&self.doc.lines, &infos, gx, y);
        self.move_to(hit.pos, extend);
        self.changed(true);
        self.goal_x = Some(gx);
    }

    fn page(&mut self, down: bool, extend: bool) {
        self.ensure_layout();
        let Some((cx, cy, _)) = self.view.caret_rect(self.cursor) else {
            return;
        };
        let gx = self.goal_x.unwrap_or(cx);
        let d = (self.view_h() - 60.0).max(40.0) * if down { 1.0 } else { -1.0 };
        self.scroll = (self.scroll + d).clamp(0.0, self.view.max_scroll());
        let y = (cy + d).clamp(0.0, self.view.total_h);
        let infos = self.infos.clone();
        let hit = self.view.hit(&self.doc.lines, &infos, gx, y);
        self.move_to(hit.pos, extend);
        self.changed(true);
        self.goal_x = Some(gx);
    }

    fn backspace(&mut self, word: bool) {
        let (a, b) = self.sel();
        let unit = a == b && a.col > 0 && self.image_unit(a.line);
        self.begin(EditKind::Delete);
        if a != b {
            self.insert("");
        } else if a.col == 0 {
            if a.line > 0 {
                let p = Pos::new(a.line - 1, self.doc.line_len(a.line - 1));
                self.doc.replace(p, a, "");
                self.set_sel(p, p);
            }
        } else {
            let info = self.info(a.line);
            let line = &self.doc.lines[a.line];
            let start = if unit {
                0
            } else if word {
                doc::prev_word(line, a.col)
            } else if a.col == info.content
                && matches!(info.kind, Kind::List { .. } | Kind::Heading(_))
                && !self.source()
            {
                // Remove the list marker / heading hashes in one go.
                info.qend
            } else if a.col == info.qend && info.quote > 0 && !self.source() {
                markdown::strip_quotes(line, info.quote - 1).1
            } else {
                doc::prev_boundary(line, a.col)
            };
            let p = Pos::new(a.line, start);
            self.doc.replace(p, a, "");
            self.set_sel(p, p);
        }
        self.commit();
    }

    fn delete_forward(&mut self, word: bool) {
        let (a, b) = self.sel();
        let unit = a == b && a.col == 0 && self.image_unit(a.line);
        self.begin(EditKind::Delete);
        if a != b {
            self.insert("");
        } else {
            let line = &self.doc.lines[a.line];
            let end = if a.col >= line.len() {
                if a.line + 1 < self.doc.lines.len() {
                    Pos::new(a.line + 1, 0)
                } else {
                    a
                }
            } else if unit {
                Pos::new(a.line, line.len())
            } else if word {
                Pos::new(a.line, doc::next_word(line, a.col))
            } else {
                Pos::new(a.line, doc::next_boundary(line, a.col))
            };
            self.doc.replace(a, end, "");
        }
        self.commit();
    }

    fn enter(&mut self) {
        self.begin(EditKind::Other);
        let (a, b) = self.sel();
        if a != b {
            self.insert("");
        }
        let c = self.cursor;
        let info = self.info(c.line);
        let line = self.doc.lines[c.line].clone();
        let quote_prefix = line[..info.qend].to_string();
        if self.source() {
            self.insert("\n");
        } else {
            match info.kind {
                Kind::List { ordered, task } if c.col >= info.content => {
                    if line[info.content..].trim().is_empty() {
                        // Empty item: outdent, or end the list.
                        if info.level > 0 {
                            let ws = markdown::leading_ws(&line[info.qend..]).1;
                            let cut = ws.min(4);
                            self.replace_keep(
                                Pos::new(c.line, info.qend),
                                Pos::new(c.line, info.qend + cut),
                                "",
                            );
                        } else {
                            self.doc.replace(
                                Pos::new(c.line, info.qend),
                                Pos::new(c.line, line.len()),
                                "",
                            );
                            let p = Pos::new(c.line, info.qend);
                            self.set_sel(p, p);
                        }
                    } else {
                        let indent = &line[info.qend..info.marker];
                        let marker_text = &line[info.marker..];
                        let marker = if ordered {
                            let digits: String = marker_text
                                .chars()
                                .take_while(|c| c.is_ascii_digit())
                                .collect();
                            let delim = marker_text[digits.len()..].chars().next().unwrap_or('.');
                            format!("{}{} ", digits.parse::<u64>().unwrap_or(0) + 1, delim)
                        } else {
                            format!("{} ", marker_text.chars().next().unwrap_or('-'))
                        };
                        let task = if task.is_some() { "[ ] " } else { "" };
                        self.insert(&format!("\n{quote_prefix}{indent}{marker}{task}"));
                    }
                }
                Kind::Para | Kind::Blank
                    if info.quote > 0 && line[info.qend..].trim().is_empty() =>
                {
                    // Empty quote line: leave the quote.
                    let p = Pos::new(c.line, 0);
                    self.doc.replace(p, Pos::new(c.line, line.len()), "");
                    self.set_sel(p, p);
                }
                Kind::Fence if c.col == line.len() && self.is_unclosed_fence(c.line) => {
                    let fence: String = line[info.qend..]
                        .chars()
                        .take_while(|&ch| ch == ' ' || ch == '`' || ch == '~')
                        .collect();
                    let indent: String = fence.chars().take_while(|&ch| ch == ' ').collect();
                    let p = self.doc.replace(
                        c,
                        c,
                        &format!("\n{quote_prefix}{indent}\n{quote_prefix}{fence}"),
                    );
                    let _ = p;
                    let mid = Pos::new(c.line + 1, quote_prefix.len() + indent.len());
                    self.set_sel(mid, mid);
                }
                Kind::Code | Kind::Fence => {
                    let ws = markdown::leading_ws(&line[info.qend..]).1;
                    let indent = &line[info.qend..info.qend + ws];
                    self.insert(&format!("\n{quote_prefix}{indent}"));
                }
                Kind::Para if info.level > 0 => {
                    let indent = &line[info.qend..info.content];
                    self.insert(&format!("\n{quote_prefix}{indent}"));
                }
                _ => {
                    let qp = if info.quote > 0 {
                        quote_prefix.as_str()
                    } else {
                        ""
                    };
                    self.insert(&format!("\n{qp}"));
                }
            }
        }
        self.commit();
    }

    fn is_unclosed_fence(&mut self, line: usize) -> bool {
        let infos = self.infos();
        let block = infos[line].block;
        let opening = line == 0 || infos[line - 1].block != block;
        opening
            && infos[line + 1..]
                .iter()
                .all(|i| i.block == block && i.kind != Kind::Fence)
    }

    fn line_range(&self) -> (usize, usize) {
        let (a, b) = self.sel();
        let last = if b.col == 0 && b.line > a.line {
            b.line - 1
        } else {
            b.line
        };
        (a.line, last)
    }

    fn indent(&mut self, forward: bool) {
        let (a, b) = self.sel();
        let (l0, l1) = self.line_range();
        let info = self.info(a.line);
        let multi = l0 != l1;
        self.begin(EditKind::Other);
        if !multi
            && forward
            && !matches!(info.kind, Kind::List { .. })
            && !(a == b && a.col <= info.content && info.level > 0)
        {
            self.insert("    ");
        } else {
            for l in l0..=l1 {
                let info = self.info(l);
                if multi && info.kind == Kind::Blank {
                    continue;
                }
                let at = Pos::new(l, info.qend);
                if forward {
                    self.replace_keep(at, at, "    ");
                } else {
                    let rest = &self.doc.lines[l][info.qend..];
                    let n = if rest.starts_with('\t') {
                        1
                    } else {
                        rest.bytes().take(4).take_while(|&c| c == b' ').count()
                    };
                    if n > 0 {
                        self.replace_keep(at, Pos::new(l, info.qend + n), "");
                    }
                }
            }
        }
        self.commit();
    }

    // -----------------------------------------------------------------------
    // Formatting commands

    fn toggle_wrap(&mut self, marker: &str) {
        let (mut a, mut b) = self.sel();
        if a.line != b.line {
            return;
        }
        let line = self.doc.lines[a.line].clone();
        let m = marker.len();
        let mc = marker.as_bytes()[0];
        self.begin(EditKind::Other);
        if a == b {
            let (s, e) = doc::word_at(&line, a.col);
            if s == e || !line[s..e].chars().any(|c| c.is_alphanumeric()) {
                self.insert(&format!("{marker}{marker}"));
                let p = Pos::new(a.line, a.col + m);
                self.set_sel(p, p);
                self.commit();
                return;
            }
            a.col = s;
            b.col = e;
        } else {
            // Keep spaces at the selection's edges outside the markers.
            let sel = &line[a.col..b.col];
            let t = sel.trim();
            if !t.is_empty() {
                a.col += sel.len() - sel.trim_start().len();
                b.col = a.col + t.len();
            }
        }
        let run_before = line[..a.col].bytes().rev().take_while(|&c| c == mc).count();
        let run_after = line[b.col..].bytes().take_while(|&c| c == mc).count();
        let around = run_before.min(run_after);
        let wrapped = if m == 1 && mc != b'`' {
            around % 2 == 1
        } else {
            around >= m
        };
        let inner = &line[a.col..b.col];
        if wrapped {
            self.anchor = a;
            self.cursor = b;
            self.replace_keep(b, Pos::new(b.line, b.col + m), "");
            self.replace_keep(Pos::new(a.line, a.col - m), a, "");
        } else if inner.len() >= 2 * m && inner.starts_with(marker) && inner.ends_with(marker) {
            self.replace_keep(Pos::new(b.line, b.col - m), b, "");
            self.replace_keep(a, Pos::new(a.line, a.col + m), "");
            self.set_sel(a, Pos::new(b.line, b.col - 2 * m));
        } else {
            self.anchor = a;
            self.cursor = b;
            self.replace_keep(b, b, marker);
            self.replace_keep(a, a, marker);
            self.set_sel(Pos::new(a.line, a.col + m), Pos::new(b.line, b.col + m));
        }
        self.commit();
    }

    fn make_link(&mut self) {
        let (a, b) = self.sel();
        if a.line != b.line {
            return;
        }
        let text = self.doc.text_range(a, b);
        self.begin(EditKind::Other);
        if text.starts_with("http://") || text.starts_with("https://") {
            self.insert(&format!("[]({text})"));
            let p = Pos::new(a.line, a.col + 1);
            self.set_sel(p, p);
        } else {
            self.insert(&format!("[{text}]()"));
            let p = Pos::new(a.line, a.col + text.len() + 3);
            self.set_sel(p, p);
        }
        self.commit();
    }

    /// Byte range of a line's block syntax prefix (after any quote markers).
    fn prefix_range(&mut self, l: usize) -> (usize, usize) {
        let info = self.info(l);
        match info.kind {
            Kind::List { .. } | Kind::Heading(_) => (info.marker, info.content),
            _ => (info.content, info.content),
        }
    }

    fn set_heading(&mut self, level: u8) {
        let (l0, l1) = self.line_range();
        self.begin(EditKind::Other);
        for l in l0..=l1 {
            let info = self.info(l);
            if matches!(info.kind, Kind::Code | Kind::Fence | Kind::Table { .. })
                || (l0 != l1 && info.kind == Kind::Blank)
            {
                continue;
            }
            let (s, e) = self.prefix_range(l);
            let new = if info.kind == Kind::Heading(level) {
                String::new()
            } else {
                format!("{} ", "#".repeat(level as usize))
            };
            self.replace_keep(Pos::new(l, s), Pos::new(l, e), &new);
        }
        self.commit();
    }

    fn set_list(&mut self, style: ListStyle) {
        let (l0, l1) = self.line_range();
        let lines: Vec<usize> = (l0..=l1)
            .filter(|&l| l0 == l1 || self.doc.lines[l].trim() != "")
            .collect();
        let all_same = lines.iter().all(|&l| {
            let k = self.info(l).kind;
            matches!(
                (style, k),
                (
                    ListStyle::Bullet,
                    Kind::List {
                        ordered: false,
                        task: None
                    }
                ) | (ListStyle::Ordered, Kind::List { ordered: true, .. })
                    | (ListStyle::Task, Kind::List { task: Some(_), .. })
            )
        });
        self.begin(EditKind::Other);
        for (n, &l) in lines.iter().enumerate() {
            let info = self.info(l);
            if matches!(info.kind, Kind::Code | Kind::Fence | Kind::Table { .. }) {
                continue;
            }
            let (s, e) = self.prefix_range(l);
            let new = if all_same {
                String::new()
            } else {
                match style {
                    ListStyle::Bullet => "- ".to_string(),
                    ListStyle::Ordered => format!("{}. ", n + 1),
                    ListStyle::Task => "- [ ] ".to_string(),
                }
            };
            self.replace_keep(Pos::new(l, s), Pos::new(l, e), &new);
        }
        self.commit();
    }

    fn toggle_quote(&mut self) {
        let (l0, l1) = self.line_range();
        let all_quoted = (l0..=l1).all(|l| self.info(l).quote > 0);
        self.begin(EditKind::Other);
        for l in l0..=l1 {
            if all_quoted {
                let n = markdown::strip_quotes(&self.doc.lines[l], 1).1;
                self.replace_keep(Pos::new(l, 0), Pos::new(l, n), "");
            } else {
                self.replace_keep(Pos::new(l, 0), Pos::new(l, 0), "> ");
            }
        }
        self.commit();
    }

    fn toggle_task(&mut self, l: usize) {
        if self.read_only() {
            return;
        }
        let info = self.info(l);
        let line = self.doc.lines[l].clone();
        self.begin(EditKind::Other);
        match info.kind {
            Kind::List {
                task: Some(done), ..
            } => {
                if let Some(i) = line[info.marker..info.content].find('[') {
                    let at = info.marker + i + 1;
                    self.replace_keep(
                        Pos::new(l, at),
                        Pos::new(l, at + 1),
                        if done { " " } else { "x" },
                    );
                }
            }
            Kind::List { .. } => {
                self.replace_keep(Pos::new(l, info.content), Pos::new(l, info.content), "[ ] ")
            }
            _ => {
                let (s, e) = self.prefix_range(l);
                self.replace_keep(Pos::new(l, s), Pos::new(l, e), "- [ ] ");
            }
        }
        self.commit();
    }

    fn move_lines(&mut self, down: bool) {
        let (l0, l1) = self.line_range();
        let n = self.doc.lines.len();
        if (!down && l0 == 0) || (down && l1 + 1 >= n) {
            return;
        }
        self.begin(EditKind::Other);
        let block = self.doc.lines[l0..=l1].join("\n");
        if down {
            let next = self.doc.lines[l1 + 1].clone();
            self.doc.replace(
                Pos::new(l0, 0),
                Pos::new(l1 + 1, next.len()),
                &format!("{next}\n{block}"),
            );
        } else {
            let prev = self.doc.lines[l0 - 1].clone();
            let end = self.doc.line_len(l1);
            self.doc.replace(
                Pos::new(l0 - 1, 0),
                Pos::new(l1, end),
                &format!("{block}\n{prev}"),
            );
        }
        let d = |p: Pos| Pos::new(if down { p.line + 1 } else { p.line - 1 }, p.col);
        self.set_sel(d(self.anchor), d(self.cursor));
        self.commit();
    }

    /// Shift+Alt+Down / Up: copy the selected lines below (moving to the copy) or
    /// above (staying on the upper copy).
    fn duplicate_lines(&mut self, down: bool) {
        let (l0, l1) = self.line_range();
        let block = self.doc.lines[l0..=l1].join("\n");
        self.begin(EditKind::Other);
        let end = Pos::new(l1, self.doc.line_len(l1));
        self.doc.replace(end, end, &format!("\n{block}"));
        if down {
            let k = l1 - l0 + 1;
            let d = |p: Pos| Pos::new(p.line + k, p.col);
            self.set_sel(d(self.anchor), d(self.cursor));
        }
        self.commit();
    }

    /// Ctrl+L: select the current line; again to extend by a line.
    fn select_line(&mut self) {
        let (a, b) = self.sel();
        let whole = a.col == 0 && b.col == 0 && b.line > a.line;
        let first = if whole { a.line } else { self.cursor.line };
        let next = if whole {
            b.line + 1
        } else {
            self.cursor.line + 1
        };
        let end = if next < self.doc.lines.len() {
            Pos::new(next, 0)
        } else {
            self.doc.end()
        };
        self.set_sel(Pos::new(first, 0), end);
        self.changed(false);
    }

    /// Ctrl+D: select the word at the caret, then each next occurrence of the
    /// selection (wrapping around the document).
    fn select_next_match(&mut self) {
        let (a, b) = self.sel();
        if a == b {
            let (s, e) = doc::word_at(&self.doc.lines[a.line], a.col);
            self.set_sel(Pos::new(a.line, s), Pos::new(a.line, e));
            return self.changed(false);
        }
        if a.line != b.line {
            return;
        }
        let needle = self.doc.text_range(a, b);
        let n = self.doc.lines.len();
        for k in 0..=n {
            let l = (b.line + k) % n;
            let from = if k == 0 { b.col } else { 0 };
            if let Some(i) = self.doc.lines[l][from..].find(&needle) {
                let start = Pos::new(l, from + i);
                self.set_sel(start, Pos::new(l, start.col + needle.len()));
                return self.changed(false);
            }
        }
    }

    /// Ctrl+/: wrap the selected lines in `<!-- -->`, or unwrap them.
    fn toggle_comment(&mut self) {
        let (l0, l1) = self.line_range();
        let lines: Vec<usize> = (l0..=l1)
            .filter(|&l| !self.doc.lines[l].trim().is_empty())
            .collect();
        if lines.is_empty() {
            return;
        }
        let commented = |s: &str| {
            let t = s.trim();
            t.starts_with("<!--") && t.ends_with("-->")
        };
        let all = lines.iter().all(|&l| commented(&self.doc.lines[l]));
        self.begin(EditKind::Other);
        for l in lines {
            let line = self.doc.lines[l].clone();
            let lead = line.len() - line.trim_start().len();
            let end = line.trim_end().len();
            if all {
                let inner = &line[lead + 4..end - 3];
                let inner = inner.strip_prefix(' ').unwrap_or(inner);
                let inner = inner.strip_suffix(' ').unwrap_or(inner);
                self.replace_keep(Pos::new(l, lead), Pos::new(l, end), inner);
            } else {
                self.replace_keep(Pos::new(l, end), Pos::new(l, end), " -->");
                self.replace_keep(Pos::new(l, lead), Pos::new(l, lead), "<!-- ");
            }
        }
        self.commit();
    }

    /// Ctrl+Enter / Ctrl+Shift+Enter: start a new empty line below / above.
    fn open_line(&mut self, below: bool) {
        let l = self.cursor.line;
        self.begin(EditKind::Other);
        let p = if below {
            let end = Pos::new(l, self.doc.line_len(l));
            self.doc.replace(end, end, "\n")
        } else {
            self.doc.replace(Pos::new(l, 0), Pos::new(l, 0), "\n");
            Pos::new(l, 0)
        };
        self.set_sel(p, p);
        self.commit();
    }

    fn source(&self) -> bool {
        self.mode == ViewMode::Source
    }

    fn toggle_source(&mut self) {
        self.set_mode(if self.source() {
            self.prev_mode
        } else {
            ViewMode::Source
        });
    }

    fn set_mode(&mut self, mode: ViewMode) {
        if self.mode != ViewMode::Source {
            self.prev_mode = self.mode;
        }
        self.mode = mode;
        let saved = Settings::update(|s| s.set("view_mode", mode.key()));
        let what = match mode {
            ViewMode::Live => "Live preview",
            ViewMode::Live2 => "Live 2: rows keep their height while you edit",
            ViewMode::View => "View: read-only",
            ViewMode::Source => "Source mode",
        };
        match saved {
            Ok(()) => self.say(what),
            Err(e) => self.say(format!("{what} (setting not saved: {e})")),
        }
        self.changed(false);
    }

    /// In View mode, says so and returns true: the edit must not happen.
    fn read_only(&mut self) -> bool {
        if self.mode == ViewMode::View {
            self.say("View mode is read-only (click View in the status bar to edit)");
            self.invalidate();
        }
        self.mode == ViewMode::View
    }

    /// In Live 2, an image line under the caret is one unit: the caret sits before or
    /// after it and Backspace/Delete remove it whole.
    fn image_unit(&mut self, line: usize) -> bool {
        if self.mode != ViewMode::Live2 {
            return false;
        }
        self.ensure_layout();
        self.view.is_image(line)
    }

    fn delete_lines(&mut self) {
        let (l0, l1) = self.line_range();
        self.begin(EditKind::Other);
        let n = self.doc.lines.len();
        if l1 + 1 < n {
            self.doc.replace(Pos::new(l0, 0), Pos::new(l1 + 1, 0), "");
        } else if l0 > 0 {
            let p = Pos::new(l0 - 1, self.doc.line_len(l0 - 1));
            self.doc.replace(p, Pos::new(l1, self.doc.line_len(l1)), "");
        } else {
            self.doc
                .replace(Pos::new(0, 0), Pos::new(l1, self.doc.line_len(l1)), "");
        }
        let p = Pos::new(l0.min(self.doc.lines.len() - 1), 0);
        self.set_sel(p, p);
        self.commit();
    }

    // -----------------------------------------------------------------------
    // Clipboard, undo

    /// The code at `p`: a whole fenced code block (without its fences), or the
    /// inline code span containing `p`. The bool is true for a block.
    fn code_at(&mut self, p: Pos) -> Option<(String, bool)> {
        self.infos();
        let infos = &self.infos;
        let info = infos[p.line];
        if info.block != 0 && matches!(info.kind, Kind::Code | Kind::Fence) {
            let code: Vec<&str> = (0..infos.len())
                .filter(|&l| infos[l].block == info.block && infos[l].kind == Kind::Code)
                .map(|l| {
                    let line = &self.doc.lines[l];
                    &line[infos[l].content.min(line.len())..]
                })
                .collect();
            return Some((code.join("\n"), true));
        }
        if matches!(info.kind, Kind::Comment) {
            return None;
        }
        let line = &self.doc.lines[p.line];
        let inl = markdown::parse_inline(line, info.content.min(line.len()), line.len());
        inl.spans
            .iter()
            .find(|(r, st)| *st == markdown::Style::Code && r.start <= p.col && p.col <= r.end)
            .map(|(r, _)| (line[r.clone()].to_string(), false))
    }

    /// Copies the code block or inline code at `p`. Returns false if there is none.
    fn copy_code(&mut self, p: Pos) -> bool {
        let Some((code, block)) = self.code_at(p) else {
            return false;
        };
        platform::set_clipboard(self.hwnd, &code);
        self.say(if block {
            "Copied the code block"
        } else {
            "Copied the inline code"
        });
        true
    }

    fn copy(&mut self, cut: bool) {
        let (a, b) = self.sel();
        if a == b && !cut && self.copy_code(a) {
            return;
        }
        if a == b {
            // Whole line, like most code editors.
            let l = a.line;
            platform::set_clipboard(self.hwnd, &format!("{}\n", self.doc.lines[l]));
            if cut {
                self.delete_lines();
            }
            return;
        }
        platform::set_clipboard(self.hwnd, &self.doc.text_range(a, b));
        if cut {
            self.begin(EditKind::Other);
            self.insert("");
            self.commit();
        }
    }

    fn paste(&mut self) {
        // Files copied in Explorer, or an image, when there is no text to paste.
        if !platform::clipboard_has_text() {
            let files = platform::clipboard_files(self.hwnd);
            if !files.is_empty() {
                return self.attach_files(&files, None);
            }
            if let Some(image) = platform::clipboard_image(self.hwnd) {
                return self.paste_image(&image);
            }
        }
        let Some(text) = platform::get_clipboard(self.hwnd) else {
            return;
        };
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        // A table cell holds one line, and pipes would split it.
        let text = if self.grid_block() != 0 {
            text.trim_end_matches('\n')
                .replace('\n', " ")
                .replace('|', r"\|")
        } else {
            text
        };
        let (a, b) = self.sel();
        let is_url = (text.starts_with("http://") || text.starts_with("https://"))
            && !text.trim().contains(char::is_whitespace);
        self.begin(EditKind::Other);
        if is_url && a != b && a.line == b.line {
            let label = self.doc.text_range(a, b);
            self.insert(&format!("[{label}]({})", text.trim()));
        } else {
            self.insert(&text);
        }
        self.commit();
    }

    fn undo(&mut self, undo: bool) {
        let r = if undo {
            self.doc.undo()
        } else {
            self.doc.redo()
        };
        if let Some((a, c)) = r {
            self.set_sel(a, c);
            self.changed(false);
        }
    }

    // -----------------------------------------------------------------------
    // Prompt (find / go to line) in the status bar

    fn open_prompt(&mut self, kind: PromptKind) {
        let (a, b) = self.sel();
        let text = match kind {
            k if k.finds() && a != b && a.line == b.line => self.doc.text_range(a, b),
            k if k.finds() => self.find_query.clone(),
            _ => String::new(),
        };
        if kind.finds() {
            self.find_query = text.clone();
        }
        self.prompt = Some(Prompt {
            kind,
            cur: text.len(),
            text,
            origin: (self.anchor, self.cursor, self.scroll),
            path: None,
            focused: true,
            other: self.replace_query.clone(),
            in_replace: false,
        });
        self.invalidate();
    }

    /// A rename / new-item prompt for `path`, starting with `text` (caret at `cur`).
    fn open_path_prompt(&mut self, kind: PromptKind, path: PathBuf, text: String, cur: usize) {
        self.slash = None;
        self.prompt = Some(Prompt {
            kind,
            cur,
            text,
            origin: (self.anchor, self.cursor, self.scroll),
            path: Some(path),
            focused: true,
            other: String::new(),
            in_replace: false,
        });
        self.invalidate();
    }

    /// Whether the prompt has the keyboard.
    fn prompt_focused(&self) -> bool {
        self.prompt.as_ref().is_some_and(|p| p.focused)
    }

    /// A click in the status bar focuses an open find prompt; a click anywhere
    /// else leaves it open but hands the keyboard back, and closes a go-to-line
    /// prompt. Returns true when the click was on the status bar prompt.
    fn prompt_click(&mut self, y: f32) -> bool {
        let on_status = y >= self.view.h - self.view.status_h();
        let Some(p) = &mut self.prompt else {
            return false;
        };
        if p.path.is_some() {
            return false;
        }
        if on_status {
            if !p.focused {
                // Searching resumes from wherever the caret is now.
                p.focused = true;
                p.cur = p.text.len();
                p.origin = (self.anchor, self.cursor, self.scroll);
                self.invalidate();
            }
            return true;
        }
        if p.kind.finds() {
            p.focused = false;
        } else {
            self.prompt = None;
        }
        self.invalidate();
        false
    }

    fn prompt_changed(&mut self) {
        let Some(p) = &self.prompt else { return };
        if p.kind == PromptKind::Replace && p.in_replace {
            self.replace_query = p.text.clone();
        } else if p.kind.finds() {
            self.find_query = p.text.clone();
            let (a, c, _) = p.origin;
            let start = a.min(c);
            self.anchor = start;
            self.cursor = start;
            self.find(true);
        }
        self.invalidate();
    }

    /// Whether the key opens a palette, which works over the prompt.
    fn opens_palette(&self, vk: u16, ctrl: bool, shift: bool) -> bool {
        let chord = Chord {
            vk,
            ctrl,
            shift,
            alt: false,
        };
        matches!(
            self.keymap.action(chord),
            Some(Action::Palette | Action::QuickOpen | Action::SearchFiles)
        )
    }

    fn prompt_key(&mut self, vk: u16, ctrl: bool, shift: bool) -> bool {
        let key = VIRTUAL_KEY(vk);
        if self.opens_palette(vk, ctrl, shift) {
            return false;
        }
        let p = self.prompt.as_mut().unwrap();
        match key {
            VK_ESCAPE => {
                let p = self.prompt.take().unwrap();
                if p.kind == PromptKind::GoTo {
                    self.set_sel(p.origin.0, p.origin.1);
                    self.scroll = p.origin.2;
                }
                self.invalidate();
            }
            VK_RETURN if p.kind == PromptKind::Replace && ctrl => self.replace_all(),
            VK_RETURN if p.kind == PromptKind::Replace && p.in_replace && !shift => {
                self.replace_one()
            }
            VK_TAB if p.kind == PromptKind::Replace => {
                std::mem::swap(&mut p.text, &mut p.other);
                p.in_replace = !p.in_replace;
                p.cur = p.text.len();
            }
            VK_RETURN => match p.kind {
                PromptKind::Find | PromptKind::Replace => self.find_next(!shift),
                PromptKind::GoTo => {
                    let n: usize = p.text.trim().parse().unwrap_or(0);
                    self.prompt = None;
                    if n > 0 {
                        let l = (n - 1).min(self.doc.lines.len() - 1);
                        self.set_sel(Pos::new(l, 0), Pos::new(l, 0));
                    }
                    self.changed(false);
                }
                kind => {
                    let p = self.prompt.take().unwrap();
                    if let Some(path) = p.path {
                        self.path_prompt_done(kind, path, &p.text);
                    }
                }
            },
            VK_BACK => {
                if p.cur > 0 {
                    let s = doc::prev_boundary(&p.text, p.cur);
                    p.text.replace_range(s..p.cur, "");
                    p.cur = s;
                    self.prompt_changed();
                }
            }
            VK_DELETE => {
                if p.cur < p.text.len() {
                    let e = doc::next_boundary(&p.text, p.cur);
                    p.text.replace_range(p.cur..e, "");
                    self.prompt_changed();
                }
            }
            VK_LEFT => p.cur = doc::prev_boundary(&p.text, p.cur),
            VK_RIGHT => p.cur = doc::next_boundary(&p.text, p.cur),
            VK_HOME => p.cur = 0,
            VK_END => p.cur = p.text.len(),
            VK_V if ctrl => {
                if let Some(t) = platform::get_clipboard(self.hwnd) {
                    let t = t.lines().next().unwrap_or("").to_string();
                    p.text.insert_str(p.cur, &t);
                    p.cur += t.len();
                    self.prompt_changed();
                }
            }
            VK_F | VK_H if ctrl && p.kind.finds() => {
                // Ctrl+F / Ctrl+H switch between find and replace, keeping the query.
                if p.in_replace {
                    std::mem::swap(&mut p.text, &mut p.other);
                    p.in_replace = false;
                    p.cur = p.text.len();
                }
                p.kind = if key == VK_F {
                    PromptKind::Find
                } else {
                    PromptKind::Replace
                };
            }
            VK_F | VK_G if ctrl => {}
            _ => return ctrl && !matches!(key, VK_F3),
        }
        self.invalidate();
        true
    }

    fn find_next(&mut self, forward: bool) {
        if self.find_query.is_empty() {
            self.open_prompt(PromptKind::Find);
            return;
        }
        if !forward {
            self.find(false);
        } else {
            let (_, b) = self.sel();
            self.anchor = b;
            self.cursor = b;
            self.find(true);
        }
    }

    /// Selects the next/previous match from the selection start.
    fn find(&mut self, forward: bool) {
        let q = self.find_query.clone();
        if q.is_empty() {
            self.changed(false);
            return;
        }
        let (a, _) = self.sel();
        let n = self.doc.lines.len();
        let mut found = None;
        for k in 0..=n {
            if forward {
                let l = (a.line + k) % n;
                let from = if k == 0 { a.col } else { 0 };
                if k == n && a.col == 0 {
                    break;
                }
                let to_limit = if k == n { a.col } else { usize::MAX };
                if let Some(i) = find_ci(&self.doc.lines[l], &q, from).filter(|&i| i < to_limit) {
                    found = Some(Pos::new(l, i));
                    break;
                }
            } else {
                let l = (a.line + n - k % n) % n;
                let before = if k == 0 {
                    a.col
                } else {
                    self.doc.lines[l].len()
                };
                if let Some(i) = rfind_ci(&self.doc.lines[l], &q, before) {
                    found = Some(Pos::new(l, i));
                    break;
                }
            }
        }
        match found {
            Some(p) => {
                self.set_sel(p, Pos::new(p.line, p.col + q.len()));
                let all = find_all(&self.doc.lines, &q);
                let index = all.iter().position(|&m| m == p).map_or(0, |i| i + 1);
                self.message = Some((format!("{index} of {}", all.len()), Instant::now()));
            }
            None => self.message = Some(("No matches".into(), Instant::now())),
        }
        self.changed(false);
    }

    /// Whether the selection is a match of the find query.
    fn selection_matches(&self) -> bool {
        let (a, b) = self.sel();
        a.line == b.line
            && !self.find_query.is_empty()
            && self
                .doc
                .text_range(a, b)
                .eq_ignore_ascii_case(&self.find_query)
    }

    /// Replaces the selected match, then selects the next one.
    fn replace_one(&mut self) {
        if self.find_query.is_empty() {
            return;
        }
        if self.selection_matches() {
            let text = self.replace_query.clone();
            self.begin(EditKind::Other);
            self.insert(&text);
            self.commit();
        }
        self.find(true);
    }

    /// Replaces every match as one undo step.
    fn replace_all(&mut self) {
        let q = self.find_query.clone();
        let all = find_all(&self.doc.lines, &q);
        if all.is_empty() {
            return self.say("No matches");
        }
        let text = self.replace_query.clone();
        self.begin(EditKind::Other);
        // Last first, so earlier positions stay valid.
        for &p in all.iter().rev() {
            self.doc
                .replace(p, Pos::new(p.line, p.col + q.len()), &text);
        }
        self.set_sel(self.cursor, self.cursor);
        self.commit();
        let n = all.len();
        self.say(format!(
            "Replaced {n} {}",
            if n == 1 { "match" } else { "matches" }
        ));
    }

    // -----------------------------------------------------------------------
    // View

    // -----------------------------------------------------------------------
    // Tables

    /// The table cell containing `p`, if any (never the delimiter row).
    fn cell_at(&mut self, p: Pos) -> Option<Cell> {
        self.infos();
        let infos = &self.infos;
        let info = infos[p.line];
        let Kind::Table { .. } = info.kind else {
            return None;
        };
        let block = info.block;
        let start = (0..=p.line)
            .rev()
            .take_while(|&l| infos[l].block == block)
            .last()?;
        let end = (p.line..infos.len())
            .take_while(|&l| infos[l].block == block)
            .last()?
            + 1;
        let row = table::row_of_line(p.line - start)?;
        let cells = markdown::split_cells(&self.doc.lines[p.line], info.qend);
        let col = table::cell_index(&cells, p.col);
        Some(Cell {
            start,
            end,
            qend: info.qend,
            row,
            col,
            range: cells.get(col).cloned().unwrap_or(p.col..p.col),
        })
    }

    /// Block id of the table being edited cell by cell: the selection is a caret in a
    /// table or lies within one cell. Any other selection touching a table shows the
    /// table as Markdown text instead (returns 0).
    fn grid_block(&mut self) -> u32 {
        if matches!(self.mode, ViewMode::Source | ViewMode::View) {
            return 0;
        }
        let (a, b) = self.sel();
        let Some(ca) = self.cell_at(a) else { return 0 };
        if a != b {
            match self.cell_at(b) {
                Some(cb) if a.line == b.line && cb.col == ca.col => {}
                _ => return 0,
            }
        }
        self.infos[a.line].block
    }

    fn table_cols(&mut self, start: usize) -> usize {
        let qend = self.info(start).qend;
        markdown::split_cells(&self.doc.lines[start], qend)
            .len()
            .max(1)
    }

    /// Byte range of cell `c` on table line `l`, first adding empty cells if the row
    /// is short.
    fn cell_range(&mut self, l: usize, c: usize) -> std::ops::Range<usize> {
        let qend = self.info(l).qend;
        let mut cells = markdown::split_cells(&self.doc.lines[l], qend);
        if c >= cells.len() {
            let line = &self.doc.lines[l];
            let mut add = String::new();
            if !line.trim_end().ends_with('|') {
                add.push_str(" |");
            }
            for _ in cells.len()..=c {
                add.push_str("   |");
            }
            let end = Pos::new(l, line.len());
            self.doc.replace(end, end, &add);
            cells = markdown::split_cells(&self.doc.lines[l], qend);
        }
        cells[c.min(cells.len() - 1)].clone()
    }

    /// Moves the caret into cell (`row`, `c`) of the table starting at line `start`.
    fn goto_cell(&mut self, start: usize, row: usize, c: usize, place: Place) {
        let l = start + table::line_of_row(row);
        let r = self.cell_range(l, c);
        let (a, b) = match place {
            Place::Start => (r.start, r.start),
            Place::End => (r.end, r.end),
            Place::All => (r.start, r.end),
        };
        self.set_sel(Pos::new(l, a), Pos::new(l, b));
        self.changed(false);
    }

    /// Moves the caret out of the table, above or below it. Below the last table in
    /// the document, a new empty line is added.
    fn leave_table(&mut self, cell: &Cell, down: bool) {
        if down {
            if cell.end < self.doc.lines.len() {
                self.set_sel(Pos::new(cell.end, 0), Pos::new(cell.end, 0));
            } else {
                self.begin(EditKind::Other);
                let end = Pos::new(cell.end - 1, self.doc.line_len(cell.end - 1));
                let p = self.doc.replace(end, end, "\n");
                self.set_sel(p, p);
                self.commit();
                return;
            }
        } else if cell.start > 0 {
            let p = Pos::new(cell.start - 1, self.doc.line_len(cell.start - 1));
            self.set_sel(p, p);
        }
        self.changed(false);
    }

    /// Keeps a caret in a table inside its cell (not on leading padding or pipes).
    /// Spaces typed after the text stay reachable: the caret may sit anywhere up to
    /// the padding space before the cell's closing pipe.
    fn snap_to_cell(&mut self) {
        if self.anchor != self.cursor || self.grid_block() == 0 {
            return;
        }
        if let Some(cell) = self.cell_at(self.cursor) {
            let line = &self.doc.lines[self.cursor.line];
            let rest = &line[cell.range.end..];
            let pipe = rest.find('|').map_or(line.len(), |i| cell.range.end + i);
            let limit = pipe.saturating_sub(1).max(cell.range.end);
            let col = self.cursor.col.clamp(cell.range.start, limit);
            let p = Pos::new(self.cursor.line, col);
            self.anchor = p;
            self.cursor = p;
        }
    }

    /// Keys for editing table cells. Returns false for keys handled as usual.
    fn table_key(&mut self, key: VIRTUAL_KEY, ctrl: bool, shift: bool) -> bool {
        let Some(cell) = self.cell_at(self.cursor) else {
            return false;
        };
        let cols = self.table_cols(cell.start);
        let rows = cell.end - cell.start - 1;
        let (r, c, l) = (cell.row, cell.col, self.cursor.line);
        let (a, b) = self.sel();
        match key {
            VK_TAB if !ctrl && shift => {
                if c > 0 {
                    self.goto_cell(cell.start, r, c - 1, Place::All);
                } else if r > 0 {
                    self.goto_cell(cell.start, r - 1, cols - 1, Place::All);
                }
            }
            VK_TAB if !ctrl => {
                if c + 1 < cols {
                    self.goto_cell(cell.start, r, c + 1, Place::All);
                } else if r + 1 < rows {
                    self.goto_cell(cell.start, r + 1, 0, Place::All);
                } else {
                    self.table_command_at(TableCmd::RowBelow, Some((r, 0)));
                }
            }
            VK_RETURN if ctrl => self.table_command(TableCmd::RowBelow),
            VK_RETURN => {
                if r + 1 < rows {
                    self.goto_cell(cell.start, r + 1, c, Place::End);
                } else {
                    self.table_command(TableCmd::RowBelow);
                }
            }
            VK_UP | VK_DOWN if !ctrl => {
                let down = key == VK_DOWN;
                if shift {
                    self.extend_cells(&cell, down);
                } else if down && r + 1 < rows {
                    self.goto_cell(cell.start, r + 1, c, Place::End);
                } else if !down && r > 0 {
                    self.goto_cell(cell.start, r - 1, c, Place::End);
                } else {
                    self.leave_table(&cell, down);
                }
            }
            VK_HOME | VK_END if !ctrl => {
                let col = if key == VK_HOME {
                    cell.range.start
                } else {
                    cell.range.end
                };
                self.move_to(Pos::new(l, col), shift);
                self.changed(false);
            }
            VK_LEFT | VK_RIGHT => {
                let left = key == VK_LEFT;
                let cur = self.cursor.col;
                let at_edge = if left {
                    cur <= cell.range.start
                } else {
                    cur >= cell.range.end
                };
                if !shift && a != b {
                    self.move_to(if left { a } else { b }, false);
                    self.changed(false);
                } else if at_edge && shift {
                    // Selections stay within one cell.
                } else if at_edge && left {
                    // Continue into the previous cell, or out of the table.
                    if c > 0 {
                        self.goto_cell(cell.start, r, c - 1, Place::End);
                    } else if r > 0 {
                        self.goto_cell(cell.start, r - 1, cols - 1, Place::End);
                    } else {
                        self.leave_table(&cell, false);
                    }
                } else if at_edge {
                    if c + 1 < cols {
                        self.goto_cell(cell.start, r, c + 1, Place::Start);
                    } else if r + 1 < rows {
                        self.goto_cell(cell.start, r + 1, 0, Place::Start);
                    } else {
                        self.leave_table(&cell, true);
                    }
                } else {
                    let line = &self.doc.lines[l];
                    let col = match (left, ctrl) {
                        (true, true) => doc::prev_word(line, cur),
                        (true, false) => doc::prev_boundary(line, cur),
                        (false, true) => doc::next_word(line, cur),
                        (false, false) => doc::next_boundary(line, cur),
                    };
                    let col = col.clamp(cell.range.start, cell.range.end);
                    self.move_to(Pos::new(l, col), shift);
                    self.changed(false);
                }
            }
            VK_DELETE | VK_BACK if self.cells.is_some() => self.clear_cells(),
            VK_C | VK_X if ctrl && self.cells.is_some() => {
                let text = self.selected_cells().join("\n");
                platform::set_clipboard(self.hwnd, &text);
                if key == VK_X {
                    self.clear_cells();
                }
            }
            // Deleting stops at the cell's edges instead of joining cells.
            VK_BACK if a == b && self.cursor.col <= cell.range.start => {}
            VK_DELETE if a == b && self.cursor.col >= cell.range.end => {}
            _ => return false,
        }
        true
    }

    /// Shift+Up/Down: select cells down the caret's column, the caret following.
    fn extend_cells(&mut self, cell: &Cell, down: bool) {
        let last = cell.end - cell.start - 2;
        let (anchor, row) = match self.cells {
            Some(cs) if cs.start == cell.start && cs.col == cell.col => (cs.anchor, cs.row),
            _ => (cell.row, cell.row),
        };
        let next = if down {
            (row + 1).min(last)
        } else {
            row.saturating_sub(1)
        };
        self.goto_cell(cell.start, next, cell.col, Place::End);
        self.cells = (next != anchor).then_some(CellSel {
            start: cell.start,
            col: cell.col,
            anchor,
            row: next,
        });
        self.invalidate();
    }

    /// The table of the cell selection, parsed.
    fn cells_grid(&mut self, cs: CellSel) -> Option<(Grid, Cell)> {
        let cell = self.cell_at(Pos::new(cs.start, 0))?;
        let lines: Vec<&str> = self.doc.lines[cell.start..cell.end]
            .iter()
            .map(String::as_str)
            .collect();
        Some((Grid::parse(&lines, cell.qend), cell))
    }

    /// Texts of the selected cells, top to bottom.
    fn selected_cells(&mut self) -> Vec<String> {
        let Some(cs) = self.cells else {
            return Vec::new();
        };
        let Some((g, _)) = self.cells_grid(cs) else {
            return Vec::new();
        };
        cs.rows()
            .filter_map(|r| g.rows.get(r)?.get(cs.col).cloned())
            .collect()
    }

    /// Delete with cells selected: empties them, as one undo step.
    fn clear_cells(&mut self) {
        let Some(cs) = self.cells.take() else { return };
        let Some((mut g, cell)) = self.cells_grid(cs) else {
            return;
        };
        for r in cs.rows() {
            if let Some(text) = g.rows.get_mut(r).and_then(|row| row.get_mut(cs.col)) {
                text.clear();
            }
        }
        self.begin(EditKind::Other);
        let last = cell.end - 1;
        let end = Pos::new(last, self.doc.line_len(last));
        self.doc
            .replace(Pos::new(cell.start, 0), end, &g.to_lines().join("\n"));
        let l = cell.start + table::line_of_row(cs.row);
        let range = self.cell_range(l, cs.col);
        self.set_sel(Pos::new(l, range.start), Pos::new(l, range.start));
        self.commit();
    }

    fn table_command(&mut self, cmd: TableCmd) {
        self.table_command_at(cmd, None);
    }

    /// Applies a table action at the caret's cell, or at (`row`, `col`) when given,
    /// rewriting the table's Markdown in one undo step.
    fn table_command_at(&mut self, cmd: TableCmd, at: Option<(usize, usize)>) {
        if self.read_only() {
            return;
        }
        let Some(cell) = self.cell_at(self.cursor) else {
            self.say("Put the cursor in a table first");
            return;
        };
        if cmd == TableCmd::DeleteTable {
            return self.delete_table(cell.start, cell.end);
        }
        let lines: Vec<&str> = self.doc.lines[cell.start..cell.end]
            .iter()
            .map(String::as_str)
            .collect();
        let mut g = Grid::parse(&lines, cell.qend);
        let (mut r, mut c) = at.unwrap_or((cell.row, cell.col));
        c = c.min(g.cols() - 1);
        match cmd {
            TableCmd::RowAbove if r == 0 => {
                return self.say("Rows can't be added above the header");
            }
            TableCmd::RowAbove => g.insert_row(r),
            TableCmd::RowBelow => {
                g.insert_row(r + 1);
                r += 1;
            }
            TableCmd::DeleteRow => {
                if !g.delete_row(r) {
                    return self.say("The header row can't be deleted");
                }
                r = r.min(g.rows.len() - 1);
            }
            TableCmd::ColumnLeft => g.insert_col(c),
            TableCmd::ColumnRight => {
                g.insert_col(c + 1);
                c += 1;
            }
            TableCmd::DeleteColumn => {
                if !g.delete_col(c) {
                    return self.say("A table needs at least one column");
                }
                c = c.min(g.cols() - 1);
            }
            _ => g.aligns[c] = cmd.align().unwrap_or(Align::Left),
        }
        self.begin(EditKind::Other);
        let last = cell.end - 1;
        let end = Pos::new(last, self.doc.line_len(last));
        self.doc
            .replace(Pos::new(cell.start, 0), end, &g.to_lines().join("\n"));
        let l = cell.start + table::line_of_row(r);
        let range = self.cell_range(l, c);
        self.set_sel(Pos::new(l, range.end), Pos::new(l, range.end));
        self.commit();
    }

    fn delete_table(&mut self, start: usize, end: usize) {
        let len = |d: &Document, l: usize| d.line_len(l);
        let (a, b) = if end < self.doc.lines.len() {
            (Pos::new(start, 0), Pos::new(end, 0))
        } else if start > 0 {
            (
                Pos::new(start - 1, len(&self.doc, start - 1)),
                Pos::new(end - 1, len(&self.doc, end - 1)),
            )
        } else {
            (Pos::new(0, 0), Pos::new(end - 1, len(&self.doc, end - 1)))
        };
        self.begin(EditKind::Other);
        self.doc.replace(a, b, "");
        self.set_sel(a, a);
        self.commit();
    }

    /// Shows the table menu at client pixel (x, y) for the caret's cell.
    fn table_menu(&mut self, x: i32, y: i32) {
        let Some(cell) = self.cell_at(self.cursor) else {
            return;
        };
        let lines: Vec<&str> = self.doc.lines[cell.start..cell.end]
            .iter()
            .map(String::as_str)
            .collect();
        let g = Grid::parse(&lines, cell.qend);
        let align = g.aligns[cell.col.min(g.cols() - 1)];
        let mut items = Vec::new();
        for (i, &(cmd, label)) in TableCmd::ALL.iter().enumerate() {
            if matches!(
                cmd,
                TableCmd::RowAbove | TableCmd::ColumnLeft | TableCmd::DeleteTable
            ) {
                items.push(platform::MenuItem::Separator);
            }
            let enabled = match cmd {
                TableCmd::RowAbove | TableCmd::DeleteRow => cell.row > 0,
                TableCmd::DeleteColumn => g.cols() > 1,
                _ => true,
            };
            items.push(platform::MenuItem::Item {
                id: i as u32 + 1,
                label,
                checked: cmd.align() == Some(align),
                enabled,
            });
        }
        if let Some(id) = platform::popup_menu(self.hwnd, &items, x, y) {
            self.table_command(TableCmd::ALL[id as usize - 1].0);
        }
    }

    /// Moves the caret to column `c` of its table row (used by the column handle).
    fn focus_column(&mut self, c: usize) {
        if let Some(cell) = self.cell_at(self.cursor)
            && cell.col != c
        {
            self.goto_cell(cell.start, cell.row, c, Place::End);
        }
    }

    /// Clicks on the add-row/add-column buttons and column handles.
    fn table_ui_click(&mut self, ui: TableUi, x: i32, y: i32) {
        let Some(cell) = self.cell_at(self.cursor) else {
            return;
        };
        match ui {
            // New columns start at their header cell, new rows at their first cell.
            TableUi::AddColumn => {
                let last = self.table_cols(cell.start) - 1;
                self.table_command_at(TableCmd::ColumnRight, Some((0, last)));
            }
            TableUi::AddRow => {
                let last = cell.end - cell.start - 2;
                self.table_command_at(TableCmd::RowBelow, Some((last, 0)));
            }
            TableUi::ColumnHandle(c) => {
                self.focus_column(c);
                self.table_menu(x, y);
            }
        }
    }

    /// Right-click: copy the code under the mouse, or in a table, place the caret in
    /// the cell and show the table menu.
    pub fn on_right_click(&mut self, x: i32, y: i32) {
        if self.palette.is_some() {
            return;
        }
        self.slash = None;
        let (dx, dy) = self.to_dip(x, y);
        if self.over_explorer(dx, dy) {
            return self.tree_right_click(dy, x, y);
        }
        self.ensure_layout();
        if self.over_outline(dx, dy) || dy < self.view.header_h() {
            return;
        }
        let dy = dy - self.view.header_h();
        if let Some(TableUi::ColumnHandle(c)) = self.view.table_ui_at(dx, dy + self.scroll) {
            self.focus_column(c);
            return self.table_menu(x, y);
        }
        let infos = self.infos.clone();
        let p = self.doc.clamp(
            self.view
                .hit(&self.doc.lines, &infos, dx, dy + self.scroll)
                .pos,
        );
        if self.spell_menu(p, x, y) || self.copy_code(p) {
            return;
        }
        if self.cell_at(p).is_none() {
            return;
        }
        let (a, b) = self.sel();
        if a == b || p < a || p > b {
            self.set_sel(p, p);
            self.changed(false);
        }
        self.ensure_layout();
        self.table_menu(x, y);
    }

    /// While dragging from inside a table cell, the selection stays in that cell;
    /// leaving the table selects across it (and it then shows as Markdown).
    fn clamp_drag(&mut self, p: Pos) -> Pos {
        let Some(cell) = self.cell_at(self.anchor) else {
            return p;
        };
        if self.infos[p.line].block != self.infos[self.anchor.line].block {
            return p;
        }
        let l = self.anchor.line;
        if p.line != l {
            return Pos::new(
                l,
                if p > self.anchor {
                    cell.range.end
                } else {
                    cell.range.start
                },
            );
        }
        Pos::new(l, p.col.clamp(cell.range.start, cell.range.end))
    }

    // -----------------------------------------------------------------------
    // Slash menu

    /// Opens the menu if the `/` just typed before the cursor can start a command.
    fn open_slash(&mut self) {
        let c = self.cursor;
        if self.source() || c.col == 0 || self.anchor != c {
            return;
        }
        let info = self.info(c.line);
        let line = &self.doc.lines[c.line];
        let col = c.col - 1;
        if line.as_bytes()[col] == b'/' && slash::can_open(line, &info, col) {
            self.slash = Some(SlashMenu {
                line: c.line,
                col,
                query: String::new(),
                matches: Vec::new(),
                selected: 0,
                notes: None,
                headings: false,
            });
            self.update_slash();
            self.invalidate();
        }
    }

    /// Opens the note menu when `[[` was just typed in a folder, outside code.
    fn open_note_menu(&mut self) {
        let c = self.cursor;
        let Some(root) = self.folder.clone() else {
            return;
        };
        if self.source() || self.anchor != c || c.col < 2 {
            return;
        }
        let info = self.info(c.line);
        let line = &self.doc.lines[c.line];
        if &line[c.col - 2..c.col] != "[[" || matches!(info.kind, Kind::Code | Kind::Fence) {
            return;
        }
        let notes = workspace::note_links(&root, &self.entries, self.doc.path.as_deref());
        self.slash = Some(SlashMenu {
            line: c.line,
            col: c.col,
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
            notes: Some(notes),
            headings: false,
        });
        self.update_slash();
        self.invalidate();
    }

    /// With the `[[` note menu open on a typed name, replaces the name with the
    /// highlighted note's (without closing the link).
    fn accept_note_name(&mut self) {
        let Some(m) = &self.slash else { return };
        let (Some(notes), false) = (&m.notes, m.headings) else {
            return;
        };
        let Some(&(item, _)) = m.matches.get(m.selected).filter(|_| !m.query.is_empty()) else {
            return;
        };
        let (at, target) = (Pos::new(m.line, m.col), notes[item].target.clone());
        self.slash = None;
        self.begin(EditKind::Other);
        let p = self.doc.replace(at, self.cursor, &target);
        self.set_sel(p, p);
        self.commit();
    }

    /// Opens the heading menu when `#` was just typed in `[[note#`: that note's
    /// headings, or the open note's for `[[#`.
    fn open_heading_menu(&mut self) {
        let c = self.cursor;
        if self.source() || self.anchor != c {
            return;
        }
        let line = &self.doc.lines[c.line];
        let Some(before) = line[..c.col].strip_suffix('#') else {
            return;
        };
        let Some(open) = before.rfind("[[") else {
            return;
        };
        let name = before[open + 2..].trim().to_string();
        if name.contains(['[', ']', '|', '#'])
            || matches!(self.info(c.line).kind, Kind::Code | Kind::Fence)
        {
            return;
        }
        let lines = if name.is_empty() {
            self.doc.lines.clone()
        } else {
            let dir = self
                .doc_dir()
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
            let Some(path) =
                workspace::resolve_note(self.folder.as_deref(), &self.entries, &dir, &name)
            else {
                return;
            };
            if self.doc.path.as_deref() == Some(path.as_path()) {
                self.doc.lines.clone()
            } else {
                let Ok(bytes) = std::fs::read(&path) else {
                    return;
                };
                Document::from_bytes(&bytes, None).0.lines
            }
        };
        let headings: Vec<NoteLink> = workspace::headings(&lines)
            .into_iter()
            .filter(|h| !h.2.is_empty())
            .map(|(_, level, text)| NoteLink {
                name: text.clone(),
                folder: format!("H{level}"),
                target: text,
            })
            .collect();
        if headings.is_empty() {
            return;
        }
        self.slash = Some(SlashMenu {
            line: c.line,
            col: c.col,
            query: String::new(),
            matches: Vec::new(),
            selected: 0,
            notes: Some(headings),
            headings: true,
        });
        self.update_slash();
        self.invalidate();
    }

    /// Refilters the menu from the text after the `/` (or `[[`), closing it when the
    /// caret leaves that text or nothing matches.
    fn update_slash(&mut self) {
        let Some(m) = &mut self.slash else { return };
        let c = self.cursor;
        let line = &self.doc.lines.get(m.line).map_or("", |l| l.as_str());
        let notes = m.notes.is_some();
        let valid = self.anchor == c
            && c.line == m.line
            && c.col <= line.len()
            && if m.headings {
                c.col >= m.col && line.as_bytes().get(m.col - 1) == Some(&b'#')
            } else if notes {
                c.col >= m.col && line.get(m.col - 2..m.col) == Some("[[")
            } else {
                c.col > m.col && line.as_bytes().get(m.col) == Some(&b'/')
            };
        let from = if notes { m.col } else { m.col + 1 };
        let query = if valid { &line[from..c.col] } else { "" };
        let matches = match &m.notes {
            _ if !valid => Vec::new(),
            Some(_) if query.contains(['[', ']', '|', '#']) => Vec::new(),
            Some(list) if m.headings => workspace::filter_headings(list, query, NOTE_MENU_ROWS),
            Some(list) => workspace::filter_notes(list, query, NOTE_MENU_ROWS),
            None if query.contains(char::is_whitespace) => Vec::new(),
            None => slash::filter(query),
        };
        if matches.is_empty() {
            self.slash = None;
            return;
        }
        if m.query != query {
            m.query = query.to_string();
            m.selected = 0;
        }
        m.matches = matches;
        m.selected = m.selected.min(m.matches.len() - 1);
    }

    /// Replaces the typed `/command` with the selected item's result, or completes
    /// the `[[` link with the selected note.
    fn slash_accept(&mut self) {
        let Some(m) = self.slash.take() else { return };
        let Some(&(item, _)) = m.matches.get(m.selected) else {
            return;
        };
        let l = m.line;
        self.begin(EditKind::Other);
        if let Some(notes) = &m.notes {
            // Replace the typed query, and a closing `]]` already there.
            let c = self.cursor;
            let end = if self.doc.lines[l][c.col..].starts_with("]]") {
                c.col + 2
            } else {
                c.col
            };
            let text = format!("{}]]", notes[item].target);
            let p = self
                .doc
                .replace(Pos::new(l, m.col), Pos::new(l, end), &text);
            self.set_sel(p, p);
            return self.commit();
        }
        let at = Pos::new(l, m.col);
        self.doc.replace(at, self.cursor, "");
        self.set_sel(at, at);
        match slash::ITEMS[item].3 {
            SlashCmd::Heading(n) => {
                let (s, e) = self.prefix_range(l);
                self.replace_keep(
                    Pos::new(l, s),
                    Pos::new(l, e),
                    &format!("{} ", "#".repeat(n as usize)),
                );
            }
            SlashCmd::Task => {
                let info = self.info(l);
                match info.kind {
                    Kind::List { task: Some(_), .. } => {}
                    Kind::List { .. } => {
                        let at = Pos::new(l, info.content);
                        self.replace_keep(at, at, "[ ] ");
                    }
                    _ => {
                        let (s, e) = self.prefix_range(l);
                        self.replace_keep(Pos::new(l, s), Pos::new(l, e), "- [ ] ");
                    }
                }
            }
            SlashCmd::Date(days) => self.insert(&slash::date_text(platform::today(), days)),
            SlashCmd::Insert(text, back) => {
                self.insert(text);
                let c = Pos::new(self.cursor.line, self.cursor.col - back);
                self.set_sel(c, c);
            }
            cmd @ (SlashCmd::CodeBlock | SlashCmd::Table | SlashCmd::Rule) => {
                let info = self.info(l);
                let line = &self.doc.lines[l];
                let edit = match cmd {
                    SlashCmd::Table => slash::table(line, &info),
                    SlashCmd::Rule => {
                        let after_para = l > 0 && self.infos[l - 1].kind == Kind::Para;
                        slash::rule(line, &info, after_para)
                    }
                    _ => slash::code_block(line, &info),
                };
                self.doc
                    .replace(Pos::new(l, edit.start), Pos::new(l, edit.end), &edit.text);
                let caret = Pos::new(l + edit.caret.0, edit.caret.1);
                self.set_sel(Pos::new(caret.line, caret.col - edit.select), caret);
            }
        }
        self.commit();
    }

    /// Handles a click while the slash menu is open. Returns true if it was on the menu.
    fn slash_click(&mut self, x: f32, y: f32) -> bool {
        let Some(g) = self.view.popup_geom else {
            self.slash = None;
            return false;
        };
        if x < g.left || x > g.right || y < g.top || y > g.bottom {
            self.slash = None;
            self.invalidate();
            return false;
        }
        if let Some(m) = &mut self.slash
            && y >= g.items_top
        {
            let row = ((y - g.items_top) / g.item_h) as usize;
            if row < m.matches.len() {
                m.selected = row;
                self.slash_accept();
            }
        }
        true
    }

    // -----------------------------------------------------------------------
    // Command palette

    fn open_palette(&mut self, p: Palette) {
        self.end_search();
        self.prompt = None;
        self.slash = None;
        self.palette = Some(p);
        self.invalidate();
    }

    /// Closes the palette; a cancelled font picker restores the previous font.
    fn close_palette(&mut self, cancel: bool) {
        if let Some(p) = self.palette.take()
            && cancel
            && let Some(font) = p.restore_font
        {
            self.preview_font(&font, p.mode == Mode::CodeFonts);
        }
        if let Some((a, c, scroll)) = self.heading_origin.take()
            && cancel
        {
            self.set_sel(a, c);
            self.scroll = scroll;
            self.clamp_scroll();
        }
        self.end_search();
        self.invalidate();
    }

    /// Shows the document in font `name`, as the text font or the code font.
    fn preview_font(&mut self, name: &str, code: bool) {
        let current = if code {
            self.view.code_font()
        } else {
            self.view.body_font()
        };
        if current != name {
            if code {
                self.view.set_code_font(name);
            } else {
                self.view.set_body_font(name);
            }
            self.relayout();
        }
    }

    /// Called after the palette query or selection changed.
    /// Go to Heading (or `@` typed first in Go to File): the open note's headings,
    /// filtered by `query`; moving through them shows each one.
    fn go_to_heading(&mut self, query: &str) {
        let origin = (self.anchor, self.cursor, self.scroll);
        let line = self.cursor.line;
        let current = self.outline_items().iter().rposition(|h| h.0 <= line);
        let mut p = Palette::headings(self.outline_items(), current);
        if !query.is_empty() {
            p.insert(query);
        }
        self.open_palette(p);
        self.heading_origin = Some(origin);
    }

    /// Goes to the heading whose text matches `name` (as a `#anchor`: case, spaces
    /// and punctuation aside), at the top of the view. Says so when there is none.
    fn go_to_heading_named(&mut self, name: &str) {
        let want = slug(name);
        let line = self
            .outline_items()
            .iter()
            .find(|h| slug(&h.2) == want)
            .map(|h| h.0);
        match line {
            Some(l) => {
                self.show_heading(l);
                self.changed(false);
            }
            None => self.say(format!("No heading \"{name}\" in {}", self.doc_name())),
        }
    }

    /// Shows the highlighted heading.
    fn preview_heading(&mut self) {
        if let Some(Target::Heading(l)) = self
            .palette
            .as_ref()
            .and_then(|p| p.selected())
            .map(|i| i.target.clone())
        {
            self.show_heading(l);
        }
    }

    /// Puts the caret on the heading on line `l`, at the top of the view.
    fn show_heading(&mut self, l: usize) {
        self.infos();
        let p = Pos::new(l, self.infos[l].content);
        self.set_sel(p, p);
        self.ensure_layout();
        if let Some(row) = self.view.rows.get(l) {
            self.scroll = row.y - 8.0 * self.view.zoom;
            self.clamp_scroll();
        }
    }

    fn palette_changed(&mut self) {
        if let Some(p) = &self.palette
            && p.mode == Mode::Files
            && let Some(rest) = p.query.strip_prefix('@')
        {
            let rest = rest.to_string();
            self.palette = None;
            return self.go_to_heading(&rest);
        }
        let Some(p) = &self.palette else { return };
        if p.mode == Mode::Headings {
            self.preview_heading();
        }
        let Some(p) = &self.palette else { return };
        if matches!(p.mode, Mode::Fonts | Mode::CodeFonts) {
            // Live preview: the document shows the highlighted font.
            let font = match p.selected().map(|item| &item.target) {
                Some(Target::Font(f)) => Some(f.clone()),
                _ => p.restore_font.clone(),
            };
            let code = p.mode == Mode::CodeFonts;
            if let Some(f) = font {
                self.preview_font(&f, code);
            }
        }
        self.search_status();
        self.invalidate();
    }

    fn palette_key(&mut self, key: VIRTUAL_KEY, ctrl: bool, shift: bool) {
        let bound = self.keymap.action(Chord {
            vk: key.0,
            ctrl,
            shift,
            alt: false,
        });
        let Some(p) = &mut self.palette else { return };
        let page = p.rows() as isize;
        // In the options list, Left/Right change the selected value.
        if p.mode == Mode::Options
            && matches!(key, VK_LEFT | VK_RIGHT)
            && let Some(Target::Option(opt)) = p.selected().map(|i| i.target.clone())
        {
            let steps = if key == VK_LEFT { -1 } else { 1 } * if shift { 5 } else { 1 };
            return self.change_options(|o| opt.adjust(o, steps));
        }
        if p.mode == Mode::FolderSettings
            && matches!(key, VK_LEFT | VK_RIGHT)
            && let Some(Target::FolderOpt(opt)) = p.selected().map(|i| i.target.clone())
        {
            let steps = if key == VK_LEFT { -1 } else { 1 } * if shift { 5 } else { 1 };
            return self.change_folder_settings(|f| opt.adjust(f, steps));
        }
        if p.mode == Mode::Preferences
            && matches!(key, VK_LEFT | VK_RIGHT)
            && let Some(Target::Pref(pref)) = p.selected().map(|i| i.target.clone())
        {
            let steps = if key == VK_LEFT { -1 } else { 1 };
            return self.change_prefs(|x| pref.adjust(x, steps));
        }
        // The key that opened the command palette or the search closes it.
        if bound == Some(Action::Palette)
            || (bound == Some(Action::SearchFiles) && p.mode == Mode::Search)
        {
            return self.close_palette(true);
        }
        let search = p.mode == Mode::Search;
        match key {
            // The first Escape clears the search, the next closes it.
            VK_ESCAPE if search && !p.query.is_empty() => p.clear(),
            VK_ESCAPE => return self.close_palette(true),
            VK_RETURN => return self.palette_accept(),
            VK_TAB if p.mode == Mode::SaveAs => p.save_complete(),
            VK_UP => p.move_by(-1),
            VK_DOWN => p.move_by(1),
            VK_PRIOR => p.move_by(-page),
            VK_NEXT => p.move_by(page),
            VK_HOME if ctrl => p.move_by(-(isize::MAX / 2)),
            VK_END if ctrl => p.move_by(isize::MAX / 2),
            VK_HOME => p.home(),
            VK_END => p.end(),
            VK_LEFT => p.left(),
            VK_RIGHT => p.right(),
            VK_BACK => p.backspace(ctrl),
            VK_DELETE => p.delete(),
            VK_V if ctrl => {
                if let Some(t) = platform::get_clipboard(self.hwnd) {
                    p.insert(t.lines().next().unwrap_or(""));
                }
            }
            _ => return,
        }
        self.palette_changed();
        // Moving through search results shows each one in the editor; changing the
        // query goes back to where the editor was.
        if search {
            match key {
                VK_UP | VK_DOWN | VK_PRIOR | VK_NEXT => self.preview_match(),
                VK_HOME | VK_END if ctrl => self.preview_match(),
                VK_ESCAPE | VK_BACK | VK_DELETE | VK_V => self.show_search_origin(),
                _ => {}
            }
        }
    }

    fn palette_accept(&mut self) {
        let Some(target) = self
            .palette
            .as_ref()
            .and_then(|p| p.selected())
            .map(|i| i.target.clone())
        else {
            return;
        };
        match target {
            // Options stay open so several can be changed in a row.
            Target::Option(opt) => return self.change_options(|o| opt.activate(o)),
            Target::ResetOptions => return self.change_options(|o| *o = RenderOptions::default()),
            Target::FolderOpt(opt) if opt.is_text() => {
                self.palette = None;
                return self.folder_text_prompt(opt);
            }
            Target::FolderOpt(opt) => return self.change_folder_settings(|f| opt.activate(f)),
            Target::Pref(pref) => return self.change_prefs(|x| pref.activate(x)),
            // Picking a folder completes it, as Tab does; the palette stays open.
            Target::SaveFolder(_) => {
                if let Some(p) = &mut self.palette {
                    p.save_complete();
                }
                return self.palette_changed();
            }
            Target::SaveInvalid(why) => return self.say(why),
            Target::SaveAs(path) => return self.save_in_folder(&path),
            _ => {}
        }
        let mode = self.palette.take().map(|p| p.mode);
        // A search result opens from where the editor was, not from its preview.
        self.end_search();
        match target {
            Target::Command(cmd) => self.palette_command(cmd),
            Target::Option(_)
            | Target::ResetOptions
            | Target::FolderOpt(_)
            | Target::Pref(_)
            | Target::SaveAs(_)
            | Target::SaveFolder(_)
            | Target::SaveInvalid(_) => {}
            Target::OpenFolderDialog => self.open_folder_dialog(),
            Target::OpenFile(path) => self.open_file_in_folder(&path),
            Target::Heading(l) => {
                self.heading_origin = None;
                self.show_heading(l);
                self.changed(false);
            }
            Target::OpenMatch {
                path,
                line,
                col,
                len,
            } => self.open_match(&path, line, col, len),
            Target::SwitchFolder(path) => self.switch_folder(&path),
            Target::Font(name) => {
                let code = mode == Some(Mode::CodeFonts);
                self.preview_font(&name, code);
                let (key, what) = if code {
                    ("code_font", "Code font")
                } else {
                    ("font", "Font")
                };
                match Settings::update(|s| s.set(key, &name)) {
                    Ok(()) => self.say(format!("{what}: {name}")),
                    Err(e) => self.say(format!("{what}: {name} (setting not saved: {e})")),
                }
            }
        }
        self.invalidate();
    }

    /// Runs a command from the palette, or from a key bound to it in the keymap.
    fn palette_command(&mut self, cmd: Command) {
        match cmd {
            Command::ChangeFont => {
                let current = self.view.body_font();
                self.open_palette(Palette::fonts(self.view.font_families(), &current, false));
            }
            Command::ChangeCodeFont => {
                let current = self.view.code_font();
                self.open_palette(Palette::fonts(self.view.font_families(), &current, true));
            }
            Command::CodeFontSize => {
                self.open_palette(Palette::options(&self.view.options(), "Code: Font size"));
            }
            Command::UiFontSize => {
                self.open_palette(Palette::options(&self.view.options(), "UI: Font size"));
            }
            Command::Table(cmd) => self.table_command(cmd),
            Command::RenderingOptions => {
                self.open_palette(Palette::options(&self.view.options(), ""));
            }
            Command::QuickOpen => self.quick_open(),
            Command::FoldHeading => self.fold(true),
            Command::UnfoldHeading => self.fold(false),
            Command::UnfoldAll => {
                self.folds.1.clear();
                self.invalidate();
            }
            Command::GoToHeading => self.go_to_heading(""),
            Command::SearchFiles => self.search_files(),
            Command::OpenFolder => self.open_folder_dialog(),
            Command::OpenRecent => self.recent_folders(),
            Command::CloseFolder => self.close_folder(),
            Command::NewFile => self.new_item_prompt(false),
            Command::NewFolder => self.new_item_prompt(true),
            Command::DailyNote => self.daily_note(),
            Command::PreviousDailyNote => self.step_daily_note(false),
            Command::NextDailyNote => self.step_daily_note(true),
            Command::ToggleExplorer => self.toggle_explorer(),
            Command::ToggleOutline => self.toggle_outline(),
            Command::FolderSettings => self.folder_settings_palette(),
            Command::Settings => self.open_palette(Palette::prefs(&self.prefs)),
            Command::ToggleSource => self.toggle_source(),
            Command::CycleViewMode => self.set_mode(self.mode.next()),
            Command::ToggleTheme => self.toggle_theme(),
            Command::OpenSettingsFolder => self.open_settings_folder(),
            Command::WriteKeymap => self.write_keymap(),
            Command::WriteTheme => self.write_theme(),
            Command::ToggleSpellCheck => self.toggle_spell_check(),
            Command::Quit => platform::close_all_windows(),
            Command::Help => {
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::process::Command::new(exe).arg("--help").spawn();
                }
            }
        }
    }

    /// Applies a change to the rendering options, re-renders, and saves it.
    fn change_options(&mut self, f: impl FnOnce(&mut RenderOptions)) {
        let mut o = self.view.options();
        f(&mut o);
        self.view.set_options(o);
        if let Some(p) = &mut self.palette {
            p.refresh_values(&o);
        }
        if let Err(e) = Settings::update(|s| o.store(s)) {
            self.say(format!("Rendering options not saved: {e}"));
        }
        self.relayout();
    }

    /// Applies a change to the general preferences and saves it.
    fn change_prefs(&mut self, f: impl FnOnce(&mut Prefs)) {
        f(&mut self.prefs);
        let prefs = self.prefs;
        if let Some(p) = &mut self.palette {
            p.refresh_prefs(&prefs);
        }
        if let Err(e) = Settings::update(|s| prefs.store(s)) {
            self.say(format!("Settings not saved: {e}"));
        }
        self.invalidate();
    }

    /// Handles a click while the palette is open: pick an item, or close when outside.
    fn palette_click(&mut self, x: f32, y: f32) {
        match self.view.palette_geom {
            Some(g) if x >= g.left && x <= g.right && y >= g.top && y <= g.bottom => {
                if y >= g.items_top
                    && let Some(p) = &mut self.palette
                {
                    p.select_visible(((y - g.items_top) / g.item_h) as usize);
                    self.palette_accept();
                }
            }
            // A click in the editor below the search panel keeps the result it shows.
            _ if self.search_previewing()
                && x >= self.view.inset()
                && x < self.view.right()
                && y < self.view.h - self.view.status_h() =>
            {
                self.palette_accept()
            }
            _ => self.close_palette(true),
        }
    }

    fn set_zoom(&mut self, z: f32) {
        self.view.zoom = clamp_zoom(z);
        let pct = (self.view.zoom * 100.0).round();
        match Settings::update(|s| s.set("zoom", &pct.to_string())) {
            Ok(()) => self.say(format!("Zoom {pct}%")),
            Err(e) => self.say(format!("Zoom {pct}% (setting not saved: {e})")),
        }
        self.relayout();
        self.scroll_to_caret();
    }

    fn toggle_theme(&mut self) {
        self.view.dark = !self.view.dark;
        self.view.apply_theme();
        platform::set_dark_titlebar(self.hwnd, self.view.dark);
        self.invalidate();
    }

    pub fn on_timer(&mut self) {
        let mut repaint = self.focused;
        if self.spell_on && self.speller.is_none() {
            self.speller();
            repaint = true;
        }
        if self.focused {
            self.caret_on = !self.caret_on;
        }
        self.autosave_tick();
        if self
            .message
            .as_ref()
            .is_some_and(|m| m.1.elapsed().as_secs() >= 4)
        {
            self.message = None;
            repaint = true;
        }
        if repaint {
            self.invalidate();
        }
        // Without focus there is no caret to blink, so the timer stops once no
        // message or autosave is waiting on it; focus, edits and messages restart it.
        if !self.focused && self.message.is_none() && !self.autosave_pending() {
            unsafe {
                let _ = KillTimer(Some(self.hwnd), 1);
            }
        }
    }

    pub fn on_focus(&mut self, focused: bool) {
        self.focused = focused;
        self.caret_on = focused;
        if focused && self.folder.is_some() {
            // Pick up files added or removed outside Penmark.
            self.rescan();
        }
        if focused {
            self.check_disk();
            self.reload_keymap();
            self.reload_theme();
            self.restart_blink();
            self.update_ime();
        }
        self.invalidate();
    }

    pub fn on_size(&mut self, pw: u32, ph: u32) {
        self.view.resize(pw, ph, self.view.dpi);
        self.fit_search_rows();
        self.relayout();
    }

    pub fn paint(&mut self) {
        self.ensure_layout();
        self.update_spelling();
        let outline_current = self.outline_state();
        let (a, b) = self.sel();
        let status_left;
        let status_right;
        let prompt_text;
        {
            let name = self.doc_name();
            let dirty = if self.doc.is_dirty() {
                "  •  modified"
            } else {
                ""
            };
            let msg = self
                .message
                .as_ref()
                .map(|m| format!("   —   {}", m.0))
                .unwrap_or_default();
            status_left = format!("{name}{dirty}{msg}");

            if self.words.0 != self.doc.version {
                self.infos();
                // Count rendered content only, not list markers or fences.
                let total = self
                    .doc
                    .lines
                    .iter()
                    .zip(&self.infos)
                    .map(|(l, i)| match i.kind {
                        Kind::Fence | Kind::Rule | Kind::Table { row: 1 } => 0,
                        _ => markdown::count_words(&l[i.content.min(l.len())..]),
                    })
                    .sum();
                self.words = (self.doc.version, total);
            }
            let line = &self.doc.lines[self.cursor.line];
            let col = line[..self.cursor.col].chars().count() + 1;
            let words = if a != b {
                format!(
                    "{} of {} words",
                    markdown::count_words(&self.doc.text_range(a, b)),
                    self.words.1
                )
            } else {
                format!("{} words", self.words.1)
            };
            let eol = if self.doc.crlf { "CRLF" } else { "LF" };
            status_right = format!(
                "Ln {}, Col {col}     {words}     {eol}",
                self.cursor.line + 1
            );
            prompt_text = self.prompt.as_ref().map(|p| {
                if p.kind == PromptKind::Replace {
                    let (find, repl) = if p.in_replace {
                        (&p.other, &p.text)
                    } else {
                        (&p.text, &p.other)
                    };
                    let head = format!("Find: {find}      Replace: ");
                    let cur = p.focused.then_some(if p.in_replace {
                        head.len() + p.cur
                    } else {
                        "Find: ".len() + p.cur
                    });
                    let extra = self
                        .message
                        .as_ref()
                        .map(|m| format!("      {}", m.0))
                        .unwrap_or_default();
                    return (format!("{head}{repl}{extra}"), cur);
                }
                let label = match p.kind {
                    PromptKind::Find | PromptKind::Replace => "Find: ",
                    PromptKind::GoTo => "Go to line: ",
                    PromptKind::Rename => "Rename to (a/b.md moves it): ",
                    PromptKind::NewFile => "New file: ",
                    PromptKind::NewFolder => "New folder: ",
                    PromptKind::FolderText(workspace::FolderOpt::DailyTemplate) => {
                        "Daily note template (a note in the folder, empty for none): "
                    }
                    PromptKind::FolderText(_) => "Daily notes folder (empty for the top folder): ",
                };
                let extra = if p.kind.finds() {
                    self.message
                        .as_ref()
                        .map(|m| format!("      {}", m.0))
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                let cur = p.focused.then_some(label.len() + p.cur);
                (format!("{label}{}{extra}", p.text), cur)
            });
        }
        let status = Status {
            left: &status_left,
            right: &status_right,
            mode: self.mode.label(),
            prompt: prompt_text.as_ref().map(|(t, c)| (t.as_str(), *c)),
        };
        let caret = self.caret_on
            && self.focused
            && !self.prompt_focused()
            && self.palette.is_none()
            && !self.tree_focus;
        let sidebar = self.sidebar_view();
        let palette = self.palette.as_ref().map(|p| {
            let (rows, selected) = p.visible();
            PaletteView {
                query: &p.query,
                cur: p.cur,
                placeholder: p.placeholder(),
                empty: p.empty_text(),
                docked: p.mode == Mode::Search,
                selected,
                items: rows
                    .into_iter()
                    .map(|(item, matches)| PaletteItem {
                        label: &item.label,
                        matches,
                        detail: &item.detail,
                        font: match &item.target {
                            Target::Font(f) if item.preview => Some(f.as_str()),
                            _ => None,
                        },
                    })
                    .collect(),
            }
        });
        // Date items show the date they insert.
        let today = platform::today();
        let dates: Vec<String> = slash::ITEMS
            .iter()
            .map(|it| match it.3 {
                SlashCmd::Date(days) => slash::date_text(today, days),
                _ => String::new(),
            })
            .collect();
        let popup = self.slash.as_ref().map(|m| PopupView {
            at: Pos::new(
                m.line,
                m.col
                    - match (m.headings, m.notes.is_some()) {
                        (true, _) => 1,
                        (_, true) => 2,
                        _ => 0,
                    },
            ),
            wide: m.notes.is_some(),
            selected: m.selected,
            items: m
                .matches
                .iter()
                .map(|(i, hits)| match &m.notes {
                    Some(notes) => PaletteItem {
                        label: &notes[*i].name,
                        matches: hits,
                        detail: &notes[*i].folder,
                        font: None,
                    },
                    None => PaletteItem {
                        label: slash::ITEMS[*i].0,
                        matches: hits,
                        detail: match slash::ITEMS[*i].3 {
                            SlashCmd::Date(_) => &dates[*i],
                            _ => slash::ITEMS[*i].1,
                        },
                        font: None,
                    },
                })
                .collect(),
        });
        self.view.cell_sel = self.cells.map(|cs| {
            let rows = cs.rows();
            (
                cs.start + table::line_of_row(*rows.start()),
                cs.start + table::line_of_row(*rows.end()),
                cs.col,
            )
        });
        let doc_name = self.doc_name();
        let folder = match (
            &self.folder,
            self.doc.path.as_deref().and_then(Path::parent),
        ) {
            (Some(root), Some(dir)) if workspace::contains(root, dir) => {
                workspace::relative(root, dir)
            }
            (_, Some(dir)) => dir.to_string_lossy().into_owned(),
            _ => String::new(),
        };
        let header = HeaderView {
            name: &doc_name,
            folder: &folder,
            dirty: self.doc.is_dirty(),
        };
        // Up to date: outline_state() sized the panel with them.
        let backlink_names: Vec<String> = if outline_current.is_some() {
            self.backlinks
                .1
                .iter()
                .map(|b| {
                    b.path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default()
                })
                .collect()
        } else {
            Vec::new()
        };
        let outline = outline_current.map(|current| OutlineView {
            items: self
                .outline_items
                .1
                .iter()
                .map(|(_, n, t)| (*n, t.as_str()))
                .collect(),
            backlinks: backlink_names
                .iter()
                .zip(&self.backlinks.1)
                .map(|(n, b)| (n.as_str(), b.text.as_str()))
                .collect(),
            current,
            hover: self.outline_hover,
            scroll: self.outline_scroll,
        });
        let infos = std::mem::take(&mut self.infos);
        self.view.paint(
            self.hwnd,
            &self.doc.lines,
            &infos,
            (a, b),
            self.cursor,
            caret,
            self.scroll,
            &status,
            palette.as_ref(),
            popup.as_ref(),
            self.mouse.filter(|_| self.palette.is_none()),
            sidebar.as_ref(),
            &header,
            outline.as_ref(),
        );
        self.infos = infos;
    }

    // -----------------------------------------------------------------------
    // Mouse

    fn to_dip(&self, x: i32, y: i32) -> (f32, f32) {
        let k = 96.0 / self.view.dpi;
        (x as f32 * k, y as f32 * k)
    }

    pub fn on_mouse_down(&mut self, x: i32, y: i32) {
        let (dx, dy) = self.to_dip(x, y);
        self.cells = None;
        if self.palette.is_some() {
            self.palette_click(dx, dy);
            return;
        }
        if self.slash.is_some() && self.slash_click(dx, dy) {
            return;
        }
        if self.view.over_mode(dx, dy) {
            return self.set_mode(self.mode.next());
        }
        if self.prompt_click(dy) {
            return;
        }
        if self.over_explorer(dx, dy) {
            return self.tree_mouse_down(dx, dy);
        }
        if self.tree_focus {
            self.tree_focus = false;
            self.invalidate();
        }
        if self.over_outline(dx, dy) {
            return self.outline_click(dy);
        }
        // From here on, y is relative to the top of the document viewport.
        let dy = dy - self.view.header_h();
        if dy < 0.0 {
            return;
        }
        if let Some(ui) = self.view.table_ui_at(dx, dy + self.scroll) {
            return self.table_ui_click(ui, x, y);
        }
        unsafe {
            SetCapture(self.hwnd);
        }
        if dy >= self.view_h() {
            return;
        }
        self.ensure_layout();
        let content = self.view.total_h + self.view_h() * 0.5;
        if dx >= self.view.right() - 12.0 && content > self.view_h() {
            let vh = self.view_h();
            let th = (vh * vh / content).max(30.0);
            let ty = self.scroll / self.view.max_scroll().max(1.0) * (vh - th);
            let off = if dy >= ty && dy <= ty + th {
                dy - ty
            } else {
                th / 2.0
            };
            self.drag = Drag::Scrollbar(off);
            self.drag_scrollbar(dy);
            return;
        }
        let now = unsafe { GetMessageTime() } as u32;
        let (t, lx, ly, n) = self.last_click;
        let dbl = unsafe { GetDoubleClickTime() };
        let count = if now.wrapping_sub(t) <= dbl && (lx - x).abs() < 5 && (ly - y).abs() < 5 {
            n % 3 + 1
        } else {
            1
        };
        self.last_click = (now, x, y, count);

        let infos = self.infos.clone();
        let hit = self.view.hit(&self.doc.lines, &infos, dx, dy + self.scroll);
        let p = self.doc.clamp(hit.pos);
        // The ⋯ after a folded heading unfolds it.
        if let Some(m) = self.view.fold_marker(p.line)
            && dx >= m.left
            && dx <= m.right
            && dy + self.scroll >= m.top - 4.0
            && dy + self.scroll <= m.bottom + 4.0
        {
            self.drag = Drag::None;
            let c = Pos::new(p.line, self.doc.line_len(p.line));
            self.set_sel(c, c);
            return self.fold(false);
        }
        if hit.checkbox && count == 1 && !shift_down() {
            self.toggle_task(p.line);
            self.drag = Drag::None;
            return;
        }
        // Rendered links open on click; on a line being edited, Ctrl+click.
        if count == 1
            && !shift_down()
            && (!hit.active || ctrl_down())
            && let Some(link) = hit.link
        {
            self.drag = Drag::None;
            if cmd::glyph(&link).is_some() {
                self.run_command(&link, &hit.link_title);
                return;
            }
            self.open_link(&link);
            return;
        }
        match count {
            1 => {
                self.move_to(p, shift_down());
                self.drag = Drag::Char;
            }
            // Live 2: double-clicking an image shows its source.
            2 if self.image_unit(p.line) => {
                self.reveal = Some(p.line);
                self.set_sel(p, p);
                self.drag = Drag::None;
            }
            2 => {
                let (s, e) = doc::word_at(&self.doc.lines[p.line], p.col);
                let (s, e) = (Pos::new(p.line, s), Pos::new(p.line, e));
                self.set_sel(s, e);
                self.drag = Drag::Word(s, e);
            }
            _ => {
                let end = if p.line + 1 < self.doc.lines.len() {
                    Pos::new(p.line + 1, 0)
                } else {
                    Pos::new(p.line, self.doc.line_len(p.line))
                };
                self.set_sel(Pos::new(p.line, 0), end);
                self.drag = Drag::Line(p.line);
            }
        }
        self.changed(false);
    }

    fn drag_scrollbar(&mut self, dy: f32) {
        let Drag::Scrollbar(off) = self.drag else {
            return;
        };
        let vh = self.view_h();
        let content = self.view.total_h + vh * 0.5;
        let th = (vh * vh / content).max(30.0);
        let frac = ((dy - off) / (vh - th).max(1.0)).clamp(0.0, 1.0);
        self.scroll = frac * self.view.max_scroll();
        self.invalidate();
    }

    pub fn on_mouse_move(&mut self, x: i32, y: i32) {
        let (dx, dy) = self.to_dip(x, y);
        match self.drag {
            Drag::None => {
                if self.explorer_visible() {
                    self.tree_mouse_move(dx, dy);
                }
                self.outline_mouse_move(dx, dy);
                self.hover(dx, dy);
            }
            Drag::Scrollbar(_) => self.drag_scrollbar(dy - self.view.header_h()),
            drag => {
                let dy = dy - self.view.header_h();
                let vh = self.view_h();
                if dy < 0.0 {
                    self.scroll = (self.scroll + dy).max(0.0);
                } else if dy > vh {
                    self.scroll = (self.scroll + dy - vh).min(self.view.max_scroll());
                }
                self.ensure_layout();
                let infos = self.infos.clone();
                let hit =
                    self.view
                        .hit(&self.doc.lines, &infos, dx, dy.clamp(0.0, vh) + self.scroll);
                let p = self.doc.clamp(hit.pos);
                match drag {
                    Drag::Char => self.cursor = self.clamp_drag(p),
                    Drag::Word(s, e) => {
                        let (ws, we) = doc::word_at(&self.doc.lines[p.line], p.col);
                        if p < s {
                            self.set_sel(e, Pos::new(p.line, ws));
                        } else {
                            self.set_sel(s, Pos::new(p.line, we).max(e));
                        }
                    }
                    Drag::Line(l) => {
                        if p.line < l {
                            let end = if l + 1 < self.doc.lines.len() {
                                Pos::new(l + 1, 0)
                            } else {
                                Pos::new(l, self.doc.line_len(l))
                            };
                            self.set_sel(end, Pos::new(p.line, 0));
                        } else {
                            let end = if p.line + 1 < self.doc.lines.len() {
                                Pos::new(p.line + 1, 0)
                            } else {
                                Pos::new(p.line, self.doc.line_len(p.line))
                            };
                            self.set_sel(Pos::new(l, 0), end);
                        }
                    }
                    _ => {}
                }
                self.caret_on = true;
                self.invalidate();
            }
        }
    }

    /// Shows the expanded command of a command link under the mouse, and highlights
    /// the link.
    fn command_hover(&mut self, x: f32, y: f32) {
        let px = (x * self.view.dpi / 96.0) as i32;
        let py = (y * self.view.dpi / 96.0) as i32;
        let mut found = None;
        if self.palette.is_none() && self.is_text_area(px, py) {
            self.ensure_layout();
            let infos = std::mem::take(&mut self.infos);
            let doc_y = y - self.view.header_h() + self.scroll;
            let hit = self.view.hit(&self.doc.lines, &infos, x, doc_y);
            self.infos = infos;
            if let Some(link) = hit.link.filter(|l| cmd::glyph(l).is_some())
                && let Some(prep) = self.prepare_command(&link, &hit.link_title)
            {
                let tip = format!("{}\nin {}", prep.line, prep.cwd.display());
                found = Some(((hit.pos.line, hit.link_start), tip));
            }
        }
        let hover = found.as_ref().map(|f| f.0);
        let tip = found.map(|f| (x, y, f.1));
        if hover != self.view.hover_link
            || tip.as_ref().map(|t| &t.2) != self.view.tooltip.as_ref().map(|t| &t.2)
        {
            self.view.hover_link = hover;
            self.view.tooltip = tip;
            self.invalidate();
        }
    }

    /// Tracks the mouse for the table controls, repainting only when they change.
    fn hover(&mut self, x: f32, y: f32) {
        if !self.tracking_leave {
            let mut tme = TRACKMOUSEEVENT {
                cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: self.hwnd,
                dwHoverTime: 0,
            };
            self.tracking_leave = unsafe { TrackMouseEvent(&mut tme) }.is_ok();
        }
        self.mouse = Some((x, y));
        let over = self.view.over_mode(x, y);
        if over != self.view.mode_hover {
            self.view.mode_hover = over;
            self.invalidate();
        }
        self.command_hover(x, y);
        let doc_y = y - self.view.header_h() + self.scroll;
        let state = (
            self.view.near_grid(x, doc_y),
            self.view.table_ui_at(x, doc_y),
            self.view.grid_column_at(x),
        );
        if state != self.table_hover {
            self.table_hover = state;
            self.invalidate();
        }
    }

    pub fn on_mouse_leave(&mut self) {
        self.tracking_leave = false;
        self.mouse = None;
        if std::mem::take(&mut self.view.mode_hover) {
            self.invalidate();
        }
        if self.tree_button.take().is_some() || self.outline_hover.take().is_some() {
            self.invalidate();
        }
        if self.view.tooltip.take().is_some() | self.view.hover_link.take().is_some() {
            self.invalidate();
        }
        if self.table_hover.0 {
            self.table_hover = (false, None, None);
            self.invalidate();
        }
    }

    pub fn on_mouse_up(&mut self) {
        self.tree_mouse_up();
        if !matches!(self.drag, Drag::None) {
            self.drag = Drag::None;
            self.update_ime();
        }
        unsafe {
            let _ = ReleaseCapture();
        }
    }

    pub fn on_wheel(&mut self, delta: i16) {
        if let Some(p) = &mut self.palette {
            let before = p.selected().map(|i| i.target.clone());
            p.scroll(-(delta as isize) * 3 / 120);
            let moved = p.selected().map(|i| i.target.clone()) != before;
            self.palette_changed();
            if moved && self.search_previewing() {
                self.preview_match();
            }
            return;
        }
        if self.mouse.is_some_and(|(x, y)| self.over_explorer(x, y)) {
            return self.tree_wheel(delta);
        }
        if self.mouse.is_some_and(|(x, y)| self.over_outline(x, y)) {
            return self.outline_wheel(delta);
        }
        if ctrl_down() {
            self.set_zoom(self.view.zoom + if delta > 0 { ZOOM_STEP } else { -ZOOM_STEP });
            return;
        }
        let mut lines = 3u32;
        unsafe {
            let _ = SystemParametersInfoW(
                SPI_GETWHEELSCROLLLINES,
                0,
                Some(&mut lines as *mut u32 as _),
                SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0),
            );
        }
        let line_h = 16.0 * 1.6 * self.view.zoom;
        let dy = -(delta as f32) / 120.0 * lines.min(10) as f32 * line_h;
        self.ensure_layout();
        self.scroll = (self.scroll + dy).clamp(0.0, self.view.max_scroll());
        self.invalidate();
    }

    /// The mouse cursor for a client point (pixels): a hand over a link that a click
    /// would open, the I-beam over text, else the arrow.
    pub fn cursor_at(&mut self, x: i32, y: i32) -> PCWSTR {
        let (dx, dy) = self.to_dip(x, y);
        if self.view.over_mode(dx, dy) {
            return IDC_HAND;
        }
        if !self.is_text_area(x, y) {
            return IDC_ARROW;
        }
        self.ensure_layout();
        let infos = std::mem::take(&mut self.infos);
        let doc_y = dy - self.view.header_h() + self.scroll;
        let hit = self.view.hit(&self.doc.lines, &infos, dx, doc_y);
        self.infos = infos;
        if hit.link.is_some() && (!hit.active || ctrl_down()) {
            IDC_HAND
        } else {
            IDC_IBEAM
        }
    }

    /// Whether the I-beam cursor applies at this client point (pixels).
    fn is_text_area(&self, x: i32, y: i32) -> bool {
        let (dx, dy) = self.to_dip(x, y);
        let over_palette = self.palette.is_some()
            && self
                .view
                .palette_geom
                .is_some_and(|g| dx >= g.left && dx <= g.right && dy >= g.top && dy <= g.bottom);
        let head = self.view.header_h();
        let over_table_ui = self.view.table_ui_at(dx, dy - head + self.scroll).is_some();
        !over_palette
            && !over_table_ui
            && !self.over_explorer(dx, dy)
            && dy >= head
            && dy < head + self.view_h()
            && dx < self.view.right() - 12.0
    }

    /// Whether a client point (DIPs) is over the outline panel.
    fn over_outline(&self, x: f32, y: f32) -> bool {
        self.outline && x >= self.view.right() && y < self.view.h - self.view.status_h()
    }

    // -----------------------------------------------------------------------
    // Outline

    /// Ctrl+Shift+O: show or hide the outline of the document's headings.
    fn toggle_outline(&mut self) {
        self.outline = !self.outline;
        self.view.outline = self.outline;
        let on = self.outline;
        self.save_setting(|s| s.set("outline", if on { "on" } else { "off" }));
        self.outline_current = None;
        self.relayout();
        self.scroll_to_caret();
    }

    /// The document's headings: (line, level, text), cached per version.
    fn outline_items(&mut self) -> &[(usize, u8, String)] {
        if self.outline_items.0 != self.doc.version {
            self.infos();
            let items = (0..self.doc.lines.len())
                .filter_map(|l| match self.infos[l].kind {
                    Kind::Heading(n) => Some((
                        l,
                        n,
                        markdown::plain_text(&self.doc.lines[l], self.infos[l].content),
                    )),
                    _ => None,
                })
                .collect();
            self.outline_items = (self.doc.version, items);
        }
        &self.outline_items.1
    }

    /// The other notes' lines that link to the open note, while the outline shows.
    fn backlinks(&mut self) -> &[workspace::Backlink] {
        let key = match (&self.folder, &self.doc.path) {
            (Some(dir), Some(p)) if self.outline && workspace::contains(dir, p) => {
                Some((p.clone(), self.scan_gen))
            }
            _ => None,
        };
        if key != self.backlinks.0 {
            self.backlinks.1 = match (&key, &self.folder) {
                (Some((path, _)), Some(dir)) => {
                    workspace::backlinks(dir, &self.entries, path, &mut self.link_cache)
                }
                _ => Vec::new(),
            };
            self.backlinks.0 = key;
        }
        &self.backlinks.1
    }

    /// Rows in the outline panel: the headings (or "No headings"), then, when other
    /// notes link here, a title row and one row per backlink.
    fn outline_rows(&mut self) -> usize {
        let headings = self.outline_items().len().max(1);
        match self.backlinks().len() {
            0 => headings,
            n => headings + 1 + n,
        }
    }

    fn outline_row_at(&mut self, y: f32) -> Option<usize> {
        let header_h = self.view.sidebar_header_h();
        if y < header_h {
            return None;
        }
        let i = ((y - header_h + self.outline_scroll) / self.view.sidebar_row_h()) as usize;
        (i < self.outline_rows()).then_some(i)
    }

    fn clamp_outline_scroll(&mut self) {
        let n = self.outline_rows() as f32;
        let h = self.view.h - self.view.status_h() - self.view.sidebar_header_h();
        let max = (n * self.view.sidebar_row_h() - h).max(0.0);
        self.outline_scroll = self.outline_scroll.clamp(0.0, max);
    }

    /// Click on a heading: move the caret there and scroll it to the top. Click on a
    /// backlink: open that note at the linking line.
    fn outline_click(&mut self, y: f32) {
        let Some(i) = self.outline_row_at(y) else {
            return;
        };
        let first = self.outline_items().len().max(1);
        if i > first {
            let b = self.backlinks()[i - first - 1].clone();
            return self.open_at(&b.path, b.line);
        }
        if i >= self.outline_items().len() {
            return;
        }
        let l = self.outline_items()[i].0;
        let p = Pos::new(l, self.infos[l].content);
        self.set_sel(p, p);
        self.changed(false);
        if let Some(row) = self.view.rows.get(l) {
            self.scroll = row.y - 8.0 * self.view.zoom;
            self.clamp_scroll();
        }
        self.invalidate();
    }

    fn outline_mouse_move(&mut self, x: f32, y: f32) {
        let hover = if self.over_outline(x, y) {
            self.outline_row_at(y)
        } else {
            None
        };
        if hover != self.outline_hover {
            self.outline_hover = hover;
            self.invalidate();
        }
    }

    fn outline_wheel(&mut self, delta: i16) {
        self.outline_scroll -= delta as f32 / 120.0 * 3.0 * self.view.sidebar_row_h();
        self.clamp_outline_scroll();
        self.invalidate();
    }

    /// Updates the outline's current heading (scrolling it into view when it
    /// changes); returns it, or None when the outline is hidden.
    fn outline_state(&mut self) -> Option<Option<usize>> {
        if !self.outline {
            return None;
        }
        let line = self.cursor.line;
        let current = self.outline_items().iter().rposition(|it| it.0 <= line);
        if current != self.outline_current {
            self.outline_current = current;
            if let Some(i) = current {
                let row_h = self.view.sidebar_row_h();
                let h = self.view.h - self.view.status_h() - self.view.sidebar_header_h();
                let top = i as f32 * row_h;
                if top < self.outline_scroll {
                    self.outline_scroll = top;
                } else if top + row_h > self.outline_scroll + h {
                    self.outline_scroll = top + row_h - h;
                }
            }
        }
        self.clamp_outline_scroll();
        Some(current)
    }

    /// Whether a client point (DIPs) is over the explorer panel.
    fn over_explorer(&self, x: f32, y: f32) -> bool {
        self.explorer_visible()
            && x < self.view.sidebar_w()
            && y < self.view.h - self.view.status_h()
    }

    // -----------------------------------------------------------------------
    // Files

    pub fn show_help(&mut self) {
        self.doc = Document::from_bytes(HELP.as_bytes(), None).0;
        self.untitled_name = "Help";
        self.infos_ver = None;
        self.relayout();
        self.changed(false);
    }

    /// Where the open file is, for the back / forward history.
    fn nav_entry(&self) -> Option<NavEntry> {
        Some(NavEntry {
            path: self.doc.path.clone()?,
            cursor: self.cursor,
            scroll: self.scroll,
        })
    }

    /// Mouse back / forward buttons, Alt+Left / Alt+Right: the previous or next file
    /// in this window's history, at the position it was left.
    pub fn navigate(&mut self, back: bool) {
        let entry = loop {
            let stack = if back {
                &mut self.back
            } else {
                &mut self.forward
            };
            match stack.pop() {
                Some(e) if e.path.is_file() => break e,
                Some(_) => {}
                None => {
                    return self.say(if back {
                        "No previous file"
                    } else {
                        "No next file"
                    });
                }
            }
        };
        if !self.confirm_discard() {
            let stack = if back {
                &mut self.back
            } else {
                &mut self.forward
            };
            stack.push(entry);
            return;
        }
        let current = self.nav_entry();
        self.read_path(&entry.path);
        if self.doc.path.as_deref() != Some(entry.path.as_path()) {
            return; // Could not be read; open_path reported it.
        }
        if let Some(c) = current {
            let other = if back {
                &mut self.forward
            } else {
                &mut self.back
            };
            other.push(c);
        }
        let p = self.doc.clamp(entry.cursor);
        self.set_sel(p, p);
        self.scroll = entry.scroll;
        self.clamp_scroll();
        self.reveal_in_tree(&entry.path);
        self.changed(true);
    }

    /// Opens `path` with the caret at the start of `line`, centered.
    fn open_at(&mut self, path: &Path, line: usize) {
        if !self.confirm_discard() {
            return;
        }
        self.open_path(path);
        if self.doc.path.as_deref() != Some(path) {
            return;
        }
        let p = self.doc.clamp(Pos::new(line, 0));
        self.set_sel(p, p);
        self.changed(false);
        self.center_caret();
        self.reveal_in_tree(path);
    }

    /// Opens `path`, recording the file being left in the back history.
    pub fn open_path(&mut self, path: &Path) {
        let leaving = self.nav_entry().filter(|e| e.path != path);
        self.read_path(path);
        if let Some(e) = leaving
            && self.doc.path.as_deref() == Some(path)
        {
            if self.back.len() >= NAV_MAX {
                self.back.remove(0);
            }
            self.back.push(e);
            self.forward.clear();
        }
    }

    fn read_path(&mut self, path: &Path) {
        // Stamped before reading, so a write in between shows up as a change.
        let stamp = doc::stamp(path);
        match std::fs::read(path) {
            Ok(bytes) => {
                let (mut doc, lossy) = Document::from_bytes(&bytes, Some(path.to_path_buf()));
                doc.disk = stamp;
                self.doc = doc;
                self.infos_ver = None;
                self.set_sel(Pos::default(), Pos::default());
                self.scroll = 0.0;
                self.prompt = None;
                self.relayout();
                self.say(if lossy {
                    "Opened (file was not valid UTF-8; invalid bytes replaced)"
                } else {
                    "Opened"
                });
                // After `say`, so a settings write failure is not hidden by it.
                self.remember_file();
                self.changed(false);
            }
            Err(e) => {
                platform::message_box(
                    self.hwnd,
                    &format!("Could not open {}:\n{e}", path.display()),
                    APP_NAME,
                    MB_OK | MB_ICONERROR,
                );
            }
        }
    }

    fn write_to(&mut self, path: &Path) -> bool {
        if self.doc.path.as_deref() == Some(path)
            && !(self.confirm_overwrite() && self.confirm_lossy_save())
        {
            return false;
        }
        match std::fs::write(path, self.doc.to_bytes()) {
            Ok(()) => {
                let new_path = self.doc.path.as_deref() != Some(path);
                self.doc.path = Some(path.to_path_buf());
                self.doc.mark_saved();
                self.doc.lossy = false;
                self.doc.disk = doc::stamp(path);
                self.say("Saved");
                self.update_title();
                if keymap::path().as_deref() == Some(path) {
                    self.reload_keymap();
                }
                if theme::path().as_deref() == Some(path) {
                    self.reload_theme();
                }
                if new_path && self.folder.is_some() {
                    self.rescan();
                    self.remember_file();
                }
                true
            }
            Err(e) => {
                platform::message_box(
                    self.hwnd,
                    &format!("Could not save {}:\n{e}", path.display()),
                    APP_NAME,
                    MB_OK | MB_ICONERROR,
                );
                false
            }
        }
    }

    pub fn save(&mut self) -> bool {
        match self.doc.path.clone() {
            Some(p) => self.write_to(&p),
            None => self.save_as(),
        }
    }

    /// Save As. With a folder open (and the note in it or untitled), the path is
    /// picked in the palette, so this returns false: the note is saved later, when
    /// the path is chosen.
    fn save_as(&mut self) -> bool {
        if self.save_palette() {
            return false;
        }
        let suggested = self.suggested_name();
        let dir = self.doc_dir().or_else(|| self.folder.clone());
        match platform::save_dialog(
            self.hwnd,
            self.doc.path.as_deref(),
            dir.as_deref(),
            &suggested,
        ) {
            Some(p) => self.write_to(&p),
            None => false,
        }
    }

    /// A file name for an untitled note, from its first line of text.
    fn suggested_name(&self) -> String {
        self.doc
            .lines
            .iter()
            .find(|l| !l.trim().is_empty())
            .map(|l| {
                l.trim_start_matches(['#', ' ', '>', '-'])
                    .chars()
                    .filter(|c| !"\\/:*?\"<>|".contains(*c))
                    .take(60)
                    .collect::<String>()
            })
            .filter(|s| !s.trim().is_empty())
            .map(|s| format!("{}.md", s.trim()))
            .unwrap_or_else(|| "Untitled.md".into())
    }

    /// Whether the open file's text on disk differs from what it was when read or
    /// last saved (another program, a sync), checking the bytes when the stamp moved.
    fn disk_differs(&mut self) -> bool {
        if self.doc.disk_change() != Some(DiskChange::Modified) {
            return false;
        }
        let path = self.doc.path.clone().unwrap();
        let stamp = doc::stamp(&path);
        match std::fs::read(&path) {
            Ok(bytes) if bytes != self.doc.to_bytes() => true,
            Ok(_) => {
                // Touched but not changed.
                self.doc.disk = stamp;
                false
            }
            Err(_) => false,
        }
    }

    /// Before saving over the open file: asks when it changed on disk since it was
    /// read. Returns false to leave the file alone.
    fn confirm_overwrite(&mut self) -> bool {
        if !self.disk_differs() {
            return true;
        }
        let name = self.doc_name();
        let r = platform::message_box(
            self.hwnd,
            &format!(
                "{name} changed on disk since it was opened.

Overwrite it with your version?"
            ),
            APP_NAME,
            MB_YESNO | MB_ICONWARNING,
        );
        r == IDYES
    }

    /// Before saving over a file whose invalid UTF-8 was replaced when it was read:
    /// asks, since the original bytes are lost. Returns false to leave it alone.
    fn confirm_lossy_save(&mut self) -> bool {
        if !self.doc.lossy {
            return true;
        }
        let name = self.doc_name();
        let r = platform::message_box(
            self.hwnd,
            &format!(
                "{name} was not valid UTF-8 when it was opened, and its invalid bytes \
                 were replaced with \u{FFFD}.\n\nSave over it anyway? The original bytes \
                 will be lost."
            ),
            APP_NAME,
            MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2,
        );
        r == IDYES
    }

    /// Notices the open file changing outside Penmark (on focus): reloads it when
    /// there are no unsaved changes, and otherwise asks.
    fn check_disk(&mut self) {
        if self.search_previewing() {
            return;
        }
        if self.doc.disk_change() == Some(DiskChange::Deleted) {
            // Report it once; saving recreates the file.
            self.doc.disk = None;
            return self.say(format!("{} was deleted on disk", self.doc_name()));
        }
        if !self.disk_differs() {
            return;
        }
        let path = self.doc.path.clone().unwrap();
        let stamp = doc::stamp(&path);
        if self.doc.is_dirty() {
            let name = self.doc_name();
            let r = platform::message_box(
                self.hwnd,
                &format!(
                    "{name} changed on disk.

Reload it and lose your unsaved changes?"
                ),
                APP_NAME,
                MB_YESNO | MB_ICONWARNING,
            );
            if r != IDYES {
                // Keep this text; saving it overwrites the file without asking again.
                self.doc.disk = stamp;
                return;
            }
        }
        let Ok(bytes) = std::fs::read(&path) else {
            return;
        };
        self.reload_text(&bytes);
        self.doc.disk = stamp;
        self.say("Reloaded (changed on disk)");
    }

    /// Replaces the text with the file's as one undo step, keeping the caret and
    /// scroll, so `Ctrl+Z` brings back what was there.
    fn reload_text(&mut self, bytes: &[u8]) {
        let fresh = Document::from_bytes(bytes, None).0;
        let end = self.doc.end();
        self.begin(EditKind::Other);
        self.doc.replace(
            Pos::default(),
            end,
            &fresh.lines.join(
                "
",
            ),
        );
        self.set_sel(self.anchor, self.cursor);
        self.commit();
        self.doc.crlf = fresh.crlf;
        self.doc.bom = fresh.bom;
        self.doc.lossy = fresh.lossy;
        self.doc.mark_saved();
        self.clamp_scroll();
        self.update_title();
    }

    /// Asks to save unsaved changes. Returns false if the user cancelled.
    pub fn confirm_discard(&mut self) -> bool {
        // Ask about the open document, not a search result shown in its place.
        if self.search_origin.is_some() {
            self.close_palette(true);
        }
        if !self.doc.is_dirty() {
            return true;
        }
        if self.autosave_applies() {
            return self.save();
        }
        let name = self.doc_name();
        let r = platform::message_box(
            self.hwnd,
            &format!("Save changes to {name}?"),
            APP_NAME,
            MB_YESNOCANCEL | MB_ICONWARNING,
        );
        match r {
            IDYES => self.save(),
            IDNO => true,
            _ => false,
        }
    }

    fn new_doc(&mut self) {
        if !self.confirm_discard() {
            return;
        }
        self.doc = Document::new();
        self.infos_ver = None;
        self.set_sel(Pos::default(), Pos::default());
        self.scroll = 0.0;
        self.relayout();
        self.changed(false);
    }

    fn new_window(&mut self) {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).arg("--new").spawn();
        }
    }

    fn open(&mut self) {
        if !self.confirm_discard() {
            return;
        }
        let start = self.doc_dir().or_else(|| self.folder.clone());
        if let Some(p) = platform::open_dialog(self.hwnd, start.as_deref()) {
            self.open_path(&p);
        }
    }

    fn open_link(&mut self, target: &str) {
        let dir = self
            .doc_dir()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        if let Some(inner) = target.strip_prefix("[[").and_then(|t| t.strip_suffix("]]")) {
            let (name, heading) = match inner.split_once('#') {
                Some((n, h)) => (n.trim(), Some(h.trim())),
                None => (inner.trim(), None),
            };
            let found = if name.is_empty() {
                // `[[#heading]]`: this note.
                self.doc.path.clone()
            } else {
                workspace::resolve_note(self.folder.as_deref(), &self.entries, &dir, name)
            };
            if found.is_some() && found == self.doc.path {
                if let Some(h) = heading {
                    self.go_to_heading_named(h);
                }
                return;
            }
            if !self.confirm_discard() {
                return;
            }
            if let Some(path) = found {
                self.open_path(&path);
                self.reveal_in_tree(&path);
                if let Some(h) = heading.filter(|_| self.doc.path.as_ref() == Some(&path)) {
                    self.go_to_heading_named(h);
                }
            } else if name.is_empty() {
                return;
            } else {
                let file = if workspace::is_markdown(Path::new(name)) {
                    name.to_string()
                } else {
                    format!("{name}.md")
                };
                let path = dir.join(file);
                self.doc = Document::new();
                self.doc.path = Some(path);
                self.infos_ver = None;
                self.set_sel(Pos::default(), Pos::default());
                self.scroll = 0.0;
                self.relayout();
                self.say("New note (not saved yet)");
                self.changed(false);
            }
            return;
        }
        if let Some(anchor) = target.strip_prefix('#') {
            self.go_to_heading_named(anchor);
            return;
        }
        if workspace::has_scheme(target) {
            platform::shell_open(self.hwnd, target);
            return;
        }
        let (file, anchor) = target.split_once('#').unwrap_or((target, ""));
        let path = dir.join(attach::decode(file));
        let is_md = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"));
        if is_md && path.exists() {
            let here = self
                .doc
                .path
                .as_deref()
                .is_some_and(|p| workspace::same_path(p, &path));
            if !here && self.confirm_discard() {
                self.open_path(&path);
            }
            if !anchor.is_empty() && (here || self.doc.path.as_deref() == Some(path.as_path())) {
                self.go_to_heading_named(&attach::decode(anchor));
            }
        } else {
            platform::shell_open(self.hwnd, &path.to_string_lossy());
        }
    }

    // -----------------------------------------------------------------------
    // Command links

    /// A command link ready to run: the full command line and its folder.
    fn prepare_command(&self, target: &str, title: &str) -> Option<Prepared> {
        let c = cmd::parse(target, title)?;
        let note_dir = self
            .doc_dir()
            .or_else(|| self.folder.clone())
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        let cwd = match &c.cwd {
            Some(d) => note_dir.join(platform::expand_env(d)),
            None => note_dir,
        };
        let text = match c.mode {
            CmdMode::Run => platform::expand_env(&c.text),
            CmdMode::Shell => c.text.clone(),
        };
        let comspec = std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".into());
        let line = cmd::command_line(&c, &text, &cwd, &comspec);
        Some(Prepared { line, cwd, c })
    }

    /// Clicking a `run:` / `shell:` link: asks first unless this exact command was
    /// allowed before, then starts it.
    fn run_command(&mut self, target: &str, title: &str) {
        let Some(Prepared { line, cwd, c }) = self.prepare_command(target, title) else {
            return;
        };
        if c.text.is_empty() {
            return self.say("The command is empty");
        }
        if !cwd.is_dir() {
            return self.say(format!("Not run: no folder {}", cwd.display()));
        }
        let key = cmd::trust_key(&line, &cwd);
        let trusted = Settings::load()
            .get("trusted_commands")
            .is_some_and(|v| v.split(' ').any(|k| k == key));
        if c.confirm || !trusted {
            let later = if c.confirm {
                ""
            } else {
                "\n\nPenmark won't ask again for this command unless it changes."
            };
            let r = platform::message_box(
                self.hwnd,
                &format!("Run this command?\n\n{line}\n\nin {}{later}", cwd.display()),
                APP_NAME,
                MB_YESNO | MB_ICONQUESTION | MB_DEFBUTTON2,
            );
            if r != IDYES {
                return;
            }
            if !c.confirm {
                self.save_setting(|s| {
                    let mut keys: Vec<String> = s
                        .get("trusted_commands")
                        .unwrap_or("")
                        .split(' ')
                        .filter(|k| !k.is_empty() && *k != key)
                        .map(String::from)
                        .collect();
                    keys.push(key.clone());
                    let skip = keys.len().saturating_sub(TRUSTED_MAX);
                    s.set("trusted_commands", &keys[skip..].join(" "));
                });
            }
        }
        let label = c.text.chars().take(60).collect::<String>();
        match platform::spawn(&line, &cwd, c.window == CmdWindow::Hidden) {
            Err(e) => self.say(format!("Could not run {label}: {}", e.trim())),
            Ok(process) if c.wait => {
                self.next_command += 1;
                let id = self.next_command;
                self.running.push((id, label.clone()));
                platform::watch_process(process, self.hwnd, WM_APP_COMMAND_DONE, id);
                self.say(format!("Running {label}"));
            }
            Ok(process) => {
                platform::close_handle(process);
                self.say(format!("Started {label}"));
            }
        }
    }

    /// A command run with `wait` ended.
    pub fn command_done(&mut self, code: u32, id: isize) {
        let Some(i) = self.running.iter().position(|r| r.0 == id) else {
            return;
        };
        let (_, label) = self.running.remove(i);
        self.say(if code == 0 {
            format!("Finished {label}")
        } else {
            format!("{label} failed with exit code {}", code as i32)
        });
    }

    // -----------------------------------------------------------------------
    // Attachments: dropped and pasted files

    /// Files dragged over the window, at client pixel (x, y): shows where they would
    /// be inserted. Returns whether they can be dropped there. Without a folder a
    /// drop opens the file instead.
    pub fn drag_over(&mut self, x: i32, y: i32) -> bool {
        if self.folder.is_none() {
            return true;
        }
        let (dx, dy) = self.to_dip(x, y);
        let (head, vh) = (self.view.header_h(), self.view_h());
        if self.over_explorer(dx, dy) || self.over_outline(dx, dy) || dy < head || dy >= head + vh {
            self.set_drop_caret(None);
            return false;
        }
        // Scroll when near the top or bottom edge.
        let ey = dy - head;
        let edge = 30.0;
        if ey < edge || ey > vh - edge {
            let step = if ey < edge { -16.0 } else { 16.0 } * self.view.zoom;
            self.scroll = (self.scroll + step).clamp(0.0, self.view.max_scroll());
            self.invalidate();
        }
        self.ensure_layout();
        let infos = std::mem::take(&mut self.infos);
        let hit = self.view.hit(&self.doc.lines, &infos, dx, ey + self.scroll);
        self.infos = infos;
        let p = self.doc.clamp(hit.pos);
        self.set_drop_caret(Some(p));
        true
    }

    pub fn drag_leave(&mut self) {
        self.set_drop_caret(None);
    }

    fn set_drop_caret(&mut self, p: Option<Pos>) {
        if self.view.drop_caret != p {
            self.view.drop_caret = p;
            self.invalidate();
        }
    }

    /// Files were dropped. They are handled after the drag ends (WM_APP_DROP), so a
    /// message box cannot hold up the app they were dragged from.
    pub fn drop_files(&mut self, files: Vec<PathBuf>) {
        let at = self.view.drop_caret.take();
        self.invalidate();
        if files.is_empty() {
            return;
        }
        self.pending_drop = (files, at);
        unsafe {
            let _ = PostMessageW(Some(self.hwnd), WM_APP_DROP, WPARAM(0), LPARAM(0));
        }
    }

    pub fn finish_drop(&mut self) {
        let (files, at) = std::mem::take(&mut self.pending_drop);
        let Some(first) = files.first() else { return };
        if self.folder.is_none() {
            if self.confirm_discard() {
                self.open_path(first);
            }
            return;
        }
        self.attach_files(&files, at);
    }

    /// Links `files` into the note at `at` (else the selection), copying those from
    /// outside the folder into the note's `data` folder.
    fn attach_files(&mut self, files: &[PathBuf], at: Option<Pos>) {
        if self.read_only() {
            return;
        }
        let (Some(root), Some(note)) = (self.folder.clone(), self.doc.path.clone()) else {
            return self.say("Open a folder and save this note to attach files");
        };
        let (links, err) = attach::attach(files, &note, &root);
        self.insert_links(&links, at);
        if let Some(e) = err {
            self.say(e);
        } else if links.len() > 1 {
            self.say(format!("Attached {} files", links.len()));
        }
        self.rescan();
    }

    /// Ctrl+V with an image on the clipboard: saved as a PNG in the `data` folder.
    fn paste_image(&mut self, image: &platform::ClipImage) {
        if self.read_only() {
            return;
        }
        let (Some(_), Some(note)) = (&self.folder, self.doc.path.clone()) else {
            return self.say("Open a folder and save this note to paste images");
        };
        let saved = attach::new_file(&attach::pasted_name(platform::now()), &note)
            .map_err(|e| e.to_string())
            .and_then(|path| {
                platform::save_png(image, &path).map_err(|e| e.message())?;
                Ok(path)
            });
        match saved {
            Ok(path) => {
                let dir = note.parent().unwrap_or(Path::new("")).to_path_buf();
                self.insert_links(&[attach::link(&dir, &path)], None);
                self.rescan();
            }
            Err(e) => self.say(format!("Could not save the image: {e}")),
        }
    }

    /// Inserts links at `at` (else over the selection) as one undo step. Images go on
    /// lines of their own, so they render.
    fn insert_links(&mut self, links: &[String], at: Option<Pos>) {
        if links.is_empty() {
            return;
        }
        let image = links.iter().any(|l| l.starts_with('!'));
        let mut text = links.join(if image { "\n" } else { " " });
        self.begin(EditKind::Other);
        if let Some(p) = at {
            self.set_sel(p, p);
        }
        let a = self.sel().0;
        if image {
            let info = self.info(a.line);
            if !self.doc.lines[a.line][info.content.min(a.col)..a.col]
                .trim()
                .is_empty()
            {
                text.insert(0, '\n');
            }
            // The caret goes below, so the image shows rendered.
            text.push('\n');
        }
        self.insert(&text);
        self.commit();
    }
}

/// A command link resolved for running.
struct Prepared {
    line: String,
    cwd: PathBuf,
    c: cmd::Command,
}

#[derive(Clone, Copy)]
enum ListStyle {
    Bullet,
    Ordered,
    Task,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An editor without a window, holding `text`.
    pub(super) fn app(text: &str) -> App {
        let mut a = App::new(View::new(false).unwrap());
        a.doc = Document::from_bytes(text.as_bytes(), None).0;
        // Tests share one settings file; another test may have saved View mode.
        a.mode = ViewMode::default();
        a
    }

    #[test]
    fn reloads_a_clean_file_changed_on_disk() {
        let path = std::env::temp_dir().join(format!("penmark-reload-{}.md", std::process::id()));
        std::fs::write(&path, "one\ntwo\nthree").unwrap();
        let mut a = app("");
        a.read_path(&path);
        a.set_sel(Pos::new(1, 2), Pos::new(1, 2));
        std::fs::write(&path, "one\r\nTWO!\r\nthree\r\nfour").unwrap();
        a.check_disk();
        assert_eq!(a.doc.lines, ["one", "TWO!", "three", "four"]);
        assert!(a.doc.crlf && !a.doc.is_dirty());
        assert_eq!(a.cursor, Pos::new(1, 2));
        // Touching the file without changing it is not a change.
        std::fs::write(&path, "one\r\nTWO!\r\nthree\r\nfour").unwrap();
        assert!(!a.disk_differs());
        // The reload is one undo step.
        a.undo(true);
        assert_eq!(a.doc.lines, ["one", "two", "three"]);
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn hash_in_note_menu_completes_headings_and_links_open_there() {
        let root = std::env::temp_dir().join(format!("penmark-sections-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("plan.md"),
            "# Plan\n\n## Goals\n\n## Risk Items\nx",
        )
        .unwrap();
        std::fs::write(root.join("note.md"), "").unwrap();
        let mut a = app("# Here\n\n");
        a.folder = Some(root.clone());
        a.entries = workspace::scan(&root);
        a.doc.path = Some(root.join("note.md"));
        a.set_sel(Pos::new(2, 0), Pos::new(2, 0));
        let typed = |a: &mut App, s: &str| s.chars().for_each(|c| a.on_char(c as u16));
        // `#` takes the highlighted note and lists its headings.
        typed(&mut a, "[[pl#");
        assert_eq!(a.doc.lines[2], "[[plan#");
        let m = a.slash.as_ref().expect("heading menu");
        let names: Vec<&str> = m
            .notes
            .as_ref()
            .unwrap()
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        assert_eq!(names, ["Plan", "Goals", "Risk Items"]);
        let order: Vec<usize> = m.matches.iter().map(|x| x.0).collect();
        assert_eq!(order, [0, 1, 2], "in document order");
        typed(&mut a, "ri");
        a.slash_accept();
        assert_eq!(a.doc.lines[2], "[[plan#Risk Items]]");
        // `[[#` lists this note's headings.
        a.set_sel(Pos::new(1, 0), Pos::new(1, 0));
        typed(&mut a, "[[#");
        let m = a.slash.as_ref().expect("this note's headings");
        assert_eq!(m.notes.as_ref().unwrap()[0].name, "Here");
        a.slash = None;
        // Links with a section go to it, matched like an anchor.
        a.doc.mark_saved();
        a.open_link("[[#here]]");
        assert_eq!(a.cursor.line, 0);
        a.open_link("[[plan#risk items]]");
        assert_eq!(a.doc.path.as_deref(), Some(root.join("plan.md").as_path()));
        assert_eq!(a.cursor.line, 4);
        a.open_link("plan.md#goals");
        assert_eq!(a.cursor.line, 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn folded_section_ranges() {
        let h = |l: usize, n: u8| (l, n, String::new());
        let heads = [h(0, 1), h(2, 2), h(5, 2), h(7, 1)];
        let f = |lines: &[usize]| folded_sections(&heads, &lines.iter().copied().collect(), 9);
        assert_eq!(f(&[0]), [(0, 1..7)]);
        assert_eq!(f(&[2, 7]), [(2, 3..5), (7, 8..9)]);
        assert_eq!(f(&[5]), [(5, 6..7)]);
    }

    #[test]
    fn folds_follow_their_heading_and_open_for_the_caret() {
        let mut a = app("# A\na1\n## B\nb1\nb2\n## C\nc1\n## B\nb3");
        a.set_sel(Pos::new(3, 1), Pos::new(3, 1));
        a.fold(true);
        a.update_folds();
        assert_eq!(a.cursor, Pos::new(2, 3), "caret moves to the heading");
        assert_eq!(a.view.folds, HashSet::from([2]));
        // A line added above moves the fold with its heading, not to the other "B".
        a.begin(EditKind::Other);
        a.doc.replace(Pos::new(0, 0), Pos::new(0, 0), "new\n");
        a.commit();
        a.set_sel(Pos::new(3, 0), Pos::new(3, 0));
        a.update_folds();
        assert_eq!(a.view.folds, HashSet::from([3]));
        // The caret going into the section (find, go to line) unfolds it.
        a.set_sel(Pos::new(5, 0), Pos::new(5, 0));
        a.update_folds();
        assert!(a.view.folds.is_empty() && a.folds.1.is_empty());
    }

    #[test]
    fn go_to_heading_previews_and_escape_returns() {
        let mut a = app("# A\ntext\n## B\nmore\n## C\nend");
        a.set_sel(Pos::new(3, 2), Pos::new(3, 2));
        a.go_to_heading("");
        // Starts on the heading the caret is under.
        let selected = |a: &App| {
            a.palette
                .as_ref()
                .unwrap()
                .selected()
                .unwrap()
                .target
                .clone()
        };
        assert_eq!(selected(&a), Target::Heading(2));
        a.palette_key(VK_DOWN, false, false);
        assert_eq!(a.cursor, Pos::new(4, 3));
        a.palette_key(VK_ESCAPE, false, false);
        assert!(a.palette.is_none());
        assert_eq!(a.cursor, Pos::new(3, 2));
        // Enter keeps the heading; `@` in Go to File switches to headings.
        a.go_to_heading("c");
        a.palette_key(VK_RETURN, false, false);
        assert_eq!(a.cursor, Pos::new(4, 3));
        a.open_palette(Palette::files(Path::new("."), &[], None));
        a.on_char('@' as u16);
        a.on_char('a' as u16);
        assert_eq!(selected(&a), Target::Heading(0));
    }

    #[test]
    fn replace_one_and_all() {
        let mut a = app("cat Cat\ndog cat");
        a.open_prompt(PromptKind::Replace);
        for c in "cat".chars() {
            a.on_char(c as u16);
        }
        assert_eq!(a.sel(), (Pos::new(0, 0), Pos::new(0, 3)));
        // Tab moves to the replacement field; Enter there replaces and moves on.
        a.prompt_key(VK_TAB.0, false, false);
        for c in "cow".chars() {
            a.on_char(c as u16);
        }
        a.prompt_key(VK_RETURN.0, false, false);
        assert_eq!(a.doc.lines, ["cow Cat", "dog cat"]);
        assert_eq!(a.sel(), (Pos::new(0, 4), Pos::new(0, 7)));
        // Ctrl+Enter replaces the rest as one undo step.
        a.prompt_key(VK_RETURN.0, true, false);
        assert_eq!(a.doc.lines, ["cow cow", "dog cow"]);
        a.undo(true);
        assert_eq!(a.doc.lines, ["cow Cat", "dog cat"]);
    }

    #[test]
    fn code_at_cursor() {
        let mut a =
            app("a `x y` b\n\n```rs\nfn f() {}\n  g();\n```\n- item\n  ```\n  in list\n  ```");
        assert_eq!(a.code_at(Pos::new(0, 4)), Some(("x y".into(), false)));
        assert_eq!(a.code_at(Pos::new(0, 0)), None);
        assert_eq!(a.code_at(Pos::new(0, 8)), None);
        let block = Some(("fn f() {}\n  g();".to_string(), true));
        assert_eq!(a.code_at(Pos::new(2, 0)), block);
        assert_eq!(a.code_at(Pos::new(4, 1)), block);
        assert_eq!(a.code_at(Pos::new(8, 3)), Some(("in list".into(), true)));
    }

    #[test]
    fn column_cells_select_and_clear() {
        let mut a = app("x\n| A | B |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n| 5 | 6 |");
        a.set_sel(Pos::new(3, 6), Pos::new(3, 6)); // cell "2"
        let cell = |a: &mut App| a.cell_at(a.cursor).unwrap();
        let c = cell(&mut a);
        a.extend_cells(&c, true);
        let c = cell(&mut a);
        a.extend_cells(&c, true);
        let c = cell(&mut a);
        a.extend_cells(&c, true); // stops at the last row
        assert_eq!(a.selected_cells(), vec!["2", "4", "6"]);
        let c = cell(&mut a);
        a.extend_cells(&c, false);
        assert_eq!(a.selected_cells(), vec!["2", "4"]);
        a.clear_cells();
        assert_eq!(
            a.doc.lines[1..].join("\n"),
            "| A   | B   |\n| --- | --- |\n| 1   |     |\n| 3   |     |\n| 5   | 6   |"
        );
        assert!(a.cells.is_none());
        a.undo(true);
        assert_eq!(a.doc.lines[3], "| 1 | 2 |");
    }

    #[test]
    fn clicking_the_document_takes_keys_from_find() {
        let mut a = app("one two\nthree");
        a.open_prompt(PromptKind::Find);
        a.on_char('t' as u16);
        assert_eq!(a.find_query, "t");
        // A click in the document (the hit test needs a window, so call the
        // part of on_mouse_down under test) keeps the prompt but types there.
        a.prompt_click(100.0);
        assert!(a.prompt.is_some() && !a.prompt_focused());
        a.set_sel(Pos::new(1, 0), Pos::new(1, 0));
        a.on_char('x' as u16);
        assert_eq!(a.doc.lines[1], "xthree");
        assert_eq!(a.find_query, "t");
        // A click on the status bar hands the keyboard back to the prompt.
        let y = (a.view.h - a.view.status_h() / 2.0) as i32;
        a.on_mouse_down(20, y);
        assert!(a.prompt_focused());
        a.on_char('h' as u16);
        assert_eq!(a.find_query, "th");
        assert_eq!(a.doc.lines[1], "xthree");
        // Escape in the document closes it.
        a.prompt_click(100.0);
        assert!(a.on_key(VK_ESCAPE.0));
        assert!(a.prompt.is_none());
        // A go-to-line prompt closes on a click instead.
        a.open_prompt(PromptKind::GoTo);
        a.prompt_click(100.0);
        assert!(a.prompt.is_none());
    }

    #[test]
    fn zoom_steps() {
        assert_eq!((clamp_zoom(1.0 + ZOOM_STEP) * 100.0).round(), 105.0);
        assert_eq!((clamp_zoom(1.07) * 100.0).round(), 105.0);
        assert_eq!((clamp_zoom(0.1) * 100.0).round(), 60.0);
        assert_eq!((clamp_zoom(9.0) * 100.0).round(), 300.0);
    }

    #[test]
    fn keys_run_keymap_commands() {
        let mut a = app("word");
        a.set_sel(Pos::new(0, 0), Pos::new(0, 4));
        a.keymap = Keymap::parse("").0;
        assert!(a.editor_key(VK_B, true, true, false));
        assert_eq!(a.doc.lines[0], "**word**");
        // A rebound key runs its new command, and the old key does nothing.
        a.keymap = Keymap::parse(
            "bold = Ctrl+Q
italic =",
        )
        .0;
        a.set_sel(Pos::new(0, 2), Pos::new(0, 6));
        assert!(!a.editor_key(VK_I, true, false, false));
        assert!(a.editor_key(VK_Q, true, false, false));
        assert_eq!(a.doc.lines[0], "word");
        // Moving by word stays fixed.
        assert!(a.editor_key(VK_RIGHT, true, false, false));
    }

    #[test]
    fn view_modes_cycle_and_source_toggles_back() {
        assert_eq!(ViewMode::Live.next(), ViewMode::Live2);
        assert_eq!(ViewMode::Source.next(), ViewMode::Live);
        for m in ViewMode::ALL {
            assert_eq!(ViewMode::from_key(m.key()), Some(m));
        }
        let mut a = app("text");
        a.set_mode(ViewMode::Live2);
        a.toggle_source();
        assert_eq!(a.mode, ViewMode::Source);
        a.toggle_source();
        assert_eq!(a.mode, ViewMode::Live2);
    }

    #[test]
    fn view_mode_is_read_only() {
        let mut a = app("- [ ] task");
        a.set_mode(ViewMode::View);
        a.set_sel(Pos::new(0, 10), Pos::new(0, 10));
        a.on_char(b'x' as u16);
        a.editor_key(VK_BACK, false, false, false);
        a.editor_key(VK_RETURN, false, false, false);
        a.run_action(Action::Bold);
        a.toggle_task(0);
        assert_eq!(a.doc.lines, ["- [ ] task"]);
        // Moving and selecting still work.
        a.editor_key(VK_HOME, false, false, false);
        assert_eq!(a.cursor, Pos::new(0, 6));
        a.set_mode(ViewMode::Live);
        a.on_char(b'x' as u16);
        assert_eq!(a.doc.lines, ["- [ ] xtask"]);
    }

    #[test]
    fn format_wrap_leaves_edge_spaces_outside() {
        let mut a = app("say two words now");
        a.set_sel(Pos::new(0, 4), Pos::new(0, 14));
        a.run_action(Action::Code);
        assert_eq!(a.doc.lines, ["say `two words` now"]);
        assert_eq!(a.sel(), (Pos::new(0, 5), Pos::new(0, 14)));
        // Toggling again with the same padded selection unwraps.
        a.set_sel(Pos::new(0, 3), Pos::new(0, 16));
        a.run_action(Action::Code);
        assert_eq!(a.doc.lines, ["say two words now"]);

        let mut a = app("a  b");
        a.set_sel(Pos::new(0, 1), Pos::new(0, 3));
        a.run_action(Action::Bold);
        // A whitespace-only selection is wrapped as is.
        assert_eq!(a.doc.lines, ["a**  **b"]);
    }
}
