//! Fast parser fuzzing: random Markdown documents, built from the syntax the
//! parsers care about, are run through block classification, inline parsing, caret
//! and delete helpers, spell-check prose extraction, code highlighting, table
//! parsing and HTML export. Besides not panicking, every range they return must lie
//! on char boundaries inside its line, and tables must survive a round trip.
//!
//! A failing document is shrunk to a small one and printed with its seed.
//! Reproduce a failure, or run longer, with (PowerShell):
//!
//! ```text
//! $env:QUILL_FUZZ_SEED = 1234; $env:QUILL_FUZZ_DOCS = 200000
//! cargo test fuzz -- --nocapture
//! ```
//!
//! A plain `cargo test` runs a fixed seed. `QUILL_FUZZ_RUNS = 8` runs that many
//! seeds taken from the clock.

use std::ops::Range;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::doc::{self, Document};
use crate::highlight::{self, State};
use crate::markdown::{self, Inline, Kind};
use crate::table::Grid;
use crate::{html, slash, spell};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
    fn pick<'a>(&mut self, xs: &[&'a str]) -> &'a str {
        xs[self.below(xs.len())]
    }
}

/// How a line may start.
const PREFIXES: &[&str] = &[
    "", "", "", "", "> ", ">", "> > ", ">>", "- ", "* ", "+ ", "-", "1. ", "2) ", "10. ",
    "999999999999. ", "- [ ] ", "- [x] ", "* [X] ", "-[ ]", "  ", "   ", "    ", "\t", "  - ",
    "    - ", "\t- ", "# ", "## ", "###### ", "####### ", "#", "```", "```rust", "``` js ",
    "~~~", "~~~python", "````", "---", "***", "___", "- - -", "|", "| ", "<!--", "-->", "+++",
    "> - ", "> 1. ", "> ```", "- > ", "- # ", "=", "===", "<b>",
];

/// What goes in a line.
const TOKENS: &[&str] = &[
    "a", "b", "word", "Word", " ", " ", " ", "  ", "\t", "*", "**", "***", "_", "__", "`", "``",
    "```", "~", "~~", "=", "==", "[", "]", "(", ")", "[[", "]]", "![", "](", "<", ">", "</",
    "&amp;", "&lt;", "&#", "&#x", "&#128512;", "&#xFFFFFFFF;", "&nbsp", ";", "\\", "\\*",
    "\\|", "http://a.b/c", "https://x.y?q=1&r=2", "www.z.com", "mailto:a@b.c", "|", "|", "-",
    "--", ":", ":-", "-:", ":-:", "#", "!", "/", "/*", "*/", "//", "\"", "'", "{", "}", "$",
    "<!--", "-->", "<b>", "</b>", "<br>", "1.", "2)", "+", "\u{e9}", "e\u{301}", "\u{65e5}\u{672c}",
    "\u{1F600}", "\u{1F600}\u{200D}\u{1F4BB}", "\u{200D}", "\u{FEFF}", "\u{645}\u{631}",
    "\u{301}", "\u{FFFD}", "\r", "\u{0}", "\u{2028}", "x\u{308}", "fn", "let", "def", "SELECT",
    "\"str\"", "'c'", "r#\"", "0x1F", "1e10", "@", "^", "[^1]", "[x]", "[ ]", "{{", "}}",
];

fn line(rng: &mut Rng) -> String {
    let mut s = rng.pick(PREFIXES).to_string();
    for _ in 0..rng.below(14) {
        s.push_str(rng.pick(TOKENS));
    }
    s
}

fn document(rng: &mut Rng) -> Vec<String> {
    let n = 1 + rng.below(24);
    let mut lines: Vec<String> = Vec::new();
    while lines.len() < n {
        match rng.below(10) {
            // A table: header, delimiter row, body rows, maybe ragged or quoted.
            0 => {
                let q = rng.pick(&["", "> ", "- ", "  "]);
                let cols = 1 + rng.below(4);
                let row = |rng: &mut Rng, cells: usize| {
                    let mut s = format!("{q}|");
                    for _ in 0..cells {
                        for _ in 0..rng.below(3) {
                            s.push_str(rng.pick(TOKENS));
                        }
                        s.push('|');
                    }
                    s
                };
                lines.push(row(rng, cols));
                let mut d = q.to_string();
                for _ in 0..cols {
                    d.push('|');
                    d.push_str(rng.pick(&["---", ":-", "-:", ":-:", "-", "", "--x"]));
                }
                lines.push(d);
                for _ in 0..rng.below(4) {
                    let cells = (cols + rng.below(3)).saturating_sub(1);
                    lines.push(row(rng, cells));
                }
            }
            // A fenced code block in some language, sometimes left open.
            1 => {
                let fence = rng.pick(&["```", "~~~", "````"]);
                let lang = rng.pick(&[
                    "", "rust", "python", "js", "c", "go", "sh", "powershell", "json", "toml",
                    "sql", "html", "pq", "nope",
                ]);
                lines.push(format!("{fence}{lang}"));
                for _ in 0..rng.below(6) {
                    lines.push(line(rng));
                }
                if rng.below(4) != 0 {
                    lines.push(fence.to_string());
                }
            }
            _ => lines.push(line(rng)),
        }
    }
    lines
}

