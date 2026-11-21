//! Chaos ("monkey") test: drives an editor in a hidden window with random keys,
//! typing, clicks, drags, wheel turns, resizes, focus changes and outside edits
//! to its notes, checking after every step that its state is still sound, and
//! failing if a step panics or hangs.
//!
//! Each run prints its seed and, on failure, the steps that led there. Reproduce a
//! failure, or run longer, with (PowerShell):
//!
//! ```text
//! $env:QUILL_CHAOS_SEED = 1234; $env:QUILL_CHAOS_STEPS = 50000
//! cargo test chaos -- --nocapture
//! ```
//!
//! A plain `cargo test` runs two fixed seeds, so it gives the same result every
//! time. To hunt for new bugs, `QUILL_CHAOS_RUNS = 20` runs that many seeds taken
//! from the clock.

use super::*;
use crate::platform::sandbox;
use windows::core::w;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A step that takes this long counts as a hang.
const HANG: Duration = Duration::from_secs(20);
/// How many of the slowest steps are reported.
const SLOWEST: usize = 5;
/// Steps kept for the failure report.
const HISTORY: usize = 80;

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
    fn chance(&mut self, percent: u64) -> bool {
        self.next() % 100 < percent
    }
    fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.below(xs.len())]
    }
}

#[derive(Debug)]
enum Disk {
    Append(usize, String),
    Rewrite(usize, String),
    Truncate(usize),
    Delete(usize),
    Create(String, String),
    NotUtf8(usize),
    Rename(usize, String),
}

#[derive(Debug)]
enum Ev {
    Key {
        vk: u16,
        ctrl: bool,
        shift: bool,
        alt: bool,
    },
    Text(String),
    Click {
        x: i32,
        y: i32,
        shift: bool,
        ctrl: bool,
        double: bool,
    },
    Drag {
        from: (i32, i32),
        to: (i32, i32),
    },
    RightClick(i32, i32),
    Wheel(i16, bool),
    Hover(i32, i32),
    Leave,
    Resize(u32, u32),
    Focus(bool),
    Timer,
    Navigate(bool),
    /// Ctrl+P, type part of a note's name, move down, Enter: the reported freeze.
    QuickOpen(String, usize),
    UndoRedo(usize),
    Disk(Disk),
}

/// What the watchdog sees of the worker.
struct Progress {
    step: AtomicUsize,
    log: Mutex<VecDeque<String>>,
}

fn note(rng: &mut Rng) -> String {
    const SNIPPETS: &[&str] = &[
        "# Heading\n\nSome *emphasis*, **strong**, `code`, ~~gone~~, ==marked==.\n",
        "- one\n  - two\n    - three\n- [ ] task\n  - [x] done\n1. first\n2. second\n",
        "| a | b |\n|---|:-:|\n| 1 | 2 |\n| ragged |\n| x \\| y | |\n",
        "```rust\nfn main() {}\n```\n~~~\nunclosed fence\n",
        "> quote\n> > nested\n>\n",
        "[[welcome]] [[missing note]] [[deep/nested/a#Heading]] [x](deep/b.md) []()\n",
        "![dot](img.png) ![bad](bad.png) ![none](missing.png) ![]()\n",
        "***a**b* ** _*_* `` ` `` <b>tag</b> &amp; &#x1F600; \\* <http://example.com>\n",
        "caf\u{e9} e\u{301} \u{65e5}\u{672c} \u{645}\u{631}\u{62d}\u{628}\u{627} \u{1F600}\u{200D}\u{1F4BB}\t\ttab\n",
        "---\n\n***\n\n   \n\n",
        "#\n##\n###### six\n####### seven\n",
        "",
    ];
    (0..1 + rng.below(4)).map(|_| *rng.pick(SNIPPETS)).collect()
}

/// A 1x1 PNG.
const PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
    0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
    0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8,
    0xCF, 0xC0, 0xF0, 0x1F, 0x00, 0x05, 0x00, 0x01, 0xFF, 0x89, 0x99, 0x3D, 0x1D, 0x00, 0x00,
    0x00, 0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

