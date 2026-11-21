//! Crosshair page: on/off, hotkey, monitor, and the built-in editor with a live preview. The overlay window
//! itself lives in `mytune_core::crosshair`; this page only edits the settings and hands them over.

use crate::theme::{self as th, sb};
use crate::widgets::{self as w, caption};
use eframe::egui::{self, pos2, vec2, Align, Color32, FontId, Layout, Rect, Sense, TextureHandle, Ui, Vec2};
use mytune_core::crosshair::{self as ch, Crosshair};
use mytune_core::display::{self, Monitor};
use mytune_core::engine::Engine;
use std::sync::Arc;

pub struct CrosshairPage {
    engine: Arc<Engine>,
    monitors: Vec<Monitor>,
    saved: Option<Crosshair>,
    saves: Vec<ch::Saved>,
    name: String,
    tab: usize,
    /// The editor's picture: the look it shows, its texture and side. Redrawn only when the look changes.
    prev: Option<(Crosshair, TextureHandle, i32, usize)>,
    /// One small picture per saved look (same order as `saves`), made when first drawn.
    thumbs: Vec<Option<(TextureHandle, i32)>>,
}

/// Width of the editor's picture box: one buffer pixel is one screen pixel, so it must hold the biggest overlay
/// (313 px) at any scale factor.
const PREVIEW_W: f32 = 340.0;
/// The editor's height on every tab (tab row + the 11 rows of Lines), so the card never jumps.
const EDITOR_H: f32 = 41.0 + 11.0 * ROW_H;

impl CrosshairPage {
    /// Env `MYTUNE_CROSSHAIR_TAB` (test switch) opens another editor tab.
    pub fn new(engine: Arc<Engine>) -> CrosshairPage {
        let saves = engine.with_store(|s| ch::load_saves(s));
        CrosshairPage { engine, monitors: display::monitors(), saved: None, saves, name: String::new(), tab: std::env::var("MYTUNE_CROSSHAIR_TAB").ok().and_then(|v| v.parse().ok()).unwrap_or(0), prev: None, thumbs: vec![] }
    }

