//! Markdown to HTML, for Copy as HTML. Built on the same block and inline analysis
//! the renderer uses, so the HTML matches what the editor shows: each source line
//! of a paragraph is its own line (`<br>`), comments and front matter are left out,
//! and inline HTML is shown as text.

use std::path::Path;

use crate::markdown::{self, Align, Kind, LineInfo, Style};

/// The HTML for a whole document (or several whole lines of one).
pub fn document(lines: &[String], doc_dir: Option<&Path>) -> String {
    let infos = markdown::classify(lines);
    let mut w = Writer {
        out: String::new(),
        doc_dir,
        quotes: 0,
        lists: Vec::new(),
        para: None,
    };
    // Front matter is the first block when the document starts with `---`.
    let front = (lines.first().is_some_and(|l| l.trim_end() == "---")
        && infos[0].kind == Kind::Fence)
        .then_some(infos[0].block);
    let mut blank = false;
    let mut i = 0;
    while i < lines.len() {
        let (line, info) = (lines[i].as_str(), infos[i]);
        if info.kind == Kind::Comment || (front.is_some() && Some(info.block) == front) {
            i += 1;
            continue;
        }
        let opened = w.quote_to(info.quote);
        match info.kind {
            Kind::Blank => {
                w.end_para();
                blank = true;
                i += 1;
                continue;
            }
            Kind::Heading(n) => {
                w.end_block();
                let text = inline(line, info.content, heading_end(line, info.content), doc_dir);
                w.out.push_str(&format!("<h{n}>{text}</h{n}>\n"));
            }
            Kind::Rule => {
                w.end_block();
                w.out.push_str("<hr>\n");
            }
            Kind::Fence | Kind::Code => {
                w.end_para();
                w.lists_to(info.level as usize);
                i = w.code_block(lines, &infos, i);
                blank = false;
                continue;
            }
            Kind::Table { .. } => {
                w.end_block();
                i = w.table(lines, &infos, i);
                blank = false;
                continue;
            }
            Kind::List { ordered, task } => {
                w.end_para();
                w.list_item(line, &info, ordered, task);
                let end = line.trim_end().len().max(info.content);
                let text = inline(line, info.content.min(end), end, doc_dir);
                w.out.push_str(&text);
                w.para = Some(("", info.level as usize + 1));
            }
            Kind::Para => {
                let level = (info.level as usize).min(w.lists.len());
                let (start, end) = (info.content, para_end(line, info.content));
                match w.para {
                    Some((_, l)) if !blank && l == level => w.out.push_str("<br>\n"),
                    _ => {
                        w.end_para();
                        w.lists_to(level);
                        if level > 0 && !blank {
                            w.out.push_str("<br>\n");
                            w.para = Some(("", level));
                        } else {
                            w.out.push_str("<p>");
                            w.para = Some(("</p>\n", level));
                        }
                    }
                }
                let start = if opened {
                    w.callout(line, start, end)
                } else {
                    start
                };
                w.out.push_str(&inline(line, start, end, doc_dir));
            }
            Kind::Comment => {}
        }
        blank = false;
        i += 1;
    }
    w.end_block();
    w.quote_to(0);
    w.out
}

