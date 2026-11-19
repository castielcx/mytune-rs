//! Sharp small pictures. A picture drawn much smaller than its pixels (a 128 px avatar at 46 px) is soft:
//! the GPU blends two mipmap levels. The owner of a texture hands its pixels to `register`; `scan`, at the
//! end of each frame, finds every rectangle drawn with such a texture, makes a copy of the picture at exactly
//! that many pixels (Lanczos) and draws the rectangle with the copy. One copy per drawn size: the same avatar
//! is 37 px in the DM list and 46 px beside a message.

use eframe::egui::{self, epaint::Shape, LayerId, TextureHandle, TextureId};
use std::{cell::RefCell, collections::HashMap, sync::Arc};

/// Copies made per frame at most (a room opening shows some dozens of avatars at once).
const PER_FRAME: usize = 24;

enum Pixels {
    /// Alpha-multiplied pixels.
    Kept(image::RgbaImage),
    /// Decoded again when a copy is needed (emoji: thousands of them, kept forever).
    Made(Box<dyn Fn() -> Option<image::RgbaImage>>),
}

#[derive(Default)]
struct State {
    /// Registered pictures and their width.
    sources: HashMap<TextureId, (u32, Pixels)>,
    /// Copies by picture and width, with the time of their last draw.
    copies: HashMap<(TextureId, u32), (TextureHandle, f64)>,
    pruned: f64,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Hands over a texture's pixels (plain, not alpha-multiplied), so its small draws get sharp copies.
pub fn register(id: TextureId, rgba: &[u8], w: u32, h: u32) {
    if let Some(img) = image::RgbaImage::from_raw(w, h, rgba.to_vec()) {
        STATE.with(|s| s.borrow_mut().sources.insert(id, (w, Pixels::Kept(premultiplied(img)))));
    }
}

/// As `register`, but the pixels (plain) are asked for only when a copy is made.
pub fn register_lazy(id: TextureId, w: u32, pixels: impl Fn() -> Option<image::RgbaImage> + 'static) {
    STATE.with(|s| s.borrow_mut().sources.insert(id, (w, Pixels::Made(Box::new(move || pixels().map(premultiplied))))));
}

/// Colours multiplied by alpha: filtering plain colours pulls the black of see-through pixels into the edges.
fn premultiplied(mut img: image::RgbaImage) -> image::RgbaImage {
    for p in img.chunks_exact_mut(4) {
        let a = p[3] as u32;
        for c in &mut p[..3] {
            *c = ((*c as u32 * a + 127) / 255) as u8;
        }
    }
    img
}

/// How many pixels wide the whole picture is drawn. egui snaps a rectangle's edges to whole pixels before
/// drawing: this counts the same pixels, or the copy is one pixel off and gets filtered again. A cropped draw
/// shows only part of the picture, so it is scaled up to the whole.
fn drawn_width(rect: egui::Rect, uv: egui::Rect, ppp: f32) -> u32 {
    if uv.width() <= 0.0 {
        return 0;
    }
    (((rect.max.x * ppp).round() - (rect.min.x * ppp).round()) / uv.width()).round().max(0.0) as u32
}

fn each_textured(s: &Shape, f: &mut impl FnMut(TextureId, u32), ppp: f32) {
    match s {
        Shape::Vec(v) => v.iter().for_each(|s| each_textured(s, f, ppp)),
        Shape::Rect(r) => {
            if let Some(b) = &r.brush {
                f(b.fill_texture_id, drawn_width(r.rect, b.uv, ppp));
            }
        }
        _ => {}
    }
}

fn swap(s: &mut Shape, copies: &HashMap<(TextureId, u32), (TextureHandle, f64)>, ppp: f32) {
    match s {
        Shape::Vec(v) => v.iter_mut().for_each(|s| swap(s, copies, ppp)),
        Shape::Rect(r) => {
            if let Some(b) = &r.brush {
                if let Some((copy, _)) = copies.get(&(b.fill_texture_id, drawn_width(r.rect, b.uv, ppp))) {
                    r.brush = Some(Arc::new(egui::epaint::Brush { fill_texture_id: copy.id(), uv: b.uv }));
                }
            }
        }
        _ => {}
    }
}

/// Call once per frame after everything is drawn (and after anything that keeps this frame's shapes for
/// later: a copy's texture may be freed).
pub fn scan(ctx: &egui::Context) {
    let ppp = ctx.pixels_per_point();
    let now = ctx.input(|i| i.time);
    let mut layers: Vec<LayerId> = ctx.memory(|m| m.layer_ids().collect());
    layers.push(LayerId::background());
    STATE.with(|st| {
        let mut st = st.borrow_mut();
        if st.sources.is_empty() {
            return;
        }
        // which copies this frame draws: only smaller than the picture, a larger draw keeps the picture itself
        let mut needs: Vec<(TextureId, u32)> = Vec::new();
        ctx.graphics(|g| {
            for list in layers.iter().filter_map(|l| g.get(*l)) {
                for s in list.all_entries() {
                    each_textured(&s.shape, &mut |id, w| {
                        if w > 0 && st.sources.get(&id).is_some_and(|(full, _)| w < *full) && !needs.contains(&(id, w)) {
                            needs.push((id, w));
                        }
                    }, ppp);
                }
            }
        });
        // textures are made outside the graphics lock (the context is locked inside it)
        let mut made = 0;
        for key in &needs {
            if let Some((_, used)) = st.copies.get_mut(key) {
                *used = now;
            } else if made == PER_FRAME {
                ctx.request_repaint(); // the rest next frame
            } else if let Some(img) = st.sources.get(&key.0).and_then(|(_, px)| copy(px, key.1)) {
                made += 1;
                let tex = ctx.load_texture("sharp", img, egui::TextureOptions::LINEAR);
                st.copies.insert(*key, (tex, now));
            }
        }
        if !needs.is_empty() {
            let copies = &st.copies;
            ctx.graphics_mut(|g| {
                for l in &layers {
                    if let Some(list) = g.get_mut(*l) {
                        let count = list.all_entries().len();
                        for i in 0..count {
                            list.mutate_shape(egui::layers::ShapeIdx(i), |c| swap(&mut c.shape, copies, ppp));
                        }
                    }
                }
            });
        }
        // every 10 s: copies not drawn for a minute go, and so do the pixels of textures that were freed
        if now - st.pruned > 10.0 {
            st.pruned = now;
            st.copies.retain(|_, (_, used)| now - *used < 60.0);
            let alive = ctx.tex_manager();
            let alive = alive.read();
            st.sources.retain(|id, _| alive.meta(*id).is_some());
        }
    });
}

fn copy(px: &Pixels, to_w: u32) -> Option<egui::ColorImage> {
    match px {
        Pixels::Kept(img) => Some(fit(img, to_w)),
        Pixels::Made(make) => make().map(|img| fit(&img, to_w)),
    }
}

/// The alpha-multiplied picture shrunk to `to_w` pixels wide.
fn fit(img: &image::RgbaImage, to_w: u32) -> egui::ColorImage {
    let (w, h) = (img.width().max(1), img.height());
    let to_w = to_w.clamp(1, w);
    let to_h = ((h as u64 * to_w as u64 + w as u64 / 2) / w as u64).max(1) as u32;
    let mut out = image::imageops::resize(img, to_w, to_h, image::imageops::FilterType::Lanczos3);
    // Lanczos overshoots: a colour above its alpha is not a valid premultiplied pixel
    for p in out.chunks_exact_mut(4) {
        let a = p[3];
        for c in &mut p[..3] {
            *c = (*c).min(a);
        }
    }
    egui::ColorImage::from_rgba_premultiplied([to_w as usize, to_h as usize], &out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, Rect};

    #[test]
    fn counts_the_pixels_egui_draws() {
        let whole = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
        // 40 pt at 1.15 is 46.000004 px: 46, not 47
        assert_eq!(drawn_width(Rect::from_min_size(pos2(10.0, 0.0), egui::vec2(40.0, 40.0)), whole, 1.15), 46);
        // 48 pt at 1.15 is 55.2 px: 55 or 56 by where its edges fall
        assert_eq!(drawn_width(Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(48.0, 48.0)), whole, 1.15), 55);
        assert_eq!(drawn_width(Rect::from_min_size(pos2(0.4, 0.0), egui::vec2(48.0, 48.0)), whole, 1.15), 56);
        // half the picture across 20 px: the whole would be 40
        let half = Rect::from_min_max(pos2(0.25, 0.0), pos2(0.75, 1.0));
        assert_eq!(drawn_width(Rect::from_min_size(pos2(0.0, 0.0), egui::vec2(20.0, 20.0)), half, 1.0), 40);
    }

    #[test]
    fn fit_keeps_shape_and_clear_pixels() {
        // left half opaque white, right half fully clear black
        let mut px = Vec::new();
        for _ in 0..4 {
            for x in 0..8 {
                px.extend_from_slice(if x < 4 { &[255, 255, 255, 255] } else { &[0, 0, 0, 0] });
            }
        }
        let img = fit(&premultiplied(image::RgbaImage::from_raw(8, 4, px).unwrap()), 4);
        assert_eq!(img.size, [4, 2]);
        assert_eq!(img.pixels[0], egui::Color32::WHITE);
        assert_eq!(img.pixels[3], egui::Color32::TRANSPARENT);
    }
}
