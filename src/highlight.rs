//! Syntax highlighting for fenced code blocks: a small tokenizer driven by a table of
//! languages (keywords, comment and string syntax). Unknown languages stay plain.

use std::ops::Range;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tok {
    Keyword,
    Str,
    Comment,
    Number,
    Type,
}

/// Where a line starts: in code, inside a block comment, or inside a string that
/// runs on (a triple-quoted or backtick string), with its quote.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum State {
    #[default]
    Code,
    Block,
    Str {
        quote: u8,
        triple: bool,
    },
}

pub struct Lang {
    names: &'static [&'static str],
    keywords: &'static [&'static str],
    types: &'static [&'static str],
    line_comments: &'static [&'static str],
    block_comment: Option<(&'static str, &'static str)>,
    quotes: &'static [u8],
    /// Python's `"""` / `'''` strings, which span lines.
    triple: bool,
    /// Backtick strings span lines (JavaScript template strings, Go raw strings).
    multiline_backtick: bool,
    /// Capitalized names are types (Rust, TypeScript, Go, C#, Java).
    caps_types: bool,
    /// Keywords match in any case (SQL, PowerShell).
    any_case: bool,
    /// `<tag` and `</tag` names are keywords (HTML, XML).
    tags: bool,
    /// `$name` (PowerShell) or `#name` (M) is one word, so `$true` and `#date` can be
    /// keywords; a quote after it starts a quoted name (`#"Changed Type"`), left plain.
    sigil: Option<u8>,
    /// `\` escapes the next character in strings. M strings end at the first quote
    /// (`""` inside one reads as two strings side by side, which looks the same).
    backslash_escapes: bool,
}

/// Languages are only ever the entries of `LANGS`, so one is equal only to itself.
impl PartialEq for Lang {
    fn eq(&self, other: &Self) -> bool {
        std::ptr::eq(self, other)
    }
}

const C_TYPES: &[&str] = &[
    "bool", "char", "double", "float", "int", "long", "short", "signed", "unsigned", "void",
    "size_t", "int8_t", "int16_t", "int32_t", "int64_t", "uint8_t", "uint16_t", "uint32_t",
    "uint64_t", "string", "byte", "object", "decimal", "var", "auto",
];

