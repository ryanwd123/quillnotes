//! Folders: opening them, quick open, the explorer panel and its file operations,
//! folder settings, and autosave.

use std::path::{Path, PathBuf};
use std::time::Duration;

use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::{APP_NAME, App, NavEntry, PromptKind};
use crate::doc::{self, Document, EditKind, Pos};
use crate::palette::{Mode, Palette, SearchFile, Target};
use crate::platform::{self, MenuItem};
use crate::render::{SidebarButton, SidebarRow, SidebarView};
use crate::settings::Settings;
use crate::workspace::{self, Entry, FolderSettings};

/// A link to rewrite on a line: (line, old target, new target).
type LinkEdit = (usize, String, String);

/// An explorer entry being dragged; it becomes a move once the mouse travels.
pub(super) struct TreeDrag {
    path: PathBuf,
    y0: f32,
    active: bool,
    /// Folder the entry would be moved into.
    target: Option<PathBuf>,
}

/// Where the editor was when Search in Files opened, to go back to when it is
/// cancelled. While another file is previewed, `doc` holds the open document.
pub(super) struct SearchOrigin {
    doc: Option<Document>,
    anchor: Pos,
    cursor: Pos,
    scroll: f32,
    /// Whether the editor shows the highlighted result rather than this place.
    previewing: bool,
}

fn name_of(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.to_string_lossy().into_owned(),
        |n| n.to_string_lossy().into_owned(),
    )
}

impl App {
    // -----------------------------------------------------------------------
    // Opening folders

    pub(super) fn open_folder_dialog(&mut self) {
        let start = self.folder.clone().or_else(|| self.doc_dir());
        if let Some(dir) = platform::folder_dialog(self.hwnd, start.as_deref()) {
            self.switch_folder(&dir);
        }
    }

    /// Opens `dir` as this window's folder, offering to save the current file first.
    pub(super) fn switch_folder(&mut self, dir: &Path) {
        if !dir.is_dir() {
            self.say(format!("{} is no longer available", dir.display()));
            return self.save_setting(|s| workspace::remove_recent(s, dir));
        }
        if self.confirm_discard() {
            self.save_session();
            self.open_folder(dir);
        }
    }

    /// Saves where the folder was left (the open file's caret and scroll, and the
    /// back history) for the next time it opens.
    pub fn save_session(&mut self) {
        let Some(dir) = self.folder.clone() else {
            return;
        };
        let spot = |e: &NavEntry| {
            workspace::contains(&dir, &e.path).then(|| workspace::Spot {
                file: workspace::relative(&dir, &e.path),
                line: e.cursor.line,
                col: e.cursor.col,
                scroll: e.scroll,
            })
        };
        let last_spot = self.nav_entry().as_ref().and_then(spot);
        let history: Vec<workspace::Spot> = self.back.iter().filter_map(spot).collect();
        self.folder_settings.last_spot = last_spot.clone();
        self.folder_settings.history = history.clone();
        self.save_setting(|s| {
            let mut f = FolderSettings::load(s, &dir);
            f.last_spot = last_spot;
            f.history = history;
            f.store(s, &dir);
        });
    }

    /// Opens `dir` and the file that was last open in it.
    pub fn open_folder(&mut self, dir: &Path) {
        self.save_setting(|s| workspace::add_recent(s, dir));
        self.folder = Some(dir.to_path_buf());
        self.folder_settings = FolderSettings::load(&Settings::load(), dir);
        self.expanded.clear();
        self.tree_sel = None;
        self.tree_scroll = 0.0;
        self.tree_cut = None;
        self.rescan();
        self.set_explorer(self.explorer);
        let last = self
            .folder_settings
            .last_file
            .as_ref()
            .map(|rel| dir.join(rel))
            .filter(|p| p.is_file());
        match last {
            Some(p) => {
                self.open_path(&p);
                self.reveal_in_tree(&p);
                // Back where it was left.
                if let Some(s) = self.folder_settings.last_spot.clone()
                    && self.folder_settings.last_file.as_ref() == Some(&s.file)
                {
                    let c = self.doc.clamp(Pos::new(s.line, s.col));
                    self.set_sel(c, c);
                    self.ensure_layout();
                    self.scroll = s.scroll;
                    self.clamp_scroll();
                }
            }
            None => self.blank_doc(),
        }
        // History is per folder: the files opened before, as the folder was left.
        self.forward.clear();
        self.back = self
            .folder_settings
            .history
            .iter()
            .map(|s| NavEntry {
                path: dir.join(&s.file),
                cursor: Pos::new(s.line, s.col),
                scroll: s.scroll,
            })
            .filter(|e| e.path.is_file())
            .collect();
        self.say(format!("Opened folder {}", name_of(dir)));
        self.update_title();
    }

    /// At startup: reopen the most recent folder, if it still exists.
    pub fn restore_last_folder(&mut self) {
        if let Some(dir) = workspace::recent(&Settings::load()).into_iter().next()
            && dir.is_dir()
        {
            self.open_folder(&dir);
        }
    }

    pub(super) fn close_folder(&mut self) {
        if self.folder.is_none() {
            return self.say("No folder is open");
        }
        if !self.confirm_discard() {
            return;
        }
        self.save_session();
        self.folder = None;
        self.entries.clear();
        self.tree_focus = false;
        self.set_explorer(self.explorer);
        self.blank_doc();
        self.update_title();
    }

    /// Replaces the document with an empty one (no prompt).
    fn blank_doc(&mut self) {
        self.doc = Document::new();
        self.infos_ver = None;
        self.set_sel(Pos::default(), Pos::default());
        self.scroll = 0.0;
        self.relayout();
        self.changed(false);
    }

    /// Re-reads the folder's file tree (after file operations or outside changes).
    pub(super) fn rescan(&mut self) {
        if let Some(dir) = &self.folder {
            self.entries = workspace::scan(dir);
        }
        self.scan_gen += 1;
        self.invalidate();
    }

    /// Records the open file as the folder's last file, for the next time it opens.
    pub(super) fn remember_file(&mut self) {
        let (Some(dir), Some(path)) = (self.folder.clone(), self.doc.path.clone()) else {
            return;
        };
        if !workspace::contains(&dir, &path) {
            return;
        }
        let rel = workspace::relative(&dir, &path);
        self.folder_settings.last_file = Some(rel.clone());
        self.save_setting(|s| {
            let mut f = FolderSettings::load(s, &dir);
            f.last_file = Some(rel);
            f.store(s, &dir);
        });
    }

    // -----------------------------------------------------------------------
    // Pickers

