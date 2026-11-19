//! Look of MyTune: palette derived from the accent exactly like `Services/Accent.cs`. Glass look: translucent
//! panels over soft colour blobs (`backdrop`) that take the accent's hue. One system for the whole app: the type
//! is Inter, corners come from one scale of radii and are all the same smooth curve, every plate has a one-pixel
//! rim lit from above, and everything that moves rides a spring (`spring`).
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

/// Between regular and semibold: button and tab labels, titles of list rows.
pub fn medium() -> FontFamily { FontFamily::Name("medium".into()) }
pub fn semibold() -> FontFamily { FontFamily::Name("semibold".into()) }
pub fn bold() -> FontFamily { FontFamily::Name("bold".into()) }
pub fn display_semibold() -> FontFamily { FontFamily::Name("dsemibold".into()) }
pub fn display_bold() -> FontFamily { FontFamily::Name("dbold".into()) }
/// Dark text on the accent fill: egui boosts glyph coverage for light-on-dark text, so a lighter weight keeps it looking Bold.
pub fn on_light() -> FontFamily { FontFamily::Name("onlight".into()) }
pub fn icons() -> FontFamily { FontFamily::Name("icons".into()) }

/// Everything is drawn 15% larger than its points (user's choice): more pixels per letter and icon on a 100%
/// scale screen, where egui's text has no ClearType. Ctrl+plus / Ctrl+minus still change it for the run.
pub const UI_ZOOM: f32 = 1.15;

/// Inter (variable, SIL Open Font License, `assets/fonts`): the app's typeface, in the exe.
const INTER: &[u8] = include_bytes!("../assets/fonts/InterVariable.ttf");

/// Inter first, in every weight the app uses (its text and display optical sizes). Behind it "Segoe UI Variable"
/// from the system font `SegUIVar.ttf` (or the static Segoe UI files) for the scripts Inter lacks, then egui's
/// own font.
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
        let variants = [("regular", 400.0, 10.5), ("medium", 500.0, 10.5), ("semibold", 600.0, 10.5), ("bold", 700.0, 10.5), ("dsemibold", 600.0, 36.0), ("dbold", 700.0, 36.0), ("onlight", 560.0, 10.5)];
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
        for (name, file) in [("regular", "segoeui.ttf"), ("medium", "seguisb.ttf"), ("semibold", "seguisb.ttf"), ("bold", "segoeuib.ttf"), ("dsemibold", "seguisb.ttf"), ("dbold", "segoeuib.ttf"), ("onlight", "seguisb.ttf")] {
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
    // Inter goes in front of each list. Weight and optical size per family; `scale` makes the same point size
    // come out 6% smaller than Segoe did (Inter's letters are larger for their size), which is also what makes
    // the whole app a little finer.
    for (name, wght, opsz) in [("regular", 400.0, 14.0), ("medium", 510.0, 14.0), ("semibold", 600.0, 14.0), ("bold", 700.0, 14.0), ("dsemibold", 620.0, 32.0), ("dbold", 700.0, 32.0), ("onlight", 590.0, 14.0)] {
        let key = format!("inter-{name}");
        let tweak = egui::FontTweak { scale: 0.94, coords: VariationCoords::new([(b"wght", wght), (b"opsz", opsz)]), ..Default::default() };
        add(&mut fonts, &key, egui::FontData::from_static(INTER).tweak(tweak));
        let family = if name == "regular" { FontFamily::Proportional } else { FontFamily::Name(name.into()) };
        fonts.families.entry(family).or_default().insert(0, key);
    }
    let mut list = base;
    if let Some(b) = read("SegoeIcons.ttf") {
        // Segoe Fluent Icons are drawn for 16, 20 and 24 pixels: at the app's scale a 16-point icon came out
        // 18.4 pixels, between two of its sizes, and looked soft. Scaled so that 16 points is 20 pixels.
        let tweak = egui::FontTweak { scale: 20.0 / (16.0 * UI_ZOOM), ..Default::default() };
        add(&mut fonts, "segoe-icons", egui::FontData::from_blob(b, 0).tweak(tweak));
        list.insert(0, "segoe-icons".into());
    }
    fonts.families.insert(FontFamily::Name("icons".into()), list);
    // Chat names and messages hold symbols Segoe UI lacks (e.g. U+271E): last-resort fallback, ~1 MB.
    if let Some(b) = read("seguisym.ttf") {
        add(&mut fonts, "segoe-symbols", egui::FontData::from_blob(b, 0));
        for fam in [FontFamily::Proportional, FontFamily::Name("medium".into()), FontFamily::Name("semibold".into()), FontFamily::Name("bold".into())] {
            if let Some(l) = fonts.families.get_mut(&fam) {
                l.push("segoe-symbols".into());
            }
        }
    }
    // Emoji in reactions, names and messages: Segoe UI Emoji's outlines (egui draws them one colour). ~12 MB.
    if let Some(b) = read("seguiemj.ttf") {
        add(&mut fonts, "segoe-emoji", egui::FontData::from_blob(b, 0));
        for fam in [FontFamily::Proportional, FontFamily::Name("medium".into()), FontFamily::Name("semibold".into()), FontFamily::Name("bold".into())] {
            if let Some(l) = fonts.families.get_mut(&fam) {
                l.push("segoe-emoji".into());
            }
        }
    }
    ctx.set_fonts(fonts);
}

pub fn rt(s: impl Into<String>, size: f32) -> RichText { RichText::new(s).size(size) }
pub fn md(s: impl Into<String>, size: f32) -> RichText { RichText::new(s).size(size).family(medium()) }
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

// ---------------------------------------------------------------- motion