const LANGS: &[Lang] = &[
    Lang {
        names: &["rust", "rs"],
        keywords: &[
            "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum",
            "extern", "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod",
            "move", "mut", "pub", "ref", "return", "self", "Self", "static", "struct", "super",
            "trait", "true", "type", "unsafe", "use", "where", "while",
        ],
        types: &[
            "bool", "char", "f32", "f64", "i8", "i16", "i32", "i64", "i128", "isize", "str", "u8",
            "u16", "u32", "u64", "u128", "usize",
        ],
        line_comments: &["//"],
        block_comment: Some(("/*", "*/")),
        quotes: b"\"",
        triple: false,
        multiline_backtick: false,
        caps_types: true,
        any_case: false,
        tags: false,
        sigil: None,
        backslash_escapes: true,
    },
    Lang {
        names: &["python", "py"],
        keywords: &[
            "False", "None", "True", "and", "as", "assert", "async", "await", "break", "class",
            "continue", "def", "del", "elif", "else", "except", "finally", "for", "from", "global",
            "if", "import", "in", "is", "lambda", "nonlocal", "not", "or", "pass", "raise",
            "return", "self", "try", "while", "with", "yield",
        ],
        types: &[
            "bool", "bytes", "dict", "float", "int", "list", "object", "set", "str", "tuple",
        ],
        line_comments: &["#"],
        block_comment: None,
        quotes: b"\"'",
        triple: true,
        multiline_backtick: false,
        caps_types: false,
        any_case: false,
        tags: false,
        sigil: None,
        backslash_escapes: true,
    },
    Lang {
        names: &[
            "javascript",
            "js",
            "jsx",
            "mjs",
            "typescript",
            "ts",
            "tsx",
            "json5",
        ],
        keywords: &[
            "async",
            "await",
            "break",
            "case",
            "catch",
            "class",
            "const",
            "continue",
            "default",
            "delete",
            "do",
            "else",
            "export",
            "extends",
            "false",
            "finally",
            "for",
            "from",
            "function",
            "if",
            "import",
            "in",
            "instanceof",
            "interface",
            "let",
            "new",
            "null",
            "of",
            "return",
            "static",
            "super",
            "switch",
            "this",
            "throw",
            "true",
            "try",
            "type",
            "typeof",
            "undefined",
            "var",
            "void",
            "while",
            "yield",
            "enum",
            "implements",
            "private",
            "protected",
            "public",
            "readonly",
            "as",
        ],
        types: &[
            "any", "boolean", "never", "number", "object", "string", "symbol", "unknown",
        ],
        line_comments: &["//"],
        block_comment: Some(("/*", "*/")),
        quotes: b"\"'`",
        triple: false,
        multiline_backtick: true,
        caps_types: true,
        any_case: false,
        tags: false,
        sigil: None,
        backslash_escapes: true,
    },
    Lang {
        names: &[
            "c", "h", "cpp", "c++", "cc", "hpp", "cs", "csharp", "c#", "java", "kotlin", "kt",
            "swift",
        ],
        keywords: &[
            "abstract",
            "break",
            "case",
            "catch",
            "class",
            "const",
            "continue",
            "default",
            "delete",
            "do",
            "else",
            "enum",
            "extends",
            "extern",
            "false",
            "final",
            "finally",
            "for",
            "foreach",
            "fun",
            "func",
            "goto",
            "if",
            "implements",
            "import",
            "in",
            "include",
            "define",
            "interface",
            "let",
            "namespace",
            "new",
            "null",
            "nullptr",
            "override",
            "package",
            "private",
            "protected",
            "public",
            "return",
            "sizeof",
            "static",
            "struct",
            "switch",
            "template",
            "this",
            "throw",
            "true",
            "try",
            "typedef",
            "typename",
            "union",
            "using",
            "val",
            "virtual",
            "volatile",
            "while",
        ],
        types: C_TYPES,
        line_comments: &["//"],
        block_comment: Some(("/*", "*/")),
        quotes: b"\"'",
        triple: false,
        multiline_backtick: false,
        caps_types: true,
        any_case: false,
        tags: false,
        sigil: None,
        backslash_escapes: true,
    },
    Lang {
        names: &["go", "golang"],
        keywords: &[
            "break",
            "case",
            "chan",
            "const",
            "continue",
            "default",
            "defer",
            "else",
            "fallthrough",
            "false",
            "for",
            "func",
            "go",
            "goto",
            "if",
            "import",
            "interface",
            "map",
            "nil",
            "package",
            "range",
            "return",
            "select",
            "struct",
            "switch",
            "true",
            "type",
            "var",
        ],
        types: &[
            "bool", "byte", "error", "float32", "float64", "int", "int8", "int16", "int32",
            "int64", "rune", "string", "uint", "uint8", "uint16", "uint32", "uint64", "any",
        ],
        line_comments: &["//"],
        block_comment: Some(("/*", "*/")),
        quotes: b"\"'`",
        triple: false,
        multiline_backtick: true,
        caps_types: true,
        any_case: false,
        tags: false,
        sigil: None,
        backslash_escapes: true,
    },
    Lang {
        names: &["sh", "bash", "shell", "zsh", "console"],
        keywords: &[
            "case", "do", "done", "echo", "elif", "else", "esac", "exit", "export", "fi", "for",
            "function", "if", "in", "local", "return", "set", "then", "until", "while", "cd",
        ],
        types: &[],
        line_comments: &["#"],
        block_comment: None,
        quotes: b"\"'",
        triple: false,
        multiline_backtick: false,
        caps_types: false,
        any_case: false,
        tags: false,
        sigil: None,
        backslash_escapes: true,
    },
    Lang {
        names: &["powershell", "ps1", "pwsh", "ps"],
        keywords: &[
            "begin", "break", "catch", "class", "continue", "do", "else", "elseif", "end", "exit",
            "filter", "finally", "for", "foreach", "function", "if", "in", "param", "process",
            "return", "switch", "throw", "try", "until", "while", "$true", "$false", "$null",
        ],
        types: &[],
        line_comments: &["#"],
        block_comment: Some(("<#", "#>")),
        quotes: b"\"'",
        triple: false,
        multiline_backtick: false,
        caps_types: false,
        any_case: true,
        tags: false,
        sigil: Some(b'$'),
        backslash_escapes: true,
    },
    Lang {
        names: &["json", "jsonc"],
        keywords: &["true", "false", "null"],
        types: &[],
        line_comments: &["//"],
        block_comment: Some(("/*", "*/")),
        quotes: b"\"",
        triple: false,
        multiline_backtick: false,
        caps_types: false,
        any_case: false,
        tags: false,
        sigil: None,
        backslash_escapes: true,
    },
    Lang {
        names: &["toml", "ini", "cfg", "conf", "yaml", "yml"],
        keywords: &["true", "false", "null", "yes", "no", "on", "off"],
        types: &[],
        line_comments: &["#", ";"],
        block_comment: None,
        quotes: b"\"'",
        triple: false,
        multiline_backtick: false,
        caps_types: false,
        any_case: false,
        tags: false,
        sigil: None,
        backslash_escapes: true,
    },
    Lang {
        names: &["sql", "sqlite", "postgres", "mysql"],
        keywords: &[
            "add", "all", "alter", "and", "as", "asc", "between", "by", "case", "create", "delete",
            "desc", "distinct", "drop", "else", "end", "exists", "from", "group", "having", "in",
            "index", "inner", "insert", "into", "is", "join", "key", "left", "like", "limit",
            "not", "null", "on", "or", "order", "outer", "primary", "right", "select", "set",
            "table", "then", "union", "update", "values", "view", "when", "where", "with",
        ],
        types: &[
            "bigint",
            "blob",
            "boolean",
            "char",
            "date",
            "datetime",
            "decimal",
            "float",
            "int",
            "integer",
            "numeric",
            "real",
            "text",
            "timestamp",
            "varchar",
        ],
        line_comments: &["--"],
        block_comment: Some(("/*", "*/")),
        quotes: b"'\"",
        triple: false,
        multiline_backtick: false,
        caps_types: false,
        any_case: true,
        tags: false,
        sigil: None,
        backslash_escapes: true,
    },
    Lang {
        names: &["html", "xml", "svg", "xaml", "vue"],
        keywords: &[],
        types: &[],
        line_comments: &[],
        block_comment: Some(("<!--", "-->")),
        quotes: b"\"'",
        triple: false,
        multiline_backtick: false,
        caps_types: false,
        any_case: false,
        tags: true,
        sigil: None,
        backslash_escapes: true,
    },
    Lang {
        names: &["powerquery", "pq", "m", "mquery"],
        keywords: &[
            "and",
            "as",
            "each",
            "else",
            "error",
            "false",
            "if",
            "in",
            "is",
            "let",
            "meta",
            "not",
            "null",
            "or",
            "otherwise",
            "section",
            "shared",
            "then",
            "true",
            "try",
            "type",
            "#binary",
            "#date",
            "#datetime",
            "#datetimezone",
            "#duration",
            "#infinity",
            "#nan",
            "#sections",
            "#shared",
            "#table",
            "#time",
        ],
        types: &[
            "any",
            "anynonnull",
            "binary",
            "date",
            "datetime",
            "datetimezone",
            "duration",
            "function",
            "list",
            "logical",
            "none",
            "nullable",
            "number",
            "optional",
            "record",
            "table",
            "text",
            "time",
        ],
        line_comments: &["//"],
        block_comment: Some(("/*", "*/")),
        quotes: b"\"",
        triple: false,
        multiline_backtick: false,
        caps_types: false,
        any_case: false,
        tags: false,
        sigil: Some(b'#'),
        backslash_escapes: false,
    },
];

