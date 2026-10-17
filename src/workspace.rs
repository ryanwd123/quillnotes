//! Folders (workspaces): scanning the file tree, recent folders, and per-folder
//! settings such as autosave.

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};

use crate::doc::{self, Document, Stamp};
use crate::markdown::{self, Kind};
use crate::palette::term_match;
use crate::settings::Settings;

/// Notes larger than this are not read when searching or collecting links.
pub const READ_MAX_BYTES: u64 = 4 << 20;

/// One file or folder under the open folder, in tree order.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub name: String,
    /// Depth below the root (children of the root are 0).
    pub depth: usize,
    pub is_dir: bool,
}

/// Stop scanning huge folders at this many entries.
const SCAN_LIMIT: usize = 20_000;

/// Folders not shown: hidden ones (`.git`, `.obsidian`, ...) and dependency caches.
fn skipped(name: &str) -> bool {
    name.starts_with('.') || name == "node_modules"
}

/// All entries under `root`, folders before files, each level sorted by name.
pub fn scan(root: &Path) -> Vec<Entry> {
    let mut out = Vec::new();
    scan_dir(root, 0, &mut out);
    out
}

fn scan_dir(dir: &Path, depth: usize, out: &mut Vec<Entry>) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut items: Vec<(String, PathBuf, bool)> = read
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let is_dir = e.file_type().ok()?.is_dir();
            (!skipped(&name)).then_some((name, e.path(), is_dir))
        })
        .collect();
    items.sort_by(|a, b| {
        b.2.cmp(&a.2)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });
    for (name, path, is_dir) in items {
        if out.len() >= SCAN_LIMIT {
            return;
        }
        out.push(Entry {
            path: path.clone(),
            name,
            depth,
            is_dir,
        });
        if is_dir {
            scan_dir(&path, depth + 1, out);
        }
    }
}

/// The folder pasted and dropped files are copied into, next to the note.
pub const DATA_DIR: &str = "data";

/// Markdown files, by extension.
pub fn is_markdown(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "md" | "markdown" | "mdown" | "mkd"
        )
    })
}

/// Files Penmark opens itself (others open in their default app).
pub fn is_text(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "md" | "markdown" | "mdown" | "mkd" | "txt"
        )
    })
}

/// A note offered when typing `[[`.
#[derive(Clone, Debug, PartialEq)]
pub struct NoteLink {
    /// File name without the extension.
    pub name: String,
    /// Folder, relative to the root (empty for the root).
    pub folder: String,
    /// Text that goes inside `[[...]]`: the name when no other note shares it, else
    /// the path from the root, both without the extension.
    pub target: String,
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// The folder's Markdown notes as `[[...]]` targets, except `exclude`.
pub fn note_links(root: &Path, entries: &[Entry], exclude: Option<&Path>) -> Vec<NoteLink> {
    let notes: Vec<&Entry> = entries
        .iter()
        .filter(|e| !e.is_dir && is_markdown(&e.path))
        .collect();
    let mut count = std::collections::HashMap::new();
    for e in &notes {
        *count.entry(stem(&e.path).to_lowercase()).or_insert(0) += 1;
    }
    notes
        .into_iter()
        .filter(|e| exclude != Some(e.path.as_path()))
        .map(|e| {
            let name = stem(&e.path);
            let rel = relative(root, &e.path);
            let target = if count[&name.to_lowercase()] > 1 {
                rel[..rel.len() - e.path.extension().map_or(0, |x| x.len() + 1)].to_string()
            } else {
                name.clone()
            };
            NoteLink {
                folder: e
                    .path
                    .parent()
                    .map(|p| relative(root, p))
                    .unwrap_or_default(),
                name,
                target,
            }
        })
        .collect()
}

/// Notes matching `query` (typed after `[[`), best first, at most `max`, with the
/// matched byte offsets in each name.
pub fn filter_notes(notes: &[NoteLink], query: &str, max: usize) -> Vec<(usize, Vec<usize>)> {
    let mut found: Vec<(i32, usize, Vec<usize>)> = notes
        .iter()
        .enumerate()
        .filter_map(|(i, n)| {
            let (score, hits) = term_match(query, &format!("{} {}", n.name, n.folder))?;
            let hits = hits.into_iter().filter(|&b| b < n.name.len()).collect();
            Some((score, i, hits))
        })
        .collect();
    found.sort_by(|a, b| {
        b.0.cmp(&a.0).then_with(|| {
            notes[a.1]
                .name
                .to_lowercase()
                .cmp(&notes[b.1].name.to_lowercase())
        })
    });
    found.truncate(max);
    found.into_iter().map(|(_, i, hits)| (i, hits)).collect()
}

/// The note a `[[name]]` link points to: `name` relative to `dir` (the linking
/// note's folder), then to `root`, then any note in `entries` with that file name.
pub fn resolve_note(
    root: Option<&Path>,
    entries: &[Entry],
    dir: &Path,
    name: &str,
) -> Option<PathBuf> {
    let file = if is_markdown(Path::new(name)) {
        name.to_string()
    } else {
        format!("{name}.md")
    };
    let candidates = [Some(dir), root];
    if let Some(p) = candidates
        .into_iter()
        .flatten()
        .map(|d| d.join(&file))
        .find(|p| p.is_file())
    {
        return Some(p);
    }
    let want = Path::new(&file)
        .file_name()?
        .to_string_lossy()
        .to_lowercase();
    entries
        .iter()
        .find(|e| !e.is_dir && e.name.to_lowercase() == want)
        .map(|e| e.path.clone())
}

/// Whether a link target starts with a URL scheme (`https:`, `mailto:`, `run:`).
pub fn has_scheme(target: &str) -> bool {
    target.split_once(':').is_some_and(|(s, _)| {
        s.len() > 1 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '+')
    })
}