    /// Ctrl+P: pick a file in the folder.
    pub(super) fn quick_open(&mut self) {
        let Some(dir) = self.folder.clone() else {
            return self.say("Open a folder first (Ctrl+R)");
        };
        self.rescan();
        let p = Palette::files(&dir, &self.entries, self.doc.path.as_deref());
        self.open_palette(p);
    }

    /// Ctrl+Shift+F: search the folder's Markdown files, starting with the selected
    /// text. The open file is searched as edited, saved or not.
    pub(super) fn search_files(&mut self) {
        let Some(dir) = self.folder.clone() else {
            return self.say("Open a folder first (Ctrl+R)");
        };
        self.rescan();
        let files = self
            .entries
            .iter()
            .filter(|e| !e.is_dir && workspace::is_markdown(&e.path))
            .filter_map(|e| {
                let lines = if self.doc.path.as_deref() == Some(e.path.as_path()) {
                    self.doc.lines.clone()
                } else {
                    if std::fs::metadata(&e.path).ok()?.len() > workspace::READ_MAX_BYTES {
                        return None;
                    }
                    let bytes = std::fs::read(&e.path).ok()?;
                    Document::from_bytes(&bytes, None).0.lines
                };
                Some(SearchFile {
                    path: e.path.clone(),
                    shown: workspace::relative(&dir, &e.path),
                    lines,
                })
            })
            .collect();
        let (a, b) = self.sel();
        let query = if a.line == b.line {
            self.doc.text_range(a, b)
        } else {
            String::new()
        };
        self.open_palette(Palette::search(files, &query));
        self.search_origin = Some(SearchOrigin {
            doc: None,
            anchor: self.anchor,
            cursor: self.cursor,
            scroll: self.scroll,
            previewing: false,
        });
        self.view.search = true;
        self.fit_search_rows();
        self.clamp_scroll();
        self.search_status();
    }

    /// Sizes the search results list to the panel.
    pub(super) fn fit_search_rows(&mut self) {
        let rows = self.view.search_rows();
        if let Some(p) = &mut self.palette
            && p.mode == Mode::Search
        {
            p.set_rows(rows);
        }
    }

    /// Whether the editor below the search panel shows a result.
    pub(super) fn search_previewing(&self) -> bool {
        self.search_origin.as_ref().is_some_and(|o| o.previewing)
    }

    /// Shows the highlighted search result in the editor, centered, without
    /// opening it: the open document is kept aside, unchanged, until the search ends.
    pub(super) fn preview_match(&mut self) {
        let Some(Target::OpenMatch {
            path,
            line,
            col,
            len,
        }) = self
            .palette
            .as_ref()
            .and_then(|p| p.selected())
            .map(|i| i.target.clone())
        else {
            return;
        };
        let Some(o) = &mut self.search_origin else {
            return;
        };
        let kept = o.doc.as_ref().map(|d| d.path.as_deref());
        if kept == Some(Some(path.as_path())) {
            // Back to the open document.
            self.doc = o.doc.take().unwrap();
        } else if self.doc.path.as_deref() != Some(path.as_path()) {
            let Ok(bytes) = std::fs::read(&path) else {
                return self.say(format!("Could not read {}", path.display()));
            };
            let doc = Document::from_bytes(&bytes, Some(path)).0;
            let open = std::mem::replace(&mut self.doc, doc);
            o.doc.get_or_insert(open);
        }
        o.previewing = true;
        self.set_sel(Pos::new(line, col), Pos::new(line, col + len));
        self.center_caret();
        self.update_title();
        self.invalidate();
    }

    /// Puts the editor back where it was when the search opened, keeping the search.
    pub(super) fn show_search_origin(&mut self) {
        let Some(o) = &mut self.search_origin else {
            return;
        };
        if !o.previewing {
            return;
        }
        if let Some(doc) = o.doc.take() {
            self.doc = doc;
        }
        o.previewing = false;
        (self.anchor, self.cursor, self.scroll) = (o.anchor, o.cursor, o.scroll);
        self.clamp_scroll();
        self.update_title();
        self.invalidate();
    }

    /// Removes the search panel, with the editor back where it was.
    pub(super) fn end_search(&mut self) {
        if self.search_origin.is_none() {
            return;
        }
        self.show_search_origin();
        self.search_origin = None;
        self.view.search = false;
        self.relayout();
    }

    /// Scrolls so the caret is in the middle of the view, where the document allows.
    pub(super) fn center_caret(&mut self) {
        self.ensure_layout();
        if let Some((_, y, h)) = self.view.caret_rect(self.cursor) {
            self.scroll = y + h / 2.0 - self.view_h() / 2.0;
        }
        self.clamp_scroll();
    }

    /// Shows the number of Search in Files results in the status bar.
    pub(super) fn search_status(&mut self) {
        let Some(p) = &self.palette else { return };
        if p.mode != Mode::Search {
            return;
        }
        if p.query.trim().is_empty() {
            self.message = None;
            return;
        }
        let (lines, files, more) = p.found;
        let s = |n: usize| if n == 1 { "" } else { "s" };
        let msg = if more {
            format!("First {lines} results in {files} file{}", s(files))
        } else {
            format!("{lines} result{} in {files} file{}", s(lines), s(files))
        };
        self.say(msg);
    }

    /// Opens a Search in Files result with the match selected.
    pub(super) fn open_match(&mut self, path: &Path, line: usize, col: usize, len: usize) {
        self.open_file_in_folder(path);
        if self.doc.path.as_deref() != Some(path) {
            return;
        }
        self.set_sel(Pos::new(line, col), Pos::new(line, col + len));
        let (a, b) = self.sel();
        if a.line == b.line {
            // F3 goes on to the next match in the file.
            self.find_query = self.doc.text_range(a, b);
        }
        self.tree_focus = false;
        self.changed(false);
        self.center_caret();
    }

    /// Ctrl+R: pick a recent folder.
    pub(super) fn recent_folders(&mut self) {
        let list = workspace::recent(&Settings::load());
        self.open_palette(Palette::folders(&list, self.folder.as_deref()));
    }

    pub(super) fn open_file_in_folder(&mut self, path: &Path) {
        if self.doc.path.as_deref() == Some(path) {
            return;
        }
        if !workspace::is_text(path) {
            return platform::shell_open(self.hwnd, &path.to_string_lossy());
        }
        if self.confirm_discard() {
            self.open_path(path);
            self.reveal_in_tree(path);
        }
    }

    // -----------------------------------------------------------------------
    // Folder settings and autosave

