//! Display page, laid out like `Views/DisplayPage.xaml`: monitor pill track, resolution + refresh pick lists
//! with one Apply (keep-or-revert dialog), colour calibration sliders, NVIDIA vibrance.
//! Display calls are quick, so they run on the UI thread like the C# page.

use crate::theme::{self as th, bd, rt, text2};
use crate::widgets::{self as w, caption, caption3};
use eframe::egui::{self, Id, Layout, Align, Ui};
use mytune_core::display::{self, DisplayMode, DisplayProfile, Monitor};
use mytune_core::engine::Engine;
use mytune_core::vibrance::VibranceRange;
use std::sync::Arc;
use std::time::{Duration, Instant};

const KEEP_SECONDS: u64 = 15;

struct Trial {
    dev: String,
    old: DisplayMode,
    new: DisplayMode,
    since: Instant,
}

pub struct DisplayPage {
    engine: Arc<Engine>,
    monitors: Vec<Monitor>,
    sel: usize,
    now: Option<DisplayMode>,
    modes: Vec<DisplayMode>,
    pick_res: (u32, u32),
    pick_hz: u32,
    profile: DisplayProfile,
    vib: Option<VibranceRange>,
    vib_pct: f64,
    loaded: bool,
    status: Option<(String, bool)>,
    trial: Option<Trial>,
}

/// C# number formats: "0.00", "+0;-0;0" and "0", plus the unit.
fn fmt_gamma(v: f64) -> String {
    format!("{v:.2}")
}
fn fmt_signed(v: f64) -> String {
    let r = v.round_ties_even() as i64;
    if r > 0 { format!("+{r}%") } else { format!("{r}%") }
}
fn fmt_pct(v: f64) -> String {
    format!("{}%", v.round_ties_even() as i64)
}

impl DisplayPage {
    pub fn new(engine: Arc<Engine>) -> DisplayPage {
        DisplayPage {
            engine,
            monitors: vec![],
            sel: 0,
            now: None,
            modes: vec![],
            pick_res: (0, 0),
            pick_hz: 0,
            profile: DisplayProfile::default(),
            vib: None,
            vib_pct: 50.0,
            loaded: false,
            status: None,
            trial: None,
        }
    }

    fn dev(&self) -> Option<String> {
        self.monitors.get(self.sel).map(|m| m.device.clone())
    }

    fn load(&mut self) {
        self.monitors = display::monitors();
        self.sel = self.sel.min(self.monitors.len().saturating_sub(1));
        self.choose();
    }

    fn choose(&mut self) {
        let Some(dev) = self.dev() else { return };
        if let Some((now, all)) = display::get_modes(&dev) {
            self.pick_res = (now.width, now.height);
            self.pick_hz = now.hz;
            self.now = Some(now);
            self.modes = all;
        } else {
            self.now = None;
            self.modes.clear();
        }
        self.profile = self.engine.with_store(|s| display::get_profile(s, &dev));
        self.vib = display::get_vibrance(&dev);
        if let Some(r) = self.vib {
            self.vib_pct = self.profile.vibrance.unwrap_or_else(|| r.percent_for());
        }
    }

    fn rates(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.modes.iter().filter(|m| (m.width, m.height) == self.pick_res).map(|m| m.hz).collect();
        v.dedup(); // modes are sorted, rates descending within a resolution
        v
    }

    fn resolutions(&self) -> Vec<(u32, u32)> {
        let mut v: Vec<(u32, u32)> = self.modes.iter().map(|m| (m.width, m.height)).collect();
        v.dedup();
        v
    }

    fn apply(&mut self) {
        let (Some(dev), Some(old)) = (self.dev(), self.now) else { return };
        let new = DisplayMode { width: self.pick_res.0, height: self.pick_res.1, hz: self.pick_hz };
        if !self.modes.contains(&new) {
            return;
        }
        if new == old {
            self.status = Some(("The display already uses this mode.".into(), true));
            return;
        }
        match display::set_mode(&dev, new, false) {
            Ok(()) => self.trial = Some(Trial { dev, old, new, since: Instant::now() }),
            Err(e) => self.status = Some((e.to_string(), false)),
        }
    }

    fn finish_trial(&mut self, keep: bool) {
        let Some(t) = self.trial.take() else { return };
        let r = display::set_mode(&t.dev, if keep { t.new } else { t.old }, keep);
        self.status = Some(match (r, keep) {
            (Ok(()), true) => ("Display mode saved.".into(), true),
            (Ok(()), false) => ("Switched back to the old mode.".into(), true),
            (Err(e), true) => (format!("Couldn't save the mode: {e}"), false),
            // honest: the old mode may not be back (a restart restores it, the change was never saved)
            (Err(e), false) => (format!("Couldn't switch back: {e}. A restart restores the old mode."), false),
        });
        self.choose();
    }