/// A value that follows `target` like a damped spring, in Apple's two numbers: `response` (seconds, how quick)
/// and `damping` (1 = settles without passing the target, less = swings past it). It sets off from where it is
/// and keeps its speed when the target changes, so a motion can be turned round at any moment without a jump.
/// The first call for an `id` starts at the target (nothing moves when a control first shows).
pub fn spring(ctx: &egui::Context, id: egui::Id, target: f32, response: f32, damping: f32) -> f32 {
    let now = ctx.input(|i| i.time);
    let (x, v, t) = ctx.data(|d| d.get_temp::<(f32, f32, f64)>(id)).unwrap_or((target, 0.0, now));
    let (x, v) = spring_step(x, v, target, ((now - t) as f32).clamp(0.0, 0.05), response, damping);
    let rest = (x - target).abs() < 0.0005 && v.abs() < 0.01;
    if !rest {
        ctx.request_repaint();
    }
    ctx.data_mut(|d| d.insert_temp(id, if rest { (target, 0.0, now) } else { (x, v, now) }));
    if rest { target } else { x }
}

/// Place and speed of a spring `dt` seconds on.
fn spring_step(mut x: f32, mut v: f32, target: f32, dt: f32, response: f32, damping: f32) -> (f32, f32) {
    let w = std::f32::consts::TAU / response.max(0.01);
    const STEPS: usize = 8;
    let h = dt / STEPS as f32;
    for _ in 0..STEPS {
        v += (-w * w * (x - target) - 2.0 * damping * w * v) * h;
        x += v * h;
    }
    (x, v)
}

/// Puts a spring at `x`, at rest (a control that is being dragged holds its spring where the pointer is).
pub fn spring_set(ctx: &egui::Context, id: egui::Id, x: f32) {
    let now = ctx.input(|i| i.time);
    ctx.data_mut(|d| d.insert_temp(id, (x, 0.0f32, now)));
}

/// Where a spring is now, without moving it on.
pub fn spring_now(ctx: &egui::Context, id: egui::Id) -> Option<f32> {
    ctx.data(|d| d.get_temp::<(f32, f32, f64)>(id)).map(|s| s.0)
}

/// 0..1 for a state that is off or on (hovered, pressed, open): a spring without overshoot.
pub fn anim(ctx: &egui::Context, id: egui::Id, on: bool, response: f32) -> f32 {
    spring(ctx, id, if on { 1.0 } else { 0.0 }, response, 1.0).clamp(0.0, 1.0)
}

/// Response times of the springs: pointer over a control, pointer down on it, a thing that travels (tab thumb,
/// sidebar).
pub const HOVER: f32 = 0.16;
pub const PRESS: f32 = 0.11;
pub const TRAVEL: f32 = 0.34;

// ---------------------------------------------------------------- glass material

/// The three glass levels, lowest first: a panel on the page, a control on a panel, and what floats over both
/// (menus, popovers, dialogs). Each is a translucent fill with a one-pixel rim that is brightest along its top
/// (light falls from above, the same on every plate), a little light inside at the top and a soft shadow; a
/// higher level is lighter and its shadow reaches further.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Glass {
    Base,
    Raised,
    Float,
}

/// Corner radii, one scale for the whole app: page panels (sidebar, chat), cards and grouped lists, dialogs,
/// menus and popovers, controls (buttons, fields, tabs, rows). Every corner is the smooth curve of the patched
/// `epaint` (`rounded_rectangle` in `vendor-src`). Knobs, thumbs and avatars stay round.
pub const R_PANEL: f32 = 18.0;
pub const R_CARD: f32 = 14.0;
pub const R_FLOAT: f32 = 16.0;
pub const R_MENU: f32 = 12.0;
pub const R_CTL: f32 = 9.0;

/// Radius of a shape that sits `gap` px inside one of radius `outer`, so both curves share a centre.
pub fn nested(outer: f32, gap: f32) -> f32 {
    (outer - gap).max(4.0)
}

impl Glass {
    pub fn fill(self) -> Color32 {
        match self {
            Glass::Base => Color32::from_white_alpha(0x0F),
            Glass::Raised => Color32::from_white_alpha(0x1A),
            // egui cannot blur what is behind, so what floats is solid: at 85% the text under a menu showed
            // through its rows and neither could be read
            Glass::Float => tone(0.105),
        }
    }

    pub fn shadow(self) -> egui::Shadow {
        let (y, blur, a) = match self {
            Glass::Base => (6, 20, 60),
            Glass::Raised => (2, 6, 64),
            Glass::Float => (16, 48, 140),
        };
        egui::Shadow { offset: [0, y], blur, spread: 0, color: Color32::from_black_alpha(a) }
    }

    /// Opacity of the rim at the sides and bottom, of the rim along the top and of the light inside at the top.
    fn light(self) -> (u8, u8, u8) {
        match self {
            Glass::Base => (0x1C, 0x40, 0x0C),
            Glass::Raised => (0x24, 0x4C, 0x18),
            Glass::Float => (0x26, 0x48, 0x0C),
        }
    }
}

/// A glass plate of `level`: shadow, `fill` and the light on it (`sheen`).
pub fn material(rect: Rect, radius: f32, level: Glass, fill: Color32) -> egui::Shape {
    let r = radius.min(rect.height() / 2.0).min(rect.width() / 2.0);
    let mut shapes = Vec::with_capacity(4);
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
    let mut shapes = Vec::with_capacity(2);
    sheen_into(&mut shapes, rect, radius.min(rect.height() / 2.0).min(rect.width() / 2.0), level.light(), true);
    p.add(egui::Shape::Vec(shapes));
}

/// One physical pixel in points (the scale `backdrop` saw this frame).
pub fn px() -> f32 {
    GLOW.with(|g| 1.0 / g.borrow().0.max(0.5))
}

/// The outline of a rounded rectangle exactly as the tessellator draws it (smooth corners), clockwise.
fn outline(rect: Rect, r: f32) -> Vec<Pos2> {
    let mut pts = Vec::new();
    egui::epaint::tessellator::path::rounded_rectangle(&mut pts, egui::epaint::RoundedRect::new(rect, egui::epaint::CornerRadiusF32::same(r.max(0.0))));
    pts
}

