//! Live-preview layout and painting with Direct2D and DirectWrite.
//!
//! Every source line becomes one [`Row`]. Lines being edited ("active") show
//! their Markdown source with delimiters dimmed; all other lines are rendered
//! with delimiters hidden and block syntax drawn as decorations.

use std::cell::Cell;
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::*;
use windows::core::*;
use windows_numerics::{Matrix3x2, Vector2};

use crate::doc::Pos;
use crate::markdown::{self, Align, Kind, LineInfo, Style};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Ink {
    Bg,
    Text,
    Heading,
    Dim,
    Link,
    CodeBg,
    InlineCodeBg,
    Quote,
    Selection,
    Caret,
    StatusBg,
    StatusText,
    Rule,
    TableHeader,
    Mark,
    Accent,
    Scrollbar,
    Note,
    Tip,
    Warning,
    Danger,
    Important,
    PaletteBg,
    PaletteSel,
    Shadow,
}
const INK_COUNT: usize = 25;

const DARK: [u32; INK_COUNT] = [
    0xFF1B1B1B, 0xFFD4D4D4, 0xFFEDEDED, 0xFF6E6E6E, 0xFF5AA9FF, 0xFF24272D, 0xFF2D3036, 0xFF4E5664,
    0x80264F78, 0xFFE6E6E6, 0xFF161616, 0xFF8E8E8E, 0xFF3C3C3C, 0xFF232528, 0x70857000, 0xFF5AA9FF,
    0x40FFFFFF, 0xFF4F8EF7, 0xFF3FB950, 0xFFD29922, 0xFFF85149, 0xFFA371F7, 0xFF252629, 0xFF2F4058,
    0x24000000,
];
const LIGHT: [u32; INK_COUNT] = [
    0xFFFFFFFF, 0xFF24292F, 0xFF101418, 0xFF9AA0A6, 0xFF0969DA, 0xFFF4F5F7, 0xFFEEF0F2, 0xFFD0D7DE,
    0x6699C8FF, 0xFF111111, 0xFFF3F3F3, 0xFF6A6A6A, 0xFFE1E4E8, 0xFFF6F8FA, 0x80FFE066, 0xFF0969DA,
    0x40000000, 0xFF0969DA, 0xFF1A7F37, 0xFF9A6700, 0xFFCF222E, 0xFF8250DF, 0xFFFFFFFF, 0xFFE3ECF8,
    0x0C000000,
];

fn color(argb: u32) -> D2D1_COLOR_F {
    D2D1_COLOR_F {
        r: ((argb >> 16) & 0xFF) as f32 / 255.0,
        g: ((argb >> 8) & 0xFF) as f32 / 255.0,
        b: (argb & 0xFF) as f32 / 255.0,
        a: (argb >> 24) as f32 / 255.0,
    }
}

fn rect(l: f32, t: f32, r: f32, b: f32) -> D2D_RECT_F {
    D2D_RECT_F {
        left: l,
        top: t,
        right: r,
        bottom: b,
    }
}

fn v2(x: f32, y: f32) -> Vector2 {
    Vector2 { X: x, Y: y }
}

fn range(a: u32, b: u32) -> DWRITE_TEXT_RANGE {
    DWRITE_TEXT_RANGE {
        startPosition: a,
        length: b.saturating_sub(a),
    }
}

pub const STATUS_H: f32 = 26.0;

#[derive(Clone, Copy, PartialEq)]
struct BoxStyle {
    mono: bool,
    size: f32,
    weight: i32,
    ink: Ink,
    /// Render a leading `[!TYPE]` as a callout title.
    callout: bool,
}

/// A laid-out run of text (one line, or one table cell) with its mapping back to source.
pub struct TextBox {
    layout: IDWriteTextLayout,
    /// UTF-16 index in the layout -> byte offset in the source line (len = n + 1).
    d2s: Vec<u32>,
    width: Cell<f32>,
    height: Cell<f32>,
    bgs: Vec<(u32, u32, Ink)>,
    links: Vec<(u32, u32, String)>,
    /// x of the content start, for active lines whose syntax prefix hangs left.
    prefix_x: f32,
    /// Ordered-list number, drawn left of the content when inactive.
    marker: Option<(IDWriteTextLayout, f32)>,
    baseline: f32,
    ink: Ink,
}

impl TextBox {
    fn s2d(&self, col: usize) -> u32 {
        let i = self.d2s.partition_point(|&s| (s as usize) < col);
        i.min(self.d2s.len() - 1) as u32
    }

    fn set_width(&self, w: f32) {
        if (self.width.get() - w).abs() > 0.01 {
            unsafe {
                let _ = self.layout.SetMaxWidth(w.max(1.0));
            }
            self.width.set(w);
            self.height.set(layout_height(&self.layout));
        }
    }

    fn rects(&self, a: u32, b: u32) -> Vec<DWRITE_HIT_TEST_METRICS> {
        let mut buf = vec![DWRITE_HIT_TEST_METRICS::default(); 8];
        let mut n = 0u32;
        unsafe {
            if self
                .layout
                .HitTestTextRange(a, b - a, 0.0, 0.0, Some(&mut buf), &mut n)
                .is_err()
            {
                buf.resize(n as usize, Default::default());
                if self
                    .layout
                    .HitTestTextRange(a, b - a, 0.0, 0.0, Some(&mut buf), &mut n)
                    .is_err()
                {
                    return Vec::new();
                }
            }
        }
        buf.truncate(n as usize);
        buf
    }

    /// (x, y, height) of the caret before UTF-16 index `i`.
    fn caret(&self, i: u32) -> (f32, f32, f32) {
        let (mut x, mut y) = (0.0, 0.0);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            let _ = self
                .layout
                .HitTestTextPosition(i, false, &mut x, &mut y, &mut m);
        }
        (x, m.top, m.height)
    }

    /// Visual line containing UTF-16 index `i`, plus (top, height) of every visual line.
    fn visual_lines(&self, i: u32) -> (usize, Vec<(f32, f32)>) {
        let buf = line_metrics(&self.layout);
        let (mut top, mut start, mut idx) = (0.0, 0u32, None);
        let mut out = Vec::with_capacity(buf.len());
        for (k, m) in buf.iter().enumerate() {
            if idx.is_none() && (i < start + m.length || k + 1 == buf.len()) {
                idx = Some(k);
            }
            out.push((top, m.height));
            top += m.height;
            start += m.length;
        }
        let idx = idx.unwrap_or(0);
        (idx, out)
    }

    fn hit(&self, x: f32, y: f32) -> (usize, Option<String>) {
        let mut trailing = BOOL(0);
        let mut inside = BOOL(0);
        let mut m = DWRITE_HIT_TEST_METRICS::default();
        unsafe {
            let _ = self
                .layout
                .HitTestPoint(x, y, &mut trailing, &mut inside, &mut m);
        }
        let idx = (m.textPosition + if trailing.as_bool() { m.length } else { 0 }) as usize;
        let col = self.d2s[idx.min(self.d2s.len() - 1)] as usize;
        let link = if inside.as_bool() {
            self.links
                .iter()
                .find(|l| m.textPosition >= l.0 && m.textPosition < l.1)
                .map(|l| l.2.clone())
        } else {
            None
        };
        (col, link)
    }
}

/// Metrics of every visual line (GetLineMetrics fails if the buffer is too small).
fn line_metrics(l: &IDWriteTextLayout) -> Vec<DWRITE_LINE_METRICS> {
    let mut buf = vec![DWRITE_LINE_METRICS::default(); 8];
    let mut n = 0u32;
    unsafe {
        if l.GetLineMetrics(Some(&mut buf), &mut n).is_err() {
            buf.resize(n as usize, Default::default());
            let _ = l.GetLineMetrics(Some(&mut buf), &mut n);
        }
    }
    buf.truncate(n as usize);
    buf
}

fn layout_height(l: &IDWriteTextLayout) -> f32 {
    let mut m = DWRITE_TEXT_METRICS::default();
    unsafe {
        let _ = l.GetMetrics(&mut m);
    }
    m.height
}