/// Every char boundary of `s`, end included.
fn boundaries(s: &str) -> Vec<usize> {
    s.char_indices().map(|(i, _)| i).chain([s.len()]).collect()
}

fn check_range(what: &str, s: &str, r: &Range<usize>, within: &Range<usize>) {
    assert!(
        within.start <= r.start && r.start <= r.end && r.end <= within.end,
        "{what} {r:?} is outside {within:?} of {s:?}"
    );
    assert!(
        s.is_char_boundary(r.start) && s.is_char_boundary(r.end),
        "{what} {r:?} splits a char in {s:?}"
    );
}

fn check_col(what: &str, s: &str, c: usize) {
    assert!(
        c <= s.len() && s.is_char_boundary(c),
        "{what} {c} is not a char boundary of {s:?}"
    );
}

fn check_inline(s: &str, start: usize, end: usize) -> Inline {
    let inl = markdown::parse_inline(s, start, end);
    let within = start..end;
    for r in &inl.hidden {
        check_range("hidden", s, r, &within);
    }
    // Hidden ranges are walked in order (plain_text, html, the renderer).
    for w in inl.hidden.windows(2) {
        assert!(
            w[0].start <= w[1].start,
            "hidden ranges out of order {:?} in {s:?}",
            inl.hidden
        );
    }
    for (r, _) in &inl.spans {
        check_range("span", s, r, &within);
    }
    for (r, _, _) in &inl.links {
        check_range("link label", s, r, &within);
    }
    for (r, _) in &inl.images {
        check_range("image", s, r, &within);
    }
    for (r, _) in &inl.subs {
        check_range("sub", s, r, &within);
    }
    inl
}

/// Everything the editor does with one line of prose.
fn check_line(s: &str, info: &markdown::LineInfo) {
    for (what, c) in [("qend", info.qend), ("marker", info.marker), ("content", info.content)] {
        check_col(what, s, c.min(s.len()));
    }
    let start = info.content.min(s.len());
    let inl = check_inline(s, start, s.len());
    let _ = markdown::plain_text(s, start);
    let _ = markdown::count_words(&s[start..]);
    let runs = markdown::caret_runs(&inl);
    for r in &runs {
        check_range("caret run", s, r, &(0..s.len()));
    }
    for c in boundaries(s) {
        for fwd in [false, true] {
            check_col("skip_hidden", s, markdown::skip_hidden(&inl, c, fwd));
            check_col("snap_col", s, markdown::snap_col(&runs, c, fwd));
            if c >= start {
                let d = markdown::visible_delete(s, &inl, c, fwd);
                check_range("visible_delete", s, &d, &(0..s.len()));
                let d = markdown::delete_range(&inl, d);
                check_range("delete_range", s, &d, &(0..s.len()));
            }
        }
        if let Some(l) = markdown::link_at(s, &inl, c) {
            check_range("link_at", s, &l.range, &(0..s.len()));
            check_range("link_at text", s, &l.text, &l.range);
        }
        let _ = slash::can_open(s, info, c);
        check_col("next_boundary", s, doc::next_boundary(s, c));
        check_col("prev_boundary", s, doc::prev_boundary(s, c));
        check_col("next_word", s, doc::next_word(s, c));
        check_col("prev_word", s, doc::prev_word(s, c));
        let (a, b) = doc::word_at(s, c);
        check_range("word_at", s, &(a..b), &(0..s.len()));
    }
    if let Some(p) = spell::prose(s, info) {
        assert_eq!(p.len(), s.len(), "prose changed byte offsets of {s:?}");
    }
    let _ = slash::code_block(s, info);
    let _ = slash::table(s, info);
    let _ = slash::rule(s, info, true);
    let _ = html::inline(s, start, s.len(), None);
}

