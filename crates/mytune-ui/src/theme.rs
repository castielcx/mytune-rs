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
pub fn text3() -> Color32 { Color32::from_white_alpha(0x90) }
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
            side: Color32::from_white_alpha(0x14),
            side_selected: Color32::from_white_alpha(0x2C),
            side_text: Color32::WHITE,
            side_muted: Color32::from_white_alpha(0xB8),
            layer_bg: t(0.0, 0.35, 0.06),
            card_bg: Color32::from_white_alpha(0x14),
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
    let read = |f: &str| std::fs::read(format!("{fonts_dir}{f}")).ok();
    let base = fonts.families[&FontFamily::Proportional].clone();
    let mut add = |fonts: &mut egui::FontDefinitions, key: &str, data: egui::FontData| {
        fonts.font_data.insert(key.into(), std::sync::Arc::new(data));
    };
    if let Some(bytes) = read("SegUIVar.ttf") {
        let blob: std::sync::Arc<Vec<u8>> = std::sync::Arc::new(bytes);
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
                add(&mut fonts, &key, egui::FontData::from_owned(b));
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
        add(&mut fonts, "segoe-icons", egui::FontData::from_owned(b));
        list.insert(0, "segoe-icons".into());
    }
    fonts.families.insert(FontFamily::Name("icons".into()), list);
    // Chat names and messages hold symbols Segoe UI lacks (e.g. U+271E): last-resort fallback, ~1 MB.
    if let Some(b) = read("seguisym.ttf") {
        add(&mut fonts, "segoe-symbols", egui::FontData::from_owned(b));
        for fam in [FontFamily::Proportional, FontFamily::Name("semibold".into()), FontFamily::Name("bold".into())] {
            if let Some(l) = fonts.families.get_mut(&fam) {
                l.push("segoe-symbols".into());
            }
        }
    }
    // Emoji in reactions, names and messages: Segoe UI Emoji's outlines (egui draws them one colour). ~12 MB.
    if let Some(b) = read("seguiemj.ttf") {
        add(&mut fonts, "segoe-emoji", egui::FontData::from_owned(b));
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
        for k in 0..=10 {
            // arc from one edge point to the other
            let t = k as f32 / 10.0 * std::f32::consts::FRAC_PI_2;
            let q = c + vec2(-sx * r * t.sin(), -sy * r * t.cos());
            m.colored_vertex(q, bg(q));
        }
        for k in 1..=10 {
            m.add_triangle(0, k, k + 1);
        }
        p.add(egui::Shape::mesh(m));
    }
}

/// The four backdrop blobs for a window rect: centre, radius, colour, opacity.
fn blobs(rect: Rect) -> [(Pos2, f32, Color32, f32); 4] {
    let a = p_accent_hsv();
    let big = rect.width().max(rect.height());
    let at = |fx: f32, fy: f32| pos2(rect.left() + rect.width() * fx, rect.top() + rect.height() * fy);
    [(0.92, 0.05, 0.62, 0.0, 0.6, 0.85, 0.7), (0.08, 1.0, 0.58, 0.14, 0.6, 0.7, 0.62), (0.62, 1.05, 0.4, -0.12, 0.55, 0.7, 0.55), (0.3, 0.1, 0.28, 0.07, 0.5, 0.7, 0.38)]
        .map(|(fx, fy, r, dh, s, v, al)| (at(fx, fy), big * r, from_hsv(a.0 + dh, s * a.1, v), al))
}