/// The light on a plate: a soft glow inside that is strongest at the top, and a rim one pixel wide that is `hi`
/// strong along the top and falls to `rim` by the middle of the sides. Both follow the plate's own outline.
fn sheen_into(out: &mut Vec<egui::Shape>, rect: Rect, r: f32, (rim, hi, glow): (u8, u8, u8), over: bool) {
    // a plate thinner than its rim (window being minimised, a panel closing): nothing to light, and the maths
    // below once panicked on such a plate and closed the app
    let (ppp, tex) = GLOW.with(|g| (g.borrow().0.max(0.5), g.borrow().1.iter().find(|(s, _)| *s == glow).map(|(_, t)| *t)));
    let px = 1.0 / ppp;
    if !(rect.width() > 4.0 * px && rect.height() > 4.0 * px) {
        return;
    }
    // on whole pixels, like the fill the tessellator draws under it
    let snap = |v: f32| (v * ppp).round() / ppp;
    let rect = Rect::from_min_max(pos2(snap(rect.left()), snap(rect.top())), pos2(snap(rect.right()), snap(rect.bottom())));
    let centre = rect.center();
    // `q` moved `by` towards the inside, square to the outline at point `i` of `pts`
    let inward = |pts: &[Pos2], i: usize, by: f32| {
        let n = pts.len();
        let d = pts[(i + 1) % n] - pts[(i + n - 1) % n];
        if d.length() < 0.001 {
            return pts[i];
        }
        let nrm = d.normalized().rot90();
        pts[i] + if nrm.dot(centre - pts[i]) >= 0.0 { nrm } else { -nrm } * by
    };
    let inner = rect.shrink(px);
    let ring = outline(inner, r - px);
    let n = ring.len();
    if glow > 0 && n >= 3 {
        let shade = |y: f32| Color32::from_white_alpha((glow as f32 * (1.0 - (y - inner.top()) / inner.height())).clamp(0.0, 255.0) as u8);
        let mut m = egui::Mesh::default();
        m.colored_vertex(centre, shade(centre.y));
        for q in &ring {
            m.colored_vertex(*q, Color32::TRANSPARENT); // the clear outer edge
        }
        for i in 0..n {
            let q = inward(&ring, i, 1.5 * px); // full strength from here in
            m.colored_vertex(q, shade(q.y));
        }
        let n = n as u32;
        for i in 0..n {
            let j = (i + 1) % n;
            m.add_triangle(0, 1 + n + i, 1 + n + j); // the fan, on the inner ring
            m.add_triangle(1 + i, 1 + j, 1 + n + i); // the feather between the rings
            m.add_triangle(1 + j, 1 + n + j, 1 + n + i);
        }
        // with the dithered picture at hand, it carries the fade: the vertices only place it and feather its edge
        if let Some(tex) = tex {
            for (i, v) in m.vertices.iter_mut().enumerate() {
                v.uv = pos2(v.pos.x * ppp / GLOW_W as f32 + if over { 0.5 } else { 0.0 }, (v.pos.y - inner.top()) / inner.height());
                if !(1..=n as usize).contains(&i) {
                    v.color = Color32::WHITE;
                }
            }
            m.texture_id = tex;
        }
        out.push(egui::Shape::mesh(m));
    }
    // the rim sits half a pixel inside the plate's edge, so its middle lies on the outermost pixel row
    let edge = rect.shrink(px * 0.5);
    let pts: Vec<(Pos2, f32)> = outline(edge, r - px * 0.5)
        .into_iter()
        .map(|q| {
            // 1 along the top edge, 0 from half way down
            let k = 1.0 - ((q.y - edge.top()) / edge.height() * 2.0).clamp(0.0, 1.0);
            (q, rim as f32 + (hi as f32 - rim as f32) * k * k * (3.0 - 2.0 * k))
        })
        .collect();
    ribbon(out, &pts, px);
}

/// A white closed line through `pts` (place, opacity 0..=255) whose strength changes along it: the colour is on
/// its middle and gone `half` to either side, so it is one pixel of light when `half` is a pixel, and curves
/// and slopes come out smooth (a plain 1 px stroke shows its pixels on a bright rim).
fn ribbon(out: &mut Vec<egui::Shape>, pts: &[(Pos2, f32)], half: f32) {
    let n = pts.len();
    if n < 3 {
        return;
    }
    let mut m = egui::Mesh::default();
    for i in 0..n {
        let d = pts[(i + 1) % n].0 - pts[(i + n - 1) % n].0;
        let nrm = if d.length() < 0.001 { Vec2::ZERO } else { d.normalized().rot90() * half };
        let (q, a) = pts[i];
        m.colored_vertex(q - nrm, Color32::TRANSPARENT);
        m.colored_vertex(q, Color32::from_white_alpha(a.clamp(0.0, 255.0) as u8));
        m.colored_vertex(q + nrm, Color32::TRANSPARENT);
    }
    let n = n as u32;
    for i in 0..n {
        let (v, w) = (i * 3, (i + 1) % n * 3);
        for (a, b, c) in [(v, v + 1, w), (v + 1, w + 1, w), (v + 1, v + 2, w + 1), (v + 2, w + 2, w + 1)] {
            m.add_triangle(a, b, c);
        }
    }
    out.push(egui::Shape::mesh(m));
}

/// A raised glass plate for a control (button, field, chip) with corner radius `r`: fill and the same light as
/// any plate. `rim` is how bright its top edge is.
pub fn plate(p: &egui::Painter, rect: Rect, r: f32, fill: Color32, rim: u8) {
    let r = r.min(rect.height() / 2.0).min(rect.width() / 2.0);
    let mut shapes = Vec::with_capacity(3);
    shapes.push(egui::Shape::rect_filled(rect, r, fill));
    sheen_into(&mut shapes, rect, r, (rim / 2, rim, 0x18), false);
    p.add(egui::Shape::Vec(shapes));
}

/// A fully round glass plate (knob, thumb, round button).
pub fn capsule(p: &egui::Painter, rect: Rect, fill: Color32, rim: u8) {
    plate(p, rect, rect.height() / 2.0, fill, rim);
}

