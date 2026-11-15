//! Look of MyTune: palette derived from the accent exactly like `Services/Accent.cs`,
//! fonts and sizes from `Themes/Fluent.xaml`. Glass look (since 2026-11-12): rounded, translucent panels with a
//! thin bright rim over soft colour blobs (`backdrop`); the blobs take the accent's hue.
use eframe::egui::{self, Color32, CornerRadius, FontFamily, FontId, Margin, Pos2, Rect, RichText, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use std::sync::RwLock;

#[derive(Clone, Copy)]
pub struct Palette {
    pub accent: Color32,
    pub accent_dark: Color32,
    pub accent_deep: Color32,
    pub on_accent: Color32,
    pub accent_soft: Color32,
    pub side: Color32,
    pub side_selected: Color32,
    pub side_text: Color32,
    pub side_muted: Color32,
    pub layer_bg: Color32,
    pub card_bg: Color32,
    pub card_hover: Color32,
    pub ink: Color32,
    pub hero_bg: Color32,
    /// red, crimson, maroon, amber, orange
    pub block: [Color32; 5],
}

pub const TEXT: Color32 = Color32::WHITE;
pub fn text2() -> Color32 { Color32::from_white_alpha(0xCC) }
pub fn text3() -> Color32 { Color32::from_white_alpha(0xB0) } // 4.5:1 on the brightest backdrop spot under glass
pub fn control_bg() -> Color32 { Color32::from_white_alpha(0x14) }
pub fn control_border() -> Color32 { Color32::from_white_alpha(0x33) }
pub const SUCCESS: Color32 = Color32::from_rgb(0x6C, 0xCB, 0x5F);
pub const WARNING: Color32 = Color32::from_rgb(0xFF, 0xD4, 0x00);
pub const DANGER: Color32 = Color32::from_rgb(0xFF, 0x7A, 0x6B);
pub fn success_soft() -> Color32 { Color32::from_rgba_unmultiplied(0x6C, 0xCB, 0x5F, 0x26) }
pub fn warning_soft() -> Color32 { Color32::from_rgba_unmultiplied(0xFF, 0xD4, 0x00, 0x26) }
pub fn danger_soft() -> Color32 { Color32::from_rgba_unmultiplied(0xFF, 0x7A, 0x6B, 0x26) }

thread_local! {
    static MIN_H: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) };
}

/// Minimum outer height of the next cards (for UniformGrid rows).
pub fn set_min_card_height(h: f32) {
    MIN_H.with(|c| c.set(h));
}

static PAL: RwLock<Option<Palette>> = RwLock::new(None);

pub fn p() -> Palette {
    PAL.read().unwrap().unwrap_or_else(|| Palette::from_hex("#FF5A0A"))
}

fn scale(c: [f32; 3], f: f32) -> Color32 {
    Color32::from_rgb((c[0] * f).clamp(0.0, 255.0) as u8, (c[1] * f).clamp(0.0, 255.0) as u8, (c[2] * f).clamp(0.0, 255.0) as u8)
}

fn to_hsv(c: [f32; 3]) -> (f32, f32, f32) {
    let (r, g, b) = (c[0] / 255.0, c[1] / 255.0, c[2] / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let h = if d == 0.0 {
        0.0
    } else if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    (h / 6.0, if max == 0.0 { 0.0 } else { d / max }, max)
}

fn from_hsv(h: f32, s: f32, v: f32) -> Color32 {
    let h = ((h % 1.0) + 1.0) % 1.0;
    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let (pp, q, t) = (v * (1.0 - s), v * (1.0 - f * s), v * (1.0 - (1.0 - f) * s));
    let (r, g, b) = match (i as i32) % 6 {
        0 => (v, t, pp),
        1 => (q, v, pp),
        2 => (pp, v, t),
        3 => (pp, q, v),
        4 => (t, pp, v),
        _ => (v, pp, q),
    };
    Color32::from_rgb((r * 255.0).round() as u8, (g * 255.0).round() as u8, (b * 255.0).round() as u8)
}

impl Palette {
    /// Same table as `Accent.Tints`; "#60CDFF" (old default) means orange.
    pub fn from_hex(hex: &str) -> Palette {
        let h = hex.trim().trim_start_matches('#');
        let h = if h.len() != 6 || h.eq_ignore_ascii_case("60CDFF") { "FF5A0A" } else { h };
        let n = u32::from_str_radix(h, 16).unwrap_or(0xFF5A0A);
        let c = [((n >> 16) & 255) as f32, ((n >> 8) & 255) as f32, (n & 255) as f32];
        let (hue, sat, _) = to_hsv(c);
        let k = (sat / 0.3).min(1.0);
        let t = |dh: f32, s: f32, v: f32| from_hsv(hue + dh, s * k, v);
        Palette {
            accent: Color32::from_rgb(c[0] as u8, c[1] as u8, c[2] as u8),
            accent_dark: scale(c, 0.72),
            accent_deep: scale(c, 0.45),
            on_accent: if (0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2]) / 255.0 < 0.62 { Color32::WHITE } else { scale(c, 0.17) }, // white unless the accent is too light for it
            accent_soft: Color32::from_rgba_unmultiplied(c[0] as u8, c[1] as u8, c[2] as u8, 0x26),
            // glass: side, side_selected and card_bg are translucent white, drawn over `backdrop`
            side: Color32::from_white_alpha(0x0D),
            side_selected: Color32::from_white_alpha(0x2C),
            side_text: Color32::WHITE,
            side_muted: Color32::from_white_alpha(0xB8),
            layer_bg: t(0.0, 0.35, 0.06),
            card_bg: Color32::from_white_alpha(0x0D),
            card_hover: t(0.0, 0.3, 0.17),
            ink: t(0.0, 0.5, 0.045),
            hero_bg: t(0.0, 0.7, 0.42),
            block: [t(-0.02, 0.8, 0.6), t(-0.05, 0.9, 0.45), t(-0.02, 0.85, 0.32), t(0.05, 0.8, 0.66), t(0.0, 0.75, 0.55)],
        }
    }
}

pub fn set_accent_value(hex: &str) {
    *PAL.write().unwrap() = Some(Palette::from_hex(hex));
}

/// Pictures drawn smaller than their pixels (avatars, server icons, emoji, attachments): mipmapped, so shrinking
/// them averages pixels instead of skipping them. Not for video frames, which change every frame.
pub const SMOOTH: egui::TextureOptions = egui::TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear));

/// A filled circle whose centre sits where its edge falls on whole pixels (even diameters on a pixel corner, odd
/// ones on a pixel centre), so small dots and rings look round and even rather than soft on one side.
pub fn dot(p: &egui::Painter, c: Pos2, r: f32, color: Color32) {
    let ppp = p.pixels_per_point();
    let d = (2.0 * r * ppp).round();
    let snap = |v: f32| if d as i32 % 2 == 0 { (v * ppp).round() / ppp } else { ((v * ppp).floor() + 0.5) / ppp };
    p.circle_filled(pos2(snap(c.x), snap(c.y)), d / ppp / 2.0, color);
}

// ---------------------------------------------------------------- fonts

pub fn semibold() -> FontFamily { FontFamily::Name("semibold".into()) }
pub fn bold() -> FontFamily { FontFamily::Name("bold".into()) }
pub fn display_semibold() -> FontFamily { FontFamily::Name("dsemibold".into()) }
pub fn display_bold() -> FontFamily { FontFamily::Name("dbold".into()) }
/// Dark text on the accent fill: egui boosts glyph coverage for light-on-dark text, so a lighter weight keeps it looking Bold.
pub fn on_light() -> FontFamily { FontFamily::Name("onlight".into()) }
pub fn icons() -> FontFamily { FontFamily::Name("icons".into()) }