/// The HTML for `s[start..end]`, part of one line: its inline Markdown only.
pub fn inline(s: &str, start: usize, end: usize, doc_dir: Option<&Path>) -> String {
    let inl = markdown::parse_inline(s, start, end);
    let mut spans: Vec<&(std::ops::Range<usize>, Style)> = inl.spans.iter().collect();
    spans.sort_by_key(|(r, _)| (r.start, std::cmp::Reverse(r.end)));
    let mut out = String::new();
    // Open tags: (end of the span, closing tag).
    let mut stack: Vec<(usize, &str)> = Vec::new();
    let (mut si, mut h, mut i) = (0, 0, start);
    loop {
        while stack.last().is_some_and(|t| t.0 <= i) {
            out.push_str(stack.pop().unwrap().1);
        }
        if i >= end {
            break;
        }
        while si < spans.len() && spans[si].0.start <= i {
            let (r, st) = spans[si];
            si += 1;
            let (open, close) = match st {
                Style::Bold => ("<strong>".to_string(), "</strong>"),
                Style::Italic => ("<em>".to_string(), "</em>"),
                Style::Strike => ("<del>".to_string(), "</del>"),
                Style::Code => ("<code>".to_string(), "</code>"),
                Style::Mark => ("<mark>".to_string(), "</mark>"),
                Style::Link => match inl.links.iter().find(|l| l.0 == *r) {
                    // A wiki link means nothing outside the notes folder.
                    Some((_, target, _)) if target.starts_with("[[") => continue,
                    Some((_, target, title)) => {
                        let title = if title.is_empty() {
                            String::new()
                        } else {
                            format!(" title=\"{}\"", escape(title))
                        };
                        (format!("<a href=\"{}\"{title}>", escape(target)), "</a>")
                    }
                    None => continue,
                },
                Style::Image | Style::Comment | Style::Command => continue,
            };
            out.push_str(&open);
            stack.push((r.end, close));
        }
        if let Some((r, src)) = inl.images.iter().find(|(r, _)| r.start == i) {
            let alt = inl
                .spans
                .iter()
                .find(|(sr, st)| *st == Style::Image && r.start < sr.start && sr.end <= r.end)
                .map(|(sr, _)| &s[sr.clone()])
                .unwrap_or_default();
            out.push_str(&format!(
                "<img src=\"{}\" alt=\"{}\">",
                escape(&image_src(src, doc_dir)),
                escape(alt)
            ));
            i = r.end;
            continue;
        }
        if let Some((r, _)) = inl
            .spans
            .iter()
            .find(|(r, st)| r.start == i && *st == Style::Comment)
        {
            i = r.end.max(i + 1);
            continue;
        }
        while h < inl.hidden.len() && inl.hidden[h].end <= i {
            h += 1;
        }
        if h < inl.hidden.len() && inl.hidden[h].start <= i {
            i = inl.hidden[h].end;
            continue;
        }
        if let Some((r, sub)) = inl.subs.iter().find(|(r, _)| r.start == i) {
            // A command link's glyph stands for its `[`: dropped.
            let command = inl
                .spans
                .iter()
                .any(|(sr, st)| *st == Style::Command && sr.start == i);
            if !command {
                out.push_str(&escape(sub));
            }
            i = r.end;
            continue;
        }
        let c = s[i..].chars().next().unwrap();
        push_escaped(&mut out, c);
        i += c.len_utf8();
    }
    out
}

struct Writer<'a> {
    out: String,
    doc_dir: Option<&'a Path>,
    /// Open `<blockquote>`s.
    quotes: u8,
    /// Open lists (true: ordered), each with an open `<li>`.
    lists: Vec<bool>,
    /// An open run of text lines: its closing tag and its list level.
    para: Option<(&'static str, usize)>,
}