    pub(super) fn folder_settings_palette(&mut self) {
        if self.folder.is_none() {
            return self.say("Open a folder first (Ctrl+R)");
        }
        let p = Palette::folder_settings(&self.folder_settings);
        self.open_palette(p);
    }

    /// Edits a text folder setting in the status bar prompt.
    pub(super) fn folder_text_prompt(&mut self, opt: workspace::FolderOpt) {
        let Some(dir) = self.folder.clone() else {
            return;
        };
        let text = opt.text(&self.folder_settings);
        let cur = text.len();
        self.open_path_prompt(PromptKind::FolderText(opt), dir, text, cur);
    }

    /// The daily notes folder.
    fn daily_dir(&self) -> Option<PathBuf> {
        let dir = self.folder.as_ref()?;
        Some(match self.folder_settings.daily_folder.as_str() {
            "" => dir.clone(),
            sub => dir.join(sub),
        })
    }

    /// Alt+D: opens today's note in the daily notes folder, making it (from the
    /// template, if one is set) when it does not exist yet.
    pub(super) fn daily_note(&mut self) {
        let Some(dir) = self.daily_dir() else {
            return self.say("Open a folder first (Ctrl+R)");
        };
        let date = crate::slash::date_text(platform::today(), 0);
        let path = dir.join(format!("{date}.md"));
        if !path.exists() {
            let text = match &self.folder_settings.daily_template {
                Some(t) => {
                    let t = self.folder.clone().unwrap_or_default().join(t);
                    match std::fs::read_to_string(&t) {
                        Ok(template) => workspace::fill_template(&template, &date),
                        Err(e) => {
                            self.say(format!("Template {} not read: {e}", t.display()));
                            format!("# {date}\n\n")
                        }
                    }
                }
                None => format!("# {date}\n\n"),
            };
            if let Err(e) = std::fs::create_dir_all(&dir).and_then(|()| std::fs::write(&path, text))
            {
                return self.say(format!("Could not make {}: {e}", path.display()));
            }
            self.rescan();
        }
        self.open_daily(&path);
    }

    /// The daily note before or after the open one (or today, when the open file is
    /// not a daily note).
    pub(super) fn step_daily_note(&mut self, forward: bool) {
        let Some(dir) = self.daily_dir() else {
            return self.say("Open a folder first (Ctrl+R)");
        };
        let stem = |p: &Path| p.file_stem().map(|s| s.to_string_lossy().into_owned());
        let names: Vec<String> = self
            .entries
            .iter()
            .filter(|e| {
                !e.is_dir
                    && workspace::is_markdown(&e.path)
                    && e.path
                        .parent()
                        .is_some_and(|p| workspace::same_path(p, &dir))
            })
            .filter_map(|e| stem(&e.path))
            .collect();
        let current = self
            .doc
            .path
            .as_deref()
            .and_then(stem)
            .filter(|s| workspace::is_date_name(s))
            .unwrap_or_else(|| crate::slash::date_text(platform::today(), 0));
        match workspace::daily_neighbor(&names, &current, forward) {
            Some(name) => self.open_daily(&dir.join(format!("{name}.md"))),
            None => self.say(if forward {
                "No later daily note"
            } else {
                "No earlier daily note"
            }),
        }
    }

    fn open_daily(&mut self, path: &Path) {
        if self.doc.path.as_deref() == Some(path) || !self.confirm_discard() {
            return;
        }
        self.open_path(path);
        if self.doc.path.as_deref() == Some(path) {
            let end = self.doc.end();
            self.set_sel(end, end);
            self.changed(false);
            self.reveal_in_tree(path);
        }
    }

    /// Applies and saves a change to the open folder's settings.
    pub(super) fn change_folder_settings(&mut self, f: impl FnOnce(&mut FolderSettings)) {
        let Some(dir) = self.folder.clone() else {
            return;
        };
        f(&mut self.folder_settings);
        let fs = self.folder_settings.clone();
        if let Some(p) = &mut self.palette {
            p.refresh_folder_values(&fs);
        }
        if let Err(e) = Settings::update(|s| fs.store(s, &dir)) {
            self.say(format!("Folder settings not saved: {e}"));
        }
        self.invalidate();
    }

    /// Whether autosave is on for the open file (it must be inside the folder).
    pub(super) fn autosave_applies(&self) -> bool {
        self.folder_settings.autosave
            && matches!((&self.folder, &self.doc.path), (Some(dir), Some(p)) if workspace::contains(dir, p))
    }

    /// Whether autosave still has unsaved changes to write.
    pub(super) fn autosave_pending(&self) -> bool {
        self.autosave_applies()
            && self.doc.is_dirty()
            && self.autosave_failed != Some(self.doc.version)
    }

    /// Called from the timer: saves once the configured delay has passed since the
    /// last edit.
    pub(super) fn autosave_tick(&mut self) {
        if !self.autosave_pending() {
            return;
        }
        let delay = Duration::from_secs(self.folder_settings.autosave_secs as u64);
        if self.last_edit.elapsed() < delay {
            return;
        }
        let path = self.doc.path.clone().unwrap();
        if self.disk_differs() {
            // Leave the newer file alone; focusing the window asks what to do.
            self.autosave_failed = Some(self.doc.version);
            return self.say(format!(
                "Autosave paused: {} changed on disk",
                self.doc_name()
            ));
        }
        match std::fs::write(&path, self.doc.to_bytes()) {
            Ok(()) => {
                self.doc.mark_saved();
                self.doc.disk = doc::stamp(&path);
                self.update_title();
                self.invalidate();
            }
            Err(e) => {
                // Don't retry until the next edit.
                self.autosave_failed = Some(self.doc.version);
                self.say(format!("Autosave failed: {e}"));
            }
        }
    }

    // -----------------------------------------------------------------------
    // Explorer panel

    pub(super) fn explorer_visible(&self) -> bool {
        self.explorer && self.folder.is_some()
    }

    pub(super) fn set_explorer(&mut self, show: bool) {
        if show != self.explorer {
            self.save_setting(|s| s.set("explorer", if show { "on" } else { "off" }));
        }
        self.explorer = show;
        if !self.explorer_visible() {
            self.tree_focus = false;
        }
        self.view.explorer = self.explorer_visible();
        self.relayout();
    }

    /// Ctrl+B: show or hide the explorer without moving focus to it.
    pub(super) fn toggle_explorer(&mut self) {
        if self.folder.is_none() {
            return self.say("Open a folder first (Ctrl+R)");
        }
        self.set_explorer(!self.explorer_visible());
        self.invalidate();
    }