/// "Segoe UI Variable" (Text and Display optical sizes, like the WPF font families) from the system font
/// `SegUIVar.ttf`; falls back to the static Segoe UI files, then to egui's own font.
pub fn install_fonts(ctx: &egui::Context) {
    use egui::epaint::text::VariationCoords;
    let mut fonts = egui::FontDefinitions::default();
    let fonts_dir = r"C:\Windows\Fonts\";
    // Mapped from disk, not read into memory: about 17 MB of font files (12 MB of emoji alone) stay out of
    // the app's own memory, and only the parts in use are loaded.
    let read = |f: &str| -> Option<std::sync::Arc<memmap2::Mmap>> {
        let file = std::fs::File::open(format!("{fonts_dir}{f}")).ok()?;
        // SAFETY: Windows font files are not rewritten while mapped (the map keeps them open).
        unsafe { memmap2::Mmap::map(&file) }.ok().map(std::sync::Arc::new)
    };
    let base = fonts.families[&FontFamily::Proportional].clone();
    let mut add = |fonts: &mut egui::FontDefinitions, key: &str, data: egui::FontData| {
        fonts.font_data.insert(key.into(), std::sync::Arc::new(data));
    };
    if let Some(bytes) = read("SegUIVar.ttf") {
        let blob = bytes;
        // (family, key, weight, optical size)
        let variants = [("regular", 400.0, 10.5), ("semibold", 600.0, 10.5), ("bold", 700.0, 10.5), ("dsemibold", 600.0, 36.0), ("dbold", 700.0, 36.0), ("onlight", 560.0, 10.5)];
        for (name, wght, opsz) in variants {
            let key = format!("segoe-var-{name}");
            let tweak = egui::FontTweak { coords: VariationCoords::new([(b"wght", wght), (b"opsz", opsz)]), ..Default::default() };
            add(&mut fonts, &key, egui::FontData::from_blob(blob.clone(), 0).tweak(tweak));
            let mut list = base.clone();
            list.insert(0, key);
            if name == "regular" {
                fonts.families.insert(FontFamily::Proportional, list);
            } else {
                fonts.families.insert(FontFamily::Name(name.into()), list);
            }
        }
    } else {
        for (name, file) in [("regular", "segoeui.ttf"), ("semibold", "seguisb.ttf"), ("bold", "segoeuib.ttf"), ("dsemibold", "seguisb.ttf"), ("dbold", "segoeuib.ttf"), ("onlight", "seguisb.ttf")] {
            let mut list = base.clone();
            if let Some(b) = read(file) {
                let key = format!("segoe-static-{name}");
                add(&mut fonts, &key, egui::FontData::from_blob(b, 0));
                list.insert(0, key);
            }
            if name == "regular" {
                fonts.families.insert(FontFamily::Proportional, list);
            } else {
                fonts.families.insert(FontFamily::Name(name.into()), list);
            }
        }
    }
    let mut list = base;
    if let Some(b) = read("SegoeIcons.ttf") {
        add(&mut fonts, "segoe-icons", egui::FontData::from_blob(b, 0));
        list.insert(0, "segoe-icons".into());
    }
    fonts.families.insert(FontFamily::Name("icons".into()), list);
    // Chat names and messages hold symbols Segoe UI lacks (e.g. U+271E): last-resort fallback, ~1 MB.
    if let Some(b) = read("seguisym.ttf") {
        add(&mut fonts, "segoe-symbols", egui::FontData::from_blob(b, 0));
        for fam in [FontFamily::Proportional, FontFamily::Name("semibold".into()), FontFamily::Name("bold".into())] {
            if let Some(l) = fonts.families.get_mut(&fam) {
                l.push("segoe-symbols".into());
            }
        }
    }
    // Emoji in reactions, names and messages: Segoe UI Emoji's outlines (egui draws them one colour). ~12 MB.
    if let Some(b) = read("seguiemj.ttf") {
        add(&mut fonts, "segoe-emoji", egui::FontData::from_blob(b, 0));
        for fam in [FontFamily::Proportional, FontFamily::Name("semibold".into()), FontFamily::Name("bold".into())] {
            if let Some(l) = fonts.families.get_mut(&fam) {
                l.push("segoe-emoji".into());
            }
        }
    }
    ctx.set_fonts(fonts);
}

pub fn rt(s: impl Into<String>, size: f32) -> RichText { RichText::new(s).size(size) }
pub fn sb(s: impl Into<String>, size: f32) -> RichText { RichText::new(s).size(size).family(semibold()) }
pub fn bd(s: impl Into<String>, size: f32) -> RichText { RichText::new(s).size(size).family(bold()) }
/// Display optical size (PageTitle, hero, section titles, nav headers, big numbers).
pub fn dsb(s: impl Into<String>, size: f32) -> RichText { RichText::new(s).size(size).family(display_semibold()) }
pub fn dbd(s: impl Into<String>, size: f32) -> RichText { RichText::new(s).size(size).family(display_bold()) }
pub fn ico(glyph: &str, size: f32) -> RichText { RichText::new(glyph).size(size).family(icons()) }

/// A soft round colour blob: centre `a` opaque, fading to nothing at radius `r` (rings of a triangle fan).
pub fn blob(p: &egui::Painter, c: Pos2, r: f32, col: Color32, a: f32) {
    const N: usize = 64;
    const R: usize = 20;
    let at = |i: usize| {
        let k = 1.0 - i as f32 / R as f32;
        Color32::from_rgba_unmultiplied(col.r(), col.g(), col.b(), (a * k * k * k * 255.0) as u8)
    };
    let mut m = egui::Mesh::default();
    m.colored_vertex(c, at(0));
    for i in 1..=R {
        for j in 0..N {
            let ang = j as f32 / N as f32 * std::f32::consts::TAU;
            m.colored_vertex(c + vec2(ang.cos(), ang.sin()) * r * (i as f32 / R as f32), at(i));
        }
    }
    let ix = |i: usize, j: usize| (1 + (i - 1) * N + j % N) as u32;
    for j in 0..N {
        m.add_triangle(0, ix(1, j), ix(1, j + 1));
    }
    for i in 1..R {
        for j in 0..N {
            m.add_triangle(ix(i, j), ix(i + 1, j), ix(i, j + 1));
            m.add_triangle(ix(i, j + 1), ix(i + 1, j), ix(i + 1, j + 1));
        }
    }
    p.add(egui::Shape::mesh(m));
}

/// Rounds the four corners of `rect` by painting what lies outside the arcs in the colour `bg` gives for each
/// point (a fan from each corner), so the corners match whatever is behind the card.
pub fn round_corners(p: &egui::Painter, rect: Rect, r: f32, bg: &dyn Fn(Pos2) -> Color32) {
    for corner in [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()] {
        let (sx, sy) = ((rect.center().x - corner.x).signum(), (rect.center().y - corner.y).signum());
        let c = corner + vec2(sx * r, sy * r);
        let mut m = egui::Mesh::default();
        m.colored_vertex(corner, bg(corner));
        const N: u32 = 28;
        for k in 0..=N {
            // arc from one edge point to the other
            let t = k as f32 / N as f32 * std::f32::consts::FRAC_PI_2;
            let q = c + vec2(-sx * r * t.sin(), -sy * r * t.cos());
            m.colored_vertex(q, bg(q));
        }
        for k in 0..=N {
            // the same arc 1 px further in, clear: the cover fades out instead of ending in steps
            let t = k as f32 / N as f32 * std::f32::consts::FRAC_PI_2;
            m.colored_vertex(c + vec2(-sx * (r - 1.0) * t.sin(), -sy * (r - 1.0) * t.cos()), Color32::TRANSPARENT);
        }
        for k in 1..=N {
            m.add_triangle(0, k, k + 1);
            m.add_triangle(k, N + 1 + k, k + 1);
            m.add_triangle(k + 1, N + 1 + k, N + 2 + k);
        }
        p.add(egui::Shape::mesh(m));
    }
}