/// A floating glass panel (menu, popover, picker) around `add`: the top level, `margin` px of padding. Frosted:
/// what is behind shows through blurred (the lens), under a film of the floating level's colour. Where the
/// lens does not run the panel is solid, since unblurred text behind a menu made both unreadable.
pub fn float<R>(ui: &mut Ui, radius: f32, margin: i8, add: impl FnOnce(&mut Ui) -> R) -> R {
    let slot = ui.painter().add(egui::Shape::Noop);
    let out = egui::Frame::new().inner_margin(Margin::same(margin)).show(ui, add);
    let rect = out.response.rect;
    let r = radius.min(rect.height() / 2.0).min(rect.width() / 2.0);
    let fill = if crate::lens::works() { Glass::Float.fill().gamma_multiply(0.76) } else { Glass::Float.fill() };
    let mut shapes = Vec::with_capacity(5);
    shapes.push(Glass::Float.shadow().as_shape(rect, CornerRadius::from(r)).into());
    shapes.push(crate::lens::shape(rect, crate::lens::Lens { radius: r, rim: 12.0, bend: 6.0, zoom: 1.0, blur: 14.0, split: 0.0 }));
    shapes.push(egui::Shape::rect_filled(rect, r, fill));
    sheen_into(&mut shapes, rect, r, Glass::Float.light(), false);
    ui.painter().set(slot, egui::Shape::Vec(shapes));
    out.inner
}

pub fn apply_visuals(ctx: &egui::Context) {
    let pal = p();
    let mut v = egui::Visuals::dark();
    let zero = CornerRadius::same(R_CTL as u8);
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
    v.window_stroke = Stroke::new(1.0, Color32::from_white_alpha(0x22));
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

thread_local! {
    /// Inside `group`: whether the next card is the group's first row.
    static GROUP: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Rows that belong together: every `card` drawn straight inside becomes a row of one glass plate, with a
/// hairline between rows (from the text's left edge to the plate's right edge) instead of a plate of its own.
pub fn group<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    let slot = ui.painter().add(egui::Shape::Noop);
    let outer = GROUP.replace(Some(true));
    let out = egui::Frame::new().show(ui, |ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
        ui.set_width(ui.available_width());
        add(ui)
    });
    GROUP.set(outer);
    let rect = out.response.rect;
    if rect.height() > 1.0 && ui.is_rect_visible(rect.expand(40.0)) {
        ui.painter().set(slot, glass(rect, R_CARD, p().card_bg));
    }
    out.inner
}

pub fn card_fill<R>(ui: &mut Ui, pad: (i8, i8, i8, i8), fill: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    if let Some(first) = GROUP.get() {
        if !first {
            let y = ui.cursor().top();
            ui.painter().line_segment([pos2(ui.max_rect().left() + 3.0 + pad.0 as f32, y), pos2(ui.max_rect().right(), y)], Stroke::new(px(), Color32::from_white_alpha(0x1C)));
        }
        GROUP.set(None); // what the row holds is not the group's: a card in it is a card again
        let out = egui::Frame::new().inner_margin(Margin { left: 3 + pad.0, top: pad.1, right: pad.2, bottom: pad.3 }).show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
            ui.set_width(ui.available_width());
            add(ui)
        });
        GROUP.set(Some(false));
        return out.inner;
    }
    let slot = ui.painter().add(egui::Shape::Noop);
    let out = egui::Frame::new()
        .inner_margin(Margin { left: 3 + pad.0, top: pad.1, right: pad.2, bottom: pad.3 })
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing = vec2(8.0, 0.0);
            ui.set_width(ui.available_width());
            add(ui)
        });
    if ui.is_rect_visible(out.response.rect.expand(40.0)) {
        // rows scrolled out of view get no plate (the margin keeps their shadow while they are just outside)
        ui.painter().set(slot, glass(out.response.rect, R_CARD, fill));
    }
    out.inner
}

/// Section heading: 13 semibold in the accent colour, margin 0,24,0,8.
pub fn section(ui: &mut Ui, text: &str) {
    section_gap(ui, text);
}

/// A small visible label over a plate or a group of rows (Home's parts). `first`: the first thing in its column,
/// so no space above it.
pub fn heading(ui: &mut Ui, text: &str, first: bool) {
    if !first {
        ui.add_space(22.0);
    }
    ui.horizontal(|ui| {
        ui.add_space(4.0);
        ui.label(md(text, 12.5).color(text3()));
    });
    ui.add_space(7.0);
}

/// Same space as `section`, without the visible heading. Right under the page title (see `page_gap`)
/// it adds nothing, so every page has the same title-to-content distance.
pub fn section_gap(ui: &mut Ui, _heading: &str) {
    if at_page_top(ui) {
        return;
    }
    ui.add_space(28.0);
}

/// The space under every page title, remembered so the page's first `section_gap` does not add to it.
pub fn page_gap(ui: &mut Ui) {
    ui.add_space(20.0);
    let y = ui.cursor().top();
    ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("page-top"), y));
}

/// True while nothing was drawn since `page_gap` this frame.
pub fn at_page_top(ui: &Ui) -> bool {
    let y = ui.cursor().top();
    ui.ctx().data(|d| d.get_temp::<f32>(egui::Id::new("page-top"))).is_some_and(|t| (t - y).abs() < 0.5)
}

/// How far a held control gives way to a pull of `raw` px: follows the pointer at first, never past `PULL_MAX`.
const PULL_MAX: f32 = 14.0;

fn rubber(raw: Vec2) -> Vec2 {
    raw * (PULL_MAX / (raw.length() + PULL_MAX))
}