    /// Ctrl+Shift+E: move focus between the explorer (showing it if hidden) and the editor.
    pub(super) fn toggle_explorer_focus(&mut self) {
        if self.tree_focus {
            self.tree_focus = false;
            self.invalidate();
        } else {
            self.focus_explorer();
        }
    }

    /// Ctrl+0: move keyboard focus to the explorer (showing it if hidden).
    pub(super) fn focus_explorer(&mut self) {
        if self.folder.is_none() {
            return self.say("Open a folder first (Ctrl+R)");
        }
        if !self.explorer_visible() {
            self.set_explorer(true);
        }
        self.tree_focus = true;
        self.slash = None;
        if self.tree_sel.is_none() {
            match self.doc.path.clone() {
                Some(p) => self.reveal_in_tree(&p),
                None => self.tree_sel = self.entries.first().map(|e| e.path.clone()),
            }
        }
        self.invalidate();
    }

    fn tree_rows(&self) -> Vec<usize> {
        workspace::visible(&self.entries, &self.expanded, self.notes_only)
    }

    fn tree_sel_row(&self, rows: &[usize]) -> Option<usize> {
        let sel = self.tree_sel.as_ref()?;
        rows.iter().position(|&i| &self.entries[i].path == sel)
    }

    fn tree_height(&self) -> f32 {
        self.view.h - self.view.status_h() - self.view.sidebar_header_h()
    }

    fn clamp_tree_scroll(&mut self, rows: usize) {
        let max = (rows as f32 * self.view.sidebar_row_h() - self.tree_height()).max(0.0);
        self.tree_scroll = self.tree_scroll.clamp(0.0, max);
    }

    fn select_row(&mut self, rows: &[usize], i: usize) {
        let Some(&e) = rows.get(i) else { return };
        self.tree_sel = Some(self.entries[e].path.clone());
        let row_h = self.view.sidebar_row_h();
        let top = i as f32 * row_h;
        if top < self.tree_scroll {
            self.tree_scroll = top;
        } else if top + row_h > self.tree_scroll + self.tree_height() {
            self.tree_scroll = top + row_h - self.tree_height();
        }
        self.clamp_tree_scroll(rows.len());
    }

    /// Expands the folders above `path` and selects it.
    pub(super) fn reveal_in_tree(&mut self, path: &Path) {
        let Some(dir) = self.folder.clone() else {
            return;
        };
        let mut p = path.parent();
        while let Some(parent) = p {
            if !workspace::contains(&dir, parent) || workspace::contains(parent, &dir) {
                break;
            }
            self.expanded.insert(parent.to_path_buf());
            p = parent.parent();
        }
        let rows = self.tree_rows();
        if let Some(i) = rows.iter().position(|&e| self.entries[e].path == path) {
            self.select_row(&rows, i);
        }
    }

    /// The folder new items go into: the selected folder, the selected file's
    /// folder, or the root.
    fn target_dir(&self) -> Option<PathBuf> {
        let root = self.folder.clone()?;
        let sel = self
            .tree_sel
            .as_ref()
            .and_then(|p| self.entries.iter().find(|e| &e.path == p));
        Some(match sel {
            Some(e) if e.is_dir => e.path.clone(),
            Some(e) => e.path.parent().map(Path::to_path_buf).unwrap_or(root),
            None => root,
        })
    }

    fn activate_entry(&mut self, e: &Entry) {
        if e.is_dir {
            if !self.expanded.remove(&e.path) {
                self.expanded.insert(e.path.clone());
            }
            let rows = self.tree_rows();
            self.clamp_tree_scroll(rows.len());
        } else {
            self.open_file_in_folder(&e.path);
        }
    }

    /// Keys while the explorer has focus. Returns false for keys it leaves to the
    /// editor's global shortcuts.
    pub(super) fn tree_key(&mut self, key: VIRTUAL_KEY, ctrl: bool) -> bool {
        let rows = self.tree_rows();
        let cur = self.tree_sel_row(&rows);
        let entry = cur.map(|i| self.entries[rows[i]].clone());
        let page = (self.tree_height() / self.view.sidebar_row_h()).max(1.0) as usize;
        let last = rows.len().saturating_sub(1);
        match key {
            VK_X if ctrl => self.tree_cut = entry.map(|e| e.path),
            VK_V if ctrl => self.paste_cut(),
            _ if ctrl => return false,
            VK_UP => self.select_row(&rows, cur.map_or(0, |i| i.saturating_sub(1))),
            VK_DOWN => self.select_row(&rows, cur.map_or(0, |i| (i + 1).min(last))),
            VK_PRIOR => self.select_row(&rows, cur.map_or(0, |i| i.saturating_sub(page))),
            VK_NEXT => self.select_row(&rows, cur.map_or(0, |i| (i + page).min(last))),
            VK_HOME => self.select_row(&rows, 0),
            VK_END => self.select_row(&rows, last),
            VK_LEFT => {
                if let (Some(i), Some(e)) = (cur, &entry) {
                    if e.is_dir && self.expanded.remove(&e.path) {
                        // Collapsed it.
                    } else if let Some(p) = (0..i)
                        .rev()
                        .find(|&j| self.entries[rows[j]].depth < e.depth)
                    {
                        self.select_row(&rows, p);
                    }
                }
            }
            VK_RIGHT => {
                if let (Some(i), Some(e)) = (cur, &entry)
                    && e.is_dir
                {
                    if self.expanded.insert(e.path.clone()) {
                        // Expanded it.
                    } else if i < last && self.entries[rows[i + 1]].depth > e.depth {
                        self.select_row(&rows, i + 1);
                    }
                }
            }
            VK_RETURN => {
                if let Some(e) = entry {
                    self.activate_entry(&e);
                    if !e.is_dir {
                        self.tree_focus = false;
                    }
                }
            }
            VK_DELETE => {
                if let Some(e) = entry {
                    self.delete_path(&e.path);
                }
            }
            VK_F2 => {
                if let Some(e) = entry {
                    self.rename_prompt(&e.path);
                }
            }
            VK_ESCAPE => {
                if self.tree_cut.take().is_none() {
                    self.tree_focus = false;
                }
            }
            VK_A => {
                self.new_item_prompt(super::shift_down());
                self.skip_char = true;
            }
            VK_F1 | VK_F3 => return false,
            _ => {}
        }
        self.invalidate();
        true
    }

    // -----------------------------------------------------------------------
    // File operations