/// The four backdrop blobs for a window rect: centre, radius, colour, opacity.
fn blobs(rect: Rect) -> [(Pos2, f32, Color32, f32); 4] {
    let a = p_accent_hsv();
    let big = rect.width().max(rect.height());
    let at = |fx: f32, fy: f32| pos2(rect.left() + rect.width() * fx, rect.top() + rect.height() * fy);
    // soft colour behind the glass stands in for blur; `backdrop_at` keeps it dark enough for text
    [(0.92, 0.05, 0.66, 0.0, 0.6, 0.8, 0.7), (0.08, 1.0, 0.62, 0.14, 0.55, 0.7, 0.6), (0.62, 1.05, 0.42, -0.12, 0.5, 0.7, 0.5), (0.3, 0.1, 0.3, 0.07, 0.45, 0.7, 0.36)]
        .map(|(fx, fy, r, dh, s, v, al)| (at(fx, fy), big * r, from_hsv(a.0 + dh, s * a.1, v), al))
}

/// Relative luminance (WCAG) of gamma-space RGB in 0..=255.
fn lum_f(c: [f32; 3]) -> f32 {
    let f = |v: f32| {
        let x = v / 255.0;
        if x <= 0.04045 { x / 12.92 } else { ((x + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2])
}

/// Relative luminance (WCAG) of an opaque colour.
fn lum(c: Color32) -> f32 {
    lum_f([c.r() as f32, c.g() as f32, c.b() as f32])
}

/// Brightest the backdrop may get: white, `text2` and `text3` keep 4.5:1 on a base panel over it (see the test).
const BACKDROP_LUM: f32 = 0.022;

/// Page background: dark base with four soft blobs in the accent's hue (a grey accent gets a calm blue).
/// Rendered once into a half-size dithered texture and redrawn as one quad; it is made again only when the
/// window size (in 16 px steps) or the accent changes.
pub fn backdrop(ctx: &egui::Context, p: &egui::Painter, rect: Rect) {
    glow_textures(ctx);
    let (w, h) = (((rect.width() / 16.0).ceil() as usize).max(1), ((rect.height() / 16.0).ceil() as usize).max(1));
    let key = (w, h, self::p().accent.to_array(), self::p().layer_bg.to_array());
    let id = egui::Id::new("backdrop_tex");
    let cached = ctx.data(|d| d.get_temp::<((usize, usize, [u8; 4], [u8; 4]), egui::TextureHandle)>(id));
    let tex = match cached {
        Some((k, t)) if k == key => t,
        _ => {
            let t = ctx.load_texture("backdrop", backdrop_image(rect, w * 8, h * 8), egui::TextureOptions::LINEAR);
            ctx.data_mut(|d| d.insert_temp(id, (key, t.clone())));
            t
        }
    };
    p.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
}

/// Interleaved gradient noise in 0..1 for a pixel: even, with no pattern the eye picks up. Added before a
/// colour is cut to 8 bits, it turns the steps of a slow gradient into grain too fine to see.
fn noise(x: usize, y: usize) -> f32 {
    (52.982_918 * (0.067_110_56 * x as f32 + 0.005_837_15 * y as f32).fract()).fract()
}

const GLOW_W: usize = 64;
const GLOW_H: usize = 2048;

thread_local! {
    /// Pixels per point and, per light strength, the picture `sheen_into` draws the light on a plate with.
    static GLOW: std::cell::RefCell<(f32, Vec<(u8, egui::TextureId)>)> = const { std::cell::RefCell::new((1.0, Vec::new())) };
}

/// The light inside a glass plate fades from its top to its bottom. On a tall plate that is one 8-bit step
/// every 40 px or so, which showed as stripes, so the fade is a picture rounded with noise: white, strongest
/// in its first row and clear in its last, repeating sideways. Made once per strength; call every frame.
fn glow_textures(ctx: &egui::Context) {
    let id = egui::Id::new("glow_tex");
    let tex = ctx.data(|d| d.get_temp::<std::sync::Arc<Vec<(u8, egui::TextureHandle)>>>(id)).unwrap_or_else(|| {
        let opts = egui::TextureOptions { magnification: egui::TextureFilter::Nearest, minification: egui::TextureFilter::Nearest, wrap_mode: egui::TextureWrapMode::Repeat, mipmap_mode: None };
        let made = [Glass::Base, Glass::Raised, Glass::Float].map(|l| {
            let glow = l.light().2;
            let mut px = Vec::with_capacity(GLOW_W * GLOW_H * 4);
            for y in 0..GLOW_H {
                for x in 0..GLOW_W {
                    // the right half holds the opposite noise: two lights on one plate (`material` under,
                    // `sheen` over) round up on different pixels, so together they stay within one step
                    let n = if x < GLOW_W / 2 { noise(x, y) } else { 1.0 - noise(x - GLOW_W / 2, y) };
                    let a = (glow as f32 * (1.0 - (y as f32 + 0.5) / GLOW_H as f32) + n) as u8;
                    px.extend([a; 4]);
                }
            }
            (glow, ctx.load_texture("glow", egui::ColorImage::from_rgba_premultiplied([GLOW_W, GLOW_H], &px), opts))
        });
        let made = std::sync::Arc::new(made.to_vec());
        ctx.data_mut(|d| d.insert_temp(id, made.clone()));
        made
    });
    GLOW.with(|g| *g.borrow_mut() = (ctx.pixels_per_point(), tex.iter().map(|(s, t)| (*s, t.id())).collect()));
}

/// The backdrop for `rect` as an `ow` x `oh` picture (both multiples of 4). The colour is worked out unrounded
/// on a coarse grid (it is smooth, so 4 texels between nodes lose nothing), and every texel is rounded with
/// its own noise: 8-bit steps in so dark a gradient showed as bands and contour shapes.
fn backdrop_image(rect: Rect, ow: usize, oh: usize) -> egui::ColorImage {
    let f = field(rect);
    let (gw, gh) = (ow / 4, oh / 4);
    let mut grid = Vec::with_capacity((gw + 1) * (gh + 1));
    for gy in 0..=gh {
        for gx in 0..=gw {
            grid.push(f(rect.min + vec2(gx as f32 / gw as f32 * rect.width(), gy as f32 / gh as f32 * rect.height())));
        }
    }
    let mut px = Vec::with_capacity(ow * oh * 3);
    for y in 0..oh {
        let v = (y as f32 + 0.5) / 4.0;
        let (j, fy) = ((v as usize).min(gh - 1), v.fract());
        for x in 0..ow {
            let u = (x as f32 + 0.5) / 4.0;
            let (i, fx) = ((u as usize).min(gw - 1), u.fract());
            let n = noise(x, y);
            let at = j * (gw + 1) + i;
            let (a, b, c, d) = (grid[at], grid[at + 1], grid[at + gw + 1], grid[at + gw + 2]);
            for k in 0..3 {
                let (top, bot) = (a[k] + (b[k] - a[k]) * fx, c[k] + (d[k] - c[k]) * fx);
                px.push((top + (bot - top) * fy + n) as u8);
            }
        }
    }
    egui::ColorImage::from_rgb([ow, oh], &px)
}

/// The colour `backdrop` paints at `pos`, for shapes that must blend into it.
pub fn backdrop_at(rect: Rect, pos: Pos2) -> Color32 {
    let c = field(rect)(pos);
    Color32::from_rgb(c[0].round() as u8, c[1].round() as u8, c[2].round() as u8)
}

/// The backdrop colour of a window rect as a function of the point: gamma-space RGB in 0..=255, unrounded,
/// and smooth everywhere (no kink at a blob's centre or edge, nor where the brightness limit starts).
fn field(rect: Rect) -> impl Fn(Pos2) -> [f32; 3] {
    let rgb = |c: Color32| [c.r() as f32, c.g() as f32, c.b() as f32];
    let bg = rgb(self::p().layer_bg);
    let blobs = blobs(rect).map(|(centre, r, col, al)| (centre, r, rgb(col), al));
    move |pos| {
        let mut c = bg;
        for (centre, r, col, al) in blobs {
            // 1 at the centre, 0 at the radius; TIP rounds the centre, which was a cone point
            const TIP: f32 = 0.08;
            let t = pos.distance(centre) / r;
            let k = 1.0 - ((t * t + TIP * TIP).sqrt() - TIP) / ((1.0 + TIP * TIP).sqrt() - TIP);
            if k > 0.0 {
                let a = al * k * k;
                for i in 0..3 {
                    c[i] += (col[i] - c[i]) * a;
                }
            }
        }
        // darker, same hue: the colour stays vivid while text on the glass above stays readable. From half the
        // limit on, brightness bends towards the limit and never reaches it (a hard stop drew its outline).
        let (l, knee) = (lum_f(c), BACKDROP_LUM * 0.5);
        if l > knee {
            let span = BACKDROP_LUM - knee;
            let f = ((knee + span * (1.0 - (-(l - knee) / span).exp())) / l).powf(1.0 / 2.4);
            c = c.map(|v| v * f);
        }
        c
    }
}

fn p_accent_hsv() -> (f32, f32) {
    let c = p().accent;
    let (h, s, _) = to_hsv([c.r() as f32, c.g() as f32, c.b() as f32]);
    if s < 0.1 { (0.6, 0.55) } else { (h, 1.0) }
}

// ---------------------------------------------------------------- glass material

/// The three glass levels, lowest first: a panel on the page, a control on a panel, and what floats over both
/// (menus, popovers, dialogs). Each is a translucent fill with a bright top-left rim, light from the top inside,
/// a darker bottom edge and a soft shadow; a higher level is lighter and its shadow reaches further.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Glass {
    Base,
    Raised,
    Float,
}

/// Corner radii: panels and cards, dialogs, menus and popovers. Controls are capsules (half their height).
pub const R_PANEL: f32 = 22.0;
pub const R_FLOAT: f32 = 22.0;
pub const R_MENU: f32 = 16.0;

/// Radius of a shape that sits `gap` px inside one of radius `outer`, so both curves share a centre.
pub fn nested(outer: f32, gap: f32) -> f32 {
    (outer - gap).max(6.0)
}

impl Glass {
    pub fn fill(self) -> Color32 {
        match self {
            Glass::Base => Color32::from_white_alpha(0x0D),
            Glass::Raised => Color32::from_white_alpha(0x18),
            // egui cannot blur what is behind, so what floats is solid: at 85% the text under a menu showed
            // through its rows and neither could be read
            Glass::Float => tone(0.1),
        }
    }

    pub fn shadow(self) -> egui::Shadow {
        let (y, blur, a) = match self {
            Glass::Base => (10, 36, 84),
            Glass::Raised => (3, 12, 70),
            Glass::Float => (18, 56, 150),
        };
        egui::Shadow { offset: [0, y], blur, spread: 0, color: Color32::from_black_alpha(a) }
    }

    /// Opacity of the rim, of the top-left highlight and of the light inside at the top.
    fn light(self) -> (u8, u8, u8) {
        match self {
            Glass::Base => (0x2A, 0x78, 0x12),
            Glass::Raised => (0x36, 0x88, 0x18),
            Glass::Float => (0x3C, 0x88, 0x0E),
        }
    }
}

/// A glass plate of `level`: shadow, `fill` and the light on it (`sheen`).
pub fn material(rect: Rect, radius: f32, level: Glass, fill: Color32) -> egui::Shape {
    let r = radius.min(rect.height() / 2.0).min(rect.width() / 2.0);
    let mut shapes = Vec::with_capacity(8);
    shapes.push(level.shadow().as_shape(rect, CornerRadius::from(r)).into());
    shapes.push(egui::Shape::rect_filled(rect, r, fill));
    sheen_into(&mut shapes, rect, r, level.light(), false);
    egui::Shape::Vec(shapes)
}

/// A base-level glass plate (cards, the sidebar).
pub fn glass(rect: Rect, radius: f32, fill: Color32) -> egui::Shape {
    material(rect, radius, Glass::Base, fill)
}

/// Only the light on a glass plate, for a frame egui filled itself (windows, modals): painted over its edge.
pub fn sheen(p: &egui::Painter, rect: Rect, radius: f32, level: Glass) {
    let mut shapes = Vec::with_capacity(6);
    sheen_into(&mut shapes, rect, radius.min(rect.height() / 2.0).min(rect.width() / 2.0), level.light(), true);
    p.add(egui::Shape::Vec(shapes));
}

fn sheen_into(out: &mut Vec<egui::Shape>, rect: Rect, r: f32, (rim, hi, glow): (u8, u8, u8), over: bool) {
    use std::f32::consts::{FRAC_PI_2, PI};
    let white = Color32::from_white_alpha;
    // light from above inside the plate: one fan over the rounded outline, fading to nothing at the bottom
    let inner = rect.shrink(1.0);
    let ri = (r - 1.0).max(0.0);
    const SEG: usize = 16; // per corner: fine enough that no corner shows its straight pieces
    let shade = |y: f32| white((glow as f32 * (1.0 - (y - inner.top()) / inner.height().max(1.0))) as u8);
    let mut m = egui::Mesh::default();
    m.colored_vertex(inner.center(), shade(inner.center().y));
    for (cx, cy, a0) in [(inner.left() + ri, inner.top() + ri, PI), (inner.right() - ri, inner.top() + ri, 1.5 * PI), (inner.right() - ri, inner.bottom() - ri, 0.0), (inner.left() + ri, inner.bottom() - ri, FRAC_PI_2)] {
        for k in 0..=SEG {
            let a = a0 + k as f32 / SEG as f32 * FRAC_PI_2;
            let q = pos2(cx + ri * a.cos(), cy + ri * a.sin());
            m.colored_vertex(q, shade(q.y));
        }
    }
    let n = 4 * (SEG as u32 + 1);
    for i in 0..n {
        m.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    let ring: Vec<Pos2> = m.vertices[1..].iter().map(|v| v.pos).collect();
    for q in &ring {
        // 1.5 px further in, at full strength; the vertices above become the clear outer edge
        let c = pos2(q.x.clamp(inner.left() + ri, inner.right() - ri), q.y.clamp(inner.top() + ri, inner.bottom() - ri));
        let d = *q - c;
        let q2 = if d.length() > 1.5 { *q - d.normalized() * 1.5 } else { *q };
        m.colored_vertex(q2, shade(q2.y));
    }
    m.indices.clear();
    for i in 0..n {
        let j = (i + 1) % n;
        m.add_triangle(0, 1 + n + i, 1 + n + j); // the fan, on the inner ring
        m.add_triangle(1 + i, 1 + j, 1 + n + i); // the feather between the rings
        m.add_triangle(1 + j, 1 + n + j, 1 + n + i);
    }
    for v in &mut m.vertices[1..=n as usize] {
        v.color = Color32::TRANSPARENT;
    }
    // with the dithered picture at hand, it carries the fade: the vertices only place it and feather its edge
    let (ppp, tex) = GLOW.with(|g| (g.borrow().0, g.borrow().1.iter().find(|(s, _)| *s == glow).map(|(_, t)| *t)));
    if let Some(tex) = tex {
        for (i, v) in m.vertices.iter_mut().enumerate() {
            v.uv = pos2(v.pos.x * ppp / GLOW_W as f32 + if over { 0.5 } else { 0.0 }, (v.pos.y - inner.top()) / inner.height().max(1.0));
            if !(1..=n as usize).contains(&i) {
                v.color = Color32::WHITE;
            }
        }
        m.texture_id = tex;
    }
    out.push(egui::Shape::mesh(m));
    out.push(egui::Shape::rect_stroke(rect, r, Stroke::new(1.0, white(rim)), StrokeKind::Inside));
    let ra = (r - 1.0).max(0.0);
    let steps = ((r * 0.8) as usize).clamp(6, 32);
    let arc = |cx: f32, cy: f32, a0: f32| (0..=steps).map(move |k| a0 + k as f32 / steps as f32 * FRAC_PI_2).map(move |a| (pos2(cx + ra * a.cos(), cy + ra * a.sin()), 1.0));
    let side = (rect.height() * 0.45).max(r);
    // bright rim: fades in up the left edge, full round the top-left corner, then a specular streak that dies
    // out along the top edge
    let (l, t) = (rect.left() + 1.0, rect.top() + 1.0);
    let (x0, x1) = (rect.left() + r, rect.right() - r);
    let mut pts = Vec::with_capacity(steps + 5);
    pts.push((pos2(l, rect.top() + side), 0.0));
    pts.extend(arc(rect.left() + r, rect.top() + r, PI));
    for (f, a) in [(0.25, 0.5), (0.6, 0.2), (1.0, 0.0)] {
        pts.push((pos2(x0 + (x1 - x0) * f, t), a));
    }
    ribbon(out, &pts, true, hi);
    // darker edge: fades in down the right side, full round the bottom-right corner, fades out along the bottom
    let (rr, b) = (rect.right() - 1.0, rect.bottom() - 1.0);
    pts.clear();
    pts.push((pos2(rr, rect.bottom() - side), 0.0));
    pts.extend(arc(rect.right() - r, rect.bottom() - r, 0.0));
    pts.push((pos2((rr - r - rect.width() * 0.55).max(rect.left() + r), b), 0.0));
    ribbon(out, &pts, false, 0x80);
}

/// A soft line through `pts` (place, strength 0..1) that fades along its length: a strip that has the colour on
/// its middle and nothing 1.7 px to either side, so curves and slopes come out smooth (a plain 1 px line shows
/// its pixels on a bright rim).
fn ribbon(out: &mut Vec<egui::Shape>, pts: &[(Pos2, f32)], white: bool, alpha: u8) {
    const HALF: f32 = 1.7;
    let mut m = egui::Mesh::default();
    let mut rows = 0u32;
    for (i, &(q, k)) in pts.iter().enumerate() {
        let (prev, next) = (pts[i.saturating_sub(1)].0, pts[(i + 1).min(pts.len() - 1)].0);
        let d = next - prev;
        if d.length() < 0.01 {
            continue;
        }
        let n = d.normalized().rot90() * HALF;
        let a = (alpha as f32 * k) as u8;
        let col = if white { Color32::from_white_alpha(a) } else { Color32::from_black_alpha(a) };
        m.colored_vertex(q - n, Color32::TRANSPARENT);
        m.colored_vertex(q, col);
        m.colored_vertex(q + n, Color32::TRANSPARENT);
        if rows > 0 {
            let v = (rows - 1) * 3;
            for (a, b, c) in [(0, 1, 3), (1, 4, 3), (1, 2, 4), (2, 5, 4)] {
                m.add_triangle(v + a, v + b, v + c);
            }
        }
        rows += 1;
    }
    out.push(egui::Shape::mesh(m));
}

/// A raised glass capsule for a control (button, thumb, knob): fill and the same light as a glass plate, so its
/// rim follows the curve (a straight highlight line on top read as a white bar). `rim` sets how strong.
pub fn capsule(p: &egui::Painter, rect: Rect, fill: Color32, rim: u8) {
    let r = (rect.height() / 2.0).min(rect.width() / 2.0);
    let mut shapes = Vec::with_capacity(6);
    shapes.push(egui::Shape::rect_filled(rect, r, fill));
    sheen_into(&mut shapes, rect, r, (rim, (rim as u16 * 2).min(0x90) as u8, 0x18), false);
    p.add(egui::Shape::Vec(shapes));
}

/// A floating glass panel (menu, popover, picker) around `add`: the top level, `margin` px of padding.
pub fn float<R>(ui: &mut Ui, radius: f32, margin: i8, add: impl FnOnce(&mut Ui) -> R) -> R {
    let slot = ui.painter().add(egui::Shape::Noop);
    let out = egui::Frame::new().inner_margin(Margin::same(margin)).show(ui, add);
    ui.painter().set(slot, material(out.response.rect, radius, Glass::Float, Glass::Float.fill()));
    out.inner
}

pub fn apply_visuals(ctx: &egui::Context) {
    let pal = p();
    let mut v = egui::Visuals::dark();
    let zero = CornerRadius::same(12);
    v.override_text_color = Some(TEXT);
    v.panel_fill = pal.layer_bg;
    v.window_fill = Glass::Float.fill();
    v.extreme_bg_color = pal.ink;
    // Dialogs, menus and combo lists are the floating glass level, like the custom popups.
    let soft = Glass::Float.shadow();
    v.window_corner_radius = CornerRadius::same(R_FLOAT as u8);
    v.menu_corner_radius = CornerRadius::same(R_MENU as u8);
    v.window_shadow = soft;
    v.popup_shadow = soft;
    v.window_stroke = Stroke::new(1.0, Color32::from_white_alpha(0x3C));
    v.selection.bg_fill = pal.accent_dark;
    v.selection.stroke = Stroke::new(1.0, TEXT);
    let w = &mut v.widgets;
    for s in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
        s.corner_radius = zero;
    }
    w.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_white_alpha(0x1F));
    w.inactive.bg_fill = control_bg();
    w.inactive.weak_bg_fill = control_bg();
    w.hovered.bg_fill = Color32::from_white_alpha(0x22);
    w.hovered.weak_bg_fill = Color32::from_white_alpha(0x22);
    w.active.bg_fill = Color32::from_white_alpha(0x0A);
    w.active.weak_bg_fill = Color32::from_white_alpha(0x0A);
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 0.0);
        s.animation_time = 0.16;
        // content keeps clear of the large corners
        s.spacing.window_margin = Margin::same(18);
        s.spacing.menu_margin = Margin::same(16); // egui modals take this one
        // egui fades scrolled content into the panel's fill; on see-through glass that fill is a dark veil, so
        // the fade showed as a darker box with hard edges over the messages
        s.spacing.scroll.fade.strength = 0.0;
    });
}