/// `p` with `.` and `..` resolved, without touching the disk.
fn clean(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

/// Whether two paths name the same file (case-insensitive, as on Windows).
pub fn same_path(a: &Path, b: &Path) -> bool {
    normalize(&clean(a)) == normalize(&clean(b))
}

/// The links in `lines` outside code: (line, target), `[[name]]` links in that form.
pub fn links_in(lines: &[String]) -> Vec<(usize, String)> {
    let infos = markdown::classify(lines);
    let mut out = Vec::new();
    for (l, (line, info)) in lines.iter().zip(&infos).enumerate() {
        if matches!(info.kind, Kind::Code | Kind::Fence | Kind::Comment)
            || !(line.contains("[[") || line.contains("]("))
        {
            continue;
        }
        let inline = markdown::parse_inline(line, info.content, line.len());
        out.extend(inline.links.into_iter().map(|(_, target, _)| (l, target)));
    }
    out
}

/// The note a link `target` in a note in `dir` points to, when it is a link to a
/// Markdown file (whether or not it exists).
pub fn link_dest(root: &Path, entries: &[Entry], dir: &Path, target: &str) -> Option<PathBuf> {
    if let Some(name) = target.strip_prefix("[[").and_then(|t| t.strip_suffix("]]")) {
        let name = name.split('#').next().unwrap_or(name).trim();
        return resolve_note(Some(root), entries, dir, name).map(|p| clean(&p));
    }
    if has_scheme(target) || target.starts_with('#') {
        return None;
    }
    let rel = crate::attach::decode(target.split('#').next().unwrap_or(""));
    let p = clean(&dir.join(rel));
    is_markdown(&p).then_some(p)
}

/// Whether a link `target` could point to a note with this lower-case file stem;
/// a cheap test before resolving it.
fn may_link_to(target: &str, stem: &str) -> bool {
    let t = target.strip_prefix("[[").and_then(|t| t.strip_suffix("]]"));
    let name = match t {
        Some(t) => t.split(['#', '|']).next().unwrap_or(t),
        None => target.split('#').next().unwrap_or(target),
    };
    let last = name.rsplit(['/', '\\']).next().unwrap_or(name).trim().to_lowercase();
    let last = crate::attach::decode(&last);
    last == stem || Path::new(&last).file_stem().is_some_and(|s| s.to_string_lossy() == stem)
}

/// A line in another note that links to the open one.
#[derive(Clone, Debug, PartialEq)]
pub struct Backlink {
    pub path: PathBuf,
    pub line: usize,
    /// The linking line as plain text.
    pub text: String,
}

/// Each note's links, (line, target, the line as plain text), with the file's stamp
/// when they were read, so unchanged files are not read again.
pub type LinkCache = HashMap<PathBuf, (Stamp, Vec<(usize, String, String)>)>;

/// The folder's notes' links, from `cache` for files that have not changed.
pub fn note_links_cached<'a>(
    entries: &[Entry],
    cache: &'a mut LinkCache,
) -> &'a LinkCache {
    let mut seen = HashSet::new();
    for e in entries.iter().filter(|e| !e.is_dir && is_markdown(&e.path)) {
        seen.insert(e.path.clone());
        let Some(stamp) = doc::stamp(&e.path) else {
            continue;
        };
        if cache.get(&e.path).is_some_and(|c| c.0 == stamp) {
            continue;
        }
        let links = if stamp.1 > READ_MAX_BYTES {
            Vec::new()
        } else {
            let bytes = std::fs::read(&e.path).unwrap_or_default();
            let lines = Document::from_bytes(&bytes, None).0.lines;
            links_in(&lines)
                .into_iter()
                .map(|(l, t)| {
                    let text = markdown::plain_text(&lines[l], 0);
                    (l, t, text)
                })
                .collect()
        };
        cache.insert(e.path.clone(), (stamp, links));
    }
    cache.retain(|p, _| seen.contains(p));
    cache
}

