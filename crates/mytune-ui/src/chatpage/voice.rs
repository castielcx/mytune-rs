//! Voice on the chat page, drawn after Yapfast's `ui/voice.slint`: the call stage (participant tiles and round
//! controls) above the chat, the voice dock under the room list, the incoming-call card, the DM call invite,
//! the Go Live picker and the audio and camera device window. The engine does the calling; this only reads
//! `AppState` and pushes `Action`s. Icons are Segoe Fluent Icons (user's choice), with a slash drawn for "off".

use super::*;
use egui::{pos2, Rect, Stroke};
use mytune_chat::yapfast_core::picture::Feed;
use mytune_chat::yapfast_core::stream_worker::{self as sw, Report, Shareable, Source, Worker};
use mytune_chat::yapfast_core::voice::{self, CameraPhase, Command as Vc, Phase, Resolution, StreamQuality};
use std::sync::{Arc, Mutex};
use std::time::Duration;

// Yapfast theme tokens (ui/theme.slint) the chat page does not have yet.
const DANGER_HOVER: Color32 = Color32::from_rgb(0xf4, 0x6f, 0x77);
/// Theme.quick, the hover fade.
pub(super) const QUICK: f32 = 0.12;

// Segoe Fluent Icons glyphs.
const I_MIC: &str = "\u{E720}";
const I_HEADPHONES: &str = "\u{E7F6}";
const I_VIDEO: &str = "\u{E714}";
const I_SCREEN: &str = "\u{E7F4}";
const I_HANGUP: &str = "\u{E778}";
const I_PHONE: &str = "\u{E717}";
const I_SETTINGS: &str = "\u{E713}";
const I_SIGNAL: &str = "\u{E701}";

/// What the call UI keeps between frames: video textures, the feeds shown, the open windows.
#[derive(Default)]
pub struct CallUi {
    video: HashMap<String, TextureHandle>,
    /// Camera feeds drawn this frame and the last; one that stops showing is told so, so its sender stops.
    feeds_now: Vec<Feed>,
    feeds_last: Vec<Feed>,
    pub audio_open: bool,
    share: ShareMenu,
    /// Repaint soon: something moves (video, a pulse).
    live: bool,
    /// A watched stream fills the window.
    full: bool,
    /// This account's own shared screen fills the window.
    full_own: bool,
}

impl CallUi {
    /// Call once per frame after drawing: feeds that left the screen stop, moving things keep repainting.
    pub fn end_frame(&mut self, ctx: &egui::Context) {
        for f in self.feeds_last.drain(..) {
            if !self.feeds_now.contains(&f) {
                f.show_at(None);
            }
        }
        std::mem::swap(&mut self.feeds_now, &mut self.feeds_last);
        if std::mem::take(&mut self.live) {
            ctx.request_repaint_after(Duration::from_millis(33));
        }
    }

    /// The newest camera frame of `feed` at the size `rect` takes on screen, as a texture.
    fn feed_texture(&mut self, ctx: &egui::Context, key: &str, feed: &Feed, rect: Rect) -> Option<(egui::TextureId, egui::Vec2)> {
        let ppp = ctx.pixels_per_point();
        feed.show_at(Some(((rect.width() * ppp) as u32 & !1, (rect.height() * ppp) as u32 & !1)));
        self.feeds_now.push(feed.clone());
        self.live = true;
        let img = feed.take(|fb| mytune_chat::frame_pixels(fb).map(|((w, h), px)| egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], px))).flatten();
        self.put(ctx, key, img)
    }

    /// The newest frame of a stream (Go Live preview or a watched stream) as a texture.
    fn frames_texture(&mut self, ctx: &egui::Context, key: &str, frames: &mytune_chat::yapfast_core::frames::Frames) -> Option<(egui::TextureId, egui::Vec2)> {
        self.live = true;
        let img = frames.take(|w, h, px| egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px[..w as usize * h as usize * 4]));
        self.put(ctx, key, img)
    }

    fn put(&mut self, ctx: &egui::Context, key: &str, img: Option<egui::ColorImage>) -> Option<(egui::TextureId, egui::Vec2)> {
        if let Some(img) = img {
            match self.video.get_mut(key) {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => {
                    self.video.insert(key.to_owned(), ctx.load_texture(key, img, egui::TextureOptions::LINEAR));
                }
            }
        }
        self.video.get(key).map(|t| (t.id(), t.size_vec2()))
    }
}

/// Tile places for `n` 16:9 tiles in `area`: the column count that makes them largest, rows centred.
pub fn grid(n: usize, area: Rect, gap: f32) -> Vec<Rect> {
    if n == 0 || area.width() <= 0.0 || area.height() <= 0.0 {
        return vec![];
    }
    let mut best = (1usize, 0.0f32);
    for cols in 1..=n {
        let rows = n.div_ceil(cols);
        let w = ((area.width() - gap * (cols - 1) as f32) / cols as f32).min((area.height() - gap * (rows - 1) as f32) / rows as f32 * 16.0 / 9.0);
        if w > best.1 {
            best = (cols, w);
        }
    }
    let (cols, w) = best;
    let h = w * 9.0 / 16.0;
    let rows = n.div_ceil(cols);
    let top = area.top() + (area.height() - (rows as f32 * h + gap * (rows - 1) as f32)) / 2.0;
    (0..n)
        .map(|i| {
            let (r, c) = (i / cols, i % cols);
            let in_row = if r == rows - 1 { n - r * cols } else { cols };
            let left = area.center().x - (in_row as f32 * w + gap * (in_row - 1) as f32) / 2.0;
            Rect::from_min_size(pos2(left + c as f32 * (w + gap), top + r as f32 * (h + gap)), vec2(w, h))
        })
        .collect()
}

/// A tile slides to its new place in 160 ms, as Yapfast's StageTile does.
fn glide(ctx: &egui::Context, id: egui::Id, r: Rect) -> Rect {
    let a = |k: &str, v: f32| ctx.animate_value_with_time(id.with(k), v, 0.16);
    Rect::from_min_size(pos2(a("x", r.min.x), a("y", r.min.y)), vec2(a("w", r.width()), a("h", r.height())))
}