// ---------------------------------------------------------------- widgets

/// `Card` style: glass plate (16 px corners), padding is (left, top, right, bottom) as in XAML.
pub fn card<R>(ui: &mut Ui, pad: (i8, i8, i8, i8), add: impl FnOnce(&mut Ui) -> R) -> R {
    card_fill(ui, pad, p().card_bg, add)
}

pub fn card_fill<R>(ui: &mut Ui, pad: (i8, i8, i8, i8), fill: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let slot = ui.painter().add(egui::Shape::Noop);
    let out = egui::Frame::new()
        .inner_margin(Margin { left: 3 + pad.0, top: pad.1, right: pad.2, bottom: pad.3 })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
            ui.set_width(ui.available_width());
            let min = MIN_H.with(|c| c.get()) - (pad.1 + pad.3) as f32;
            if min > 0.0 {
                ui.set_min_height(min);
            }
            add(ui)
        });
    if ui.is_rect_visible(out.response.rect.expand(40.0)) {
        // rows scrolled out of view get no plate (the margin keeps their shadow while they are just outside)
        ui.painter().set(slot, glass(out.response.rect, R_PANEL, fill));
    }
    out.inner
}

/// Section heading: 13 semibold in the accent colour, margin 0,24,0,8.
pub fn section(ui: &mut Ui, text: &str) {
    section_gap(ui, text);
}

