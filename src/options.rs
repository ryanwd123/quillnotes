//! User-adjustable rendering options and general preferences, persisted in the
//! settings file.

use crate::settings::Settings;

/// Rendering options. Lengths are in DIPs at 100% zoom.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct RenderOptions {
    pub dash_bullets: bool,
    /// Keep the `#` markers visible on rendered headings.
    pub heading_hashes: bool,
    /// Rule under headings 1–6.
    pub heading_rule: [bool; 6],
    pub nums: [f32; NUM_COUNT],
}

/// Numeric options: (settings key, palette label, default, min, max, step).
const NUMS: [(&str, &str, f32, f32, f32, f32); NUM_COUNT] = [
    (
        "paragraph",
        "Spacing: Around paragraphs",
        3.0,
        0.0,
        40.0,
        1.0,
    ),
    (
        "heading_above",
        "Spacing: Above headings",
        16.0,
        0.0,
        80.0,
        1.0,
    ),
    (
        "heading_below",
        "Spacing: Below headings",
        5.0,
        0.0,
        60.0,
        1.0,
    ),
    ("list_before", "Spacing: Before lists", 2.0, 0.0, 40.0, 1.0),
    ("list_after", "Spacing: After lists", 2.0, 0.0, 40.0, 1.0),
    (
        "list_item",
        "Spacing: Between list items",
        4.0,
        0.0,
        40.0,
        1.0,
    ),
    ("list_indent", "Spacing: List indent", 28.0, 12.0, 80.0, 1.0),
    ("blank_line", "Spacing: Blank lines", 12.0, 0.0, 60.0, 1.0),
    (
        "quote_indent",
        "Spacing: Quote indent",
        20.0,
        8.0,
        80.0,
        1.0,
    ),
    (
        "code_padding",
        "Spacing: Code block padding",
        10.0,
        0.0,
        40.0,
        1.0,
    ),
    (
        "table_padding",
        "Spacing: Table cell padding",
        6.0,
        0.0,
        30.0,
        1.0,
    ),
    (
        "rule_spacing",
        "Spacing: Horizontal rules",
        26.0,
        4.0,
        100.0,
        1.0,
    ),
    ("line_height", "Text: Line height", 125.0, 100.0, 250.0, 5.0),
    (
        "code_size",
        "Code: Font size (% of the text size)",
        88.0,
        60.0,
        150.0,
        2.0,
    ),
    ("ui_size", "UI: Font size", 100.0, 70.0, 200.0, 5.0),
    (
        "content_width",
        "Layout: Content width",
        760.0,
        400.0,
        2400.0,
        20.0,
    ),
];
pub const NUM_COUNT: usize = 16;

/// Indices into [`RenderOptions::nums`].
pub mod num {
    pub const PARAGRAPH: usize = 0;
    pub const HEADING_ABOVE: usize = 1;
    pub const HEADING_BELOW: usize = 2;
    pub const LIST_BEFORE: usize = 3;
    pub const LIST_AFTER: usize = 4;
    pub const LIST_ITEM: usize = 5;
    pub const LIST_INDENT: usize = 6;
    pub const BLANK_LINE: usize = 7;
    pub const QUOTE_INDENT: usize = 8;
    pub const CODE_PADDING: usize = 9;
    pub const TABLE_PADDING: usize = 10;
    pub const RULE_SPACING: usize = 11;
    pub const LINE_HEIGHT: usize = 12;
    pub const CODE_SIZE: usize = 13;
    pub const UI_SIZE: usize = 14;
    pub const CONTENT_WIDTH: usize = 15;
}

/// One adjustable option, as listed in the palette.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Opt {
    Bullets,
    HeadingHashes,
    HeadingRule(usize),
    Num(usize),
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions {
            dash_bullets: false,
            heading_hashes: false,
            heading_rule: [true, true, false, false, false, false],
            nums: NUMS.map(|n| n.2),
        }
    }
}

impl RenderOptions {
    pub fn num(&self, i: usize) -> f32 {
        self.nums[i]
    }

