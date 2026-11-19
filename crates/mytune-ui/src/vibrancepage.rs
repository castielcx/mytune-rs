//! Vibrance page: the colours of each display. NVIDIA Digital Vibrance, brightness / contrast / gamma /
//! shadow lift and the colour balance. The values are a `DisplayProfile` in state.json (`Displays`); the
//! page only says what it wants (`display::want`), the colour worker in mytune-core makes the calls.

use crate::theme::{self as th, sb};
use crate::widgets::{self as w, caption};
use eframe::egui::Ui;
use mytune_core::display::{self, DisplayProfile, Monitor};
use mytune_core::engine::Engine;
use mytune_core::vibrance::VibranceRange;
use std::sync::Arc;

pub struct VibrancePage {
    engine: Arc<Engine>,
    monitors: Vec<Monitor>,
    sel: usize,
    profile: DisplayProfile,
    /// The driver's vibrance range; `None` when no NVIDIA card drives the chosen display.
    vib: Option<VibranceRange>,
    vib_pct: f64,
    loaded: bool,
}

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

/// Card heading with one hint line under it.
fn head(ui: &mut Ui, title: &str, hint: &str) {
    ui.label(sb(title, 14.0));
    ui.add_space(4.0);
    caption(ui, hint);
    ui.add_space(16.0);
}

impl VibrancePage {
    pub fn new(engine: Arc<Engine>) -> VibrancePage {
        VibrancePage { engine, monitors: vec![], sel: 0, profile: DisplayProfile::default(), vib: None, vib_pct: 50.0, loaded: false }
    }

    fn dev(&self) -> Option<String> {
        self.monitors.get(self.sel).map(|m| m.device.clone())
    }

    fn choose(&mut self) {
        let Some(dev) = self.dev() else { return };
        self.profile = self.engine.with_store(|s| display::get_profile(s, &dev));
        self.vib = display::get_vibrance(&dev);
        if let Some(r) = self.vib {
            self.vib_pct = self.profile.vibrance.unwrap_or_else(|| r.percent_for());
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        if !self.loaded {
            self.loaded = true;
            self.monitors = display::monitors();
            self.choose();
        }
        let ctx = ui.ctx().clone();
        if let Some(e) = display::take_problem() {
            crate::notice::post(&ctx, e, false);
        }
        let Some(dev) = self.dev() else {
            ui.add_space(18.0);
            caption(ui, "No active displays found.");
            return;
        };
        if self.monitors.len() > 1 {
            // the same monitor row as the Display page
            ui.add_space(18.0);
            let mut pick = None;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (i, m) in self.monitors.iter().enumerate() {
                    if th::tab(ui, &m.label, i == self.sel).clicked() {
                        pick = Some(i);
                    }
                }
            });
            if let Some(i) = pick {
                self.sel = i;
                self.choose();
                return ctx.request_repaint(); // draw the other display's values next frame
            }
        }
        let (mut changed, mut vib_changed, mut sat_changed, mut released) = (false, false, false, false);

        th::section(ui, "Digital Vibrance");
        th::card(ui, (12, 16, 18, 4), |ui| match self.vib {
            Some(r) => {
                let normal = VibranceRange { current: r.default, ..r }.percent_for();
                head(ui, "Digital Vibrance", &format!("Makes every colour stronger, so things stand out more in games. {normal:.0}% is normal."));
                let o = w::slider_row(ui, "Vibrance", &mut self.vib_pct, 0.0, 100.0, &fmt_pct, true);
                vib_changed = o.changed;
                released |= o.released;
                let o = w::slider_row(ui, "Extra saturation", &mut self.profile.saturation, 0.0, 100.0, &fmt_signed, true);
                sat_changed = o.changed;
                released |= o.released;
                caption(ui, "Goes beyond the graphics driver's maximum. The mouse pointer's colour can look slightly off.");
                ui.add_space(12.0);
            }
            None => {
                head(ui, "Digital Vibrance", "Digital Vibrance needs an NVIDIA graphics card on this display. The settings below work on every card.");
                ui.add_space(-4.0);
            }
        });

        let p = &mut self.profile;
        let mut row = |ui: &mut Ui, label: &str, v: &mut f64, min: f64, max: f64, f: &dyn Fn(f64) -> String| {
            let o = w::slider_row(ui, label, v, min, max, f, true);
            changed |= o.changed;
            released |= o.released;
        };
        th::section(ui, "Brightness and contrast");
        th::card(ui, (12, 16, 18, 4), |ui| {
            head(ui, "Brightness and contrast", "Shadow lift brightens only the dark areas, so you can see into dark corners while the rest stays as it is.");
            row(ui, "Brightness", &mut p.brightness, -50.0, 50.0, &fmt_signed);
            row(ui, "Contrast", &mut p.contrast, 50.0, 150.0, &fmt_pct);
            row(ui, "Gamma", &mut p.gamma, 0.30, 2.80, &fmt_gamma);
            row(ui, "Shadow lift", &mut p.shadows, 0.0, 100.0, &fmt_pct);
        });

        th::section(ui, "Colour balance");
        th::card(ui, (12, 16, 18, 4), |ui| {
            head(ui, "Colour balance", "Turn one colour down to take a tint out of the picture, for example blue for a warmer look.");
            row(ui, "Red", &mut p.red, 50.0, 100.0, &fmt_pct);
            row(ui, "Green", &mut p.green, 50.0, 100.0, &fmt_pct);
            row(ui, "Blue", &mut p.blue, 50.0, 100.0, &fmt_pct);
        });

        ui.add_space(16.0);
        let (mut reset, mut all) = (false, false);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            reset = th::button(ui, "Reset", false, th::BTN).on_hover_text("Puts this display's colours and vibrance back to normal").clicked();
            all = self.monitors.len() > 1 && th::button(ui, "Apply to all displays", false, th::BTN).on_hover_text("Copies these values to your other displays").clicked();
        });
        ui.add_space(14.0);
        w::caption3(ui, "These colours stay on everywhere, also on the desktop. MyTune puts them back when a game or Windows clears them.");

        if changed || vib_changed || sat_changed {
            if vib_changed {
                self.profile.vibrance = Some(self.vib_pct);
                if let Some(r) = self.vib {
                    self.profile.original_vibrance_level.get_or_insert(r.current); // what Reset puts back
                }
            }
            display::want(&dev, &self.profile);
            let p = self.profile.clone();
            self.engine.with_store(|s| display::register(s, &dev, Some(&p)));
        }
        if reset {
            match self.engine.with_store(|s| display::reset(s, &dev)) {
                Ok(level) => {
                    display::want_reset(&dev, level);
                    self.profile = DisplayProfile::default();
                    if let (Some(r), Some(level)) = (self.vib.as_mut(), level) {
                        r.current = level;
                        self.vib_pct = r.percent_for();
                    }
                    crate::notice::post(&ctx, "Colours reset.".into(), true);
                }
                Err(e) => crate::notice::post(&ctx, e.to_string(), false),
            }
        }
        if all {
            let p = self.profile.clone();
            for o in self.monitors.iter().map(|m| &m.device).filter(|d| **d != dev) {
                let range = display::get_vibrance(o);
                let copy = self.engine.with_store(|s| display::copy_to(s, &p, o, range));
                display::want(o, &copy);
            }
        }
        if released || all {
            match self.engine.with_store(|s| s.save()) {
                Err(e) => crate::notice::post(&ctx, e.to_string(), false),
                Ok(()) if all => crate::notice::post(&ctx, "Applied to all displays.".into(), true),
                Ok(()) => {}
            }
        }
    }
}