/// Same space as `section`, without the visible heading. Right under the page title (see `page_gap`)
/// it adds nothing, so every page has the same title-to-content distance.
pub fn section_gap(ui: &mut Ui, text: &str) {
    if at_page_top(ui) {
        return;
    }
    ui.add_space(24.0);
    ui.add_visible(false, egui::Label::new(dsb(text, 13.0)));
    ui.add_space(8.0);
}

/// The space under every page title (what a hidden section heading takes), remembered so the page's
/// first `section_gap` does not add it twice.
pub fn page_gap(ui: &mut Ui) {
    ui.add_space(24.0);
    ui.add_visible(false, egui::Label::new(dsb(" ", 13.0)));
    ui.add_space(8.0);
    let y = ui.cursor().top();
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("page-top"), y));
}

/// True while nothing was drawn since `page_gap` this frame.
pub fn at_page_top(ui: &Ui) -> bool {
    let y = ui.cursor().top();
    ui.ctx().data(|d| d.get_temp::<f32>(egui::Id::new("page-top"))).is_some_and(|t| (t - y).abs() < 0.5)
}

/// XAML button: default (ControlBg + 1px border) or accent (filled, bold OnAccent text).
pub fn button(ui: &mut Ui, text: &str, accent: bool, pad: (f32, f32, f32, f32)) -> egui::Response {
    button_tint(ui, text, accent, None, pad)
}

