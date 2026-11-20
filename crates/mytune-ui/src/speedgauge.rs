//! The speed test's gauge: a round Start button that opens into a 270 degree dial. Painted by hand; every motion
//! is a spring from `theme`, so nothing repaints once the needle rests.

use crate::theme::{self as th, text2, text3};
use eframe::egui::{self, pos2, vec2, Align2, Color32, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui};

/// Download and upload tones.
pub const DOWN: Color32 = Color32::from_rgb(0x5A, 0xD1, 0xE6);
pub const UP: Color32 = Color32::from_rgb(0xA7, 0x8B, 0xFA);

/// Dial labels in Mbit/s, evenly spaced round the arc, so slow lines still move the needle.
pub const TICKS: [f64; 9] = [0.0, 5.0, 10.0, 50.0, 100.0, 250.0, 500.0, 750.0, 1000.0];

/// Where `mbps` sits on the dial, 0..1: straight lines between neighbouring ticks.
pub fn frac(mbps: f64) -> f32 {
    let v = if mbps.is_nan() { 0.0 } else { mbps.clamp(0.0, 1000.0) };
    let i = TICKS.iter().rposition(|t| *t <= v).unwrap_or(0).min(TICKS.len() - 2);
    ((i as f64 + (v - TICKS[i]) / (TICKS[i + 1] - TICKS[i])) / (TICKS.len() - 1) as f64) as f32
}

pub struct Gauge<'a> {
    /// Running or showing a result: the dial is open. Else the Start button shows.
    pub open: bool,
    /// Value the needle follows, Mbit/s.
    pub value: f64,
    /// Colour of the arc behind the needle and of the phase label.
    pub color: Color32,
    pub phase: &'a str,
    /// `ctx` time the test started at, for the needle's one sweep to the end and back.
    pub started: Option<f64>,
}

const R_BTN: f32 = 44.0;
const R_DIAL: f32 = 112.0;
const SWEEP: f32 = 0.45; // seconds the needle heads for the end, then as long back, before it follows the value

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Screen angle of dial position `f`: from lower left, over the top, to lower right.
fn angle(f: f32) -> f32 {
    (135.0 + 270.0 * f).to_radians()
}

fn at(c: Pos2, r: f32, a: f32) -> Pos2 {
    c + vec2(a.cos(), a.sin()) * r
}

fn arc(c: Pos2, r: f32, f0: f32, f1: f32, stroke: Stroke) -> Shape {
    let n = (((f1 - f0) * 90.0).ceil() as usize).max(1);
    Shape::line((0..=n).map(|i| at(c, r, angle(lerp(f0, f1, i as f32 / n as f32)))).collect(), stroke)
}