/// The lines in the folder's other notes that link to `target`.
pub fn backlinks(
    root: &Path,
    entries: &[Entry],
    target: &Path,
    cache: &mut LinkCache,
) -> Vec<Backlink> {
    let stem = stem(target).to_lowercase();
    let mut out = Vec::new();
    for (path, (_, links)) in note_links_cached(entries, cache) {
        if same_path(path, target) {
            continue;
        }
        let dir = path.parent().unwrap_or(root);
        let mut last = None;
        for (l, t, text) in links {
            if last == Some(*l) || !may_link_to(t, &stem) {
                continue;
            }
            if link_dest(root, entries, dir, t).is_some_and(|d| same_path(&d, target)) {
                last = Some(*l);
                out.push(Backlink {
                    path: path.clone(),
                    line: *l,
                    text: text.trim().to_string(),
                });
            }
        }
    }
    out.sort_by(|a, b| {
        relative(root, &a.path)
            .to_lowercase()
            .cmp(&relative(root, &b.path).to_lowercase())
            .then(a.line.cmp(&b.line))
    });
    out
}

/// Indices of the entries visible in the tree: those whose parent folders are all
/// expanded. With `notes_only`, `data` folders and files other than Markdown are
/// left out.
pub fn visible(entries: &[Entry], expanded: &HashSet<PathBuf>, notes_only: bool) -> Vec<usize> {
    let mut out = Vec::new();
    let mut hide_below: Option<usize> = None;
    for (i, e) in entries.iter().enumerate() {
        if let Some(d) = hide_below {
            if e.depth > d {
                continue;
            }
            hide_below = None;
        }
        if notes_only {
            if e.is_dir && e.name.eq_ignore_ascii_case(DATA_DIR) {
                hide_below = Some(e.depth);
                continue;
            }
            if !e.is_dir && !is_markdown(&e.path) {
                continue;
            }
        }
        out.push(i);
        if e.is_dir && !expanded.contains(&e.path) {
            hide_below = Some(e.depth);
        }
    }
    out
}