impl Writer<'_> {
    fn end_para(&mut self) {
        if let Some((close, _)) = self.para.take() {
            self.out.push_str(close);
        }
    }

    /// Closes lists until `n` are open.
    fn lists_to(&mut self, n: usize) {
        while self.lists.len() > n {
            let ordered = self.lists.pop().unwrap();
            self.out.push_str(if ordered {
                "</li>\n</ol>\n"
            } else {
                "</li>\n</ul>\n"
            });
        }
    }

    fn end_block(&mut self) {
        self.end_para();
        self.lists_to(0);
    }

    /// Opens or closes blockquotes to `depth`. True if one was opened.
    fn quote_to(&mut self, depth: u8) -> bool {
        if depth == self.quotes {
            return false;
        }
        self.end_block();
        while self.quotes > depth {
            self.out.push_str("</blockquote>\n");
            self.quotes -= 1;
        }
        let opened = self.quotes < depth;
        while self.quotes < depth {
            self.out.push_str("<blockquote>\n");
            self.quotes += 1;
        }
        opened
    }

    /// A `[!NOTE] Title` at the start of a blockquote: written as a bold title line.
    /// Returns where the rest of the line starts.
    fn callout(&mut self, line: &str, start: usize, end: usize) -> usize {
        let t = &line[start..end];
        let Some(close) = t.strip_prefix("[!").and_then(|r| r.find(']')) else {
            return start;
        };
        let name = &t[2..2 + close];
        let rest = start + 3 + close;
        let rest = rest + (line[rest..end].len() - line[rest..end].trim_start().len());
        let title = if rest < end {
            inline(line, rest, end, self.doc_dir)
        } else {
            let mut c = name.chars();
            let first = c.next().map(|f| f.to_uppercase().collect::<String>());
            escape(&(first.unwrap_or_default() + &c.as_str().to_lowercase()))
        };
        self.out.push_str(&format!("<strong>{title}</strong>"));
        end
    }

    fn list_item(&mut self, line: &str, info: &LineInfo, ordered: bool, task: Option<bool>) {
        let level = info.level as usize;
        self.lists_to(level + 1);
        if self.lists.len() == level + 1 {
            if self.lists[level] == ordered {
                self.out.push_str("</li>\n");
            } else {
                self.lists_to(level);
            }
        }
        // An item indented deeper than the list it follows opens lists in between.
        while self.lists.len() < level {
            self.lists.push(false);
            self.out.push_str("<ul>\n<li>");
        }
        if self.lists.len() == level {
            self.lists.push(ordered);
            let start: u64 = line[info.marker..]
                .split(['.', ')'])
                .next()
                .and_then(|n| n.parse().ok())
                .unwrap_or(1);
            match (ordered, start) {
                (true, 1) => self.out.push_str("<ol>\n"),
                (true, n) => self.out.push_str(&format!("<ol start=\"{n}\">\n")),
                _ => self.out.push_str("<ul>\n"),
            }
        }
        self.out.push_str("<li>");
        match task {
            Some(true) => self.out.push_str("\u{2611} "),
            Some(false) => self.out.push_str("\u{2610} "),
            None => {}
        }
    }

    /// Writes the code block (or the rest of one) starting at line `i`; returns the
    /// line after it.
    fn code_block(&mut self, lines: &[String], infos: &[LineInfo], mut i: usize) -> usize {
        let block = infos[i].block;
        let mut lang = "";
        if infos[i].kind == Kind::Fence {
            let t = lines[i][infos[i].qend..].trim_start();
            lang = t
                .trim_start_matches(['`', '~'])
                .split_whitespace()
                .next()
                .unwrap_or("");
            i += 1;
        }
        if lang.is_empty() {
            self.out.push_str("<pre><code>");
        } else {
            self.out
                .push_str(&format!("<pre><code class=\"language-{}\">", escape(lang)));
        }
        while i < lines.len() && infos[i].block == block {
            if infos[i].kind == Kind::Fence {
                i += 1;
                break;
            }
            let line = &lines[i];
            self.out
                .push_str(&escape(&line[infos[i].content.min(line.len())..]));
            self.out.push('\n');
            i += 1;
        }
        self.out.push_str("</code></pre>\n");
        i
    }

    /// Writes the table starting at line `i`; returns the line after it.
    fn table(&mut self, lines: &[String], infos: &[LineInfo], mut i: usize) -> usize {
        let block = infos[i].block;
        let aligns = lines
            .get(i + 1)
            .map(|l| markdown::delim_row(&l[infos[i + 1].qend..]).unwrap_or_default())
            .unwrap_or_default();
        self.out.push_str("<table>\n");
        let mut body = false;
        while i < lines.len() && infos[i].block == block {
            let Kind::Table { row } = infos[i].kind else {
                break;
            };
            let line = &lines[i];
            let qend = infos[i].qend;
            i += 1;
            if row == 1 {
                continue;
            }
            if row >= 2 && !body {
                self.out.push_str("<tbody>\n");
                body = true;
            }
            let tag = if row == 0 { "th" } else { "td" };
            self.out
                .push_str(if row == 0 { "<thead>\n<tr>" } else { "<tr>" });
            for (k, r) in markdown::split_cells(line, qend).iter().enumerate() {
                let style = match aligns.get(k) {
                    Some(Align::Center) => " style=\"text-align:center\"",
                    Some(Align::Right) => " style=\"text-align:right\"",
                    _ => "",
                };
                let text = inline(line, r.start, r.end, self.doc_dir);
                self.out.push_str(&format!("<{tag}{style}>{text}</{tag}>"));
            }
            self.out.push_str(if row == 0 {
                "</tr>\n</thead>\n"
            } else {
                "</tr>\n"
            });
        }
        if body {
            self.out.push_str("</tbody>\n");
        }
        self.out.push_str("</table>\n");
        i
    }
}

