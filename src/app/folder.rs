//! Folders: opening them, quick open, the explorer panel and its file operations,
//! folder settings, and autosave.

use std::path::{Path, PathBuf};
use std::time::Duration;

use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::WindowsAndMessaging::*;

use super::{APP_NAME, App, PromptKind};
use crate::doc::{Document, Pos};
use crate::palette::Palette;
use crate::platform::{self, MenuItem};
use crate::render::{SidebarButton, SidebarRow, SidebarView};
use crate::settings::Settings;
use crate::workspace::{self, Entry, FolderSettings};

/// An explorer entry being dragged; it becomes a move once the mouse travels.
pub(super) struct TreeDrag {
    path: PathBuf,
    y0: f32,
    active: bool,
    /// Folder the entry would be moved into.
    target: Option<PathBuf>,
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
            let _ = Settings::update(|s| workspace::remove_recent(s, dir));
            return self.say(format!("{} is no longer available", dir.display()));
        }
        if self.confirm_discard() {
            self.open_folder(dir);
        }
    }

    /// Opens `dir` and the file that was last open in it.
    pub fn open_folder(&mut self, dir: &Path) {
        let _ = Settings::update(|s| workspace::add_recent(s, dir));
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
            }
            None => self.blank_doc(),
        }
        // History is per folder.
        self.back.clear();
        self.forward.clear();
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
        let _ = Settings::update(|s| {
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

    /// Called from the timer: saves once the configured delay has passed since the
    /// last edit.
    pub(super) fn autosave_tick(&mut self) {
        if !self.autosave_applies()
            || !self.doc.is_dirty()
            || self.autosave_failed == Some(self.doc.version)
        {
            return;
        }
        let delay = Duration::from_secs(self.folder_settings.autosave_secs as u64);
        if self.last_edit.elapsed() < delay {
            return;
        }
        let path = self.doc.path.clone().unwrap();
        match std::fs::write(&path, self.doc.to_bytes()) {
            Ok(()) => {
                self.doc.mark_saved();
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
            let _ = Settings::update(|s| s.set("explorer", if show { "on" } else { "off" }));
        }
        self.explorer = show;
        if !self.explorer_visible() {
            self.tree_focus = false;
        }
        self.view.explorer = self.explorer_visible();
        self.relayout();
    }

    /// Ctrl+Shift+E: show the explorer and focus it, or hide it.
    pub(super) fn toggle_explorer(&mut self) {
        if self.folder.is_none() {
            return self.say("Open a folder first (Ctrl+R)");
        }
        if self.explorer_visible() {
            self.set_explorer(false);
        } else {
            self.set_explorer(true);
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

    /// Renames or moves `src` to `target`, following the open file if it moves.
    fn rename_to(&mut self, src: &Path, target: &Path) -> bool {
        if target.exists() {
            self.say(format!(
                "{} already exists",
                workspace::relative(self.folder.as_deref().unwrap_or(Path::new("")), target)
            ));
            return false;
        }
        if let Some(parent) = target.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::rename(src, target) {
            platform::message_box(
                self.hwnd,
                &format!("Could not move {}:\n{e}", name_of(src)),
                APP_NAME,
                MB_OK | MB_ICONERROR,
            );
            return false;
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
        true
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
        if self.rename_to(src, &target) {
            let root = self.folder.clone().unwrap_or_default();
            let to = workspace::relative(&root, dest);
            self.say(format!(
                "Moved {} to {}",
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
        let name = text.trim().replace('/', "\\");
        if name.is_empty() {
            return;
        }
        match kind {
            PromptKind::Rename => {
                let target = path.parent().unwrap_or(Path::new("")).join(&name);
                if target != path && self.rename_to(&path, &target) {
                    self.say(format!("Renamed to {name}"));
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
                let _ = Settings::update(|s| {
                    s.set("explorer.notes_only", if on { "on" } else { "off" })
                });
                let rows = self.tree_rows();
                self.clamp_tree_scroll(rows.len());
                self.say(if on {
                    "Explorer: showing Markdown files only"
                } else {
                    "Explorer: showing all files"
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