/// A glyph, with a diagonal slash when it is off (mic-off, headphone-off, video-off).
pub(super) fn glyph(p: &egui::Painter, c: egui::Pos2, g: &str, size: f32, color: Color32, slash: bool) {
    p.text(c, egui::Align2::CENTER_CENTER, g, egui::FontId::new(size, th::icons()), color);
    if slash {
        let d = size * 0.45;
        p.line_segment([c + vec2(-d, -d), c + vec2(d, d)], Stroke::new(2.0, color));
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Look {
    Plain,
    /// Muted or deafened: red tint.
    Alert,
    /// Camera or sharing on: green tint.
    On,
}

/// Yapfast's IconButton: a round 52 px button (or `size`), raised fill, a quick hover fade.
fn round_button(ui: &mut Ui, size: f32, g: &str, slash: bool, look: Look, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), QUICK);
    let (fill, icon) = match look {
        Look::Plain => (k_raised(), SECONDARY),
        Look::Alert => (DANGER.gamma_multiply(0.18), DANGER),
        Look::On => (LIVE.gamma_multiply(0.22), LIVE),
    };
    let fill = fill.lerp_to_gamma(Color32::from_rgb(0x3a, 0x3b, 0x44), t * 0.6);
    let icon = if look == Look::Plain { icon.lerp_to_gamma(TEXT, t) } else { icon };
    ui.painter().rect_filled(rect, size / 2.0, fill);
    glyph(ui.painter(), rect.center(), g, size * 0.42, icon, slash);
    resp.on_hover_text(tip)
}

/// Yapfast's Leave button: a red pill with the hang-up glyph.
fn leave_button(ui: &mut Ui, w: f32, h: f32, leaving: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, h), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), QUICK);
    let fill = DANGER.lerp_to_gamma(DANGER_HOVER, t).gamma_multiply(if leaving { 0.5 } else { 1.0 });
    ui.painter().rect_filled(rect, h / 2.0, fill);
    glyph(ui.painter(), rect.center(), I_HANGUP, h * 0.42, Color32::WHITE, false);
    resp.on_hover_text("Leave call")
}

/// Yapfast's CallButton: a 64x48 pill with a label under it (Accept / Decline / Join).
fn call_button(ui: &mut Ui, g: &str, label: &str, fill: Color32) -> bool {
    let mut clicked = false;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 4.0;
        let (rect, resp) = ui.allocate_exact_size(vec2(64.0, 48.0), Sense::click());
        let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), QUICK);
        ui.painter().rect_filled(rect, 24.0, fill.lerp_to_gamma(Color32::BLACK, t * 0.15));
        glyph(ui.painter(), rect.center(), g, 22.0, Color32::WHITE, false);
        let (lr, _) = ui.allocate_exact_size(vec2(64.0, 16.0), Sense::hover());
        ui.painter().text(lr.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(13.0), SECONDARY);
        clicked = resp.clicked();
    });
    clicked
}

/// A pill on a tile (Watch stream, LIVE, Stop watching).
fn tile_pill(ui: &mut Ui, at: egui::Pos2, text: &str, fill: Color32, id: egui::Id) -> bool {
    let g = ui.painter().layout_no_wrap(text.to_owned(), egui::FontId::new(12.0, th::semibold()), Color32::WHITE);
    let rect = Rect::from_min_size(pos2(at.x - g.size().x - 20.0, at.y), vec2(g.size().x + 20.0, 26.0));
    let resp = ui.interact(rect, id, Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), QUICK);
    ui.painter().rect_filled(rect, 13.0, fill.lerp_to_gamma(Color32::WHITE, t * 0.12));
    ui.painter().galley(rect.center() - g.size() / 2.0, g, Color32::WHITE);
    resp.clicked()
}

/// A picture filling `rect` (cropped to fit, as a video tile does), with rounded corners.
fn cover(ui: &Ui, rect: Rect, (id, size): (egui::TextureId, egui::Vec2), radius: f32, fit: bool) {
    let (ra, ta) = (rect.width() / rect.height(), size.x / size.y.max(1.0));
    if fit {
        // a shared screen shows whole, letterboxed
        let s = if ta > ra { vec2(rect.width(), rect.width() / ta) } else { vec2(rect.height() * ta, rect.height()) };
        ui.painter().rect_filled(rect, radius, Color32::BLACK);
        egui::Image::new(egui::load::SizedTexture::new(id, s)).corner_radius(radius).paint_at(ui, Rect::from_center_size(rect.center(), s));
        return;
    }
    let uv = if ta > ra {
        let w = ra / ta;
        Rect::from_min_max(pos2((1.0 - w) / 2.0, 0.0), pos2((1.0 + w) / 2.0, 1.0))
    } else {
        let h = ta / ra;
        Rect::from_min_max(pos2(0.0, (1.0 - h) / 2.0), pos2(1.0, (1.0 + h) / 2.0))
    };
    egui::Image::new(egui::load::SizedTexture::new(id, rect.size())).uv(uv).corner_radius(radius).paint_at(ui, rect);
}

/// The name chip at a tile's bottom left: dark glass, the name, and a mic-off / headphone-off glyph.
fn name_chip(ui: &Ui, rect: Rect, name: &str, muted: bool, deafened: bool, server: bool) {
    let g = ui.painter().layout_no_wrap(name.to_owned(), egui::FontId::proportional(13.0), TEXT);
    let icon = muted || deafened;
    let w = (g.size().x + 16.0 + if icon { 20.0 } else { 0.0 }).min(rect.width() - 16.0);
    let chip = Rect::from_min_size(pos2(rect.left() + 8.0, rect.bottom() - 34.0), vec2(w, 26.0));
    ui.painter().rect_filled(chip, 6.0, Color32::from_black_alpha(166));
    let mut x = chip.left() + 8.0;
    if icon {
        let c = if server { DANGER } else { SECONDARY };
        glyph(ui.painter(), pos2(x + 7.0, chip.center().y), if deafened { I_HEADPHONES } else { I_MIC }, 12.0, c, true);
        x += 20.0;
    }
    ui.painter().with_clip_rect(chip).galley(pos2(x, chip.center().y - g.size().y / 2.0), g, TEXT);
}

