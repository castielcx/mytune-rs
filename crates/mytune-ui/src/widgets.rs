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
    let (rect, mut resp) = ui.allocate_exact_size(dim, Sense::click_and_drag());
    let off = th::pull(ui, &mut resp);
    let hov = th::anim(ui.ctx(), resp.id.with("hover"), resp.hovered() || off != egui::Vec2::ZERO, th::HOVER);
    if hov > 0.0 {
        ui.painter().rect_filled(th::pulled(rect, off), th::R_CTL, white((20.0 * hov) as u8));
    }
    let col = if resp.is_pointer_button_down_on() { text2() } else { TEXT };
    ui.painter().galley(pos2(rect.left() + pad.0, rect.top() + pad.1) + off / 2.0, galley, col);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

const FIELD_PAD: (f32, f32) = (12.0, 7.0); // left/right, top (bottom 8)
const ROW_H_PAD: (f32, f32, f32) = (12.0, 6.0, 7.0); // left/right, top, bottom

/// `DropButton` field with its `PickList` popup. `rows` = (label, picked). Returns the clicked row.
/// The field is `width` wide; the list opens 4 px below it (above when there is no room below), stays inside
/// the window's right edge and closes on any outside click.
pub fn dropdown(ui: &mut Ui, id: &str, width: f32, value: &str, rows: &[(String, bool)]) -> Option<usize> {
    dropdown_styled(ui, id, width, value, rows, false)
}

/// The same pick list behind bare text with a small arrow: no glass field. As wide as the text needs.
pub fn dropdown_text(ui: &mut Ui, id: &str, value: &str, rows: &[(String, bool)]) -> Option<usize> {
    let width = ui.painter().layout_no_wrap(value.to_string(), FontId::proportional(14.0), TEXT).size().x + 2.0 * FIELD_PAD.0 + 2.0 + 16.0;
    dropdown_styled(ui, id, width, value, rows, true)
}

