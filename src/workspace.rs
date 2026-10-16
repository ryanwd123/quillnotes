//! Folders (workspaces): scanning the file tree, recent folders, and per-folder
//! settings such as autosave.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::settings::Settings;

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

/// Files Penmark opens itself (others open in their default app).
pub fn is_text(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        matches!(
            e.to_ascii_lowercase().as_str(),
            "md" | "markdown" | "mdown" | "mkd" | "txt"
        )
    })
}

/// Indices of the entries visible in the tree: those whose parent folders are all
/// expanded.
pub fn visible(entries: &[Entry], expanded: &HashSet<PathBuf>) -> Vec<usize> {
    let mut out = Vec::new();
    let mut hide_below: Option<usize> = None;
    for (i, e) in entries.iter().enumerate() {
        if let Some(d) = hide_below {
            if e.depth > d {
                continue;
            }
            hide_below = None;
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
            visible(&entries, exp)
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
        assert_eq!(
            relative(&root, &root.join("notes").join("2024").join("jan.md")),
            "notes/2024/jan.md"
        );
        assert!(contains(&root, &root.join("notes")));
        assert!(!contains(&root.join("notes"), &root.join("notes2")));
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