/// One participant's tile: camera video or their avatar, a green border while they speak, the name chip, and a
/// "Watch stream" pill when they share their screen (LIVE for this account's own share, with its preview).
#[allow(clippy::too_many_arguments)]
fn participant_tile(ui: &mut Ui, cu: &mut CallUi, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>, pe: &voice::Participant, rect: Rect) {
    let ctx = ui.ctx().clone();
    let id = egui::Id::new(("tile", &pe.id));
    let rect = glide(&ctx, id, rect);
    ui.painter().rect_filled(rect, 12.0, k_raised());
    let v = &s.voice;
    let share = if pe.local && !cu.full_own { v.sharing.as_ref().and_then(|sh| cu.frames_texture(&ctx, "share", &sh.frames)) } else { None };
    let video = match (&pe.video, share) {
        (_, Some(t)) => Some((t, true)),
        (Some(feed), None) => cu.feed_texture(&ctx, &format!("v:{}", pe.id), feed, rect).map(|t| (t, false)),
        _ => None,
    };
    match video {
        Some((t, fit)) => cover(ui, rect, t, 12.0, fit),
        None => {
            let d = (rect.height() * 0.45).min(80.0);
            let t = texture(&ctx, tex, asked, s, &pe.avatar, want);
            avatar(ui, Rect::from_center_size(rect.center(), vec2(d, d)), t, &pe.user);
            // someone the call is still ringing: dimmed with a pulsing ring and "Calling...", not as if they joined
            if !pe.connected && !pe.local {
                cu.live = true;
                ui.painter().circle_filled(rect.center(), d / 2.0, k_raised().gamma_multiply(0.62));
                let a = 0.45 + 0.4 * (ctx.input(|i| i.time) as f32 * std::f32::consts::TAU).sin();
                ui.painter().circle_stroke(rect.center(), d / 2.0 + 7.0, Stroke::new(3.0, LIVE.gamma_multiply(a.clamp(0.0, 1.0))));
                ui.painter().text(pos2(rect.center().x, rect.center().y + d / 2.0 + 20.0), egui::Align2::CENTER_CENTER, pe.problem.as_deref().unwrap_or("Calling..."), egui::FontId::proportional(13.0), MUTED);
            }
            if pe.local && v.camera_phase == CameraPhase::Starting {
                cu.live = true;
                let a = 0.6 + 0.4 * (ctx.input(|i| i.time) as f32 * std::f32::consts::TAU).sin();
                ui.painter().text(pos2(rect.center().x, rect.center().y + d / 2.0 + 14.0), egui::Align2::CENTER_CENTER, "Starting camera\u{2026}", egui::FontId::proportional(12.0), MUTED.gamma_multiply(a));
            }
        }
    }
    let spk = ctx.animate_bool_with_time(id.with("spk"), pe.speaking, QUICK);
    if spk > 0.0 {
        ui.painter().rect_stroke(rect, 12.0, Stroke::new(3.0, LIVE.gamma_multiply(spk)), egui::StrokeKind::Inside);
    }
    let name = if pe.local { format!("{} (you)", pe.user) } else { pe.user.clone() };
    name_chip(ui, rect, &name, pe.muted, pe.deafened, pe.server_muted || pe.server_deafened);
    let at = pos2(rect.right() - 8.0, rect.top() + 8.0);
    if pe.local && v.sharing.is_some() {
        // a click on one's own shared screen opens it over the whole window
        if ui.interact(rect, id.with("open"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Click for fullscreen").clicked() {
            cu.full_own = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        }
        tile_pill(ui, at, "LIVE", DANGER, id.with("live"));
    } else if pe.streaming && !pe.local && v.watching.as_ref().is_none_or(|w| w.user != pe.id) && tile_pill(ui, at, "Watch stream", DANGER, id.with("watch")) {
        acts.push(Action::Voice(Vc::Watch(pe.id.clone())));
    }
}

/// The call stage for the call in front: tiles (a watched stream large with the others in a strip) and the
/// round controls under them. `rect` is the stage's whole area.
#[allow(clippy::too_many_arguments)]
pub fn stage(ui: &mut Ui, rect: Rect, cu: &mut CallUi, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    let v = &s.voice;
    ui.painter().rect_filled(rect, 0.0, k_chat());
    let controls = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 76.0), rect.max);
    let area = Rect::from_min_max(rect.min + vec2(16.0, 16.0), pos2(rect.right() - 16.0, controls.top()));
    let people: Vec<&voice::Participant> = v.participants.iter().collect();
    if let Some(w) = &v.watching {
        let strip_h = if people.is_empty() { 0.0 } else { (area.height() * 0.22).clamp(70.0, 108.0) };
        let big = Rect::from_min_max(area.min, pos2(area.right(), area.bottom() - strip_h - if strip_h > 0.0 { 8.0 } else { 0.0 }));
        let big = grid(1, big, 0.0).first().copied().unwrap_or(big);
        ui.painter().rect_filled(big, 12.0, Color32::BLACK);
        match w.frames.as_ref().filter(|_| !cu.full).and_then(|f| cu.frames_texture(&ctx, "watch", f)) {
            Some(t) if !w.opening => cover(ui, big, t, 12.0, true),
            _ => {
                cu.live = true;
                let a = 0.6 + 0.4 * (ctx.input(|i| i.time) as f32 * std::f32::consts::TAU).sin();
                ui.painter().text(big.center(), egui::Align2::CENTER_CENTER, "Opening stream\u{2026}", egui::FontId::proportional(14.0), SECONDARY.gamma_multiply(a));
            }
        }
        if ui.interact(big, egui::Id::new("watch_open"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            cu.full = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        }
        name_chip(ui, big, &format!("{}'s screen", w.name), false, false, false);
        if tile_pill(ui, pos2(big.right() - 8.0 - 118.0, big.top() + 8.0), "Fullscreen", k_raised(), egui::Id::new("watch_full")) {
            cu.full = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
        }
        if tile_pill(ui, pos2(big.right() - 8.0, big.top() + 8.0), "Stop watching", k_raised(), egui::Id::new("stop_watch")) {
            acts.push(Action::Voice(Vc::StopWatching));
        }
        let strip = Rect::from_min_max(pos2(area.left(), area.bottom() - strip_h), area.max);
        for (pe, r) in people.iter().zip(grid(people.len(), strip, 8.0)) {
            participant_tile(ui, cu, s, tex, asked, want, acts, pe, r);
        }
    } else {
        for (pe, r) in people.iter().zip(grid(people.len(), area, 12.0)) {
            participant_tile(ui, cu, s, tex, asked, want, acts, pe, r);
        }
    }
    // controls: Mute, Deafen, Camera, Share, then Leave (CallControls)
    let n_round = 2 + usize::from(v.can_film) + 1;
    let width = n_round as f32 * 52.0 + 76.0 + 12.0 * n_round as f32;
    let row = Rect::from_center_size(pos2(controls.center().x, controls.top() + 8.0 + 26.0), vec2(width, 52.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(row).layout(Layout::left_to_right(Align::Center)), |ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        controls_row(ui, cu, v, acts, 52.0);
        if leave_button(ui, 76.0, 52.0, v.phase == Phase::Leaving).clicked() {
            acts.push(Action::Voice(Vc::Leave));
        }
    });
}