fn dropdown_styled(ui: &mut Ui, id: &str, width: f32, value: &str, rows: &[(String, bool)], bare: bool) -> Option<usize> {
    let pal = th::p();
    let ctx = ui.ctx().clone();
    let key = Id::new(("dropdown", id));
    let mut open = ctx.data(|d| d.get_temp::<bool>(key)).unwrap_or(false);

    let text_h = ui.painter().layout_no_wrap("Ag".into(), FontId::proportional(14.0), TEXT).size().y;
    let (rect, resp) = ui.allocate_exact_size(vec2(width, text_h + FIELD_PAD.1 + 8.0 + 2.0), Sense::click());
    // as wide as the field, or wider when a row's text needs it (long device names)
    let longest = rows.iter().map(|(l, _)| ui.painter().layout_no_wrap(l.clone(), FontId::new(14.0, th::semibold()), TEXT).size().x).fold(0.0, f32::max);
    let list_w = (width - 10.0).max(longest + 2.0 * ROW_H_PAD.0);
    let list_h = (rows.len() as f32 * (text_h + ROW_H_PAD.1 + ROW_H_PAD.2 + 1.0) + 8.0).min(272.0);
    let screen = ctx.content_rect(); // this window
    let up = rect.bottom() + 4.0 + list_h > screen.bottom() && rect.top() - screen.top() > screen.bottom() - rect.bottom();
    let paint = ui.painter();
    if !bare {
        let hov = th::anim(&ctx, key.with("hover"), resp.hovered() || open, th::HOVER);
        th::plate(paint, rect, th::R_CTL, white(0x1A + (14.0 * hov) as u8), 0x34);
        if open {
            paint.rect_stroke(rect, th::R_CTL, Stroke::new(1.0, pal.accent), StrokeKind::Inside);
        }
    }
    let ink = if !bare || open || resp.hovered() { TEXT } else { text2() };
    paint.text(pos2(rect.right() - 1.0 - FIELD_PAD.0, rect.center().y + 1.5), egui::Align2::RIGHT_CENTER, if up { "\u{E70E}" } else { "\u{E70D}" }, FontId::new(10.0, th::icons()), if bare { ink } else { text2() });
    paint.text(pos2(rect.left() + 1.0 + FIELD_PAD.0, rect.center().y), egui::Align2::LEFT_CENTER, value, FontId::proportional(14.0), ink);
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);

    let mut picked = None;
    let mut list_rect = Rect::NOTHING;
    if open {
        let x = rect.left().min(screen.right() - list_w - 14.0).max(screen.left() + 4.0);
        let area = egui::Area::new(key.with("list")).order(Order::Foreground);
        let area = if up { area.pivot(egui::Align2::LEFT_BOTTOM).fixed_pos(pos2(x, rect.top() - 4.0)) } else { area.fixed_pos(pos2(x, rect.bottom() + 4.0)) };
        let area = area.show(&ctx, |ui| {
            th::float(ui, th::R_MENU, 4, |ui| {
                ui.set_width(list_w);
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
        ui.painter().rect_filled(rect, th::nested(th::R_MENU, 4.0), bg);
    }
    ui.painter().galley_with_override_text_color(pos2(rect.left() + ROW_H_PAD.0, rect.top() + ROW_H_PAD.1), galley, th::hover_text(TEXT, selected || resp.hovered()));
    if selected && !th::p().black {
        ui.painter().rect_filled(Rect::from_min_size(pos2(rect.left(), rect.center().y - 8.0), vec2(3.0, 16.0)), 1.5, th::p().accent);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

pub struct SliderOut {
    pub changed: bool,
    /// Pointer let go (or a click): time to save.
    pub released: bool,
}

/// Slider: a 4 px track (accent left of the thumb, white 16 % right) and a round 18 px thumb that grows a
/// little under the pointer and more while held; a click moves the thumb to the pointer. Disabled = 40 % opacity.
pub fn slider(ui: &mut Ui, id: Id, value: &mut f64, min: f64, max: f64, width: f32, enabled: bool) -> SliderOut {
    slider_sized(ui, id, value, min, max, width, enabled, false)
}

/// `slider`; `thin` is the volume flyout's: a 3 px track and a round 12 px thumb.
pub fn slider_sized(ui: &mut Ui, id: Id, value: &mut f64, min: f64, max: f64, width: f32, enabled: bool, thin: bool) -> SliderOut {
    // half the thumb's width (its centre travels over the track minus the thumb), track height, thumb size and
    // how much it grows while held
    let (half, track_h, thumb_d, swell) = if thin { (6.0, 3.0, 12.0, 2.0) } else { (9.0, 4.0, 18.0, 4.0) };
    let pal = th::p();
    // never narrower than its thumb: egui stops the app on a negative size, and a caller with no room left
    // would ask for one
    let (rect, resp) = ui.allocate_exact_size(vec2(width.max(2.0 * half), 20.0), if enabled { Sense::click_and_drag() } else { Sense::hover() });
    let mut changed = false;
    if enabled && resp.is_pointer_button_down_on() {
        if let Some(p) = resp.interact_pointer_pos() {
            // the thumb centre travels over the track minus the thumb width
            let t = ((p.x - rect.left() - half) / (rect.width() - 2.0 * half)).clamp(0.0, 1.0) as f64;
            let v = ((min + t * (max - min)) * 100.0).round() / 100.0; // the C# row rounds to 2 decimals
            if v != *value {
                *value = v;
                changed = true;
            }
        }
    }
    let t = if max > min { ((*value - min) / (max - min)).clamp(0.0, 1.0) as f32 } else { 0.0 };
    let cx = rect.left() + half + t * (rect.width() - 2.0 * half);
    let k = th::anim(ui.ctx(), id.with("hover"), enabled && resp.hovered(), th::HOVER);
    // held, the thumb swells on a spring that runs a little past and settles
    let held = th::spring(ui.ctx(), id.with("held"), if enabled && resp.is_pointer_button_down_on() { 1.0 } else { 0.0 }, 0.26, 0.6);
    let op = if enabled { 1.0 } else { 0.4 };
    let fade = |c: Color32| c.gamma_multiply(op);
    let paint = ui.painter();
    let track = Rect::from_center_size(rect.center(), vec2(rect.width(), track_h));
    paint.rect_filled(track, track_h / 2.0, fade(white(0x29)));
    paint.rect_filled(Rect::from_min_max(track.min, pos2(cx, track.max.y)), track_h / 2.0, fade(pal.accent));
    let thumb = Rect::from_center_size(pos2(cx, rect.center().y), egui::Vec2::splat(thumb_d + 1.0 * k + swell * held));
    paint.add(egui::Shadow { offset: [0, 1], blur: 5, spread: 0, color: fade(Color32::from_black_alpha(96)) }.as_shape(thumb, egui::CornerRadius::from(thumb.height() / 2.0)));
    // held, the thumb turns to clear glass: the track under it shows through, magnified and bent at the rim
    let glass = held.clamp(0.0, 1.0);
    if glass > 0.01 {
        paint.add(crate::lens::shape(thumb, crate::lens::Lens { radius: thumb.height() / 2.0, rim: thumb.height() * 0.45, bend: 5.0 * glass, zoom: 1.0 + 0.25 * glass, blur: 0.0, split: 0.3 }));
    }
    th::capsule(paint, thumb, fade(if pal.black { Color32::WHITE } else { Color32::WHITE.gamma_multiply(1.0 - 0.8 * glass) }), 0x60);
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
        let (fill, stroke) = if *checked { (pal.accent, pal.accent) } else { (white(0x10), white(0x70)) };
        if !th::bare_check(ui.painter(), rect.center(), *checked) {
            ui.painter().rect_filled(rect, 6.0, fill.gamma_multiply(fade));
            ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.0, stroke.gamma_multiply(fade)), StrokeKind::Inside);
        }
        if *checked && !pal.black {
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
