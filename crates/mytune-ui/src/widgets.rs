//! Widgets of `Themes/Fluent.xaml` that `theme.rs` does not have yet: SubtleButton, DropButton + PickList
//! with RowButton rows, and the square Slider. Sizes, paddings and colours are copied from the XAML.

use crate::theme::{self as th, control_border, control_bg, ico, rt, sb, text2, TEXT};
use eframe::egui::{self, pos2, vec2, Color32, FontId, Id, Order, Rect, Sense, Stroke, StrokeKind, Ui};

fn white(a: u8) -> Color32 {
    Color32::from_white_alpha(a)
}

/// `SubtleButton`: transparent, hover = white 7 %. `selected` is not a style here (the XAML swaps in AccentButton).
pub fn subtle_button(ui: &mut Ui, text: &str, size: f32, pad: (f32, f32, f32, f32)) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::proportional(size), TEXT);
    let dim = vec2(galley.size().x + pad.0 + pad.2, galley.size().y + pad.1 + pad.3);
    let (rect, resp) = ui.allocate_exact_size(dim, Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, rect.height() / 2.0, white(18)); // 0.07 * 255
    }
    let col = if resp.is_pointer_button_down_on() { text2() } else { TEXT };
    ui.painter().galley(pos2(rect.left() + pad.0, rect.top() + pad.1), galley, col);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

const FIELD_PAD: (f32, f32) = (12.0, 7.0); // left/right, top (bottom 8)
const ROW_H_PAD: (f32, f32, f32) = (12.0, 6.0, 7.0); // left/right, top, bottom

/// `DropButton` field with its `PickList` popup. `rows` = (label, picked). Returns the clicked row.
/// The field is `width` wide; the list opens 4 px below it and closes on any outside click.
pub fn dropdown(ui: &mut Ui, id: &str, width: f32, value: &str, rows: &[(String, bool)]) -> Option<usize> {
    let pal = th::p();
    let ctx = ui.ctx().clone();
    let key = Id::new(("dropdown", id));
    let mut open = ctx.data(|d| d.get_temp::<bool>(key)).unwrap_or(false);

    let text_h = ui.painter().layout_no_wrap("Ag".into(), FontId::proportional(14.0), TEXT).size().y;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, text_h + FIELD_PAD.1 + 8.0 + 2.0), Sense::click());
    let paint = ui.painter();
    paint.rect_filled(rect, 12.0, if resp.hovered() { white(0x22) } else { control_bg() });
    paint.rect_stroke(rect, 12.0, Stroke::new(1.0, if open { pal.accent } else { control_border() }), StrokeKind::Inside);
    paint.text(pos2(rect.left() + 1.0 + FIELD_PAD.0, rect.center().y), egui::Align2::LEFT_CENTER, value, FontId::proportional(14.0), TEXT);
    paint.text(pos2(rect.right() - 1.0 - FIELD_PAD.0, rect.center().y), egui::Align2::RIGHT_CENTER, "\u{E70D}", FontId::new(10.0, th::icons()), text2());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);

    let mut picked = None;
    let mut list_rect = Rect::NOTHING;
    if open {
        let area = egui::Area::new(key.with("list")).order(Order::Foreground).fixed_pos(pos2(rect.left(), rect.bottom() + 4.0)).show(&ctx, |ui| {
            egui::Frame::new().corner_radius(14).fill(pal.card_hover).stroke(Stroke::new(1.0, white(0x40))).inner_margin(4).show(ui, |ui| {
                ui.set_width(width - 10.0);
                ui.spacing_mut().item_spacing.y = 1.0;
                egui::ScrollArea::vertical().max_height(264.0).auto_shrink([false, true]).show(ui, |ui| {
                    for (i, (label, sel)) in rows.iter().enumerate() {
                        if row_button(ui, label, *sel) {
                            picked = Some(i);
                        }
                    }
                });
            });
        });
        list_rect = area.response.rect;
    }
    let outside_click = open && ctx.input(|i| i.pointer.any_click()) && ctx.input(|i| i.pointer.interact_pos()).is_some_and(|p| !list_rect.contains(p) && !rect.contains(p));
    if picked.is_some() || outside_click {
        open = false;
    } else if resp.clicked() {
        open = !open;
    }
    ctx.data_mut(|d| d.insert_temp(key, open));
    picked
}

