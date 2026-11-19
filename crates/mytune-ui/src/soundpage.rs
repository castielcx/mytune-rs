//! Sound page: a volume mixer. One card per output device (master volume, mute, level, default device),
//! one row per app playing on it (volume, mute, level, move to another device).
//! `mixer` draws the whole thing, so a tray flyout can reuse it.

use crate::theme::{self as th, p, rt, sb, text2};
use crate::widgets;
use eframe::egui::{self, pos2, vec2, Align, Color32, FontId, Id, Layout, Rect, Sense, Ui};
use mytune_core::audio::{Cmd, Device, Mixer, Msg};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const SLIDER_W: f32 = 160.0;
/// A tick: lit on the default device, a button on the others. True when clicked (make it the default).
fn default_tick(ui: &mut Ui, is_default: bool) -> bool {
    let (b, mut resp) = ui.allocate_exact_size(vec2(28.0, 28.0), if is_default { Sense::hover() } else { Sense::click_and_drag() });
    let b = crate::theme::pulled(b, crate::theme::pull(ui, &mut resp));
    if resp.hovered() && !is_default {
        ui.painter().rect_filled(b, 14.0, Color32::from_white_alpha(18));
    }
    let col = if is_default { p().accent } else { Color32::from_white_alpha(0x60) };
    ui.painter().text(b.center(), egui::Align2::CENTER_CENTER, "\u{E73E}", FontId::new(14.0, th::icons()), col);
    if is_default {
        resp.on_hover_text("Default device");
        false
    } else {
        resp.on_hover_text("Make this the default device").on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
    }
}
const TICK_W: f32 = 28.0; // flyout: the default tick at the end of a device row

/// Where the speaker tray icon was clicked, in screen pixels (centre x, top y), until `Flyout` takes it.
static FLYOUT_ASK: Mutex<Option<(f32, f32)>> = Mutex::new(None);

/// Called by the tray icon: open the flyout above the icon, or close it when it is open.
pub fn ask_flyout(x: f32, top: f32) {
    *FLYOUT_ASK.lock().unwrap_or_else(|p| p.into_inner()) = Some((x, top));
}

/// White speaker with two sound waves for the tray, `size` x `size` RGBA.
pub fn speaker(size: usize) -> Vec<u8> {
    let s = size as f32 / 32.0; // drawn on a 32 grid
    let inside = |x: f32, y: f32| {
        let (u, v) = (x / s, y / s);
        let dy = (v - 16.0).abs();
        let body = (5.0..=10.0).contains(&u) && dy <= 4.0;
        let cone = (10.0..=17.0).contains(&u) && dy <= 4.0 + (u - 10.0) * 6.0 / 7.0;
        let d = ((u - 17.0).powi(2) + (v - 16.0).powi(2)).sqrt();
        let wave = u > 18.0 && dy < (u - 17.0) * 1.2 && ((d - 6.0).abs() <= 1.1 || (d - 10.5).abs() <= 1.1);
        body || cone || wave
    };
    let mut out = vec![0u8; size * size * 4];
    for y in 0..size {
        for x in 0..size {
            // 4x4 samples per pixel for smooth edges
            let hits = (0..16).filter(|i| inside(x as f32 + (i % 4) as f32 / 4.0 + 0.125, y as f32 + (i / 4) as f32 / 4.0 + 0.125)).count();
            let a = (hits * 255 / 16) as u8;
            out[(y * size + x) * 4..][..4].copy_from_slice(&[255, 255, 255, a]);
        }
    }
    out
}

/// The mixer as a small window above the tray icon. Closes on a click elsewhere, Esc or a second icon click.
pub struct Flyout {
    page: SoundPage,
    /// The tray icon while open, in points: centre x and top y. The window hangs above it.
    open: Option<(f32, f32)>,
    /// Frames drawn since it was opened with the device list loaded; the window is shown from the third,
    /// when its height has settled on what the rows need.
    frames: u32,
    /// Height the content asked for in the last frame.
    height: f32,
    had_focus: bool,
    opened: Instant,
    closed: Instant,
}

impl Flyout {
    const WIDTH: f32 = 340.0;
    const MAX_HEIGHT: f32 = 520.0;

    pub fn new() -> Self {
        Self { page: SoundPage { compact: true, ..SoundPage::new() }, open: None, frames: 0, height: 160.0, had_focus: false, opened: Instant::now(), closed: Instant::now() }
    }