/// Apple-style pull for a control allocated with `Sense::click_and_drag()`: while the pointer is held on it, returns
/// how far the control is pulled (rubber-banded); let go, it springs back. Letting go over the control after a pull
/// or a long hold counts as a click (`resp.clicked()` turns true), letting go beside it cancels.
pub fn pull(ui: &Ui, resp: &mut egui::Response) -> Vec2 {
    let ctx = ui.ctx();
    let key = resp.id.with("pull");
    let now = ctx.input(|i| i.time);
    if resp.drag_stopped() {
        let over = ctx.input(|i| i.pointer.button_released(egui::PointerButton::Primary) && i.pointer.interact_pos().is_some_and(|p| resp.rect.expand(8.0).contains(p)));
        if over {
            resp.flags |= egui::response::Flags::FAKE_PRIMARY_CLICKED;
        }
    }
    if resp.is_pointer_button_down_on() {
        let raw = ctx.input(|i| match (i.pointer.press_origin(), i.pointer.latest_pos()) {
            (Some(a), Some(b)) => b - a,
            _ => Vec2::ZERO,
        });
        let off = rubber(raw);
        ctx.data_mut(|d| d.insert_temp(key, (off, now)));
        return off;
    }
    let Some((off, t)) = ctx.data(|d| d.get_temp::<(Vec2, f64)>(key)) else { return Vec2::ZERO };
    let dt = (now - t) as f32;
    if dt > 0.5 || off == Vec2::ZERO {
        return Vec2::ZERO;
    }
    ctx.request_repaint();
    off * (-dt * 9.0).exp() * (dt * 24.0).cos()
}

/// `rect` pulled by `off`: the edge on the pointer's side follows it, the other edge stays, so the plate stretches.
pub fn pulled(rect: Rect, off: Vec2) -> Rect {
    Rect::from_center_size(rect.center() + off / 2.0, rect.size() + off.abs())
}

/// Button: a glass plate with a light rim, or (accent) filled with the accent colour and dark-on-light text.
pub fn button(ui: &mut Ui, text: &str, accent: bool, pad: (f32, f32, f32, f32)) -> egui::Response {
    button_tint(ui, text, accent, None, pad)
}

