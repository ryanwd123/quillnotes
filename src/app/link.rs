//! The link editor (Ctrl+K): a popup with a link's text and URL, for changing the
//! link at the caret or inserting a new one.

use std::ops::Range;

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::UI::Input::KeyboardAndMouse::*;

use super::App;
use crate::doc::{self, EditKind, Pos};
use crate::markdown::{self, Kind};
use crate::platform;
use crate::render::{LinkView, ViewMode};

pub(super) struct LinkEdit {
    line: usize,
    /// The source the new link replaces: the link being edited, or the selection.
    range: Range<usize>,
    /// The document version when it opened; after an edit `range` may be stale.
    version: u64,
    text: String,
    url: String,
    /// Kept from the link being edited.
    title: String,
    bare: bool,
    editing: bool,
    /// The field with the keyboard (true for the URL) and its caret.
    in_url: bool,
    cur: usize,
}

impl LinkEdit {
    fn field(&mut self) -> (&mut String, &mut usize) {
        let f = if self.in_url {
            &mut self.url
        } else {
            &mut self.text
        };
        (f, &mut self.cur)
    }

    /// The editor for painting.
    pub(super) fn view(&self) -> LinkView<'_> {
        LinkView {
            at: Pos::new(self.line, self.range.start),
            text: &self.text,
            url: &self.url,
            caret: (self.in_url, self.cur),
        }
    }

    fn focus(&mut self, url: bool) {
        self.in_url = url;
        self.cur = if url { self.url.len() } else { self.text.len() };
    }
}

fn contains(r: &D2D_RECT_F, x: f32, y: f32) -> bool {
    x >= r.left && x < r.right && y >= r.top && y < r.bottom
}

fn is_url(s: &str) -> bool {
    ["http://", "https://", "mailto:", "www."]
        .iter()
        .any(|p| s.starts_with(p))
        && !s.contains(char::is_whitespace)
}

impl App {
    /// Ctrl+K: edits the link at the caret in the link editor. Elsewhere, Live 2
    /// opens it to insert a link (the selection as its text, or its URL), and the
    /// other modes wrap the selection in link syntax.
    pub(super) fn link_command(&mut self) {
        let (a, b) = self.sel();
        if a.line != b.line {
            return;
        }
        let info = self.info(a.line);
        let line = &self.doc.lines[a.line];
        let found = if matches!(info.kind, Kind::Code | Kind::Fence | Kind::Comment) {
            None
        } else {
            let inl = markdown::parse_inline(line, info.content.min(line.len()), line.len());
            markdown::link_at(line, &inl, a.col).filter(|l| b.col <= l.range.end)
        };
        let mut edit = LinkEdit {
            line: a.line,
            range: a.col..b.col,
            version: self.doc.version,
            text: String::new(),
            url: String::new(),
            title: String::new(),
            bare: false,
            editing: false,
            in_url: false,
            cur: 0,
        };
        if let Some(l) = found {
            edit.text = line[l.text].to_string();
            edit.range = l.range;
            edit.url = l.target;
            edit.title = l.title;
            edit.bare = l.bare;
            edit.editing = true;
            edit.focus(true);
        } else if self.mode == ViewMode::Live2 {
            let sel = self.doc.text_range(a, b);
            if is_url(&sel) {
                edit.url = sel;
            } else {
                edit.text = sel;
            }
            edit.focus(!edit.text.is_empty());
        } else {
            return self.make_link();
        }
        self.slash = None;
        self.link_edit = Some(edit);
        self.invalidate();
    }

    /// A key while the link editor is open, which takes them all.
    pub(super) fn link_key(&mut self, key: VIRTUAL_KEY, ctrl: bool) {
        let paste = (ctrl && key == VK_V)
            .then(|| platform::get_clipboard(self.hwnd))
            .flatten();
        let Some(e) = &mut self.link_edit else { return };
        match key {
            VK_ESCAPE => self.link_edit = None,
            VK_RETURN => self.apply_link(),
            // With two fields, Tab and Shift+Tab both go to the other one.
            VK_TAB => e.focus(!e.in_url),
            VK_UP => e.focus(false),
            VK_DOWN => e.focus(true),
            _ => {
                let (f, cur) = e.field();
                match key {
                    VK_BACK if *cur > 0 => {
                        let s = if ctrl {
                            doc::prev_word(f, *cur)
                        } else {
                            doc::prev_boundary(f, *cur)
                        };
                        f.replace_range(s..*cur, "");
                        *cur = s;
                    }
                    VK_DELETE if *cur < f.len() => {
                        let e = if ctrl {
                            doc::next_word(f, *cur)
                        } else {
                            doc::next_boundary(f, *cur)
                        };
                        f.replace_range(*cur..e, "");
                    }
                    VK_LEFT if ctrl => *cur = doc::prev_word(f, *cur),
                    VK_LEFT => *cur = doc::prev_boundary(f, *cur),
                    VK_RIGHT if ctrl => *cur = doc::next_word(f, *cur),
                    VK_RIGHT => *cur = doc::next_boundary(f, *cur),
                    VK_HOME => *cur = 0,
                    VK_END => *cur = f.len(),
                    VK_V => {
                        let t = paste.as_deref().unwrap_or("");
                        let t = t.lines().next().unwrap_or("").trim();
                        f.insert_str(*cur, t);
                        *cur += t.len();
                    }
                    _ => {}
                }
            }
        }
        self.invalidate();
    }