/// `RowButton` inside a pick list; the picked row has a 3x16 accent mark on the left and semibold text.
fn row_button(ui: &mut Ui, text: &str, selected: bool) -> bool {
    let rt_font = if selected { th::semibold() } else { egui::FontFamily::Proportional };
    let galley = ui.painter().layout_no_wrap(text.to_string(), FontId::new(14.0, rt_font), TEXT);
    let h = galley.size().y + ROW_H_PAD.1 + ROW_H_PAD.2;
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::click());
    let bg = if resp.is_pointer_button_down_on() { Some(white(0x0A)) } else if resp.hovered() { Some(white(0x14)) } else { None };
    if let Some(bg) = bg {
        ui.painter().rect_filled(rect, 10.0, bg);
    }
    ui.painter().galley(pos2(rect.left() + ROW_H_PAD.0, rect.top() + ROW_H_PAD.1), galley, TEXT);
    if selected {
        ui.painter().rect_filled(Rect::from_min_size(pos2(rect.left(), rect.center().y - 8.0), vec2(3.0, 16.0)), 1.5, th::p().accent);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

pub struct SliderOut {
    pub changed: bool,
    /// Pointer let go (or a click): time to save.
    pub released: bool,
}

/// `Slider` style: 6 px square track (accent left of the thumb, white 16 % right), 10x20 accent thumb with a
/// 2 px Ink outline that grows to 1.2x on hover, a click moves the thumb to the pointer. Disabled = 40 % opacity.
pub fn slider(ui: &mut Ui, id: Id, value: &mut f64, min: f64, max: f64, width: f32, enabled: bool) -> SliderOut {
    let pal = th::p();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 20.0), if enabled { Sense::click_and_drag() } else { Sense::hover() });
    let mut changed = false;
    if enabled && resp.is_pointer_button_down_on() {
        if let Some(p) = resp.interact_pointer_pos() {
            // the thumb centre travels over the track minus the thumb width
            let t = ((p.x - rect.left() - 5.0) / (rect.width() - 10.0)).clamp(0.0, 1.0) as f64;
            let v = ((min + t * (max - min)) * 100.0).round() / 100.0; // the C# row rounds to 2 decimals
            if v != *value {
                *value = v;
                changed = true;
            }
        }
    }
    let t = if max > min { ((*value - min) / (max - min)).clamp(0.0, 1.0) as f32 } else { 0.0 };
    let cx = rect.left() + 5.0 + t * (rect.width() - 10.0);
    let k = ui.ctx().animate_bool_with_time(id, enabled && resp.hovered(), 0.1);
    let op = if enabled { 1.0 } else { 0.4 };
    let fade = |c: Color32| c.gamma_multiply(op);
    let paint = ui.painter();
    let track = Rect::from_center_size(rect.center(), vec2(rect.width(), 6.0));
    paint.rect_filled(track, 3.0, fade(white(0x29)));
    paint.rect_filled(Rect::from_min_max(track.min, pos2(cx, track.max.y)), 3.0, fade(pal.accent));
    let scale = 1.0 + 0.2 * k;
    let thumb = Rect::from_center_size(pos2(cx, rect.center().y), vec2(10.0 * scale, 20.0 * scale));
    paint.rect_filled(thumb, thumb.width() / 2.0, fade(pal.accent));
    paint.rect_stroke(thumb, thumb.width() / 2.0, Stroke::new(2.0, fade(pal.ink)), StrokeKind::Inside);
    SliderOut { changed, released: enabled && (resp.drag_stopped() || resp.clicked()) }
}

/// One line of the colour cards: label column 150, slider, value column 64 right aligned, 14 below.
pub fn slider_row(ui: &mut Ui, label: &str, value: &mut f64, min: f64, max: f64, fmt: &dyn Fn(f64) -> String, enabled: bool) -> SliderOut {
    let id = Id::new(("slider", label));
    let mut out = SliderOut { changed: false, released: false };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let fade = if enabled { 1.0 } else { 0.4 };
        let (lr, _) = ui.allocate_exact_size(vec2(150.0, 20.0), Sense::hover());
        ui.painter().text(lr.left_center(), egui::Align2::LEFT_CENTER, label, FontId::proportional(14.0), TEXT.gamma_multiply(fade));
        let w = (ui.available_width() - 64.0).max(60.0);
        out = slider(ui, id, value, min, max, w, enabled);
        let (vr, _) = ui.allocate_exact_size(vec2(64.0, 20.0), Sense::hover());
        ui.painter().text(vr.right_center(), egui::Align2::RIGHT_CENTER, fmt(*value), FontId::proportional(14.0), text2().gamma_multiply(fade));
    });
    ui.add_space(14.0);
    out
}

/// Caption style: 12, TextSecondary.
pub fn caption(ui: &mut Ui, text: &str) {
    ui.label(rt(text, 12.0).color(text2()));
}

/// Tertiary caption.
pub fn caption3(ui: &mut Ui, text: &str) {
    ui.label(rt(text, 12.0).color(th::text3()));
}

pub fn semibold_label(ui: &mut Ui, text: &str, size: f32) {
    ui.label(sb(text, size));
}

/// Segoe icon glyph helper kept for pick lists that want one.
pub fn icon(ui: &mut Ui, glyph: &str, size: f32) {
    ui.label(ico(glyph, size));
}

/// `CheckBox` style: 20x20 square, 1 px border white 60 %, fill white 3 %; checked = accent with a tick.
/// `label` draws the text next to it (10 px gap); the whole row toggles. Returns true when toggled.
pub fn checkbox(ui: &mut Ui, id: Id, checked: &mut bool, label: impl FnOnce(&mut Ui)) -> bool {
    let pal = th::p();
    let row = ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let (rect, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
        let hover = ui.rect_contains_pointer(rect);
        let fade = if hover { 0.85 } else { 1.0 };
        let (fill, stroke) = if *checked { (pal.accent, pal.accent) } else { (white(0x08), white(0x9A)) };
        ui.painter().rect_filled(rect, 6.0, fill.gamma_multiply(fade));
        ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.0, stroke.gamma_multiply(fade)), StrokeKind::Inside);
        if *checked {
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "\u{E73E}", FontId::new(12.0, th::icons()), pal.on_accent);
        }
        ui.vertical(label);
    });
    let resp = ui.interact(row.response.rect, id, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
    if resp.clicked() {
        *checked = !*checked;
    }
    resp.clicked()
}