    fn delete_path(&mut self, path: &Path) {
        if self.prefs.confirm_delete {
            let what = if path.is_dir() { "folder" } else { "file" };
            let r = platform::message_box(
                self.hwnd,
                &format!(
                    "Move the {what} \u{201c}{}\u{201d} to the Recycle Bin?",
                    name_of(path)
                ),
                APP_NAME,
                MB_YESNO | MB_ICONWARNING,
            );
            if r != IDYES {
                return;
            }
        }
        let rows = self.tree_rows();
        let at = self.tree_sel_row(&rows);
        if !platform::recycle(self.hwnd, path) {
            return self.say(format!("Could not delete {}", name_of(path)));
        }
        if self.tree_cut.as_deref() == Some(path) {
            self.tree_cut = None;
        }
        self.rescan();
        // Keep the selection near where it was.
        let rows = self.tree_rows();
        if let Some(i) = at.filter(|_| !rows.is_empty()) {
            self.select_row(&rows, i.min(rows.len() - 1));
        }
        let open_deleted = self
            .doc
            .path
            .as_deref()
            .is_some_and(|p| workspace::contains(path, p));
        self.say(if open_deleted {
            format!(
                "Moved {} to the Recycle Bin (the open file stays open until you close it)",
                name_of(path)
            )
        } else {
            format!("Moved {} to the Recycle Bin", name_of(path))
        });
    }

    /// Save As with a folder open: picks the path in the palette, starting in the
    /// note's folder (or the one selected in the explorer). Returns false when the
    /// Windows dialog is used instead: the setting is off, or the note is not in
    /// the folder.
    pub(super) fn save_palette(&mut self) -> bool {
        let Some(root) = self.folder.clone() else {
            return false;
        };
        let outside = self
            .doc
            .path
            .as_deref()
            .is_some_and(|p| !workspace::contains(&root, p));
        if !self.prefs.save_prompt || outside {
            return false;
        }
        let dirs = self
            .entries
            .iter()
            .filter(|e| e.is_dir)
            .map(|e| workspace::relative(&root, &e.path).replace('/', "\\"))
            .collect();
        let dir = self
            .doc_dir()
            .or_else(|| self.target_dir())
            .unwrap_or_else(|| root.clone());
        let rel = workspace::relative(&root, &dir);
        let start = if rel.is_empty() {
            rel
        } else {
            format!("{rel}/")
        };
        let suggested = match self.doc.path.as_deref() {
            Some(p) => name_of(p),
            None => self.suggested_name(),
        };
        self.open_palette(Palette::save_as(&root, dirs, &start, &suggested));
        true
    }