/// Like `button`, with a solid `tint` fill (e.g. DANGER) and white text instead of the accent.
pub fn button_tint(ui: &mut Ui, text: &str, accent: bool, tint: Option<Color32>, pad: (f32, f32, f32, f32)) -> egui::Response {
    let pal = p();
    let accent = accent || tint.is_some();
    let family = if accent { on_light() } else { FontFamily::Proportional };
    let color = if tint.is_some() { Color32::BLACK } else if accent { pal.on_accent } else { TEXT };
    let size = {
        let g = ui.painter().layout_no_wrap(text.to_string(), FontId::new(14.0, family.clone()), color);
        vec2(g.size().x + pad.0 + pad.2, g.size().y + pad.1 + pad.3)
    };
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    // pill that grows 5% under the pointer and shrinks 4% while pressed (text grows with it)
    let ctx = ui.ctx().clone();
    let ease = egui::emath::easing::cubic_out;
    let hov = ctx.animate_bool_with_time_and_easing(resp.id.with("hover"), resp.hovered(), 0.14, ease);
    let down = ctx.animate_bool_with_time_and_easing(resp.id.with("down"), resp.is_pointer_button_down_on(), 0.12, ease);
    let s = 1.0 + 0.05 * hov - 0.09 * down;
    let plate = Rect::from_center_size(rect.center(), rect.size() * s);
    let paint = ui.painter();
    if accent {
        capsule(paint, plate, tint.unwrap_or(pal.accent).lerp_to_gamma(Color32::WHITE, 0.12 * hov), 0x68);
    } else {
        capsule(paint, plate, Color32::from_white_alpha(0x18 + (16.0 * hov) as u8), 0x38 + (16.0 * hov) as u8);
    }
    let galley = paint.layout_no_wrap(text.to_string(), FontId::new(14.0 * s, family), color);
    paint.galley(pos2(plate.left() + pad.0 * s, plate.top() + pad.1 * s), galley, color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub const BTN: (f32, f32, f32, f32) = (15.0, 5.0, 15.0, 6.0);

/// iOS-style switch: 46x26 pill track, 24x20 pill knob. Pressed, the knob swells and turns glassy; a click slides it
/// over, overshoots a few px and wobbles left and right twice before it settles (about 0.6 s). The whole switch grows
/// 5% under the pointer.
pub fn switch(ui: &mut Ui, on: bool) -> egui::Response {
    let pal = p();
    let (slot, resp) = ui.allocate_exact_size(vec2(46.0, 26.0), egui::Sense::click());
    let ctx = ui.ctx().clone();
    let now = ctx.input(|i| i.time);
    let key = resp.id.with("flip");
    if resp.clicked() {
        // direction the knob is about to travel: right when turning on
        ctx.data_mut(|d| d.insert_temp(key, (now, if on { -1.0f32 } else { 1.0 })));
    }
    let (since, dir) = ctx.data(|d| d.get_temp::<(f64, f32)>(key)).unwrap_or((-10.0, 1.0));
    let dt = (now - since) as f32;
    let wobble = if dt < 0.8 {
        ctx.request_repaint();
        dir * 3.5 * (-dt * 6.5).exp() * (dt * 20.0).sin()
    } else {
        0.0
    };
    let t = ctx.animate_bool_with_time_and_easing(resp.id, on, 0.2, egui::emath::easing::cubic_out);
    let hov = ctx.animate_bool_with_time(resp.id.with("hover"), resp.hovered(), 0.12);
    let held = ctx.animate_bool_with_time(resp.id.with("held"), resp.is_pointer_button_down_on(), 0.12);
    let rect = slot.scale_from_center(1.0 + 0.05 * hov);
    let paint = ui.painter();
    let off_bg = Color32::from_white_alpha(0x24 + (10.0 * hov) as u8);
    // the track is a well: shade under its top edge, light on its bottom one
    let rad = rect.height() / 2.0;
    paint.rect_filled(rect, rad, off_bg.lerp_to_gamma(pal.accent, t));
    paint.rect_stroke(rect, rad, Stroke::new(1.0, Color32::from_white_alpha(0x30)), StrokeKind::Inside);
    paint.line_segment([pos2(rect.left() + rad * 0.7, rect.top() + 1.5), pos2(rect.right() - rad * 0.7, rect.top() + 1.5)], Stroke::new(1.0, Color32::from_black_alpha(0x30)));
    // glass knob: stretches sideways while pressed
    let kw = 24.0 + 10.0 * held;
    let kh = 20.0 + 3.0 * held;
    let half = kw / 2.0 + 3.0;
    let cx = egui::lerp((rect.left() + half)..=(rect.right() - half), t) + wobble;
    let knob = Rect::from_center_size(pos2(cx, rect.center().y), vec2(kw, kh));
    let off_knob = Color32::WHITE;
    let fill = off_knob.lerp_to_gamma(pal.on_accent, t);
    paint.add(egui::Shadow { offset: [0, 2], blur: 6, spread: 0, color: Color32::from_black_alpha(70) }.as_shape(knob, CornerRadius::from(kh / 2.0)));
    capsule(paint, knob, fill.gamma_multiply(1.0 - 0.7 * held), (0x50 as f32 + 0x2F as f32 * held) as u8);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// One row of segmented tabs: the first tab of a frame takes a shape slot under all of them, every tab redraws
/// the track and the glass thumb into it.
#[derive(Clone, Copy)]
struct TabRow {
    pass: u64,
    slot: egui::layers::ShapeIdx,
    /// Top left of the first tab; the thumb is kept relative to it, so scrolling does not animate it.
    origin: Pos2,
    right: f32,
    /// Thumb travel: from, to (both relative to `origin`) and when it set off.
    from: Rect,
    to: Rect,
    t0: f64,
}

const TAB_SLIDE: f32 = 0.3;

fn tab_plate(ui: &Ui, rect: Rect, selected: bool) {
    let ctx = ui.ctx();
    let id = ui.id().with("tab_row");
    let pass = ctx.cumulative_pass_nr();
    let now = ctx.input(|i| i.time);
    let old = ctx.data(|d| d.get_temp::<TabRow>(id));
    let mut row = match old {
        Some(mut r) if r.pass == pass => {
            r.right = r.right.max(rect.right());
            r
        }
        _ => {
            let slot = ui.painter().add(egui::Shape::Noop);
            let (from, to, t0) = old.map_or((Rect::NOTHING, Rect::NOTHING, 0.0), |r| (r.from, r.to, r.t0));
            TabRow { pass, slot, origin: rect.min, right: rect.right(), from, to, t0 }
        }
    };
    if selected {
        let rel = rect.translate(-row.origin.to_vec2());
        if row.to == Rect::NOTHING {
            (row.from, row.to) = (rel, rel); // first time on screen: no travel
        } else if (rel.min - row.to.min).length() > 0.5 || (rel.width() - row.to.width()).abs() > 0.5 {
            row.from = thumb_at(&row, now);
            row.to = rel;
            row.t0 = now;
        }
    }
    let mut shapes = Vec::with_capacity(8);
    let track = Rect::from_min_max(row.origin, pos2(row.right, rect.bottom())).expand(3.0);
    shapes.push(egui::Shape::rect_filled(track, track.height() / 2.0, Color32::from_black_alpha(0x38)));
    shapes.push(egui::Shape::rect_stroke(track, track.height() / 2.0, Stroke::new(1.0, Color32::from_white_alpha(0x16)), StrokeKind::Inside));
    if row.to != Rect::NOTHING {
        let thumb = thumb_at(&row, now).translate(row.origin.to_vec2());
        shapes.push(material(thumb, thumb.height() / 2.0, Glass::Raised, Color32::from_white_alpha(0x34)));
        if (now - row.t0) < TAB_SLIDE as f64 {
            ctx.request_repaint();
        }
    }
    ui.painter().set(row.slot, egui::Shape::Vec(shapes));
    ctx.data_mut(|d| d.insert_temp(id, row));
}

/// Where the thumb is now: it eases into its target and never passes it (user choice: no spring).
fn thumb_at(row: &TabRow, now: f64) -> Rect {
    slide(row.from, row.to, ((now - row.t0) as f32) / TAB_SLIDE)
}

fn slide(from: Rect, to: Rect, t: f32) -> Rect {
    let k = egui::emath::easing::cubic_out(t.clamp(0.0, 1.0));
    Rect::from_min_size(from.min + (to.min - from.min) * k, from.size() + (to.size() - from.size()) * k)
}

pub fn spark(ui: &mut Ui, values: &[f32], height: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), egui::Sense::hover());
    if values.len() < 2 {
        return;
    }
    let pts: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| pos2(rect.left() + rect.width() * i as f32 / (values.len() - 1) as f32, rect.bottom() - 2.0 - (rect.height() - 4.0) * (v / 100.0).clamp(0.0, 1.0)))
        .collect();
    ui.painter().add(egui::Shape::line(pts, Stroke::new(1.5, p().accent)));
}