/// Mute, Deafen, Camera (when the call carries video) and Share, as round buttons of `size`.
fn controls_row(ui: &mut Ui, cu: &mut CallUi, v: &voice::State, acts: &mut Vec<Action>, size: f32) {
    let alert = |on: bool| if on { Look::Alert } else { Look::Plain };
    if round_button(ui, size, I_MIC, v.muted, alert(v.muted), if v.muted { "Unmute" } else { "Mute" }).clicked() {
        acts.push(Action::Voice(Vc::Mute(!v.muted)));
    }
    if round_button(ui, size, I_HEADPHONES, v.deafened, alert(v.deafened), if v.deafened { "Undeafen" } else { "Deafen" }).clicked() {
        acts.push(Action::Voice(Vc::Deafen(!v.deafened)));
    }
    if v.can_film {
        let on = v.camera_phase != CameraPhase::Off;
        if round_button(ui, size, I_VIDEO, !on, if on { Look::On } else { Look::Plain }, if on { "Turn camera off" } else { "Turn camera on" }).clicked() {
            acts.push(Action::Voice(Vc::Video(!on)));
        }
    }
    let sharing = v.sharing.is_some();
    if round_button(ui, size, I_SCREEN, false, if sharing { Look::On } else { Look::Plain }, if sharing { "Stop sharing" } else { "Share your screen" }).clicked() {
        if sharing {
            acts.push(Action::Voice(Vc::StopLive));
        } else {
            cu.share.open(ui.ctx(), v.quality);
        }
    }
}

/// Yapfast's VoiceDock under the room list: signal and status, the call's name (click opens it), a leave button,
/// then the dock buttons (mic, headphones, camera, screen, settings).
pub fn dock(ui: &mut Ui, cu: &mut CallUi, s: &AppState, acts: &mut Vec<Action>) {
    let v = &s.voice;
    if !v.active() {
        return;
    }
    let (status, color) = match v.phase {
        Phase::Joining => ("Connecting", th::WARNING),
        Phase::Reconnecting => ("Reconnecting", th::WARNING),
        Phase::Leaving => ("Leaving", MUTED),
        _ => ("Voice Connected", LIVE),
    };
    egui::Frame::new().fill(k_panel()).inner_margin(egui::Margin::symmetric(10, 10)).show(ui, |ui| {
        ui.horizontal(|ui| {
            let (ic, _) = ui.allocate_exact_size(vec2(24.0, 36.0), Sense::hover());
            glyph(ui.painter(), ic.center(), I_SIGNAL, 16.0, color, false);
            let r = ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(sb(status, 15.0).color(color));
                ui.add(egui::Label::new(rt(&v.name, 12.0).color(MUTED)).truncate());
            });
            if let Some(room) = &v.room {
                if ui.interact(r.response.rect, egui::Id::new("dock_open"), Sense::click()).on_hover_text("Open the call").clicked() {
                    acts.push(Action::Open(room.clone()));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if round_button(ui, 32.0, I_HANGUP, false, Look::Alert, "Leave call").clicked() {
                    acts.push(Action::Voice(Vc::Leave));
                }
            });
        });
        if let Some(e) = v.error.as_ref().or(v.warning.as_ref()) {
            ui.add(egui::Label::new(rt(e, 11.0).color(DANGER)).wrap());
        }
        ui.add_space(6.0);
        let w = (ui.available_width() - 4.0 * 6.0) / 5.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let mut b = |ui: &mut Ui, g: &str, slash: bool, on: bool, alert: bool, tip: &str| dock_button(ui, w, g, slash, on, alert, tip);
            if b(ui, I_MIC, v.muted, false, v.muted, if v.muted { "Unmute" } else { "Mute" }).clicked() {
                acts.push(Action::Voice(Vc::Mute(!v.muted)));
            }
            if b(ui, I_HEADPHONES, v.deafened, false, v.deafened, if v.deafened { "Undeafen" } else { "Deafen" }).clicked() {
                acts.push(Action::Voice(Vc::Deafen(!v.deafened)));
            }
            let cam = v.camera_phase != CameraPhase::Off;
            if v.can_film && b(ui, I_VIDEO, !cam, cam, false, "Camera").clicked() {
                acts.push(Action::Voice(Vc::Video(!cam)));
            }
            let sharing = v.sharing.is_some();
            if b(ui, I_SCREEN, false, sharing, false, if sharing { "Stop sharing" } else { "Share your screen" }).clicked() {
                if sharing {
                    acts.push(Action::Voice(Vc::StopLive));
                } else {
                    cu.share.open(ui.ctx(), v.quality);
                }
            }
            if b(ui, I_SETTINGS, false, cu.audio_open, false, "Voice and video devices").clicked() {
                cu.audio_open = !cu.audio_open;
                if cu.audio_open {
                    acts.push(Action::Voice(Vc::Devices));
                }
            }
        });
    });
}

/// Yapfast's DockButton: 32 px high, radius 8; green when on, red glyph when it alerts.
fn dock_button(ui: &mut Ui, w: f32, g: &str, slash: bool, on: bool, alert: bool, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), QUICK);
    let fill = if on { LIVE.gamma_multiply(0.22 + 0.10 * t) } else { k_raised().lerp_to_gamma(k_hover(), t) };
    ui.painter().rect_filled(rect, 8.0, fill);
    let icon = if alert { DANGER } else if on { LIVE } else { SECONDARY.lerp_to_gamma(TEXT, t) };
    glyph(ui.painter(), rect.center(), g, 17.0, icon, slash);
    resp.on_hover_text(tip)
}

/// The rooms whose DM call rings this account and which it is not in.
fn ringing(s: &AppState) -> impl Iterator<Item = &mytune_chat::yapfast_core::chat::ChatRoom> {
    let v = &s.voice;
    s.rooms.iter().map(|r| r.as_ref()).filter(move |r| r.incoming && !(v.active() && v.room.as_ref() == Some(&r.id)))
}