/// Page background: dark base with four soft blobs in the accent's hue (a grey accent gets a calm blue).
/// Rendered once into a small texture (1/8 size, smoothed when stretched) and redrawn as one quad; it is made
/// again only when the window size (in 16 px steps) or the accent changes.
pub fn backdrop(ctx: &egui::Context, p: &egui::Painter, rect: Rect) {
    let (w, h) = (((rect.width() / 16.0).ceil() as usize).max(1), ((rect.height() / 16.0).ceil() as usize).max(1));
    let key = (w, h, self::p().accent.to_array(), self::p().layer_bg.to_array());
    let id = egui::Id::new("backdrop_tex");
    let cached = ctx.data(|d| d.get_temp::<((usize, usize, [u8; 4], [u8; 4]), egui::TextureHandle)>(id));
    let tex = match cached {
        Some((k, t)) if k == key => t,
        _ => {
            let (iw, ih) = (w * 2, h * 2); // 8 px per texel
            let mut px = Vec::with_capacity(iw * ih * 3);
            let step = rect.width() / iw as f32;
            for y in 0..ih {
                for x in 0..iw {
                    let c = backdrop_at(rect, rect.min + vec2((x as f32 + 0.5) * step, (y as f32 + 0.5) * rect.height() / ih as f32));
                    px.extend([c.r(), c.g(), c.b()]);
                }
            }
            let t = ctx.load_texture("backdrop", egui::ColorImage::from_rgb([iw, ih], &px), egui::TextureOptions::LINEAR);
            ctx.data_mut(|d| d.insert_temp(id, (key, t.clone())));
            t
        }
    };
    p.image(tex.id(), rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
}

/// The colour `backdrop` paints at `pos`, for shapes that must blend into it.
pub fn backdrop_at(rect: Rect, pos: Pos2) -> Color32 {
    let mut c = self::p().layer_bg;
    for (centre, r, col, al) in blobs(rect) {
        let k = 1.0 - pos.distance(centre) / r;
        if k > 0.0 {
            c = c.lerp_to_gamma(col, al * k * k * k);
        }
    }
    c
}

fn p_accent_hsv() -> (f32, f32) {
    let c = p().accent;
    let (h, s, _) = to_hsv([c.r() as f32, c.g() as f32, c.b() as f32]);
    if s < 0.1 { (0.6, 0.55) } else { (h, 1.0) }
}

/// Rounded translucent plate with a soft shadow and a thin rim that is brighter at the top left.
pub fn glass(rect: Rect, radius: f32, fill: Color32) -> egui::Shape {
    let r = radius.min(rect.height() / 2.0).min(rect.width() / 2.0);
    let shadow = egui::Shadow { offset: [0, 6], blur: 22, spread: 0, color: Color32::from_black_alpha(46) };
    let mut shapes = vec![shadow.as_shape(rect, CornerRadius::from(r)).into(), egui::Shape::rect_filled(rect, r, fill)];
    shapes.push(egui::Shape::rect_stroke(rect, r, Stroke::new(1.0, Color32::from_white_alpha(0x1E)), StrokeKind::Inside));
    // highlight: up the left edge, round the corner, along the top (fading out at both ends)
    let (l, t) = (rect.left() + 0.5, rect.top() + 0.5);
    let mut pts = vec![pos2(l, t + (rect.height() * 0.45).max(r))];
    for k in 0..=8 {
        let a = std::f32::consts::PI + k as f32 / 8.0 * std::f32::consts::FRAC_PI_2;
        pts.push(pos2(l + r + r * a.cos(), t + r + r * a.sin()));
    }
    pts.push(pos2((l + r + rect.width() * 0.55).min(rect.right() - r), t));
    let n = pts.len();
    for (seg, alpha) in [(0..n / 2 + 1, 0x78u8), (n / 2..n, 0x38u8)] {
        shapes.push(egui::Shape::line(pts[seg].to_vec(), Stroke::new(1.0, Color32::from_white_alpha(alpha))));
    }
    egui::Shape::Vec(shapes)
}

pub fn apply_visuals(ctx: &egui::Context) {
    let pal = p();
    let mut v = egui::Visuals::dark();
    let zero = CornerRadius::same(8);
    v.override_text_color = Some(TEXT);
    v.panel_fill = pal.layer_bg;
    v.window_fill = tone(0.13); // dialogs and menus are not blurred, so they stay opaque
    v.extreme_bg_color = pal.ink;
    // Dialogs, menus and combo lists share one soft shape and shadow, like the custom popups.
    let soft = egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(102) };
    v.window_corner_radius = CornerRadius::same(16);
    v.menu_corner_radius = CornerRadius::same(12);
    v.window_shadow = soft;
    v.popup_shadow = soft;
    v.window_stroke = Stroke::new(1.0, Color32::from_white_alpha(0x24));
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
    ctx.global_style_mut(|s| { s.spacing.item_spacing = vec2(8.0, 0.0); s.animation_time = 0.16; });
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
    ui.painter().set(slot, glass(out.response.rect, 16.0, fill));
    out.inner
}

