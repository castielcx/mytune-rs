//! Mouse page: guided 10 s polling-rate test and the pointer-acceleration switch.
//! Layout follows `Views/MousePage.xaml` and `Views/MouseRateCard.xaml`.

use crate::theme::{self as th, dsb, p, rt, sb, text2, text3};
use eframe::egui::{self, vec2, Align, Layout, Ui};
use mytune_core::mouse::{self, RateTest, Snapshot};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

const SECONDS: f32 = 10.0;
const DASH: &str = "\u{2013}";

pub struct MousePage {
    test: Option<RateTest>,
    rx: Option<Receiver<Snapshot>>,
    started: Instant,
    progress: f32,
    hint: String,
    rate: String,
    matched: String,
    summary: String,
    show_details: bool,
    spread: String,
    slowest: String,
    reports: String,
    load: String,
    dropouts: String,
    worst: String,
    stability: String,
    verdict: String,
    rates: Vec<f64>,
    slows: Vec<f64>,
    peaks: Vec<f64>,
    loads: Vec<f64>,
    accel: bool,
    status: Option<(String, bool)>,
}

impl MousePage {
    pub fn new() -> MousePage {
        MousePage {
            test: None,
            rx: None,
            started: Instant::now(),
            progress: 0.0,
            hint: mouse::IDLE_HINT.into(),
            rate: format!("{DASH} Hz"),
            matched: String::new(),
            summary: String::new(),
            show_details: false,
            spread: String::new(),
            slowest: DASH.into(),
            reports: "0".into(),
            load: DASH.into(),
            dropouts: DASH.into(),
            worst: DASH.into(),
            stability: String::new(),
            verdict: String::new(),
            rates: vec![],
            slows: vec![],
            peaks: vec![],
            loads: vec![],
            accel: mouse::acceleration_on(),
            status: None,
        }
    }

    pub fn running(&self) -> bool {
        self.test.is_some()
    }

    /// Stops listening (the page is left or the user pressed Cancel).
    pub fn cancel(&mut self) {
        if let Some(t) = self.test.take() {
            t.stop();
            self.rx = None;
            self.hint = mouse::IDLE_HINT.into();
            self.progress = 0.0;
        }
    }

    fn begin(&mut self, ctx: &egui::Context) {
        self.rates.clear();
        self.slows.clear();
        self.peaks.clear();
        self.loads.clear();
        self.rate = format!("{DASH} Hz");
        self.matched.clear();
        self.spread.clear();
        self.slowest = DASH.into();
        self.reports = "0".into();
        self.load = DASH.into();
        self.dropouts = DASH.into();
        self.worst = DASH.into();
        self.stability.clear();
        self.verdict.clear();
        self.summary.clear();
        self.progress = 0.0;
        let (tx, rx) = mpsc::channel();
        let ctx2 = ctx.clone();
        match RateTest::start(tx, move || ctx2.request_repaint()) {
            Ok(t) => {
                self.test = Some(t);
                self.rx = Some(rx);
                self.started = Instant::now();
                self.hint = "Measuring. Keep moving in fast, large circles.".into();
            }
            Err(e) => self.hint = format!("Couldn't start: {e}"),
        }
    }

    fn on_snapshot(&mut self, s: Snapshot) {
        self.reports = mouse::n0(s.reports as f64);
        if s.rate <= 0.0 {
            self.hint = "Keep moving: collecting samples\u{2026}".into();
            return;
        }
        self.rate = format!("{} Hz", mouse::n0(s.rate));
        if s.active_seconds < 0.8 {
            self.hint = "Move faster and don't pause; slow movement sends fewer reports.".into();
            return;
        }
        self.hint = "Good, keep going.".into();
        if s.active_seconds >= 1.2 {
            self.rates.push(s.rate);
            self.slows.push(s.slowest);
            self.peaks.push(s.peak);
            self.loads.push(s.load);
        }
    }

    fn finish(&mut self) {
        let stability = self.test.as_ref().and_then(|t| t.stability());
        if let Some(t) = self.test.take() {
            t.stop();
        }
        self.rx = None;
        self.progress = 1.0;
        let f = mouse::finish(&self.rates, &self.slows, &self.peaks, &self.loads, stability);
        self.hint = mouse::IDLE_HINT.into();
        self.rate = f.rate_text;
        self.matched = f.match_text;
        self.spread = f.spread_text;
        if f.enough {
            self.slowest = f.slowest_text;
            self.load = f.load_text;
            self.dropouts = f.dropouts_text;
            self.worst = f.worst_text;
        }
        self.stability = f.stability_text;
        self.verdict = f.verdict;
        self.summary = f.summary;
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        // measurement ticks
        let snaps: Vec<Snapshot> = self.rx.as_ref().map(|rx| rx.try_iter().collect()).unwrap_or_default();
        for s in snaps {
            self.on_snapshot(s);
        }
        if self.running() {
            self.progress = (self.started.elapsed().as_secs_f32() / SECONDS).min(1.0);
            ctx.request_repaint_after(Duration::from_millis(100));
            if self.progress >= 1.0 {
                self.finish();
            }
        }

        ui.label(rt("Polling rate test and pointer settings.", 12.0).color(text2()));
        th::section(ui, "Polling rate");
        self.rate_card(ui, &ctx);
        th::section(ui, "Pointer");
        self.pointer_card(ui);
    }