/// An avatar with a green ring that pulses once a second (IncomingCall).
fn pulsing_avatar(ui: &mut Ui, t: Option<egui::TextureId>, name: &str, d: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(d + 20.0, d + 20.0), Sense::hover());
    let a = 0.45 + 0.4 * (ui.input(|i| i.time) as f32 * std::f32::consts::TAU).sin();
    ui.painter().circle_stroke(rect.center(), d / 2.0 + 7.0, Stroke::new(3.0, LIVE.gamma_multiply(a.clamp(0.0, 1.0))));
    avatar(ui, Rect::from_center_size(rect.center(), vec2(d, d)), t, name);
}

/// Who is in a call, as Yapfast words it.
fn who_is_here(people: &[mytune_chat::yapfast_core::chat::VoicePerson]) -> String {
    match people.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().as_slice() {
        [one] => format!("{one} is"),
        [a, b] => format!("{a} and {b} are"),
        [a, b, c] => format!("{a}, {b} and {c} are"),
        [a, b, rest @ ..] => format!("{a}, {b} and {} others are", rest.len()),
        [] => String::new(),
    }
}

/// A floating card for a DM call ringing in another conversation (the open one answers in its own invite).
pub fn incoming_card(ctx: &egui::Context, cu: &mut CallUi, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>, any_page: bool) {
    let Some(r) = ringing(s).find(|r| any_page || s.selected.as_ref() != Some(&r.id)) else { return };
    cu.live = true;
    egui::Area::new(egui::Id::new("incoming_call")).anchor(egui::Align2::RIGHT_TOP, vec2(-24.0, 64.0)).order(egui::Order::Foreground).show(ctx, |ui| {
        egui::Frame::new()
            .fill(k_raised())
            .stroke(Stroke::new(1.0, k_edge()))
            .corner_radius(12.0)
            .shadow(egui::Shadow { offset: [0, 12], blur: 32, spread: 0, color: Color32::from_black_alpha(0x66) })
            .inner_margin(egui::Margin::same(18))
            .show(ui, |ui| {
                ui.set_width(264.0);
                ui.vertical_centered(|ui| {
                    let t = texture(ctx, tex, asked, s, &r.avatar, want);
                    pulsing_avatar(ui, t, &r.name, 72.0);
                    ui.label(sb(&r.name, 16.0).color(TEXT));
                    let detail = if r.person.is_some() || r.people.is_empty() { "Incoming call".to_owned() } else { format!("{} calling", who_is_here(&r.people)) };
                    ui.label(rt(detail, 13.0).color(MUTED));
                    ui.add_space(12.0);
                    answer_buttons(ui, &r.id, acts);
                });
            });
    });
}

fn answer_buttons(ui: &mut Ui, room: &str, acts: &mut Vec<Action>) {
    let w = 64.0 * 2.0 + 40.0;
    ui.allocate_ui_with_layout(vec2(w, 70.0), Layout::left_to_right(Align::Min), |ui| {
        ui.spacing_mut().item_spacing.x = 40.0;
        if call_button(ui, I_HANGUP, "Decline", DANGER) {
            acts.push(Action::DeclineCall(room.to_owned()));
        }
        if call_button(ui, I_PHONE, "Accept", LIVE) {
            acts.push(Action::AcceptCall(room.to_owned()));
        }
    });
}

/// The stage slot of an open DM whose call this account is not in but that rings it or has people in it
/// (CallInvite): the people, then Accept/Decline or Join. Returns false when there is nothing to show.
#[allow(clippy::too_many_arguments)]
pub fn invite(ui: &mut Ui, rect: Rect, s: &AppState, room: &mytune_chat::yapfast_core::chat::ChatRoom, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>, cu: &mut CallUi) {
    let ctx = ui.ctx().clone();
    ui.painter().rect_filled(rect, 0.0, k_chat());
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect.shrink(16.0)).layout(Layout::top_down(Align::Center)), |ui| {
        ui.add_space((rect.height() - 220.0).max(0.0) / 2.0);
        if room.incoming {
            cu.live = true;
            let t = texture(&ctx, tex, asked, s, &room.avatar, want);
            pulsing_avatar(ui, t, &room.name, 72.0);
            ui.label(rt(if room.people.is_empty() || room.person.is_some() { "Incoming call".to_owned() } else { format!("{} calling", who_is_here(&room.people)) }, 14.0).color(SECONDARY));
            ui.add_space(10.0);
            answer_buttons(ui, &room.id, acts);
        } else {
            let n = room.people.len().min(5);
            let (row, _) = ui.allocate_exact_size(vec2(n as f32 * 64.0, 64.0), Sense::hover());
            for (i, pe) in room.people.iter().take(5).enumerate() {
                let t = texture(&ctx, tex, asked, s, &pe.avatar, want);
                avatar(ui, Rect::from_min_size(row.min + vec2(i as f32 * 64.0 + 4.0, 4.0), vec2(56.0, 56.0)), t, &pe.name);
            }
            ui.add_space(6.0);
            ui.label(rt(format!("{} in the call", who_is_here(&room.people)), 14.0).color(SECONDARY));
            ui.add_space(10.0);
            if call_button(ui, I_PHONE, "Join", LIVE) {
                acts.push(Action::RequestJoinVoice(room.id.clone()));
            }
        }
    });
}

/// Microphone, speaker and camera pickers; the engine lists the devices once a `Devices` command ran.
pub fn devices_window(ctx: &egui::Context, s: &AppState, acts: &mut Vec<Action>, open: &mut bool) {
    let v: &voice::State = &s.voice;
    egui::Window::new("Voice and video").open(open).collapsible(false).resizable(false).show(ctx, |ui| {
        let pick = |ui: &mut Ui, label: &str, list: &[voice::Device], cur: &Option<String>, make: fn(String) -> Vc, acts: &mut Vec<Action>| {
            ui.label(rt(label, 12.0).color(MUTED));
            let shown = list.iter().find(|d| Some(&d.id) == cur.as_ref()).map(|d| d.name.as_str()).unwrap_or("System default");
            egui::ComboBox::from_id_salt(label).width(300.0).selected_text(shown).show_ui(ui, |ui| {
                for d in list {
                    if ui.selectable_label(Some(&d.id) == cur.as_ref(), &d.name).clicked() {
                        acts.push(Action::Voice(make(d.id.clone())));
                    }
                }
            });
            ui.add_space(8.0);
        };
        pick(ui, "Microphone", &v.microphones, &v.microphone, Vc::Microphone, acts);
        pick(ui, "Speakers", &v.speakers, &v.speaker, Vc::Speaker, acts);
        pick(ui, "Camera", &v.cameras, &v.camera, Vc::Camera, acts);
        if v.microphones.is_empty() && v.speakers.is_empty() {
            ui.label(rt("No devices listed yet. Join a call, then open this again.", 12.0).color(MUTED));
        }
    });
}

