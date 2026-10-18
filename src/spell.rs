//! Spell checking: which parts of a line are prose, and mapping the checker's
//! UTF-16 results back to byte ranges. The checker itself is Windows' (platform.rs).

use std::ops::Range;

use crate::markdown::{self, Kind, LineInfo, Style};

/// `line` with everything that is not prose (syntax, code, link targets, URLs, note
/// names, words with digits or path characters) replaced by spaces, keeping every
/// byte offset; None when the line has no prose at all.
pub fn prose(line: &str, info: &LineInfo) -> Option<String> {
    if matches!(
        info.kind,
        Kind::Blank | Kind::Rule | Kind::Fence | Kind::Code | Kind::Comment
    ) || info.content >= line.len()
    {
        return None;
    }
    let n = line.len();
    let mut keep = vec![true; n];
    let blank =
        |keep: &mut [bool], r: &Range<usize>| keep[r.start.min(n)..r.end.min(n)].fill(false);
    blank(&mut keep, &(0..info.content));
    let inl = markdown::parse_inline(line, info.content, n);
    for r in &inl.hidden {
        blank(&mut keep, r);
    }
    for (r, sty) in &inl.spans {
        if matches!(
            sty,
            Style::Code | Style::Image | Style::Command | Style::Comment
        ) {
            blank(&mut keep, r);
        }
    }
    for (r, target, _) in &inl.links {
        let label = &line[r.clone()];
        if target.starts_with("[[") || label.contains("://") || label == target {
            blank(&mut keep, r);
        }
    }
    for (r, _) in &inl.subs {
        blank(&mut keep, r);
    }
    // Words that are names, numbers, paths or addresses rather than prose. A word
    // ends at whitespace or at text already left out.
    let mut words = Vec::new();
    let mut start = None;
    for (i, c) in line.char_indices().chain(std::iter::once((n, ' '))) {
        let gap = c.is_whitespace() || !keep.get(i).copied().unwrap_or(false);
        match (gap, start) {
            (false, None) => start = Some(i),
            (true, Some(s)) => {
                words.push(s..i);
                start = None;
            }
            _ => {}
        }
    }
    for r in words {
        let w = &line[r.clone()];
        let inner_dot = w
            .trim_end_matches(|c: char| c.is_ascii_punctuation())
            .contains('.');
        if inner_dot || w.contains(|c: char| c.is_ascii_digit() || r"_/\@#<>{}=".contains(c)) {
            blank(&mut keep, &r);
        }
    }
    let mut out = String::with_capacity(line.len());
    for (i, c) in line.char_indices() {
        if keep[i] {
            out.push(c);
        } else {
            out.extend(std::iter::repeat_n(' ', c.len_utf8()));
        }
    }
    (!out.trim().is_empty()).then_some(out)
}

/// `text` as null-terminated UTF-16, with the byte offset of each UTF-16 unit (and
/// one past the end).
pub fn utf16(text: &str) -> (Vec<u16>, Vec<usize>) {
    let mut wide = Vec::with_capacity(text.len() + 1);
    let mut map = Vec::with_capacity(text.len() + 1);
    for (i, c) in text.char_indices() {
        let mut buf = [0u16; 2];
        for &u in c.encode_utf16(&mut buf).iter() {
            wide.push(u);
            map.push(i);
        }
    }
    map.push(text.len());
    wide.push(0);
    (wide, map)
}

/// Misspellings, leaving out the word the caret (byte `col`) is in or just after:
/// that one is still being typed.
pub fn without_caret_word(errors: &[Range<usize>], col: Option<usize>) -> Vec<Range<usize>> {
    errors
        .iter()
        .filter(|r| col.is_none_or(|c| c < r.start || c > r.end))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The words left as prose, or None for a line without any.
    fn p(line: &str) -> Option<Vec<String>> {
        let infos = markdown::classify(&[line.to_string()]);
        let text = prose(line, &infos[0])?;
        assert_eq!(text.len(), line.len());
        Some(text.split_whitespace().map(String::from).collect())
    }

    #[test]
    fn keeps_only_prose() {
        assert_eq!(
            p("## Teh `codez` and [a lnk](http://x.y/z) see [[Some Note]]").unwrap(),
            ["Teh", "and", "a", "lnk", "see"]
        );
        assert_eq!(
            p("- visit https://example.com or file.md, v2 and snake_case!").unwrap(),
            ["visit", "or", "and"]
        );
        assert_eq!(p("Is e.g. fine.").unwrap(), ["Is", "fine."]);
        assert_eq!(p("```rust"), None);
        assert_eq!(p("`only code`"), None);
        assert_eq!(p(""), None);
    }

    #[test]
    fn maps_utf16_and_skips_the_caret_word() {
        let (w, map) = utf16("é😀a");
        assert_eq!(w.len(), 5); // é, two units for 😀, a, NUL
        assert_eq!(map, [0, 2, 2, 6, 7]);
        let errs = [0..3, 5..9];
        assert_eq!(without_caret_word(&errs, Some(9)), vec![0..3]);
        assert_eq!(without_caret_word(&errs, Some(4)), [0..3, 5..9]);
        assert_eq!(without_caret_word(&errs, None), [0..3, 5..9]);
    }
}
