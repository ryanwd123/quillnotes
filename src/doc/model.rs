//! Model test for the text model: random edits, undo, redo and saves are applied
//! to a [`Document`] and to a plain `String` with a list of snapshots, and the two
//! must agree after every step: the text, the end position each edit returns, the
//! selection undo and redo restore, whether the document is dirty, and that the
//! version changes whenever the text does.
//!
//! Reproduce a failure, or run longer, with (PowerShell):
//!
//! ```text
//! $env:QUILL_MODEL_SEED = 1234; $env:QUILL_MODEL_STEPS = 1000000
//! cargo test doc::model -- --nocapture
//! ```
//!
//! A plain `cargo test` runs a fixed seed. `QUILL_MODEL_RUNS = 8` runs that many
//! seeds taken from the clock.

use super::*;

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
}

/// Text to insert: often a character or two, sometimes lines or nothing.
fn text(rng: &mut Rng) -> String {
    const PIECES: &[&str] = &[
        "a", "b", "x", " ", " ", "\n", "\n\n", "\u{e9}", "\u{65e5}", "\u{1F600}", "e\u{301}",
        "ab", " word", "word ", "- ", "# ", "\t", "|",
    ];
    match rng.below(10) {
        0 => String::new(),
        1 => (0..1 + rng.below(8))
            .map(|_| PIECES[rng.below(PIECES.len())])
            .collect(),
        _ => PIECES[rng.below(PIECES.len())].to_string(),
    }
}

/// The position of byte offset `o` in `s`.
fn pos(s: &str, o: usize) -> Pos {
    let before = &s[..o];
    let line = before.matches('\n').count();
    let col = o - before.rfind('\n').map_or(0, |i| i + 1);
    Pos::new(line, col)
}

/// A random char boundary of `s`, often near `near`.
fn boundary(rng: &mut Rng, s: &str, near: usize) -> usize {
    let mut o = if rng.below(3) == 0 {
        rng.below(s.len() + 1)
    } else {
        (near + rng.below(9)).saturating_sub(4).min(s.len())
    };
    while !s.is_char_boundary(o) {
        o -= 1;
    }
    o
}

/// The model: snapshots of the text after each undo step, where the document is in
/// them, and which one is saved on disk.
struct Model {
    history: Vec<(String, Sel, Sel)>,
    at: usize,
    saved: Option<usize>,
}

impl Model {
    fn text(&self) -> &str {
        &self.history[self.at].0
    }
}

fn check(d: &Document, m: &Model, step: usize, what: &str) {
    let text = d.lines.join("\n");
    assert_eq!(text, m.text(), "step {step} ({what}): text differs");
    assert_eq!(
        d.undo.len(),
        m.at,
        "step {step} ({what}): undo depth differs from the model"
    );
    assert_eq!(
        d.is_dirty(),
        m.saved != Some(m.at),
        "step {step} ({what}): dirty is wrong (model saved at {:?}, at {})",
        m.saved,
        m.at
    );
}