    fn write_saves(&mut self) {
        self.thumbs.clear();
        let _ = self.engine.with_store(|s| ch::save_saves(s, &self.saves));
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        let mut c = ch::live(); // the hotkey can flip `on` behind the page's back
        let before = c.clone();
        self.saved.get_or_insert_with(|| before.clone());

        th::group(ui, |ui| {
            th::card(ui, (16, 4, 16, 4), |ui| {
                row_body(ui, "Crosshair overlay", "", |ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if th::switch(ui, c.on).clicked() {
                            c.on = !c.on;
                        }
                    });
                });
                if let Some(p) = ch::problem() {
                    ui.label(crate::theme::rt(p, 12.0).color(th::WARNING));
                    ui.add_space(6.0);
                }
            });
            th::card(ui, (16, 4, 16, 4), |ui| {
                row_body(ui, "Hotkey", "", |ui| {
                    let rows: Vec<(String, bool)> = ch::HOTKEYS.iter().map(|k| (k.to_string(), *k == c.hotkey)).collect();
                    if let Some(i) = end_dropdown(ui, "crosshair-key", &c.hotkey, &rows) {
                        c.hotkey = ch::HOTKEYS[i].to_string();
                    }
                });
            });
            if self.monitors.len() > 1 {
                th::card(ui, (16, 4, 16, 4), |ui| {
                    row_body(ui, "Monitor", "", |ui| {
                        let primary = if c.monitor.is_empty() { self.monitors.first().map(|m| m.device.clone()).unwrap_or_default() } else { c.monitor.clone() };
                        let shown = self.monitors.iter().find(|m| m.device == primary).map_or("Primary".to_string(), |m| m.label.clone());
                        let rows: Vec<(String, bool)> = self.monitors.iter().map(|m| (m.label.clone(), m.device == primary)).collect();
                        if let Some(i) = end_dropdown(ui, "crosshair-monitor", &shown, &rows) {
                            c.monitor = self.monitors[i].device.clone();
                        }
                    });
                });
            }
        });
        ui.add_space(th::GAP);

        // editor: presets, tabs and their controls on the left, the picture the screen will show on the right
        // whole-number magnification so the real pixels stay countable, drawn enlarged by the rasteriser itself
        // (smooth edges stay smooth, no blocks); 1x when it is big
        let ppp = ui.ctx().pixels_per_point();
        let z = ((PREVIEW_W * ppp / ch::side(&c) as f32).floor()).clamp(1.0, 8.0) as usize;
        if !self.prev.as_ref().is_some_and(|(l, _, _, pz)| l.same_look(&c) && *pz == z) {
            let (side, px) = ch::render_rgba_at(&c, z);
            let img = egui::ColorImage::from_rgba_premultiplied([side as usize; 2], &px);
            let tex = ui.ctx().load_texture("crosshair-preview", img, egui::TextureOptions::NEAREST);
            self.prev = Some((c.clone(), tex, side, z));
        }
        let prev = self.prev.as_ref().map(|(_, t, s, _)| (t.clone(), *s));
        let mut tab = self.tab;
        th::card(ui, (16, 14, 16, 14), |ui| {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let lw = (ui.available_width() - PREVIEW_W - 24.0).max(340.0);
                let left = ui.vertical(|ui| {
                    ui.set_width(lw);
                    ui.set_min_height(EDITOR_H);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for (i, name) in ["Lines", "Center Dot", "Outline"].iter().enumerate() {
                            if th::tab(ui, name, tab == i).clicked() {
                                tab = i;
                            }
                        }
                    });
                    ui.add_space(8.0);
                    match tab {
                        0 => lines_tab(ui, &mut c),
                        1 => dot_tab(ui, &mut c),
                        _ => outline_tab(ui, &mut c),
                    }
                });
                let left_h = left.response.rect.height();
                ui.add_space(24.0);
                // picture on top, presets as an even grid under it; together exactly as tall as the left column
                ui.vertical(|ui| {
                    ui.set_width(PREVIEW_W);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let names: Vec<&str> = ch::PRESETS.iter().map(|p| p.0).chain(["Reset"]).collect();
                    let btn_h = ui.painter().layout_no_wrap("Ag".into(), FontId::new(14.0, th::medium()), th::TEXT).size().y + th::BTN.1 + th::BTN.3;
                    let grid_h = names.len().div_ceil(3) as f32 * (btn_h + 8.0);
                    let (rect, _) = ui.allocate_exact_size(vec2(PREVIEW_W, left_h - grid_h), Sense::hover());
                    ui.painter().rect(rect, th::R_CTL, Color32::from_black_alpha(0x48), egui::Stroke::new(1.0, Color32::from_white_alpha(0x1C)), egui::StrokeKind::Inside);
                    if let Some((tex, side)) = &prev {
                        draw_picture(ui, rect, tex, *side, 1.0 / ppp, true);
                        let note = if z > 1 { format!("Preview at {z}\u{D7} zoom") } else { "Preview at real size".to_string() };
                        ui.painter().text(rect.left_bottom() + vec2(10.0, -10.0), egui::Align2::LEFT_BOTTOM, note, FontId::proportional(12.0), th::text3());
                    }
                    let cell = (PREVIEW_W - 16.0) / 3.0;
                    for (r, chunk) in names.chunks(3).enumerate() {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            for (j, name) in chunk.iter().enumerate() {
                                if even_button(ui, name, cell).clicked() {
                                    let i = r * 3 + j;
                                    if i < ch::PRESETS.len() {
                                        c.preset(i);
                                    } else {
                                        c.apply_look(&Crosshair::default());
                                    }
                                }
                            }
                        });
                    }
                });
            });
        });
        self.tab = tab;
        ui.add_space(th::GAP);

        // saved crosshairs: own block, name field + Save on top, a tile per save (picture, name); click applies, × deletes
        let (mut apply, mut remove, mut save_now) = (None, None, false);
        self.thumbs.resize_with(self.saves.len(), || None);
        let thumbs = &mut self.thumbs;
        let saves = &self.saves;
        let name = &mut self.name;
        th::card(ui, (16, 14, 16, 14), |ui| {
            ui.horizontal(|ui| {
                ui.label(sb("Saved crosshairs", 14.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    save_now = th::button(ui, "Save", false, th::BTN).clicked();
                    ui.add(egui::TextEdit::singleline(name).desired_width(170.0).char_limit(20).hint_text("Name"));
                });
            });
            if saves.is_empty() {
                ui.add_space(8.0);
                ui.label(crate::theme::rt("Nothing saved yet. Give the current look a name and press Save.", 13.0).color(th::text3()));
                return;
            }
            ui.add_space(12.0);
            const TILE: Vec2 = vec2(120.0, 112.0);
            let gap = th::GAP;
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(gap, gap);
                for (i, s) in saves.iter().enumerate() {
                    let (rect, resp) = ui.allocate_exact_size(TILE, Sense::click());
                    let same = s.look.same_look(&c);
                    let fill = Color32::from_white_alpha(if resp.hovered() { 0x14 } else { 0x0A });
                    ui.painter().rect_filled(rect, th::R_CTL, fill);
                    if same {
                        ui.painter().rect_stroke(rect, th::R_CTL, egui::Stroke::new(1.5, th::p().accent), egui::StrokeKind::Inside);
                    }
                    let pic = Rect::from_min_size(rect.min + vec2(8.0, 8.0), vec2(TILE.x - 16.0, 66.0));
                    ui.painter().rect(pic, 6.0, Color32::from_black_alpha(0x48), egui::Stroke::new(1.0, Color32::from_white_alpha(0x1C)), egui::StrokeKind::Inside);
                    let (tex, side) = thumbs[i].get_or_insert_with(|| texture(ui.ctx(), &format!("crosshair-save-{i}"), &s.look, egui::TextureOptions::LINEAR)).clone();
                    // 1:1 on whole screen pixels when it fits the box, else scaled down (smooth)
                    let ppp = ui.ctx().pixels_per_point();
                    let room = pic.width().min(pic.height());
                    let snap = side as f32 / ppp <= room;
                    draw_picture(ui, pic, &tex, side, room / side as f32, snap);
                    ui.painter().text(pos2(rect.center().x, rect.bottom() - 15.0), egui::Align2::CENTER_CENTER, &s.name, egui::FontId::proportional(13.0), if th::p().black && (same || resp.hovered()) { th::TEXT } else { th::text2() });
                    let x = Rect::from_center_size(pic.right_top() + vec2(-11.0, 11.0), vec2(18.0, 18.0));
                    let xr = ui.interact(x, ui.id().with(("ch-del", i)), Sense::click());
                    if resp.hovered() || xr.hovered() {
                        ui.painter().circle_filled(x.center(), 9.0, Color32::from_black_alpha(0xB0));
                        ui.painter().text(x.center(), egui::Align2::CENTER_CENTER, "×", egui::FontId::proportional(14.0), if xr.hovered() { th::p().accent } else { th::text2() });
                    }
                    if xr.clicked() {
                        remove = Some(i);
                    } else if resp.clicked() {
                        apply = Some(i);
                    }
                }
            });
        });
        if save_now {
            let name = self.name.trim().to_string();
            let name = if name.is_empty() { format!("Crosshair {}", self.saves.len() + 1) } else { name };
            self.saves.push(ch::Saved { name, look: c.clone() });
            self.name.clear();
            self.write_saves();
        }
        if let Some(i) = apply {
            c.apply_look(&self.saves[i].look);
        }
        if let Some(i) = remove {
            self.saves.remove(i);
            self.write_saves();
        }

        let c = c.clamped(); // the dot size snaps to the lines' odd or even here, not one frame later
        if c != before {
            ch::set(c.clone());
        }
        // saved once the pointer is up, not on every slider step
        if self.saved.as_ref() != Some(&c) && !ui.input(|i| i.pointer.any_down()) {
            let _ = self.engine.with_store(|s| c.save(s));
            self.saved = Some(c);
        }
    }
}

