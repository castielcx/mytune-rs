//! Display page, laid out like `Views/DisplayPage.xaml`: monitor pill track, resolution + refresh pick lists
//! with one Apply (keep-or-revert dialog). Colours and NVIDIA vibrance are on the Vibrance page.
//! Display calls are quick, so they run on the UI thread like the C# page.

use crate::theme::{self as th, bd, rt, text2};
use crate::widgets::{self as w, caption, caption3};
use eframe::egui::{self, Id, Layout, Align, Ui};
use mytune_core::customres;
use mytune_core::display::{self, DisplayMode, Monitor};
use mytune_core::engine::Engine;
use std::sync::Arc;
use std::time::{Duration, Instant};

const KEEP_SECONDS: u64 = 15;

struct Trial {
    dev: String,
    old: DisplayMode,
    new: DisplayMode,
    since: Instant,
    /// A custom resolution on driver trial (not a mode Windows lists).
    custom: bool,
}

pub struct DisplayPage {
    engine: Arc<Engine>,
    /// Typed width and height of a new custom resolution.
    custom_w: String,
    custom_h: String,
    /// The chosen display's own resolution at its highest refresh rate.
    native: Option<DisplayMode>,
    monitors: Vec<Monitor>,
    sel: usize,
    now: Option<DisplayMode>,
    modes: Vec<DisplayMode>,
    pick_res: (u32, u32),
    pick_hz: u32,
    loaded: bool,
    pub(crate) status: Option<(String, bool)>,
    trial: Option<Trial>,
}

impl DisplayPage {
    pub fn new(engine: Arc<Engine>) -> DisplayPage {
        DisplayPage {
            engine,
            custom_w: String::new(),
            custom_h: String::new(),
            native: None,
            monitors: vec![],
            sel: 0,
            now: None,
            modes: vec![],
            pick_res: (0, 0),
            pick_hz: 0,
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
        self.native = customres::native(&dev, &self.modes);
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
            Ok(()) => self.trial = Some(Trial { dev, old, new, since: Instant::now(), custom: false }),
            Err(e) => self.status = Some((e.to_string(), false)),
        }
    }

    /// Test: the typed size on driver trial, then the keep-or-revert dialog.
    fn test_custom(&mut self) {
        let (Some(dev), Some(old)) = (self.dev(), self.now) else { return };
        let (Ok(w), Ok(h)) = (self.custom_w.trim().parse::<u32>(), self.custom_h.trim().parse::<u32>()) else {
            self.status = Some(("Type a width and a height in pixels.".into(), false));
            return;
        };
        match customres::validate(w, h, self.native, &self.modes).map_err(|e| e.to_string()).and_then(|new| customres::try_mode(&dev, new).map(|()| new).map_err(|e| format!("The graphics driver refused {w} x {h}: {e}"))) {
            Ok(new) => self.trial = Some(Trial { dev, old, new, since: Instant::now(), custom: true }),
            Err(e) => self.status = Some((e, false)),
        }
    }

    /// Keep: the driver saves the mode on trial, it becomes the display's mode, the display is set to stretch
    /// and the mode joins the saved list. Otherwise the trial ends.
    fn finish_custom(&mut self, t: Trial, keep: bool) {
        let size = t.new.label();
        self.status = Some(if !keep {
            match customres::revert(&t.dev) {
                Ok(()) => ("Switched back to the old mode.".into(), true),
                Err(e) => (format!("Couldn't switch back: {e}. A restart restores the old mode."), false),
            }
        } else if let Err(e) = customres::save(&t.dev) {
            let _ = customres::revert(&t.dev);
            (format!("Couldn't save {size}: {e}"), false)
        } else {
            let stored = self.engine.with_store(|s| {
                customres::register(s, &t.dev, t.new, true);
                s.save()
            });
            // the trial was never written to Windows' settings: make it the display's saved mode
            let _ = display::set_mode(&t.dev, t.new, true);
            match (customres::stretch(&t.dev), stored) {
                (Ok(()), Ok(())) => (format!("{size} saved. This display now stretches every resolution to fill the screen."), true),
                (Err(e), _) => (format!("{size} saved, but stretching couldn't be turned on: {e}"), false),
                (_, Err(e)) => (format!("{size} saved in the driver, but MyTune couldn't note it: {e}"), false),
            }
        });
        self.custom_w.clear();
        self.custom_h.clear();
        self.choose();
    }