/// Section heading: 13 semibold in the accent colour, margin 0,24,0,8.
pub fn section(ui: &mut Ui, text: &str) {
    ui.add_space(24.0);
    ui.label(dsb(text, 13.0).color(TEXT));
    ui.add_space(8.0);
}

/// XAML button: default (ControlBg + 1px border) or accent (filled, bold OnAccent text).
pub fn button(ui: &mut Ui, text: &str, accent: bool, pad: (f32, f32, f32, f32)) -> egui::Response {
    let pal = p();
    let family = if accent { on_light() } else { FontFamily::Proportional };
    let color = if accent { pal.on_accent } else { TEXT };
    let size = {
        let g = ui.painter().layout_no_wrap(text.to_string(), FontId::new(14.0, family.clone()), color);
        vec2(g.size().x + pad.0 + pad.2, g.size().y + pad.1 + pad.3)
    };
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    // pill that grows 5% under the pointer and shrinks 4% while pressed (text grows with it)
    let ctx = ui.ctx().clone();
    let hov = ctx.animate_bool_with_time(resp.id.with("hover"), resp.hovered(), 0.12);
    let down = ctx.animate_bool_with_time(resp.id.with("down"), resp.is_pointer_button_down_on(), 0.08);
    let s = 1.0 + 0.05 * hov - 0.09 * down;
    let plate = Rect::from_center_size(rect.center(), rect.size() * s);
    let paint = ui.painter();
    if accent {
        paint.rect_filled(plate, plate.height() / 2.0, pal.accent.lerp_to_gamma(Color32::WHITE, 0.12 * hov));
    } else {
        paint.rect_filled(plate, plate.height() / 2.0, Color32::from_white_alpha(20 + (16.0 * hov) as u8));
        paint.rect_stroke(plate, plate.height() / 2.0, Stroke::new(1.0, Color32::from_white_alpha(0x2A + (16.0 * hov) as u8)), StrokeKind::Inside);
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
    paint.rect_filled(rect, rect.height() / 2.0, off_bg.lerp_to_gamma(pal.accent, t));
    paint.rect_stroke(rect, rect.height() / 2.0, Stroke::new(1.0, Color32::from_white_alpha(0x30)), StrokeKind::Inside);
    let kw = 24.0 + 8.0 * held;
    let kh = 20.0 + 4.0 * held;
    let half = kw / 2.0 + 3.0;
    let cx = egui::lerp((rect.left() + half)..=(rect.right() - half), t) + wobble;
    let knob = Rect::from_center_size(pos2(cx, rect.center().y), vec2(kw, kh));
    let off_knob = Color32::WHITE;
    let fill = off_knob.lerp_to_gamma(pal.on_accent, t);
    paint.rect_filled(knob, kh / 2.0, fill.gamma_multiply(1.0 - 0.7 * held));
    paint.rect_stroke(knob, kh / 2.0, Stroke::new(1.0, Color32::from_white_alpha((0x50 as f32 + 0xA0 as f32 * held) as u8)), StrokeKind::Inside);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
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

/// `TabItem` radio tab: hover tint, selected = accent fill with bold OnAccent text. Padding 15,6,15,7.
pub fn tab(ui: &mut Ui, text: &str, selected: bool) -> egui::Response {
    let pal = p();
    let font = if selected { FontId::new(14.0, bold()) } else { FontId::proportional(14.0) };
    let fg = if selected { pal.on_accent } else { text2() };
    let galley = ui.painter().layout_no_wrap(text.to_string(), font, fg);
    let size = vec2(galley.size().x + 30.0, galley.size().y + 13.0);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let paint = ui.painter();
    if selected {
        paint.rect_filled(rect, rect.height() / 2.0, pal.accent);
    } else if resp.hovered() {
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