    fn trial_modal(&mut self, ctx: &egui::Context) {
        let Some(t) = &self.trial else { return };
        let left = KEEP_SECONDS.saturating_sub(t.since.elapsed().as_secs());
        let text = format!("{} at {} Hz", t.new.label(), t.new.hz);
        let mut keep = None;
        egui::Modal::new(Id::new("keepdisplay")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(bd("Keep this display mode?", 18.0));
            ui.add_space(6.0);
            ui.label(rt(format!("{text}. Going back in {left} s if you do nothing."), 13.0).color(text2()));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if th::button(ui, "Keep", true, th::BTN).clicked() {
                    keep = Some(true);
                }
                if th::button(ui, "Revert", false, th::BTN).clicked() {
                    keep = Some(false);
                }
            });
        });
        if keep.is_none() && left == 0 {
            keep = Some(false);
        }
        ctx.request_repaint_after(Duration::from_millis(250));
        if let Some(k) = keep {
            self.finish_trial(k);
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        if !self.loaded {
            self.loaded = true;
            self.load();
        }
        let ctx = ui.ctx().clone();
        self.trial_modal(&ctx);
        caption(ui, "Resolution, refresh rate and colors for each monitor. Color changes apply live.");
        if let Some((msg, ok)) = &self.status {
            ui.add_space(6.0);
            ui.label(rt(msg, 12.0).color(if *ok { th::SUCCESS } else { th::DANGER }));
        }
        if self.monitors.is_empty() {
            ui.add_space(18.0);
            caption(ui, "No active displays found.");
            return;
        }
        if self.monitors.len() > 1 {
            ui.add_space(18.0);
            self.monitor_track(ui);
        }
        self.mode_card(ui);
        self.color_cards(ui);
        ui.add_space(14.0);
        caption3(ui, "Windows clears the color curve after a restart. This version does not re-apply your values by itself: press Apply saved colors. Games, f.lux or NVIDIA Control Panel color settings can override them.");
    }

    /// One pill track (ControlBg, padding 3); the chosen monitor is an AccentButton, the others SubtleButtons.
    fn monitor_track(&mut self, ui: &mut Ui) {
        let mut pick = None;
        egui::Frame::new().fill(th::control_bg()).inner_margin(3).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (i, m) in self.monitors.iter().enumerate() {
                    let clicked = if i == self.sel { th::button(ui, &m.label, true, th::BTN).clicked() } else { w::subtle_button(ui, &m.label, 14.0, th::BTN).clicked() };
                    if clicked {
                        pick = Some(i);
                    }
                }
            });
        });
        if let Some(i) = pick {
            self.sel = i;
            self.choose();
        }
    }

    fn mode_card(&mut self, ui: &mut Ui) {
        ui.add_space(24.0);
        ui.label(th::dsb("Resolution and refresh rate", 13.0).color(th::TEXT));
        ui.add_space(8.0);
        let mut apply = false;
        let mut pick_res = None;
        let mut pick_hz = None;
        th::card(ui, (12, 16, 18, 16), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                ui.vertical(|ui| {
                    ui.add_space(0.0);
                    ui.label(rt("Resolution", 12.0).color(text2()));
                    ui.add_space(6.0);
                    let rows: Vec<(String, bool)> = self.resolutions().into_iter().map(|r| (format!("{} x {}", r.0, r.1), r == self.pick_res)).collect();
                    let res = self.resolutions();
                    pick_res = w::dropdown(ui, "res", 210.0, &format!("{} x {}", self.pick_res.0, self.pick_res.1), &rows).map(|i| res[i]);
                });
                ui.vertical(|ui| {
                    ui.label(rt("Refresh rate", 12.0).color(text2()));
                    ui.add_space(6.0);
                    let rates = self.rates();
                    let rows: Vec<(String, bool)> = rates.iter().map(|hz| (format!("{hz} Hz"), *hz == self.pick_hz)).collect();
                    pick_hz = w::dropdown(ui, "hz", 150.0, &format!("{} Hz", self.pick_hz), &rows).map(|i| rates[i]);
                });
                ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                    if th::button(ui, "Apply", true, (22.0, 7.0, 22.0, 8.0)).on_hover_text("Switches the display. You are asked to keep it; without an answer it switches back after 15 seconds.").clicked() {
                        apply = true;
                    }
                });
            });
            ui.add_space(14.0);
            let (cur, max) = self.dev().map(|d| display::refresh(&d)).unwrap_or((0, 0));
            caption(
                ui,
                &if cur == 0 {
                    "Refresh rate unavailable.".to_string()
                } else if cur >= max {
                    format!("Running at {cur} Hz, the highest this display offers at its current resolution.")
                } else {
                    format!("Running at {cur} Hz, but this display supports up to {max} Hz at its current resolution.")
                },
            );
            ui.add_space(4.0);
            caption3(ui, "After switching, MyTune asks you to keep the new mode. If you don't answer within 15 seconds, for example because the screen went black, it switches back by itself.");
        });
        if let Some(r) = pick_res {
            self.pick_res = r;
            let rates = self.rates();
            // a rate the new resolution doesn't offer jumps to its highest
            if !rates.contains(&self.pick_hz) {
                self.pick_hz = rates.first().copied().unwrap_or(self.pick_hz);
            }
        }
        if let Some(hz) = pick_hz {
            self.pick_hz = hz;
        }
        if apply {
            self.apply();
        }
    }

    fn color_cards(&mut self, ui: &mut Ui) {
        let Some(dev) = self.dev() else { return };
        let (mut changed, mut vib_changed, mut released) = (false, false, false);
        let (mut reset, mut all, mut reapply) = (false, false, false);

        // header: section title left, the section's own actions right
        ui.add_space(24.0);
        {
            // Reset has margin 0,0,-12,0 in the XAML: its text lines up with the card edge. Drawn in a child
            // that reaches 12 px past the row, so the page layout itself is not widened.
            let (row, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 24.0), egui::Sense::hover());
            ui.painter().text(row.left_center(), egui::Align2::LEFT_CENTER, "Color calibration", egui::FontId::new(13.0, th::display_semibold()), th::TEXT);
            let wide = egui::Rect::from_min_max(row.min, egui::pos2(row.max.x + 12.0, row.max.y));
            let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(wide).layout(Layout::right_to_left(Align::Center)));
            let ui = &mut ui;
            {
                if w::subtle_button(ui, "Reset", 12.0, (12.0, 4.0, 12.0, 5.0)).on_hover_text("Puts this display's colors and vibrance back to normal").clicked() {
                    reset = true;
                }
                if self.monitors.len() > 1 && w::subtle_button(ui, "Apply to all displays", 12.0, (12.0, 4.0, 12.0, 5.0)).clicked() {
                    all = true;
                }
                if w::subtle_button(ui, "Apply saved colors", 12.0, (12.0, 4.0, 12.0, 5.0)).on_hover_text("Windows clears the color curve after a restart; this puts your saved values back").clicked() {
                    reapply = true;
                }
            }
        }
        ui.add_space(8.0);
        th::card(ui, (12, 18, 18, 4), |ui| {
            let p = &mut self.profile;
            let rows: [(&str, &mut f64, f64, f64, String); 6] = [
                ("Gamma", &mut p.gamma, 0.30, 2.80, String::new()),
                ("Brightness", &mut p.brightness, -50.0, 50.0, String::new()),
                ("Contrast", &mut p.contrast, 50.0, 150.0, String::new()),
                ("Red balance", &mut p.red, 50.0, 100.0, String::new()),
                ("Green balance", &mut p.green, 50.0, 100.0, String::new()),
                ("Blue balance", &mut p.blue, 50.0, 100.0, String::new()),
            ];
            for (label, v, min, max, _) in rows {
                let f: &dyn Fn(f64) -> String = match label {
                    "Gamma" => &fmt_gamma,
                    "Brightness" => &fmt_signed,
                    _ => &fmt_pct,
                };
                let o = w::slider_row(ui, label, v, min, max, f, true);
                changed |= o.changed;
                released |= o.released;
            }
        });

        ui.add_space(24.0);
        ui.label(th::dsb("NVIDIA", 13.0).color(th::TEXT));
        ui.add_space(8.0);
        th::card(ui, (12, 18, 18, 4), |ui| {
            let on = self.vib.is_some();
            let o = w::slider_row(ui, "Digital vibrance", &mut self.vib_pct, 0.0, 100.0, &fmt_pct, on);
            vib_changed |= o.changed;
            released |= o.released;
            if !on {
                ui.add_space(-6.0);
                caption(ui, "Digital vibrance needs an NVIDIA GPU driving this display.");
                ui.add_space(12.0);
            }
        });

        if reapply {
            changed = true;
            vib_changed = self.profile.vibrance.is_some();
            if let Some(v) = self.profile.vibrance {
                self.vib_pct = v;
            }
        }
        if changed || vib_changed {
            let mut err = None;
            if changed {
                if let Err(e) = display::apply_ramp(&dev, &self.profile) {
                    err = Some(e.to_string());
                }
            }
            if vib_changed {
                self.profile.vibrance = Some(self.vib_pct);
                if let Err(e) = display::apply_vibrance(&dev, &mut self.profile) {
                    err = Some(e.to_string());
                }
            }
            let p = self.profile.clone();
            self.engine.with_store(|s| display::register(s, &dev, Some(&p)));
            self.status = err.map(|e| (e, false));
        }
        if released || reapply {
            if let Err(e) = self.engine.with_store(|s| s.save()) {
                self.status = Some((e.to_string(), false));
            }
        }
        if reset {
            let r = self.engine.with_store(|s| display::reset(s, &dev));
            self.status = Some(match r {
                Ok(()) => ("Colors reset.".into(), true),
                Err(e) => (e.to_string(), false),
            });
            self.choose();
        }
        if all {
            let p = self.profile.clone();
            let others: Vec<String> = self.monitors.iter().map(|m| m.device.clone()).filter(|d| *d != dev).collect();
            let mut err = None;
            for o in &others {
                if let Err(e) = self.engine.with_store(|s| display::copy_to(s, &p, o)) {
                    err = Some(format!("{o}: {e}"));
                }
            }
            self.status = Some(err.map(|e| (e, false)).unwrap_or(("Applied to all displays.".into(), true)));
        }
    }
}
