//! Home's storage card: one bar for the system drive, a segment per category (`mytune_core::diskuse`).
//! The card only reads the last result; measuring runs on diskuse's background thread.

use crate::theme::{self as th, dsb, rt, text2};
use eframe::egui::{pos2, vec2, Align, Color32, CornerRadius, Id, Layout, Rect, Sense, Ui, UiBuilder};
use mytune_core::diskuse::{self, NAMES};
use mytune_core::engine::Engine;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// One flat colour per category in `NAMES` order, then Other. The only hues on the card.
const COLORS: [Color32; 9] = [
    Color32::from_rgb(0x6E, 0x8B, 0xE8), // Games
    Color32::from_rgb(0xD4, 0x8A, 0x5E), // Apps
    Color32::from_rgb(0x5F, 0xB3, 0xB0), // Windows
    Color32::from_rgb(0xDD, 0xBF, 0x6A), // Documents
    Color32::from_rgb(0xC9, 0x7B, 0xB2), // Pictures
    Color32::from_rgb(0x9A, 0x7F, 0xD6), // Videos
    Color32::from_rgb(0xD4, 0x6A, 0x6A), // Music
    Color32::from_rgb(0x7D, 0xB8, 0x7A), // Downloads
    Color32::from_rgb(0x8A, 0x8F, 0x98), // Other
];

/// Sizes as Explorer shows them (1 GB = 1024^3 bytes).
fn size(bytes: u64) -> String {
    let gb = bytes as f64 / 1_073_741_824.0;
    if gb >= 100.0 { format!("{gb:.0} GB") } else if gb >= 1.0 { format!("{gb:.1} GB") } else { format!("{:.0} MB", gb * 1024.0) }
}

fn ago(at: u64) -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()).saturating_sub(at);
    let n = |v: u64, unit: &str| format!("{v} {unit}{} ago", if v == 1 { "" } else { "s" });
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => n(secs / 60, "minute"),
        3600..=172_799 => n(secs / 3600, "hour"),
        _ => n(secs / 86_400, "day"),
    }
}

/// `shown`: the window is on screen (an automatic measurement only starts then).
pub fn ui(ui: &mut Ui, engine: &Engine, shown: bool) {
    let ctx = ui.ctx().clone();
    let begin = |auto: bool| {
        let (games, ctx) = (engine.with_store(|s| diskuse::game_stems(s)), ctx.clone());
        diskuse::start(auto, games, move || ctx.request_repaint());
    };
    // once a minute at most: diskuse's thread decides whether a scan is due (older than a day, no game running)
    static ASKED: Mutex<Option<Instant>> = Mutex::new(None);
    if shown {
        let mut asked = ASKED.lock().unwrap();
        if asked.is_none_or(|t| t.elapsed() > Duration::from_secs(60)) {
            *asked = Some(Instant::now());
            begin(true);
        }
    }
    let snap = diskuse::snapshot();
    let u = snap.usage.unwrap_or_default();
    let measured = u.at != 0;
    // (name, bytes, colour): the categories, then the rest of the used space (all of it before the first scan)
    let mut items: Vec<(&str, u64, Color32)> = NAMES.iter().zip(u.cats).zip(COLORS).map(|((n, b), c)| (*n, b, c)).collect();
    items.push((if measured { "Other" } else { "Used" }, u.other(), COLORS[8]));

    let open_id = Id::new("disk-legend");
    let mut open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
    th::card(ui, (13, 16, 16, 16), |ui| {
        ui.horizontal(|ui| {
            let avail = ui.available_width();
            ui.vertical(|ui| {
                ui.set_max_width((avail - 220.0).max(120.0));
                // shortcut: Explorer's name for an unnamed drive; read the volume label when a named system drive matters
                ui.label(dsb(if u.total == 0 { "Storage".to_string() } else { format!("Local Disk ({}:)", u.drive) }, 19.0));
                ui.add_space(3.0);
                let scanning = snap.scanning || !measured;
                let sub = if u.total == 0 { "Measuring\u{2026}".to_string() } else { format!("{} available of {}{}", size(u.free), size(u.total), if scanning { ", measuring\u{2026}" } else { "" }) };
                let line = ui.label(rt(sub, 13.0).color(Color32::from_white_alpha(0xD9)));
                if !scanning {
                    line.on_hover_text(format!("Measured {}", ago(u.at)));
                }
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if !snap.scanning && th::button(ui, "Refresh", false, th::BTN).clicked() {
                    begin(false);
                }
                if th::subtle_button(ui, "Categories", th::BTN).on_hover_text("What the colours stand for").clicked() {
                    open = !open;
                    ui.data_mut(|d| d.insert_temp(open_id, open));
                }
            });
        });
        ui.add_space(14.0);
        bar(ui, &items, u.total);

        // the legend opens inside the card; its height from the last frame it was laid out
        let t = th::anim(ui.ctx(), open_id.with("t"), open, th::TRAVEL);
        if t > 0.0 {
            let full = ui.data(|d| d.get_temp::<f32>(open_id.with("h"))).unwrap_or(0.0);
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), full * t), Sense::hover());
            let mut child = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(rect.min, vec2(rect.width(), f32::INFINITY))));
            child.set_clip_rect(rect.intersect(ui.clip_rect()));
            child.set_opacity(t);
            legend(&mut child, &items);
            let h = child.min_rect().height();
            if (h - full).abs() > 0.5 {
                ui.data_mut(|d| d.insert_temp(open_id.with("h"), h));
                ui.ctx().request_repaint();
            }
        }
    });
}

