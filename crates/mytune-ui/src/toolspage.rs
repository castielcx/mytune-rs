//! Tools page, laid out like `Views/ToolsPage.xaml`: junk cleaner, startup programs, shader caches, drives
//! and system files, stability log. Slow work runs on threads and reports through one channel.

use crate::theme::{self as th, rt, text2, text3, TEXT};
use crate::widgets::{self as w, caption, caption3};
use eframe::egui::{self, Align, Id, Layout, Ui};
use mytune_core::catalog::{registry_tweaks, DataTweak, SystemHooks};
use mytune_core::cleaner::{self, CleanTarget, ShaderCache};
use mytune_core::engine::Engine;
use mytune_core::stability::{self, ChangeEntry, StabilityEvent};
use mytune_core::startup::{self, StartupEntry};
use mytune_core::tools::{self, Disk};
use mytune_core::tweaks::system_cmd;
use std::collections::HashMap;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

enum Msg {
    Sizes(Vec<u64>),
    CleanProgress(String),
    CleanDone(u64),
    Startup(Vec<StartupEntry>),
    Shaders(Vec<ShaderCache>),
    ShaderCleared(&'static str, u64),
    Disks(Result<Vec<Disk>, String>),
    Stability(Result<(Vec<StabilityEvent>, Vec<ChangeEntry>), String>),
}

pub struct ToolsPage {
    engine: Arc<Engine>,
    tweaks: Vec<DataTweak>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    pub(crate) status: Option<(String, bool)>,
    started: bool,

    targets: Arc<Vec<CleanTarget>>,
    selected: Vec<bool>,
    sizes: Vec<Option<u64>>,
    clean_summary: String,
    clean_busy: bool,

    startup: Option<Vec<StartupEntry>>,
    shaders: Option<Vec<ShaderCache>>,
    shader_busy: bool,
    disks: Option<Result<Vec<Disk>, String>>,

    stab_open: bool,
    stab_all: bool,
    stab_busy: bool,
    stab: Option<Result<(Vec<StabilityEvent>, Vec<ChangeEntry>), String>>,
    /// change id -> new label of its button after "Switch off"
    undone: HashMap<String, String>,
}

impl ToolsPage {
    pub fn new(engine: Arc<Engine>) -> ToolsPage {
        let (tx, rx) = channel();
        let targets = Arc::new(cleaner::targets());
        ToolsPage {
            engine,
            tweaks: registry_tweaks(Arc::new(SystemHooks)).unwrap_or_default(),
            tx,
            rx,
            status: None,
            started: false,
            selected: targets.iter().map(|t| t.default_on).collect(),
            sizes: vec![None; targets.len()],
            targets,
            clean_summary: "Scan to see how much space you can free.".into(),
            clean_busy: false,
            startup: None,
            shaders: None,
            shader_busy: false,
            disks: None,
            stab_open: false,
            stab_all: false,
            stab_busy: false,
            stab: None,
            undone: HashMap::new(),
        }
    }

