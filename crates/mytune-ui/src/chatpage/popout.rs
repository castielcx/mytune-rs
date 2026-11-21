//! Pop-out windows: small windows of their own that stay above every other window, also while MyTune sits in
//! the tray. One for the playing song, one for the watched stream. Drag anywhere to move them; where they were
//! left is kept in the chat layout (`widths`, keys `pop_*`).

use super::*;
use egui::{pos2, Id, Rect};
use std::collections::BTreeMap;
use std::time::Duration;

/// Is the song window open? In egui's memory, so the media bar can flip it.
pub(super) fn song_id() -> Id {
    Id::new("pop_song_open")
}

/// False when no screen shows this point (physical pixels): a place saved on a monitor that is gone.
fn on_a_screen(x: f32, y: f32) -> bool {
    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::Graphics::Gdi::{MonitorFromPoint, MONITOR_DEFAULTTONULL};
    !unsafe { MonitorFromPoint(POINT { x: x as i32 + 20, y: y as i32 + 20 }, MONITOR_DEFAULTTONULL) }.is_null()
}

/// One pop-out window, drawn by `add`. False once it was closed.
fn window(ctx: &egui::Context, key: &str, title: &str, size: egui::Vec2, resizable: bool, widths: &mut BTreeMap<String, f32>, mut add: impl FnMut(&mut Ui, Rect)) -> bool {
    let k = |part: &str| format!("{key}_{part}");
    // the place it opens at is fixed for as long as it is open: a builder that followed the window would
    // make egui move the window back while it is dragged
    let start_id = Id::new(("pop_start", key));
    let start: (Option<egui::Pos2>, egui::Vec2) = ctx.data(|d| d.get_temp(start_id)).unwrap_or_else(|| {
        let ppp = ctx.pixels_per_point() * th::shell(); // this window's scale, not the chat page's
        let pos = widths.get(&k("x")).zip(widths.get(&k("y"))).map(|(x, y)| pos2(*x, *y)).filter(|p| on_a_screen(p.x * ppp, p.y * ppp));
        let size = widths.get(&k("w")).zip(widths.get(&k("h"))).filter(|_| resizable).map_or(size, |(w, h)| vec2(*w, *h));
        (pos, size)
    });
    ctx.data_mut(|d| d.insert_temp(start_id, start));
    let mut builder = egui::ViewportBuilder::default()
        .with_title(title) // never just "MyTune": the main window is found by that title
        .with_decorations(false)
        .with_taskbar(false)
        .with_always_on_top()
        .with_resizable(resizable)
        .with_min_inner_size(size * 0.5)
        .with_inner_size(start.1);
    if let Some(pos) = start.0 {
        builder = builder.with_position(pos);
    }
    let mut open = true;
    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(key), builder, |ui, _| {
        let c = ui.ctx().clone();
        th::glow_textures(&c); // this window's pixel grid; `App::ui` sets the main window's back after it
        let rect = ui.max_rect();
        open = !c.input(|i| i.viewport().close_requested());
        if ui.interact(rect, Id::new("pop_move"), Sense::drag()).drag_started() {
            c.send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        add(ui, rect);
        if ui.rect_contains_pointer(rect) {
            let r = Rect::from_min_size(pos2(rect.right() - 26.0, rect.top() + 4.0), vec2(22.0, 22.0));
            let x = ui.interact(r, Id::new("pop_close"), Sense::click());
            ui.painter().circle_filled(r.center(), 11.0, Color32::from_black_alpha(if x.hovered() { 0xE0 } else { 0x80 }));
            voice::glyph(ui.painter(), r.center(), "\u{E711}", 10.0, Color32::WHITE, false);
            if x.on_hover_text("Close").clicked() {
                open = false;
            }
            if resizable {
                let r = Rect::from_min_size(rect.max - vec2(18.0, 18.0), vec2(18.0, 18.0));
                let grip = ui.interact(r, Id::new("pop_size"), Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeNwSe);
                voice::glyph(ui.painter(), r.center(), "\u{E788}", 12.0, Color32::from_white_alpha(0xB0), false);
                if grip.drag_started() {
                    c.send_viewport_cmd(egui::ViewportCommand::BeginResize(egui::ResizeDirection::SouthEast));
                }
            }
        }
        ui.painter().rect_stroke(rect, 0.0, egui::Stroke::new(1.0, Color32::from_white_alpha(0x24)), egui::StrokeKind::Inside);
        // where it is, kept once it has rested for half a second (not on every step of a drag)
        let (outer, inner, now) = c.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect, i.time));
        if let (Some(o), Some(i)) = (outer, inner) {
            let at = [o.min.x, o.min.y, i.width(), i.height()].map(f32::round);
            let at_id = Id::new(("pop_at", key));
            match c.data(|d| d.get_temp::<([f32; 4], f64)>(at_id)) {
                Some((last, since)) if last == at => {
                    if now - since >= 0.5 {
                        for (part, v) in ["x", "y", "w", "h"].iter().zip(at) {
                            widths.insert(k(part), v);
                        }
                    }
                }
                _ => {
                    c.data_mut(|d| d.insert_temp(at_id, (at, now)));
                    c.request_repaint_after(Duration::from_millis(600));
                }
            }
        }
    });
    if !open {
        ctx.data_mut(|d| d.remove::<(Option<egui::Pos2>, egui::Vec2)>(start_id));
    }
    open
}

impl ChatPage {
    /// The open pop-out windows. Call every frame of the main window, on every page and while it is hidden.
    pub fn popouts(&mut self, ctx: &egui::Context) {
        let (Some(chat), Some(layout)) = (self.chat.clone(), self.layout.as_mut()) else { return };
        // Tests: MYTUNE_POP opens the song window at start.
        if std::env::var_os("MYTUNE_POP").is_some() {
            std::env::remove_var("MYTUNE_POP");
            ctx.data_mut(|d| d.insert_temp(song_id(), true));
        }
        let song = ctx.data(|d| d.get_temp::<bool>(song_id())).unwrap_or(false);
        if !song && !self.call.pop {
            return;
        }
        let before = layout.widths.clone();
        if song {
            let now = crate::nowplaying::now(ctx).unwrap_or_default();
            let open = window(ctx, "pop_song", "MyTune song", vec2(320.0, 56.0), false, &mut layout.widths, |ui, rect| {
                ui.painter().rect_filled(rect, 0.0, th::tone(0.10));
                // the main window may be hidden and not repaint: this window looks for a new song itself
                ui.ctx().request_repaint_after(Duration::from_secs(1));
                egui::Frame::new().inner_margin(egui::Margin { left: 14, right: 32, top: 10, bottom: 8 }).show(ui, |ui| media_row(ui, &now, false));
            });
            if !open {
                ctx.data_mut(|d| d.insert_temp(song_id(), false));
            }
        }
        if self.call.pop {
            let cu = &mut self.call;
            let mut acts: Vec<Action> = Vec::new();
            let open = chat.with_state(|s| s.voice.watching.is_some() && window(ctx, "pop_stream", "MyTune stream", vec2(480.0, 270.0), true, &mut layout.widths, |ui, rect| voice::pop_stream(ui, rect, cu, s, &mut acts)));
            cu.pop = open;
            for a in acts {
                chat.dispatch(a); // after the engine lock is let go
            }
        }
        if layout.widths != before {
            chat.set_layout(layout.clone());
        }
    }
}
