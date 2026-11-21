//! Internet page: net.* tweaks, speed test, network repair, VPN list. All slow work runs on threads and
//! reports through channels, so the window never blocks (adapter tweaks can take 30 s).

use crate::theme::{self as th, rt, sb, text2, text3};
use eframe::egui::{self, Align, Layout, Ui};
use mytune_core::catalog::TweakFlags;
use mytune_core::connhealth::{self, Grade};
use mytune_core::engine::Engine;
use mytune_core::netrepair::{self, NetTweak};
use mytune_core::speedtest::{self, LastSpeed, Phase, SpeedResult};
use mytune_core::tweaks::system_cmd;
use mytune_core::vpn::{self, SysEnv, VpnEntry};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{mpsc, Arc};
use std::time::Duration;

type Rx<T> = Option<mpsc::Receiver<T>>;

enum SpeedMsg {
    Live(Phase, f64),
    Done(Result<SpeedResult, String>),
}

enum RepairMsg {
    Line(String),
    Done,
}

struct VpnDone {
    id: String,
    result: Result<(), String>,
}

pub struct InternetPage {
    engine: Arc<Engine>,
    tweaks: Arc<Vec<NetTweak>>,
    applied: Vec<bool>,
    applied_rx: Rx<Vec<bool>>,
    tweak_rx: Rx<(String, bool)>,
    /// Row of the tweak being switched while `tweak_rx` is set.
    tweak_busy: usize,
    pub(crate) status: Option<(String, bool)>,

    speed_rx: Rx<SpeedMsg>,
    speed_cancel: Arc<AtomicBool>,
    speed_live: Option<(Phase, f64)>,
    speed: Option<Result<SpeedResult, String>>,
    /// Last download reading of the running test, kept once the upload starts.
    speed_down: Option<f64>,
    /// `ctx` time the running test started at (the gauge's needle sweeps once from it).
    speed_started: Option<f64>,
    /// Download and upload readings of the running test.
    speed_samples: [Vec<f64>; 2],
    /// The last finished test, also after a restart. A stopped or failed run leaves it alone.
    speed_last: Option<LastSpeed>,

    health_rx: Rx<Vec<connhealth::Row>>,
    health: Vec<connhealth::Row>,

    repair_rx: Rx<RepairMsg>,
    repair_log: String,

    vpns: Option<Vec<VpnEntry>>,
    scan_rx: Rx<Result<Vec<VpnEntry>, String>>,
    vpn_rx: Rx<VpnDone>,
    /// ids being switched -> cancel flag
    vpn_busy: HashMap<String, Arc<AtomicBool>>,
    vpn_error: Option<String>,
    started: bool,
}

fn text_of(e: &dyn std::fmt::Display) -> String {
    e.to_string()
}

/// Reads every tweak's state on a thread (about 5 s: eight PowerShell starts).
fn read_states(tw: Arc<Vec<NetTweak>>, ctx: Option<egui::Context>) -> Rx<Vec<bool>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(tw.iter().map(|t| t.tweak.is_applied().unwrap_or(false)).collect());
        if let Some(ctx) = ctx {
            ctx.request_repaint();
        }
    });
    Some(rx)
}

impl InternetPage {
    /// Also starts the VPN watcher (it must run while the app sits in the tray, not only on this page).
    pub fn new(engine: Arc<Engine>) -> InternetPage {
        let e = engine.clone();
        vpn::start_watcher(Arc::new(move || e.with_store(|s| vpn::wanted(s))));
        let tweaks = Arc::new(netrepair::net_tweaks(system_cmd()).unwrap_or_default());
        let speed_last = engine.with_store(|s| speedtest::load_last(s.dir()));
        InternetPage {
            engine,
            applied: vec![false; tweaks.len()],
            // Read at app start, so the switches are right when the page is first opened.
            applied_rx: read_states(tweaks.clone(), None),
            tweaks,
            tweak_rx: None,
            tweak_busy: 0,
            status: None,
            speed_rx: None,
            speed_cancel: Arc::new(AtomicBool::new(false)),
            speed_live: None,
            speed: None,
            speed_down: None,
            speed_started: None,
            speed_samples: Default::default(),
            speed_last,
            health_rx: None,
            health: Vec::new(),
            repair_rx: None,
            repair_log: String::new(),
            vpns: None,
            scan_rx: None,
            vpn_rx: None,
            vpn_busy: HashMap::new(),
            vpn_error: None,
            started: false,
        }
    }

