//! Sound page: a volume mixer. One card per output device (master volume, mute, level, default device),
//! one row per app playing on it (volume, mute, level, move to another device).
//! `mixer` draws the whole thing, so a tray flyout can reuse it.

use crate::theme::{self as th, p, rt, sb, text2};
use crate::widgets;
use eframe::egui::{self, pos2, vec2, Align, Color32, FontId, Id, Layout, Rect, Sense, Ui};
use mytune_core::audio::{Cmd, Device, Mixer, Msg};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const SLIDER_W: f32 = 160.0;
const MOVE_W: f32 = 200.0;

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
        Self { mixer: None, rx: None, devices: Vec::new(), loaded: false, error: None, apps: HashMap::new(), hold: None }
    }

    /// Ends the audio worker; called while another page is shown.
    pub fn stop(&mut self) {
        self.mixer = None;
        self.rx = None;
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        ui.label(rt("Volume for every app, and which device plays it.", 12.0).color(text2()));
        ui.add_space(12.0);
        self.mixer(ui);
    }

    pub fn mixer(&mut self, ui: &mut Ui) {
        if !on_screen() {
            return self.stop(); // no audio calls while nobody can see the page
        }
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
        for d in &devices {
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
                        if d.is_default {
                            ui.label(rt("Default", 12.0).color(p().accent));
                        } else if th::button(ui, "Set default", false, th::BTN).clicked() {
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
                                if let Some(i) = widgets::dropdown(ui, &format!("move|{key}"), MOVE_W, "Move to\u{2026}", &rows) {
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
