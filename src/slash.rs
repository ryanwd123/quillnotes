//! The `/` command menu: its items, filtering, and the edits they make.

use crate::markdown::{Kind, LineInfo};
use crate::palette::term_match;
use crate::table::Grid;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SlashCmd {
    CodeBlock,
    Table,
    Heading(u8),
    Task,
    Rule,
    /// Today's date plus this many days, as YYYY-MM-DD.
    Date(i64),
}

/// (label, hint shown on the right, extra search words, command). Date items show
/// the date as their hint instead.
pub const ITEMS: &[(&str, &str, &str, SlashCmd)] = &[
    ("Code block", "```", "fence", SlashCmd::CodeBlock),
    ("Table", "| |", "grid", SlashCmd::Table),
    ("Task", "- [ ]", "todo checkbox", SlashCmd::Task),
    ("Heading 1", "#", "h1 title", SlashCmd::Heading(1)),
    ("Heading 2", "##", "h2", SlashCmd::Heading(2)),
    ("Heading 3", "###", "h3", SlashCmd::Heading(3)),
    ("Heading 4", "####", "h4", SlashCmd::Heading(4)),
    ("Heading 5", "#####", "h5", SlashCmd::Heading(5)),
    (
        "Horizontal rule",
        "---",
        "hr line divider separator",
        SlashCmd::Rule,
    ),
    ("Today", "", "date now", SlashCmd::Date(0)),
    ("Yesterday", "", "date", SlashCmd::Date(-1)),
    ("Tomorrow", "", "date", SlashCmd::Date(1)),
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
    /// Bytes before the caret to select (0 for none).
    pub select: usize,
}

/// Inserts `block` (lines without any prefix) for `line`, from which the `/command`
/// was removed. An empty line becomes the block; otherwise the block goes below the
/// line, indented to stay inside a list item or quote, after a blank line if `gap`.
/// `caret` is a (line, byte) position within `block`.
fn insert_block(
    line: &str,
    info: &LineInfo,
    block: &[String],
    caret: (usize, usize),
    select: usize,
    gap: bool,
) -> LineEdit {
    let quote = &line[..info.qend];
    let empty = line[info.content.min(line.len())..].trim().is_empty();
    if empty && !matches!(info.kind, Kind::List { .. }) {
        let text: Vec<String> = block
            .iter()
            .enumerate()
            .map(|(i, l)| {
                if i == 0 {
                    l.clone()
                } else {
                    format!("{quote}{l}")
                }
            })
            .collect();
        return LineEdit {
            start: info.qend,
            end: line.len(),
            text: text.join("\n"),
            caret: (caret.0, quote.len() + caret.1),
            select,
        };
    }
    let indent = match info.kind {
        Kind::List { .. } | Kind::Para => " ".repeat(info.content - info.qend),
        _ => String::new(),
    };
    let pre = format!("{quote}{indent}");
    let mut text = String::new();
    let mut below = 1;
    if gap {
        text.push('\n');
        text.push_str(quote.trim_end());
        below += 1;
    }
    for l in block {
        text.push('\n');
        text.push_str(&pre);
        text.push_str(l);
    }
    LineEdit {
        start: line.len(),
        end: line.len(),
        text,
        caret: (caret.0 + below, pre.len() + caret.1),
        select,
    }
}

/// A fenced code block with the caret on its empty middle line.
pub fn code_block(line: &str, info: &LineInfo) -> LineEdit {
    let block = ["```".to_string(), String::new(), "```".to_string()];
    insert_block(line, info, &block, (1, 0), 0, false)
}

/// A table with a header row and one body row; the first header is selected so
/// typing replaces it.
pub fn table(line: &str, info: &LineInfo) -> LineEdit {
    let first = "Column 1";
    let block = Grid::new("", &[first, "Column 2"]).to_lines();
    insert_block(line, info, &block, (0, 2 + first.len()), first.len(), true)
}

/// A horizontal rule with the caret on a new line below it. Directly under a
/// paragraph a blank line comes first, or `---` would make the paragraph a heading.
pub fn rule(line: &str, info: &LineInfo, after_para: bool) -> LineEdit {
    let mut e = insert_block(line, info, &["---".into(), String::new()], (1, 0), 0, true);
    if after_para && e.start == info.qend {
        e.text
            .insert_str(0, &format!("\n{}", line[..info.qend].trim_end()));
        e.caret.0 += 1;
    }
    e
}

/// Days since 1970-01-01 of a civil date (proleptic Gregorian).
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = y as i64 - (m <= 2) as i64;
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i32, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    ((yoe + era * 400 + (m <= 2) as i64) as i32, m, d)
}

/// `date` moved by `days`, as YYYY-MM-DD.
pub fn date_text(date: (i32, u32, u32), days: i64) -> String {
    let (y, m, d) = civil_from_days(days_from_civil(date.0, date.1, date.2) + days);
    format!("{y:04}-{m:02}-{d:02}")
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
        assert_eq!(labels("").len(), ITEMS.len());
        assert_eq!(labels("hr"), vec!["Horizontal rule"]);
        assert_eq!(labels("date"), vec!["Today", "Yesterday", "Tomorrow"]);
        assert_eq!(labels("tab"), vec!["Table"]);
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
    fn table_edits() {
        let e = table("", &info(""));
        assert_eq!(
            e.text,
            "| Column 1 | Column 2 |
| -------- | -------- |
|          |          |"
        );
        assert_eq!((e.caret, e.select), ((0, 10), 8));
        // After text: a blank line first, so the table is not part of the paragraph.
        let e = table("intro", &info("intro"));
        assert!(e.text.starts_with(
            "

| Column 1"
        ));
        assert_eq!(e.caret, (2, 10));
        let e = table("> ", &info("> "));
        assert_eq!(e.text.lines().nth(1), Some("> | -------- | -------- |"));
        assert_eq!(e.caret, (0, 12));
    }

    #[test]
    fn rules_and_dates() {
        let e = rule("", &info(""), false);
        assert_eq!((e.text.as_str(), e.caret), ("---\n", (1, 0)));
        let e = rule("", &info(""), true);
        assert_eq!((e.text.as_str(), e.caret), ("\n---\n", (2, 0)));
        let e = rule("text", &info("text"), true);
        assert_eq!(
            (e.start, e.text.as_str(), e.caret),
            (4, "\n\n---\n", (3, 0))
        );
        assert_eq!(date_text((2026, 10, 16), 0), "2026-10-16");
        assert_eq!(date_text((2026, 3, 1), -1), "2026-02-28");
        assert_eq!(date_text((2024, 3, 1), -1), "2024-02-29");
        assert_eq!(date_text((2025, 12, 31), 1), "2026-01-01");
        assert_eq!(date_text((2000, 1, 1), -1), "1999-12-31");
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
