//! The `/` command menu: its items, filtering, and the edits they make.

use crate::markdown::{Kind, LineInfo};
use crate::palette::term_match;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SlashCmd {
    CodeBlock,
    Heading(u8),
}

/// (label, hint shown on the right, extra search words, command)
pub const ITEMS: [(&str, &str, &str, SlashCmd); 6] = [
    ("Code block", "```", "fence", SlashCmd::CodeBlock),
    ("Heading 1", "#", "h1 title", SlashCmd::Heading(1)),
    ("Heading 2", "##", "h2", SlashCmd::Heading(2)),
    ("Heading 3", "###", "h3", SlashCmd::Heading(3)),
    ("Heading 4", "####", "h4", SlashCmd::Heading(4)),
    ("Heading 5", "#####", "h5", SlashCmd::Heading(5)),
];

/// Items matching `query` (the text typed after `/`), best first, with the matched
/// byte offsets inside each label.
pub fn filter(query: &str) -> Vec<(usize, Vec<usize>)> {
    let mut found: Vec<(i32, usize, Vec<usize>)> = ITEMS
        .iter()
        .enumerate()
        .filter_map(|(i, &(label, _, words, _))| {
            // Search the label plus its extra words; highlight only within the label.
            let (score, hits) = term_match(query, &format!("{label} {words}"))?;
            Some((
                score,
                i,
                hits.into_iter().filter(|&b| b < label.len()).collect(),
            ))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    found.into_iter().map(|(_, i, hits)| (i, hits)).collect()
}

/// A single-line replacement and where the caret ends up.
#[derive(PartialEq, Eq, Debug)]
pub struct LineEdit {
    pub start: usize,
    pub end: usize,
    pub text: String,
    /// (lines below the edited line, byte column)
    pub caret: (usize, usize),
}

/// Inserts a fenced code block for `line` (from which the `/command` was removed).
/// An empty line becomes the block; otherwise the block goes below the line, indented
/// to stay inside a list item or quote.
pub fn code_block(line: &str, info: &LineInfo) -> LineEdit {
    let quote = &line[..info.qend];
    let empty = line[info.content.min(line.len())..].trim().is_empty();
    if empty && !matches!(info.kind, Kind::List { .. }) {
        return LineEdit {
            start: info.qend,
            end: line.len(),
            text: format!("```\n{quote}\n{quote}```"),
            caret: (1, quote.len()),
        };
    }
    let indent = match info.kind {
        Kind::List { .. } | Kind::Para => " ".repeat(info.content - info.qend),
        _ => String::new(),
    };
    let pre = format!("{quote}{indent}");
    LineEdit {
        start: line.len(),
        end: line.len(),
        text: format!("\n{pre}```\n{pre}\n{pre}```"),
        caret: (2, pre.len()),
    }
}

/// Whether a `/` typed at byte `col` should open the menu: at the start of the
/// line's content or after whitespace, and not inside code.
pub fn can_open(line: &str, info: &LineInfo, col: usize) -> bool {
    if matches!(info.kind, Kind::Code | Kind::Fence) {
        return false;
    }
    col <= info.content || line[..col].ends_with(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markdown::classify;

    fn info(line: &str) -> LineInfo {
        classify(&[line.to_string()])[0]
    }

    #[test]
    fn filtering() {
        let labels = |q: &str| {
            filter(q)
                .iter()
                .map(|(i, _)| ITEMS[*i].0)
                .collect::<Vec<_>>()
        };
        assert_eq!(labels("").len(), 6);
        assert_eq!(labels("code"), vec!["Code block"]);
        assert_eq!(labels("h2"), vec!["Heading 2"]);
        assert_eq!(labels("head 3"), vec!["Heading 3"]);
        assert!(labels("xyz").is_empty());
        // Hits from the extra search words are not highlighted in the label.
        assert_eq!(filter("h1")[0].1, Vec::<usize>::new());
        assert_eq!(filter("co")[0].1, vec![0, 1]);
    }

    #[test]
    fn code_block_edits() {
        let e = code_block("", &info(""));
        assert_eq!(
            (e.start, e.end, e.text.as_str(), e.caret),
            (0, 0, "```\n\n```", (1, 0))
        );
        let e = code_block("> ", &info("> "));
        assert_eq!(e.text, "```\n> \n> ```");
        assert_eq!(e.caret, (1, 2));
        let e = code_block("some text ", &info("some text "));
        assert_eq!(
            (e.start, e.text.as_str(), e.caret),
            (10, "\n```\n\n```", (2, 0))
        );
        // Inside a list item the block is indented to the item's content.
        let e = code_block("- item", &info("- item"));
        assert_eq!(e.text, "\n  ```\n  \n  ```");
        assert_eq!(e.caret, (2, 2));
    }

    #[test]
    fn opening() {
        assert!(can_open("/", &info("/"), 0));
        assert!(can_open("word /", &info("word /"), 5));
        assert!(can_open("- /", &info("- /"), 2));
        assert!(!can_open("a/b", &info("a/b"), 1));
        assert!(!can_open("https://x", &info("https://x"), 6));
    }
}