    pub fn load(s: &Settings) -> Self {
        let mut o = RenderOptions::default();
        if let Some(v) = s.get("render.bullets") {
            o.dash_bullets = v == "dash";
        }
        if let Some(v) = s.get("render.heading_hashes") {
            o.heading_hashes = v == "on";
        }
        if let Some(v) = s.get("render.heading_rules") {
            o.heading_rule = [false; 6];
            for level in v.split(',').filter_map(|l| l.trim().parse::<usize>().ok()) {
                if (1..=6).contains(&level) {
                    o.heading_rule[level - 1] = true;
                }
            }
        }
        for (i, &(key, _, _, min, max, _)) in NUMS.iter().enumerate() {
            if let Some(v) = s
                .get(&format!("render.{key}"))
                .and_then(|v| v.parse::<f32>().ok())
            {
                o.nums[i] = v.clamp(min, max);
            }
        }
        o
    }

    pub fn store(&self, s: &mut Settings) {
        s.set(
            "render.bullets",
            if self.dash_bullets { "dash" } else { "dot" },
        );
        s.set(
            "render.heading_hashes",
            if self.heading_hashes { "on" } else { "off" },
        );
        let levels: Vec<String> = (1..=6)
            .filter(|l| self.heading_rule[l - 1])
            .map(|l| l.to_string())
            .collect();
        s.set("render.heading_rules", &levels.join(","));
        for (i, &(key, ..)) in NUMS.iter().enumerate() {
            s.set(&format!("render.{key}"), &self.nums[i].to_string());
        }
    }
}

fn on_off(on: bool) -> String {
    (if on { "☑ On" } else { "☐ Off" }).into()
}

impl Opt {
    /// Every option in palette order.
    pub fn all() -> Vec<Opt> {
        let mut v = vec![Opt::Bullets, Opt::HeadingHashes];
        v.extend((0..6).map(Opt::HeadingRule));
        v.extend((0..NUM_COUNT).map(Opt::Num));
        v
    }

    pub fn label(self) -> String {
        match self {
            Opt::Bullets => "Lists: Bullet style".into(),
            Opt::HeadingHashes => "Headings: Always show # markers".into(),
            Opt::HeadingRule(l) => format!("Headings: Line under heading {}", l + 1),
            Opt::Num(i) => NUMS[i].1.into(),
        }
    }

    pub fn value_text(self, o: &RenderOptions) -> String {
        match self {
            Opt::Bullets => (if o.dash_bullets {
                "‹ Dash ›"
            } else {
                "‹ Dot ›"
            })
            .into(),
            Opt::HeadingHashes => on_off(o.heading_hashes),
            Opt::HeadingRule(l) => on_off(o.heading_rule[l]),
            Opt::Num(i) => {
                let unit = if matches!(i, num::LINE_HEIGHT | num::CODE_SIZE | num::UI_SIZE) {
                    "%"
                } else {
                    " px"
                };
                format!("‹ {}{unit} ›", o.nums[i])
            }
        }
    }

    /// Steps a value; `steps` is ±1 (or larger for coarse changes).
    pub fn adjust(self, o: &mut RenderOptions, steps: i32) {
        match self {
            Opt::Bullets => o.dash_bullets = !o.dash_bullets,
            Opt::HeadingHashes => o.heading_hashes = steps > 0,
            Opt::HeadingRule(l) => o.heading_rule[l] = steps > 0,
            Opt::Num(i) => {
                let (_, _, _, min, max, step) = NUMS[i];
                o.nums[i] = (o.nums[i] + step * steps as f32).clamp(min, max);
            }
        }
    }

    /// Enter / click: toggles switches; numbers are adjusted with arrows instead.
    pub fn activate(self, o: &mut RenderOptions) {
        match self {
            Opt::Bullets => o.dash_bullets = !o.dash_bullets,
            Opt::HeadingHashes => o.heading_hashes = !o.heading_hashes,
            Opt::HeadingRule(l) => o.heading_rule[l] = !o.heading_rule[l],
            Opt::Num(_) => {}
        }
    }
}

/// General preferences, listed by "Preferences: Settings".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Prefs {
    /// Ask before moving an explorer entry to the Recycle Bin.
    pub confirm_delete: bool,
    /// With a folder open, Save As picks a path in the palette instead of the
    /// Windows save dialog.
    pub save_prompt: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            confirm_delete: true,
            save_prompt: true,
        }
    }
}