/// The segmented bar: used space by category on a dim track that stands for the whole drive.
fn bar(ui: &mut Ui, items: &[(&str, u64, Color32)], total: u64) {
    const GAP: f32 = 2.0;
    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 10.0), Sense::hover());
    ui.painter().rect_filled(bar, 5.0, Color32::from_white_alpha(0x1C));
    let ppp = ui.ctx().pixels_per_point();
    let snap = |v: f32| (v * ppp).round() / ppp;
    let mut x = bar.left();
    for (i, (name, bytes, colour)) in items.iter().enumerate() {
        let share = if total == 0 { 0.0 } else { (*bytes as f64 / total as f64) as f32 };
        // the spring starts at its target, so the bar only moves when a new measurement changes it
        let w = th::spring(ui.ctx(), Id::new(("disk-seg", i)), share, th::TRAVEL, 1.0) * bar.width();
        if *bytes == 0 && w < 0.5 {
            continue;
        }
        let end = snap((x + w.max(GAP + 2.0)).min(bar.right())); // a small category stays visible
        let last = end >= bar.right() - 0.5;
        let seg = Rect::from_min_max(pos2(x, bar.top()), pos2(if last { end } else { end - GAP }, bar.bottom()));
        if seg.width() > 0.0 {
            let (l, r) = (if x == bar.left() { 5 } else { 2 }, if last { 5 } else { 2 });
            ui.painter().rect_filled(seg, CornerRadius { nw: l, sw: l, ne: r, se: r }, *colour);
            ui.interact(seg, Id::new(("disk-tip", i)), Sense::hover()).on_hover_text(format!("{name}: {}", size(*bytes)));
        }
        x = end;
    }
}

/// Colour dot, name and size per category that holds anything, in as many columns as fit.
fn legend(ui: &mut Ui, items: &[(&str, u64, Color32)]) {
    ui.add_space(14.0);
    let shown: Vec<_> = items.iter().filter(|i| i.1 > 0).collect();
    let cols = ((ui.available_width() / 190.0).floor() as usize).max(1);
    let cell = (ui.available_width() / cols as f32).floor();
    for row in shown.chunks(cols) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            for (name, bytes, colour) in row {
                ui.allocate_ui_with_layout(vec2(cell, 22.0), Layout::left_to_right(Align::Center), |ui| {
                    ui.set_width(cell);
                    let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                    th::dot(ui.painter(), dot.center(), 4.0, *colour);
                    ui.add_space(8.0);
                    ui.label(rt(*name, 13.0));
                    ui.add_space(6.0);
                    ui.label(rt(size(*bytes), 13.0).color(text2()));
                });
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::size;

    #[test]
    fn sizes_read_like_explorer() {
        assert_eq!(size(931 * 1_073_741_824), "931 GB");
        assert_eq!(size(1_073_741_824 * 3 / 2), "1.5 GB");
        assert_eq!(size(300 * 1_048_576), "300 MB");
    }
}