/// Like `button`, with a solid `tint` fill (e.g. DANGER) and black text instead of the accent.
pub fn button_tint(ui: &mut Ui, text: &str, accent: bool, tint: Option<Color32>, pad: (f32, f32, f32, f32)) -> egui::Response {
    let pal = p();
    let accent = accent || tint.is_some();
    let family = if accent { on_light() } else { medium() };
    let color = if tint.is_some() { Color32::BLACK } else if accent { pal.on_accent } else { TEXT };
    let size = {
        let g = ui.painter().layout_no_wrap(text.to_string(), FontId::new(14.0, family.clone()), color);
        vec2(g.size().x + pad.0 + pad.2, g.size().y + pad.1 + pad.3)
    };
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let off = pull(ui, &mut resp);
    // answers on the press, not the release: it lightens under the pointer and gives way 3% while held
    let ctx = ui.ctx().clone();
    let hov = anim(&ctx, resp.id.with("hover"), resp.hovered(), HOVER);
    let down = anim(&ctx, resp.id.with("down"), resp.is_pointer_button_down_on(), PRESS);
    let s = 1.0 - 0.03 * down;
    let plate_rect = pulled(Rect::from_center_size(rect.center(), rect.size() * s), off);
    let paint = ui.painter();
    if accent {
        let fill = tint.unwrap_or(pal.accent).lerp_to_gamma(Color32::WHITE, 0.10 * hov).lerp_to_gamma(Color32::BLACK, 0.10 * down);
        paint.add(egui::Shadow { offset: [0, 1], blur: 4, spread: 0, color: Color32::from_black_alpha(70) }.as_shape(plate_rect, CornerRadius::from(R_CTL)));
        plate(paint, plate_rect, R_CTL, fill, 0x70);
    } else {
        plate(paint, plate_rect, R_CTL, Color32::from_white_alpha((0x1A as f32 + 14.0 * hov - 8.0 * down) as u8), 0x38 + (16.0 * hov) as u8);
    }
    let galley = paint.layout_no_wrap(text.to_string(), FontId::new(14.0 * s, family), color);
    paint.galley(plate_rect.center() - rect.size() * s / 2.0 + vec2(pad.0, pad.1) * s, galley, color);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub const BTN: (f32, f32, f32, f32) = (14.0, 7.0, 14.0, 7.0);

/// Switch: a 40x24 track and a round knob. The knob rides a spring: a click sends it over, it runs a little
/// past and settles; held, it stretches towards the middle; pulled, it follows the pointer and takes the nearer
/// side when let go (that is the click the caller sees).
pub fn switch(ui: &mut Ui, on: bool) -> egui::Response {
    let pal = p();
    let (rect, mut resp) = ui.allocate_exact_size(vec2(40.0, 24.0), egui::Sense::click_and_drag());
    let ctx = ui.ctx().clone();
    let pos = resp.id.with("pos");
    const KNOB: f32 = 20.0;
    let pulled_to = ctx.input(|i| i.pointer.latest_pos()).filter(|_| resp.dragged() || resp.drag_stopped()).map(|p| ((p.x - rect.left() - rect.height() / 2.0) / (rect.width() - rect.height())).clamp(0.0, 1.0));
    let t = match pulled_to {
        Some(v) => {
            if resp.drag_stopped() && (v > 0.5) != on {
                resp.flags |= egui::response::Flags::FAKE_PRIMARY_CLICKED;
            }
            spring_set(&ctx, pos, v);
            v
        }
        // runs up to 2 px past its end and comes back
        None => spring(&ctx, pos, if on { 1.0 } else { 0.0 }, 0.3, 0.55).clamp(-0.12, 1.12),
    };
    let k = t.clamp(0.0, 1.0);
    let hov = anim(&ctx, resp.id.with("hover"), resp.hovered(), HOVER);
    let held = anim(&ctx, resp.id.with("held"), resp.is_pointer_button_down_on(), PRESS);
    let paint = ui.painter();
    let rad = rect.height() / 2.0;
    let off_bg = Color32::from_white_alpha(0x26 + (10.0 * hov) as u8);
    paint.rect_filled(rect, rad, off_bg.lerp_to_gamma(pal.accent, k));
    paint.rect_stroke(rect, rad, Stroke::new(px(), Color32::from_white_alpha(0x20)), StrokeKind::Inside);
    // the knob keeps 2 px from the track's edge; held, it grows 6 px towards the other side
    let stretch = 6.0 * held;
    let left = rect.left() + 2.0 + (rect.width() - 4.0 - KNOB) * t - stretch * k;
    let knob = Rect::from_min_size(pos2(left, rect.top() + 2.0), vec2(KNOB + stretch, KNOB));
    let fill = Color32::WHITE.lerp_to_gamma(pal.on_accent, k);
    paint.add(egui::Shadow { offset: [0, 1], blur: 4, spread: 0, color: Color32::from_black_alpha(90) }.as_shape(knob, CornerRadius::from(KNOB / 2.0)));
    // held, the knob turns to clear glass: the track under it shows through, magnified and bent at the rim
    if held > 0.01 {
        paint.add(crate::lens::shape(knob, crate::lens::Lens { radius: KNOB / 2.0, rim: KNOB * 0.45, bend: 5.0 * held, zoom: 1.0 + 0.2 * held, blur: 0.0, split: 0.3 }));
    }
    capsule(paint, knob, fill.gamma_multiply(1.0 - 0.8 * held), 0x60);
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
    /// The selected tab, relative to `origin`: where the thumb is heading.
    to: Rect,
    /// Row width last frame (the thumb's limit while it is pulled).
    span: f32,
    /// Centre of the thumb while it is pulled, relative to `origin`.
    drag: Option<f32>,
    /// Where a pulled thumb was let go and in which pass: the tab there takes it as a click.
    drop: Option<(f32, u64)>,
    /// Where the thumb is this pass, and the shape it was last drawn into: it is glass over the tab names, so
    /// each tab draws it again after its own name and clears the one before.
    thumb: Rect,
    over: Option<egui::layers::ShapeIdx>,
}

fn tab_plate(ui: &Ui, rect: Rect, selected: bool, resp: &mut egui::Response) {
    let ctx = ui.ctx();
    let id = ui.id().with("tab_row");
    let (left_id, width_id) = (id.with("left"), id.with("width"));
    let pass = ctx.cumulative_pass_nr();
    let old = ctx.data(|d| d.get_temp::<TabRow>(id));
    let mut row = match old {
        Some(mut r) if r.pass == pass => {
            r.right = r.right.max(rect.right());
            r
        }
        _ => {
            let slot = ui.painter().add(egui::Shape::Noop);
            let span = old.map_or(0.0, |r| r.right - r.origin.x);
            let drop = old.and_then(|r| r.drop).filter(|d| d.1 + 1 >= pass);
            TabRow { pass, slot, origin: rect.min, right: rect.right(), to: old.map_or(Rect::NOTHING, |r| r.to), span, drag: old.and_then(|r| r.drag).filter(|_| ctx.input(|i| i.pointer.any_down() || i.pointer.any_released())), drop, thumb: Rect::NOTHING, over: None }
        }
    };
    if row.to != Rect::NOTHING && (resp.dragged() || resp.drag_stopped()) {
        let width = spring_now(ctx, width_id).unwrap_or(row.to.width());
        let half = width / 2.0;
        let (px, dt) = ctx.input(|i| (i.pointer.latest_pos().map_or(rect.center().x, |p| p.x), i.stable_dt));
        let target = (px - row.origin.x).clamp(half, (row.span - half).max(half));
        let cx = row.drag.unwrap_or(spring_now(ctx, left_id).unwrap_or(row.to.left()) + half);
        let cx = cx + (target - cx) * (1.0 - (-dt * 30.0).exp());
        ctx.request_repaint();
        if resp.drag_stopped() {
            // the thumb sets off from where it was let go; the tab under it becomes the selected one next
            spring_set(ctx, left_id, cx - half);
            row.drag = None;
            row.drop = Some((cx, pass));
        } else {
            row.drag = Some(cx);
        }
    }
    if let Some((x, _)) = row.drop {
        let at = row.origin.x + x;
        if rect.left() - 4.0 <= at && at < rect.right() + 4.0 {
            row.drop = None;
            if !selected {
                resp.flags |= egui::response::Flags::FAKE_PRIMARY_CLICKED;
            }
        }
    }
    if selected {
        row.to = rect.translate(-row.origin.to_vec2());
    }
    let mut shapes = Vec::with_capacity(4);
    let track = Rect::from_min_max(row.origin, pos2(row.right, rect.bottom())).expand(3.0);
    let radius = R_CTL + 2.0;
    shapes.push(egui::Shape::rect_filled(track, radius, Color32::from_black_alpha(0x44)));
    shapes.push(egui::Shape::rect_stroke(track, radius, Stroke::new(px(), Color32::from_white_alpha(0x18)), StrokeKind::Inside));
    if row.to != Rect::NOTHING {
        // it eases into the selected tab and never passes it (user's choice: no overshoot here)
        let left = spring(ctx, left_id, row.to.left(), TRAVEL, 1.0);
        let width = spring(ctx, width_id, row.to.width(), TRAVEL, 1.0);
        let mut thumb = Rect::from_min_size(pos2(left, row.to.top()), vec2(width, row.to.height())).translate(row.origin.to_vec2());
        if let Some(cx) = row.drag {
            thumb = thumb.translate(vec2(row.origin.x + cx - thumb.center().x, 0.0)).expand(1.5); // lifted while pulled
        }
        // only its shadow lies under the names; the glass itself goes over them (`tab_thumb`)
        shapes.push(Glass::Raised.shadow().as_shape(thumb, CornerRadius::from(nested(radius, 3.0))).into());
        row.thumb = thumb;
    }
    ui.painter().set(row.slot, egui::Shape::Vec(shapes));
    ctx.data_mut(|d| d.insert_temp(id, row));
}

/// The tab row's thumb, drawn over the names painted so far: a glass plate that bends what is under its rim and,
/// while it is pulled, magnifies the names it passes over.
fn tab_thumb(ui: &Ui) {
    let id = ui.id().with("tab_row");
    let Some(mut row) = ui.ctx().data(|d| d.get_temp::<TabRow>(id)) else { return };
    if let Some(before) = row.over.take() {
        ui.painter().set(before, egui::Shape::Noop);
    }
    if row.thumb != Rect::NOTHING {
        let r = nested(R_CTL + 2.0, 3.0);
        let pulled = if row.drag.is_some() { 1.0 } else { 0.0 };
        let mut shapes = Vec::with_capacity(4);
        shapes.push(crate::lens::shape(row.thumb, crate::lens::Lens { radius: r, rim: 7.0, bend: 1.5 + 3.5 * pulled, zoom: 1.0 + 0.1 * pulled, blur: 0.0, split: 0.3 }));
        shapes.push(egui::Shape::rect_filled(row.thumb, r, Color32::from_white_alpha(0x2A)));
        sheen_into(&mut shapes, row.thumb, r, Glass::Raised.light(), false);
        row.over = Some(ui.painter().add(egui::Shape::Vec(shapes)));
    }
    ui.ctx().data_mut(|d| d.insert_temp(id, row));
}

pub fn spark(ui: &mut Ui, values: &[f32], height: f32) {
    spark_with(ui, values, height, false);
}

/// `spark` with a flat, faint fill under the line.
pub fn spark_filled(ui: &mut Ui, values: &[f32], height: f32) {
    spark_with(ui, values, height, true);
}

fn spark_with(ui: &mut Ui, values: &[f32], height: f32, fill: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), height), egui::Sense::hover());
    if values.len() < 2 {
        return;
    }
    let pts: Vec<Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| pos2(rect.left() + rect.width() * i as f32 / (values.len() - 1) as f32, rect.bottom() - 2.0 - (rect.height() - 4.0) * (v / 100.0).clamp(0.0, 1.0)))
        .collect();
    if fill {
        // a strip of quads down to the baseline: the area under a line graph is not convex, so no polygon fill
        let mut mesh = egui::Mesh::default();
        let col = p().accent.gamma_multiply(0.14);
        for (i, pt) in pts.iter().enumerate() {
            mesh.colored_vertex(*pt, col);
            mesh.colored_vertex(pos2(pt.x, rect.bottom()), col);
            if i > 0 {
                let b = 2 * i as u32;
                mesh.add_triangle(b - 2, b - 1, b);
                mesh.add_triangle(b - 1, b + 1, b);
            }
        }
        ui.painter().add(mesh);
    }
    ui.painter().add(egui::Shape::line(pts, Stroke::new(1.5, p().accent)));
}

