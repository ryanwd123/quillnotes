//! Markdown analysis used by the live-preview renderer.
//!
//! The document is always kept as plain Markdown source. This module only
//! *describes* it: [`classify`] assigns every source line a block role, and
//! [`parse_inline`] finds inline styles plus the delimiter ranges that the
//! renderer hides when a line is not being edited.

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Blank,
    Para,
    Heading(u8),
    List {
        ordered: bool,
        task: Option<bool>,
    },
    Rule,
    /// Opening/closing ``` / ~~~ fence, or a front-matter `---` delimiter.
    Fence,
    /// A line inside a fenced code block or front matter.
    Code,
    /// GFM table row: 0 = header, 1 = delimiter row, 2.. = body rows.
    Table {
        row: u16,
    },
    /// A line inside a multi-line `<!-- ... -->` comment.
    Comment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct LineInfo {
    pub kind: Kind,
    /// Blockquote nesting depth.
    pub quote: u8,
    /// Byte offset just past the blockquote markers.
    pub qend: usize,
    /// List nesting level (items, and paragraphs/code continuing an item).
    pub level: u8,
    /// Byte offset of the list marker or heading hashes.
    pub marker: usize,
    /// Byte offset where the inline content starts.
    pub content: usize,
    /// Non-zero id shared by every line of one code block, table or front matter.
    pub block: u32,
}

impl LineInfo {
    fn new(kind: Kind, quote: u8, qend: usize) -> Self {
        LineInfo {
            kind,
            quote,
            qend,
            level: 0,
            marker: qend,
            content: qend,
            block: 0,
        }
    }
}

/// Columns and bytes of leading whitespace.
pub fn leading_ws(s: &str) -> (usize, usize) {
    let mut cols = 0;
    for (i, b) in s.bytes().enumerate() {
        match b {
            b' ' => cols += 1,
            b'\t' => cols += 4 - cols % 4,
            _ => return (cols, i),
        }
    }
    (cols, s.len())
}

/// Strips up to `max` blockquote markers. Returns (depth, bytes consumed).
pub fn strip_quotes(s: &str, max: u8) -> (u8, usize) {
    let b = s.as_bytes();
    let (mut depth, mut i) = (0u8, 0usize);
    while depth < max {
        let mut j = i;
        while j < b.len() && j - i < 3 && b[j] == b' ' {
            j += 1;
        }
        if j < b.len() && b[j] == b'>' {
            j += 1;
            if j < b.len() && (b[j] == b' ' || b[j] == b'\t') {
                j += 1;
            }
            i = j;
            depth += 1;
        } else {
            break;
        }
    }
    (depth, i)
}

/// Opening code fence: (fence char, run length, bytes of indentation).
fn fence_open(s: &str, max_indent: usize) -> Option<(u8, usize, usize)> {
    let (cols, ws) = leading_ws(s);
    if cols > max_indent {
        return None;
    }
    let rest = &s.as_bytes()[ws..];
    let ch = *rest.first()?;
    if ch != b'`' && ch != b'~' {
        return None;
    }
    let n = rest.iter().take_while(|&&c| c == ch).count();
    if n < 3 || (ch == b'`' && rest[n..].contains(&b'`')) {
        return None;
    }
    Some((ch, n, ws))
}

fn fence_close(s: &str, ch: u8, len: usize) -> bool {
    let t = s.trim_start_matches([' ', '\t']).as_bytes();
    let n = t.iter().take_while(|&&c| c == ch).count();
    n >= len && t[n..].iter().all(|c| *c == b' ' || *c == b'\t')
}

/// ATX heading: (level, byte offset of the hashes, content offset).
fn heading(s: &str) -> Option<(u8, usize, usize)> {
    let (cols, ws) = leading_ws(s);
    if cols > 3 {
        return None;
    }
    let b = &s.as_bytes()[ws..];
    let n = b.iter().take_while(|&&c| c == b'#').count();
    if n == 0 || n > 6 {
        return None;
    }
    if n < b.len() && b[n] != b' ' && b[n] != b'\t' {
        return None;
    }
    let mut c = ws + n;
    while c < s.len() && matches!(s.as_bytes()[c], b' ' | b'\t') {
        c += 1;
    }
    Some((n as u8, ws, c))
}