    fn rate_card(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let pal = p();
        let (mut start, mut stop, mut toggle) = (false, false, false);
        th::card(ui, (10, 16, 16, 16), |ui| {
            ui.horizontal(|ui| {
                let avail = ui.available_width();
                ui.vertical(|ui| {
                    ui.set_max_width((avail - 150.0).max(120.0));
                    ui.label(rt(&self.hint, 12.0).color(text2()));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.running() {
                        stop = th::button(ui, "Cancel", false, th::BTN).clicked();
                    } else {
                        start = th::button(ui, "Start (10 s)", true, th::BTN).clicked();
                    }
                });
            });
            if self.running() {
                ui.add_space(12.0);
                th::progress(ui, self.progress);
            }

            // result
            ui.add_space(14.0);
            ui.allocate_ui_with_layout(vec2(ui.available_width(), 46.0), Layout::left_to_right(Align::Max), |ui| {
                ui.label(dsb(&self.rate, 34.0));
                if !self.matched.is_empty() {
                    ui.add_space(14.0 - 8.0);
                    ui.vertical(|ui| {
                        ui.add_space(0.0);
                        ui.label(rt(&self.matched, 14.0).color(th::TEXT));
                        ui.add_space(6.0);
                    });
                }
            });
            if !self.summary.is_empty() {
                ui.add_space(6.0);
                ui.label(rt(&self.summary, 14.0));
            }

            ui.add_space(10.0);
            let label = if self.show_details { "Hide details" } else { "Show details" };
            toggle = th::subtle_button_bleed(ui, label, (8.0, 4.0, 8.0, 4.0), 8.0).clicked();

            if self.show_details {
                ui.add_space(8.0);
                let w = (ui.available_width() / 3.0).floor();
                let tiles = [
                    (&self.spread, "Average while moving", None),
                    (&self.slowest, "Slowest 5% of 0.1 s slices", None),
                    (&self.reports, "Movement reports seen", None),
                    (&self.load, "Test's own CPU time", Some("High (above 70%) means the test itself can't keep up; low means the limit is the mouse, USB or Windows.")),
                    (&self.dropouts, "Dropouts (10 ms slices)", None),
                    (&self.worst, "Worst 1% vs typical", None),
                ];
                for (row, chunk) in tiles.chunks(3).enumerate() {
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for (value, caption, tip) in chunk {
                            ui.allocate_ui_with_layout(vec2(w, 0.0), Layout::top_down(Align::Min), |ui| {
                                ui.set_width(w);
                                ui.label(sb(value.as_str(), 14.0).color(text2()));
                                let c = ui.label(rt(*caption, 12.0).color(text2()));
                                if let Some(t) = tip {
                                    c.on_hover_text(*t);
                                }
                            });
                        }
                    });
                    if row == 0 {
                        ui.add_space(10.0);
                    }
                }
                if !self.stability.is_empty() {
                    ui.add_space(4.0);
                    ui.label(rt(&self.stability, 12.0).color(text2()));
                }
                if !self.verdict.is_empty() {
                    ui.add_space(8.0);
                    ui.label(rt(&self.verdict, 12.0).color(text2()));
                }
                ui.add_space(12.0);
                egui::Frame::new().fill(pal.accent_soft).inner_margin(egui::Margin::symmetric(12, 10)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(rt(GOOD_TO_KNOW, 12.0));
                });
            }
        });
        if start {
            self.begin(ctx);
        }
        if stop {
            self.cancel();
        }
        if toggle {
            self.show_details = !self.show_details;
        }
    }

    fn pointer_card(&mut self, ui: &mut Ui) {
        let mut click = false;
        th::card(ui, (10, 16, 16, 16), |ui| {
            ui.horizontal(|ui| {
                let avail = ui.available_width();
                ui.vertical(|ui| {
                    ui.set_max_width((avail - 64.0).max(120.0));
                    ui.label(sb("Mouse acceleration", 14.0));
                    ui.label(rt("\"Enhance pointer precision\". Most players turn this off for consistent aim.", 12.0).color(text2()));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    click = th::switch(ui, self.accel).clicked();
                });
            });
        });
        if let Some((msg, ok)) = &self.status {
            ui.add_space(6.0);
            ui.label(rt(msg, 12.0).color(if *ok { th::SUCCESS } else { th::DANGER }));
        }
        if click {
            let on = !self.accel;
            self.status = match mouse::set_acceleration(on) {
                Ok(()) => None,
                Err(e) => Some((format!("Mouse acceleration: {e}"), false)),
            };
            self.accel = mouse::acceleration_on();
        }
        let _ = text3;
    }
}

const GOOD_TO_KNOW: &str = "Good to know: a mouse only sends a report when it has moved since the last one, so a fast mouse needs fast movement to show its full rate. The speed needed is roughly rate \u{F7} DPI \u{D7} 2.5 cm/s. At 8000 Hz that is about 25 cm/s with 800 DPI, 13 cm/s with 1600 DPI and 6 cm/s with 3200 DPI. If your result is below your mouse's setting, raise the DPI just for this test and make fast swipes. You can set your normal DPI again afterwards.";
