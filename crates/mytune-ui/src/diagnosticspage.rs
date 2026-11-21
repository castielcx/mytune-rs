//! Diagnostics page (without the benchmark): background programs, driver latency, drivers.
//! Layout follows `Views/DiagnosticsPage.xaml`, `ProcessesCard.xaml`, `DriversCard.xaml`.

use crate::theme::{self as th, rt, sb, text2, text3};
use eframe::egui::{self, vec2, Align, Color32, Id, Layout, Ui};
use mytune_core::diagnostics::{self as diag, ChipLevel, DriverView, ProcessMonitor, ProcessSample};
use mytune_core::engine::Engine;
use mytune_core::etw::{self, LatencyRow, NetTrace};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Sort {
    Cpu,
    Disk,
    Net,
    Mem,
}

const LIVE: &str = "What is using your PC right now, updated every 2 seconds. Programs with several processes are added together.";
const LIVE_NO_NET: &str = "CPU, disk and memory use, updated every 2 seconds. Network use isn't available (the network trace couldn't start).";
const DRIVERS_INTRO: &str = "The drivers that matter most for games: graphics, network and sound. Old drivers are a common cause of stutter and crashes; Windows Update is not a good source for graphics drivers.";

pub struct DiagnosticsPage {
    engine: Arc<Engine>,
    // background programs
    monitor: Arc<Mutex<ProcessMonitor>>,
    /// The monitor's network trace runs (kept from the last sample, so no frame waits on the monitor lock).
    net_seen: bool,
    running: bool,
    sampling: bool,
    samples: Vec<ProcessSample>,
    sample_rx: Option<Receiver<Result<Vec<ProcessSample>, String>>>,
    last_tick: Option<Instant>,
    hover: bool,
    sort: Sort,
    status: String,
    confirm: Option<ProcessSample>,
    note: Option<(String, bool)>,
    end_rx: Option<Receiver<(String, usize)>>,
    net: Option<NetTrace>,
    // driver latency
    lat_rows: Vec<LatencyRow>,
    lat_summary: String,
    lat_rx: Option<Receiver<Result<Vec<LatencyRow>, String>>>,
    lat_previous: Option<f64>,
    // drivers
    drivers: Vec<DriverView>,
    drivers_summary: String,
    drivers_rx: Option<Receiver<Result<Vec<diag::DriverRow>, String>>>,
    drivers_loaded: bool,
}

impl DiagnosticsPage {
    pub fn new(engine: Arc<Engine>) -> DiagnosticsPage {
        DiagnosticsPage {
            engine,
            monitor: Arc::new(Mutex::new(ProcessMonitor::new(None))),
            net_seen: false,
            running: false,
            sampling: false,
            samples: vec![],
            sample_rx: None,
            last_tick: None,
            hover: false,
            sort: Sort::Cpu,
            status: "Reading\u{2026}".into(),
            confirm: None,
            note: None,
            end_rx: None,
            net: None,
            lat_rows: vec![],
            lat_summary: etw::LATENCY_INTRO.into(),
            lat_rx: None,
            lat_previous: None,
            drivers: vec![],
            drivers_summary: DRIVERS_INTRO.into(),
            drivers_rx: None,
            drivers_loaded: false,
        }
    }

    /// Sampling only runs while the page is shown (`SyncSampling` in the C# card).
    pub fn stop(&mut self) {
        if self.running {
            self.running = false;
            self.last_tick = None;
            self.net = None; // stops the network trace
            // a new monitor, not a lock on the old one: a sample in flight holds that lock
            self.monitor = Arc::new(Mutex::new(ProcessMonitor::new(None)));
        }
    }

    fn game_paths(&self) -> Vec<String> {
        self.engine.with_store(|s| {
            s.top("Games")
                .and_then(|g| g.as_array())
                .map(|a| a.iter().filter_map(|g| g.get("ExePath").and_then(|p| p.as_str()).map(String::from)).collect())
                .unwrap_or_default()
        })
    }

    fn tick(&mut self, ctx: &egui::Context) {
        if self.sampling {
            return;
        }
        self.sampling = true;
        self.last_tick = Some(Instant::now());
        let (tx, rx) = mpsc::channel();
        let (monitor, games, ctx) = (self.monitor.clone(), self.game_paths(), ctx.clone());
        std::thread::spawn(move || {
            let r = monitor.lock().map_err(|e| e.to_string()).and_then(|mut m| m.sample(&games).map_err(|e| e.to_string()));
            let _ = tx.send(r);
            ctx.request_repaint();
        });
        self.sample_rx = Some(rx);
    }