struct Cellbox {
    tb: TextBox,
    align: Align,
}

pub struct TableBox {
    col_x: Vec<f32>,
    col_w: Vec<f32>,
    rows: Vec<(Vec<Cellbox>, f32)>,
}

pub struct Img {
    bmp: ID2D1Bitmap,
    w: f32,
    h: f32,
}

pub enum Vis {
    None,
    Text(Rc<TextBox>),
    Rule,
    Table(Rc<TableBox>, usize),
    Image(Rc<Img>, f32, f32),
}

pub struct Row {
    pub y: f32,
    pub h: f32,
    /// Text top relative to `y`.
    top: f32,
    /// Text origin x.
    x: f32,
    vis: Vis,
    active: bool,
}

enum Decor {
    Block { x0: f32, x1: f32, y0: f32, y1: f32 },
    Bar { x: f32, y0: f32, y1: f32, ink: Ink },
    HLine { x0: f32, x1: f32, y: f32 },
}

pub struct HitResult {
    pub pos: Pos,
    pub link: Option<String>,
    pub checkbox: bool,
}

/// Which font a text format uses.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Family {
    /// The user-selectable document font.
    Body,
    Mono,
    /// Status bar and palette.
    Ui,
}

/// One row of the command palette.
pub struct PaletteItem<'a> {
    pub label: &'a str,
    /// Byte offsets of label characters matched by the query (drawn bold).
    pub matches: &'a [usize],
    pub detail: &'a str,
    /// Draw the label in this font family (font picker previews).
    pub font: Option<&'a str>,
}

pub struct PaletteView<'a> {
    pub query: &'a str,
    /// Caret byte offset in `query`.
    pub cur: usize,
    pub placeholder: &'a str,
    /// The visible slice of items.
    pub items: Vec<PaletteItem<'a>>,
    /// Index of the selected item within `items`.
    pub selected: Option<usize>,
    /// Text shown when nothing matches.
    pub empty: &'a str,
}

/// Where the palette was last drawn, for mouse hit-testing.
#[derive(Clone, Copy)]
pub struct PaletteGeom {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub items_top: f32,
    pub item_h: f32,
}

/// Number of palette items visible at once.
pub const PALETTE_ROWS: usize = 10;

pub struct Status<'a> {
    pub left: &'a str,
    pub right: &'a str,
    /// Prompt text and its caret byte offset.
    pub prompt: Option<(&'a str, usize)>,
}

pub struct View {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    wic: Option<IWICImagingFactory>,
    rt: Option<ID2D1HwndRenderTarget>,
    brushes: Vec<ID2D1SolidColorBrush>,
    formats: HashMap<(Family, u32, i32), IDWriteTextFormat>,
    /// Formats for font-picker previews, by family name.
    preview_formats: HashMap<String, IDWriteTextFormat>,
    pub palette_geom: Option<PaletteGeom>,
    body_font: HSTRING,
    mono_font: HSTRING,
    boxes: HashMap<u64, Rc<TextBox>>,
    tables: HashMap<u64, Rc<TableBox>>,
    images: HashMap<PathBuf, Option<Rc<Img>>>,
    pub dark: bool,
    pub zoom: f32,
    pub dpi: f32,
    /// Client size in DIPs.
    pub w: f32,
    pub h: f32,
    pub rows: Vec<Row>,
    decor: Vec<Decor>,
    pub total_h: f32,
    x0: f32,
    cw: f32,
}

fn font_exists(dw: &IDWriteFactory, name: &str) -> bool {
    unsafe {
        let mut coll = None;
        if dw.GetSystemFontCollection(&mut coll, false).is_err() {
            return false;
        }
        let Some(coll) = coll else { return false };
        let mut idx = 0;
        let mut exists = BOOL(0);
        coll.FindFamilyName(&HSTRING::from(name), &mut idx, &mut exists)
            .is_ok()
            && exists.as_bool()
    }
}

