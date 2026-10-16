//! Editor state and input handling.

mod folder;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::{InvalidateRect, MONITOR_DEFAULTTONULL, MonitorFromRect};
use windows::Win32::UI::Input::Ime::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, PCWSTR};

use crate::doc::{self, Document, EditKind, Pos};
use crate::markdown::{self, Align, Kind, LineInfo};
use crate::options::RenderOptions;
use crate::palette::{Command, Mode, Palette, Target};
use crate::platform;
use crate::render::{
    PALETTE_ROWS, PaletteItem, PaletteView, PopupView, SIDEBAR_W, STATUS_H, SidebarButton, Status,
    TableUi, View,
};
use crate::settings::Settings;
use crate::slash::{self, SlashCmd};
use crate::table::{self, Grid, TableCmd};
use crate::workspace::{Entry, FolderSettings};

pub const APP_NAME: &str = "Penmark";

const ZOOM_STEP: f32 = 0.05;

/// Rounds to the nearest 5% step within 60%–300%.
fn clamp_zoom(z: f32) -> f32 {
    (z / ZOOM_STEP).round().clamp(12.0, 60.0) * ZOOM_STEP
}

#[derive(Clone, Copy, PartialEq)]
enum PromptKind {
    Find,
    GoTo,
    /// Rename (or move, with a relative path) the explorer entry `Prompt::path`.
    Rename,
    /// Create a file or folder inside the folder `Prompt::path`.
    NewFile,
    NewFolder,
}

struct Prompt {
    kind: PromptKind,
    text: String,
    cur: usize,
    origin: (Pos, Pos, f32),
    /// The file or folder a rename / new-item prompt acts on.
    path: Option<PathBuf>,
}

/// Everything the layout depends on: doc version, selected lines, table being edited,
/// source mode, width, zoom, DPI.
type LayoutKey = (u64, usize, usize, u32, bool, u32, u32, u32);

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