    /// Enter on the palette's save row: creates any missing folders and saves.
    pub(super) fn save_in_folder(&mut self, path: &Path) {
        if let Some(dir) = path.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            return self.say(format!("Could not create {}: {e}", name_of(dir)));
        }
        self.close_palette(false);
        if self.write_to(path) {
            self.reveal_in_tree(path);
        }
    }

    /// Renames or moves `src` to `target`, following the open file if it moves and
    /// offering to update links to it. Returns None when it failed, else text to add
    /// to the status message (about links updated).
    fn rename_to(&mut self, src: &Path, target: &Path) -> Option<String> {
        if target.exists() {
            self.say(format!(
                "{} already exists",
                workspace::relative(self.folder.as_deref().unwrap_or(Path::new("")), target)
            ));
            return None;
        }
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Links are found before the move, while the old paths still resolve.
        let moved = self.notes_moving(src, target);
        let refs = self.links_to_move(&moved);
        if let Err(e) = std::fs::rename(src, target) {
            platform::message_box(
                self.hwnd,
                &format!("Could not move {}:\n{e}", name_of(src)),
                APP_NAME,
                MB_OK | MB_ICONERROR,
            );
            return None;
        }
        if let Some(open) = self.doc.path.clone()
            && workspace::contains(src, &open)
        {
            let rest = open
                .strip_prefix(src)
                .map(Path::to_path_buf)
                .unwrap_or_default();
            self.doc.path = Some(if rest.as_os_str().is_empty() {
                target.to_path_buf()
            } else {
                target.join(rest)
            });
            self.remember_file();
            self.update_title();
        }
        if self.expanded.remove(src) {
            self.expanded.insert(target.to_path_buf());
        }
        self.tree_cut = None;
        self.rescan();
        self.reveal_in_tree(target);
        Some(self.update_links(&moved, refs))
    }

    /// The notes that move when `src` moves to `target`: (old path, new path).
    fn notes_moving(&self, src: &Path, target: &Path) -> Vec<(PathBuf, PathBuf)> {
        if self.folder.is_none() {
            return Vec::new();
        }
        if !src.is_dir() {
            return if workspace::is_markdown(src) {
                vec![(src.to_path_buf(), target.to_path_buf())]
            } else {
                Vec::new()
            };
        }
        self.entries
            .iter()
            .filter(|e| !e.is_dir && workspace::is_markdown(&e.path))
            .filter_map(|e| {
                let rest = e.path.strip_prefix(src).ok()?;
                Some((e.path.clone(), target.join(rest)))
            })
            .collect()
    }

    fn links_to_move(&mut self, moved: &[(PathBuf, PathBuf)]) -> Vec<workspace::LinkRef> {
        let Some(root) = self.folder.clone().filter(|_| !moved.is_empty()) else {
            return Vec::new();
        };
        let old: Vec<PathBuf> = moved.iter().map(|m| m.0.clone()).collect();
        let open = self.doc.path.clone();
        let open = open.as_deref().map(|p| (p, self.doc.lines.as_slice()));
        workspace::links_to_move(&root, &self.entries, &old, &mut self.link_cache, open)
    }

    /// After notes moved: offers to rewrite the links found before the move (`refs`)
    /// to their new places. The open note is edited as one undo step. Returns what
    /// was done, for the status bar (empty when nothing was).
    fn update_links(
        &mut self,
        moved: &[(PathBuf, PathBuf)],
        refs: Vec<workspace::LinkRef>,
    ) -> String {
        let Some(root) = self.folder.clone() else {
            return String::new();
        };
        let now = |p: &Path| {
            moved
                .iter()
                .find(|(o, _)| workspace::same_path(o, p))
                .map_or_else(|| p.to_path_buf(), |(_, n)| n.clone())
        };
        // Each note to change, with its (line, old target, new target) edits.
        let mut edits: Vec<(PathBuf, Vec<LinkEdit>)> = Vec::new();
        for r in refs {
            let (note, dest) = (now(&r.note), now(&r.dest));
            let Some(new) = workspace::new_target(&root, &self.entries, &note, &r.target, &dest)
            else {
                continue;
            };
            match edits.iter_mut().find(|(n, _)| *n == note) {
                Some((_, list)) => list.push((r.line, r.target, new)),
                None => edits.push((note, vec![(r.line, r.target, new)])),
            }
        }
        if edits.is_empty() {
            return String::new();
        }
        let n: usize = edits.iter().map(|e| e.1.len()).sum();
        let plural =
            |k: usize, one: &str, many: &str| format!("{k} {}", if k == 1 { one } else { many });
        let what = format!(
            "{} in {}",
            plural(n, "link", "links"),
            plural(edits.len(), "note", "notes")
        );
        let r = platform::message_box(
            self.hwnd,
            &format!("Update {what} to the new location?"),
            APP_NAME,
            MB_YESNO | MB_ICONQUESTION,
        );
        if r != IDYES {
            return String::new();
        }
        let mut failed = 0;
        for (note, list) in edits {
            if self.doc.path.as_deref() == Some(note.as_path()) {
                let clean = !self.doc.is_dirty();
                self.begin(EditKind::Other);
                for (line, old, new) in list {
                    let Some(text) = self.doc.lines.get(line) else {
                        continue;
                    };
                    if let Some(text) = workspace::retarget(text, &old, &new) {
                        let len = self.doc.line_len(line);
                        self.replace_keep(Pos::new(line, 0), Pos::new(line, len), &text);
                    }
                }
                self.commit();
                if clean {
                    self.save();
                }
                continue;
            }
            let Ok(bytes) = std::fs::read(&note) else {
                failed += 1;
                continue;
            };
            let mut doc = Document::from_bytes(&bytes, None).0;
            for (line, old, new) in list {
                if let Some(text) = doc
                    .lines
                    .get(line)
                    .and_then(|t| workspace::retarget(t, &old, &new))
                {
                    doc.lines[line] = text;
                }
            }
            if std::fs::write(&note, doc.to_bytes()).is_err() {
                failed += 1;
            }
        }
        if failed == 0 {
            format!("; updated {what}")
        } else {
            format!("; {} could not be updated", plural(failed, "note", "notes"))
        }
    }

    /// Moves `src` into the folder `dest`.
    fn move_into(&mut self, src: &Path, dest: &Path) {
        if workspace::contains(src, dest) {
            return self.say("A folder can't be moved into itself");
        }
        if src
            .parent()
            .is_some_and(|p| workspace::contains(p, dest) && workspace::contains(dest, p))
        {
            return;
        }
        let target = dest.join(name_of(src));
        if let Some(links) = self.rename_to(src, &target) {
            let root = self.folder.clone().unwrap_or_default();
            let to = workspace::relative(&root, dest);
            self.say(format!(
                "Moved {} to {}{links}",
                name_of(src),
                if to.is_empty() { name_of(&root) } else { to }
            ));
        }
    }

    /// Ctrl+V in the explorer: move the cut entry into the selected folder.
    fn paste_cut(&mut self) {
        let (Some(src), Some(dest)) = (self.tree_cut.clone(), self.target_dir()) else {
            return self.say("Cut a file or folder first (Ctrl+X)");
        };
        self.move_into(&src, &dest);
    }

    fn rename_prompt(&mut self, path: &Path) {
        let name = name_of(path);
        // Caret before the extension, like Explorer.
        let cur = if path.is_dir() {
            name.len()
        } else {
            name.rfind('.').filter(|&i| i > 0).unwrap_or(name.len())
        };
        self.open_path_prompt(PromptKind::Rename, path.to_path_buf(), name, cur);
    }

    pub(super) fn new_item_prompt(&mut self, folder: bool) {
        let Some(dir) = self.target_dir() else {
            return self.say("Open a folder first (Ctrl+R)");
        };
        let kind = if folder {
            PromptKind::NewFolder
        } else {
            PromptKind::NewFile
        };
        self.open_path_prompt(kind, dir, String::new(), 0);
    }

    /// Enter in the rename / new file / new folder prompt.
    pub(super) fn path_prompt_done(&mut self, kind: PromptKind, path: PathBuf, text: &str) {
        if let PromptKind::FolderText(opt) = kind {
            self.change_folder_settings(|f| opt.set_text(f, text));
            return self.say(format!(
                "{}: {}",
                opt.label(),
                opt.value_text(&self.folder_settings)
            ));
        }
        let name = text.trim().replace('/', "\\");
        if name.is_empty() {
            return;
        }
        match kind {
            PromptKind::Rename => {
                let target = path.parent().unwrap_or(Path::new("")).join(&name);
                if target != path
                    && let Some(links) = self.rename_to(&path, &target)
                {
                    self.say(format!("Renamed to {name}{links}"));
                }
            }
            PromptKind::NewFile => {
                let mut target = path.join(&name);
                if target.extension().is_none() {
                    target.set_extension("md");
                }
                if target.exists() {
                    return self.say(format!("{} already exists", name_of(&target)));
                }
                if let Some(parent) = target.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::write(&target, "") {
                    return self.say(format!("Could not create {}: {e}", name_of(&target)));
                }
                self.rescan();
                self.open_file_in_folder(&target);
                // Ready to write in, as after opening a file from the explorer.
                self.tree_focus = false;
            }
            PromptKind::NewFolder => {
                let target = path.join(&name);
                if let Err(e) = std::fs::create_dir_all(&target) {
                    return self.say(format!("Could not create {name}: {e}"));
                }
                self.rescan();
                self.reveal_in_tree(&target);
            }
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Mouse in the explorer

    fn tree_row_at(&self, y: f32, rows: usize) -> Option<usize> {
        let header_h = self.view.sidebar_header_h();
        if y < header_h {
            return None;
        }
        let i = ((y - header_h + self.tree_scroll) / self.view.sidebar_row_h()) as usize;
        (i < rows).then_some(i)
    }

    /// The explorer header's buttons.
    fn tree_button_click(&mut self, b: SidebarButton) {
        match b {
            SidebarButton::NewFile => self.new_item_prompt(false),
            SidebarButton::NewFolder => self.new_item_prompt(true),
            SidebarButton::NotesOnly => {
                self.notes_only = !self.notes_only;
                let on = self.notes_only;
                let rows = self.tree_rows();
                self.clamp_tree_scroll(rows.len());
                self.say(if on {
                    "Explorer: showing Markdown files only"
                } else {
                    "Explorer: showing all files"
                });
                self.save_setting(|s| {
                    s.set("explorer.notes_only", if on { "on" } else { "off" })
                });
            }
        }
        self.invalidate();
    }

    pub(super) fn tree_mouse_down(&mut self, x: f32, y: f32) {
        self.tree_focus = true;
        self.slash = None;
        if let Some(b) = SidebarButton::at(x, y, self.view.ui()) {
            return self.tree_button_click(b);
        }
        let rows = self.tree_rows();
        self.tree_drag = self.tree_row_at(y, rows.len()).map(|i| {
            let path = self.entries[rows[i]].path.clone();
            self.tree_sel = Some(path.clone());
            TreeDrag {
                path,
                y0: y,
                active: false,
                target: None,
            }
        });
        unsafe {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetCapture(self.hwnd);
        }
        self.invalidate();
    }

    /// Mouse movement over (or dragging from) the explorer.
    pub(super) fn tree_mouse_move(&mut self, x: f32, y: f32) {
        let rows = self.tree_rows();
        let Some(d) = &self.tree_drag else {
            let inside = x < self.view.sidebar_w();
            let hover = inside.then(|| self.tree_row_at(y, rows.len())).flatten();
            let button = inside
                .then(|| SidebarButton::at(x, y, self.view.ui()))
                .flatten();
            if hover != self.tree_hover || button != self.tree_button {
                self.tree_hover = hover;
                self.tree_button = button;
                self.invalidate();
            }
            return;
        };
        if !d.active && (y - d.y0).abs() < 5.0 {
            return;
        }
        let src = d.path.clone();
        // Drop onto a folder row, the folder of a file row, or below the rows: the root.
        let target = if x >= self.view.sidebar_w() {
            None
        } else {
            match self.tree_row_at(y.max(self.view.sidebar_header_h()), rows.len()) {
                Some(i) => {
                    let e = &self.entries[rows[i]];
                    Some(if e.is_dir {
                        e.path.clone()
                    } else {
                        e.path.parent().map(Path::to_path_buf).unwrap_or_default()
                    })
                }
                None => self.folder.clone(),
            }
        };
        let target = target.filter(|t| !workspace::contains(&src, t));
        if let Some(d) = &mut self.tree_drag {
            d.active = true;
            d.target = target;
        }
        self.invalidate();
    }

    pub(super) fn tree_mouse_up(&mut self) {
        let Some(d) = self.tree_drag.take() else {
            return;
        };
        if d.active {
            if let Some(dest) = d.target {
                self.move_into(&d.path, &dest);
            }
        } else if let Some(e) = self.entries.iter().find(|e| e.path == d.path).cloned() {
            self.activate_entry(&e);
        }
        self.invalidate();
    }

    pub(super) fn tree_wheel(&mut self, delta: i16) {
        self.tree_scroll -= delta as f32 / 120.0 * 3.0 * self.view.sidebar_row_h();
        let rows = self.tree_rows();
        self.clamp_tree_scroll(rows.len());
        self.invalidate();
    }

    /// Right-click in the explorer: a menu for the entry under the mouse.
    pub(super) fn tree_right_click(&mut self, y: f32, px: i32, py: i32) {
        self.tree_focus = true;
        let rows = self.tree_rows();
        let entry = self
            .tree_row_at(y, rows.len())
            .map(|i| self.entries[rows[i]].clone());
        self.tree_sel = entry.as_ref().map(|e| e.path.clone());
        self.invalidate();
        let has = entry.is_some();
        let items = [
            MenuItem::Item {
                id: 1,
                label: "New File\u{2026}",
                checked: false,
                enabled: true,
            },
            MenuItem::Item {
                id: 2,
                label: "New Folder\u{2026}",
                checked: false,
                enabled: true,
            },
            MenuItem::Separator,
            MenuItem::Item {
                id: 3,
                label: "Cut\tCtrl+X",
                checked: false,
                enabled: has,
            },
            MenuItem::Item {
                id: 4,
                label: "Paste\tCtrl+V",
                checked: false,
                enabled: self.tree_cut.is_some(),
            },
            MenuItem::Item {
                id: 5,
                label: "Rename\u{2026}\tF2",
                checked: false,
                enabled: has,
            },
            MenuItem::Item {
                id: 6,
                label: "Delete\tDel",
                checked: false,
                enabled: has,
            },
            MenuItem::Separator,
            MenuItem::Item {
                id: 7,
                label: "Reveal in File Explorer",
                checked: false,
                enabled: true,
            },
        ];
        let target = entry
            .as_ref()
            .map(|e| e.path.clone())
            .or_else(|| self.folder.clone());
        match platform::popup_menu(self.hwnd, &items, px, py) {
            Some(1) => self.new_item_prompt(false),
            Some(2) => self.new_item_prompt(true),
            Some(3) => self.tree_cut = target,
            Some(4) => self.paste_cut(),
            Some(5) => {
                if let Some(p) = target {
                    self.rename_prompt(&p);
                }
            }
            Some(6) => {
                if let Some(p) = target {
                    self.delete_path(&p);
                }
            }
            Some(7) => {
                if let Some(p) = target {
                    platform::reveal(self.hwnd, &p);
                }
            }
            _ => {}
        }
        self.invalidate();
    }

    /// What the renderer needs to draw the explorer.
    pub(super) fn sidebar_view(&self) -> Option<SidebarView> {
        if !self.explorer_visible() {
            return None;
        }
        let title = name_of(self.folder.as_deref()?);
        let rows = self.tree_rows();
        let drag_target = self
            .tree_drag
            .as_ref()
            .filter(|d| d.active)
            .and_then(|d| d.target.clone());
        let drop_root = drag_target.is_some() && drag_target == self.folder;
        Some(SidebarView {
            title,
            notes_only: self.notes_only,
            hover_button: self.tree_button,
            selected: self.tree_sel_row(&rows),
            hover: self.tree_hover,
            drop: drag_target
                .as_ref()
                .and_then(|t| rows.iter().position(|&i| &self.entries[i].path == t)),
            drop_root,
            scroll: self.tree_scroll,
            focused: self.tree_focus && self.focused,
            rows: rows
                .iter()
                .map(|&i| {
                    let e = &self.entries[i];
                    SidebarRow {
                        name: e.name.clone(),
                        depth: e.depth,
                        is_dir: e.is_dir,
                        expanded: self.expanded.contains(&e.path),
                        current: self.doc.path.as_deref() == Some(e.path.as_path()),
                        cut: self.tree_cut.as_deref() == Some(e.path.as_path()),
                    }
                })
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::app;
    use super::*;
    use crate::palette::Target;

    #[test]
    fn save_as_in_a_folder_uses_the_palette() {
        let root = std::env::temp_dir().join(format!("penmark-savepal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("notes")).unwrap();
        let mut a = app("# Plan\n");
        a.prefs.save_prompt = true;
        a.folder = Some(root.clone());
        a.entries = workspace::scan(&root);
        assert!(!a.save());
        let p = a.palette.as_mut().expect("save palette");
        assert_eq!(p.mode, Mode::SaveAs);
        assert_eq!(p.visible().0[0].0.label, "Save as Plan.md");
        p.insert("notes/new\\idea");
        a.palette_accept();
        let saved = root.join("notes").join("new").join("idea.md");
        assert!(a.palette.is_none());
        assert_eq!(a.doc.path.as_deref(), Some(saved.as_path()));
        assert_eq!(std::fs::read_to_string(&saved).unwrap(), "# Plan\n");
        assert!(!a.doc.is_dirty());

        // Save As starts in the note's folder.
        a.save_as();
        assert_eq!(a.palette.as_ref().unwrap().query, "notes/new/");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn search_in_files() {
        let root = std::env::temp_dir().join(format!("penmark-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let open = root.join("open.md");
        std::fs::write(&open, "saved text").unwrap();
        std::fs::write(root.join("sub").join("b.md"), "x\r\nA Needle here").unwrap();
        std::fs::write(root.join("skip.txt"), "needle").unwrap();

        // The open file is searched as edited; other files as saved; .txt is skipped.
        let mut a = app("one\ntwo needle");
        a.doc.path = Some(open.clone());
        a.folder = Some(root.clone());
        a.set_sel(Pos::new(1, 4), Pos::new(1, 10));
        a.search_files();
        let p = a.palette.as_ref().unwrap();
        assert_eq!(p.query, "needle");
        assert_eq!(p.found, (2, 2, false));
        let (rows, _) = p.visible();
        assert_eq!(rows[0].0.detail, "sub/b.md:2");
        assert_eq!(rows[1].0.detail, "open.md:2");

        // Accepting a result in the open file selects the match.
        a.set_sel(Pos::default(), Pos::default());
        a.palette.as_mut().unwrap().move_by(1);
        let target = a
            .palette
            .as_ref()
            .unwrap()
            .selected()
            .unwrap()
            .target
            .clone();
        assert!(matches!(
            target,
            Target::OpenMatch {
                line: 1,
                col: 4,
                ..
            }
        ));
        a.palette_accept();
        assert!(a.palette.is_none());
        assert_eq!(a.sel(), (Pos::new(1, 4), Pos::new(1, 10)));
        assert_eq!(a.find_query, "needle");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn search_previews_results() {
        let root = std::env::temp_dir().join(format!("penmark-preview-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("sub")).unwrap();
        let open = root.join("open.md");
        let other = root.join("sub").join("b.md");
        std::fs::write(&open, "saved text").unwrap();
        std::fs::write(&other, "x\r\nA Needle here").unwrap();

        let mut a = app("one\ntwo needle\nthree");
        a.doc.path = Some(open.clone());
        a.folder = Some(root.clone());
        a.set_sel(Pos::new(1, 4), Pos::new(1, 10));
        let version = a.doc.version;
        a.search_files();
        assert!(a.view.search);
        assert!(a.view.header_h() > a.view.h / 3.0);

        // Results are shown in the editor as they are highlighted.
        a.palette_key(VK_DOWN, false, false);
        assert_eq!(a.doc.version, version);
        assert_eq!(a.sel(), (Pos::new(1, 4), Pos::new(1, 10)));
        a.palette_key(VK_UP, false, false);
        assert_eq!(a.doc.path.as_deref(), Some(other.as_path()));
        assert_eq!(a.sel(), (Pos::new(1, 2), Pos::new(1, 8)));

        // Escape clears the query and goes back to the open document...
        a.palette_key(VK_ESCAPE, false, false);
        let p = a.palette.as_ref().unwrap();
        assert!(p.query.is_empty() && p.selected().is_none());
        assert_eq!(a.doc.version, version);
        assert_eq!(a.doc.lines[2], "three");
        // ...and again closes the search.
        a.palette_key(VK_ESCAPE, false, false);
        assert!(a.palette.is_none() && !a.view.search);

        // Escape from a preview restores the place in the open document.
        a.set_sel(Pos::new(2, 1), Pos::new(2, 1));
        a.search_files();
        a.on_char(b'n' as u16);
        a.palette_key(VK_DOWN, false, false);
        assert_eq!(a.doc.path.as_deref(), Some(open.as_path()));
        a.palette_key(VK_UP, false, false);
        assert_eq!(a.doc.path.as_deref(), Some(other.as_path()));
        a.palette_key(VK_ESCAPE, false, false);
        a.palette_key(VK_ESCAPE, false, false);
        assert_eq!(a.doc.version, version);
        assert_eq!(a.sel(), (Pos::new(2, 1), Pos::new(2, 1)));

        // Enter opens the highlighted result (here in the open document, after
        // previewing another file).
        a.search_files();
        a.on_char(b'e' as u16);
        a.palette_key(VK_DOWN, false, false);
        a.palette_key(VK_UP, false, false);
        assert_eq!(a.doc.path.as_deref(), Some(other.as_path()));
        a.palette_key(VK_DOWN, false, false);
        a.palette_key(VK_RETURN, false, false);
        assert!(a.palette.is_none() && a.search_origin.is_none() && !a.view.search);
        assert_eq!(a.doc.version, version);
        assert_eq!(a.sel(), (Pos::new(0, 2), Pos::new(0, 3)));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn new_file_prompt_takes_keys_from_explorer() {
        let root = std::env::temp_dir().join(format!("penmark-newfile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        // A in the explorer starts a new file; the next A types into its name.
        let mut a = app("");
        a.folder = Some(root.clone());
        a.explorer = true;
        a.tree_focus = true;
        // A key press: the key, then the character it types.
        let press = |a: &mut App, vk: VIRTUAL_KEY, c: u8| {
            a.on_key(vk.0);
            a.on_char(c as u16);
        };
        press(&mut a, VK_A, b'a');
        assert_eq!(a.prompt.as_ref().unwrap().text, "");
        press(&mut a, VK_A, b'a');
        press(&mut a, VK_B, b'b');
        assert_eq!(a.prompt.as_ref().unwrap().text, "ab");
        // Up / Down stay in the prompt instead of moving the explorer.
        assert!(a.on_key(VK_DOWN.0));
        assert!(a.on_key(VK_RETURN.0));

        // The new file opens with the editor focused.
        let file = root.join("ab.md");
        assert!(file.exists());
        assert_eq!(a.doc.path.as_deref(), Some(file.as_path()));
        assert!(a.prompt.is_none() && !a.tree_focus);
        let _ = std::fs::remove_dir_all(&root);
    }
}