/// The overlay's picture as an egui texture (premultiplied, so it blends as the real window does).
fn texture(ctx: &egui::Context, name: &str, c: &Crosshair, opt: egui::TextureOptions) -> (TextureHandle, i32) {
    let (side, px) = ch::render_rgba(c);
    let img = egui::ColorImage::from_rgba_premultiplied([side as usize; 2], &px);
    (ctx.load_texture(name, img, opt), side)
}

/// Draws the buffer centred in `rect`. `fit` is points per buffer pixel (1/pixels_per_point = one buffer pixel
/// per screen pixel). With `snap` the corner sits on a screen pixel, so nothing is filtered.
fn draw_picture(ui: &Ui, rect: Rect, tex: &TextureHandle, side: i32, fit: f32, snap: bool) {
    let ppp = ui.ctx().pixels_per_point();
    let size = Vec2::splat(side as f32) * if snap { 1.0 / ppp } else { fit };
    let mut min = rect.center() - size / 2.0;
    if snap {
        min = pos2((min.x * ppp).round() / ppp, (min.y * ppp).round() / ppp);
    }
    ui.painter().with_clip_rect(rect).image(tex.id(), Rect::from_min_size(min, size), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
}

const TEXT_W: f32 = 190.0;
const VALUE_W: f32 = 56.0;
const ROW_H: f32 = 44.0;

/// One settings row: name with a dim explanation under it, `control` over the rest of the width.
fn row_body(ui: &mut Ui, name: &str, desc: &str, control: impl FnOnce(&mut Ui)) {
    ui.allocate_ui_with_layout(vec2(ui.available_width(), ROW_H), Layout::left_to_right(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let (r, _) = ui.allocate_exact_size(vec2(TEXT_W, ROW_H), Sense::hover());
        let p = ui.painter();
        if desc.is_empty() {
            p.text(r.left_center(), egui::Align2::LEFT_CENTER, name, FontId::proportional(14.0), th::TEXT);
        } else {
            p.text(pos2(r.left(), r.center().y), egui::Align2::LEFT_BOTTOM, name, FontId::proportional(14.0), th::TEXT);
            p.text(pos2(r.left(), r.center().y + 2.0), egui::Align2::LEFT_TOP, desc, FontId::proportional(12.0), th::text3());
        }
        control(ui);
    });
}

/// `row_body` under a hairline.
fn row(ui: &mut Ui, name: &str, desc: &str, control: impl FnOnce(&mut Ui)) {
    let y = ui.cursor().top();
    let x = ui.max_rect().x_range();
    ui.painter().hline(x, y, egui::Stroke::new(th::px(), Color32::from_white_alpha(0x1C)));
    row_body(ui, name, desc, control);
}

/// Bare dropdown whose arrow ends on the row's right edge, in line with the switches and values.
fn end_dropdown(ui: &mut Ui, id: &str, value: &str, rows: &[(String, bool)]) -> Option<usize> {
    // the field overhangs the row by its inset right of the arrow; a child Ui, so the overhang cannot widen the row
    let r = ui.available_rect_before_wrap();
    let r = Rect::from_min_max(r.min, pos2(r.right() + 13.0, r.bottom()));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r).layout(Layout::right_to_left(Align::Center)));
    w::dropdown_text(&mut child, id, value, rows)
}