impl View {
    pub fn new(dark: bool) -> Result<Self> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let wic = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok();
            let mono = ["Cascadia Mono", "Consolas"]
                .into_iter()
                .find(|f| font_exists(&dw, f))
                .unwrap_or("Courier New");
            Ok(View {
                d2d,
                dw,
                wic,
                rt: None,
                brushes: Vec::new(),
                formats: HashMap::new(),
                preview_formats: HashMap::new(),
                palette_geom: None,
                body_font: HSTRING::from("Segoe UI"),
                mono_font: HSTRING::from(mono),
                boxes: HashMap::new(),
                tables: HashMap::new(),
                images: HashMap::new(),
                dark,
                zoom: 1.0,
                dpi: 96.0,
                w: 800.0,
                h: 600.0,
                rows: Vec::new(),
                decor: Vec::new(),
                total_h: 0.0,
                x0: 0.0,
                cw: 0.0,
            })
        }
    }

    fn palette(&self) -> &'static [u32; INK_COUNT] {
        if self.dark { &DARK } else { &LIGHT }
    }

    /// Re-applies the palette to existing brushes (layouts keep referencing them).
    pub fn apply_theme(&mut self) {
        let pal = self.palette();
        for (b, &c) in self.brushes.iter().zip(pal.iter()) {
            unsafe { b.SetColor(&color(c)) };
        }
    }

    pub fn ensure_target(&mut self, hwnd: HWND) -> Result<()> {
        if self.rt.is_some() {
            return Ok(());
        }
        unsafe {
            let mut rc = RECT::default();
            let _ = windows::Win32::UI::WindowsAndMessaging::GetClientRect(hwnd, &mut rc);
            let props = D2D1_RENDER_TARGET_PROPERTIES::default();
            let hprops = D2D1_HWND_RENDER_TARGET_PROPERTIES {
                hwnd,
                pixelSize: D2D_SIZE_U {
                    width: (rc.right - rc.left).max(1) as u32,
                    height: (rc.bottom - rc.top).max(1) as u32,
                },
                presentOptions: D2D1_PRESENT_OPTIONS_NONE,
            };
            let rt = self.d2d.CreateHwndRenderTarget(&props, &hprops)?;
            rt.SetDpi(self.dpi, self.dpi);
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            self.brushes = self
                .palette()
                .iter()
                .map(|&c| rt.CreateSolidColorBrush(&color(c), None))
                .collect::<Result<_>>()?;
            self.rt = Some(rt);
            // Layouts reference the old brushes as drawing effects.
            self.boxes.clear();
            self.tables.clear();
            self.images.clear();
        }
        Ok(())
    }

    pub fn resize(&mut self, pw: u32, ph: u32, dpi: f32) {
        self.dpi = dpi;
        self.w = pw as f32 * 96.0 / dpi;
        self.h = ph as f32 * 96.0 / dpi;
        if let Some(rt) = &self.rt {
            unsafe {
                rt.SetDpi(dpi, dpi);
                let _ = rt.Resize(&D2D_SIZE_U {
                    width: pw.max(1),
                    height: ph.max(1),
                });
            }
        }
    }

    fn brush(&self, ink: Ink) -> &ID2D1SolidColorBrush {
        &self.brushes[ink as usize]
    }

    pub fn body_font(&self) -> String {
        self.body_font.to_string()
    }

    pub fn has_font(&self, name: &str) -> bool {
        font_exists(&self.dw, name)
    }

    /// Switches the document font; every cached layout is rebuilt.
    pub fn set_body_font(&mut self, name: &str) {
        if self.body_font == name {
            return;
        }
        self.body_font = HSTRING::from(name);
        self.formats.retain(|k, _| k.0 != Family::Body);
        self.boxes.clear();
        self.tables.clear();
    }

    /// Installed font family names (English names where available), sorted, each with
    /// whether the font can draw its own name (false for icon and symbol fonts).
    pub fn font_families(&self) -> Vec<(String, bool)> {
        let mut out = Vec::new();
        unsafe {
            let mut coll = None;
            if self.dw.GetSystemFontCollection(&mut coll, false).is_err() {
                return out;
            }
            let Some(coll) = coll else { return out };
            for i in 0..coll.GetFontFamilyCount() {
                let Ok(family) = coll.GetFontFamily(i) else {
                    continue;
                };
                let Ok(names) = family.GetFamilyNames() else {
                    continue;
                };
                let font = family
                    .GetFirstMatchingFont(
                        DWRITE_FONT_WEIGHT_NORMAL,
                        DWRITE_FONT_STRETCH_NORMAL,
                        DWRITE_FONT_STYLE_NORMAL,
                    )
                    .ok();
                let (mut idx, mut exists) = (0u32, BOOL(0));
                let _ = names.FindLocaleName(w!("en-us"), &mut idx, &mut exists);
                if !exists.as_bool() {
                    idx = 0;
                }
                let Ok(len) = names.GetStringLength(idx) else {
                    continue;
                };
                let mut buf = vec![0u16; len as usize + 1];
                if names.GetString(idx, &mut buf).is_ok() {
                    let name = String::from_utf16_lossy(&buf[..len as usize]);
                    let previewable = font.is_some_and(|f| {
                        !f.IsSymbolFont().as_bool()
                            && name
                                .chars()
                                .all(|c| f.HasCharacter(c as u32).is_ok_and(|b| b.as_bool()))
                    });
                    out.push((name, previewable));
                }
            }
        }
        out.sort_by_key(|n| n.0.to_lowercase());
        out.dedup_by(|a, b| a.0 == b.0);
        out
    }

    fn format(&mut self, family: Family, size: f32, weight: i32) -> IDWriteTextFormat {
        let key = (family, (size * 100.0) as u32, weight);
        if let Some(f) = self.formats.get(&key) {
            return f.clone();
        }
        let mono = family == Family::Mono;
        let ui = HSTRING::from("Segoe UI");
        let family = match family {
            Family::Body => &self.body_font,
            Family::Mono => &self.mono_font,
            Family::Ui => &ui,
        };
        let f = unsafe {
            let f = self
                .dw
                .CreateTextFormat(
                    family,
                    None,
                    DWRITE_FONT_WEIGHT(weight),
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("en-us"),
                )
                .expect("CreateTextFormat");
            let _ = f.SetWordWrapping(DWRITE_WORD_WRAPPING_EMERGENCY_BREAK);
            let (spacing, baseline) = if mono { (1.3, 1.15) } else { (1.25, 1.12) };
            let _ = f.SetLineSpacing(DWRITE_LINE_SPACING_METHOD_PROPORTIONAL, spacing, baseline);
            f
        };
        self.formats.insert(key, f.clone());
        f
    }

    fn base(&self) -> f32 {
        16.0 * self.zoom
    }

    /// Builds a text box for `line[from..to]`. Inline syntax is parsed from `content`;
    /// when `active`, delimiters stay visible (dimmed) and `[from, content)` is the
    /// hanging syntax prefix.
    #[allow(clippy::too_many_arguments)]
    fn make_box(
        &mut self,
        line: &str,
        from: usize,
        to: usize,
        content: usize,
        st: BoxStyle,
        active: bool,
        inline: bool,
        width: f32,
    ) -> TextBox {
        let mut inl = if inline {
            markdown::parse_inline(line, content, to)
        } else {
            Default::default()
        };
        let mut callout = None;
        if st.callout {
            let lead = line[content..to].len() - line[content..to].trim_start().len();
            let t = content + lead;
            if let Some(close) = line[t..to].strip_prefix("[!").and_then(|r| r.find(']')) {
                let name = t + 2..t + 2 + close;
                inl.hidden.push(t..t + 2);
                inl.hidden.push(name.end..name.end + 1);
                inl.hidden.sort_by_key(|r| r.start);
                callout = Some((name, callout_ink(&line[t..])));
            }
        }
        let hidden: &[std::ops::Range<usize>] = if active { &[] } else { &inl.hidden };
        let subs: &[(std::ops::Range<usize>, char)] = if active { &[] } else { &inl.subs };
        let mut text = Vec::with_capacity(to - from);
        let mut d2s = Vec::with_capacity(to - from + 1);
        let (mut h, mut sb, mut skip_to) = (0, 0, 0);
        for (off, ch) in line[from..to].char_indices() {
            let i = from + off;
            if i < skip_to {
                continue;
            }
            while h < hidden.len() && hidden[h].end <= i {
                h += 1;
            }
            if h < hidden.len() && hidden[h].start <= i {
                continue;
            }
            while sb < subs.len() && subs[sb].0.start < i {
                sb += 1;
            }
            let ch = if sb < subs.len() && subs[sb].0.start == i {
                skip_to = subs[sb].0.end;
                subs[sb].1
            } else {
                ch
            };
            let mut buf = [0u16; 2];
            for u in ch.encode_utf16(&mut buf) {
                text.push(*u);
                d2s.push(i as u32);
            }
        }
        d2s.push(to as u32);
        let s2d = |src: usize| {
            d2s.partition_point(|&s| (s as usize) < src)
                .min(d2s.len() - 1) as u32
        };

        let family = if st.mono { Family::Mono } else { Family::Body };
        let format = self.format(family, st.size, st.weight);
        let layout = unsafe {
            self.dw
                .CreateTextLayout(&text, &format, width.max(1.0), 1.0e6)
                .expect("CreateTextLayout")
        };
        let mut bgs = Vec::new();
        let mut links = Vec::new();
        unsafe {
            for (r, sty) in &inl.spans {
                let rr = range(s2d(r.start), s2d(r.end));
                match sty {
                    Style::Bold => {
                        let _ = layout.SetFontWeight(
                            if st.weight >= 600 {
                                DWRITE_FONT_WEIGHT_EXTRA_BOLD
                            } else {
                                DWRITE_FONT_WEIGHT_BOLD
                            },
                            rr,
                        );
                    }
                    Style::Italic => {
                        let _ = layout.SetFontStyle(DWRITE_FONT_STYLE_ITALIC, rr);
                    }
                    Style::Strike => {
                        let _ = layout.SetStrikethrough(true, rr);
                    }
                    Style::Code => {
                        let _ = layout.SetFontFamilyName(&self.mono_font, rr);
                        let _ = layout.SetFontSize(st.size * 0.88, rr);
                        bgs.push((
                            rr.startPosition,
                            rr.startPosition + rr.length,
                            Ink::InlineCodeBg,
                        ));
                    }
                    Style::Link => {
                        let _ = layout.SetUnderline(true, rr);
                        let _ = layout.SetDrawingEffect(self.brush(Ink::Link), rr);
                    }
                    Style::Image => {
                        let _ = layout.SetFontStyle(DWRITE_FONT_STYLE_ITALIC, rr);
                        let _ = layout.SetDrawingEffect(self.brush(Ink::Link), rr);
                    }
                    Style::Mark => {
                        bgs.push((rr.startPosition, rr.startPosition + rr.length, Ink::Mark))
                    }
                    Style::Comment => {
                        let _ = layout.SetDrawingEffect(self.brush(Ink::Dim), rr);
                    }
                }
            }
            if let Some((r, ink)) = &callout {
                let rr = range(s2d(r.start), s2d(r.end));
                let _ = layout.SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, rr);
                let _ = layout.SetDrawingEffect(self.brush(*ink), rr);
            }
            for (r, target) in &inl.links {
                links.push((s2d(r.start), s2d(r.end), target.clone()));
            }
            if active {
                let dim = self.brush(Ink::Dim);
                for r in &inl.hidden {
                    let _ = layout.SetDrawingEffect(dim, range(s2d(r.start), s2d(r.end)));
                }
                if content > from {
                    let _ = layout.SetDrawingEffect(dim, range(0, s2d(content)));
                }
            }
        }
        let mut tb = TextBox {
            layout,
            d2s,
            width: Cell::new(width),
            height: Cell::new(0.0),
            bgs,
            links,
            prefix_x: 0.0,
            marker: None,
            baseline: 0.0,
            ink: st.ink,
        };
        tb.height.set(layout_height(&tb.layout));
        if active && content > from {
            tb.prefix_x = tb.caret(tb.s2d(content)).0;
        }
        tb.baseline = line_metrics(&tb.layout).first().map_or(0.0, |m| m.baseline);
        tb
    }

    fn cached_box<F: FnOnce(&mut Self) -> TextBox>(
        &mut self,
        old: &mut HashMap<u64, Rc<TextBox>>,
        key: u64,
        width: f32,
        make: F,
    ) -> Rc<TextBox> {
        let tb = match old.remove(&key).or_else(|| self.boxes.get(&key).cloned()) {
            Some(tb) => tb,
            None => Rc::new(make(self)),
        };
        let _ = width;
        self.boxes.insert(key, tb.clone());
        tb
    }

    fn load_image(&mut self, src: &str, doc_dir: Option<&Path>) -> Option<Rc<Img>> {
        if src.starts_with("http://") || src.starts_with("https://") || src.starts_with("data:") {
            return None;
        }
        let src = src
            .strip_prefix("file:///")
            .unwrap_or(src)
            .replace("%20", " ");
        let mut path = PathBuf::from(&src);
        if path.is_relative() {
            path = doc_dir.map(|d| d.join(&src)).unwrap_or(path);
        }
        if let Some(cached) = self.images.get(&path) {
            return cached.clone();
        }
        let img = (|| -> Result<Img> {
            let wic = self.wic.as_ref().ok_or(Error::from(E_FAIL))?;
            let rt = self.rt.as_ref().ok_or(Error::from(E_FAIL))?;
            unsafe {
                let dec = wic.CreateDecoderFromFilename(
                    &HSTRING::from(path.as_os_str()),
                    None,
                    GENERIC_READ,
                    WICDecodeMetadataCacheOnDemand,
                )?;
                let frame = dec.GetFrame(0)?;
                let conv = wic.CreateFormatConverter()?;
                conv.Initialize(
                    &frame,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapDitherTypeNone,
                    None,
                    0.0,
                    WICBitmapPaletteTypeMedianCut,
                )?;
                let bmp = rt.CreateBitmapFromWicBitmap(&conv, None)?;
                let (mut w, mut h) = (0u32, 0u32);
                conv.GetSize(&mut w, &mut h)?;
                Ok(Img {
                    bmp,
                    w: w as f32,
                    h: h as f32,
                })
            }
        })()
        .ok()
        .map(Rc::new);
        self.images.insert(path, img.clone());
        img
    }

    fn make_table(
        &mut self,
        lines: &[String],
        infos: &[LineInfo],
        start: usize,
        end: usize,
        width: f32,
    ) -> TableBox {
        let z = self.zoom;
        let (pad_x, pad_y) = (10.0 * z, 6.0 * z);
        let qend = infos[start].qend;
        let aligns =
            markdown::delim_row(&lines[start + 1][infos[start + 1].qend..]).unwrap_or_default();
        let header = markdown::split_cells(&lines[start], qend);
        let ncols = header.len().max(1);
        let size = self.base();
        let mut rows: Vec<(Vec<Cellbox>, f32)> = Vec::new();
        let mut natural = vec![40.0 * z; ncols];
        for (r, li) in (start..end).enumerate() {
            if r == 1 {
                rows.push((Vec::new(), 0.0));
                continue;
            }
            let line = lines[li].as_str();
            let cells = markdown::split_cells(line, infos[li].qend);
            let st = BoxStyle {
                mono: false,
                size,
                weight: if r == 0 { 600 } else { 400 },
                ink: if r == 0 { Ink::Heading } else { Ink::Text },
                callout: false,
            };
            let mut row = Vec::new();
            for (c, nat) in natural.iter_mut().enumerate() {
                let rg = cells.get(c).cloned().unwrap_or(line.len()..line.len());
                let tb = self.make_box(line, rg.start, rg.end, rg.start, st, false, true, 1.0e5);
                let mut m = DWRITE_TEXT_METRICS::default();
                unsafe {
                    let _ = tb.layout.GetMetrics(&mut m);
                }
                *nat = f32::max(*nat, m.widthIncludingTrailingWhitespace + 2.0 * pad_x + 1.0);
                row.push(Cellbox {
                    tb,
                    align: aligns.get(c).copied().unwrap_or(Align::Left),
                });
            }
            rows.push((row, 0.0));
        }
        let total: f32 = natural.iter().sum();
        let col_w: Vec<f32> = if total <= width {
            natural
        } else {
            let min_w = 60.0 * z;
            natural
                .iter()
                .map(|&n| (n * width / total).max(min_w.min(n)))
                .collect()
        };
        let mut col_x = Vec::with_capacity(ncols);
        let mut x = 0.0;
        for w in &col_w {
            col_x.push(x);
            x += w;
        }
        for (cells, h) in rows.iter_mut() {
            for (c, cell) in cells.iter_mut().enumerate() {
                cell.tb.set_width(col_w[c] - 2.0 * pad_x);
                let a = match cell.align {
                    Align::Left => DWRITE_TEXT_ALIGNMENT_LEADING,
                    Align::Center => DWRITE_TEXT_ALIGNMENT_CENTER,
                    Align::Right => DWRITE_TEXT_ALIGNMENT_TRAILING,
                };
                unsafe {
                    let _ = cell.tb.layout.SetTextAlignment(a);
                }
                *h = f32::max(*h, cell.tb.height.get() + 2.0 * pad_y);
            }
        }
        TableBox { col_x, col_w, rows }
    }

    /// Lays out every line. `active` is the inclusive line range being edited.
    pub fn layout(
        &mut self,
        hwnd: HWND,
        lines: &[String],
        infos: &[LineInfo],
        active: (usize, usize),
        source: bool,
        doc_dir: Option<&Path>,
    ) {
        if self.ensure_target(hwnd).is_err() {
            return;
        }
        let z = self.zoom;
        let base = self.base();
        let max_w = 760.0 * z;
        let margin = 48.0 * z;
        self.cw = (self.w - 2.0 * margin).min(max_w).max(120.0);
        self.x0 = ((self.w - self.cw) / 2.0).max(16.0).floor();
        let (x0, cw) = (self.x0, self.cw);
        let (qw, lw, code_pad) = (20.0 * z, 28.0 * z, 16.0 * z);
        let right = x0 + cw;

        let mut old_boxes = std::mem::take(&mut self.boxes);
        let mut old_tables = std::mem::take(&mut self.tables);
        let mut rows = Vec::with_capacity(lines.len());
        self.decor.clear();

        // Code blocks and tables become active as a whole.
        let (mut lo, mut hi) = (active.0.min(lines.len() - 1), active.1.min(lines.len() - 1));
        while lo > 0 && infos[lo].block != 0 && infos[lo - 1].block == infos[lo].block {
            lo -= 1;
        }
        while hi + 1 < lines.len() && infos[hi].block != 0 && infos[hi + 1].block == infos[hi].block
        {
            hi += 1;
        }

        let mut y = 28.0 * z;
        let mut table: Option<Rc<TableBox>> = None;
        for (i, line) in lines.iter().enumerate() {
            let info = infos[i];
            let act = source || (lo..=hi).contains(&i);
            let qx = x0 + info.quote as f32 * qw;
            let lx = qx + info.level as f32 * lw;
            let mut row = Row {
                y,
                h: 0.0,
                top: 0.0,
                x: lx,
                vis: Vis::None,
                active: act,
            };

            let mut hasher = DefaultHasher::new();
            (line, info, act, source, (z * 100.0) as u32).hash(&mut hasher);
            let key = hasher.finish();

            let body = BoxStyle {
                mono: false,
                size: base,
                weight: 400,
                ink: Ink::Text,
                callout: false,
            };
            let mono = BoxStyle {
                mono: true,
                size: base * 0.875,
                weight: 400,
                ink: Ink::Text,
                callout: false,
            };
            // Lines of one paragraph sit closer together than separate blocks.
            let joins = |j: usize| {
                infos.get(j).is_some_and(|o| {
                    o.kind == Kind::Para && o.quote == info.quote && o.level == info.level
                })
            };
            let para_pad = (
                if i > 0 && joins(i - 1) { 0.0 } else { 3.0 * z },
                if joins(i + 1) { 0.0 } else { 3.0 * z },
            );

            // Generic text row: content at `cx`, `pad` above and below.
            let text_row = |this: &mut Self,
                            row: &mut Row,
                            st: BoxStyle,
                            cx: f32,
                            rpad: f32,
                            inline: bool,
                            pad: (f32, f32),
                            old: &mut HashMap<u64, Rc<TextBox>>| {
                let from = if act { 0 } else { info.content };
                let content = if act && !source { info.content } else { from };
                let w0 = right - cx - rpad;
                let tb = this.cached_box(old, key, w0, |this| {
                    let mut tb =
                        this.make_box(line, from, line.len(), content, st, act, inline, w0);
                    if let (false, Kind::List { ordered: true, .. }) = (act, info.kind) {
                        let end = line[info.marker..]
                            .find(['.', ')'])
                            .map(|p| info.marker + p + 1)
                            .unwrap_or(info.marker);
                        let num: Vec<u16> = line[info.marker..end].encode_utf16().collect();
                        let f = this.format(Family::Body, st.size, 400);
                        if let Ok(l) = unsafe { this.dw.CreateTextLayout(&num, &f, 200.0, 100.0) } {
                            let mut m = DWRITE_TEXT_METRICS::default();
                            unsafe {
                                let _ = l.GetMetrics(&mut m);
                            }
                            tb.marker = Some((l, m.width));
                        }
                    }
                    tb
                });
                // While editing, the syntax prefix hangs left so content does not shift.
                let hang = if act && !source { tb.prefix_x } else { 0.0 };
                tb.set_width(w0 + hang);
                row.x = cx - hang;
                row.top = pad.0;
                row.h = tb.height.get() + pad.0 + pad.1;
                row.vis = Vis::Text(tb);
            };

            if source {
                text_row(
                    self,
                    &mut row,
                    mono,
                    x0,
                    0.0,
                    false,
                    (0.0, 0.0),
                    &mut old_boxes,
                );
                row.x = x0;
            } else {
                match info.kind {
                    Kind::Blank => {
                        text_row(
                            self,
                            &mut row,
                            body,
                            qx,
                            0.0,
                            false,
                            (0.0, 0.0),
                            &mut old_boxes,
                        );
                        let bh = (base * 0.75).round();
                        row.top = (bh - row.h) / 2.0;
                        row.h = bh;
                    }
                    Kind::Para => {
                        let mut done = false;
                        if !act {
                            let inl = markdown::parse_inline(line, info.content, line.len());
                            if let [(r, src)] = inl.images.as_slice()
                                && line[info.content..r.start].trim().is_empty()
                                && line[r.end..].trim().is_empty()
                                && let Some(img) = self.load_image(src, doc_dir)
                            {
                                let s = ((right - lx) / img.w).min(1.0);
                                let (w, h) = (img.w * s, img.h * s);
                                row.vis = Vis::Image(img, w, h);
                                row.top = 6.0 * z;
                                row.h = h + 12.0 * z;
                                done = true;
                            }
                        }
                        if !done {
                            let first_in_quote =
                                info.quote > 0 && (i == 0 || infos[i - 1].quote < info.quote);
                            let st = BoxStyle {
                                callout: first_in_quote,
                                ..body
                            };
                            text_row(self, &mut row, st, lx, 0.0, true, para_pad, &mut old_boxes);
                        }
                    }
                    Kind::Heading(n) => {
                        let scale = [2.0, 1.6, 1.3, 1.15, 1.0, 0.9][n as usize - 1];
                        let st = BoxStyle {
                            mono: false,
                            size: base * scale,
                            weight: 600,
                            ink: Ink::Heading,
                            callout: false,
                        };
                        let rule = n <= 2;
                        let top = base * if n <= 2 { 1.0 } else { 0.8 };
                        let bottom = base * 0.3 + if rule { 8.0 * z } else { 0.0 };
                        text_row(
                            self,
                            &mut row,
                            st,
                            lx,
                            0.0,
                            true,
                            (top, bottom),
                            &mut old_boxes,
                        );
                        if rule {
                            self.decor.push(Decor::HLine {
                                x0: qx,
                                x1: right,
                                y: y + row.h - 4.0 * z,
                            });
                        }
                    }
                    Kind::List { task, .. } => {
                        let st = BoxStyle {
                            ink: if task == Some(true) {
                                Ink::Dim
                            } else {
                                Ink::Text
                            },
                            ..body
                        };
                        text_row(
                            self,
                            &mut row,
                            st,
                            lx + lw,
                            0.0,
                            true,
                            (2.0 * z, 2.0 * z),
                            &mut old_boxes,
                        );
                    }
                    Kind::Rule if !act => {
                        row.vis = Vis::Rule;
                        row.h = base * 1.6;
                    }
                    Kind::Rule => text_row(
                        self,
                        &mut row,
                        body,
                        lx,
                        0.0,
                        false,
                        (3.0 * z, 3.0 * z),
                        &mut old_boxes,
                    ),
                    Kind::Fence if !act => row.h = 10.0 * z,
                    Kind::Fence | Kind::Code => {
                        let st = BoxStyle {
                            ink: if act && info.kind == Kind::Fence {
                                Ink::Dim
                            } else {
                                Ink::Text
                            },
                            ..mono
                        };
                        text_row(
                            self,
                            &mut row,
                            st,
                            lx + code_pad,
                            code_pad,
                            false,
                            (0.0, 0.0),
                            &mut old_boxes,
                        );
                    }
                    Kind::Table { row: r } if !act => {
                        if r == 0 || table.is_none() {
                            let end = (i..lines.len())
                                .find(|&j| infos[j].block != info.block)
                                .unwrap_or(lines.len());
                            let mut h = DefaultHasher::new();
                            (
                                &lines[i..end],
                                (cw * 10.0) as u32,
                                info.quote,
                                (z * 100.0) as u32,
                            )
                                .hash(&mut h);
                            let tkey = h.finish();
                            let tb = match old_tables
                                .remove(&tkey)
                                .or_else(|| self.tables.get(&tkey).cloned())
                            {
                                Some(t) => t,
                                None => Rc::new(self.make_table(lines, infos, i, end, right - qx)),
                            };
                            self.tables.insert(tkey, tb.clone());
                            table = Some(tb);
                        }
                        let t = table.clone().unwrap();
                        let r = (r as usize).min(t.rows.len() - 1);
                        row.h = t.rows[r].1;
                        row.x = qx;
                        row.vis = Vis::Table(t, r);
                    }
                    Kind::Table { .. } => text_row(
                        self,
                        &mut row,
                        body,
                        lx,
                        0.0,
                        true,
                        (3.0 * z, 3.0 * z),
                        &mut old_boxes,
                    ),
                    Kind::Comment => {
                        let st = BoxStyle {
                            ink: Ink::Dim,
                            ..body
                        };
                        text_row(
                            self,
                            &mut row,
                            st,
                            lx,
                            0.0,
                            false,
                            (0.0, 0.0),
                            &mut old_boxes,
                        );
                    }
                }
            }
            if !matches!(info.kind, Kind::Table { .. }) {
                table = None;
            }
            y += row.h;
            rows.push(row);
        }

        // Code block backgrounds.
        if !source {
            let mut i = 0;
            while i < rows.len() {
                let b = infos[i].block;
                if b != 0 && matches!(infos[i].kind, Kind::Fence | Kind::Code) {
                    let mut j = i;
                    while j + 1 < rows.len() && infos[j + 1].block == b {
                        j += 1;
                    }
                    let lx = x0 + infos[i].quote as f32 * qw + infos[i].level as f32 * lw;
                    self.decor.push(Decor::Block {
                        x0: lx,
                        x1: right,
                        y0: rows[i].y,
                        y1: rows[j].y + rows[j].h,
                    });
                    i = j + 1;
                } else {
                    i += 1;
                }
            }
            // Blockquote bars, colored for callouts such as `> [!NOTE]`.
            let max_q = infos.iter().map(|i| i.quote).max().unwrap_or(0);
            for d in 1..=max_q {
                let mut i = 0;
                while i < rows.len() {
                    if infos[i].quote >= d {
                        let mut j = i;
                        while j + 1 < rows.len() && infos[j + 1].quote >= d {
                            j += 1;
                        }
                        let first = &lines[i][infos[i].qend.min(lines[i].len())..];
                        let ink = callout_ink(first);
                        let x = x0 + (d - 1) as f32 * qw + 2.0 * z;
                        self.decor.push(Decor::Bar {
                            x,
                            y0: rows[i].y,
                            y1: rows[j].y + rows[j].h,
                            ink,
                        });
                        i = j + 1;
                    } else {
                        i += 1;
                    }
                }
            }
        }
        self.total_h = y;
        self.rows = rows;
    }

    pub fn max_scroll(&self) -> f32 {
        (self.total_h + (self.h - STATUS_H) * 0.5 - (self.h - STATUS_H)).max(0.0)
    }

    /// Caret (x, top, height) in document coordinates.
    pub fn caret_rect(&self, p: Pos) -> Option<(f32, f32, f32)> {
        let row = self.rows.get(p.line)?;
        match &row.vis {
            Vis::Text(tb) => {
                let (x, y, h) = tb.caret(tb.s2d(p.col));
                Some((row.x + x, row.y + row.top + y, h))
            }
            _ => Some((row.x, row.y, row.h.max(self.base()))),
        }
    }

    /// For vertical movement within a wrapped row: a y (document coordinates) on the
    /// adjacent visual line, or None when the caret is on the row's first/last line.
    pub fn adjacent_line_y(&self, p: Pos, down: bool) -> Option<f32> {
        let row = self.rows.get(p.line)?;
        let Vis::Text(tb) = &row.vis else { return None };
        let (idx, lines) = tb.visual_lines(tb.s2d(p.col));
        let target = if down { idx + 1 } else { idx.checked_sub(1)? };
        let (top, h) = *lines.get(target)?;
        Some(row.y + row.top + top + h / 2.0)
    }

    /// y inside the first (or last) visual line of row `i`.
    pub fn row_probe_y(&self, i: usize, last: bool) -> f32 {
        let row = &self.rows[i];
        let y = match &row.vis {
            Vis::Text(tb) => {
                let top = row.y + row.top;
                if last {
                    top + tb.height.get() - 2.0
                } else {
                    top + 2.0
                }
            }
            _ => row.y + row.h / 2.0,
        };
        // Blank rows are shorter than their text box; stay inside the row.
        y.clamp(row.y + 0.5, row.y + row.h - 0.5)
    }

    pub fn row_at(&self, y: f32) -> usize {
        self.rows
            .partition_point(|r| r.y <= y)
            .saturating_sub(1)
            .min(self.rows.len().saturating_sub(1))
    }

    pub fn hit(&self, lines: &[String], infos: &[LineInfo], x: f32, y: f32) -> HitResult {
        let i = self.row_at(y);
        let row = &self.rows[i];
        let line = &lines[i];
        let mut res = HitResult {
            pos: Pos::new(i, 0),
            link: None,
            checkbox: false,
        };
        match &row.vis {
            Vis::Text(tb) => {
                let (col, link) = tb.hit(x - row.x, y - row.y - row.top);
                res.pos.col = col;
                res.link = link;
                if !row.active
                    && let Kind::List { task: Some(_), .. } = infos[i].kind
                {
                    let s = self.base();
                    res.checkbox = x >= row.x - s * 1.6 && x < row.x - s * 0.2;
                }
            }
            Vis::Table(t, r) => {
                let pad_x = 10.0 * self.zoom;
                let (cells, h) = (&t.rows[*r].0, t.rows[*r].1);
                let c = t
                    .col_x
                    .partition_point(|&cx| row.x + cx <= x)
                    .saturating_sub(1);
                if let Some(cell) = cells.get(c) {
                    let top = row.y + (h - cell.tb.height.get()) / 2.0;
                    let (col, link) = cell.tb.hit(x - row.x - t.col_x[c] - pad_x, y - top);
                    res.pos.col = col;
                    res.link = link;
                }
            }
            _ => {
                res.pos.col = if x > row.x + 40.0 { line.len() } else { 0 };
            }
        }
        res
    }

    // -----------------------------------------------------------------------
    // Painting

    #[allow(clippy::too_many_arguments)]
    pub fn paint(
        &mut self,
        hwnd: HWND,
        lines: &[String],
        infos: &[LineInfo],
        sel: (Pos, Pos),
        cursor: Pos,
        caret_on: bool,
        scroll: f32,
        status: &Status,
        palette: Option<&PaletteView>,
    ) {
        if self.ensure_target(hwnd).is_err() {
            return;
        }
        let rt = self.rt.clone().unwrap();
        let z = self.zoom;
        let view_h = self.h - STATUS_H;
        unsafe {
            rt.BeginDraw();
            rt.Clear(Some(&color(self.palette()[Ink::Bg as usize])));
            rt.SetTransform(&Matrix3x2::translation(0.0, -scroll));
            rt.PushAxisAlignedClip(
                &rect(0.0, scroll, self.w, scroll + view_h),
                D2D1_ANTIALIAS_MODE_ALIASED,
            );

            let (top, bottom) = (scroll, scroll + view_h);
            for d in &self.decor {
                match *d {
                    Decor::Block { x0, x1, y0, y1 } if y1 >= top && y0 <= bottom => {
                        let rr = D2D1_ROUNDED_RECT {
                            rect: rect(x0, y0, x1, y1),
                            radiusX: 6.0 * z,
                            radiusY: 6.0 * z,
                        };
                        rt.FillRoundedRectangle(&rr, self.brush(Ink::CodeBg));
                    }
                    Decor::Bar { x, y0, y1, ink } if y1 >= top && y0 <= bottom => {
                        rt.FillRectangle(
                            &rect(x, y0 + 2.0, x + 3.0 * z, y1 - 2.0),
                            self.brush(ink),
                        );
                    }
                    Decor::HLine { x0, x1, y } if y >= top && y <= bottom => {
                        rt.FillRectangle(&rect(x0, y, x1, y + 1.0), self.brush(Ink::Rule));
                    }
                    _ => {}
                }
            }

            let first = self.row_at(top);
            for i in first..self.rows.len() {
                let row = &self.rows[i];
                if row.y > bottom {
                    break;
                }
                self.paint_row(&rt, i, row, &lines[i], &infos[i], sel);
            }

            if caret_on && let Some((x, y, h)) = self.caret_rect(cursor) {
                let h = h.max(self.base());
                rt.FillRectangle(
                    &rect(x, y, x + 1.6 * z.max(1.0), y + h),
                    self.brush(Ink::Caret),
                );
            }
            rt.PopAxisAlignedClip();
            rt.SetTransform(&Matrix3x2::identity());

            // Scrollbar.
            let content = self.total_h + view_h * 0.5;
            if content > view_h + 1.0 {
                let th = (view_h * view_h / content).max(30.0);
                let ty = scroll / self.max_scroll().max(1.0) * (view_h - th);
                let rr = D2D1_ROUNDED_RECT {
                    rect: rect(self.w - 8.0, ty + 2.0, self.w - 3.0, ty + th - 2.0),
                    radiusX: 2.5,
                    radiusY: 2.5,
                };
                rt.FillRoundedRectangle(&rr, self.brush(Ink::Scrollbar));
            }

            self.paint_status(&rt, status);
            self.palette_geom = None;
            if let Some(p) = palette {
                self.paint_palette(&rt, p);
            }

            if let Err(e) = rt.EndDraw(None, None)
                && e.code() == D2DERR_RECREATE_TARGET
            {
                self.rt = None;
                self.brushes.clear();
            }
        }
    }

    unsafe fn paint_row(
        &self,
        rt: &ID2D1HwndRenderTarget,
        i: usize,
        row: &Row,
        line: &str,
        info: &LineInfo,
        sel: (Pos, Pos),
    ) {
        let z = self.zoom;
        let base = self.base();
        unsafe {
            match &row.vis {
                Vis::Text(tb) => {
                    let (ox, oy) = (row.x, row.y + row.top);
                    // Selection.
                    if row.active && sel.0 != sel.1 && i >= sel.0.line && i <= sel.1.line {
                        let a = if i == sel.0.line { sel.0.col } else { 0 };
                        let b = if i == sel.1.line {
                            sel.1.col
                        } else {
                            line.len()
                        };
                        let (a16, b16) = (tb.s2d(a), tb.s2d(b));
                        if b16 > a16 {
                            for m in tb.rects(a16, b16) {
                                rt.FillRectangle(
                                    &rect(
                                        ox + m.left,
                                        oy + m.top,
                                        ox + m.left + m.width,
                                        oy + m.top + m.height,
                                    ),
                                    self.brush(Ink::Selection),
                                );
                            }
                        }
                        if i < sel.1.line {
                            let (x, y, h) = tb.caret(b16);
                            rt.FillRectangle(
                                &rect(ox + x, oy + y, ox + x + 6.0 * z, oy + y + h),
                                self.brush(Ink::Selection),
                            );
                        }
                    }
                    for &(a, b, ink) in &tb.bgs {
                        for m in tb.rects(a, b) {
                            let rr = D2D1_ROUNDED_RECT {
                                rect: rect(
                                    ox + m.left - 2.0 * z,
                                    oy + m.top + 1.0,
                                    ox + m.left + m.width + 2.0 * z,
                                    oy + m.top + m.height - 1.0,
                                ),
                                radiusX: 3.0 * z,
                                radiusY: 3.0 * z,
                            };
                            rt.FillRoundedRectangle(&rr, self.brush(ink));
                        }
                    }
                    rt.DrawTextLayout(
                        v2(ox, oy),
                        &tb.layout,
                        self.brush(tb.ink),
                        D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                    );

                    if !row.active
                        && let Kind::List { ordered, task } = info.kind
                    {
                        let cy = oy + tb.baseline - base * 0.3;
                        match (task, ordered) {
                            (Some(done), _) => {
                                let s = base * 0.85;
                                let cx = ox - base * 0.85;
                                let r =
                                    rect(cx - s / 2.0, cy - s / 2.0, cx + s / 2.0, cy + s / 2.0);
                                let rr = D2D1_ROUNDED_RECT {
                                    rect: r,
                                    radiusX: 3.0 * z,
                                    radiusY: 3.0 * z,
                                };
                                if done {
                                    rt.FillRoundedRectangle(&rr, self.brush(Ink::Accent));
                                    let b = self.brush(Ink::Bg);
                                    rt.DrawLine(
                                        v2(r.left + s * 0.22, cy + s * 0.02),
                                        v2(r.left + s * 0.42, cy + s * 0.22),
                                        b,
                                        2.0 * z,
                                        None,
                                    );
                                    rt.DrawLine(
                                        v2(r.left + s * 0.42, cy + s * 0.22),
                                        v2(r.left + s * 0.78, cy - s * 0.2),
                                        b,
                                        2.0 * z,
                                        None,
                                    );
                                } else {
                                    rt.DrawRoundedRectangle(
                                        &rr,
                                        self.brush(Ink::Dim),
                                        1.5 * z,
                                        None,
                                    );
                                }
                            }
                            (None, true) => {
                                if let Some((l, w)) = &tb.marker {
                                    rt.DrawTextLayout(
                                        v2(ox - 6.0 * z - w, oy),
                                        l,
                                        self.brush(Ink::Dim),
                                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                                    );
                                }
                            }
                            (None, false) => {
                                let cx = ox - base * 0.75;
                                let r = base * 0.17;
                                let e = D2D1_ELLIPSE {
                                    point: v2(cx, cy),
                                    radiusX: r,
                                    radiusY: r,
                                };
                                match info.level % 3 {
                                    0 => rt.FillEllipse(&e, self.brush(Ink::Text)),
                                    1 => rt.DrawEllipse(&e, self.brush(Ink::Text), 1.2 * z, None),
                                    _ => rt.FillRectangle(
                                        &rect(cx - r, cy - r, cx + r, cy + r),
                                        self.brush(Ink::Text),
                                    ),
                                }
                            }
                        }
                    }
                }
                Vis::Rule => {
                    let y = row.y + row.h / 2.0;
                    rt.FillRectangle(
                        &rect(row.x, y, self.x0 + self.cw, y + 1.0),
                        self.brush(Ink::Rule),
                    );
                }
                Vis::Table(t, r) => {
                    let (cells, h) = (&t.rows[*r].0, t.rows[*r].1);
                    if h <= 0.0 {
                        return;
                    }
                    let pad_x = 10.0 * z;
                    let border = self.brush(Ink::Rule);
                    let total_w: f32 = t.col_w.iter().sum();
                    if *r == 0 {
                        rt.FillRectangle(
                            &rect(row.x, row.y, row.x + total_w, row.y + h),
                            self.brush(Ink::TableHeader),
                        );
                    }
                    for (c, cell) in cells.iter().enumerate() {
                        let cx = row.x + t.col_x[c];
                        rt.DrawRectangle(
                            &rect(
                                cx + 0.5,
                                row.y + 0.5,
                                cx + t.col_w[c] - 0.5,
                                row.y + h - 0.5,
                            ),
                            border,
                            1.0,
                            None,
                        );
                        // Center vertically: cells of only code have a shorter line.
                        let (ox, oy) = (cx + pad_x, row.y + (h - cell.tb.height.get()) / 2.0);
                        for &(a, b, ink) in &cell.tb.bgs {
                            for m in cell.tb.rects(a, b) {
                                rt.FillRectangle(
                                    &rect(
                                        ox + m.left - 2.0,
                                        oy + m.top + 1.0,
                                        ox + m.left + m.width + 2.0,
                                        oy + m.top + m.height - 1.0,
                                    ),
                                    self.brush(ink),
                                );
                            }
                        }
                        rt.DrawTextLayout(
                            v2(ox, oy),
                            &cell.tb.layout,
                            self.brush(cell.tb.ink),
                            D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                        );
                    }
                }
                Vis::Image(img, w, h) => {
                    let r = rect(row.x, row.y + row.top, row.x + w, row.y + row.top + h);
                    rt.DrawBitmap(
                        &img.bmp,
                        Some(&r),
                        1.0,
                        D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
                        None,
                    );
                }
                Vis::None => {}
            }
        }
    }

    fn preview_format(&mut self, family: &str, size: f32) -> IDWriteTextFormat {
        if let Some(f) = self.preview_formats.get(family) {
            return f.clone();
        }
        let f = unsafe {
            self.dw.CreateTextFormat(
                &HSTRING::from(family),
                None,
                DWRITE_FONT_WEIGHT_NORMAL,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!("en-us"),
            )
        };
        let f = f.unwrap_or_else(|_| self.format(Family::Ui, size, 400));
        self.preview_formats.insert(family.to_string(), f.clone());
        f
    }

    /// Draws the command palette near the top of the window.
    unsafe fn paint_palette(&mut self, rt: &ID2D1HwndRenderTarget, p: &PaletteView) {
        let pw = (self.w - 48.0).clamp(240.0, 600.0);
        let left = ((self.w - pw) / 2.0).floor();
        let right = left + pw;
        let top = 40.0f32.min(self.h * 0.08).floor();
        let (input_h, item_h, pad) = (44.0, 34.0, 6.0);
        let rows = p.items.len().max(1);
        let bottom = top + input_h + pad + rows as f32 * item_h + pad;
        self.palette_geom = Some(PaletteGeom {
            left,
            top,
            right,
            bottom,
            items_top: top + input_h + pad,
            item_h,
        });
        let ui = self.format(Family::Ui, 15.0, 400);
        let small = self.format(Family::Ui, 12.5, 400);
        let tx = left + 16.0;
        let tw = pw - 32.0;
        unsafe {
            // Soft shadow, panel, border.
            for k in 1..=4 {
                let o = k as f32 * 1.5;
                let rr = D2D1_ROUNDED_RECT {
                    rect: rect(left - o, top - o + 3.0, right + o, bottom + o + 3.0),
                    radiusX: 8.0 + o,
                    radiusY: 8.0 + o,
                };
                rt.FillRoundedRectangle(&rr, self.brush(Ink::Shadow));
            }
            let panel = D2D1_ROUNDED_RECT {
                rect: rect(left, top, right, bottom),
                radiusX: 8.0,
                radiusY: 8.0,
            };
            rt.FillRoundedRectangle(&panel, self.brush(Ink::PaletteBg));
            rt.DrawRoundedRectangle(&panel, self.brush(Ink::Rule), 1.0, None);

            // Query input with caret (placeholder when empty).
            let (text, ink) = if p.query.is_empty() {
                (p.placeholder, Ink::Dim)
            } else {
                (p.query, Ink::Text)
            };
            let wide: Vec<u16> = text.encode_utf16().collect();
            if let Ok(l) = self.dw.CreateTextLayout(&wide, &ui, tw, input_h) {
                let _ = l.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                let _ = l.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                rt.DrawTextLayout(
                    v2(tx, top),
                    &l,
                    self.brush(ink),
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
                let i = p.query[..p.cur].encode_utf16().count() as u32;
                let (mut x, mut y) = (0.0, 0.0);
                let mut m = DWRITE_HIT_TEST_METRICS::default();
                let _ = l.HitTestTextPosition(i, false, &mut x, &mut y, &mut m);
                rt.FillRectangle(
                    &rect(tx + x, top + y, tx + x + 1.5, top + y + m.height),
                    self.brush(Ink::Caret),
                );
            }
            rt.FillRectangle(
                &rect(left + 1.0, top + input_h, right - 1.0, top + input_h + 1.0),
                self.brush(Ink::Rule),
            );

            let mut y = top + input_h + pad;
            if p.items.is_empty() {
                let wide: Vec<u16> = p.empty.encode_utf16().collect();
                if let Ok(l) = self.dw.CreateTextLayout(&wide, &small, tw, item_h) {
                    let _ = l.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                    rt.DrawTextLayout(
                        v2(tx, y),
                        &l,
                        self.brush(Ink::Dim),
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                    );
                }
            }
            for (i, item) in p.items.iter().enumerate() {
                if p.selected == Some(i) {
                    let rr = D2D1_ROUNDED_RECT {
                        rect: rect(left + pad, y, right - pad, y + item_h),
                        radiusX: 5.0,
                        radiusY: 5.0,
                    };
                    rt.FillRoundedRectangle(&rr, self.brush(Ink::PaletteSel));
                }
                let detail_w = if item.detail.is_empty() {
                    0.0
                } else {
                    170.0f32.min(tw * 0.4)
                };
                if !item.detail.is_empty() {
                    let wide: Vec<u16> = item.detail.encode_utf16().collect();
                    if let Ok(l) = self.dw.CreateTextLayout(&wide, &small, detail_w, item_h) {
                        let _ = l.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING);
                        let _ = l.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                        let _ = l.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                        rt.DrawTextLayout(
                            v2(right - 16.0 - detail_w, y),
                            &l,
                            self.brush(Ink::Dim),
                            D2D1_DRAW_TEXT_OPTIONS_NONE,
                        );
                    }
                }
                let f = match item.font {
                    Some(name) => self.preview_format(name, 15.0),
                    None => ui.clone(),
                };
                let wide: Vec<u16> = item.label.encode_utf16().collect();
                if let Ok(l) = self
                    .dw
                    .CreateTextLayout(&wide, &f, tw - detail_w - 8.0, item_h)
                {
                    let _ = l.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                    let _ = l.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                    let trim = DWRITE_TRIMMING {
                        granularity: DWRITE_TRIMMING_GRANULARITY_CHARACTER,
                        delimiter: 0,
                        delimiterCount: 0,
                    };
                    let _ = l.SetTrimming(&trim, None);
                    for &b in item.matches {
                        let Some(ch) = item.label[b..].chars().next() else {
                            continue;
                        };
                        let at = item.label[..b].encode_utf16().count() as u32;
                        let r = range(at, at + ch.len_utf16() as u32);
                        let _ = l.SetFontWeight(DWRITE_FONT_WEIGHT_BOLD, r);
                        let _ = l.SetDrawingEffect(self.brush(Ink::Accent), r);
                    }
                    rt.DrawTextLayout(
                        v2(tx, y),
                        &l,
                        self.brush(Ink::Text),
                        D2D1_DRAW_TEXT_OPTIONS_ENABLE_COLOR_FONT,
                    );
                }
                y += item_h;
            }
        }
    }

    unsafe fn paint_status(&mut self, rt: &ID2D1HwndRenderTarget, status: &Status) {
        let (w, h) = (self.w, self.h);
        let y0 = h - STATUS_H;
        unsafe {
            rt.FillRectangle(&rect(0.0, y0, w, h), self.brush(Ink::StatusBg));
            rt.FillRectangle(&rect(0.0, y0, w, y0 + 1.0), self.brush(Ink::Rule));
            let f = self.format(Family::Ui, 12.5, 400);
            let pad = 12.0;
            let left: Vec<u16> = status
                .prompt
                .map(|p| p.0)
                .unwrap_or(status.left)
                .encode_utf16()
                .collect();
            let right: Vec<u16> = status.right.encode_utf16().collect();
            if let Ok(rl) = self.dw.CreateTextLayout(&right, &f, w / 2.0, STATUS_H) {
                let _ = rl.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_TRAILING);
                let _ = rl.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                let _ = rl.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                rt.DrawTextLayout(
                    v2(w / 2.0 - pad, y0),
                    &rl,
                    self.brush(Ink::StatusText),
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
            if let Ok(ll) = self.dw.CreateTextLayout(&left, &f, w / 2.0, STATUS_H) {
                let _ = ll.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP);
                let _ = ll.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER);
                let ink = if status.prompt.is_some() {
                    Ink::Text
                } else {
                    Ink::StatusText
                };
                rt.DrawTextLayout(
                    v2(pad, y0),
                    &ll,
                    self.brush(ink),
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
                if let Some((text, cur)) = status.prompt {
                    let i = text[..cur].encode_utf16().count() as u32;
                    let (mut x, mut y) = (0.0, 0.0);
                    let mut m = DWRITE_HIT_TEST_METRICS::default();
                    let _ = ll.HitTestTextPosition(i, false, &mut x, &mut y, &mut m);
                    rt.FillRectangle(
                        &rect(pad + x, y0 + y, pad + x + 1.5, y0 + y + m.height),
                        self.brush(Ink::Caret),
                    );
                }
            }
        }
    }
}

fn callout_ink(first: &str) -> Ink {
    let t = first.trim_start().to_ascii_lowercase();
    let Some(rest) = t.strip_prefix("[!") else {
        return Ink::Quote;
    };
    let kind = rest.split(']').next().unwrap_or("");
    match kind {
        "note" | "info" | "abstract" | "summary" => Ink::Note,
        "tip" | "hint" | "success" | "check" | "done" => Ink::Tip,
        "warning" | "attention" => Ink::Warning,
        "caution" | "danger" | "error" | "bug" | "failure" => Ink::Danger,
        "important" | "question" | "example" => Ink::Important,
        _ => Ink::Quote,
    }
}