/// Draws the gauge at the cursor, centred. True when the Start button was clicked.
pub fn show(ui: &mut Ui, g: &Gauge) -> bool {
    let ctx = ui.ctx().clone();
    let id = ui.scope_id().with("speedgauge");
    let o = th::anim(&ctx, id.with("open"), g.open, 0.5);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), lerp(R_BTN * 2.0 + 16.0, R_DIAL + 100.0, o)), Sense::hover());
    let c = pos2(rect.center().x, rect.top() + lerp(R_BTN + 8.0, R_DIAL + 6.0, o));
    let r = lerp(R_BTN, R_DIAL, o);
    let paint = ui.painter().clone();

    // The Start button. It is also the needle's hub: it shrinks to it as the dial opens.
    let mut clicked = false;
    let (mut hub, mut hub_r, mut lit) = (c, lerp(R_BTN, 7.0, o), 0.0);
    if !g.open {
        let mut resp = ui.interact(Rect::from_center_size(c, vec2(r, r) * 2.0), id.with("start"), Sense::click_and_drag());
        let off = th::pull(ui, &mut resp);
        let down = th::anim(&ctx, resp.id.with("down"), resp.is_pointer_button_down_on(), th::PRESS);
        lit = th::anim(&ctx, resp.id.with("hover"), resp.hovered(), th::HOVER) * 14.0 - down * 8.0;
        hub += off / 2.0;
        hub_r *= 1.0 - 0.04 * down;
        clicked = resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
    }
    let draw_hub = |paint: &egui::Painter| {
        let ring = Stroke::new(2.0 * (1.0 - o), Color32::from_white_alpha((176.0 * (1.0 - o)) as u8));
        paint.circle(hub, hub_r, Color32::from_white_alpha(lerp(20.0 + lit, 255.0, o) as u8), ring);
        if o < 0.99 {
            paint.text(hub, Align2::CENTER_CENTER, "Start", FontId::new(15.0, th::semibold()), th::TEXT.gamma_multiply(1.0 - o));
        }
    };
    if o < 0.01 {
        draw_hub(&paint);
        return clicked;
    }

    // Needle: once to the end and back when a test starts, then after the value.
    let since = g.started.map(|t| (ctx.input(|i| i.time) - t) as f32);
    let target = match since {
        Some(s) if s < SWEEP => 1.0,
        Some(s) if s < SWEEP * 2.0 => 0.0,
        _ => frac(g.value),
    };
    if since.is_some_and(|s| s < SWEEP * 2.0) {
        ctx.request_repaint(); // the target changes by the clock here; the spring alone may be at rest
    }
    let f = th::spring(&ctx, id.with("needle"), target, 0.45, 0.85).clamp(0.0, 1.0);

    let track = Stroke::new(8.0, Color32::from_white_alpha((0x24 as f32 * o) as u8));
    paint.add(arc(c, r, 0.0, o, track));
    let f_arc = f.min(o);
    if f_arc > 0.002 {
        paint.add(arc(c, r, 0.0, f_arc, Stroke::new(8.0, g.color.gamma_multiply(o))));
    }
    let label = text3().gamma_multiply(o);
    for (i, t) in TICKS.iter().enumerate() {
        let tf = i as f32 / (TICKS.len() - 1) as f32;
        if tf > o + 0.001 {
            break;
        }
        let a = angle(tf);
        paint.line_segment([at(c, r - 9.0, a), at(c, r - 13.0, a)], Stroke::new(1.5, label));
        paint.text(at(c, r - 27.0, a), Align2::CENTER_CENTER, format!("{t:.0}"), FontId::new(10.5, th::medium()), label);
    }
    let white = th::TEXT.gamma_multiply(o);
    paint.line_segment([c, at(c, r - 18.0, angle(f))], Stroke::new(2.5, white));
    draw_hub(&paint);

    let num = if g.value >= 100.0 { format!("{:.0}", g.value) } else { format!("{:.1}", g.value) };
    paint.text(c + vec2(0.0, 36.0), Align2::CENTER_CENTER, num, FontId::new(30.0, th::bold()), white);
    paint.text(c + vec2(0.0, 60.0), Align2::CENTER_CENTER, "Mbit/s", FontId::new(11.0, th::medium()), text2().gamma_multiply(o));
    paint.text(c + vec2(0.0, 77.0), Align2::CENTER_CENTER, g.phase, FontId::new(12.0, th::semibold()), g.color.gamma_multiply(o));
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_scale() {
        assert_eq!(frac(0.0), 0.0);
        assert_eq!(frac(1000.0), 1.0);
        assert_eq!(frac(5000.0), 1.0);
        assert_eq!(frac(-3.0), 0.0);
        assert_eq!(frac(f64::NAN), 0.0);
        assert!((frac(10.0) - 2.0 / 8.0).abs() < 1e-6);
        assert!((frac(30.0) - 2.5 / 8.0).abs() < 1e-6);
        for (i, t) in TICKS.iter().enumerate() {
            assert!((frac(*t) - i as f32 / 8.0).abs() < 1e-6);
        }
        let mut last = -1.0;
        for v in 0..=2000 {
            let f = frac(v as f64 * 0.5);
            assert!(f > last, "not rising at {v}");
            last = f;
        }
    }
}