pub fn v2(x: f32, y: f32) -> Vec2 { vec2(x, y) }

/// `ChipTemplate`: 11 semibold text on a tinted square, padding 8,1,8,2, margin 0,6,6,0.
pub fn chip(ui: &mut Ui, text: &str, fg: Color32, bg: Color32) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::new(11.0, semibold()), fg);
    let size = vec2(galley.size().x + 16.0, galley.size().y + 3.0);
    let (rect, _) = ui.allocate_exact_size(size + vec2(6.0, 6.0), egui::Sense::hover());
    let chip = Rect::from_min_size(rect.min + vec2(0.0, 6.0), size);
    ui.painter().rect_filled(chip, chip.height() / 2.0, bg);
    ui.painter().galley(pos2(chip.left() + 8.0, chip.top() + 1.0), galley, fg);
}

/// Chips under a tweak, in the same order and wording as `TweakVM`.
pub fn flag_chips(ui: &mut Ui, flags: mytune_core::catalog::TweakFlags) {
    use mytune_core::catalog::TweakFlags as F;
    let neutral = (text2(), control_bg());
    let mut list: Vec<(&str, (Color32, Color32))> = vec![];
    if flags.has(F::SECURITY_RISK) {
        list.push(("Security risk", (DANGER, danger_soft())));
    }
    if flags.has(F::EXPERIMENTAL) {
        list.push(("Experimental", (WARNING, warning_soft())));
    }
    if flags.has(F::RESTART) {
        list.push(("Restart required", neutral));
    }
    if flags.has(F::SIGN_OUT) {
        list.push(("Sign out to apply", neutral));
    }
    if flags.has(F::RESTARTS_EXPLORER) {
        list.push(("Restarts Explorer", neutral));
    }
    if flags.has(F::NETWORK_BLIP) {
        list.push(("Brief disconnect", neutral));
    }
    if list.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        for (t, (fg, bg)) in list {
            chip(ui, t, fg, bg);
        }
    });
}

/// `SubtleButton`: transparent, 7% white on hover.
pub fn subtle_button(ui: &mut Ui, text: &str, pad: (f32, f32, f32, f32)) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(14.0), TEXT);
    let size = vec2(galley.size().x + pad.0 + pad.2, galley.size().y + pad.1 + pad.3);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, rect.height() / 2.0, Color32::from_white_alpha(18));
    }
    ui.painter().galley(pos2(rect.left() + pad.0, rect.top() + pad.1), galley, TEXT);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// `ProgressBar`: 6 px, #29FFFFFF track, accent fill.
pub fn progress(ui: &mut Ui, fraction: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 3.0, Color32::from_white_alpha(0x29));
    let fill = Rect::from_min_size(rect.min, vec2(rect.width() * fraction.clamp(0.0, 1.0), 6.0));
    ui.painter().rect_filled(fill, 3.0, p().accent);
}