    fn live_text(&self) -> &'static str {
        if self.net_seen {
            LIVE
        } else {
            LIVE_NO_NET
        }
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.sample_rx {
            if let Ok(r) = rx.try_recv() {
                self.sampling = false;
                self.sample_rx = None;
                match r {
                    Ok(s) => {
                        self.samples = s;
                        // read here, between samples: the sampling thread holds this lock while it works
                        self.net_seen = self.monitor.try_lock().map(|m| m.network_available()).unwrap_or(self.net_seen);
                        if !self.hover {
                            self.status = self.live_text().into();
                        }
                    }
                    Err(e) => self.status = format!("Couldn't read processes: {e}"),
                }
            }
        }
        if let Some(rx) = &self.end_rx {
            if let Ok((base, n)) = rx.try_recv() {
                self.end_rx = None;
                self.note = Some(if n > 0 { (format!("Ended {base}."), true) } else { (format!("Couldn't end {base}."), false) });
                self.last_tick = None; // sample again right away
            }
        }
        if let Some(rx) = &self.lat_rx {
            if let Ok(r) = rx.try_recv() {
                self.lat_rx = None;
                match r {
                    Ok(mut rows) => {
                        self.lat_summary = etw::latency_summary(&rows, self.lat_previous);
                        self.lat_previous = rows.first().map(|r| r.max_us);
                        rows.truncate(8);
                        self.lat_rows = rows;
                    }
                    Err(e) => self.lat_summary = format!("The scan couldn't run: {e}"),
                }
            }
        }
        if let Some(rx) = &self.drivers_rx {
            if let Ok(r) = rx.try_recv() {
                self.drivers_rx = None;
                self.drivers_loaded = true;
                match r {
                    Ok(rows) => {
                        let today = diag::today_days();
                        self.drivers = rows.iter().map(|r| diag::driver_view(r, today)).collect();
                        self.drivers_summary = if rows.is_empty() { "No drivers were found.".into() } else { DRIVERS_INTRO.into() };
                    }
                    Err(e) => self.drivers_summary = format!("Couldn't read the drivers: {e}"),
                }
            }
        }
        let _ = ctx;
    }

    fn scan_latency(&mut self, ctx: &egui::Context) {
        self.lat_summary = "Scanning for 15 seconds…".into();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(etw::latency_scan(15));
            ctx.request_repaint();
        });
        self.lat_rx = Some(rx);
    }

    fn latency_card(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let mut scan = false;
        let busy = self.lat_rx.is_some();
        th::card(ui, (10, 16, 16, 16), |ui| {
            ui.horizontal(|ui| {
                let avail = ui.available_width();
                ui.vertical(|ui| {
                    ui.set_max_width((avail - 200.0).max(120.0));
                    ui.label(rt(&self.lat_summary, 12.0).color(text2()));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if busy {
                        ui.label(rt("Scanning…", 12.0).color(text3()));
                    } else {
                        scan = th::button(ui, "Scan for 15 seconds", true, th::BTN).clicked();
                    }
                });
            });
            if !self.lat_rows.is_empty() {
                ui.add_space(12.0);
            }
            for r in &self.lat_rows {
                ui.horizontal(|ui| {
                    let avail = ui.available_width();
                    ui.vertical(|ui| {
                        ui.set_max_width((avail - 170.0).max(120.0));
                        ui.label(rt(&r.driver, 14.0));
                        ui.label(rt(etw::latency_detail(r), 12.0).color(text3()));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let (level, text) = etw::latency_verdict(r.max_us);
                        let (fg, bg) = match level {
                            ChipLevel::Good => (th::SUCCESS, th::success_soft()),
                            ChipLevel::Warn => (th::DANGER, th::danger_soft()),
                            ChipLevel::Neutral => (th::WARNING, th::warning_soft()),
                        };
                        th::chip(ui, text, fg, bg);
                    });
                });
                ui.add_space(8.0);
            }
        });
        if scan {
            self.scan_latency(ctx);
        }
    }

    fn scan_drivers(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(diag::read_drivers().map_err(|e| e.to_string()));
            ctx.request_repaint();
        });
        self.drivers_rx = Some(rx);
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        if !self.drivers_loaded && self.drivers_rx.is_none() {
            self.scan_drivers(&ctx);
        }

        if let Some((msg, ok)) = &self.note {
            ui.label(rt(msg, 12.0).color(if *ok { th::SUCCESS } else { th::DANGER }));
        }
        th::section(ui, "Driver latency (DPC and interrupts)");
        self.latency_card(ui, &ctx);
        th::section(ui, "Drivers");
        self.drivers_card(ui, &ctx);
        if self.confirm.is_some() {
            self.confirm_modal(&ctx);
        }
    }

    fn sorted(&self) -> Vec<&ProcessSample> {
        let key = |s: &ProcessSample| match self.sort {
            Sort::Cpu => s.cpu_percent,
            Sort::Disk => s.disk_mb_per_sec,
            Sort::Net => s.net_kb_per_sec,
            Sort::Mem => s.memory_mb,
        };
        let mut v: Vec<&ProcessSample> = self.samples.iter().collect();
        v.sort_by(|a, b| key(b).total_cmp(&key(a)));
        v.truncate(12);
        v
    }

    fn processes_card(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let net = self.net_seen;
        let mut new_sort = None;
        let mut end = None;
        let rows: Vec<ProcessSample> = self.sorted().into_iter().cloned().collect();
        let status = self.status.clone();
        let r = ui.scope(|ui| {
            th::card(ui, (10, 16, 16, 16), |ui| {
                ui.horizontal(|ui| {
                    let avail = ui.available_width();
                    ui.vertical(|ui| {
                        ui.set_max_width((avail - 330.0).max(120.0));
                        ui.label(rt(&status, 12.0).color(text2()));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        for (label, s) in [("Memory", Sort::Mem), ("Network", Sort::Net), ("Disk", Sort::Disk), ("CPU", Sort::Cpu)] {
                            if th::tab(ui, label, self.sort == s).clicked() {
                                new_sort = Some(s);
                            }
                        }
                        ui.add_space(2.0);
                        ui.label(rt("Sort by", 12.0).color(text2()));
                    });
                });
                ui.add_space(14.0);
                let widths = [70.0, 90.0, 90.0, 90.0, 80.0];
                let name_w = (ui.available_width() - widths.iter().sum::<f32>()).max(80.0);
                let cell = |ui: &mut Ui, w: f32, right: bool, f: &mut dyn FnMut(&mut Ui)| {
                    let layout = if right { Layout::right_to_left(Align::Center) } else { Layout::left_to_right(Align::Center) };
                    ui.allocate_ui_with_layout(vec2(w, 24.0), layout, |ui| {
                        ui.set_min_width(w);
                        f(ui)
                    });
                };
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    cell(ui, name_w, false, &mut |ui| {
                        ui.label(rt("Program", 12.0).color(text2()));
                    });
                    for (w, t) in [(70.0, "CPU"), (90.0, "Disk"), (90.0, "Network"), (90.0, "Memory")] {
                        cell(ui, w, true, &mut |ui| {
                            ui.label(rt(t, 12.0).color(text2()));
                        });
                    }
                });
                ui.add_space(6.0 - 6.0);
                for s in &rows {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        cell(ui, name_w, false, &mut |ui| {
                            let l = ui.add(egui::Label::new(rt(&s.name, 14.0)).truncate());
                            if let Some(p) = &s.path {
                                l.on_hover_text(p);
                            }
                        });
                        let disk = if s.disk_mb_per_sec >= 0.05 { format!("{:.1} MB/s", s.disk_mb_per_sec) } else { "0 MB/s".into() };
                        let netv = if !net {
                            "n/a".to_string()
                        } else if s.net_kb_per_sec >= 1024.0 {
                            format!("{:.1} MB/s", s.net_kb_per_sec / 1024.0)
                        } else {
                            format!("{:.0} KB/s", s.net_kb_per_sec)
                        };
                        let mem = if s.memory_mb >= 1024.0 { format!("{:.1} GB", s.memory_mb / 1024.0) } else { format!("{:.0} MB", s.memory_mb) };
                        for (w, t) in [(70.0, format!("{:.1}%", s.cpu_percent)), (90.0, disk), (90.0, netv), (90.0, mem)] {
                            cell(ui, w, true, &mut |ui| {
                                ui.label(rt(&t, 14.0));
                            });
                        }
                        cell(ui, 80.0, true, &mut |ui| {
                            if s.can_end && th::button(ui, "End", false, (12.0, 3.0, 12.0, 3.0)).on_hover_text("Close this program").clicked() {
                                end = Some(s.clone());
                            }
                        });
                    });
                    ui.add_space(6.0);
                }
                ui.add_space(2.0);
                ui.label(rt("Windows, security software, anti-cheat and your Game Hub games never get an End button. Unsaved work in an ended program is lost.", 12.0).color(text3()));
            });
        });
        // The list stands still while the mouse is over it, so a button doesn't move away before you click it.
        let hover = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| r.response.rect.contains(p));
        if hover != self.hover {
            self.hover = hover;
            if hover {
                self.status = "Paused while your mouse is over the list. Move away to update.".into();
            } else {
                self.status = self.live_text().into();
                self.last_tick = None;
            }
        }
        if let Some(s) = new_sort {
            self.sort = s;
        }
        if let Some(s) = end {
            self.confirm = Some(s);
        }
    }

    fn confirm_modal(&mut self, ctx: &egui::Context) {
        let Some(s) = self.confirm.clone() else { return };
        let m = egui::Modal::new(Id::new("end-program")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(sb(format!("End {}?", s.name), 20.0));
            ui.add_space(4.0);
            ui.label(rt("Any unsaved work in it is lost.", 12.0).color(text2()));
            ui.add_space(12.0);
            let mut ok = false;
            ui.horizontal(|ui| {
                ok = th::button(ui, "OK", true, (18.0, 8.0, 18.0, 9.0)).clicked();
                if th::button(ui, "Cancel", false, (14.0, 8.0, 14.0, 9.0)).clicked() {
                    ui.close();
                }
            });
            ok
        });
        if m.inner {
            self.confirm = None;
            let (tx, rx) = mpsc::channel();
            let (games, base, ctx) = (self.game_paths(), s.base.clone(), ctx.clone());
            std::thread::spawn(move || {
                let n = diag::end_processes(&base, &games);
                let _ = tx.send((base, n));
                ctx.request_repaint();
            });
            self.end_rx = Some(rx);
        } else if m.should_close() {
            self.confirm = None;
        }
    }

    fn drivers_card(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        let mut rescan = false;
        let busy = self.drivers_rx.is_some();
        th::card(ui, (10, 16, 16, 16), |ui| {
            ui.horizontal(|ui| {
                let avail = ui.available_width();
                ui.vertical(|ui| {
                    ui.set_max_width((avail - 120.0).max(120.0));
                    ui.label(rt(&self.drivers_summary, 12.0).color(text2()));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if busy {
                        ui.label(rt("Scanning\u{2026}", 12.0).color(text3()));
                    } else if th::button(ui, "Rescan", false, th::BTN).clicked() {
                        rescan = true;
                    }
                });
            });
            ui.add_space(12.0);
            for d in &self.drivers {
                ui.horizontal(|ui| {
                    let avail = ui.available_width();
                    ui.vertical(|ui| {
                        ui.set_max_width((avail - 230.0).max(120.0));
                        ui.add(egui::Label::new(rt(&d.title, 14.0)).truncate());
                        ui.label(rt(&d.detail, 12.0).color(text3()));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if let Some(url) = d.url {
                            if th::button(ui, "Get drivers", false, th::BTN).on_hover_text("Opens the vendor's download page").clicked() {
                                open_url(url);
                            }
                        }
                        ui.add_space(4.0);
                        let (fg, bg) = match d.age.0 {
                            ChipLevel::Good => (th::SUCCESS, th::success_soft()),
                            ChipLevel::Warn => (th::WARNING, th::warning_soft()),
                            ChipLevel::Neutral => (text2(), th::control_bg()),
                        };
                        th::chip(ui, &d.age.1, fg, bg);
                    });
                });
                ui.add_space(10.0);
            }
        });
        if rescan {
            self.drivers_loaded = false;
            self.scan_drivers(ctx);
        }
        let _: Option<Color32> = None;
    }
}

/// Opens a page in the default browser without going through a shell.
pub(crate) fn open_url(url: &str) {
    let _ = std::process::Command::new("rundll32.exe").args(["url.dll,FileProtocolHandler", url]).spawn();
}