    /// Call every frame of the main window, also while that window is hidden in the tray.
    pub fn show(&mut self, ctx: &egui::Context) {
        if let Some((x, top)) = FLYOUT_ASK.lock().unwrap_or_else(|p| p.into_inner()).take() {
            if self.open.is_some() {
                self.close();
            } else if self.closed.elapsed() > Duration::from_millis(300) {
                // (the icon click that took the focus away has just closed it: do not open it again)
                // shortcut: uses the main window's scale and screen size, so a taskbar on a second screen
                // with another scale gets a slightly off position; use that monitor's values if it matters
                let ppp = ctx.pixels_per_point();
                self.open = Some((x / ppp, top / ppp));
                self.had_focus = false;
                self.frames = 0;
                self.opened = Instant::now();
            }
        }
        // The window lives hidden for the whole run and is only shown and hidden: creating it on each click
        // flashed an empty window on screen for about a second. (Tests embed viewports: nothing to keep there.)
        if self.open.is_none() && ctx.embed_viewports() {
            return;
        }
        let id = egui::ViewportId::from_hash_of("sound-flyout");
        // shown only once the mixer has been drawn at the new place, so no empty or stale frame is seen
        let mut builder = egui::ViewportBuilder::default()
            .with_title("MyTune volume")
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_always_on_top()
            .with_inner_size(vec2(Self::WIDTH, self.height))
            .with_visible(self.open.is_some() && self.frames >= 2);
        if let Some((x, top)) = self.open {
            let mut left = x - Self::WIDTH / 2.0;
            if let Some(screen) = ctx.input(|i| i.viewport().monitor_size) {
                left = left.min(screen.x - Self::WIDTH - 12.0);
            }
            builder = builder.with_position(pos2(left.max(12.0), (top - self.height - 12.0).max(12.0)));
        }
        let mut close = false;
        ctx.show_viewport_immediate(id, builder, |ui, _| {
            ui.painter().rect_filled(ui.max_rect(), 0.0, th::tone(0.10));
            if self.open.is_none() {
                return;
            }
            let (focused, esc, asked) = ui.ctx().input(|i| (i.viewport().focused, i.key_pressed(egui::Key::Escape), i.viewport().close_requested()));
            self.had_focus |= focused == Some(true);
            close = esc || asked || (self.had_focus && focused == Some(false));
            // the list takes the height its rows need (it scrolls past the maximum); the window follows it
            let top = ui.max_rect().top();
            egui::Frame::new().inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                let head = ui.cursor().top() - top;
                let list = egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.page.mixer(ui));
                // `content_size` is what the rows need, whatever the window's height is now
                self.height = (head + list.content_size.y + 12.0).clamp(100.0, Self::MAX_HEIGHT).round();
            });
        });
        if close {
            self.close();
        } else if self.open.is_some() && self.frames < 4 {
            // wait for the device list (2 s at most), so the window does not appear small and then grow
            if self.page.loaded || self.opened.elapsed() > Duration::from_secs(2) {
                self.frames += 1;
            }
            if self.frames == 3 {
                ctx.send_viewport_cmd_to(id, egui::ViewportCommand::Focus); // shown now: a click elsewhere must close it
            }
            ctx.request_repaint();
        }
    }

    fn close(&mut self) {
        self.open = None;
        self.closed = Instant::now();
        self.page.stop();
    }
}

pub struct SoundPage {
    mixer: Option<Mixer>,
    rx: Option<Receiver<Msg>>,
    devices: Vec<Device>,
    loaded: bool,
    error: Option<String>,
    /// exe path -> name and icon
    apps: HashMap<String, (String, Option<egui::TextureHandle>)>,
    /// The slider being dragged keeps its own value for a moment, so an older snapshot cannot pull it back.
    hold: Option<(String, f64, Instant)>,
    /// Flyout layout: narrower sliders, icon buttons, no "Move to" (that stays on the page).
    compact: bool,
}

/// False while MyTune's window is hidden in the tray or minimized (true when there is no window: tests).
fn on_screen() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, IsIconic, IsWindowVisible};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    unsafe {
        let w = FindWindowW(wide("Window Class").as_ptr(), wide("MyTune").as_ptr()); // as `already_running` in main.rs
        w.is_null() || (IsWindowVisible(w) != 0 && IsIconic(w) == 0)
    }
}

