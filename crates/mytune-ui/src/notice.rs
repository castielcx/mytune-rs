//! One coloured bar under the title bar for every page's status messages (user's choice 2026-11-16: more
//! visible than a line of text inside the page). Pages hand their `status` to `post`; `App::ui` calls `show`.

use crate::theme as th;
use eframe::egui::{self, pos2, vec2, Align2, Color32, Context, Id, Order, Rect, Sense};

const SHOWN: f64 = 6.0; // seconds a success stays
const SHOWN_ERR: f64 = 10.0;
const FADE: f64 = 0.2;

#[derive(Clone)]
struct Notice {
    text: String,
    ok: bool,
    at: f64,
}

fn id() -> Id { Id::new("mytune-notice") }

/// Status for a switch (user's choice 2026-11-16): turned on is green, turned off is red, a failure is red
/// with the error badge and the reason.
pub fn toggled(name: &str, on: bool, err: Option<&str>) -> (String, bool) {
    let verb = if on { "on" } else { "off" };
    match err {
        None => (format!("{SUCCESS} {verb} {name}"), on),
        Some(e) => (format!("Unable to turn {verb} {name}: {e}"), false),
    }
}
const SUCCESS: &str = "Successfully turned";

/// Shows `text` in the bar, replacing the one there.
pub fn post(ctx: &Context, text: String, ok: bool) {
    let at = ctx.input(|i| i.time);
    ctx.data_mut(|d| d.insert_temp(id(), Some(Notice { text, ok, at })));
}

/// Draws the bar `top` px below the window's top edge, full width; a click closes it.
pub fn show(ctx: &Context, top: f32) {
    let Some(n) = ctx.data(|d| d.get_temp::<Option<Notice>>(id())).flatten() else { return };
    let age = ctx.input(|i| i.time) - n.at;
    let life = if n.ok { SHOWN } else { SHOWN_ERR };
    if age >= life {
        ctx.data_mut(|d| d.remove::<Option<Notice>>(id()));
        return;
    }
    let alpha = (age / FADE).min((life - age) / FADE).clamp(0.0, 1.0) as f32;
    let full = ctx.content_rect();
    let rect = Rect::from_min_size(pos2(full.left(), full.top() + top), vec2(full.width(), 36.0));
    let fill = if n.ok { Color32::from_rgb(0x1F, 0x7A, 0x34) } else { Color32::from_rgb(0xC4, 0x2B, 0x1C) };
    let closed = egui::Area::new(id()).order(Order::Foreground).fixed_pos(rect.min).show(ctx, |ui| {
        let resp = ui.allocate_rect(rect, Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand);
        let p = ui.painter();
        p.rect_filled(rect, 0.0, fill.gamma_multiply(alpha));
        let white = Color32::WHITE.gamma_multiply(alpha);
        // check mark, also on the red "turned off" bar; error badge for failures
        let glyph = if n.ok || n.text.starts_with(SUCCESS) { "\u{E73E}" } else { "\u{E783}" };
        let font = |s: f32, f: egui::FontFamily| egui::FontId::new(s, f);
        let text_w = p.layout_no_wrap(n.text.clone(), font(13.0, th::semibold()), white).size().x;
        let x = rect.center().x - (text_w + 24.0) / 2.0;
        p.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, glyph, font(14.0, th::icons()), white);
        p.text(pos2(x + 24.0, rect.center().y), Align2::LEFT_CENTER, &n.text, font(13.0, th::semibold()), white);
        p.text(pos2(rect.right() - 18.0, rect.center().y), Align2::CENTER_CENTER, "\u{E711}", font(11.0, th::icons()), white);
        resp.clicked()
    });
    if closed.inner {
        ctx.data_mut(|d| d.remove::<Option<Notice>>(id()));
    } else if alpha < 1.0 {
        ctx.request_repaint();
    } else {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((life - FADE - age).max(0.0)));
    }
}