    fn spawn(&self, ctx: &egui::Context, f: impl FnOnce(&Sender<Msg>) + Send + 'static) {
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            f(&tx);
            ctx.request_repaint();
        });
    }

    fn scan(&mut self, ctx: &egui::Context) {
        self.clean_busy = true;
        self.clean_summary = "Scanning…".into();
        let t = self.targets.clone();
        self.spawn(ctx, move |tx| {
            let _ = tx.send(Msg::Sizes(t.iter().map(cleaner::size).collect()));
        });
    }

    fn clean(&mut self, ctx: &egui::Context) {
        self.clean_busy = true;
        let t = self.targets.clone();
        let pick = self.selected.clone();
        let ctx2 = ctx.clone();
        self.spawn(ctx, move |tx| {
            let mut freed = 0;
            for (i, target) in t.iter().enumerate().filter(|(i, _)| pick[*i]) {
                let _ = tx.send(Msg::CleanProgress(format!("Cleaning {}…", target.name)));
                ctx2.request_repaint();
                freed += cleaner::clean(target);
            }
            let _ = tx.send(Msg::CleanDone(freed));
        });
    }

    fn load_startup(&self, ctx: &egui::Context) {
        self.spawn(ctx, |tx| {
            let _ = tx.send(Msg::Startup(startup::list()));
        });
    }

    fn load_shaders(&mut self, ctx: &egui::Context) {
        self.shader_busy = true;
        self.spawn(ctx, |tx| {
            let _ = tx.send(Msg::Shaders(cleaner::shader_caches()));
        });
    }

    fn load_stability(&mut self, ctx: &egui::Context) {
        self.stab_busy = true;
        let (dir, all) = (self.engine.with_store(|s| s.dir().to_path_buf()), self.stab_all);
        self.spawn(ctx, move |tx| {
            let r = stability::events(&*system_cmd(), &dir, all).map(|e| (e, stability::changes(&dir, stability::now()))).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Stability(r));
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Sizes(v) => {
                    self.sizes = v.into_iter().map(Some).collect();
                    self.clean_busy = false;
                    self.update_summary();
                }
                Msg::CleanProgress(s) => self.clean_summary = s,
                Msg::CleanDone(freed) => {
                    self.status = Some((format!("Freed {}. Files in use were skipped.", cleaner::fmt_size(freed)), true));
                    self.scan(ctx);
                }
                Msg::Startup(v) => self.startup = Some(v),
                Msg::Shaders(v) => {
                    self.shaders = Some(v);
                    self.shader_busy = false;
                }
                Msg::ShaderCleared(name, freed) => {
                    self.status = Some((format!("{name}: freed {}. Games rebuild it, so the first launch may stutter.", shader_fmt(freed)), true));
                    self.load_shaders(ctx);
                }
                Msg::Disks(r) => self.disks = Some(r),
                Msg::Stability(r) => {
                    self.stab = Some(r);
                    self.stab_busy = false;
                }
            }
        }
    }

    fn update_summary(&mut self) {
        let total: u64 = (0..self.targets.len()).filter(|&i| self.selected[i]).filter_map(|i| self.sizes[i]).sum();
        self.clean_summary = format!("{} can be freed from the selected items.", cleaner::fmt_size(total));
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        if !self.started {
            self.started = true;
            self.load_startup(&ctx);
            self.load_shaders(&ctx);
            self.spawn(&ctx, |tx| {
                let _ = tx.send(Msg::Disks(tools::disks(&*system_cmd()).map_err(|e| e.to_string())));
            });
            self.scan(&ctx);
        }
        self.poll(&ctx);
        if self.clean_busy || self.shader_busy || self.stab_busy {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        caption(ui, "Clean junk files, choose what starts with Windows and check your drives. The PC check is on the Home page.");
        if let Some((m, ok)) = &self.status {
            ui.add_space(6.0);
            ui.label(rt(m, 12.0).color(if *ok { th::SUCCESS } else { th::DANGER }));
        }
        self.junk_card(ui, &ctx);
        self.startup_card(ui, &ctx);
        self.shader_card(ui, &ctx);
        self.drives_card(ui);
        self.stability_card(ui, &ctx);
    }

    fn heading(ui: &mut Ui, text: &str) {
        th::section(ui, text);
    }

    fn junk_card(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        Self::heading(ui, "Junk cleaner");
        let (mut scan, mut clean, mut toggled) = (false, false, false);
        th::card(ui, (0, 0, 0, 0), |ui| {
            egui::Frame::new().inner_margin(egui::Margin { left: 18, right: 18, top: 14, bottom: 6 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                for i in 0..self.targets.len() {
                    let t = &self.targets[i];
                    ui.horizontal(|ui| {
                        let avail = ui.available_width();
                        ui.vertical(|ui| {
                            ui.set_max_width((avail - 90.0).max(120.0));
                            toggled |= w::checkbox(ui, Id::new(("clean", i)), &mut self.selected[i], |ui| {
                                ui.label(rt(t.name, 14.0));
                                ui.label(rt(t.description, 12.0).color(text3()));
                            });
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(rt(self.sizes[i].map(cleaner::fmt_size).unwrap_or_default(), 14.0).color(text2()));
                        });
                    });
                    ui.add_space(10.0);
                }
            });
            ui.painter().hline(ui.min_rect().x_range(), ui.cursor().top(), egui::Stroke::new(1.0, egui::Color32::from_white_alpha(0x1F)));
            egui::Frame::new().inner_margin(egui::Margin::symmetric(18, 12)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    caption(ui, &self.clean_summary);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.add_enabled_ui(!self.clean_busy, |ui| {
                            if th::button(ui, "Clean selected", true, th::BTN).clicked() {
                                clean = true;
                            }
                            if th::button(ui, "Scan", false, th::BTN).clicked() {
                                scan = true;
                            }
                        });
                    });
                });
            });
        });
        if toggled && !self.clean_busy {
            self.update_summary();
        }
        if scan {
            self.scan(ctx);
        }
        if clean {
            self.clean(ctx);
        }
    }

    fn startup_card(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        Self::heading(ui, "Startup programs");
        let mut refresh = false;
        let mut flip: Option<(usize, bool)> = None;
        th::card(ui, (10, 16, 16, 16), |ui| {
            ui.horizontal(|ui| {
                let avail = ui.available_width();
                ui.vertical(|ui| {
                    ui.set_max_width((avail - 120.0).max(120.0));
                    caption(
                        ui,
                        &match &self.startup {
                            None => "Looking for startup programs…".to_string(),
                            Some(l) if l.is_empty() => "No startup programs found.".to_string(),
                            Some(l) => format!(
                                "{} of {} start with Windows. Turning one off keeps it installed, and it can be turned back on here or in Task Manager.",
                                l.iter().filter(|e| e.enabled).count(),
                                l.len()
                            ),
                        },
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if th::button(ui, "Refresh", false, th::BTN).clicked() {
                        refresh = true;
                    }
                });
            });
            if let Some(list) = &self.startup {
                ui.add_space(12.0);
                for (i, e) in list.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let avail = ui.available_width();
                        ui.vertical(|ui| {
                            ui.set_max_width((avail - 70.0).max(120.0));
                            ui.label(rt(&e.name, 14.0));
                            ui.add(egui::Label::new(rt(format!("{}: {}", e.source, e.command), 12.0).color(text3())).truncate()).on_hover_text(&e.command);
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if th::switch(ui, e.enabled).clicked() {
                                flip = Some((i, !e.enabled));
                            }
                        });
                    });
                    ui.add_space(10.0);
                }
            }
        });
        if let Some((i, on)) = flip {
            let id = self.startup.as_ref().map(|l| l[i].id.clone()).unwrap_or_default();
            match startup::set(&id, on) {
                Ok(()) => {
                    if let Some(l) = &mut self.startup {
                        l[i].enabled = on;
                    }
                }
                Err(e) => {
                    let name = self.startup.as_ref().map(|l| l[i].name.clone()).unwrap_or_default();
                    mytune_core::log::error("startup toggle", Some(&e));
                    self.status = Some((format!("{name}: {e}"), false));
                }
            }
        }
        if refresh {
            self.load_startup(ctx);
        }
    }

    fn shader_card(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        Self::heading(ui, "Shader caches");
        let mut refresh = false;
        let mut clear: Option<usize> = None;
        th::card(ui, (10, 16, 16, 16), |ui| {
            ui.horizontal(|ui| {
                let avail = ui.available_width();
                ui.vertical(|ui| {
                    ui.set_max_width((avail - 120.0).max(120.0));
                    caption(
                        ui,
                        &match &self.shaders {
                            None => "Looking for shader caches…".to_string(),
                            Some(l) if l.is_empty() => "No shader caches found.".to_string(),
                            Some(l) => format!("{} in total. Clearing helps after a driver update or when a game stutters.", shader_fmt(l.iter().map(|c| c.bytes).sum())),
                        },
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_enabled_ui(!self.shader_busy, |ui| {
                        if th::button(ui, "Refresh", false, th::BTN).clicked() {
                            refresh = true;
                        }
                    });
                });
            });
            if let Some(list) = &self.shaders {
                ui.add_space(12.0);
                for (i, c) in list.iter().enumerate() {
                    ui.horizontal(|ui| {
                        let avail = ui.available_width();
                        ui.vertical(|ui| {
                            ui.set_max_width((avail - 170.0).max(120.0));
                            ui.label(rt(c.name, 14.0));
                            ui.label(rt(c.detail, 12.0).color(text3()));
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if th::button(ui, "Clear", false, th::BTN).clicked() {
                                clear = Some(i);
                            }
                            ui.add_space(8.0);
                            ui.label(rt(shader_fmt(c.bytes), 14.0).color(text2()));
                        });
                    });
                    ui.add_space(10.0);
                }
            }
        });
        if let Some(i) = clear {
            if let Some(c) = self.shaders.as_ref().and_then(|l| l.get(i)) {
                let (name, paths) = (c.name, c.paths.clone());
                self.shader_busy = true;
                self.spawn(ctx, move |tx| {
                    let _ = tx.send(Msg::ShaderCleared(name, cleaner::clear_shader_cache(&paths)));
                });
            }
        }
        if refresh {
            self.load_shaders(ctx);
        }
    }

    fn drives_card(&mut self, ui: &mut Ui) {
        Self::heading(ui, "Drives and system files");
        let (mut sfc, mut dism) = (false, false);
        th::card(ui, (10, 16, 16, 16), |ui| {
            match &self.disks {
                Some(Ok(list)) => {
                    for d in list {
                        ui.horizontal(|ui| {
                            ui.label(th::ico("\u{EDA2}", 20.0).color(text2()));
                            ui.add_space(6.0);
                            let avail = ui.available_width();
                            ui.vertical(|ui| {
                                ui.set_max_width((avail - 110.0).max(120.0));
                                ui.label(rt(&d.name, 14.0));
                                ui.label(rt(&d.detail, 12.0).color(text3()));
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| match d.health.as_str() {
                                "Healthy" => th::chip(ui, "Healthy", th::SUCCESS, th::success_soft()),
                                "Warning" => th::chip(ui, "Warning", th::WARNING, th::warning_soft()),
                                other => th::chip(ui, other, th::DANGER, th::danger_soft()),
                            });
                        });
                        ui.add_space(10.0);
                    }
                }
                Some(Err(e)) => caption(ui, &format!("Couldn't read the drives: {e}")),
                None => caption(ui, "Reading the drives…"),
            }
            ui.add_space(4.0);
            ui.painter().hline(ui.min_rect().x_range(), ui.cursor().top(), egui::Stroke::new(1.0, egui::Color32::from_white_alpha(0x1F)));
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let avail = ui.available_width();
                ui.vertical(|ui| {
                    ui.set_max_width((avail - 280.0).max(120.0));
                    caption(ui, "Check and repair Windows system files. Each opens in a console window and can take 10–30 minutes.");
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if th::button(ui, "Run DISM repair", false, th::BTN).clicked() {
                        dism = true;
                    }
                    if th::button(ui, "Run SFC scan", false, th::BTN).clicked() {
                        sfc = true;
                    }
                });
            });
        });
        for (go, r) in [(sfc, tools::run_sfc as fn() -> std::io::Result<()>), (dism, tools::run_dism)] {
            if go {
                if let Err(e) = r() {
                    self.status = Some((format!("Couldn't open the console: {e}"), false));
                }
            }
        }
    }

    fn stability_card(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        Self::heading(ui, "Stability log");
        let mut toggle = false;
        let mut all_changed = false;
        let mut undo: Option<ChangeEntry> = None;
        let applied = self.engine.with_store(|s| s.applied_ids());
        let known: Vec<String> = self.tweaks.iter().map(|t| t.id.clone()).collect();
        th::card(ui, (10, 16, 16, 16), |ui| {
            ui.horizontal(|ui| {
                let avail = ui.available_width();
                ui.vertical(|ui| {
                    ui.set_max_width((avail - 120.0).max(120.0));
                    caption(
                        ui,
                        &match &self.stab {
                            Some(Ok((e, _))) => stability::summary(e),
                            Some(Err(_)) => "Couldn't read the Windows event log.".into(),
                            None => "Crashes and reboots from the last 30 days that a MyTune change could cause, next to what MyTune changed.".into(),
                        },
                    );
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if th::button(ui, if self.stab_open { "Hide" } else { "Show" }, false, th::BTN).clicked() {
                        toggle = true;
                    }
                    if self.stab_busy {
                        ui.label(rt("Reading…", 12.0).color(text3()));
                    }
                });
            });
            if self.stab_open {
                ui.add_space(14.0);
                all_changed = w::checkbox(ui, Id::new("stab_all"), &mut self.stab_all, |ui| {
                    ui.label(rt("Show all app crashes, not only Windows components and Store apps", 14.0));
                });
                ui.add_space(12.0);
                if let Some(Ok((events, changes))) = &self.stab {
                    ui.label(rt("Crashes and reboots", 14.0).color(TEXT));
                    ui.add_space(6.0);
                    for (n, e) in events.iter().enumerate() {
                        let (title, detail) = stability::describe(e);
                        ui.label(rt(title, 14.0));
                        ui.add(egui::Label::new(rt(detail, 12.0).color(text3())).wrap());
                        let mut seen: Vec<(Option<String>, Option<String>)> = vec![];
                        ui.add_space(6.0);
                        ui.horizontal_wrapped(|ui| {
                            for c in &e.related {
                                let key = (c.kind.clone(), c.id.clone());
                                if seen.contains(&key) || !stability::can_undo(c, &applied, |id| known.iter().any(|k| k == id)) {
                                    continue;
                                }
                                seen.push(key);
                                let label = c.label.clone().unwrap_or_default();
                                let done = c.id.as_ref().and_then(|i| self.undone.get(i));
                                let text = match (c.kind.as_deref(), done) {
                                    (_, Some(d)) => d.clone(),
                                    (Some("app"), _) => format!("Open in Microsoft Store: {label}"),
                                    _ => format!("Switch off: {label}"),
                                };
                                ui.add_enabled_ui(done.is_none(), |ui| {
                                    if th::button(ui, &text, false, th::BTN).clicked() {
                                        undo = Some(c.clone());
                                    }
                                });
                            }
                        });
                        let _ = n;
                        ui.add_space(10.0);
                    }
                    ui.add_space(8.0);
                    ui.label(rt("What MyTune changed", 14.0));
                    ui.add_space(6.0);
                    for c in changes {
                        caption(ui, &format!("{}  {}", stability::short(c.time), c.text));
                        ui.add_space(4.0);
                    }
                }
            }
        });
        if toggle {
            self.stab_open = !self.stab_open;
            if self.stab_open && self.stab.is_none() {
                self.load_stability(ctx);
            }
        }
        if all_changed && self.stab_open {
            self.load_stability(ctx);
        }
        if let Some(c) = undo {
            self.undo(c);
        }
        let _ = caption3;
    }

    fn undo(&mut self, c: ChangeEntry) {
        let label = c.label.clone().unwrap_or_default();
        if c.kind.as_deref() == Some("app") {
            let q: String = label.bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
            let _ = std::process::Command::new("cmd.exe").args(["/c", "start", "", &format!("ms-windows-store://search/?query={q}")]).spawn();
            return;
        }
        let Some(id) = c.id else { return };
        let Some(t) = self.tweaks.iter().find(|t| t.id == id) else { return };
        match self.engine.revert(t) {
            Ok(_) => {
                self.undone.insert(id, format!("Switched off: {label}"));
                self.status = Some((format!("Switched off {label}."), true));
            }
            Err(e) => {
                mytune_core::log::error(&format!("stability undo {id}"), Some(&e));
                self.status = Some((format!("Couldn't switch off {label}. See the log."), false));
            }
        }
    }
}

fn shader_fmt(bytes: u64) -> String {
    if bytes >= 1 << 30 {
        format!("{:.1} GB", bytes as f64 / (1u64 << 30) as f64)
    } else {
        format!("{:.0} MB", (bytes as f64 / (1u64 << 20) as f64).round_ties_even())
    }
}