    fn read_applied(&mut self, ctx: &egui::Context) {
        self.applied_rx = read_states(self.tweaks.clone(), Some(ctx.clone()));
    }

    fn rescan(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(vpn::scan(&SysEnv).map_err(|e| text_of(&e)));
            ctx.request_repaint();
        });
        self.scan_rx = Some(rx);
    }

    fn toggle_tweak(&mut self, ctx: &egui::Context, i: usize, on: bool) {
        let (tx, rx) = mpsc::channel();
        let (tw, eng, ctx) = (self.tweaks.clone(), self.engine.clone(), ctx.clone());
        std::thread::spawn(move || {
            let t = &tw[i].tweak;
            let r = if on {
                mytune_core::sysreport::restore_point_once(&*system_cmd());
                eng.apply(&**t).map_err(|e| e.to_string())
            } else {
                match eng.revert(&**t) {
                    Ok(mytune_core::engine::RevertOutcome::Reverted) => Ok(()),
                    Ok(mytune_core::engine::RevertOutcome::NoBackup) => Err("it is already at your driver's default, nothing changed".to_string()),
                    Err(e) => Err(e.to_string()),
                }
            };
            let msg = crate::notice::toggled(t.title(), on, r.err().as_deref());
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
        self.tweak_busy = i;
        self.tweak_rx = Some(rx);
    }

    fn start_speed(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.speed_cancel = cancel.clone();
        self.speed = None;
        self.speed_live = Some((Phase::Ping, 0.0));
        self.speed_down = None;
        self.speed_samples = Default::default();
        self.speed_started = Some(ctx.input(|i| i.time));
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let (t2, c2) = (tx.clone(), ctx.clone());
            let r = speedtest::run_test(
                &move |ph, m| {
                    let _ = t2.send(SpeedMsg::Live(ph, m));
                    c2.request_repaint();
                },
                &cancel,
            );
            let _ = tx.send(SpeedMsg::Done(r));
            ctx.request_repaint();
        });
        self.speed_rx = Some(rx);
    }

    fn start_repair(&mut self, ctx: &egui::Context, key: &'static str) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let t2 = tx.clone();
            let c2 = ctx.clone();
            netrepair::repair(&*system_cmd(), key, &mut |s| {
                let _ = t2.send(RepairMsg::Line(s.to_string()));
                c2.request_repaint();
            });
            let _ = tx.send(RepairMsg::Done);
            ctx.request_repaint();
        });
        self.repair_rx = Some(rx);
    }

    fn switch_vpn(&mut self, ctx: &egui::Context, id: String, on: bool) {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        self.vpn_busy.insert(id.clone(), cancel.clone());
        self.vpn_error = None;
        let (eng, ctx) = (self.engine.clone(), ctx.clone());
        std::thread::spawn(move || {
            let result = vpn::set(&SysEnv, &id, on, &cancel).map_err(|e| text_of(&e));
            if result.is_ok() {
                // Only a switch that worked is remembered, so a VPN that won't connect isn't retried forever.
                if let Err(e) = eng.with_store(|s| vpn::remember(s, &id, on)) {
                    mytune_core::log::error("saving vpn state", Some(&e));
                }
            }
            let _ = tx.send(VpnDone { id, result });
            ctx.request_repaint();
        });
        self.vpn_rx = Some(rx);
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(v) = self.applied_rx.as_ref().and_then(|r| r.try_recv().ok()) {
            self.applied = v;
            self.applied_rx = None;
        }
        if let Some(m) = self.tweak_rx.as_ref().and_then(|r| r.try_recv().ok()) {
            self.status = Some(m);
            self.tweak_rx = None;
            self.read_applied(ctx);
        }
        let mut done = false;
        let mut msgs = vec![];
        if let Some(rx) = &self.speed_rx {
            while let Ok(m) = rx.try_recv() {
                msgs.push(m);
            }
        }
        for m in msgs {
            match m {
                SpeedMsg::Live(ph, v) => {
                    if let (Phase::Upload, Some((Phase::Download, d))) = (ph, self.speed_live) {
                        self.speed_down = Some(d);
                    }
                    self.speed_live = Some((ph, v));
                    match ph {
                        Phase::Download => self.speed_samples[0].push(v),
                        Phase::Upload => self.speed_samples[1].push(v),
                        Phase::Ping => {}
                    }
                }
                SpeedMsg::Done(r) => {
                    if let Ok(result) = r {
                        let [down, up] = std::mem::take(&mut self.speed_samples);
                        let mut at = mytune_core::util::local_stamp();
                        at.truncate(16); // no seconds
                        let last = LastSpeed { at, result, down, up };
                        self.speed_last = Some(last.clone());
                        let engine = self.engine.clone();
                        std::thread::spawn(move || {
                            let dir = engine.with_store(|s| s.dir().to_path_buf());
                            if let Err(e) = speedtest::save_last(&dir, &last) {
                                mytune_core::log::error("saving speed test", Some(&e));
                            }
                        });
                    }
                    self.speed = Some(r);
                    self.speed_live = None;
                    done = true;
                }
            }
        }
        if done {
            self.speed_rx = None;
        }
        if let Some(r) = self.health_rx.as_ref().map(|r| r.try_recv()) {
            match r {
                Ok(rows) => {
                    self.health = rows;
                    self.health_rx = None;
                }
                Err(mpsc::TryRecvError::Disconnected) => self.health_rx = None,
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        let mut lines = vec![];
        let mut finished = false;
        if let Some(rx) = &self.repair_rx {
            while let Ok(m) = rx.try_recv() {
                match m {
                    RepairMsg::Line(s) => lines.push(s),
                    RepairMsg::Done => finished = true,
                }
            }
        }
        for l in lines {
            self.repair_log.push_str(&l);
        }
        if finished {
            self.repair_rx = None;
        }
        if let Some(r) = self.scan_rx.as_ref().and_then(|r| r.try_recv().ok()) {
            self.scan_rx = None;
            match r {
                Ok(v) => self.vpns = Some(v),
                Err(e) => {
                    self.vpns.get_or_insert_with(Vec::new);
                    self.vpn_error = Some(e);
                }
            }
        }
        if let Some(d) = self.vpn_rx.as_ref().and_then(|r| r.try_recv().ok()) {
            self.vpn_rx = None;
            self.vpn_busy.remove(&d.id);
            if let Err(e) = d.result {
                self.vpn_error = Some(e);
            }
            self.rescan(ctx);
        }
    }

    fn busy(&self) -> bool {
        self.applied_rx.is_some()
            || self.tweak_rx.is_some()
            || self.speed_rx.is_some()
            || self.health_rx.is_some()
            || self.repair_rx.is_some()
            || self.scan_rx.is_some()
            || self.vpn_rx.is_some()
    }

    pub fn ui(&mut self, ui: &mut Ui, search: &str) {
        let ctx = ui.ctx().clone();
        if !self.started {
            self.started = true;
            // Keep a start read that is still running; read again only once it is in (a start at
            // Windows boot may have read before the adapters were up).
            if let Some(v) = self.applied_rx.as_ref().and_then(|r| r.try_recv().ok()) {
                self.applied = v;
                self.read_applied(&ctx);
            }
            self.rescan(&ctx);
        }
        self.poll(&ctx);
        if self.busy() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        let q = search.to_lowercase();
        let show = |s: &str| q.is_empty() || s.to_lowercase().contains(&q);

        if let Some((msg, ok)) = self.status.take() {
            crate::notice::post(&ctx, msg, ok);
        }
        if show("speed test download upload ping jitter") {
            self.speed_ui(ui, &ctx);
        }
        if show("connection health cable router ping packet loss adapter errors link speed wi-fi wifi signal band channel") {
            self.health_ui(ui, &ctx);
        }
        if self.tweaks.iter().any(|n| show(n.tweak.title()) || show(&n.description)) {
            th::section(ui, "Tweaks");
        }
        self.tweaks_ui(ui, &ctx, &show);
        if show("network repair dns winsock tcp ip reset flush renew") {
            self.repair_ui(ui, &ctx);
        }
        if show("vpn") {
            self.vpn_ui(ui, &ctx);
        }
    }

    fn tweaks_ui(&mut self, ui: &mut Ui, ctx: &egui::Context, show: &dyn Fn(&str) -> bool) {
        let mut click = None;
        let working = self.tweak_rx.is_some();
        // one plate, a row per tweak (as on Optimize)
        th::group(ui, |ui| {
            for (i, n) in self.tweaks.iter().enumerate() {
                if !show(n.tweak.title()) && !show(&n.description) {
                    continue;
                }
                th::card(ui, (10, 13, 14, 13), |ui| {
                    ui.horizontal(|ui| {
                        let avail = ui.available_width();
                        ui.vertical(|ui| {
                            ui.set_max_width((avail - 120.0).max(120.0));
                            ui.label(th::md(n.tweak.title(), 14.0));
                            ui.add_space(2.0);
                            th::flag_chips(ui, n.flags);
                            th::more_info(ui, egui::Id::new(("more_info_net", i)), &n.description);
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if working && i == self.tweak_busy {
                                ui.label(rt("Working…", 12.0).color(text3()));
                            } else if th::switch(ui, self.applied[i]).on_hover_text(&n.details).clicked() && !working {
                                click = Some((i, !self.applied[i]));
                            }
                        });
                    });
                });
            }
        });
        if let Some((i, on)) = click {
            self.toggle_tweak(ctx, i, on);
        }
    }

    /// Shot tests: the page as it looks in the middle of a test.
    #[doc(hidden)]
    pub fn fake_speed(&mut self, phase: Phase, down: &[f64], up: &[f64]) {
        self.speed_rx = Some(mpsc::channel().1);
        let now = if phase == Phase::Upload { up } else { down };
        self.speed_live = Some((phase, now.last().copied().unwrap_or(0.0)));
        self.speed_down = down.last().copied().filter(|_| phase == Phase::Upload);
        self.speed_samples = [down.to_vec(), up.to_vec()];
    }

    /// Shot tests: the page right after a finished test.
    #[doc(hidden)]
    pub fn fake_speed_done(&mut self, last: LastSpeed) {
        self.speed = Some(Ok(last.result));
        self.speed_last = Some(last);
    }

    fn speed_ui(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        use crate::speedgauge::{self as sg, Gauge};
        th::section(ui, "Speed test");
        let running = self.speed_rx.is_some();
        let (mut start, mut stop) = (false, false);
        th::card(ui, (10, 16, 16, 16), |ui| {
            let result = self.speed.as_ref().and_then(|r| r.as_ref().ok());
            let last = self.speed_last.as_ref().filter(|_| !running);
            ui.horizontal(|ui| {
                ui.set_min_height(32.0); // the button comes and goes; the row keeps its height
                ui.label(sb("Download, upload and ping", 14.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if running {
                        stop = th::button(ui, "Stop", false, th::BTN).clicked();
                    } else if result.is_some() {
                        start |= th::button(ui, "Test again", false, th::BTN).clicked();
                    }
                    if let Some(l) = last {
                        ui.label(rt(format!("Last test · {}", l.at), 12.0).color(text3()));
                    }
                });
            });
            let (value, color, phase) = match (self.speed_live, result) {
                (Some((Phase::Ping, _)), _) => (0.0, text3(), "Ping"),
                (Some((Phase::Download, v)), _) => (v, sg::DOWN, "Download"),
                (Some((Phase::Upload, v)), _) => (v, sg::UP, "Upload"),
                (None, Some(r)) => (r.down_mbps, sg::DOWN, "Download"),
                _ => (0.0, text3(), ""),
            };
            let gauge = Gauge { open: running || result.is_some(), value, color, phase, started: self.speed_started.filter(|_| running) };
            let (down, up): (&[f64], &[f64]) = match last {
                Some(l) => (&l.down, &l.up),
                None if running => (&self.speed_samples[0], &self.speed_samples[1]),
                None => (&[], &[]),
            };
            let (down_now, up_now) = match (self.speed_live, last) {
                (Some((Phase::Download, v)), _) => (Some(v), None),
                (Some((Phase::Upload, v)), _) => (self.speed_down, Some(v)),
                (_, Some(l)) => (Some(l.result.down_mbps), Some(l.result.up_mbps)),
                _ => (None, None),
            };
            // The ping comes with the result: the test reports no ping reading on the way.
            let stats = match last {
                Some(l) => [format!("{:.0} ms", l.result.ping_ms), format!("{:.1} ms", l.result.jitter_ms), format!("{:.0} ms", l.result.loaded_ms)],
                None => [if running { "…" } else { "—" }.to_string(), if running { "…" } else { "—" }.to_string(), if running { "…" } else { "—" }.to_string()],
            };
            let error = self.speed.as_ref().filter(|_| !running).and_then(|r| r.as_ref().err()).filter(|e| *e != "Stopped.");
            let note = match (error, last) {
                (Some(e), _) => Some((e.as_str(), th::DANGER)),
                (None, Some(l)) if l.result.loaded_ms > l.result.ping_ms + 50.0 => {
                    Some(("Ping rises a lot while the line is busy. A router with smart queue management (SQM) helps.", th::WARNING))
                }
                _ => None,
            };
            let side = sg::Side { down, up, down_now, up_now, running, stats, note };
            // Below this the graph would be too short to read beside the dial.
            if ui.available_width() >= 600.0 {
                ui.horizontal(|ui| {
                    ui.allocate_ui(egui::vec2(250.0, sg::HEIGHT), |ui| start |= sg::show(ui, &gauge));
                    ui.add_space(16.0);
                    sg::side(ui, &side);
                });
            } else {
                start |= sg::show(ui, &gauge);
                ui.add_space(8.0);
                sg::side(ui, &side);
            }
        });
        if stop {
            self.speed_cancel.store(true, Relaxed);
        }
        if start && !running {
            self.start_speed(ctx);
        }
    }

    /// Shot tests: the card with finished rows.
    #[doc(hidden)]
    pub fn fake_health(&mut self, rows: Vec<connhealth::Row>) {
        self.health = rows;
    }

    fn health_ui(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        th::section(ui, "Connection health");
        let running = self.health_rx.is_some();
        let mut start = false;
        th::group(ui, |ui| {
            th::card(ui, (10, 16, 16, 16), |ui| {
                ui.horizontal(|ui| {
                    ui.set_min_height(32.0);
                    ui.vertical(|ui| {
                        ui.label(sb("Find bad cables, weak links and packet loss", 14.0));
                        ui.label(rt("Read-only. Takes about 10 seconds and changes nothing.", 12.0).color(text2()));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if running {
                            ui.label(rt("Checking…", 12.0).color(text3()));
                        } else {
                            start = th::button(ui, "Check", true, th::BTN).clicked();
                        }
                    });
                });
            });
            for r in &self.health {
                th::card(ui, (10, 13, 14, 13), |ui| {
                    ui.horizontal(|ui| {
                        let avail = ui.available_width();
                        ui.vertical(|ui| {
                            ui.set_max_width((avail - 120.0).max(120.0));
                            ui.label(sb(r.name, 14.0));
                            ui.label(rt(&r.text, 12.0).color(text2()));
                            if let Some(fix) = r.fix {
                                ui.label(rt(fix, 12.0).color(text3()));
                            }
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let (fg, bg) = match r.grade {
                                Grade::Good => (th::SUCCESS, th::success_soft()),
                                Grade::Warning => (th::WARNING, th::WARNING.gamma_multiply(0.15)),
                                Grade::Problem => (th::DANGER, th::DANGER.gamma_multiply(0.15)),
                                Grade::Info => (text2(), th::control_bg()),
                                Grade::NotMeasured => (text3(), th::control_bg()),
                            };
                            th::chip(ui, r.grade.label(), fg, bg);
                        });
                    });
                });
            }
        });
        if start {
            let (tx, rx) = mpsc::channel();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(connhealth::run(&*system_cmd()));
                ctx.request_repaint();
            });
            self.health_rx = Some(rx);
        }
    }

    fn repair_ui(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        th::section(ui, "Network repair");
        let running = self.repair_rx.is_some();
        let mut start: Option<&'static str> = None;
        th::card(ui, (10, 16, 16, 16), |ui| {
            ui.label(sb("Fix a connection that misbehaves", 14.0));
            ui.label(rt("Winsock and TCP/IP resets need a restart of the PC.", 12.0).color(text2()));
            ui.add_space(8.0);
            ui.add_enabled_ui(!running, |ui| {
                ui.horizontal_wrapped(|ui| {
                    for (label, key) in [
                        ("Flush DNS", "flushdns"),
                        ("Renew IP", "renew"),
                        ("Register DNS", "registerdns"),
                        ("Reset Winsock", "winsock"),
                        ("Reset TCP/IP", "tcpip"),
                    ] {
                        if th::button(ui, label, false, th::BTN).clicked() {
                            start = Some(key);
                        }
                    }
                    if th::button(ui, "Run full repair", true, th::BTN).clicked() {
                        start = Some("all");
                    }
                });
            });
            if !self.repair_log.is_empty() {
                ui.add_space(8.0);
                ui.label(rt(self.repair_log.trim_end(), 12.0).color(text2()));
            }
        });
        if let Some(k) = start {
            self.repair_log.clear();
            self.start_repair(ctx, k);
        }
    }

    fn vpn_ui(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        th::section(ui, "VPN");
        let mut rescan = false;
        let mut switch: Option<(String, bool)> = None;
        let mut cancel: Option<String> = None;
        // One plate: the header row, then each VPN as a row under it (not cards of their own).
        th::group(ui, |ui| {
        th::card(ui, (10, 16, 16, 16), |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(sb("VPNs on this PC", 14.0));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.scan_rx.is_some() {
                        ui.label(rt("Scanning…", 12.0).color(text3()));
                    } else if th::button(ui, "Rescan", false, th::BTN).clicked() {
                        rescan = true;
                    }
                });
            });
            if let Some(e) = &self.vpn_error {
                ui.add_space(6.0);
                ui.label(rt(e, 12.0).color(th::DANGER));
            }
        });
        match &self.vpns {
            None => {}
            Some(l) if l.is_empty() => {
                th::card(ui, (10, 16, 16, 16), |ui| {
                    ui.label(rt("No VPN found.", 13.0).color(text2()));
                });
            }
            Some(list) => {
                for v in list {
                    let busy = self.vpn_busy.contains_key(&v.id);
                    th::card(ui, (10, 16, 16, 16), |ui| {
                        ui.horizontal(|ui| {
                            let avail = ui.available_width();
                            ui.vertical(|ui| {
                                ui.set_max_width((avail - 140.0).max(120.0));
                                ui.label(sb(&v.name, 14.0));
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if busy {
                                    if th::button(ui, "Cancel", false, th::BTN).clicked() {
                                        cancel = Some(v.id.clone());
                                    }
                                    ui.label(rt("Working…", 12.0).color(text3()));
                                } else if v.pending {
                                    if th::button(ui, "Cancel", false, th::BTN).clicked() {
                                        cancel = Some(v.id.clone());
                                    }
                                } else if v.controllable {
                                    if th::switch(ui, v.connected).clicked() {
                                        switch = Some((v.id.clone(), !v.connected));
                                    }
                                } else {
                                    th::chip(ui, "Status only", text2(), th::control_bg());
                                }
                            });
                        });
                    });
                }
            }
        }
        });
        ui.add_space(th::GAP);
        if let Some(id) = cancel {
            if let Some(f) = self.vpn_busy.get(&id) {
                f.store(true, Relaxed); // the switching thread cancels its own attempt
            } else {
                let ctx2 = ctx.clone();
                std::thread::spawn(move || {
                    if let Err(e) = vpn::cancel_connect(&SysEnv, &id) {
                        mytune_core::log::error("vpn cancel", Some(&e));
                    }
                    ctx2.request_repaint();
                });
                rescan = true;
            }
        }
        if let Some((id, on)) = switch {
            self.switch_vpn(ctx, id, on);
        }
        if rescan && self.scan_rx.is_none() {
            self.rescan(ctx);
        }
    }
}