/// The end of a heading's text: trailing spaces and a closing `#` run removed.
fn heading_end(s: &str, start: usize) -> usize {
    let t = s.trim_end();
    let head = t.trim_end_matches('#');
    if head.len() < t.len() && (head.len() <= start || head.ends_with([' ', '\t'])) {
        head.trim_end().len().max(start)
    } else {
        t.len().max(start)
    }
}

/// The end of a paragraph line's text: trailing spaces and a hard-break `\` removed
/// (every line ends with a break anyway).
fn para_end(s: &str, start: usize) -> usize {
    let t = s.trim_end();
    let t = t.strip_suffix('\\').unwrap_or(t);
    t.len().max(start)
}

/// An image's `src`: web and data URLs as they are, local paths as `file:` URLs
/// (relative ones against the document's folder).
fn image_src(src: &str, doc_dir: Option<&Path>) -> String {
    if ["http://", "https://", "data:", "file:"]
        .iter()
        .any(|p| src.starts_with(p))
    {
        return src.to_string();
    }
    let path = crate::attach::decode(src);
    let path = match doc_dir {
        Some(dir) if Path::new(&path).is_relative() => dir.join(&path),
        _ => return src.to_string(),
    };
    let path = path.to_string_lossy().replace('\\', "/");
    format!(
        "file:///{}",
        crate::attach::encode(path.trim_start_matches('/'))
    )
}

fn push_escaped(out: &mut String, c: char) {
    match c {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        c => out.push(c),
    }
}

fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        push_escaped(&mut out, c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn html(src: &str) -> String {
        let lines: Vec<String> = src.split('\n').map(String::from).collect();
        document(&lines, None)
    }

    #[test]
    fn inline_styles() {
        let s = "**bold** *it* ~~del~~ ==mark== `a<b` &amp; ***both***";
        assert_eq!(
            inline(s, 0, s.len(), None),
            "<strong>bold</strong> <em>it</em> <del>del</del> <mark>mark</mark> \
             <code>a&lt;b</code> &amp; <em><strong>both</strong></em>"
        );
    }

    #[test]
    fn links_and_images() {
        let s = "[a *b*](http://x.com \"T\") <https://y.org> [[Note|n]] ![pic](a%20b.png) \\*";
        assert_eq!(
            inline(s, 0, s.len(), Some(Path::new("C:\\notes"))),
            "<a href=\"http://x.com\" title=\"T\">a <em>b</em></a> \
             <a href=\"https://y.org\">https://y.org</a> n \
             <img src=\"file:///C:/notes/a%20b.png\" alt=\"pic\"> *"
        );
    }

    #[test]
    fn paragraphs_and_headings() {
        assert_eq!(
            html("# Title #\n\none\ntwo  \n\nthree <!-- hidden -->"),
            "<h1>Title</h1>\n<p>one<br>\ntwo</p>\n<p>three </p>\n"
        );
    }

    #[test]
    fn nested_lists() {
        assert_eq!(
            html("- a\n  - [x] b\n  - [ ] c\n- d\n\n  more\n\n3. e\n4. f"),
            "<ul>\n<li>a<ul>\n<li>\u{2611} b</li>\n<li>\u{2610} c</li>\n</ul>\n</li>\n\
             <li>d<p>more</p>\n</li>\n</ul>\n<ol start=\"3\">\n<li>e</li>\n<li>f</li>\n</ol>\n"
        );
    }

    #[test]
    fn code_blocks_tables_quotes() {
        assert_eq!(
            html(
                "---\ntitle: x\n---\n```rust\nlet a = 1 < 2;\n```\n| A | B |\n|:-:|--:|\n| 1 | `|` |"
            ),
            "<pre><code class=\"language-rust\">let a = 1 &lt; 2;\n</code></pre>\n\
             <table>\n<thead>\n<tr><th style=\"text-align:center\">A</th>\
             <th style=\"text-align:right\">B</th></tr>\n</thead>\n<tbody>\n\
             <tr><td style=\"text-align:center\">1</td><td style=\"text-align:right\">\
             <code>|</code></td></tr>\n</tbody>\n</table>\n"
        );
        assert_eq!(
            html("> [!NOTE]\n> text\n>> deep\n\n---"),
            "<blockquote>\n<p><strong>Note</strong><br>\ntext</p>\n<blockquote>\n\
             <p>deep</p>\n</blockquote>\n</blockquote>\n<hr>\n"
        );
    }
}
