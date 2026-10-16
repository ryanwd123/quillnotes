//! Editor state and input handling.

use std::path::{Path, PathBuf};
use std::time::Instant;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::Input::Ime::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::HSTRING;

use crate::doc::{self, Document, EditKind, Pos};
use crate::markdown::{self, Kind, LineInfo};
use crate::options::RenderOptions;
use crate::palette::{Command, Mode, Palette, Target};
use crate::platform;
use crate::render::{PALETTE_ROWS, PaletteItem, PaletteView, STATUS_H, Status, View};
use crate::settings::Settings;

pub const APP_NAME: &str = "Penmark";

#[derive(Clone, Copy, PartialEq)]
enum PromptKind {
    Find,
    GoTo,
}

struct Prompt {
    kind: PromptKind,
    text: String,
    cur: usize,
    origin: (Pos, Pos, f32),
}

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
    layout_key: Option<(u64, usize, usize, bool, u32, u32, u32)>,
    message: Option<(String, Instant)>,
    prompt: Option<Prompt>,
    find_query: String,
    drag: Drag,
    last_click: (u32, i32, i32, u32),
    high_surrogate: Option<u16>,
    words: (u64, usize),
    last_title: String,
    /// Display name for a document without a path.
    untitled_name: &'static str,
    palette: Option<Palette>,
    settings: Settings,
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
            words: (u64::MAX, 0),
            last_title: String::new(),
            untitled_name: "Untitled",
            palette: None,
            settings,
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
        let key = (
            self.doc.version,
            a.line,
            b.line,
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

    pub fn update_title(&mut self) {
        let name = self
            .doc
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.untitled_name.into());
        let title = format!(
            "{}{} — {APP_NAME}",
            if self.doc.is_dirty() { "• " } else { "" },
            name
        );
        if title != self.last_title {
            unsafe {
                let _ = SetWindowTextW(self.hwnd, &HSTRING::from(&title));
            }
            self.last_title = title;
        }
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
        self.type_text(&s);
    }

    /// Returns true when the key was handled.
    pub fn on_key(&mut self, vk: u16) -> bool {
        let (ctrl, shift, alt) = (ctrl_down(), shift_down(), alt_down());
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
        let key = VIRTUAL_KEY(vk);
        if ctrl {
            match key {
                VK_P if shift => self.open_palette(Palette::commands()),
                VK_N if shift => self.new_window(),
                VK_N => self.new_doc(),
                VK_O => self.open(),
                VK_S if shift => {
                    self.save_as();
                }
                VK_S => {
                    self.save();
                }
                VK_W => unsafe {
                    let _ = PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
                },
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
                VK_V => self.paste(),
                VK_B => self.toggle_wrap("**"),
                VK_I => self.toggle_wrap("*"),
                VK_E => self.toggle_wrap("`"),
                VK_H if shift => self.toggle_wrap("=="),
                VK_K if shift => self.delete_lines(),
                VK_K => self.make_link(),
                VK_D if !shift => self.duplicate_lines(),
                VK_T if shift => self.toggle_theme(),
                VK_F => self.open_prompt(PromptKind::Find),
                VK_G => self.open_prompt(PromptKind::GoTo),
                VK_OEM_2 => {
                    self.source = !self.source;
                    self.say(if self.source {
                        "Source mode"
                    } else {
                        "Live preview"
                    });
                    self.changed(false);
                }
                VK_OEM_PLUS | VK_ADD => self.set_zoom(self.view.zoom + 0.1),
                VK_OEM_MINUS | VK_SUBTRACT => self.set_zoom(self.view.zoom - 0.1),
                VK_0 | VK_NUMPAD0 if !shift => self.set_zoom(1.0),
                VK_1 | VK_2 | VK_3 | VK_4 | VK_5 | VK_6 if !shift => {
                    self.set_heading((vk - VK_0.0) as u8)
                }
                VK_8 if shift => self.set_list(ListStyle::Bullet),
                VK_7 if shift => self.set_list(ListStyle::Ordered),
                VK_9 if shift => self.set_list(ListStyle::Task),
                VK_OEM_PERIOD if shift => self.toggle_quote(),
                VK_RETURN => self.toggle_task(self.cursor.line),
                VK_LEFT | VK_RIGHT | VK_UP | VK_DOWN | VK_HOME | VK_END | VK_BACK | VK_DELETE => {
                    self.nav_key(key, true, shift)
                }
                _ => return false,
            }
            return true;
        }
        if alt {
            match key {
                VK_UP => self.move_lines(false),
                VK_DOWN => self.move_lines(true),
                _ => return false,
            }
            return true;
        }
        match key {
            VK_F1 => {
                if let Ok(exe) = std::env::current_exe() {
                    let _ = std::process::Command::new(exe).arg("--help").spawn();
                }
            }
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

    fn duplicate_lines(&mut self) {
        let (l0, l1) = self.line_range();
        let block = self.doc.lines[l0..=l1].join("\n");
        self.begin(EditKind::Other);
        let end = Pos::new(l1, self.doc.line_len(l1));
        self.doc.replace(end, end, &format!("\n{block}"));
        let k = l1 - l0 + 1;
        let d = |p: Pos| Pos::new(p.line + k, p.col);
        self.set_sel(d(self.anchor), d(self.cursor));
        self.commit();
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

    fn copy(&mut self, cut: bool) {
        let (a, b) = self.sel();
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
            PromptKind::GoTo => String::new(),
        };
        self.prompt = Some(Prompt {
            kind,
            cur: text.len(),
            text,
            origin: (self.anchor, self.cursor, self.scroll),
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
    // Command palette

    fn open_palette(&mut self, p: Palette) {
        self.prompt = None;
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
            _ => {}
        }
        self.palette = None;
        match target {
            Target::Command(Command::ChangeFont) => {
                let current = self.view.body_font();
                self.open_palette(Palette::fonts(self.view.font_families(), &current));
            }
            Target::Command(Command::RenderingOptions) => {
                self.open_palette(Palette::options(&self.view.options()));
            }
            Target::Option(_) | Target::ResetOptions => {}
            Target::Font(name) => {
                self.preview_font(&name);
                self.settings.set("font", &name);
                match self.settings.save() {
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
        o.store(&mut self.settings);
        if let Err(e) = self.settings.save() {
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
        self.view.zoom = (z * 10.0).round().clamp(6.0, 30.0) / 10.0;
        self.say(format!("Zoom {}%", (self.view.zoom * 100.0).round()));
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
            let name = self
                .doc
                .path
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.untitled_name.into());
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
        let caret =
            self.caret_on && self.focused && self.prompt.is_none() && self.palette.is_none();
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
        if self.palette.is_some() {
            self.palette_click(dx, dy);
            return;
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
        if ctrl_down()
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
            Drag::None => {}
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
                    Drag::Char => self.cursor = p,
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

    pub fn on_mouse_up(&mut self) {
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
        if ctrl_down() {
            self.set_zoom(self.view.zoom + if delta > 0 { 0.1 } else { -0.1 });
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

    /// Whether the I-beam cursor applies at this client point (pixels).
    pub fn is_text_area(&self, x: i32, y: i32) -> bool {
        let (dx, dy) = self.to_dip(x, y);
        let over_palette = self.palette.is_some()
            && self
                .view
                .palette_geom
                .is_some_and(|g| dx >= g.left && dx <= g.right && dy >= g.top && dy <= g.bottom);
        !over_palette && dy < self.view_h() && dx < self.view.w - 12.0
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

    pub fn open_path(&mut self, path: &Path) {
        match std::fs::read(path) {
            Ok(bytes) => {
                let (doc, lossy) = Document::from_bytes(&bytes, Some(path.to_path_buf()));
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
                self.doc.path = Some(path.to_path_buf());
                self.doc.mark_saved();
                self.say("Saved");
                self.update_title();
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
        let name = self
            .doc
            .path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.untitled_name.into());
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
            let _ = std::process::Command::new(exe).spawn();
        }
    }

    fn open(&mut self) {
        if !self.confirm_discard() {
            return;
        }
        if let Some(p) = platform::open_dialog(self.hwnd, self.doc_dir().as_deref()) {
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