struct Controls {
    volume: Option<f32>,
    mute: bool,
}

impl SoundPage {
    pub fn new() -> Self {
        Self { mixer: None, rx: None, devices: Vec::new(), loaded: false, error: None, apps: HashMap::new(), hold: None, compact: false }
    }

    /// Ends the audio worker; called while another page is shown.
    pub fn stop(&mut self) {
        self.mixer = None;
        self.rx = None;
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        if !on_screen() {
            return self.stop(); // no audio calls while nobody can see the page
        }
        self.mixer(ui);
    }

    pub fn mixer(&mut self, ui: &mut Ui) {
        if self.mixer.is_none() {
            let (tx, rx) = mpsc::channel();
            let ctx = ui.ctx().clone();
            self.mixer = Some(Mixer::start(tx, move || ctx.request_repaint()));
            self.rx = Some(rx);
        }
        if let Some(m) = &self.mixer {
            m.poke();
        }
        let msgs: Vec<Msg> = self.rx.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for m in msgs {
            match m {
                Msg::Snapshot(d) => {
                    self.devices = d;
                    self.loaded = true;
                }
                Msg::Error(e) => self.error = Some(e),
            }
        }
        if let Some(e) = &self.error {
            ui.label(rt(e, 12.0).color(th::DANGER));
            ui.add_space(8.0);
        }
        if self.devices.is_empty() {
            th::card(ui, (10, 16, 16, 16), |ui| {
                ui.label(rt(if self.loaded { "No sound output device found." } else { "Looking for sound devices\u{2026}" }, 14.0).color(text2()));
            });
            return;
        }

        let devices = self.devices.clone();
        let mut cmds: Vec<Cmd> = Vec::new();
        let compact = self.compact;
        if compact {
            self.slim(ui, &devices, &mut cmds);
        }
        for d in devices.iter().filter(|_| !compact) {
            th::card(ui, (10, 16, 16, 16), |ui| {
                ui.horizontal(|ui| {
                    let name_w = (ui.available_width() - 470.0).max(80.0);
                    ui.allocate_ui_with_layout(vec2(name_w, 28.0), Layout::left_to_right(Align::Center), |ui| {
                        ui.add(egui::Label::new(sb(&d.name, 14.0)).truncate());
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let c = self.controls(ui, &d.id, d.volume, d.muted, d.peak);
                        if let Some(volume) = c.volume {
                            cmds.push(Cmd::DeviceVolume { device: d.id.clone(), volume });
                        }
                        if c.mute {
                            cmds.push(Cmd::DeviceMute { device: d.id.clone(), muted: !d.muted });
                        }
                        ui.add_space(8.0);
                        if default_tick(ui, d.is_default) {
                            cmds.push(Cmd::SetDefault { device: d.id.clone() });
                        }
                    });
                });
                if d.apps.is_empty() {
                    ui.add_space(4.0);
                    ui.label(rt("No apps playing here.", 12.0).color(text2()));
                }
                for a in &d.apps {
                    ui.add_space(4.0);
                    let key = format!("{}|{}|{}", d.id, a.path, a.pids[0]);
                    let (name, icon) = self.app(ui.ctx(), a.system, &a.path, a.pids[0]);
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(20.0, 28.0), Sense::hover());
                        let slot = Rect::from_center_size(r.center(), vec2(20.0, 20.0));
                        match icon {
                            Some(id) => {
                                ui.painter().image(id, slot, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                            }
                            None => {
                                ui.painter().text(slot.center(), egui::Align2::CENTER_CENTER, "\u{E7F5}", FontId::new(16.0, th::icons()), text2());
                            }
                        }
                        let name_w = (ui.available_width() - 470.0).max(80.0);
                        ui.allocate_ui_with_layout(vec2(name_w, 28.0), Layout::left_to_right(Align::Center), |ui| {
                            ui.add(egui::Label::new(rt(name, 14.0)).truncate());
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let c = self.controls(ui, &key, a.volume, a.muted, a.peak);
                            if let Some(volume) = c.volume {
                                cmds.push(Cmd::AppVolume { device: d.id.clone(), pids: a.pids.clone(), volume });
                            }
                            if c.mute {
                                cmds.push(Cmd::AppMute { device: d.id.clone(), pids: a.pids.clone(), muted: !a.muted });
                            }
                            // Windows' own sounds have no process to move
                            if !a.system && devices.len() > 1 {
                                ui.add_space(8.0);
                                let others: Vec<&Device> = devices.iter().filter(|o| o.id != d.id).collect();
                                let rows: Vec<(String, bool)> = others.iter().map(|o| (o.name.clone(), false)).collect();
                                if let Some(i) = widgets::dropdown_text(ui, &format!("move|{key}"), "Move to", &rows) {
                                    cmds.push(Cmd::Move { pids: a.pids.clone(), device: others[i].id.clone() });
                                }
                            }
                        });
                    });
                }
            });
            ui.add_space(8.0);
        }
        if !cmds.is_empty() {
            self.error = None;
        }
        if let Some(m) = &self.mixer {
            for c in cmds {
                m.send(c);
            }
        }
    }

    /// The flyout's layout, as Windows' own volume mixer (user's choice): no boxes, no app names. Per device
    /// a small name line, then a row for the device and one per app: icon (a click mutes, the name is its
    /// tooltip), thin slider, percent. A device row ends in its default tick. No "Move to" here (user's
    /// choice): moving an app is on the Sound page.
    fn slim(&mut self, ui: &mut Ui, devices: &[Device], cmds: &mut Vec<Cmd>) {
        for (n, d) in devices.iter().enumerate() {
            if n > 0 {
                ui.add_space(4.0);
                let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
                ui.painter().rect_filled(line, 0.0, Color32::from_white_alpha(0x14));
                ui.add_space(4.0);
            }
            ui.add(egui::Label::new(rt(&d.name, 12.0).color(text2())).truncate());
            ui.horizontal(|ui| {
                let c = self.slim_row(ui, &d.id, &d.name, None, "\u{E767}", d.volume, d.muted);
                if let Some(volume) = c.volume {
                    cmds.push(Cmd::DeviceVolume { device: d.id.clone(), volume });
                }
                if c.mute {
                    cmds.push(Cmd::DeviceMute { device: d.id.clone(), muted: !d.muted });
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if default_tick(ui, d.is_default) {
                        cmds.push(Cmd::SetDefault { device: d.id.clone() });
                    }
                });
            });
            for a in &d.apps {
                let key = format!("{}|{}|{}", d.id, a.path, a.pids[0]);
                let (name, icon) = self.app(ui.ctx(), a.system, &a.path, a.pids[0]);
                ui.horizontal(|ui| {
                    let c = self.slim_row(ui, &key, &name, icon, "\u{E7F5}", a.volume, a.muted);
                    if let Some(volume) = c.volume {
                        cmds.push(Cmd::AppVolume { device: d.id.clone(), pids: a.pids.clone(), volume });
                    }
                    if c.mute {
                        cmds.push(Cmd::AppMute { device: d.id.clone(), pids: a.pids.clone(), muted: !a.muted });
                    }
                });
            }
        }
    }

    /// Icon (a click mutes; `glyph` when there is no picture), slider, percent. `TICK_W` stays free
    /// at the right for the caller.
    fn slim_row(&mut self, ui: &mut Ui, key: &str, name: &str, icon: Option<egui::TextureId>, glyph: &str, volume: f32, muted: bool) -> Controls {
        let (b, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(b, 14.0, Color32::from_white_alpha(18));
        }
        let slot = Rect::from_center_size(b.center(), vec2(20.0, 20.0));
        let tint = if muted { Color32::from_white_alpha(0x50) } else { Color32::WHITE };
        match icon {
            Some(id) => {
                ui.painter().image(id, slot, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), tint);
            }
            None => {
                ui.painter().text(slot.center(), egui::Align2::CENTER_CENTER, glyph, FontId::new(16.0, th::icons()), tint);
            }
        }
        if muted {
            ui.painter().text(slot.right_bottom(), egui::Align2::CENTER_CENTER, "\u{E74F}", FontId::new(11.0, th::icons()), th::DANGER);
        }
        let mute = resp.on_hover_text(format!("{name}\nClick to {}", if muted { "unmute" } else { "mute" })).on_hover_cursor(egui::CursorIcon::PointingHand).clicked();

        let gap = ui.spacing().item_spacing.x;
        let width = (ui.available_width() - 36.0 - TICK_W - 3.0 * gap).max(60.0);
        let mut v = match &self.hold {
            Some((k, v, t)) if k == key && t.elapsed() < Duration::from_millis(400) => *v,
            _ => (volume * 100.0).round() as f64,
        };
        let out = widgets::slider_sized(ui, Id::new(("sound", key)), &mut v, 0.0, 100.0, width, true, true);
        let (pr, _) = ui.allocate_exact_size(vec2(36.0, 20.0), Sense::hover());
        ui.painter().text(pr.right_center(), egui::Align2::RIGHT_CENTER, format!("{}%", v.round()), FontId::proportional(13.0), text2());
        let mut volume = None;
        if out.changed {
            self.hold = Some((key.to_owned(), v, Instant::now()));
            volume = Some(v as f32 / 100.0);
        }
        Controls { volume, mute }
    }

    /// Name and icon texture of an app row, read once per exe.
    fn app(&mut self, ctx: &egui::Context, system: bool, path: &str, pid: u32) -> (String, Option<egui::TextureId>) {
        if system {
            return ("System sounds".into(), None);
        }
        if path.is_empty() {
            return (format!("Process {pid}"), None);
        }
        let (name, tex) = self.apps.entry(path.to_owned()).or_insert_with(|| {
            // Store apps: Start menu name and logo; everything else: the exe's description and icon
            let (name, icon) = match mytune_core::audio::packaged(pid) {
                Some(l) => (l.name, l.icon.map(|(w, h, px)| egui::ColorImage::from_rgba_premultiplied([w, h], &px))),
                None => {
                    let app = crate::appinfo::of_path(path);
                    (app.name, app.icon)
                }
            };
            (name, icon.map(|i| ctx.load_texture(format!("snd:{path}"), i, th::SMOOTH)))
        });
        (name.clone(), tex.as_ref().map(|t| t.id()))
    }

    /// Right-to-left: level meter, percent, slider, mute button.
    fn controls(&mut self, ui: &mut Ui, key: &str, volume: f32, muted: bool, peak: f32) -> Controls {
        let pal = p();
        let (r, _) = ui.allocate_exact_size(vec2(48.0, 6.0), Sense::hover());
        ui.painter().rect_filled(r, 3.0, Color32::from_white_alpha(0x29));
        let lit = Rect::from_min_size(r.min, vec2(r.width() * peak.clamp(0.0, 1.0), r.height()));
        ui.painter().rect_filled(lit, 3.0, if muted { text2() } else { pal.accent });

        let mut v = match &self.hold {
            Some((k, v, t)) if k == key && t.elapsed() < Duration::from_millis(400) => *v,
            _ => (volume * 100.0).round() as f64,
        };
        let (pr, _) = ui.allocate_exact_size(vec2(36.0, 20.0), Sense::hover());
        let out_slot = ui.painter().add(egui::Shape::Noop); // percent text is laid out after the slider moved
        let out = widgets::slider(ui, Id::new(("sound", key)), &mut v, 0.0, 100.0, SLIDER_W, true);
        let text = ui.painter().layout_no_wrap(format!("{}%", v.round()), FontId::proportional(13.0), text2());
        ui.painter().set(out_slot, egui::Shape::galley(pos2(pr.right() - text.size().x, pr.center().y - text.size().y / 2.0), text, text2()));
        let mut volume = None;
        if out.changed {
            self.hold = Some((key.to_owned(), v, Instant::now()));
            volume = Some(v as f32 / 100.0);
        }

        let (b, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(b, 14.0, Color32::from_white_alpha(18));
        }
        let glyph = if muted { "\u{E74F}" } else { "\u{E767}" };
        ui.painter().text(b.center(), egui::Align2::CENTER_CENTER, glyph, FontId::new(15.0, th::icons()), if muted { th::DANGER } else { th::TEXT });
        let mute = resp.on_hover_text(if muted { "Unmute" } else { "Mute" }).on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
        Controls { volume, mute }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn speaker_icon_has_a_shape() {
        let px = super::speaker(32);
        let alpha = |x: usize, y: usize| px[(y * 32 + x) * 4 + 3];
        assert_eq!(px.len(), 32 * 32 * 4);
        assert_eq!(alpha(0, 0), 0, "corner is empty");
        assert_eq!(alpha(7, 16), 255, "body is solid");
        assert_eq!(alpha(23, 16), 255, "inner wave crosses the middle line");
        assert_eq!(alpha(20, 16), 0, "gap between cone and wave");
    }
}