fn write_corpus(root: &Path, rng: &mut Rng) {
    let w = |rel: &str, bytes: &[u8]| {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, bytes).unwrap();
    };
    w("welcome.md", include_str!("../../sample.md").as_bytes());
    w("help.md", include_str!("../../assets/help.md").as_bytes());
    w("empty.md", b"");
    w("crlf.md", b"# CRLF\r\n\r\n- a\r\n- b\r\n");
    w("latin1.md", b"caf\xe9 \xff\xfe\n");
    w("img.png", PNG);
    w("bad.png", b"not a png");
    w("longline.md", "word ".repeat(10_000).as_bytes());
    let big: String = (0..300).map(|_| note(rng)).collect();
    w("big.md", big.as_bytes());
    for rel in [
        "todo.md",
        "links.md",
        "deep/b.md",
        "deep/nested/a.md",
        "Other Folder/c d.md",
        "notes.txt",
    ] {
        w(rel, note(rng).as_bytes());
    }
}

/// The notes now in the folder, for outside edits and Quick Open queries.
fn files(root: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if !p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.')) {
                    walk(&p, out);
                }
            } else if p.extension().is_some_and(|x| x == "md" || x == "txt") {
                out.push(p);
            }
        }
    }
    let mut out = Vec::new();
    walk(root, &mut out);
    out.sort();
    out
}

fn text(rng: &mut Rng, path_safe: bool) -> String {
    const CHARS: &[&str] = &[
        "a", "b", "e", "o", "t", "x", "Z", " ", " ", "#", "*", "_", "`", "[", "]", "(", ")", "|",
        "-", ">", "!", "=", "~", "+", ".", "1", "2", "&", "<", "\\", "/", ":", "\u{e9}",
        "\u{65e5}", "\u{1F600}", "\u{301}",
    ];
    (0..1 + rng.below(6))
        .map(|_| *rng.pick(CHARS))
        // Kept out of file names typed in the palette or a prompt, so a Save As or
        // rename can't name a file outside the test's folder.
        .map(|c| {
            if path_safe && matches!(c, "\\" | "/" | ":") {
                "x"
            } else {
                c
            }
        })
        .collect()
}

fn point(rng: &mut Rng, w: i32, h: i32) -> (i32, i32) {
    // Weighted towards the explorer, the header and the status bar.
    let (w, h) = (w.max(1), h.max(1));
    match rng.below(10) {
        0..=2 => (rng.below(280) as i32, rng.below(h as usize) as i32),
        3 => (rng.below(w as usize) as i32, rng.below(48) as i32),
        4 => (rng.below(w as usize) as i32, h - 1 - rng.below(30) as i32),
        _ => (
            rng.below(w as usize + 40) as i32 - 20,
            rng.below(h as usize + 40) as i32 - 20,
        ),
    }
}