fn run(seed: u64, steps: usize) {
    let mut rng = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut d = Document::from_bytes(b"one\ntwo three\n\nfour", None).0;
    let origin = (Pos::default(), Pos::default());
    let mut m = Model {
        history: vec![(d.lines.join("\n"), origin, origin)],
        at: 0,
        saved: Some(0),
    };
    let mut caret = 0;
    // New undo steps, merged groups, and undos and redos that did something.
    let (mut pushed, mut merged, mut undos, mut redos) = (0, 0, 0, 0);
    for step in 0..steps {
        let version = d.version;
        let before_text = m.text().to_string();
        let what;
        match rng.below(20) {
            // A group of edits, as the editor makes them.
            0..=9 => {
                let kind = [EditKind::Typing, EditKind::Delete, EditKind::Other][rng.below(3)];
                what = "group";
                let sel = (pos(&before_text, caret), pos(&before_text, caret));
                d.begin(kind, sel);
                let mut s = before_text.clone();
                let mut changed = false;
                for _ in 0..1 + rng.below(3) {
                    let a = boundary(&mut rng, &s, caret);
                    let b = if rng.below(2) == 0 { a } else { boundary(&mut rng, &s, a) };
                    let t = text(&mut rng);
                    let end = d.replace(pos(&s, a), pos(&s, b), &t);
                    let (a, b) = (a.min(b), a.max(b));
                    changed |= s[a..b] != t;
                    s.replace_range(a..b, &t);
                    caret = a + t.len();
                    assert_eq!(end, pos(&s, caret), "step {step}: replace returned a wrong end");
                }
                let after = (pos(&s, caret), pos(&s, caret));
                let depth = d.undo.len();
                d.commit(after);
                if !changed {
                    // No edits: no undo step, and redo is kept.
                } else if d.undo.len() > depth {
                    pushed += 1;
                    // A new undo step.
                    m.history.truncate(m.at + 1);
                    m.history.push((s, sel, after));
                    m.at += 1;
                    if m.saved.is_some_and(|sv| sv >= m.at) {
                        m.saved = None;
                    }
                } else {
                    merged += 1;
                    // Merged into the step before; never the saved one.
                    assert_ne!(m.saved, Some(m.at), "step {step}: merged into the saved step");
                    assert!(m.at > 0, "step {step}: merged with nothing to merge into");
                    m.history.truncate(m.at + 1);
                    let top = &mut m.history[m.at];
                    top.0 = s;
                    top.2 = after;
                }
            }
            // A single edit outside a group: its own undo step.
            10..=11 => {
                what = "bare replace";
                let s = &before_text;
                let a = boundary(&mut rng, s, caret);
                let b = boundary(&mut rng, s, a);
                let t = text(&mut rng);
                let end = d.replace(pos(s, a), pos(s, b), &t);
                let (a, b) = (a.min(b), a.max(b));
                let mut s = s.clone();
                s.replace_range(a..b, &t);
                caret = a + t.len();
                assert_eq!(end, pos(&s, caret), "step {step}: replace returned a wrong end");
                if before_text[a..b] != t {
                    let sel = (pos(&before_text, a), pos(&before_text, b));
                    m.history.truncate(m.at + 1);
                    m.history.push((s, sel, (end, end)));
                    m.at += 1;
                    if m.saved.is_some_and(|sv| sv >= m.at) {
                        m.saved = None;
                    }
                }
            }
            12..=15 => {
                what = "undo";
                let sel = d.undo();
                if m.at == 0 {
                    assert_eq!(sel, None, "step {step}: undo with nothing to undo");
                } else {
                    assert_eq!(sel, Some(m.history[m.at].1), "step {step}: undo selection");
                    m.at -= 1;
                    undos += 1;
                }
            }
            16..=18 => {
                what = "redo";
                let sel = d.redo();
                if m.at + 1 == m.history.len() {
                    assert_eq!(sel, None, "step {step}: redo with nothing to redo");
                } else {
                    m.at += 1;
                    redos += 1;
                    assert_eq!(sel, Some(m.history[m.at].2), "step {step}: redo selection");
                }
            }
            _ => {
                what = "save";
                d.mark_saved();
                m.saved = Some(m.at);
            }
        }
        check(&d, &m, step, what);
        if m.text() != before_text {
            assert_ne!(d.version, version, "step {step} ({what}): text changed, version did not");
        }
        caret = caret.min(m.text().len());
        while !m.text().is_char_boundary(caret) {
            caret -= 1;
        }
        // Keep documents small enough to stay quick.
        if m.text().len() > 4000 {
            let e = d.end();
            d.replace(Pos::default(), e, "reset\nlines");
            m.history.truncate(m.at + 1);
            m.history.push(("reset\nlines".into(), (Pos::default(), e), (e, e)));
            m.at += 1;
            if m.saved.is_some_and(|sv| sv >= m.at) {
                m.saved = None;
            }
            caret = 0;
            check(&d, &m, step, "reset");
        }
    }
    println!(
        "doc model seed {seed}: {steps} steps ok ({pushed} groups, {merged} merged, {undos} undos, {redos} redos)"
    );
}

fn env(name: &str) -> Option<u64> {
    std::env::var(name).ok()?.trim().parse().ok()
}

#[test]
fn model() {
    let steps = env("QUILL_MODEL_STEPS").unwrap_or(20_000) as usize;
    let seeds: Vec<u64> = match (env("QUILL_MODEL_SEED"), env("QUILL_MODEL_RUNS")) {
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
        run(seed, steps);
    }
}