/// What the Go Live picker lists, filled by a thread: screens (with pictures), windows, cameras.
#[derive(Default)]
struct Listed {
    done: bool,
    screens: Vec<Shareable>,
    windows: Vec<Shareable>,
    cameras: Vec<Shareable>,
    pictures: HashMap<u64, egui::ColorImage>,
}

/// Yapfast's ShareMenu: what to share, then resolution, frame rate and "Higher quality", then Go Live.
#[derive(Default)]
struct ShareMenu {
    open: bool,
    listed: Arc<Mutex<Listed>>,
    thumbs: HashMap<u64, TextureHandle>,
    picked: Option<Source>,
    quality: StreamQuality,
}

impl ShareMenu {
    fn open(&mut self, ctx: &egui::Context, quality: StreamQuality) {
        self.open = true;
        self.quality = quality;
        self.picked = None;
        self.thumbs.clear();
        let listed = Arc::new(Mutex::new(Listed::default()));
        self.listed = listed.clone();
        let ctx = ctx.clone();
        // Listing starts screen and window capturers, so it runs beside the UI; pictures come from a worker.
        std::thread::spawn(move || {
            let mut l = Listed::default();
            for x in sw::shareable() {
                match x.source {
                    Source::Display(_) => l.screens.push(x),
                    Source::Camera(_) => l.cameras.push(x),
                    _ => l.windows.push(x),
                }
            }
            let ids: Vec<u64> = l.screens.iter().filter_map(|x| if let Source::Display(id) = x.source { Some(id) } else { None }).collect();
            l.done = true;
            *listed.lock().unwrap_or_else(|e| e.into_inner()) = l;
            ctx.request_repaint();
            if ids.is_empty() {
                return;
            }
            let ppp = ctx.pixels_per_point();
            let Ok((_worker, mut reports)) = Worker::glance(ids, (160.0 * ppp) as u32, (90.0 * ppp) as u32) else { return };
            while let Some(r) = reports.blocking_recv() {
                match r {
                    Report::Screen { id, width, height, rgba } => {
                        let img = egui::ColorImage::from_rgba_unmultiplied([width as usize, height as usize], &rgba);
                        listed.lock().unwrap_or_else(|e| e.into_inner()).pictures.insert(id, img);
                        ctx.request_repaint();
                    }
                    Report::Ended { .. } => break,
                    _ => {}
                }
            }
        });
    }