fn next_event(rng: &mut Rng, a: &App, root: &Path, size: (u32, u32)) -> Ev {
    const NAV: &[VIRTUAL_KEY] = &[
        VK_LEFT, VK_RIGHT, VK_UP, VK_DOWN, VK_HOME, VK_END, VK_PRIOR, VK_NEXT, VK_BACK, VK_DELETE,
        VK_RETURN, VK_RETURN, VK_TAB, VK_ESCAPE, VK_F2, VK_SPACE,
    ];
    let (w, h) = (size.0 as i32, size.1 as i32);
    let path_safe = a.palette.is_some() || a.prompt.is_some() || a.link_edit.is_some() || a.tree_focus;
    match rng.below(100) {
        0..=33 => Ev::Text(text(rng, path_safe)),
        34..=58 => Ev::Key {
            vk: rng.pick(NAV).0,
            ctrl: rng.chance(20),
            shift: rng.chance(30),
            alt: rng.chance(3),
        },
        59..=68 => {
            // Any chord: letters, digits, F keys and punctuation with Ctrl.
            let vk = match rng.below(4) {
                0 | 1 => b'A' as u16 + rng.below(26) as u16,
                2 => b'0' as u16 + rng.below(10) as u16,
                _ => *rng.pick(&[
                    VK_F1.0, VK_F3.0, VK_F5.0, VK_F12.0, VK_OEM_PLUS.0, VK_OEM_MINUS.0,
                    VK_OEM_2.0, VK_OEM_4.0, VK_OEM_6.0, VK_OEM_COMMA.0, VK_OEM_PERIOD.0,
                ]),
            };
            Ev::Key {
                vk,
                ctrl: rng.chance(85),
                shift: rng.chance(35),
                alt: rng.chance(8),
            }
        }
        69..=72 => {
            let notes = files(root);
            let name = notes
                .get(rng.below(notes.len()))
                .and_then(|p| p.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            let n = rng.below(name.chars().count() + 1);
            Ev::QuickOpen(name.chars().take(n).collect(), rng.below(3))
        }
        73..=79 => {
            let (x, y) = point(rng, w, h);
            Ev::Click {
                x,
                y,
                shift: rng.chance(15),
                ctrl: rng.chance(10),
                double: rng.chance(15),
            }
        }
        80..=81 => Ev::Drag {
            from: point(rng, w, h),
            to: point(rng, w, h),
        },
        82 => {
            let (x, y) = point(rng, w, h);
            Ev::RightClick(x, y)
        }
        83..=85 => Ev::Wheel(
            *rng.pick(&[-360, -120, -120, 120, 120, 360, 40]),
            rng.chance(10),
        ),
        86 => {
            let (x, y) = point(rng, w, h);
            Ev::Hover(x, y)
        }
        87 => Ev::Leave,
        88 => {
            if rng.chance(15) {
                Ev::Resize(rng.below(60) as u32, rng.below(60) as u32)
            } else {
                Ev::Resize(300 + rng.below(1700) as u32, 200 + rng.below(1200) as u32)
            }
        }
        89 => Ev::Focus(rng.chance(50)),
        90..=93 => Ev::Timer,
        94 => Ev::Navigate(rng.chance(60)),
        95 => Ev::UndoRedo(1 + rng.below(30)),
        _ => {
            let n = rng.below(16);
            Ev::Disk(match rng.below(9) {
                0 | 1 => Disk::Append(n, note(rng)),
                2 | 3 => Disk::Rewrite(n, note(rng)),
                4 => Disk::Truncate(n),
                5 => Disk::Delete(n),
                6 => Disk::NotUtf8(n),
                7 => Disk::Rename(n, format!("renamed {}.md", rng.below(100))),
                _ => Disk::Create(format!("new {}.md", rng.below(100)), note(rng)),
            })
        }
    }
}

/// The character a key types, as TranslateMessage would make it.
fn typed_char(vk: u16, shift: bool) -> Option<char> {
    match vk {
        0x41..=0x5A => Some(if shift { vk as u8 as char } else { (vk as u8 + 32) as char }),
        0x30..=0x39 if !shift => Some(vk as u8 as char),
        0x20 => Some(' '),
        _ => None,
    }
}

fn chars(a: &mut App, s: &str) {
    let mut buf = [0u16; 2];
    for c in s.chars() {
        for &u in c.encode_utf16(&mut buf).iter() {
            a.on_char(u);
        }
    }
}

fn key(a: &mut App, vk: u16, ctrl: bool, shift: bool, alt: bool) {
    sandbox::set_mods(ctrl, shift, alt);
    a.on_key(vk);
    // Alt makes WM_SYSCHAR, which the app leaves to Windows; Ctrl+letter makes a
    // control character, which on_char drops.
    if !alt && let Some(c) = typed_char(vk, shift) {
        let unit = if ctrl && c.is_ascii_alphabetic() {
            c.to_ascii_uppercase() as u16 - 0x40
        } else {
            c as u16
        };
        a.on_char(unit);
    }
    sandbox::set_mods(false, false, false);
}

fn apply(a: &mut App, ev: &Ev, root: &Path, size: &mut (u32, u32)) {
    match *ev {
        Ev::Key {
            vk,
            ctrl,
            shift,
            alt,
        } => key(a, vk, ctrl, shift, alt),
        Ev::Text(ref s) => chars(a, s),
        Ev::Click {
            x,
            y,
            shift,
            ctrl,
            double,
        } => {
            sandbox::set_mods(ctrl, shift, false);
            for _ in 0..1 + double as usize {
                a.on_mouse_down(x, y);
                a.on_mouse_up();
            }
            sandbox::set_mods(false, false, false);
        }
        Ev::Drag { from, to } => {
            a.on_mouse_down(from.0, from.1);
            for i in 1..=4 {
                a.on_mouse_move(
                    from.0 + (to.0 - from.0) * i / 4,
                    from.1 + (to.1 - from.1) * i / 4,
                );
            }
            a.on_mouse_up();
        }
        Ev::RightClick(x, y) => a.on_right_click(x, y),
        Ev::Wheel(d, ctrl) => {
            sandbox::set_mods(ctrl, false, false);
            a.on_wheel(d);
            sandbox::set_mods(false, false, false);
        }
        Ev::Hover(x, y) => a.on_mouse_move(x, y),
        Ev::Leave => a.on_mouse_leave(),
        Ev::Resize(w, h) => {
            *size = (w, h);
            a.on_size(w, h);
        }
        Ev::Focus(f) => a.on_focus(f),
        Ev::Timer => a.on_timer(),
        Ev::Navigate(back) => a.navigate(back),
        Ev::QuickOpen(ref q, down) => {
            key(a, VK_P.0, true, false, false);
            chars(a, q);
            for _ in 0..down {
                key(a, VK_DOWN.0, false, false, false);
            }
            key(a, VK_RETURN.0, false, false, false);
        }
        Ev::UndoRedo(n) => {
            // Undoing up to n steps and redoing as many gives back the same text.
            // (Redoing more would replay steps already undone before.)
            let before = a.doc.lines.clone();
            let mut undone = 0;
            while undone < n && let Some((x, c)) = a.doc.undo() {
                a.set_sel(x, c);
                a.changed(false);
                undone += 1;
            }
            for _ in 0..undone {
                if let Some((x, c)) = a.doc.redo() {
                    a.set_sel(x, c);
                    a.changed(false);
                }
            }
            if a.doc.lines != before {
                let l = (0..before.len().max(a.doc.lines.len()))
                    .find(|&l| before.get(l) != a.doc.lines.get(l))
                    .unwrap();
                panic!(
                    "undo then redo {undone} changed line {l} of {:?}: {:?} became {:?}                      ({} lines became {})",
                    a.doc.path,
                    before.get(l),
                    a.doc.lines.get(l),
                    before.len(),
                    a.doc.lines.len()
                );
            }
        }
        Ev::Disk(ref d) => {
            let notes = files(root);
            let at = |n: usize| notes.get(n % notes.len().max(1)).cloned();
            // Outside edits can race the app's own writes; failures are fine.
            let _ = match d {
                Disk::Append(n, s) => at(*n).map(|p| {
                    let mut t = std::fs::read(&p).unwrap_or_default();
                    t.extend_from_slice(s.as_bytes());
                    std::fs::write(p, t)
                }),
                Disk::Rewrite(n, s) => at(*n).map(|p| std::fs::write(p, s)),
                Disk::Truncate(n) => at(*n).map(|p| std::fs::write(p, "")),
                Disk::Delete(n) => at(*n).map(std::fs::remove_file),
                Disk::NotUtf8(n) => at(*n).map(|p| std::fs::write(p, b"\xff\xfe\xc0bad\n")),
                Disk::Rename(n, to) => at(*n).map(|p| std::fs::rename(&p, p.with_file_name(to))),
                Disk::Create(name, s) => Some(std::fs::write(root.join(name), s)),
            };
            // Coming back to the window is when the app looks at the disk.
            a.on_focus(false);
            a.on_focus(true);
        }
    }
}

/// Handles what the app posted to its own window: closing, and dropped files.
fn pump(a: &mut App) {
    unsafe {
        let mut msg = MSG::default();
        for _ in 0..100 {
            if !PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                break;
            }
            match msg.message {
                WM_CLOSE => {
                    // Closing asks (or saves); the test window stays open.
                    if a.confirm_discard() {
                        a.save_session();
                    }
                }
                WM_APP_DROP => a.finish_drop(),
                WM_TIMER => a.on_timer(),
                _ => {
                    DispatchMessageW(&msg);
                }
            }
        }
    }
}