impl Prefs {
    pub fn load(s: &Settings) -> Self {
        let d = Prefs::default();
        let on = |key: &str, default: bool| s.get(key).map_or(default, |v| v == "on");
        Prefs {
            confirm_delete: on("confirm_delete", d.confirm_delete),
            save_prompt: on("save_prompt", d.save_prompt),
        }
    }

    pub fn store(&self, s: &mut Settings) {
        let on = |b: bool| if b { "on" } else { "off" };
        s.set("confirm_delete", on(self.confirm_delete));
        s.set("save_prompt", on(self.save_prompt));
    }
}

/// One preference, as listed in the palette.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pref {
    ConfirmDelete,
    SavePrompt,
}

impl Pref {
    pub const ALL: [Pref; 2] = [Pref::ConfirmDelete, Pref::SavePrompt];

    pub fn label(self) -> &'static str {
        match self {
            Pref::ConfirmDelete => "Explorer: Confirm before deleting",
            Pref::SavePrompt => "Save: Choose the path in the palette when a folder is open",
        }
    }

    fn flag(self, p: &mut Prefs) -> &mut bool {
        match self {
            Pref::ConfirmDelete => &mut p.confirm_delete,
            Pref::SavePrompt => &mut p.save_prompt,
        }
    }

    pub fn value_text(self, p: &Prefs) -> String {
        on_off(match self {
            Pref::ConfirmDelete => p.confirm_delete,
            Pref::SavePrompt => p.save_prompt,
        })
    }

    /// Left / Right: off / on.
    pub fn adjust(self, p: &mut Prefs, steps: i32) {
        *self.flag(p) = steps > 0;
    }

    /// Enter / click: toggles.
    pub fn activate(self, p: &mut Prefs) {
        let f = self.flag(p);
        *f = !*f;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adjust_clamps_and_toggles() {
        let mut o = RenderOptions::default();
        Opt::Num(num::LIST_ITEM).adjust(&mut o, -100);
        assert_eq!(o.num(num::LIST_ITEM), 0.0);
        Opt::Num(num::LINE_HEIGHT).adjust(&mut o, 2);
        assert_eq!(o.num(num::LINE_HEIGHT), 135.0);
        Opt::HeadingRule(0).activate(&mut o);
        assert!(!o.heading_rule[0]);
        Opt::HeadingRule(2).adjust(&mut o, 1);
        assert!(o.heading_rule[2]);
        Opt::Bullets.activate(&mut o);
        assert!(o.dash_bullets);
        Opt::HeadingHashes.activate(&mut o);
        assert!(o.heading_hashes);
        Opt::HeadingHashes.adjust(&mut o, -1);
        assert!(!o.heading_hashes);
        assert_eq!(Opt::all().len(), 2 + 6 + NUM_COUNT);
    }

    #[test]
    fn settings_round_trip() {
        let mut o = RenderOptions::default();
        o.dash_bullets = true;
        o.heading_hashes = true;
        o.heading_rule = [false, true, true, false, false, true];
        o.nums[num::PARAGRAPH] = 9.0;
        let mut s = Settings::default();
        o.store(&mut s);
        assert_eq!(s.get("render.heading_rules"), Some("2,3,6"));
        assert_eq!(RenderOptions::load(&s), o);
        // Out-of-range values are clamped.
        s.set("render.list_indent", "1");
        assert_eq!(RenderOptions::load(&s).num(num::LIST_INDENT), 12.0);
    }

    #[test]
    fn prefs_toggle_and_round_trip() {
        let mut p = Prefs::default();
        assert!(p.confirm_delete && p.save_prompt);
        Pref::ConfirmDelete.activate(&mut p);
        Pref::SavePrompt.adjust(&mut p, -1);
        assert_eq!(Pref::ConfirmDelete.value_text(&p), "☐ Off");
        let mut s = Settings::default();
        p.store(&mut s);
        assert_eq!(s.get("confirm_delete"), Some("off"));
        assert_eq!(Prefs::load(&s), p);
        assert_eq!(Prefs::load(&Settings::default()), Prefs::default());
    }
}
