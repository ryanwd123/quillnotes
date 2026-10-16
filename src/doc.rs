//! The text model: Markdown source as a list of lines, with grouped undo/redo.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// Versions are unique across all documents, so caches keyed by version (layout,
/// word count) never mistake a newly opened document for the previous one.
static NEXT_VERSION: AtomicU64 = AtomicU64::new(1);

fn next_version() -> u64 {
    NEXT_VERSION.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pos {
    pub line: usize,
    /// Byte offset within the line.
    pub col: usize,
}

impl Pos {
    pub fn new(line: usize, col: usize) -> Self {
        Pos { line, col }
    }
}

/// (anchor, cursor)
pub type Sel = (Pos, Pos);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditKind {
    Typing,
    Delete,
    Other,
}

struct Edit {
    start: Pos,
    removed: String,
    inserted: String,
}

struct Group {
    edits: Vec<Edit>,
    before: Sel,
    after: Sel,
    kind: EditKind,
    time: Instant,
}

pub struct Document {
    pub lines: Vec<String>,
    pub path: Option<PathBuf>,
    pub crlf: bool,
    pub bom: bool,
    /// Bumped on every text change.
    pub version: u64,
    undo: Vec<Group>,
    redo: Vec<Group>,
    open: Option<Group>,
    /// Undo depth at which the document matches the file on disk.
    saved_depth: Option<usize>,
}

/// Position just after `text` inserted at `start`.
pub fn end_of(start: Pos, text: &str) -> Pos {
    match text.rfind('\n') {
        None => Pos::new(start.line, start.col + text.len()),
        Some(i) => Pos::new(start.line + text.matches('\n').count(), text.len() - i - 1),
    }
}

impl Document {
    pub fn new() -> Self {
        Document {
            lines: vec![String::new()],
            path: None,
            crlf: false,
            bom: false,
            version: next_version(),
            undo: Vec::new(),
            redo: Vec::new(),
            open: None,
            saved_depth: Some(0),
        }
    }

    pub fn from_bytes(bytes: &[u8], path: Option<PathBuf>) -> (Self, bool) {
        let bom = bytes.starts_with(&[0xEF, 0xBB, 0xBF]);
        let body = if bom { &bytes[3..] } else { bytes };
        let (text, lossy) = match std::str::from_utf8(body) {
            Ok(s) => (s.to_string(), false),
            Err(_) => (String::from_utf8_lossy(body).into_owned(), true),
        };
        let crlf = text.contains("\r\n");
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        let mut doc = Document::new();
        doc.lines = text.split('\n').map(String::from).collect();
        doc.path = path;
        doc.crlf = crlf;
        doc.bom = bom;
        (doc, lossy)
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if self.bom {
            out.extend_from_slice(&[0xEF, 0xBB, 0xBF]);
        }
        let nl = if self.crlf { "\r\n" } else { "\n" };
        out.extend_from_slice(self.lines.join(nl).as_bytes());
        out
    }

    #[cfg(test)]
    pub fn text(&self) -> String {
        self.lines.join("\n")
    }

    pub fn is_dirty(&self) -> bool {
        self.saved_depth != Some(self.undo.len())
    }

    pub fn mark_saved(&mut self) {
        self.saved_depth = Some(self.undo.len());
    }

    pub fn line_len(&self, line: usize) -> usize {
        self.lines[line].len()
    }

    pub fn end(&self) -> Pos {
        let l = self.lines.len() - 1;
        Pos::new(l, self.lines[l].len())
    }

    /// Clamps a position into the document and onto a char boundary.
    pub fn clamp(&self, p: Pos) -> Pos {
        let line = p.line.min(self.lines.len() - 1);
        let s = &self.lines[line];
        let mut col = p.col.min(s.len());
        while !s.is_char_boundary(col) {
            col -= 1;
        }
        Pos::new(line, col)
    }

    pub fn text_range(&self, a: Pos, b: Pos) -> String {
        if a.line == b.line {
            return self.lines[a.line][a.col..b.col].to_string();
        }
        let mut s = self.lines[a.line][a.col..].to_string();
        for l in a.line + 1..b.line {
            s.push('\n');
            s.push_str(&self.lines[l]);
        }
        s.push('\n');
        s.push_str(&self.lines[b.line][..b.col]);
        s
    }

    fn raw_replace(&mut self, a: Pos, b: Pos, text: &str) -> Pos {
        let head = &self.lines[a.line][..a.col];
        let tail = &self.lines[b.line][b.col..];
        let mut new_lines: Vec<String> = text.split('\n').map(String::from).collect();
        let end = end_of(a, text);
        new_lines[0].insert_str(0, head);
        new_lines.last_mut().unwrap().push_str(tail);
        self.lines.splice(a.line..=b.line, new_lines);
        self.version = next_version();
        end
    }

    /// Starts an undo group. Edits made before [`commit`](Self::commit) undo together.
    pub fn begin(&mut self, kind: EditKind, before: Sel) {
        if self.open.is_none() {
            self.open = Some(Group {
                edits: Vec::new(),
                before,
                after: before,
                kind,
                time: Instant::now(),
            });
        }
    }

    /// Replaces `a..b` with `text` and returns the end of the inserted text.
    pub fn replace(&mut self, a: Pos, b: Pos, text: &str) -> Pos {
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        let removed = self.text_range(a, b);
        if removed == text {
            return end_of(a, text);
        }
        let end = self.raw_replace(a, b, text);
        let edit = Edit {
            start: a,
            removed,
            inserted: text.to_string(),
        };
        match &mut self.open {
            Some(g) => g.edits.push(edit),
            None => {
                self.undo.push(Group {
                    edits: vec![edit],
                    before: (a, b),
                    after: (end, end),
                    kind: EditKind::Other,
                    time: Instant::now(),
                });
                self.redo.clear();
            }
        }
        end
    }

    pub fn commit(&mut self, after: Sel) {
        let Some(mut g) = self.open.take() else {
            return;
        };
        if g.edits.is_empty() {
            return;
        }
        g.after = after;
        if self.saved_depth.is_some_and(|d| d > self.undo.len()) {
            self.saved_depth = None;
        }
        self.redo.clear();
        let can_merge = g.kind != EditKind::Other
            && self.saved_depth != Some(self.undo.len())
            && g.edits.iter().all(|e| !e.inserted.contains('\n'))
            && self.undo.last().is_some_and(|top| {
                top.kind == g.kind
                    && top.after == g.before
                    && g.time.duration_since(top.time) < Duration::from_millis(1500)
            });
        if can_merge {
            let top = self.undo.last_mut().unwrap();
            // Typing groups break at word boundaries.
            let word_break = g.kind == EditKind::Typing
                && g.edits.iter().any(|e| e.inserted.starts_with(' '))
                && top.edits.last().is_some_and(|e| !e.inserted.ends_with(' '));
            if !word_break {
                top.edits.extend(g.edits);
                top.after = g.after;
                top.time = g.time;
                return;
            }
        }
        self.undo.push(g);
    }

    pub fn undo(&mut self) -> Option<Sel> {
        let g = self.undo.pop()?;
        for e in g.edits.iter().rev() {
            let end = end_of(e.start, &e.inserted);
            self.raw_replace(e.start, end, &e.removed);
        }
        let sel = g.before;
        self.redo.push(g);
        Some(sel)
    }

    pub fn redo(&mut self) -> Option<Sel> {
        let g = self.redo.pop()?;
        for e in &g.edits {
            let end = end_of(e.start, &e.removed);
            self.raw_replace(e.start, end, &e.inserted);
        }
        let sel = g.after;
        self.undo.push(g);
        Some(sel)
    }
}

// ---------------------------------------------------------------------------
// Grapheme-ish and word boundaries (no external Unicode tables).

fn is_extend(c: char) -> bool {
    matches!(c as u32,
        0x300..=0x36F | 0x483..=0x489 | 0x591..=0x5BD | 0x610..=0x61A | 0x64B..=0x65F
        | 0x900..=0x903 | 0x93A..=0x94F | 0xE31 | 0xE34..=0xE3A | 0xE47..=0xE4E
        | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x200C | 0x200D | 0x20D0..=0x20FF
        | 0xFE00..=0xFE0F | 0xFE20..=0xFE2F | 0x1F3FB..=0x1F3FF | 0xE0020..=0xE007F | 0xE0100..=0xE01EF)
}

fn is_ri(c: char) -> bool {
    matches!(c as u32, 0x1F1E6..=0x1F1FF)
}

pub fn next_boundary(s: &str, i: usize) -> usize {
    let Some(c) = s[i..].chars().next() else {
        return i;
    };
    let mut j = i + c.len_utf8();
    let mut prev = c;
    if is_ri(c)
        && let Some(n) = s[j..].chars().next().filter(|&n| is_ri(n))
    {
        j += n.len_utf8();
    }
    while let Some(n) = s[j..].chars().next() {
        if is_extend(n) || prev == '\u{200D}' {
            j += n.len_utf8();
            prev = n;
        } else {
            break;
        }
    }
    j
}

pub fn prev_boundary(s: &str, i: usize) -> usize {
    let mut j = i;
    while let Some(c) = s[..j].chars().next_back() {
        j -= c.len_utf8();
        if is_extend(c) {
            continue;
        }
        match s[..j].chars().next_back() {
            Some('\u{200D}') => {
                j -= 3;
                continue;
            }
            Some(p) if is_ri(c) && is_ri(p) => {
                let run = s[..j].chars().rev().take_while(|&x| is_ri(x)).count();
                if run % 2 == 1 {
                    j -= p.len_utf8();
                }
            }
            _ => {}
        }
        break;
    }
    j
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum Class {
    Space,
    Word,
    Other,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' || c == '\'' {
        Class::Word
    } else {
        Class::Other
    }
}

/// Start of the next word (Windows-style Ctrl+Right).
pub fn next_word(s: &str, i: usize) -> usize {
    let mut it = s[i..].char_indices().peekable();
    let Some(&(_, first)) = it.peek() else {
        return i;
    };
    let cls = class(first);
    let mut j = s.len();
    let mut seen_space = false;
    for (k, c) in it {
        let cl = class(c);
        if cl == Class::Space {
            seen_space = true;
            continue;
        }
        if cl != cls || seen_space {
            j = i + k;
            break;
        }
    }
    j
}

/// Start of the current/previous word (Ctrl+Left).
pub fn prev_word(s: &str, i: usize) -> usize {
    let mut j = i;
    let mut chars = s[..i].char_indices().rev().peekable();
    while let Some(&(k, c)) = chars.peek() {
        if class(c) != Class::Space {
            break;
        }
        j = k;
        chars.next();
    }
    let Some(&(_, c0)) = chars.peek() else {
        return j;
    };
    let cls = class(c0);
    for (k, c) in chars {
        if class(c) != cls {
            break;
        }
        j = k;
    }
    j
}

/// Byte range of the word (or run of same-class characters) around `i`.
pub fn word_at(s: &str, i: usize) -> (usize, usize) {
    let c = s[i..].chars().next().or_else(|| s[..i].chars().next_back());
    let Some(c) = c else { return (i, i) };
    let cls = class(c);
    let mut a = i;
    for (k, ch) in s[..i].char_indices().rev() {
        if class(ch) != cls {
            break;
        }
        a = k;
    }
    let mut b = i;
    for (k, ch) in s[i..].char_indices() {
        if class(ch) != cls {
            break;
        }
        b = i + k + ch.len_utf8();
    }
    (a, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_and_undo() {
        let (mut d, _) = Document::from_bytes(b"hello\r\nworld", None);
        assert!(d.crlf);
        let s0 = (Pos::new(0, 0), Pos::new(0, 0));
        d.begin(EditKind::Other, s0);
        let e = d.replace(Pos::new(0, 5), Pos::new(1, 0), " big\nnew ");
        d.commit((e, e));
        assert_eq!(d.lines, vec!["hello big", "new world"]);
        assert_eq!(e, Pos::new(1, 4));
        assert!(d.is_dirty());
        d.undo();
        assert_eq!(d.lines, vec!["hello", "world"]);
        assert!(!d.is_dirty());
        d.redo();
        assert_eq!(d.text(), "hello big\nnew world");
        assert_eq!(d.to_bytes(), b"hello big\r\nnew world");
    }

    #[test]
    fn typing_merges() {
        let mut d = Document::new();
        let mut p = Pos::new(0, 0);
        for c in ["a", "b", " ", "c"] {
            d.begin(EditKind::Typing, (p, p));
            let e = d.replace(p, p, c);
            d.commit((e, e));
            p = e;
        }
        assert_eq!(d.text(), "ab c");
        d.undo();
        assert_eq!(d.text(), "ab");
        d.undo();
        assert_eq!(d.text(), "");
    }

    #[test]
    fn boundaries() {
        let s = "a👍🏽b👨‍👩‍👧c🇺🇸";
        let b1 = next_boundary(s, 1);
        assert_eq!(&s[1..b1], "👍🏽");
        let b2 = next_boundary(s, b1 + 1);
        assert_eq!(&s[b1 + 1..b2], "👨‍👩‍👧");
        assert_eq!(prev_boundary(s, b2), b1 + 1);
        assert_eq!(prev_boundary(s, s.len()), s.len() - 8);
        assert_eq!(next_word("foo bar", 0), 4);
        assert_eq!(prev_word("foo bar", 7), 4);
        assert_eq!(prev_word("foo bar ", 8), 4);
        assert_eq!(word_at("foo bar", 5), (4, 7));
    }
}