fn check_pos(what: &str, a: &App, p: Pos) -> std::result::Result<(), String> {
    let line = a
        .doc
        .lines
        .get(p.line)
        .ok_or(format!("{what} {p:?} is past the last line ({})", a.doc.lines.len()))?;
    if p.col > line.len() || !line.is_char_boundary(p.col) {
        return Err(format!("{what} {p:?} is not a char boundary in {line:?}"));
    }
    Ok(())
}

fn check_text(what: &str, s: &str, cur: usize) -> std::result::Result<(), String> {
    if cur > s.len() || !s.is_char_boundary(cur) {
        return Err(format!("{what} cursor {cur} is not a char boundary in {s:?}"));
    }
    Ok(())
}

/// The editor's state is sound.
fn check(a: &App) -> std::result::Result<(), String> {
    if a.doc.lines.is_empty() {
        return Err("the document has no lines".into());
    }
    if let Some(e) = a.view.draw_error {
        return Err(format!("the frame failed to draw: {e:?}"));
    }
    check_pos("cursor", a, a.cursor)?;
    check_pos("anchor", a, a.anchor)?;
    if !a.scroll.is_finite() || a.scroll < 0.0 {
        return Err(format!("scroll is {}", a.scroll));
    }
    if a.infos.len() != a.doc.lines.len() {
        return Err(format!(
            "{} line infos for {} lines",
            a.infos.len(),
            a.doc.lines.len()
        ));
    }
    if let Some(p) = &a.palette {
        check_text("palette", &p.query, p.cur)?;
    }
    if let Some(p) = &a.prompt {
        check_text("prompt", &p.text, p.cur)?;
    }
    if let Some(m) = &a.slash
        && m.selected >= m.matches.len().max(1)
    {
        return Err(format!(
            "slash menu selects {} of {}",
            m.selected,
            m.matches.len()
        ));
    }
    Ok(())
}