fn is_rule(s: &str) -> bool {
    let (cols, ws) = leading_ws(s);
    if cols > 3 {
        return false;
    }
    let rest = s[ws..].trim_end();
    let Some(ch) = rest.bytes().next() else {
        return false;
    };
    if !matches!(ch, b'-' | b'*' | b'_') {
        return false;
    }
    let mut n = 0;
    for c in rest.bytes() {
        if c == ch {
            n += 1;
        } else if c != b' ' && c != b'\t' {
            return false;
        }
    }
    n >= 3
}

pub struct ListMarker {
    pub indent_cols: usize,
    pub marker: usize,
    pub content: usize,
    pub ordered: bool,
    pub task: Option<bool>,
}

/// Parses a list item marker at the start of `s`.
pub fn list_marker(s: &str) -> Option<ListMarker> {
    let (cols, ws) = leading_ws(s);
    let b = s.as_bytes();
    let mut i = ws;
    let ordered;
    match b.get(i)? {
        b'-' | b'*' | b'+' => {
            ordered = false;
            i += 1;
        }
        b'0'..=b'9' => {
            let n = b[i..].iter().take_while(|c| c.is_ascii_digit()).count();
            if n > 9 {
                return None;
            }
            i += n;
            if !matches!(b.get(i), Some(b'.' | b')')) {
                return None;
            }
            ordered = true;
            i += 1;
        }
        _ => return None,
    }
    if i < b.len() && b[i] != b' ' && b[i] != b'\t' {
        return None;
    }
    let after_marker = i;
    let spaces = b[i..].iter().take_while(|&&c| c == b' ').count();
    let mut content = if i == b.len() {
        i
    } else if (1..=4).contains(&spaces) {
        i + spaces
    } else {
        i + 1
    };
    let mut task = None;
    let t = &b[content.min(b.len())..];
    if content > after_marker
        && t.len() >= 3
        && t[0] == b'['
        && t[2] == b']'
        && (t.len() == 3 || t[3] == b' ')
    {
        match t[1] {
            b' ' => task = Some(false),
            b'x' | b'X' => task = Some(true),
            _ => {}
        }
        if task.is_some() {
            content += if t.len() > 3 { 4 } else { 3 };
        }
    }
    Some(ListMarker {
        indent_cols: cols,
        marker: ws,
        content,
        ordered,
        task,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// Parses a table delimiter row such as `| :-- | :-: |`.
pub fn delim_row(s: &str) -> Option<Vec<Align>> {
    let t = s.trim();
    if !t.contains('|') && !t.contains(':') {
        return None;
    }
    let t = t.strip_prefix('|').unwrap_or(t);
    let t = t.strip_suffix('|').unwrap_or(t);
    let mut out = Vec::new();
    for cell in t.split('|') {
        let c = cell.trim();
        let left = c.starts_with(':');
        let right = c.ends_with(':') && c.len() > 1;
        let dashes = c.trim_matches(':');
        if dashes.is_empty() || !dashes.bytes().all(|b| b == b'-') {
            return None;
        }
        out.push(match (left, right) {
            (true, true) => Align::Center,
            (false, true) => Align::Right,
            _ => Align::Left,
        });
    }
    (!out.is_empty()).then_some(out)
}

/// Byte ranges (absolute in `line`) of each trimmed table cell, starting at `start`.
pub fn split_cells(line: &str, start: usize) -> Vec<Range<usize>> {
    let b = line.as_bytes();
    let mut i = start;
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    if i < b.len() && b[i] == b'|' {
        i += 1;
    }
    let mut cells = Vec::new();
    let mut cell_start = i;
    let mut in_code = false;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 1,
            b'`' => in_code = !in_code,
            b'|' if !in_code => {
                cells.push(cell_start..i);
                cell_start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    let tail = cell_start.min(b.len())..b.len();
    if !line[tail.clone()].trim().is_empty() || cells.is_empty() {
        cells.push(tail);
    }
    cells
        .into_iter()
        .map(|r| {
            let s = &line[r.clone()];
            let lead = s.len() - s.trim_start().len();
            let trail = s.len() - s.trim_end().len();
            if lead + trail >= s.len() {
                // Empty cell: an empty range just inside its padding, where typing goes.
                let at = r.start + s.len().min(1);
                at..at
            } else {
                r.start + lead..r.end - trail
            }
        })
        .collect()
}

/// Assigns a block role to every line of the document.
pub fn classify(lines: &[String]) -> Vec<LineInfo> {
    let mut out: Vec<LineInfo> = Vec::with_capacity(lines.len());
    let mut next_block = 1u32;
    let mut i = 0;

    // YAML front matter.
    if lines
        .first()
        .map(|l| l.trim_end() == "---")
        .unwrap_or(false)
        && let Some(end) = lines
            .iter()
            .skip(1)
            .position(|l| matches!(l.trim_end(), "---" | "..."))
    {
        let end = end + 1;
        for j in 0..=end {
            let kind = if j == 0 || j == end {
                Kind::Fence
            } else {
                Kind::Code
            };
            let mut info = LineInfo::new(kind, 0, 0);
            info.block = next_block;
            out.push(info);
        }
        next_block += 1;
        i = end + 1;
    }

    struct Fence {
        ch: u8,
        len: usize,
        quote: u8,
        indent: usize,
        level: u8,
        block: u32,
    }
    let mut fence: Option<Fence> = None;
    // (marker column, content column) for each open list item.
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut stack_quote = 0u8;
    let mut table: Option<(u32, u16, u8)> = None; // block, next row, quote
    let mut in_comment = false;

    while i < lines.len() {
        let line = lines[i].as_str();

        if let Some(f) = &fence {
            let (q, qlen) = strip_quotes(line, f.quote);
            let rest = &line[qlen..];
            let mut info = LineInfo::new(Kind::Code, q, qlen);
            info.block = f.block;
            info.level = f.level;
            if fence_close(rest, f.ch, f.len) {
                info.kind = Kind::Fence;
                fence = None;
            } else {
                let ws = rest
                    .bytes()
                    .take(f.indent)
                    .take_while(|&c| c == b' ')
                    .count();
                info.content = qlen + ws;
                info.marker = info.content;
            }
            out.push(info);
            i += 1;
            continue;
        }

        if in_comment {
            out.push(LineInfo::new(Kind::Comment, 0, 0));
            in_comment = !line.contains("-->");
            i += 1;
            continue;
        }

        let (q, qlen) = strip_quotes(line, u8::MAX);
        let rest = &line[qlen..];

        if let Some((block, row, tq)) = table {
            if tq == q && !rest.trim().is_empty() && rest.contains('|') {
                let mut info = LineInfo::new(Kind::Table { row }, q, qlen);
                info.block = block;
                out.push(info);
                table = Some((block, row.saturating_add(1), tq));
                i += 1;
                continue;
            }
            table = None;
        }

        if q != stack_quote {
            stack.clear();
            stack_quote = q;
        }

        if rest.trim().is_empty() {
            out.push(LineInfo::new(Kind::Blank, q, qlen));
            i += 1;
            continue;
        }

        let (cols, ws) = leading_ws(rest);
        let nested_level =
            |stack: &[(usize, usize)]| stack.iter().filter(|e| e.1 <= cols).count() as u8;
        let max_fence_indent = if stack.is_empty() {
            3
        } else {
            stack.last().unwrap().1 + 3
        };

        if let Some((ch, len, indent)) = fence_open(rest, max_fence_indent) {
            let level = nested_level(&stack);
            stack.truncate(level as usize);
            let mut info = LineInfo::new(Kind::Fence, q, qlen);
            info.block = next_block;
            info.level = level;
            fence = Some(Fence {
                ch,
                len,
                quote: q,
                indent,
                level,
                block: next_block,
            });
            next_block += 1;
            out.push(info);
            i += 1;
            continue;
        }

        if q == 0
            && rest.trim_start().starts_with("<!--")
            && !rest[rest.find("<!--").unwrap() + 4..].contains("-->")
        {
            stack.clear();
            out.push(LineInfo::new(Kind::Comment, 0, 0));
            in_comment = true;
            i += 1;
            continue;
        }

        if let Some((level, marker, content)) = heading(rest) {
            stack.clear();
            let mut info = LineInfo::new(Kind::Heading(level), q, qlen);
            info.marker = qlen + marker;
            info.content = qlen + content;
            out.push(info);
            i += 1;
            continue;
        }

        if is_rule(rest) {
            stack.clear();
            out.push(LineInfo::new(Kind::Rule, q, qlen));
            i += 1;
            continue;
        }

        if let Some(m) = list_marker(rest) {
            while stack.last().is_some_and(|top| m.indent_cols < top.1) {
                stack.pop();
            }
            let level = stack.len().min(u8::MAX as usize) as u8;
            let content_cols = m.indent_cols + (m.content - m.marker).min(rest.len() - m.marker);
            stack.push((m.indent_cols, content_cols.max(m.indent_cols + 2)));
            let mut info = LineInfo::new(
                Kind::List {
                    ordered: m.ordered,
                    task: m.task,
                },
                q,
                qlen,
            );
            info.level = level;
            info.marker = qlen + m.marker;
            info.content = qlen + m.content;
            out.push(info);
            i += 1;
            continue;
        }

        // GFM table: a header row followed by a delimiter row.
        if rest.contains('|') && i + 1 < lines.len() {
            let next = lines[i + 1].as_str();
            let (nq, nlen) = strip_quotes(next, u8::MAX);
            if nq == q && delim_row(&next[nlen..]).is_some() {
                stack.clear();
                let mut info = LineInfo::new(Kind::Table { row: 0 }, q, qlen);
                info.block = next_block;
                out.push(info);
                table = Some((next_block, 1, q));
                next_block += 1;
                i += 1;
                continue;
            }
        }

        let mut info = LineInfo::new(Kind::Para, q, qlen);
        if cols > 0 && !stack.is_empty() {
            info.level = nested_level(&stack);
            stack.truncate(info.level as usize);
            info.content = qlen + ws;
            info.marker = info.content;
        } else if cols == 0 {
            stack.clear();
        }
        out.push(info);
        i += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Inline parsing

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Bold,
    Italic,
    Strike,
    Code,
    Link,
    Mark,
    Image,
    Comment,
}

#[derive(Default, Debug)]
pub struct Inline {
    /// Delimiters hidden when the line is rendered (shown dimmed while editing).
    pub hidden: Vec<Range<usize>>,
    pub spans: Vec<(Range<usize>, Style)>,
    /// Visible label range and link target.
    pub links: Vec<(Range<usize>, String)>,
    /// Whole source range and target of each image.
    pub images: Vec<(Range<usize>, String)>,
    /// HTML entities shown as a single character when rendered.
    pub subs: Vec<(Range<usize>, char)>,
}

fn is_punct(c: char) -> bool {
    c.is_ascii_punctuation() || (!c.is_ascii() && !c.is_alphanumeric() && !c.is_whitespace())
}

struct Delim {
    ch: u8,
    orig: usize,
    /// Unconsumed part of the run: openers consume from `hi`, closers from `lo`.
    lo: usize,
    hi: usize,
    can_open: bool,
    can_close: bool,
}

/// Parses inline Markdown in `s[start..end]`; all ranges are absolute byte offsets in `s`.
pub fn parse_inline(s: &str, start: usize, end: usize) -> Inline {
    let mut out = Inline::default();
    parse_range(s, start, end, &mut out);
    out.hidden.sort_by_key(|r| r.start);
    out.spans.sort_by_key(|r| r.0.start);
    out.subs.sort_by_key(|r| r.0.start);
    out
}

/// Decodes an HTML entity starting at `s[i] == '&'`: (byte length, char).
fn entity(s: &str, i: usize, end: usize) -> Option<(usize, char)> {
    let t = &s[i + 1..end];
    let semi = t.find(';').filter(|&k| k > 0 && k <= 10)?;
    let name = &t[..semi];
    let c = if let Some(num) = name.strip_prefix('#') {
        let v = match num.strip_prefix(['x', 'X']) {
            Some(hex) => u32::from_str_radix(hex, 16).ok()?,
            None => num.parse().ok()?,
        };
        char::from_u32(v).filter(|&c| c != '\0')?
    } else {
        match name {
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "quot" => '"',
            "apos" => '\'',
            "nbsp" => '\u{a0}',
            "copy" => '©',
            "reg" => '®',
            "trade" => '™',
            "hellip" => '…',
            "mdash" => '—',
            "ndash" => '–',
            "laquo" => '«',
            "raquo" => '»',
            "lsquo" => '‘',
            "rsquo" => '’',
            "ldquo" => '“',
            "rdquo" => '”',
            "deg" => '°',
            "times" => '×',
            "divide" => '÷',
            "plusmn" => '±',
            "middot" => '·',
            "bull" => '•',
            "larr" => '←',
            "rarr" => '→',
            "uarr" => '↑',
            "darr" => '↓',
            "harr" => '↔',
            "para" => '¶',
            "sect" => '§',
            "euro" => '€',
            "pound" => '£',
            "yen" => '¥',
            "cent" => '¢',
            "check" => '✓',
            _ => return None,
        }
    };
    Some((semi + 2, c))
}

fn char_before(s: &str, i: usize, start: usize) -> char {
    if i <= start {
        ' '
    } else {
        s[..i].chars().next_back().unwrap_or(' ')
    }
}

fn char_at(s: &str, i: usize, end: usize) -> char {
    if i >= end {
        ' '
    } else {
        s[i..].chars().next().unwrap_or(' ')
    }
}

/// Parses `[label](target)` starting at `i` (which must be `[`).
fn parse_link(s: &str, i: usize, end: usize) -> Option<(Range<usize>, String, usize)> {
    let b = s.as_bytes();
    let mut j = i + 1;
    let mut depth = 1;
    while j < end {
        match b[j] {
            b'\\' => j += 1,
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            b'`' => {
                // Skip code spans inside the label.
                if let Some(k) = s[j + 1..end].find('`') {
                    j += k + 1;
                }
            }
            _ => {}
        }
        j += 1;
    }
    if j >= end || b.get(j + 1) != Some(&b'(') || j + 1 >= end {
        return None;
    }
    let label = i + 1..j;
    let mut k = j + 2;
    let mut paren = 1;
    let mut angle = false;
    while k < end {
        match b[k] {
            b'\\' => k += 1,
            b'<' => angle = true,
            b'>' => angle = false,
            b'(' if !angle => paren += 1,
            b')' if !angle => {
                paren -= 1;
                if paren == 0 {
                    break;
                }
            }
            _ => {}
        }
        k += 1;
    }
    if k >= end {
        return None;
    }
    let inner = s[j + 2..k].trim();
    let target = if let Some(rest) = inner.strip_prefix('<') {
        rest.split('>').next().unwrap_or("").to_string()
    } else {
        inner.split_whitespace().next().unwrap_or("").to_string()
    };
    Some((label, target, k + 1))
}

fn parse_range(s: &str, start: usize, end: usize, out: &mut Inline) {
    let b = s.as_bytes();
    let mut delims: Vec<Delim> = Vec::new();
    let mut i = start;
    while i < end {
        let c = b[i];
        match c {
            b'\\' if i + 1 < end && b[i + 1].is_ascii_punctuation() => {
                out.hidden.push(i..i + 1);
                i += 2;
                continue;
            }
            b'`' => {
                let n = b[i..end].iter().take_while(|&&x| x == b'`').count();
                let mut j = i + n;
                let mut found = None;
                while j < end {
                    if b[j] == b'`' {
                        let m = b[j..end].iter().take_while(|&&x| x == b'`').count();
                        if m == n {
                            found = Some(j);
                            break;
                        }
                        j += m;
                    } else {
                        j += 1;
                    }
                }
                if let Some(j) = found {
                    out.hidden.push(i..i + n);
                    out.hidden.push(j..j + n);
                    out.spans.push((i + n..j, Style::Code));
                    i = j + n;
                } else {
                    i += n;
                }
                continue;
            }
            b'!' if i + 1 < end && b[i + 1] == b'[' => {
                if let Some((label, target, after)) = parse_link(s, i + 1, end) {
                    out.hidden.push(i..label.start);
                    out.hidden.push(label.end..after);
                    out.spans.push((label.clone(), Style::Image));
                    out.images.push((i..after, target));
                    i = after;
                    continue;
                }
            }
            b'[' => {
                // Wiki link: [[page]] or [[page|alias]].
                if i + 1 < end
                    && b[i + 1] == b'['
                    && let Some(k) = s[i + 2..end].find("]]")
                {
                    let inner = i + 2..i + 2 + k;
                    let text = &s[inner.clone()];
                    if !text.is_empty() && !text.contains('[') {
                        let (target, label) = match text.find('|') {
                            Some(p) => (text[..p].to_string(), inner.start + p + 1..inner.end),
                            None => (text.to_string(), inner.clone()),
                        };
                        out.hidden.push(i..label.start);
                        out.hidden.push(inner.end..inner.end + 2);
                        out.spans.push((label.clone(), Style::Link));
                        out.links.push((label, format!("[[{target}]]")));
                        i = inner.end + 2;
                        continue;
                    }
                }
                if let Some((label, target, after)) = parse_link(s, i, end) {
                    out.hidden.push(i..i + 1);
                    out.hidden.push(label.end..after);
                    out.spans.push((label.clone(), Style::Link));
                    out.links.push((label.clone(), target));
                    parse_range(s, label.start, label.end, out);
                    i = after;
                    continue;
                }
            }
            b'&' => {
                if let Some((len, c)) = entity(s, i, end) {
                    out.subs.push((i..i + len, c));
                    i += len;
                    continue;
                }
            }
            b'<' if s[i..end].starts_with("<!--") => {
                let close = s[i + 4..end]
                    .find("-->")
                    .map(|k| i + 4 + k + 3)
                    .unwrap_or(end);
                out.spans.push((i..close, Style::Comment));
                i = close;
                continue;
            }
            b'<' => {
                if let Some(k) = s[i + 1..end].find('>') {
                    let inner = &s[i + 1..i + 1 + k];
                    let is_url = inner.starts_with("http://")
                        || inner.starts_with("https://")
                        || inner.starts_with("mailto:");
                    let is_mail =
                        inner.contains('@') && !inner.contains(' ') && !inner.contains(':');
                    if (is_url || is_mail) && !inner.contains(' ') {
                        let r = i + 1..i + 1 + k;
                        out.hidden.push(i..i + 1);
                        out.hidden.push(r.end..r.end + 1);
                        out.spans.push((r.clone(), Style::Link));
                        let target = if is_mail {
                            format!("mailto:{inner}")
                        } else {
                            inner.to_string()
                        };
                        out.links.push((r.clone(), target));
                        i = r.end + 1;
                        continue;
                    }
                }
            }
            b'h' | b'w' => {
                let prev = char_before(s, i, start);
                let t = &s[i..end];
                if (prev.is_whitespace() || prev == '(')
                    && (t.starts_with("https://")
                        || t.starts_with("http://")
                        || t.starts_with("www."))
                {
                    let mut len = t
                        .find(|ch: char| ch.is_whitespace() || ch == '<')
                        .unwrap_or(t.len());
                    while len > 0
                        && matches!(
                            t.as_bytes()[len - 1],
                            b'.' | b','
                                | b':'
                                | b';'
                                | b'!'
                                | b'?'
                                | b'"'
                                | b'\''
                                | b'*'
                                | b'_'
                                | b'~'
                        )
                    {
                        len -= 1;
                    }
                    if len > 0 && t.as_bytes()[len - 1] == b')' && !t[..len].contains('(') {
                        len -= 1;
                    }
                    if len > 8 {
                        let r = i..i + len;
                        let url = &s[r.clone()];
                        let target = if url.starts_with("www.") {
                            format!("https://{url}")
                        } else {
                            url.to_string()
                        };
                        out.spans.push((r.clone(), Style::Link));
                        out.links.push((r, target));
                        i += len;
                        continue;
                    }
                }
            }
            b'*' | b'_' | b'~' | b'=' => {
                let n = b[i..end].iter().take_while(|&&x| x == c).count();
                let prev = char_before(s, i, start);
                let next = char_at(s, i + n, end);
                let left = !next.is_whitespace()
                    && (!is_punct(next) || prev.is_whitespace() || is_punct(prev));
                let right = !prev.is_whitespace()
                    && (!is_punct(prev) || next.is_whitespace() || is_punct(next));
                let (can_open, can_close) = match c {
                    b'_' => (
                        left && (!right || is_punct(prev)),
                        right && (!left || is_punct(next)),
                    ),
                    b'*' => (left, right),
                    _ => (left && n == 2, right && n == 2),
                };
                if can_open || can_close {
                    delims.push(Delim {
                        ch: c,
                        orig: n,
                        lo: i,
                        hi: i + n,
                        can_open,
                        can_close,
                    });
                }
                i += n;
                continue;
            }
            _ => {}
        }
        i += s[i..].chars().next().map(|ch| ch.len_utf8()).unwrap_or(1);
    }

    // CommonMark-style "process emphasis".
    for ci in 0..delims.len() {
        if !delims[ci].can_close {
            continue;
        }
        while delims[ci].hi > delims[ci].lo {
            let (cch, corig, ccan_open) = (delims[ci].ch, delims[ci].orig, delims[ci].can_open);
            let opener = (0..ci).rev().find(|&oi| {
                let o = &delims[oi];
                if o.ch != cch || !o.can_open || o.hi == o.lo {
                    return false;
                }
                !((cch == b'*' || cch == b'_')
                    && (o.can_close || ccan_open)
                    && (o.orig + corig).is_multiple_of(3)
                    && !(o.orig.is_multiple_of(3) && corig % 3 == 0))
            });
            let Some(oi) = opener else { break };
            let (olen, clen) = (delims[oi].hi - delims[oi].lo, delims[ci].hi - delims[ci].lo);
            let use_n = match cch {
                b'~' | b'=' => 2,
                _ if olen >= 2 && clen >= 2 => 2,
                _ => 1,
            };
            if olen < use_n || clen < use_n {
                break;
            }
            let open_r = delims[oi].hi - use_n..delims[oi].hi;
            let close_r = delims[ci].lo..delims[ci].lo + use_n;
            let style = match (cch, use_n) {
                (b'~', _) => Style::Strike,
                (b'=', _) => Style::Mark,
                (_, 2) => Style::Bold,
                _ => Style::Italic,
            };
            out.hidden.push(open_r.clone());
            out.hidden.push(close_r.clone());
            out.spans.push((open_r.end..close_r.start, style));
            delims[oi].hi -= use_n;
            delims[ci].lo += use_n;
            for d in &mut delims[oi + 1..ci] {
                d.lo = d.hi;
            }
        }
    }
}

/// Counts words: CJK characters individually, other text split on whitespace.
pub fn count_words(text: &str) -> usize {
    let mut count = 0;
    let mut in_word = false;
    for ch in text.chars() {
        let cjk = matches!(ch as u32, 0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0x20000..=0x2A6DF | 0xF900..=0xFAFF | 0x3040..=0x30FF | 0xAC00..=0xD7AF);
        if cjk {
            count += 1 + in_word as usize;
            in_word = false;
        } else if ch.is_whitespace() {
            count += in_word as usize;
            in_word = false;
        } else if ch.is_alphanumeric() {
            in_word = true;
        }
    }
    count + in_word as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &str) -> Vec<String> {
        s.split('\n').map(String::from).collect()
    }

    fn styled(s: &str) -> Vec<(String, Style)> {
        let inl = parse_inline(s, 0, s.len());
        inl.spans
            .iter()
            .map(|(r, st)| (s[r.clone()].to_string(), *st))
            .collect()
    }

    fn hidden(s: &str) -> String {
        let inl = parse_inline(s, 0, s.len());
        let mut out = String::new();
        let mut i = 0;
        for r in &inl.hidden {
            if r.start >= i {
                out.push_str(&s[i..r.start]);
                i = r.end;
            }
        }
        out.push_str(&s[i..]);
        out
    }

    #[test]
    fn emphasis() {
        assert_eq!(styled("a **b** c"), vec![("b".into(), Style::Bold)]);
        assert_eq!(styled("*i*"), vec![("i".into(), Style::Italic)]);
        assert_eq!(hidden("***bi***"), "bi");
        assert_eq!(
            hidden("This is **bold with *italic inside* and more**."),
            "This is bold with italic inside and more."
        );
        assert_eq!(hidden("foo_bar_baz"), "foo_bar_baz");
        assert_eq!(hidden("2 * 3 * 4"), "2 * 3 * 4");
        assert_eq!(hidden("\\*not italic\\*"), "*not italic*");
        assert_eq!(hidden("~~gone~~ and ==mark=="), "gone and mark");
    }

    #[test]
    fn code_and_links() {
        assert_eq!(hidden("use `a*b*c` here"), "use a*b*c here");
        assert_eq!(styled("`x`"), vec![("x".into(), Style::Code)]);
        let s = "see [the **docs**](https://x.y/z (t)) now";
        assert_eq!(hidden(s), "see the docs now");
        let inl = parse_inline(s, 0, s.len());
        assert_eq!(inl.links[0].1, "https://x.y/z");
        assert_eq!(hidden("![alt](img.png)"), "alt");
        assert_eq!(
            hidden("go to https://example.com."),
            "go to https://example.com."
        );
        assert_eq!(
            parse_inline("go https://e.com/a.", 0, 19).links[0].1,
            "https://e.com/a"
        );
        assert_eq!(hidden("[[Page|alias]] and [[Other]]"), "alias and Other");
        assert_eq!(hidden("<https://a.b>"), "https://a.b");
        let e = parse_inline("a &amp; b &#x41; &bogus;", 0, 24);
        assert_eq!(e.subs, vec![(2..7, '&'), (10..16, 'A')]);
        let c = parse_inline("x <!-- *no* --> y", 0, 17);
        assert_eq!(c.spans, vec![(2..15, Style::Comment)]);
        let doc = lines("<!--\nhidden\n-->\ntext");
        let k: Vec<Kind> = classify(&doc).iter().map(|i| i.kind).collect();
        assert_eq!(
            k,
            vec![Kind::Comment, Kind::Comment, Kind::Comment, Kind::Para]
        );
    }

    #[test]
    fn blocks() {
        let doc = lines(
            "# Title\n\n- a\n  - b\n    text\n1. one\n> quote\n> - item\n```rs\ncode\n```\n---\n| a | b |\n|---|:-:|\n| 1 | 2 |\npara",
        );
        let k: Vec<Kind> = classify(&doc).iter().map(|i| i.kind).collect();
        assert_eq!(k[0], Kind::Heading(1));
        assert_eq!(k[1], Kind::Blank);
        assert!(matches!(k[2], Kind::List { ordered: false, .. }));
        let infos = classify(&doc);
        assert_eq!(infos[3].level, 1);
        assert_eq!(infos[4].kind, Kind::Para);
        assert_eq!(infos[4].level, 2);
        assert_eq!(infos[5].level, 0);
        assert_eq!(infos[6].quote, 1);
        assert_eq!(infos[6].content, 2);
        assert!(matches!(infos[7].kind, Kind::List { .. }));
        assert_eq!(infos[7].quote, 1);
        assert_eq!(k[8], Kind::Fence);
        assert_eq!(k[9], Kind::Code);
        assert_eq!(k[10], Kind::Fence);
        assert_eq!(k[11], Kind::Rule);
        assert_eq!(k[12], Kind::Table { row: 0 });
        assert_eq!(k[13], Kind::Table { row: 1 });
        assert_eq!(k[14], Kind::Table { row: 2 });
        assert_eq!(k[15], Kind::Para);
    }

    #[test]
    fn tasks_and_frontmatter() {
        let doc = lines("---\ntitle: x\n---\n- [ ] todo\n- [x] done");
        let infos = classify(&doc);
        assert_eq!(infos[0].kind, Kind::Fence);
        assert_eq!(infos[1].kind, Kind::Code);
        assert_eq!(
            infos[3].kind,
            Kind::List {
                ordered: false,
                task: Some(false)
            }
        );
        assert_eq!(&doc[3][infos[3].content..], "todo");
        assert_eq!(
            infos[4].kind,
            Kind::List {
                ordered: false,
                task: Some(true)
            }
        );
    }

    #[test]
    fn cells() {
        let l = "| a | `x|y` | c |";
        let c: Vec<&str> = split_cells(l, 0).into_iter().map(|r| &l[r]).collect();
        assert_eq!(c, vec!["a", "`x|y`", "c"]);
        // Empty cells sit after the first padding space.
        assert_eq!(split_cells("|   | b |", 0), vec![2..2, 6..7]);
        assert_eq!(split_cells("|| b |", 0), vec![1..1, 3..4]);
        assert_eq!(
            delim_row("|:--|:-:|--:|"),
            Some(vec![Align::Left, Align::Center, Align::Right])
        );
    }

    #[test]
    fn words() {
        assert_eq!(count_words("hello world"), 2);
        assert_eq!(count_words("你好 world"), 3);
        assert_eq!(count_words("# Title - item"), 2);
    }
}