    fn window(&mut self, ctx: &egui::Context, acts: &mut Vec<Action>) {
        if !self.open {
            return;
        }
        const W: f32 = 560.0;
        let (mut close, mut go) = (false, false);
        let pal = p();
        let modal = egui::Modal::new(egui::Id::new("share_picker"))
            .frame(egui::Frame::new().fill(k_panel()).stroke(Stroke::new(1.0, k_edge())).corner_radius(14.0).inner_margin(egui::Margin::same(22)))
            .show(ctx, |ui| {
                ui.set_width(W);
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                // header: title left, close right
                let (head, _) = ui.allocate_exact_size(vec2(W, 28.0), Sense::hover());
                ui.painter().text(head.left_center(), egui::Align2::LEFT_CENTER, "Share your screen", egui::FontId::new(18.0, th::semibold()), TEXT);
                let xr = Rect::from_center_size(pos2(head.right() - 14.0, head.center().y), vec2(28.0, 28.0));
                let xe = ui.interact(xr, egui::Id::new("share_close"), Sense::click());
                if xe.hovered() {
                    ui.painter().rect_filled(xr, 8.0, k_raised());
                }
                glyph(ui.painter(), xr.center(), "\u{E711}", 13.0, if xe.hovered() { TEXT } else { SECONDARY }, false);
                if xe.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    close = true;
                }
                ui.add_space(6.0);
                let mut l = self.listed.lock().unwrap_or_else(|e| e.into_inner());
                if !l.done {
                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| {
                        ui.add(egui::Spinner::new());
                        ui.label(rt("Looking for screens and windows\u{2026}", 13.0).color(MUTED));
                    });
                    ui.add_space(24.0);
                    return;
                }
                for (id, img) in l.pictures.drain() {
                    self.thumbs.insert(id, ctx.load_texture(format!("screen:{id}"), img, egui::TextureOptions::LINEAR));
                }
                let section = |ui: &mut Ui, t: &str| {
                    ui.add_space(6.0);
                    ui.label(sb(t, 11.0).color(MUTED));
                };
                // screens: two equal cards per row
                if !l.screens.is_empty() {
                    section(ui, "SCREENS");
                    let gap = 12.0;
                    let cw = (W - gap) / 2.0;
                    let ph = (cw - 16.0) * 9.0 / 16.0;
                    for pair in l.screens.chunks(2) {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = gap;
                            for x in pair {
                                let Source::Display(id) = x.source else { continue };
                                let on = self.picked.as_ref() == Some(&x.source);
                                let (rect, resp) = ui.allocate_exact_size(vec2(cw, ph + 46.0), Sense::click());
                                let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), QUICK);
                                ui.painter().rect_filled(rect, 10.0, if on { pal.accent_soft } else { k_raised().lerp_to_gamma(k_hover(), t) });
                                if on {
                                    ui.painter().rect_stroke(rect, 10.0, Stroke::new(2.0, pal.accent), egui::StrokeKind::Inside);
                                }
                                let pic = Rect::from_min_size(rect.min + vec2(8.0, 8.0), vec2(cw - 16.0, ph));
                                match self.thumbs.get(&id) {
                                    Some(tx) => egui::Image::new(egui::load::SizedTexture::new(tx.id(), pic.size())).corner_radius(6.0).paint_at(ui, pic),
                                    None => {
                                        ui.painter().rect_filled(pic, 6.0, Color32::BLACK);
                                    }
                                }
                                if on {
                                    let c = pos2(pic.right() - 14.0, pic.top() + 14.0);
                                    ui.painter().circle_filled(c, 11.0, pal.accent);
                                    glyph(ui.painter(), c, "\u{E73E}", 12.0, pal.on_accent, false);
                                }
                                ui.painter().text(pos2(rect.center().x, rect.bottom() - 19.0), egui::Align2::CENTER_CENTER, &x.title, egui::FontId::new(13.0, th::semibold()), if on { TEXT } else { SECONDARY });
                                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    self.picked = Some(x.source.clone());
                                }
                            }
                        });
                    }
                }
                // windows and cameras: full-width rows with a radio dot
                let mut rows = |ui: &mut Ui, title: &str, list: &[Shareable]| {
                    if list.is_empty() {
                        return;
                    }
                    section(ui, title);
                    egui::ScrollArea::vertical().id_salt(title).max_height(5.0 * 42.0).auto_shrink([false, true]).show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 6.0;
                        for x in list {
                            let on = self.picked.as_ref() == Some(&x.source);
                            let (rect, resp) = ui.allocate_exact_size(vec2(W, 36.0), Sense::click());
                            let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), QUICK);
                            ui.painter().rect_filled(rect, 8.0, if on { pal.accent_soft } else { k_raised().lerp_to_gamma(k_hover(), t) });
                            if on {
                                ui.painter().rect_stroke(rect, 8.0, Stroke::new(1.5, pal.accent), egui::StrokeKind::Inside);
                            }
                            let dot = pos2(rect.left() + 20.0, rect.center().y);
                            ui.painter().circle_stroke(dot, 7.0, Stroke::new(1.5, if on { pal.accent } else { FAINT }));
                            if on {
                                ui.painter().circle_filled(dot, 3.5, pal.accent);
                            }
                            let clip = ui.painter().with_clip_rect(Rect::from_min_max(pos2(rect.left() + 38.0, rect.top()), pos2(rect.right() - 12.0, rect.bottom())));
                            clip.text(pos2(rect.left() + 38.0, rect.center().y), egui::Align2::LEFT_CENTER, &x.title, egui::FontId::proportional(13.5), if on { TEXT } else { SECONDARY });
                            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                self.picked = Some(x.source.clone());
                            }
                        }
                    });
                };
                rows(ui, "WINDOWS", &l.windows);
                rows(ui, "CAMERAS", &l.cameras);
                drop(l);
                // stream quality: one label column, equal segments
                ui.add_space(12.0);
                let y = ui.cursor().top();
                ui.painter().hline(ui.max_rect().x_range(), y, Stroke::new(1.0, k_edge()));
                ui.add_space(12.0);
                let res = [Resolution::P720, Resolution::P1080, Resolution::Source];
                let at = res.iter().position(|r| *r == self.quality.resolution).unwrap_or(1);
                if let Some(i) = setting_row(ui, "Resolution", &["720p", "1080p", "Source"], at, pal) {
                    self.quality.resolution = res[i];
                }
                let fps = [15, 30, 60];
                let at = fps.iter().position(|f| *f == self.quality.fps).unwrap_or(2);
                if let Some(i) = setting_row(ui, "Frame rate", &["15 fps", "30 fps", "60 fps"], at, pal) {
                    self.quality.fps = fps[i];
                }
                // higher quality: a switch with its note
                let (row, resp) = ui.allocate_exact_size(vec2(W, 44.0), Sense::click());
                ui.painter().text(pos2(row.left(), row.center().y - 9.0), egui::Align2::LEFT_CENTER, "Higher quality", egui::FontId::new(13.5, th::semibold()), TEXT);
                ui.painter().text(pos2(row.left(), row.center().y + 10.0), egui::Align2::LEFT_CENTER, "Sharper picture, at twice the CPU", egui::FontId::proportional(12.0), MUTED);
                let sw_rect = Rect::from_center_size(pos2(row.right() - 24.0, row.center().y), vec2(44.0, 24.0));
                let on = self.quality.high_quality;
                let k = ui.ctx().animate_bool_with_time(resp.id, on, QUICK);
                ui.painter().rect_filled(sw_rect, 12.0, k_raised().lerp_to_gamma(pal.accent, k));
                ui.painter().circle_filled(pos2(sw_rect.left() + 12.0 + 20.0 * k, sw_rect.center().y), 9.0, if on { pal.on_accent } else { SECONDARY });
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    self.quality.high_quality = !on;
                }
                ui.add_space(8.0);
                // footer: two equal buttons on the right
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    ui.add_space(W - 2.0 * 128.0 - 10.0);
                    if footer_button(ui, "Cancel", false, true, pal) {
                        close = true;
                    }
                    if footer_button(ui, "Go Live", true, self.picked.is_some(), pal) {
                        go = true;
                    }
                });
            });
        if modal.should_close() {
            close = true;
        }
        if go {
            if let Some(src) = self.picked.take() {
                sw::pick(src);
                acts.push(Action::Voice(Vc::GoLive(self.quality)));
            }
            close = true;
        }
        if close {
            self.open = false;
            self.thumbs.clear();
        }
    }
}

/// A label on the left and a row of equal segments on the right; returns the segment clicked.
fn setting_row(ui: &mut Ui, label: &str, items: &[&str], selected: usize, pal: th::Palette) -> Option<usize> {
    let (row, _) = ui.allocate_exact_size(vec2(560.0, 38.0), Sense::hover());
    ui.painter().text(row.left_center(), egui::Align2::LEFT_CENTER, label, egui::FontId::new(13.5, th::semibold()), TEXT);
    let bar = Rect::from_min_size(pos2(row.left() + 150.0, row.top()), vec2(row.width() - 150.0, 38.0));
    ui.painter().rect_filled(bar, 9.0, k_raised());
    let w = bar.width() / items.len() as f32;
    let mut hit = None;
    for (i, name) in items.iter().enumerate() {
        let seg = Rect::from_min_size(pos2(bar.left() + w * i as f32, bar.top()), vec2(w, 38.0)).shrink(3.0);
        let resp = ui.interact(seg, ui.id().with(("seg", label, i)), Sense::click());
        let on = i == selected;
        if on {
            ui.painter().rect_filled(seg, 7.0, pal.accent);
        } else if resp.hovered() {
            ui.painter().rect_filled(seg, 7.0, k_hover());
        }
        ui.painter().text(seg.center(), egui::Align2::CENTER_CENTER, *name, egui::FontId::new(13.0, th::semibold()), if on { pal.on_accent } else { SECONDARY });
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            hit = Some(i);
        }
    }
    hit
}