/// `path` relative to `root`, with `/` separators.
pub fn relative(root: &Path, path: &Path) -> String {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Whether `path` is `root` or inside it (case-insensitive, as on Windows).
pub fn contains(root: &Path, path: &Path) -> bool {
    let (r, p) = (normalize(root), normalize(path));
    p == r || p.starts_with(&format!("{r}\\"))
}

fn normalize(p: &Path) -> String {
    p.to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .replace('/', "\\")
        .to_lowercase()
}

// ---------------------------------------------------------------------------
// Recent folders

const RECENT_KEY: &str = "recent_folders";
const RECENT_MAX: usize = 12;

/// Recently opened folders, most recent first.
pub fn recent(s: &Settings) -> Vec<PathBuf> {
    s.get(RECENT_KEY)
        .unwrap_or("")
        .split('|')
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Moves `folder` to the front of the recent list.
pub fn add_recent(s: &mut Settings, folder: &Path) {
    let mut list = recent(s);
    list.retain(|p| normalize(p) != normalize(folder));
    list.insert(0, folder.to_path_buf());
    list.truncate(RECENT_MAX);
    let joined: Vec<String> = list
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    s.set(RECENT_KEY, &joined.join("|"));
}

pub fn remove_recent(s: &mut Settings, folder: &Path) {
    let mut list = recent(s);
    list.retain(|p| normalize(p) != normalize(folder));
    let joined: Vec<String> = list
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    s.set(RECENT_KEY, &joined.join("|"));
}

// ---------------------------------------------------------------------------
// Folder settings

/// Settings that apply to one folder.
#[derive(Clone, Debug, PartialEq)]
pub struct FolderSettings {
    pub autosave: bool,
    /// Seconds after the last edit before autosaving.
    pub autosave_secs: u32,
    /// The file last open in this folder (relative path).
    pub last_file: Option<String>,
}

impl Default for FolderSettings {
    fn default() -> Self {
        FolderSettings {
            autosave: false,
            autosave_secs: 5,
            last_file: None,
        }
    }
}

fn key(folder: &Path, name: &str) -> String {
    format!("folder[{}].{name}", normalize(folder))
}

impl FolderSettings {
    pub fn load(s: &Settings, folder: &Path) -> Self {
        let mut f = FolderSettings::default();
        if let Some(v) = s.get(&key(folder, "autosave")) {
            f.autosave = v == "on";
        }
        if let Some(v) = s
            .get(&key(folder, "autosave_secs"))
            .and_then(|v| v.parse().ok())
        {
            f.autosave_secs = clamp_secs(v);
        }
        f.last_file = s
            .get(&key(folder, "last_file"))
            .filter(|v| !v.is_empty())
            .map(String::from);
        f
    }

    pub fn store(&self, s: &mut Settings, folder: &Path) {
        s.set(
            &key(folder, "autosave"),
            if self.autosave { "on" } else { "off" },
        );
        s.set(
            &key(folder, "autosave_secs"),
            &self.autosave_secs.to_string(),
        );
        s.set(
            &key(folder, "last_file"),
            self.last_file.as_deref().unwrap_or(""),
        );
    }
}

fn clamp_secs(v: u32) -> u32 {
    v.clamp(1, 600)
}

/// A folder setting listed in the palette.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FolderOpt {
    Autosave,
    AutosaveDelay,
}

impl FolderOpt {
    pub const ALL: [FolderOpt; 2] = [FolderOpt::Autosave, FolderOpt::AutosaveDelay];

    pub fn label(self) -> &'static str {
        match self {
            FolderOpt::Autosave => "Autosave",
            FolderOpt::AutosaveDelay => "Autosave delay (seconds after the last edit)",
        }
    }

    pub fn value_text(self, f: &FolderSettings) -> String {
        match self {
            FolderOpt::Autosave => (if f.autosave { "☑ On" } else { "☐ Off" }).into(),
            FolderOpt::AutosaveDelay => format!("‹ {} s ›", f.autosave_secs),
        }
    }

    /// Left/Right: `steps` is ±1 (or ±5 with Shift).
    pub fn adjust(self, f: &mut FolderSettings, steps: i32) {
        match self {
            FolderOpt::Autosave => f.autosave = steps > 0,
            FolderOpt::AutosaveDelay => {
                f.autosave_secs = clamp_secs((f.autosave_secs as i32 + steps).max(1) as u32);
            }
        }
    }

    /// Enter / click toggles switches.
    pub fn activate(self, f: &mut FolderSettings) {
        if self == FolderOpt::Autosave {
            f.autosave = !f.autosave;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_tree() -> PathBuf {
        let root = std::env::temp_dir().join(format!("penmark-ws-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["notes/2024", "B", ".git", "node_modules"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in [
            "readme.md",
            "a.txt",
            "notes/todo.md",
            "notes/2024/jan.md",
            "B/image.png",
            ".git/config",
        ] {
            std::fs::write(root.join(f), "x").unwrap();
        }
        root
    }

    #[test]
    fn scan_orders_and_skips() {
        let root = temp_tree();
        let names: Vec<(usize, String)> =
            scan(&root).into_iter().map(|e| (e.depth, e.name)).collect();
        assert_eq!(
            names,
            vec![
                (0, "B".to_string()),
                (1, "image.png".to_string()),
                (0, "notes".to_string()),
                (1, "2024".to_string()),
                (2, "jan.md".to_string()),
                (1, "todo.md".to_string()),
                (0, "a.txt".to_string()),
                (0, "readme.md".to_string()),
            ]
        );
        let entries = scan(&root);
        // Collapsed folders hide their contents.
        let mut expanded = HashSet::new();
        let shown = |exp: &HashSet<PathBuf>| {
            visible(&entries, exp, false)
                .iter()
                .map(|&i| entries[i].name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(shown(&expanded), vec!["B", "notes", "a.txt", "readme.md"]);
        expanded.insert(root.join("notes"));
        assert_eq!(
            shown(&expanded),
            vec!["B", "notes", "2024", "todo.md", "a.txt", "readme.md"]
        );
        // Notes only: no data folders or non-Markdown files.
        std::fs::create_dir_all(root.join("notes/data")).unwrap();
        std::fs::write(root.join("notes/data/x.md"), "x").unwrap();
        let entries = scan(&root);
        let notes: Vec<&str> = visible(&entries, &expanded, true)
            .iter()
            .map(|&i| entries[i].name.as_str())
            .collect();
        assert_eq!(notes, vec!["B", "notes", "2024", "todo.md", "readme.md"]);
        assert_eq!(
            relative(&root, &root.join("notes").join("2024").join("jan.md")),
            "notes/2024/jan.md"
        );
        assert!(contains(&root, &root.join("notes")));
        assert!(!contains(&root.join("notes"), &root.join("notes2")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn finds_backlinks() {
        let root = std::env::temp_dir().join(format!("penmark-back-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("a")).unwrap();
        let files = [
            ("plan.md", "# Plan"),
            ("a/one.md", "See [[plan]] and [[Plan|the plan]].\n\n```\n[[plan]]\n```"),
            ("a/two.md", "x\n[the plan](../plan.md#goals)\n[web](https://plan.md)"),
            ("three.md", "[[a/one]] and [[nothing]]"),
        ];
        for (f, text) in files {
            std::fs::write(root.join(f), text).unwrap();
        }
        let entries = scan(&root);
        let mut cache = LinkCache::new();
        let found = |cache: &mut LinkCache, f: &str| -> Vec<(String, usize)> {
            backlinks(&root, &entries, &root.join(f), cache)
                .into_iter()
                .map(|b| (relative(&root, &b.path), b.line))
                .collect()
        };
        // One row per line; links in code are skipped.
        assert_eq!(found(&mut cache, "plan.md"), [("a/one.md".into(), 0), ("a/two.md".into(), 1)]);
        assert_eq!(found(&mut cache, "a/one.md"), [("three.md".into(), 0)]);
        assert!(found(&mut cache, "a/two.md").is_empty());
        let b = backlinks(&root, &entries, &root.join("plan.md"), &mut cache);
        assert_eq!(b[0].text, "See plan and the plan.");
        // Changed files (here, a new size) are read again.
        std::fs::write(root.join("a/two.md"), "nothing").unwrap();
        assert_eq!(found(&mut cache, "plan.md"), [("a/one.md".into(), 0)]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn note_links_and_resolution() {
        let root = std::env::temp_dir().join(format!("penmark-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for d in ["a", "b"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in ["todo.md", "a/todo.md", "a/plan.md", "b/pic.png"] {
            std::fs::write(root.join(f), "x").unwrap();
        }
        let entries = scan(&root);
        let links = note_links(&root, &entries, Some(&root.join("todo.md")));
        let t: Vec<(&str, &str, &str)> = links
            .iter()
            .map(|l| (l.name.as_str(), l.folder.as_str(), l.target.as_str()))
            .collect();
        assert_eq!(t, vec![("plan", "a", "plan"), ("todo", "a", "a/todo")]);
        let names = |q: &str| -> Vec<String> {
            filter_notes(&links, q, 10)
                .iter()
                .map(|(i, _)| links[*i].target.clone())
                .collect()
        };
        assert_eq!(names("pl"), vec!["plan"]);
        assert_eq!(names(""), vec!["plan", "a/todo"]);
        assert_eq!(names("a todo"), vec!["a/todo"]);
        // Next to the note, then from the root, then by file name anywhere.
        let r = |dir: &str, name: &str| {
            resolve_note(Some(&root), &entries, &root.join(dir), name).map(|p| relative(&root, &p))
        };
        assert_eq!(r("a", "todo").as_deref(), Some("a/todo.md"));
        assert_eq!(r("b", "todo").as_deref(), Some("todo.md"));
        assert_eq!(r("b", "a/plan").as_deref(), Some("a/plan.md"));
        assert_eq!(r("b", "plan").as_deref(), Some("a/plan.md"));
        assert_eq!(r("b", "nope"), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn recents_and_settings() {
        let mut s = Settings::default();
        add_recent(&mut s, Path::new(r"C:\a"));
        add_recent(&mut s, Path::new(r"C:\b"));
        add_recent(&mut s, Path::new(r"c:\A\"));
        assert_eq!(
            recent(&s),
            vec![PathBuf::from(r"c:\A\"), PathBuf::from(r"C:\b")]
        );
        remove_recent(&mut s, Path::new(r"C:\B"));
        assert_eq!(recent(&s).len(), 1);

        let folder = Path::new(r"C:\Notes");
        let mut f = FolderSettings::load(&s, folder);
        assert_eq!(f, FolderSettings::default());
        FolderOpt::Autosave.activate(&mut f);
        FolderOpt::AutosaveDelay.adjust(&mut f, -10);
        f.last_file = Some("notes/todo.md".into());
        f.store(&mut s, folder);
        assert_eq!(FolderSettings::load(&s, Path::new(r"c:\notes\")), f);
        assert_eq!(f.autosave_secs, 1);
        assert!(is_text(Path::new("x.MD")) && !is_text(Path::new("x.png")));
    }
}
