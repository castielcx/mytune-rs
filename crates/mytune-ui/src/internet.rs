//! Internet page: net.* tweaks, speed test, network repair, VPN list. All slow work runs on threads and
//! reports through channels, so the window never blocks (adapter tweaks can take 30 s).

use crate::theme::{self as th, bd, rt, sb, text2, text3};
use eframe::egui::{self, Align, Layout, Ui};
use mytune_core::catalog::TweakFlags;
use mytune_core::engine::Engine;
use mytune_core::netrepair::{self, NetTweak};
use mytune_core::speedtest::{self, Phase, SpeedResult};
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

    repair_rx: Rx<RepairMsg>,
    repair_log: String,
    test_rx: Rx<String>,
    test_text: String,

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

impl InternetPage {
    /// Also starts the VPN watcher (it must run while the app sits in the tray, not only on this page).
    pub fn new(engine: Arc<Engine>) -> InternetPage {
        let e = engine.clone();
        vpn::start_watcher(Arc::new(move || e.with_store(|s| vpn::wanted(s))));
        let tweaks = Arc::new(netrepair::net_tweaks(system_cmd()).unwrap_or_default());
        InternetPage {
            engine,
            applied: vec![false; tweaks.len()],
            tweaks,
            applied_rx: None,
            tweak_rx: None,
            tweak_busy: 0,
            status: None,
            speed_rx: None,
            speed_cancel: Arc::new(AtomicBool::new(false)),
            speed_live: None,
            speed: None,
            repair_rx: None,
            repair_log: String::new(),
            test_rx: None,
            test_text: String::new(),
            vpns: None,
            scan_rx: None,
            vpn_rx: None,
            vpn_busy: HashMap::new(),
            vpn_error: None,
            started: false,
        }
    }

    fn read_applied(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let (tw, ctx) = (self.tweaks.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(tw.iter().map(|t| t.tweak.is_applied().unwrap_or(false)).collect());
            ctx.request_repaint();
        });
        self.applied_rx = Some(rx);
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

    fn start_test(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        self.test_text = "Testing…".into();
        std::thread::spawn(move || {
            let _ = tx.send(netrepair::test_connection());
            ctx.request_repaint();
        });
        self.test_rx = Some(rx);
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
                SpeedMsg::Live(ph, v) => self.speed_live = Some((ph, v)),
                SpeedMsg::Done(r) => {
                    self.speed = Some(r);
                    self.speed_live = None;
                    done = true;
                }
            }
        }
        if done {
            self.speed_rx = None;
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
        if let Some(t) = self.test_rx.as_ref().and_then(|r| r.try_recv().ok()) {
            self.test_text = t;
            self.test_rx = None;
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
            || self.repair_rx.is_some()
            || self.test_rx.is_some()
            || self.scan_rx.is_some()
            || self.vpn_rx.is_some()
    }

    pub fn ui(&mut self, ui: &mut Ui, search: &str) {
        let ctx = ui.ctx().clone();
        if !self.started {
            self.started = true;
            self.read_applied(&ctx);
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
        for (i, n) in self.tweaks.iter().enumerate() {
            if !show(n.tweak.title()) && !show(&n.description) {
                continue;
            }
            th::card(ui, (10, 16, 16, 16), |ui| {
                ui.horizontal(|ui| {
                    let avail = ui.available_width();
                    ui.vertical(|ui| {
                        ui.set_max_width((avail - 120.0).max(120.0));
                        ui.label(sb(n.tweak.title(), 14.0));
                        ui.label(rt(&n.description, 12.0).color(text2()));
                        th::flag_chips(ui, n.flags);
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
            ui.add_space(8.0);
        }
        if let Some((i, on)) = click {
            self.toggle_tweak(ctx, i, on);
        }
    }

    fn speed_ui(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        th::section(ui, "Speed test");
        let running = self.speed_rx.is_some();
        th::card(ui, (10, 16, 16, 16), |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(sb("Download, upload and ping", 14.0));
                    let sub = match (&self.speed_live, &self.speed) {
                        (Some((Phase::Ping, _)), _) => "Measuring ping…".to_string(),
                        (Some((ph, v)), _) => format!("{}: {v:.0} Mbit/s", if *ph == Phase::Download { "Download" } else { "Upload" }),
                        (None, Some(Err(e))) => e.clone(),
                        (None, Some(Ok(_))) => "Done.".into(),
                        _ => "Uses Cloudflare's public speed servers. About 20 seconds and some data.".into(),
                    };
                    let col = if matches!(self.speed, Some(Err(_))) { th::DANGER } else { text2() };
                    ui.label(rt(sub, 12.0).color(col));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if running {
                        if th::button(ui, "Stop", false, th::BTN).clicked() {
                            self.speed_cancel.store(true, Relaxed);
                        }
                    } else if th::button(ui, "Start", true, th::BTN).clicked() {
                        self.start_speed(ctx);
                    }
                });
            });
            if let Some(Ok(r)) = &self.speed {
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    for (label, val) in [
                        ("Download", format!("{:.0} Mbit/s", r.down_mbps)),
                        ("Upload", format!("{:.0} Mbit/s", r.up_mbps)),
                        ("Ping", format!("{:.0} ms", r.ping_ms)),
                        ("Jitter", format!("{:.1} ms", r.jitter_ms)),
                        ("Ping under load", format!("{:.0} ms", r.loaded_ms)),
                    ] {
                        ui.vertical(|ui| {
                            ui.set_min_width(110.0);
                            ui.label(rt(label, 11.0).color(text3()));
                            ui.label(bd(val, 18.0));
                        });
                    }
                });
                if r.loaded_ms > r.ping_ms + 50.0 {
                    ui.add_space(6.0);
                    ui.label(rt("Ping rises a lot while the line is busy. A router with smart queue management (SQM) helps.", 12.0).color(th::WARNING));
                }
            }
        });
        ui.add_space(8.0);
    }

    fn repair_ui(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        th::section(ui, "Network repair");
        let running = self.repair_rx.is_some();
        let mut start: Option<&'static str> = None;
        let mut test = false;
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
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if th::button(ui, "Test connection", false, th::BTN).clicked() && self.test_rx.is_none() {
                    test = true;
                }
                ui.label(rt(&self.test_text, 12.0).color(text2()));
            });
        });
        ui.add_space(8.0);
        if let Some(k) = start {
            self.repair_log.clear();
            self.start_repair(ctx, k);
        }
        if test {
            self.start_test(ctx);
        }
    }

    fn vpn_ui(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        th::section(ui, "VPN");
        let mut rescan = false;
        let mut switch: Option<(String, bool)> = None;
        let mut cancel: Option<String> = None;
        th::card(ui, (10, 16, 16, 16), |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(sb("VPNs on this PC", 14.0));
                    ui.label(rt("MyTune keeps a VPN you switched on connected, also after a restart.", 12.0).color(text2()));
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
        ui.add_space(8.0);
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
                                ui.label(rt(&v.detail, 12.0).color(text2()));
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
                    ui.add_space(8.0);
                }
            }
        }
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