/// A 128 by 38 button: accent when primary, flat otherwise; a disabled one does not click.
fn footer_button(ui: &mut Ui, text: &str, primary: bool, enabled: bool, pal: th::Palette) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(128.0, 38.0), if enabled { Sense::click() } else { Sense::hover() });
    let (fill, fg) = match (primary, enabled) {
        (true, true) => (if resp.hovered() { pal.accent_dark.lerp_to_gamma(pal.accent, 0.5) } else { pal.accent }, pal.on_accent),
        (true, false) => (pal.accent.gamma_multiply(0.3), pal.on_accent.gamma_multiply(0.6)),
        _ => (if resp.hovered() { k_hover() } else { k_raised() }, TEXT),
    };
    ui.painter().rect_filled(rect, 9.0, fill);
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::new(13.5, th::semibold()), fg);
    enabled && resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// A stream (someone's, or this account's own shared screen) over the whole window, the window itself fullscreen.
/// The small button at the bottom right leaves it; Esc and a double click do too.
pub fn fullscreen(ctx: &egui::Context, cu: &mut CallUi, s: &AppState, acts: &mut Vec<Action>) {
    if !cu.full && !cu.full_own {
        return;
    }
    let leave = |cu: &mut CallUi| {
        cu.full = false;
        cu.full_own = false;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(false));
    };
    let v = &s.voice;
    // what fills the window: this account's own share, or the stream being watched; none left ends it
    let (frames, title, opening) = if cu.full_own {
        match &v.sharing {
            Some(sh) => (Some((&sh.frames, "share")), "Your screen".to_owned(), false),
            None => return leave(cu),
        }
    } else {
        match &v.watching {
            Some(w) => (w.frames.as_ref().map(|f| (f, "watch")), format!("{}'s screen", w.name), w.opening),
            None => return leave(cu),
        }
    };
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        return leave(cu);
    }
    let rect = ctx.content_rect();
    let tex = frames.and_then(|(f, key)| cu.frames_texture(ctx, key, f));
    let (mut exit, own) = (false, cu.full_own);
    egui::Area::new(egui::Id::new("watch_fullscreen")).order(egui::Order::Foreground).fixed_pos(rect.min).show(ctx, |ui| {
        ui.set_clip_rect(rect);
        let (r, resp) = ui.allocate_exact_size(rect.size(), Sense::click());
        ui.painter().rect_filled(r, 0.0, Color32::BLACK);
        match tex {
            Some(t) if !opening => cover(ui, r, t, 0.0, true),
            _ => {
                cu.live = true;
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "Opening stream\u{2026}", egui::FontId::proportional(15.0), SECONDARY);
            }
        }
        name_chip(ui, r.shrink(8.0), &title, false, false, false);
        if !own && tile_pill(ui, pos2(r.right() - 16.0, r.top() + 16.0), "Stop watching", DANGER, egui::Id::new("fs_stop")) {
            acts.push(Action::Voice(Vc::StopWatching));
            exit = true;
        }
        if resp.double_clicked() {
            exit = true;
        }
        // the small button at the bottom right: back to the window
        let b = Rect::from_center_size(pos2(r.right() - 34.0, r.bottom() - 34.0), vec2(40.0, 40.0));
        let be = ui.interact(b, egui::Id::new("fs_exit"), Sense::click());
        ui.painter().circle_filled(b.center(), 20.0, if be.hovered() { Color32::from_black_alpha(0xE0) } else { Color32::from_black_alpha(0x99) });
        ui.painter().circle_stroke(b.center(), 20.0, Stroke::new(1.0, Color32::from_white_alpha(0x30)));
        glyph(ui.painter(), b.center(), "\u{E73F}", 16.0, Color32::WHITE, false);
        if be.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Exit fullscreen").clicked() {
            exit = true;
        }
    });
    if exit {
        leave(cu);
    }
}

/// The windows the call UI may have open: devices and the Go Live picker.
pub fn windows(ctx: &egui::Context, cu: &mut CallUi, s: &AppState, acts: &mut Vec<Action>) {
    if cu.audio_open {
        devices_window(ctx, s, acts, &mut cu.audio_open);
    }
    cu.share.window(ctx, acts);
    fullscreen(ctx, cu, s, acts);
}

#[cfg(test)]
mod tests {
    use super::*;


    /// The Go Live picker with sample screens and windows: writes share.png to SHOT_DIR (nothing is shared).
    #[test]
    fn share_picker_shot() {
        use egui_kittest::Harness;
        let mut menu = ShareMenu::default();
        menu.open = true;
        menu.picked = Some(Source::Display(1));
        {
            let mut l = menu.listed.lock().unwrap();
            l.done = true;
            let sh = |source, t: &str| Shareable { source, title: t.into() };
            l.screens = vec![sh(Source::Display(1), "Screen 1"), sh(Source::Display(2), "Screen 2")];
            l.windows = vec![sh(Source::Window(10), "@someone - Discord"), sh(Source::Window(11), "Claude"), sh(Source::Window(12), "SoundCloud - Music & Audio")];
        }
        let mut first = true;
        let mut h = Harness::builder().with_size(vec2(760.0, 820.0)).build_ui(move |ui| {
            if first {
                crate::theme::install_fonts(ui.ctx());
                crate::theme::apply_visuals(ui.ctx());
                first = false;
            }
            menu.window(ui.ctx(), &mut Vec::new());
        });
        h.run_steps(10);
        let out = std::env::var("SHOT_DIR").unwrap_or_else(|_| ".".into());
        h.render().expect("render").save(format!("{out}/share.png")).unwrap();
    }
    #[test]
    fn grid_fits_and_centres() {
        let area = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 400.0));
        let one = grid(1, area, 12.0);
        assert_eq!(one.len(), 1);
        assert!((one[0].height() - 400.0).abs() < 0.5 && (one[0].center().x - 500.0).abs() < 0.5);
        let five = grid(5, area, 12.0);
        assert_eq!(five.len(), 5);
        for r in &five {
            assert!(area.expand(0.5).contains_rect(*r), "{r:?} outside");
            assert!((r.width() / r.height() - 16.0 / 9.0).abs() < 0.01);
        }
        // no two tiles overlap
        for (i, a) in five.iter().enumerate() {
            for b in &five[i + 1..] {
                assert!(!a.shrink(0.5).intersects(*b));
            }
        }
        assert!(grid(0, area, 12.0).is_empty());
    }
}

pub(super) fn k_raised() -> Color32 { th::tone(0.18) }
pub(super) fn k_panel() -> Color32 { th::tone(0.085) }
pub(super) fn k_edge() -> Color32 { th::tone(0.05) }