/// A `th::button` exactly `width` wide, so a grid of them lines up.
fn even_button(ui: &mut Ui, text: &str, width: f32) -> egui::Response {
    let tw = ui.painter().layout_no_wrap(text.to_string(), FontId::new(14.0, th::medium()), th::TEXT).size().x;
    let pad = ((width - tw) / 2.0).max(2.0);
    th::button(ui, text, false, (pad, th::BTN.1, pad, th::BTN.3))
}

/// The on/off row of a tab: first under the tabs, so no hairline.
fn switch_first(ui: &mut Ui, name: &str, desc: &str, on: &mut bool) {
    row_body(ui, name, desc, |ui| switch_end(ui, on));
}

fn switch_row(ui: &mut Ui, name: &str, desc: &str, on: &mut bool) {
    row(ui, name, desc, |ui| switch_end(ui, on));
}

fn switch_end(ui: &mut Ui, on: &mut bool) {
    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if th::switch(ui, *on).clicked() {
            *on = !*on;
        }
    });
}

fn pick_row(ui: &mut Ui, id: &str, name: &str, desc: &str, options: &[&str], at: usize) -> Option<usize> {
    let mut out = None;
    row(ui, name, desc, |ui| {
        let rows: Vec<(String, bool)> = options.iter().enumerate().map(|(i, o)| (o.to_string(), i == at)).collect();
        out = end_dropdown(ui, id, options[at], &rows);
    });
    out
}

fn num_row(ui: &mut Ui, name: &str, desc: &str, v: &mut i32, min: i32, max: i32, unit: &str) {
    row(ui, name, desc, |ui| {
        let mut f = *v as f64;
        let width = (ui.available_width() - VALUE_W).max(60.0);
        // a switched-off tab's Ui already fades the slider, so it is not told to fade again
        if w::slider(ui, egui::Id::new(("ch-num", name, desc)), &mut f, min as f64, max as f64, width, true).changed && ui.is_enabled() {
            *v = f.round() as i32;
        }
        let (r, _) = ui.allocate_exact_size(vec2(VALUE_W, 20.0), Sense::hover());
        ui.painter().text(r.right_center(), egui::Align2::RIGHT_CENTER, format!("{}{unit}", *v), FontId::proportional(14.0), th::text2());
    });
}