/// Runs every check on one document.
fn check_doc(lines: &[String]) {
    let infos = markdown::classify(lines);
    assert_eq!(infos.len(), lines.len(), "one info per line");
    let mut lang: Option<&'static highlight::Lang> = None;
    let mut state = State::default();
    let mut l = 0;
    while l < lines.len() {
        let (s, info) = (&lines[l], &infos[l]);
        match info.kind {
            Kind::Fence => {
                let fence_info = s[info.content.min(s.len())..].trim_start_matches(['`', '~']);
                lang = highlight::lang(fence_info.trim());
                state = State::default();
            }
            Kind::Code => {
                if let Some(lang) = lang {
                    let spans = highlight::spans(lang, s, &mut state);
                    for (r, _) in &spans {
                        check_range("code token", s, r, &(0..s.len()));
                    }
                    for w in spans.windows(2) {
                        assert!(w[0].0.end <= w[1].0.start, "tokens overlap in {s:?}");
                    }
                }
            }
            Kind::Table { .. } => {
                for r in markdown::split_cells(s, info.qend.min(s.len())) {
                    check_range("cell", s, &r, &(0..s.len()));
                    check_inline(s, r.start, r.end);
                }
                // A whole table parses, prints, and parses back to the same grid.
                let first = l;
                while l + 1 < lines.len() && infos[l + 1].block == info.block && info.block != 0 {
                    l += 1;
                }
                let rows: Vec<&str> = lines[first..=l].iter().map(|s| s.as_str()).collect();
                let grid = Grid::parse(&rows, info.qend);
                let printed = grid.to_lines();
                let again: Vec<&str> = printed.iter().map(|s| s.as_str()).collect();
                let qend = markdown::classify(&printed)[0].qend;
                assert_eq!(
                    Grid::parse(&again, qend),
                    grid,
                    "table changed when printed: {rows:?} -> {printed:?}"
                );
                let mut g = grid.clone();
                g.insert_row(g.rows.len() / 2);
                g.insert_col(g.cols() / 2);
                g.delete_row(1);
                g.delete_col(0);
                let _ = g.to_lines();
                l += 1;
                continue;
            }
            _ => check_line(s, info),
        }
        l += 1;
    }
    let _ = html::document(lines, None);
    // Saving and reopening the text gives the same lines.
    let d = Document::from_bytes(lines.join("\n").as_bytes(), None).0;
    let again = Document::from_bytes(&d.to_bytes(), None).0;
    assert_eq!(again.lines, d.lines, "save and reopen changed the text");
}

fn fails(lines: &[String]) -> bool {
    catch_unwind(AssertUnwindSafe(|| check_doc(lines))).is_err()
}

/// A smaller document that still fails: whole lines dropped, then tokens of text.
fn shrink(mut lines: Vec<String>) -> Vec<String> {
    loop {
        let before = lines.clone();
        let mut i = 0;
        while i < lines.len() && lines.len() > 1 {
            let mut fewer = lines.clone();
            fewer.remove(i);
            if fails(&fewer) {
                lines = fewer;
            } else {
                i += 1;
            }
        }
        for l in 0..lines.len() {
            let mut cut = lines[l].chars().count();
            while cut > 0 {
                let chars: Vec<char> = lines[l].chars().collect();
                let mut at = 0;
                while at < chars.len() {
                    let n = cut.min(chars.len() - at);
                    let mut shorter = lines.clone();
                    shorter[l] = chars[..at].iter().chain(&chars[at + n..]).collect();
                    if fails(&shorter) {
                        lines = shorter;
                        break;
                    }
                    at += n;
                }
                if at >= chars.len() {
                    cut /= 2;
                }
            }
        }
        if lines == before {
            return lines;
        }
    }
}

fn env(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse().ok()
}

fn run(seed: u64, docs: usize) {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    for i in 0..docs {
        let lines = document(&mut rng);
        if fails(&lines) {
            let small = shrink(lines.clone());
            // Run it once more outside catch_unwind, so the test fails with the message.
            eprintln!(
                "\nfuzz seed {seed}, document {i} fails. Shrunk to:\n{small:#?}\n\nOriginal:\n{lines:#?}\n\n\
                 Rerun with $env:QUILL_FUZZ_SEED = {seed}; cargo test fuzz -- --nocapture\n"
            );
            check_doc(&small);
            panic!("the shrunk document passed; the original fails: {lines:?}");
        }
    }
    println!("fuzz seed {seed}: {docs} documents ok");
}

#[test]
fn fuzz() {
    let docs = env("QUILL_FUZZ_DOCS").unwrap_or(3000) as usize;
    let seeds: Vec<u64> = match (env("QUILL_FUZZ_SEED"), env("QUILL_FUZZ_RUNS")) {
        (Some(s), _) => vec![s],
        (None, Some(runs)) => {
            let t = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64;
            (0..runs)
                .map(|i| (t ^ i.wrapping_mul(0x9E37_79B9)) % 1_000_000_000)
                .collect()
        }
        (None, None) => vec![1],
    };
    for seed in seeds {
        run(seed, docs);
    }
}