    /// Text typed while the link editor is open.
    pub(super) fn link_char(&mut self, s: &str) {
        if let Some(e) = &mut self.link_edit {
            let (f, cur) = e.field();
            f.insert_str(*cur, s);
            *cur += s.len();
            self.invalidate();
        }
    }

    /// A click while the link editor is open: a field takes the keyboard, and a
    /// click off the editor closes it. Returns true when the click was on it.
    pub(super) fn link_click(&mut self, x: f32, y: f32) -> bool {
        if self.link_edit.is_none() {
            return false;
        }
        self.invalidate();
        let geom = self.view.link_geom;
        let e = self.link_edit.as_mut().unwrap();
        match geom {
            Some(g) if contains(&g.panel, x, y) => {
                if contains(&g.text, x, y) || contains(&g.url, x, y) {
                    e.focus(contains(&g.url, x, y));
                }
                true
            }
            _ => {
                self.link_edit = None;
                false
            }
        }
    }

    /// Writes the link, as one undo step. An empty URL leaves just the text, so
    /// clearing it removes the link.
    fn apply_link(&mut self) {
        let Some(e) = self.link_edit.take() else {
            return;
        };
        if e.version != self.doc.version || e.line >= self.doc.lines.len() {
            return self.say("The note changed, so the link was left as it was");
        }
        let url = e.url.trim();
        let text = if url.is_empty() {
            e.text.clone()
        } else {
            markdown::link_markdown(&e.text, url, &e.title, e.bare)
        };
        if text.is_empty() && e.range.is_empty() {
            return;
        }
        self.begin(EditKind::Other);
        let a = Pos::new(e.line, e.range.start);
        let end = self.doc.replace(a, Pos::new(e.line, e.range.end), &text);
        self.set_sel(end, end);
        self.commit();
        if url.is_empty() && e.editing {
            self.say("Removed the link");
        }
    }

    /// Right-click on a link: copies where it goes. Returns false off a link.
    pub(super) fn copy_link(&mut self, target: Option<String>) -> bool {
        let Some(target) = target else {
            return false;
        };
        platform::set_clipboard(self.hwnd, &target);
        self.say("Copied the link");
        true
    }
}

#[cfg(test)]
mod tests {
    use super::super::tests::app;
    use super::*;

    fn at(a: &mut App, col: usize) {
        a.set_sel(Pos::new(0, col), Pos::new(0, col));
    }

    #[test]
    fn edits_the_link_at_the_caret() {
        let mut a = app("see [docs](https://a.b) now");
        at(&mut a, 7);
        a.link_command();
        let e = a.link_edit.as_ref().unwrap();
        assert_eq!((e.text.as_str(), e.url.as_str()), ("docs", "https://a.b"));
        assert!(e.in_url);
        for _ in 0..3 {
            a.link_key(VK_BACK, false);
        }
        a.link_char("x.y");
        a.link_key(VK_TAB, false);
        a.link_char("!");
        a.link_key(VK_RETURN, false);
        assert!(a.link_edit.is_none());
        assert_eq!(a.doc.lines, ["see [docs!](https://x.y) now"]);
        assert_eq!(a.cursor, Pos::new(0, 24));
        a.undo(true);
        assert_eq!(a.doc.lines, ["see [docs](https://a.b) now"]);
    }

    #[test]
    fn a_wiki_link_edits_as_its_note() {
        let mut a = app("[[Note|alias]]");
        at(&mut a, 14);
        a.link_command();
        a.link_key(VK_UP, false);
        a.link_key(VK_BACK, true);
        a.link_key(VK_RETURN, false);
        assert_eq!(a.doc.lines, ["[[Note]]"]);
    }

    #[test]
    fn clearing_the_url_removes_the_link() {
        let mut a = app("a [b](u) c");
        at(&mut a, 4);
        a.link_command();
        a.link_key(VK_BACK, false);
        a.link_key(VK_RETURN, false);
        assert_eq!(a.doc.lines, ["a b c"]);
    }

    #[test]
    fn live2_inserts_a_link() {
        let mut a = app("word here");
        a.set_sel(Pos::new(0, 0), Pos::new(0, 4));
        a.link_command();
        assert!(a.link_edit.as_ref().unwrap().in_url);
        a.link_char("https://x");
        a.link_key(VK_RETURN, false);
        assert_eq!(a.doc.lines, ["[word](https://x) here"]);
        assert_eq!(a.cursor, Pos::new(0, 17));

        // With nothing selected, the text comes first; Escape leaves the note alone.
        let mut a = app("ab");
        at(&mut a, 1);
        a.link_command();
        assert!(!a.link_edit.as_ref().unwrap().in_url);
        a.link_key(VK_RETURN, false);
        assert_eq!(a.doc.lines, ["ab"]);
        a.link_command();
        a.link_char("t");
        a.link_key(VK_TAB, false);
        a.link_char("[[Note]]");
        a.link_key(VK_ESCAPE, false);
        assert_eq!(a.doc.lines, ["ab"]);
        a.link_command();
        a.link_char("t");
        a.link_key(VK_TAB, false);
        a.link_char("[[Note]]");
        a.link_key(VK_RETURN, false);
        assert_eq!(a.doc.lines, ["a[[Note|t]]b"]);
    }

    #[test]
    fn live_mode_still_wraps_the_selection() {
        let mut a = app("x");
        a.mode = ViewMode::Live;
        a.set_sel(Pos::new(0, 0), Pos::new(0, 1));
        a.link_command();
        assert!(a.link_edit.is_none());
        assert_eq!(a.doc.lines, ["[x]()"]);
    }
}