/// The language named after a code fence (```rust), if it is one we highlight.
pub fn lang(info: &str) -> Option<&'static Lang> {
    let name = info
        .trim_start_matches(['`', '~'])
        .trim_start()
        .split(|c: char| c.is_whitespace() || c == '{' || c == ',')
        .next()?
        .to_ascii_lowercase();
    if name.is_empty() {
        return None;
    }
    LANGS.iter().find(|l| l.names.contains(&name.as_str()))
}

/// An id for the language, for cache keys.
pub fn id(lang: &'static Lang) -> usize {
    lang as *const Lang as usize
}

fn is_ident(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c >= 0x80
}

/// The tokens on `line`, as byte ranges, starting in `state`; `state` becomes the
/// state at the end of the line.
pub fn spans(lang: &Lang, line: &str, state: &mut State) -> Vec<(Range<usize>, Tok)> {
    let b = line.as_bytes();
    let n = b.len();
    let mut out = Vec::new();
    let mut i = 0;
    // Finish what the previous line left open.
    match *state {
        State::Code => {}
        State::Block => {
            let (_, close) = lang.block_comment.unwrap_or(("", ""));
            match find(line, close, 0) {
                Some(e) => {
                    out.push((0..e + close.len(), Tok::Comment));
                    i = e + close.len();
                    *state = State::Code;
                }
                None => return vec![(0..n, Tok::Comment)],
            }
        }
        State::Str { quote, triple } => {
            match close_string(b, 0, quote, triple, lang.backslash_escapes) {
                Some(e) => {
                    out.push((0..e, Tok::Str));
                    i = e;
                    *state = State::Code;
                }
                None => return vec![(0..n, Tok::Str)],
            }
        }
    }
    while i < n {
        let rest = &line[i..];
        let c = b[i];
        if lang.line_comments.iter().any(|p| rest.starts_with(p)) {
            out.push((i..n, Tok::Comment));
            break;
        }
        if let Some((open, close)) = lang.block_comment
            && rest.starts_with(open)
        {
            match find(line, close, i + open.len()) {
                Some(e) => {
                    out.push((i..e + close.len(), Tok::Comment));
                    i = e + close.len();
                }
                None => {
                    out.push((i..n, Tok::Comment));
                    *state = State::Block;
                    break;
                }
            }
            continue;
        }
        if lang.quotes.contains(&c) {
            let triple = lang.triple && b[i..].starts_with(&[c, c, c]);
            let start = i + if triple { 3 } else { 1 };
            match close_string(b, start, c, triple, lang.backslash_escapes) {
                Some(e) => {
                    out.push((i..e, Tok::Str));
                    i = e;
                }
                None => {
                    out.push((i..n, Tok::Str));
                    if triple || (c == b'`' && lang.multiline_backtick) {
                        *state = State::Str { quote: c, triple };
                    }
                    break;
                }
            }
            continue;
        }
        if lang.tags && c == b'<' {
            let s = i + 1 + usize::from(b.get(i + 1) == Some(&b'/'));
            let e = s + b[s..]
                .iter()
                .take_while(|&&c| is_ident(c) || c == b'-' || c == b':')
                .count();
            if e > s {
                out.push((s..e, Tok::Keyword));
                i = e;
                continue;
            }
        }
        let starts_word = i == 0 || !is_ident(b[i - 1]);
        if c.is_ascii_digit() && starts_word {
            let e = i + b[i..]
                .iter()
                .take_while(|&&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.')
                .count();
            out.push((i..e, Tok::Number));
            i = e;
            continue;
        }
        let sigil = usize::from(lang.sigil == Some(c));
        if sigil == 1
            && let Some(&q) = b.get(i + 1)
            && lang.quotes.contains(&q)
        {
            // A quoted name: skip it, or the rest of the line if it isn't closed.
            i = close_string(b, i + 2, q, false, lang.backslash_escapes).unwrap_or(n);
            continue;
        }
        if starts_word && (is_ident(c) || sigil == 1) {
            let e = i + sigil + b[i + sigil..].iter().take_while(|&&c| is_ident(c)).count();
            let word = &line[i..e];
            let is = |list: &[&str]| {
                if lang.any_case {
                    list.iter().any(|k| k.eq_ignore_ascii_case(word))
                } else {
                    list.contains(&word)
                }
            };
            if is(lang.keywords) {
                out.push((i..e, Tok::Keyword));
            } else if is(lang.types)
                || (lang.caps_types && sigil == 0 && word.as_bytes()[0].is_ascii_uppercase())
            {
                out.push((i..e, Tok::Type));
            }
            i = e.max(i + 1);
            continue;
        }
        i += 1;
        while i < n && !line.is_char_boundary(i) {
            i += 1;
        }
    }
    out
}

fn find(line: &str, pat: &str, from: usize) -> Option<usize> {
    line.get(from..)?.find(pat).map(|k| from + k)
}

/// Just past the quote closing a string that started before `from`, skipping
/// backslash escapes if the language has them; None when the line ends first.
fn close_string(b: &[u8], from: usize, quote: u8, triple: bool, escapes: bool) -> Option<usize> {
    let mut i = from;
    while i < b.len() {
        if escapes && b[i] == b'\\' {
            i += 2;
            continue;
        }
        if b[i] == quote && (!triple || b[i..].starts_with(&[quote, quote, quote])) {
            return Some(i + if triple { 3 } else { 1 });
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(lang: &str, lines: &[&str]) -> Vec<Vec<(String, Tok)>> {
        let l = super::lang(lang).unwrap();
        let mut st = State::Code;
        lines
            .iter()
            .map(|line| {
                spans(l, line, &mut st)
                    .into_iter()
                    .map(|(r, t)| (line[r].to_string(), t))
                    .collect()
            })
            .collect()
    }

    fn t(s: &str, tok: Tok) -> (String, Tok) {
        (s.to_string(), tok)
    }

    #[test]
    fn finds_languages() {
        assert!(lang("```rust").is_some());
        assert!(lang("~~~ Python title=x").is_some());
        assert!(lang("```{.js}").is_none());
        assert!(lang("```").is_none());
        assert!(lang("```cobol").is_none());
    }

    #[test]
    fn rust_tokens() {
        use Tok::*;
        assert_eq!(
            toks("rs", &[r#"let x: Vec<u8> = f("a\"b", 0x1F); // done"#]),
            [vec![
                t("let", Keyword),
                t("Vec", Type),
                t("u8", Type),
                t(r#""a\"b""#, Str),
                t("0x1F", Number),
                t("// done", Comment),
            ]]
        );
        // Block comments carry over to the next lines; words inside names don't match.
        assert_eq!(
            toks("rust", &["a /* one", "two */ fn x1() {}"]),
            [
                vec![t("/* one", Comment)],
                vec![t("two */", Comment), t("fn", Keyword)],
            ]
        );
    }

    #[test]
    fn strings_that_span_lines() {
        use Tok::*;
        assert_eq!(
            toks("py", &["s = '''a", "b''' if x"]),
            [vec![t("'''a", Str)], vec![t("b'''", Str), t("if", Keyword)]]
        );
        assert_eq!(
            toks("js", &["const s = `a", "b`;"]),
            [vec![t("const", Keyword), t("`a", Str)], vec![t("b`", Str)]]
        );
        // An unclosed ordinary quote ends with the line.
        assert_eq!(
            toks("js", &["x = 'a", "if"]),
            [vec![t("'a", Str)], vec![t("if", Keyword)]]
        );
    }

    #[test]
    fn other_languages() {
        use Tok::*;
        assert_eq!(
            toks("sql", &["SELECT id FROM t -- note"]),
            [vec![
                t("SELECT", Keyword),
                t("FROM", Keyword),
                t("-- note", Comment)
            ]]
        );
        assert_eq!(
            toks("ps1", &["if ($x -eq $TRUE) { }"]),
            [vec![t("if", Keyword), t("$TRUE", Keyword)]]
        );
        assert_eq!(
            toks("html", &["<a href=\"x\"><!-- c --></a>"]),
            [vec![
                t("a", Keyword),
                t("\"x\"", Str),
                t("<!-- c -->", Comment),
                t("a", Keyword),
            ]]
        );
        assert_eq!(
            toks("sh", &["echo $HOME # hi"]),
            [vec![t("echo", Keyword), t("# hi", Comment)]]
        );
    }

    #[test]
    fn power_query_m() {
        use Tok::*;
        // `#"..."` names stay plain, `#date` is a keyword, and a backslash does not
        // escape the closing quote.
        assert_eq!(
            toks(
                "powerquery",
                &[
                    r#"    #"Changed Type" = Csv.Document(File.Contents("C:\data\"), #date(2024, 1, 2)),"#
                ]
            ),
            [vec![
                t(r#""C:\data\""#, Str),
                t("#date", Keyword),
                t("2024", Number),
                t("1", Number),
                t("2", Number),
            ]]
        );
        assert_eq!(
            toks(
                "pq",
                &["let x = each [a] as number /* c */ in try x otherwise null // done"]
            ),
            [vec![
                t("let", Keyword),
                t("each", Keyword),
                t("as", Keyword),
                t("number", Type),
                t("/* c */", Comment),
                t("in", Keyword),
                t("try", Keyword),
                t("otherwise", Keyword),
                t("null", Keyword),
                t("// done", Comment),
            ]]
        );
        // M is case-sensitive.
        assert_eq!(toks("m", &["Let IF"]), [vec![]]);
    }
}