/// `SubtleButton` with a negative left margin (`Margin="-8,..."`): the plate reaches `bleed` px left of the layout
/// position, so the text lines up with the content above.
pub fn subtle_button_bleed(ui: &mut Ui, text: &str, pad: (f32, f32, f32, f32), bleed: f32) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(14.0), TEXT);
    let size = vec2(galley.size().x + pad.0 + pad.2, galley.size().y + pad.1 + pad.3);
    let (slot, _) = ui.allocate_exact_size(size - vec2(bleed, 0.0), egui::Sense::hover());
    let rect = Rect::from_min_size(slot.min - vec2(bleed, 0.0), size);
    let resp = ui.interact(rect, ui.id().with(text), egui::Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, rect.height() / 2.0, Color32::from_white_alpha(18));
    }
    ui.painter().galley(pos2(rect.left() + pad.0, rect.top() + pad.1), galley, TEXT);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// One tab of a segmented row (call it for each tab inside one `ui.horizontal`): the row has a dark track and a
/// glass thumb that slides to the selected tab. Padding 15,6,15,7.
pub fn tab(ui: &mut Ui, text: &str, selected: bool) -> egui::Response {
    let font = if selected { FontId::new(14.0, semibold()) } else { FontId::proportional(14.0) };
    let fg = if selected { TEXT } else { text2() };
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, fg);
    let size = vec2(galley.size().x + 30.0, galley.size().y + 13.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    tab_plate(ui, rect, selected);
    let paint = ui.painter();
    if !selected && resp.hovered() {
        paint.rect_filled(rect, rect.height() / 2.0, Color32::from_white_alpha(0x14));
    }
    let col = if !selected && resp.hovered() { TEXT } else { fg };
    paint.galley(pos2(rect.left() + 15.0, rect.top() + 6.0), galley, col);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A neutral-to-warm dark surface at brightness `v`, in the hue of the accent and as saturated as the accent
/// allows (a grey accent gives greys), so the chat's layers match the MyTune pages around it.
pub fn tone(v: f32) -> Color32 {
    let a = p().accent;
    let (h, s, _) = to_hsv([a.r() as f32, a.g() as f32, a.b() as f32]);
    from_hsv(h, 0.33 * (s / 0.3).min(1.0), v)
}

/// A small pick chip (one of several choices, a filter, a category): a glass capsule, filled with the accent
/// when it is the chosen one. 28 px high unless `min_size` asks for more.
pub struct Choice {
    text: String,
    on: bool,
    min: Vec2,
}

impl Choice {
    pub fn new(text: impl Into<String>, on: bool) -> Choice {
        Choice { text: text.into(), on, min: vec2(0.0, 28.0) }
    }

    pub fn min_size(mut self, min: Vec2) -> Choice {
        self.min = min;
        self
    }
}

impl egui::Widget for Choice {
    fn ui(self, ui: &mut Ui) -> egui::Response {
        let pal = p();
        let fg = if self.on { pal.on_accent } else { text2() };
        let family = if self.on { on_light() } else { semibold() };
        let galley = ui.painter().layout_no_wrap(self.text, FontId::new(13.0, family), fg);
        let size = vec2((galley.size().x + 26.0).max(self.min.x), (galley.size().y + 10.0).max(self.min.y));
        let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
        let hov = ui.ctx().animate_bool_with_time_and_easing(resp.id.with("hover"), resp.hovered(), 0.14, egui::emath::easing::cubic_out);
        if self.on {
            capsule(ui.painter(), rect, pal.accent.lerp_to_gamma(Color32::WHITE, 0.12 * hov), 0x68);
        } else {
            capsule(ui.painter(), rect, Color32::from_white_alpha(0x18 + (16.0 * hov) as u8), 0x30);
        }
        let at = rect.center() - galley.size() / 2.0;
        ui.painter().galley(at, galley, fg);
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `fg` (maybe translucent) drawn over opaque `bg`.
    fn over(bg: Color32, fg: Color32) -> Color32 {
        let a = fg.a() as f32 / 255.0;
        // `fg` is premultiplied
        Color32::from_rgb((fg.r() as f32 + bg.r() as f32 * (1.0 - a)) as u8, (fg.g() as f32 + bg.g() as f32 * (1.0 - a)) as u8, (fg.b() as f32 + bg.b() as f32 * (1.0 - a)) as u8)
    }

    fn ratio(a: Color32, b: Color32) -> f32 {
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// The tab thumb never passes its target, however far it slides.
    #[test]
    fn tab_thumb_stays_near_target() {
        let from = Rect::from_min_size(pos2(0.0, 0.0), vec2(80.0, 30.0));
        let to = Rect::from_min_size(pos2(250.0, 0.0), vec2(150.0, 30.0));
        for i in 0..=100 {
            let r = slide(from, to, i as f32 / 100.0);
            assert!(r.left() <= to.left() + 0.01 && r.right() <= to.right() + 0.01, "{r:?}");
        }
        assert_eq!(slide(from, to, 1.0), to);
    }

    /// The accent is one global: tests that set it run one at a time.
    static ACCENT: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// The backdrop picture follows the unrounded colour: averaged over a block, rounding leaves no step.
    #[test]
    fn backdrop_has_no_bands() {
        let _one = ACCENT.lock().unwrap_or_else(|e| e.into_inner());
        set_accent_value("#F2F2F2");
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1184.0, 784.0));
        let (ow, oh) = (592, 392);
        let (img, f) = (backdrop_image(rect, ow, oh), field(rect));
        for by in 0..oh / 16 {
            for bx in 0..ow / 16 {
                let (mut got, mut want) = ([0.0f32; 3], [0.0f32; 3]);
                for y in by * 16..by * 16 + 16 {
                    for x in bx * 16..bx * 16 + 16 {
                        let (c, w) = (img.pixels[y * ow + x], f(pos2(x as f32 * 2.0 + 1.0, y as f32 * 2.0 + 1.0)));
                        for (k, v) in [c.r(), c.g(), c.b()].into_iter().enumerate() {
                            got[k] += v as f32 / 256.0;
                            want[k] += w[k] / 256.0;
                        }
                    }
                }
                for k in 0..3 {
                    assert!((got[k] - want[k]).abs() < 0.2, "block {bx},{by} channel {k}: {} vs {}", got[k], want[k]);
                }
            }
        }
    }

    /// Text keeps 4.5:1 on every glass level over the brightest backdrop spot, for each accent.
    #[test]
    fn text_contrast_on_glass() {
        let _one = ACCENT.lock().unwrap_or_else(|e| e.into_inner());
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1180.0, 780.0));
        for hex in ["#FF5A0A", "#F2F2F2", "#FF3B30", "#FFD400", "#34C759", "#00C7BE", "#0A84FF", "#BF5AF2", "#FF2D92"] {
            set_accent_value(hex);
            let mut worst = Color32::BLACK;
            for y in (0..=780).step_by(20) {
                for x in (0..=1180).step_by(20) {
                    let c = backdrop_at(rect, pos2(x as f32, y as f32));
                    if lum(c) > lum(worst) {
                        worst = c;
                    }
                }
            }
            // a panel adds its fill and, at its top, the light inside; a control on it adds its own fill
            let glow = |l: Glass| Color32::from_white_alpha(l.light().2);
            let base = over(over(worst, Glass::Base.fill()), glow(Glass::Base));
            let raised = over(over(base, Glass::Raised.fill()), glow(Glass::Raised));
            let float = over(over(base, Glass::Float.fill()), glow(Glass::Float));
            for (name, bg, texts) in [("base", base, &[TEXT, text2(), text3()][..]), ("raised", raised, &[TEXT][..]), ("float", float, &[TEXT, text2(), text3()][..])] {
                for &fg in texts {
                    let r = ratio(over(bg, fg), bg);
                    assert!(r >= 4.5, "{hex} {name}: {r:.2}:1 for text alpha {}", fg.a());
                }
            }
        }
    }
}
