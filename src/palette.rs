//! Command palette state: the query, fuzzy filtering, and selection.

use crate::doc;
use crate::render::PALETTE_ROWS;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    ChangeFont,
}

/// Every command, in the order shown for an empty query: (label, shortcut hint, command).
const COMMANDS: &[(&str, &str, Command)] = &[("Font: Change Editor Font", "", Command::ChangeFont)];

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Target {
    Command(Command),
    Font(String),
}

pub struct Item {
    pub label: String,
    pub detail: String,
    pub target: Target,
    /// For fonts: whether to draw the label in the font itself.
    pub preview: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Commands,
    Fonts,
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

/// Case-insensitive subsequence match. Returns a score (higher is better) and the
/// byte offsets of the matched characters in `label`.
pub fn fuzzy(query: &str, label: &str) -> Option<(i32, Vec<usize>)> {
    let q: Vec<char> = query
        .chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect();
    if q.is_empty() {
        return Some((0, Vec::new()));
    }
    let chars: Vec<(usize, char)> = label.char_indices().collect();
    let folded: Vec<char> = chars
        .iter()
        .map(|&(_, c)| c.to_lowercase().next().unwrap_or(c))
        .collect();
    let word_start = |k: usize| {
        k == 0 || {
            let (p, c) = (chars[k - 1].1, chars[k].1);
            !p.is_alphanumeric() || (p.is_lowercase() && c.is_uppercase())
        }
    };
    let can_finish = |from: usize, qi: usize| {
        let mut it = folded[from.min(folded.len())..].iter();
        q[qi..].iter().all(|c| it.any(|f| f == c))
    };
    // Prefer word starts and runs, as long as the rest of the query still matches.
    let mut picks: Vec<usize> = Vec::with_capacity(q.len());
    let mut from = 0;
    for (qi, &qc) in q.iter().enumerate() {
        let mut cands = (from..folded.len()).filter(|&k| folded[k] == qc);
        let first = cands.next()?;
        let preferred = std::iter::once(first).chain(cands).find(|&k| {
            (picks.last() == Some(&(k.wrapping_sub(1))) || word_start(k))
                && can_finish(k + 1, qi + 1)
        });
        let k = preferred.unwrap_or(first);
        picks.push(k);
        from = k + 1;
    }
    let mut score = 0i32;
    for (n, &k) in picks.iter().enumerate() {
        score += 1 + if word_start(k) { 8 } else { 0 };
        if n > 0 {
            let gap = k - picks[n - 1];
            score += if gap == 1 { 5 } else { -(gap as i32).min(5) };
        }
    }
    let hits = picks.iter().map(|&k| chars[k].0).collect();
    let lower = label.to_lowercase();
    let qs: String = q.iter().collect();
    if lower.starts_with(&qs) {
        score += 20;
    } else if lower.contains(&qs) {
        score += 10;
    }
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
        let items = COMMANDS
            .iter()
            .map(|&(label, detail, cmd)| Item {
                label: label.into(),
                detail: detail.into(),
                target: Target::Command(cmd),
                preview: false,
            })
            .collect();
        Palette::new(Mode::Commands, items)
    }

    /// A picker over `families` (name, can draw its own name), starting on `current`.
    pub fn fonts(families: Vec<(String, bool)>, current: &str) -> Self {
        let items = families
            .into_iter()
            .map(|(name, preview)| Item {
                detail: match (name == current, preview) {
                    (true, _) => "Current".into(),
                    (false, false) => "Symbols".into(),
                    _ => String::new(),
                },
                target: Target::Font(name.clone()),
                label: name,
                preview,
            })
            .collect();
        let mut p = Palette::new(Mode::Fonts, items);
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

    pub fn placeholder(&self) -> &'static str {
        match self.mode {
            Mode::Commands => "Type a command",
            Mode::Fonts => "Choose the editor font",
        }
    }

    pub fn empty_text(&self) -> &'static str {
        match self.mode {
            Mode::Commands => "No matching commands",
            Mode::Fonts => "No matching fonts",
        }
    }

    fn refilter(&mut self) {
        let mut scored: Vec<(i32, usize, Vec<usize>)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| fuzzy(&self.query, &it.label).map(|(s, m)| (s, i, m)))
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
    fn fuzzy_matching() {
        assert!(fuzzy("xyz", "Font: Change Editor Font").is_none());
        let (_, hits) = fuzzy("cef", "Font: Change Editor Font").unwrap();
        assert_eq!(hits, vec![6, 13, 20]);
        // Word starts and prefixes beat scattered matches.
        assert!(fuzzy("con", "Consolas").unwrap().0 > fuzzy("con", "Cambria Condensed").unwrap().0);
        assert!(
            fuzzy("seg", "Segoe UI").unwrap().0
                > fuzzy("seg", "Lucida Sans Unicode Edge").map_or(-99, |m| m.0)
        );
        assert_eq!(fuzzy("", "anything").unwrap().1, Vec::<usize>::new());
    }

    #[test]
    fn font_picker_filters_and_selects() {
        let names = ["Arial", "Cascadia Mono", "Consolas", "Georgia", "Segoe UI"]
            .iter()
            .map(|s| (s.to_string(), true))
            .collect();
        let mut p = Palette::fonts(names, "Georgia");
        assert_eq!(p.selected().unwrap().label, "Georgia");
        assert_eq!(p.selected().unwrap().detail, "Current");
        p.insert("co");
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
    fn selection_scrolls() {
        let names = (0..30).map(|i| (format!("Font {i:02}"), true)).collect();
        let mut p = Palette::fonts(names, "Font 00");
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