pub fn v2(x: f32, y: f32) -> Vec2 { vec2(x, y) }

/// A small label on a tinted plate: 11 semibold, padding 7,2, margin 0,6,6,0.
pub fn chip(ui: &mut Ui, text: &str, fg: Color32, bg: Color32) {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::new(11.0, semibold()), fg);
    let size = vec2(galley.size().x + 14.0, galley.size().y + 4.0);
    let (rect, _) = ui.allocate_exact_size(size + vec2(6.0, 6.0), egui::Sense::hover());
    let chip = Rect::from_min_size(rect.min + vec2(0.0, 6.0), size);
    ui.painter().rect_filled(chip, 6.0, bg);
    ui.painter().galley(pos2(chip.left() + 7.0, chip.top() + 2.0), galley, fg);
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

/// The plate a text-only button shows under the pointer and while it is pulled.
fn subtle_plate(ui: &Ui, resp: &egui::Response, rect: Rect, off: Vec2) {
    let hov = anim(ui.ctx(), resp.id.with("hover"), resp.hovered() || off != Vec2::ZERO, HOVER);
    if hov > 0.0 {
        ui.painter().rect_filled(pulled(rect, off), R_CTL, Color32::from_white_alpha((20.0 * hov) as u8));
    }
}

/// A text-only button: no plate until the pointer is on it.
pub fn subtle_button(ui: &mut Ui, text: &str, pad: (f32, f32, f32, f32)) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::new(14.0, medium()), TEXT);
    let size = vec2(galley.size().x + pad.0 + pad.2, galley.size().y + pad.1 + pad.3);
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    let off = pull(ui, &mut resp);
    subtle_plate(ui, &resp, rect, off);
    ui.painter().galley(pos2(rect.left() + pad.0, rect.top() + pad.1) + off / 2.0, galley, TEXT);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Progress bar: 4 px, accent fill on a faint track.
pub fn progress(ui: &mut Ui, fraction: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, Color32::from_white_alpha(0x29));
    let fill = Rect::from_min_size(rect.min, vec2(rect.width() * fraction.clamp(0.0, 1.0), 4.0));
    ui.painter().rect_filled(fill, 2.0, p().accent);
}

/// `subtle_button` whose plate reaches `bleed` px left of the layout position, so the text lines up with the
/// content above.
pub fn subtle_button_bleed(ui: &mut Ui, text: &str, pad: (f32, f32, f32, f32), bleed: f32) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::new(14.0, medium()), TEXT);
    let size = vec2(galley.size().x + pad.0 + pad.2, galley.size().y + pad.1 + pad.3);
    let (slot, _) = ui.allocate_exact_size(size - vec2(bleed, 0.0), egui::Sense::hover());
    let rect = Rect::from_min_size(slot.min - vec2(bleed, 0.0), size);
    let mut resp = ui.interact(rect, ui.id().with(text), egui::Sense::click_and_drag());
    let off = pull(ui, &mut resp);
    subtle_plate(ui, &resp, rect, off);
    ui.painter().galley(pos2(rect.left() + pad.0, rect.top() + pad.1) + off / 2.0, galley, TEXT);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// One tab of a segmented row (call it for each tab inside one `ui.horizontal`): the row has a dark track and a