unsafe extern "system" fn test_wndproc(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    unsafe { DefWindowProcW(h, m, w, l) }
}

/// A hidden window to paint into, so drawing and the hit-testing it sets up are
/// exercised too. Its own class keeps "close all windows" from finding it.
fn hidden_window() -> HWND {
    unsafe {
        let instance: HINSTANCE = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)
            .unwrap()
            .into();
        let class = w!("QuillNotesChaosTest");
        let wc = WNDCLASSW {
            hInstance: instance,
            lpszClassName: class,
            lpfnWndProc: Some(test_wndproc),
            ..Default::default()
        };
        RegisterClassW(&wc); // Fails harmlessly when a previous run registered it.
        CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class,
            w!("chaos"),
            WS_OVERLAPPEDWINDOW,
            0,
            0,
            1000,
            780,
            None,
            None,
            Some(instance),
            None,
        )
        .unwrap()
    }
}

/// Runs `steps` random steps; returns the slowest, slowest first.
fn run(seed: u64, steps: usize, progress: &Progress) -> Vec<(Duration, String)> {
    let base = std::env::temp_dir().join(format!("quillnotes-chaos-{}-{seed}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    // Deep, so a ".." typed into a file name still lands inside `base`.
    let root = base.join("a").join("b").join("c").join("notes");
    std::fs::create_dir_all(&root).unwrap();
    crate::settings::TEST_DIR.with(|d| *d.borrow_mut() = Some(base.join("settings")));
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    write_corpus(&root, &mut rng);
    sandbox::answer_randomly(rng.next());
    unsafe {
        let _ = windows::Win32::System::Ole::OleInitialize(None);
    }

    let hwnd = hidden_window();
    let mut a = App::new(View::new(false).unwrap());
    a.hwnd = hwnd;
    a.view.dpi = 96.0;
    let mut size = (1000, 780);
    a.on_size(size.0, size.1);
    a.open_folder(&root);
    a.on_focus(true);

    let mut slow: Vec<(Duration, String)> = Vec::new();
    for step in 0..steps {
        let ev = next_event(&mut rng, &a, &root, size);
        let desc = format!("{step}: {ev:?}");
        {
            let mut log = progress.log.lock().unwrap();
            if log.len() == HISTORY {
                log.pop_front();
            }
            log.push_back(desc.clone());
        }
        let t = Instant::now();
        apply(&mut a, &ev, &root, &mut size);
        pump(&mut a);
        a.paint();
        let took = t.elapsed();
        if slow.len() < SLOWEST || took > slow[SLOWEST - 1].0 {
            slow.push((took, desc));
            slow.sort_by(|a, b| b.0.cmp(&a.0));
            slow.truncate(SLOWEST);
        }
        if let Err(e) = check(&a) {
            panic!("seed {seed}, step {step}: {e}");
        }
        progress.step.store(step + 1, Ordering::Relaxed);
    }

    unsafe {
        let _ = DestroyWindow(hwnd);
    }
    crate::settings::TEST_DIR.with(|d| *d.borrow_mut() = None);
    let _ = std::fs::remove_dir_all(&base);
    slow
}

/// Runs one seed on its own thread, watching for a panic or a step that never ends.
fn run_watched(seed: u64, steps: usize) {
    let progress = Arc::new(Progress {
        step: AtomicUsize::new(0),
        log: Mutex::new(VecDeque::new()),
    });
    let p = progress.clone();
    let worker = std::thread::spawn(move || run(seed, steps, &p));
    let report = |why: &str| {
        let log = progress.log.lock().map(|l| l.clone()).unwrap_or_default();
        let steps: Vec<String> = log.into_iter().collect();
        format!(
            "chaos seed {seed} {why} at step {}.\nLast steps:\n{}\n\nRerun with \
             $env:QUILL_CHAOS_SEED = {seed}; cargo test chaos -- --nocapture",
            progress.step.load(Ordering::Relaxed),
            steps.join("\n")
        )
    };
    let (mut last, mut since) = (usize::MAX, Instant::now());
    loop {
        if worker.is_finished() {
            match worker.join() {
                Ok(slow) => {
                    for (took, step) in &slow {
                        println!("chaos seed {seed}: slow step {took:.1?} {step:.150}");
                    }
                    println!("chaos seed {seed}: {steps} steps ok");
                    return;
                }
                Err(_) => panic!("{}", report("panicked")),
            }
        }
        let now = progress.step.load(Ordering::Relaxed);
        if now != last {
            (last, since) = (now, Instant::now());
        } else if since.elapsed() > HANG {
            // The worker is stuck; it is left behind when the test process exits.
            panic!("{}", report(&format!("hung for {HANG:?}")));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn env(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse().ok()
}

#[test]
fn chaos() {
    let steps = env("QUILL_CHAOS_STEPS").unwrap_or(1500) as usize;
    let seeds: Vec<u64> = match (env("QUILL_CHAOS_SEED"), env("QUILL_CHAOS_RUNS")) {
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
        (None, None) => vec![1, 2],
    };
    for seed in seeds {
        println!("chaos seed {seed}, {steps} steps");
        run_watched(seed, steps);
    }
}