    /// Apply on a saved row: the normal switch with the keep-or-revert dialog.
    fn apply_custom(&mut self, m: DisplayMode) {
        if !self.modes.contains(&m) {
            self.status = Some((format!("Windows no longer lists {}. Remove it and add it again.", m.label()), false));
            return;
        }
        self.pick_res = (m.width, m.height);
        self.pick_hz = m.hz;
        self.apply();
    }

    /// Remove: off the mode first when the display runs it, then out of the driver and the saved list.
    fn remove_custom(&mut self, m: DisplayMode) {
        let Some(dev) = self.dev() else { return };
        if self.now == Some(m) {
            if let Some(Err(e)) = self.native.map(|n| display::set_mode(&dev, n, true)) {
                self.status = Some((format!("Couldn't leave {}: {e}", m.label()), false));
                return;
            }
        }
        self.status = Some(match customres::delete(&dev, m) {
            Ok(()) => {
                let stored = self.engine.with_store(|s| {
                    customres::register(s, &dev, m, false);
                    s.save()
                });
                match stored {
                    Ok(()) => (format!("{} removed.", m.label()), true),
                    Err(e) => (format!("{} removed from the driver, but MyTune couldn't note it: {e}", m.label()), false),
                }
            }
            Err(e) => (format!("Couldn't remove {}: {e}", m.label()), false),
        });
        self.choose();
    }

    fn finish_trial(&mut self, keep: bool) {
        let Some(t) = self.trial.take() else { return };
        if t.custom {
            return self.finish_custom(t, keep);
        }
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
        if let Some((msg, ok)) = self.status.take() {
            crate::notice::post(&ctx, msg, ok);
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
        self.custom_card(ui);
    }

    /// Custom resolutions (NVIDIA only): one plate with the add row and a row per saved resolution.
    fn custom_card(&mut self, ui: &mut Ui) {
        th::section(ui, "Custom resolutions");
        if !mytune_core::nvapi::available() {
            th::card(ui, (12, 16, 18, 16), |ui| {
                ui.label(th::sb("Custom resolutions", 14.0));
                ui.add_space(4.0);
                caption(ui, "Custom resolutions need an NVIDIA graphics card.");
            });
            return;
        }
        let saved = self.dev().map(|d| self.engine.with_store(|s| customres::saved(s, &d))).unwrap_or_default();
        let hz = self.native.map(|n| n.hz).unwrap_or(0);
        let (mut test, mut apply, mut remove) = (false, None, None);
        th::group(ui, |ui| {
            th::card(ui, (12, 16, 18, 16), |ui| {
                ui.label(th::sb("Custom resolutions", 14.0));
                ui.add_space(4.0);
                caption(ui, "Add a resolution Windows does not list. Always stretched to fill the screen.");
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    for (label, value) in [("Width", &mut self.custom_w), ("Height", &mut self.custom_h)] {
                        ui.label(rt(label, 12.0).color(text2()));
                        ui.add(egui::TextEdit::singleline(value).desired_width(64.0).char_limit(5));
                    }
                    ui.label(rt(format!("{hz} Hz (highest)"), 12.0).color(text2()));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        test = th::button(ui, "Test", true, (22.0, 7.0, 22.0, 8.0)).on_hover_text("Shows the resolution. You are asked to keep it; without an answer it switches back after 15 seconds.").clicked();
                    });
                });
            });
            for m in &saved {
                th::card(ui, (12, 10, 18, 10), |ui| {
                    ui.horizontal(|ui| {
                        ui.label(rt(format!("{}   {} Hz", m.label(), m.hz), 14.0));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if th::subtle_button(ui, "Remove", th::BTN).on_hover_text("Deletes this resolution from the graphics driver. Stretching stays on.").clicked() {
                                remove = Some(*m);
                            }
                            if th::button(ui, "Apply", false, th::BTN).clicked() {
                                apply = Some(*m);
                            }
                        });
                    });
                });
            }
        });
        if test {
            self.test_custom();
        }
        if let Some(m) = apply {
            self.apply_custom(m);
        }
        if let Some(m) = remove {
            self.remove_custom(m);
        }
    }

    /// One pill track (ControlBg, padding 3); the chosen monitor is an AccentButton, the others SubtleButtons.
    fn monitor_track(&mut self, ui: &mut Ui) {
        let mut pick = None;
        // same tab row as Settings (track + sliding glass thumb)
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
        }
    }

    fn mode_card(&mut self, ui: &mut Ui) {
        th::section(ui, "Resolution and refresh rate");
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
}