/// glass thumb that travels to the selected tab. Padding 13,6.
pub fn tab(ui: &mut Ui, text: &str, selected: bool) -> egui::Response {
    let fg = if selected { TEXT } else { text2() };
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::new(14.0, medium()), fg);
    let size = vec2(galley.size().x + 26.0, galley.size().y + 12.0);
    let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    tab_plate(ui, rect, selected, &mut resp);
    let hov = anim(ui.ctx(), resp.id.with("hover"), resp.hovered() && !selected, HOVER);
    let paint = ui.painter();
    if hov > 0.0 {
        paint.rect_filled(rect, nested(R_CTL + 2.0, 3.0), Color32::from_white_alpha((16.0 * hov) as u8));
    }
    paint.galley(pos2(rect.left() + 13.0, rect.top() + 6.0), galley, fg.lerp_to_gamma(TEXT, hov));
    tab_thumb(ui);
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
        let (rect, mut resp) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
        let rect = pulled(rect, pull(ui, &mut resp));
        let hov = anim(ui.ctx(), resp.id.with("hover"), resp.hovered(), HOVER);
        if self.on {
            plate(ui.painter(), rect, R_CTL, pal.accent.lerp_to_gamma(Color32::WHITE, 0.10 * hov), 0x70);
        } else {
            plate(ui.painter(), rect, R_CTL, Color32::from_white_alpha(0x1A + (14.0 * hov) as u8), 0x34);
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

    /// A spring with damping 1 (tab thumb, hover, press) reaches its target and never passes it; one with less
    /// damping (the switch knob) passes it once, by less than its travel, and still comes to rest there.
    #[test]
    fn spring_settles_on_target() {
        for (damping, may_pass) in [(1.0, 0.0005), (0.55, 0.2)] {
            let (mut x, mut v, mut most) = (0.0f32, 0.0f32, 0.0f32);
            for _ in 0..240 {
                (x, v) = spring_step(x, v, 1.0, 1.0 / 120.0, 0.3, damping);
                most = most.max(x);
            }
            assert!(most <= 1.0 + may_pass, "damping {damping}: went to {most}");
            assert!((x - 1.0).abs() < 0.001 && v.abs() < 0.01, "damping {damping}: ended at {x}, speed {v}");
        }
    }

    /// Every corner is the smooth curve: it starts 1.5 radii from the corner where there is room, cuts the corner
    /// exactly as deep as a circle of that radius, and a circle or a pill end stays a true circle.
    #[test]
    fn corners_are_smooth_and_circles_stay_round() {
        let at = |rect: Rect, r: f32| outline(rect, r);
        // a wide plate, radius 10: the top edge's straight part ends 15 px from the corner
        let pts = at(Rect::from_min_size(pos2(0.0, 0.0), vec2(200.0, 100.0)), 10.0);
        let top: Vec<f32> = pts.iter().filter(|q| q.y.abs() < 0.001).map(|q| q.x).collect();
        assert!(top.iter().any(|x| (x - 15.0).abs() < 0.01) && top.iter().any(|x| (x - 185.0).abs() < 0.01), "{top:?}");
        // on the diagonal the curve is where the circle of radius 10 is: 10 * (1 - 1/sqrt 2) from both edges
        let want = 10.0 * (1.0 - std::f32::consts::FRAC_1_SQRT_2);
        let near = pts.iter().map(|q| ((q.x - want).powi(2) + (q.y - want).powi(2)).sqrt()).fold(f32::MAX, f32::min);
        assert!(near < 0.6, "nearest outline point is {near} px from the circle's diagonal point");
        // a circle and a pill: every point of the left end is 10 px from the end's centre
        for w in [20.0, 80.0] {
            for q in at(Rect::from_min_size(pos2(0.0, 0.0), vec2(w, 20.0)), 10.0) {
                if q.x <= 10.0 {
                    let d = ((q.x - 10.0).powi(2) + (q.y - 10.0).powi(2)).sqrt();
                    assert!((d - 10.0).abs() < 0.01, "width {w}: point {q:?} is {d} from the centre");
                }
            }
        }
        // no point repeats its neighbour (a repeat drew a line to the centre)
        for (w, h, r) in [(20.0, 20.0, 10.0), (80.0, 20.0, 10.0), (200.0, 100.0, 10.0), (30.0, 30.0, 9.0), (7.0, 40.0, 3.5)] {
            let pts = at(Rect::from_min_size(pos2(3.3, 7.7), vec2(w, h)), r);
            for i in 0..pts.len() {
                assert!((pts[i] - pts[(i + 1) % pts.len()]).length() >= 0.01, "{w}x{h} r{r}: point {i} repeats");
            }
        }
    }

    /// A pull follows the pointer at first and never gives more than `PULL_MAX`.
    #[test]
    fn pull_is_rubber_banded() {
        assert!((rubber(vec2(1.0, 0.0)).x - 1.0).abs() < 0.1);
        let mut last = 0.0;
        for i in 1..400 {
            let x = rubber(vec2(i as f32 * 5.0, 0.0)).x;
            assert!(x > last && x < PULL_MAX, "{x}");
            last = x;
        }
        assert_eq!(pulled(Rect::from_min_size(pos2(0.0, 0.0), vec2(40.0, 20.0)), vec2(6.0, 0.0)), Rect::from_min_max(pos2(0.0, 0.0), pos2(46.0, 20.0)));
    }

    /// The accent is one global: tests that set it run one at a time.
    static ACCENT: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A plate thinner than its corner radius must not panic (it closed the app while the window shrank).
    #[test]
    fn sheen_survives_thin_plates() {
        for (w, h) in [(10.0, 200.0), (200.0, 10.0), (1.0, 1.0), (0.0, 50.0), (-5.0, 50.0), (64.0, 64.0)] {
            let mut out = Vec::new();
            sheen_into(&mut out, Rect::from_min_size(pos2(27.0, 27.0), vec2(w, h)), 22.0, (40, 40, 40), true);
        }
    }

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