/// The open `/` menu, for the `/` at `line`, `col`.
struct SlashMenu {
    line: usize,
    col: usize,
    query: String,
    /// Matching [`slash::ITEMS`] indices with highlighted label offsets.
    matches: Vec<(usize, Vec<usize>)>,
    selected: usize,
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
    source: bool,
    caret_on: bool,
    focused: bool,
    layout_key: Option<LayoutKey>,
    message: Option<(String, Instant)>,
    prompt: Option<Prompt>,
    find_query: String,
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
    /// Files to go back / forward to (most recent last).
    back: Vec<NavEntry>,
    forward: Vec<NavEntry>,
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

/// Case-insensitive (ASCII) search for `needle` in `hay` starting at `from`.
fn find_ci(hay: &str, needle: &str, from: usize) -> Option<usize> {
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (from..=h.len() - n.len())
        .find(|&i| hay.is_char_boundary(i) && h[i..i + n.len()].eq_ignore_ascii_case(n))
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
        view.set_options(RenderOptions::load(&settings));
        let explorer = settings.get("explorer") != Some("off");
        if let Some(pct) = settings.get("zoom").and_then(|z| z.parse::<f32>().ok()) {
            view.zoom = clamp_zoom(pct / 100.0);
        }
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
            source: false,
            caret_on: true,
            focused: true,
            layout_key: None,
            message: None,
            prompt: None,
            find_query: String::new(),
            drag: Drag::None,
            last_click: (0, 0, 0, 0),
            high_surrogate: None,
            skip_char: false,
            words: (u64::MAX, 0),
            last_title: String::new(),
            untitled_name: "Untitled",
            palette: None,
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
            back: Vec::new(),
            forward: Vec::new(),
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
        let (a, b) = self.sel();
        let grid = self.grid_block();
        let key = (
            self.doc.version,
            a.line,
            b.line,
            grid,
            self.source,
            self.view.w.to_bits(),
            self.view.zoom.to_bits(),
            self.view.dpi.to_bits(),
        );
        if self.layout_key != Some(key) {
            let dir = self.doc_dir();
            self.view.layout(
                self.hwnd,
                &self.doc.lines,
                &self.infos,
                (a.line, b.line),
                grid,
                self.source,
                dir.as_deref(),
            );
            self.layout_key = Some(key);
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
        self.view.h - STATUS_H
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
                        y: ((y - self.scroll) * k) as i32,
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
        self.invalidate();
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
            return;
        }
        if let Some(p) = &mut self.prompt {
            p.text.insert_str(p.cur, &s);
            p.cur += s.len();
            self.prompt_changed();
            return;
        }
        if self.tree_focus {
            return;
        }
        // A pipe typed in a table cell must not split the cell.
        let s = if s == "|" && self.grid_block() != 0 {
            r"\|".to_string()
        } else {
            s
        };
        self.type_text(&s);
        if s == "/" {
            self.open_slash();
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
        if self.prompt.is_some() && self.prompt_key(vk, ctrl, shift) {
            return true;
        }
        if self.tree_focus && !alt && self.tree_key(key, ctrl) {
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
        let vk = key.0;
        if ctrl {
            match key {
                VK_P if shift => self.open_palette(Palette::commands()),
                VK_P => self.quick_open(),
                VK_R => self.recent_folders(),
                VK_E if shift => self.toggle_explorer(),
                VK_0 if !shift => self.focus_explorer(),
                VK_N if shift => self.new_window(),
                VK_N => self.new_doc(),
                VK_O => self.open(),
                VK_S if shift => {
                    self.save_as();
                }
                VK_S => {
                    self.save();
                }
                VK_W if shift => unsafe {
                    let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                },
                VK_W => self.new_doc(),
                VK_Z if shift => self.undo(false),
                VK_Z => self.undo(true),
                VK_Y => self.undo(false),
                VK_A => {
                    self.set_sel(Pos::default(), self.doc.end());
                    self.changed(false);
                }
                VK_C => self.copy(false),
                VK_X if shift => self.toggle_wrap("~~"),
                VK_X => self.copy(true),
                VK_V if shift => self.toggle_source(),
                VK_V => self.paste(),
                VK_B => self.toggle_wrap("**"),
                VK_I => self.toggle_wrap("*"),
                VK_E => self.toggle_wrap("`"),
                VK_H if shift => self.toggle_wrap("=="),
                VK_K if shift => self.delete_lines(),
                VK_K => self.make_link(),
                VK_D if !shift => self.select_next_match(),
                VK_L if !shift => self.select_line(),
                VK_OEM_6 => self.indent(true),
                VK_OEM_4 => self.indent(false),
                VK_T if shift => self.toggle_theme(),
                VK_F => self.open_prompt(PromptKind::Find),
                VK_G => self.open_prompt(PromptKind::GoTo),
                VK_OEM_2 => self.toggle_comment(),
                VK_OEM_PLUS | VK_ADD => self.set_zoom(self.view.zoom + ZOOM_STEP),
                VK_OEM_MINUS | VK_SUBTRACT => self.set_zoom(self.view.zoom - ZOOM_STEP),
                VK_NUMPAD0 => self.set_zoom(1.0),
                VK_1 | VK_2 | VK_3 | VK_4 | VK_5 | VK_6 if !shift => {
                    self.set_heading((vk - VK_0.0) as u8)
                }
                VK_8 if shift => self.set_list(ListStyle::Bullet),
                VK_7 if shift => self.set_list(ListStyle::Ordered),
                VK_9 if shift => self.set_list(ListStyle::Task),
                VK_OEM_PERIOD if shift => self.toggle_quote(),
                VK_RETURN => self.open_line(!shift),
                VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN | VK_HOME | VK_END | VK_BACK | VK_DELETE => {
                    self.nav_key(key, true, shift)
                }
                _ => return false,
            }
            return true;
        }
        if alt {
            match key {
                VK_UP if shift => self.duplicate_lines(false),
                VK_DOWN if shift => self.duplicate_lines(true),
                VK_UP => self.move_lines(false),
                VK_DOWN => self.move_lines(true),
                VK_C => self.toggle_task(self.cursor.line),
                VK_LEFT => self.navigate(true),
                VK_RIGHT => self.navigate(false),
                _ => return false,
            }
            return true;
        }
        match key {
            VK_F1 => self.open_palette(Palette::commands()),
            VK_F3 => self.find_next(!shift),
            VK_ESCAPE => {
                if self.anchor != self.cursor {
                    self.anchor = self.cursor;
                    self.changed(false);
                }
            }
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

    fn nav_key(&mut self, key: VIRTUAL_KEY, ctrl: bool, shift: bool) {
        let c = self.cursor;
        let line = self.doc.lines[c.line].clone();
        let (a, b) = self.sel();
        match key {
            VK_LEFT => {
                let p = if !shift && a != b {
                    a
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
                self.backspace(ctrl);
                return;
            }
            VK_DELETE => {
                self.delete_forward(ctrl);
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
            let start = if word {
                doc::prev_word(line, a.col)
            } else if a.col == info.content
                && matches!(info.kind, Kind::List { .. } | Kind::Heading(_))
                && !self.source
            {
                // Remove the list marker / heading hashes in one go.
                info.qend
            } else if a.col == info.qend && info.quote > 0 && !self.source {
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
        if self.source {
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

    fn toggle_source(&mut self) {
        self.source = !self.source;
        self.say(if self.source {
            "Source mode"
        } else {
            "Live preview"
        });
        self.changed(false);
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
            PromptKind::Find if a != b && a.line == b.line => self.doc.text_range(a, b),
            PromptKind::Find => self.find_query.clone(),
            _ => String::new(),
        };
        self.prompt = Some(Prompt {
            kind,
            cur: text.len(),
            text,
            origin: (self.anchor, self.cursor, self.scroll),
            path: None,
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
        });
        self.invalidate();
    }

    fn prompt_changed(&mut self) {
        let Some(p) = &self.prompt else { return };
        if p.kind == PromptKind::Find {
            self.find_query = p.text.clone();
            let (a, c, _) = p.origin;
            let start = a.min(c);
            self.anchor = start;
            self.cursor = start;
            self.find(true);
        }
        self.invalidate();
    }

    fn prompt_key(&mut self, vk: u16, ctrl: bool, shift: bool) -> bool {
        let key = VIRTUAL_KEY(vk);
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
            VK_RETURN => match p.kind {
                PromptKind::Find => self.find_next(!shift),
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
                let (mut total, mut index) = (0, 0);
                for (l, line) in self.doc.lines.iter().enumerate() {
                    let mut from = 0;
                    while let Some(i) = find_ci(line, &q, from) {
                        total += 1;
                        if Pos::new(l, i) == p {
                            index = total;
                        }
                        from = i + q.len().max(1);
                    }
                }
                self.message = Some((format!("{index} of {total}"), Instant::now()));
            }
            None => self.message = Some(("No matches".into(), Instant::now())),
        }
        self.changed(false);
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
        if self.source {
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
        if self.copy_code(p) {
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
        if self.source || c.col == 0 || self.anchor != c {
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
            });
            self.update_slash();
            self.invalidate();
        }
    }

    /// Refilters the menu from the text after the `/`, closing it when the caret
    /// leaves that text or nothing matches.
    fn update_slash(&mut self) {
        let Some(m) = &mut self.slash else { return };
        let c = self.cursor;
        let line = &self.doc.lines.get(m.line).map_or("", |l| l.as_str());
        let valid = self.anchor == c
            && c.line == m.line
            && c.col > m.col
            && c.col <= line.len()
            && line.as_bytes().get(m.col) == Some(&b'/');
        let query = if valid { &line[m.col + 1..c.col] } else { "" };
        let matches = if valid && !query.contains(char::is_whitespace) {
            slash::filter(query)
        } else {
            Vec::new()
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

    /// Replaces the typed `/command` with the selected item's result.
    fn slash_accept(&mut self) {
        let Some(m) = self.slash.take() else { return };
        let Some(&(item, _)) = m.matches.get(m.selected) else {
            return;
        };
        let l = m.line;
        self.begin(EditKind::Other);
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
            self.preview_font(&font);
        }
        self.invalidate();
    }

    fn preview_font(&mut self, name: &str) {
        if self.view.body_font() != name {
            self.view.set_body_font(name);
            self.relayout();
        }
    }

    /// Called after the palette query or selection changed.
    fn palette_changed(&mut self) {
        let Some(p) = &self.palette else { return };
        if p.mode == Mode::Fonts {
            // Live preview: the document shows the highlighted font.
            let font = match p.selected().map(|item| &item.target) {
                Some(Target::Font(f)) => Some(f.clone()),
                _ => p.restore_font.clone(),
            };
            if let Some(f) = font {
                self.preview_font(&f);
            }
        }
        self.invalidate();
    }

    fn palette_key(&mut self, key: VIRTUAL_KEY, ctrl: bool, shift: bool) {
        let Some(p) = &mut self.palette else { return };
        let page = PALETTE_ROWS as isize;
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
        match key {
            VK_ESCAPE => return self.close_palette(true),
            VK_P if ctrl && shift => return self.close_palette(true),
            VK_RETURN => return self.palette_accept(),
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
            Target::FolderOpt(opt) => return self.change_folder_settings(|f| opt.activate(f)),
            _ => {}
        }
        self.palette = None;
        match target {
            Target::Command(Command::ChangeFont) => {
                let current = self.view.body_font();
                self.open_palette(Palette::fonts(self.view.font_families(), &current));
            }
            Target::Command(Command::Table(cmd)) => self.table_command(cmd),
            Target::Command(Command::RenderingOptions) => {
                self.open_palette(Palette::options(&self.view.options()));
            }
            Target::Option(_) | Target::ResetOptions | Target::FolderOpt(_) => {}
            Target::Command(Command::QuickOpen) => self.quick_open(),
            Target::Command(Command::OpenFolder) | Target::OpenFolderDialog => {
                self.open_folder_dialog()
            }
            Target::Command(Command::OpenRecent) => self.recent_folders(),
            Target::Command(Command::CloseFolder) => self.close_folder(),
            Target::Command(Command::NewFile) => self.new_item_prompt(false),
            Target::Command(Command::NewFolder) => self.new_item_prompt(true),
            Target::Command(Command::ToggleExplorer) => self.toggle_explorer(),
            Target::Command(Command::FolderSettings) => self.folder_settings_palette(),
            Target::Command(Command::ToggleSource) => self.toggle_source(),
            Target::Command(Command::ToggleTheme) => self.toggle_theme(),
            Target::Command(Command::Help) => {
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::process::Command::new(exe).arg("--help").spawn();
                }
            }
            Target::OpenFile(path) => self.open_file_in_folder(&path),
            Target::SwitchFolder(path) => self.switch_folder(&path),
            Target::Font(name) => {
                self.preview_font(&name);
                match Settings::update(|s| s.set("font", &name)) {
                    Ok(()) => self.say(format!("Font: {name}")),
                    Err(e) => self.say(format!("Font: {name} (setting not saved: {e})")),
                }
            }
        }
        self.invalidate();
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
        self.caret_on = !self.caret_on;
        self.autosave_tick();
        if self
            .message
            .as_ref()
            .is_some_and(|m| m.1.elapsed().as_secs() >= 4)
        {
            self.message = None;
        }
        self.invalidate();
    }

    pub fn on_focus(&mut self, focused: bool) {
        self.focused = focused;
        self.caret_on = focused;
        if focused && self.folder.is_some() {
            // Pick up files added or removed outside Penmark.
            self.rescan();
        }
        if focused {
            self.restart_blink();
            self.update_ime();
        }
        self.invalidate();
    }

    pub fn on_size(&mut self, pw: u32, ph: u32) {
        self.view.resize(pw, ph, self.view.dpi);
        self.relayout();
    }

    pub fn paint(&mut self) {
        self.ensure_layout();
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
            let mode = if self.source { "Source" } else { "Live" };
            let eol = if self.doc.crlf { "CRLF" } else { "LF" };
            status_right = format!(
                "Ln {}, Col {col}     {words}     {eol}     {mode}",
                self.cursor.line + 1
            );
            prompt_text = self.prompt.as_ref().map(|p| {
                let label = match p.kind {
                    PromptKind::Find => "Find: ",
                    PromptKind::GoTo => "Go to line: ",
                    PromptKind::Rename => "Rename to (a/b.md moves it): ",
                    PromptKind::NewFile => "New file: ",
                    PromptKind::NewFolder => "New folder: ",
                };
                let extra = if p.kind == PromptKind::Find {
                    self.message
                        .as_ref()
                        .map(|m| format!("      {}", m.0))
                        .unwrap_or_default()
                } else {
                    String::new()
                };
                (format!("{label}{}{extra}", p.text), label.len() + p.cur)
            });
        }
        let status = Status {
            left: &status_left,
            right: &status_right,
            prompt: prompt_text.as_ref().map(|(t, c)| (t.as_str(), *c)),
        };
        let caret = self.caret_on
            && self.focused
            && self.prompt.is_none()
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
            at: Pos::new(m.line, m.col),
            selected: m.selected,
            items: m
                .matches
                .iter()
                .map(|(i, hits)| PaletteItem {
                    label: slash::ITEMS[*i].0,
                    matches: hits,
                    detail: match slash::ITEMS[*i].3 {
                        SlashCmd::Date(_) => &dates[*i],
                        _ => slash::ITEMS[*i].1,
                    },
                    font: None,
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
        if self.over_explorer(dx, dy) {
            return self.tree_mouse_down(dx, dy);
        }
        if self.tree_focus {
            self.tree_focus = false;
            self.invalidate();
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
        if dx >= self.view.w - 12.0 && content > self.view_h() {
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
            self.open_link(&link);
            return;
        }
        match count {
            1 => {
                self.move_to(p, shift_down());
                self.drag = Drag::Char;
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
                self.hover(dx, dy);
            }
            Drag::Scrollbar(_) => self.drag_scrollbar(dy),
            drag => {
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
        let doc_y = y + self.scroll;
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
        if self.tree_button.take().is_some() {
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
            p.scroll(-(delta as isize) * 3 / 120);
            self.palette_changed();
            return;
        }
        if self.mouse.is_some_and(|(x, y)| self.over_explorer(x, y)) {
            return self.tree_wheel(delta);
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
        if !self.is_text_area(x, y) {
            return IDC_ARROW;
        }
        let (dx, dy) = self.to_dip(x, y);
        self.ensure_layout();
        let infos = std::mem::take(&mut self.infos);
        let hit = self.view.hit(&self.doc.lines, &infos, dx, dy + self.scroll);
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
        let over_table_ui = self.view.table_ui_at(dx, dy + self.scroll).is_some();
        !over_palette
            && !over_table_ui
            && !self.over_explorer(dx, dy)
            && dy < self.view_h()
            && dx < self.view.w - 12.0
    }

    /// Whether a client point (DIPs) is over the explorer panel.
    fn over_explorer(&self, x: f32, y: f32) -> bool {
        self.explorer_visible() && x < SIDEBAR_W && y < self.view_h()
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
            let stack = if back { &mut self.back } else { &mut self.forward };
            match stack.pop() {
                Some(e) if e.path.is_file() => break e,
                Some(_) => {}
                None => return self.say(if back { "No previous file" } else { "No next file" }),
            }
        };
        if !self.confirm_discard() {
            let stack = if back { &mut self.back } else { &mut self.forward };
            stack.push(entry);
            return;
        }
        let current = self.nav_entry();
        self.read_path(&entry.path);
        if self.doc.path.as_deref() != Some(entry.path.as_path()) {
            return; // Could not be read; open_path reported it.
        }
        if let Some(c) = current {
            let other = if back { &mut self.forward } else { &mut self.back };
            other.push(c);
        }
        let p = self.doc.clamp(entry.cursor);
        self.set_sel(p, p);
        self.scroll = entry.scroll;
        self.clamp_scroll();
        self.reveal_in_tree(&entry.path);
        self.changed(true);
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
        match std::fs::read(path) {
            Ok(bytes) => {
                let (doc, lossy) = Document::from_bytes(&bytes, Some(path.to_path_buf()));
                self.doc = doc;
                self.infos_ver = None;
                self.set_sel(Pos::default(), Pos::default());
                self.scroll = 0.0;
                self.prompt = None;
                self.remember_file();
                self.relayout();
                self.say(if lossy {
                    "Opened (file was not valid UTF-8; invalid bytes replaced)"
                } else {
                    "Opened"
                });
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
        match std::fs::write(path, self.doc.to_bytes()) {
            Ok(()) => {
                let new_path = self.doc.path.as_deref() != Some(path);
                self.doc.path = Some(path.to_path_buf());
                self.doc.mark_saved();
                self.say("Saved");
                self.update_title();
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

    fn save_as(&mut self) -> bool {
        let suggested = self
            .doc
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
            .unwrap_or_else(|| "Untitled.md".into());
        match platform::save_dialog(self.hwnd, self.doc.path.as_deref(), &suggested) {
            Some(p) => self.write_to(&p),
            None => false,
        }
    }

    /// Asks to save unsaved changes. Returns false if the user cancelled.
    pub fn confirm_discard(&mut self) -> bool {
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
        if let Some(name) = target.strip_prefix("[[").and_then(|t| t.strip_suffix("]]")) {
            let name = name.split('#').next().unwrap_or(name).trim();
            let file = if name.ends_with(".md") {
                name.to_string()
            } else {
                format!("{name}.md")
            };
            let path = dir.join(file);
            if !self.confirm_discard() {
                return;
            }
            if path.exists() {
                self.open_path(&path);
            } else {
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
            let want = slug(anchor);
            let infos = self.infos().to_vec();
            if let Some(l) = (0..self.doc.lines.len()).find(|&l| {
                matches!(infos[l].kind, Kind::Heading(_))
                    && slug(&self.doc.lines[l][infos[l].content..]) == want
            }) {
                self.set_sel(Pos::new(l, 0), Pos::new(l, 0));
                self.changed(false);
            }
            return;
        }
        let has_scheme = target.split_once(':').is_some_and(|(s, _)| {
            s.len() > 1 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '+')
        });
        if has_scheme {
            platform::shell_open(self.hwnd, target);
            return;
        }
        let path = dir.join(target.replace("%20", " ").split('#').next().unwrap_or(""));
        let is_md = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"));
        if is_md && path.exists() {
            if self.confirm_discard() {
                self.open_path(&path);
            }
        } else {
            platform::shell_open(self.hwnd, &path.to_string_lossy());
        }
    }

    pub fn on_drop(&mut self, path: PathBuf) {
        if self.confirm_discard() {
            self.open_path(&path);
        }
    }
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
        a
    }

    #[test]
    fn code_at_cursor() {
        let mut a = app("a `x y` b\n\n```rs\nfn f() {}\n  g();\n```\n- item\n  ```\n  in list\n  ```");
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
    fn zoom_steps() {
        assert_eq!((clamp_zoom(1.0 + ZOOM_STEP) * 100.0).round(), 105.0);
        assert_eq!((clamp_zoom(1.07) * 100.0).round(), 105.0);
        assert_eq!((clamp_zoom(0.1) * 100.0).round(), 60.0);
        assert_eq!((clamp_zoom(9.0) * 100.0).round(), 300.0);
    }
}