fn hex(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    (s.len() == 6 && s.is_ascii()).then(|| u32::from_str_radix(s, 16).ok()).flatten().map(|n| [(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

/// Colour: swatch and hex field at the row's right edge.
fn colour_row(ui: &mut Ui, key: &str, desc: &str, rgb: &mut [u8; 3]) {
    row(ui, "Color", desc, |ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let k = egui::Id::new(("ch-hex", key));
            let mut buf: String = ui.data(|d| d.get_temp(k)).unwrap_or_else(|| hex(*rgb));
            let r = ui.add(egui::TextEdit::singleline(&mut buf).desired_width(70.0).char_limit(7));
            if r.changed() {
                if let Some(v) = parse_hex(&buf) {
                    *rgb = v;
                }
            }
            if !r.has_focus() {
                buf = hex(*rgb); // follows the swatch and presets while not being typed in
            }
            ui.data_mut(|d| d.insert_temp(k, buf));
            ui.add_space(8.0);
            ui.spacing_mut().interact_size = vec2(32.0, 22.0);
            // the hex field is already drawn this frame: one more frame shows the new value
            if ui.color_edit_button_srgb(rgb).changed() {
                ui.ctx().request_repaint();
            }
        });
    });
}

fn lines_tab(ui: &mut Ui, c: &mut Crosshair) {
    switch_first(ui, "Show lines", "The arms of the crosshair", &mut c.lines_on);
    ui.add_enabled_ui(c.lines_on, |ui| {
        colour_row(ui, "lines", "Color of the lines", &mut c.color);
        num_row(ui, "Opacity", "Lower is more see-through", &mut c.opacity, 10, 100, "%");
        if let Some(i) = pick_row(ui, "crosshair-style", "Line ends", "Square or rounded", &["Rectangle", "Round ends"], c.round as usize) {
            c.round = i == 1;
        }
        num_row(ui, "Number of lines", "Spread evenly around center", &mut c.arms, 1, 8, "");
        let mut up = !c.hide_top;
        switch_row(ui, "Top line", "Turn off for a T shape", &mut up);
        c.hide_top = !up;
        num_row(ui, "Length", "How long each line is", &mut c.size, 2, 80, " px");
        num_row(ui, "Thickness", "How wide each line is", &mut c.thickness, 1, 10, " px");
        num_row(ui, "Center gap", "Empty space in the middle", &mut c.gap, 0, 40, " px");
        num_row(ui, "Rotation", "Turns the lines", &mut c.rotation, 0, 359, "\u{B0}");
        num_row(ui, "Blur", "Softens the line edges", &mut c.blur, 0, 8, " px");
    });
}

fn dot_tab(ui: &mut Ui, c: &mut Crosshair) {
    switch_first(ui, "Show center dot", "A dot on the exact center", &mut c.dot);
    ui.add_enabled_ui(c.dot, |ui| {
        colour_row(ui, "dot", "Color of the dot", &mut c.dot_color);
        num_row(ui, "Opacity", "Lower lets more show through", &mut c.dot_opacity, 10, 100, "%");
        if let Some(i) = pick_row(ui, "crosshair-dot-shape", "Shape", "Square or circle", &["Square", "Circle"], c.dot_circle as usize) {
            c.dot_circle = i == 1;
        }
        num_row(ui, "Size", "Width of the dot", &mut c.dot_size, 1, 40, " px");
        num_row(ui, "Blur", "Softens the dot's edge", &mut c.dot_blur, 0, 8, " px");
    });
}

fn outline_tab(ui: &mut Ui, c: &mut Crosshair) {
    switch_first(ui, "Show outline", "Stands out on bright scenes", &mut c.outline);
    ui.add_enabled_ui(c.outline, |ui| {
        colour_row(ui, "outline", "Color of the outline", &mut c.outline_color);
        num_row(ui, "Opacity", "Lower makes the edge fainter", &mut c.outline_opacity, 10, 100, "%");
        num_row(ui, "Thickness", "Width around lines and dot", &mut c.outline_thickness, 1, 5, " px");
        num_row(ui, "Blur", "Softens the outline", &mut c.outline_blur, 0, 8, " px");
    });
}
