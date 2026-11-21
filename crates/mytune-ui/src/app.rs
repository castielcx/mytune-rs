use crate::nav::{Item, Page, GROUPS, SETTINGS};
use crate::theme::{self as th, dsb, ico, p, rt, sb, text2, text3};
use eframe::egui::{self, pos2, vec2, Align, Color32, FontId, Id, Layout, Margin, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder};
use mytune_core::alltweaks::{self, TweakItem};
use mytune_core::catalog::TweakFlags;
use mytune_core::engine::{Engine, Tweak};
use mytune_core::pccheck::{self, Check, Level, ThisPc};
use mytune_core::store::Store;
use std::collections::HashMap;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

/// How long the pointer rests on the closed sidebar rail before it opens.
const HOVER_OPEN: Duration = Duration::from_secs(1);
/// Height and corner radius of a sidebar row (search, nav items).
const ROW: f32 = 32.0;
const ROW_R: f32 = 10.0;

/// Cubic ease-out over `dur` seconds, like the WPF `CubicEase EaseOut` (egui's own animate is linear).
struct Anim {
    cur: f32,
    from: f32,
    to: f32,
    /// Progress 0..1, advanced by frame time: a slow frame pauses the motion instead of skipping it.
    t: f32,
}

impl Anim {
    fn new(v: f32) -> Anim {
        Anim { cur: v, from: v, to: v, t: 1.0 }
    }

    fn update(&mut self, ctx: &egui::Context, target: f32, dur: f32) -> f32 {
        if (target - self.to).abs() > f32::EPSILON {
            self.from = self.cur;
            self.to = target;
            self.t = 0.0;
        } else if self.t < 1.0 {
            self.t = (self.t + ctx.input(|i| i.stable_dt).min(1.0 / 30.0) / dur).min(1.0);
        }
        let t = self.t;
        self.cur = self.from + (self.to - self.from) * (1.0 - (1.0 - t).powi(3));
        if t < 1.0 {
            ctx.request_repaint();
        }
        self.cur
    }
}

pub struct App {
    page: Page,
    engine: Arc<Engine>,
    internet: crate::internet::InternetPage,
    tools: crate::toolspage::ToolsPage,
    settings: crate::settingspage::SettingsPage,
    mouse: crate::mouse::MousePage,
    sound: crate::soundpage::SoundPage,
    flyout: crate::soundpage::Flyout,
    diagnostics: crate::diagnosticspage::DiagnosticsPage,
    display: crate::displaypage::DisplayPage,
    vibrance: crate::vibrancepage::VibrancePage,
    crosshair: crate::crosshairpage::CrosshairPage,
    games: crate::gamespage::GamesPage,
    bios: crate::biospage::BiosPage,
    #[cfg(feature = "taskbar-look")]
    taskbar: crate::taskbarcard::TaskbarCard,
    chat: crate::chatpage::ChatPage,
    /// Empty until the build thread is done (it runs PowerShell).
    tweaks: Arc<Vec<TweakItem>>,
    items_rx: Option<mpsc::Receiver<Vec<TweakItem>>>,
    applied: HashMap<String, bool>,
    /// False until the first states thread reports; Home must not claim "all set" before.
    states_loaded: bool,
    states_rx: Option<mpsc::Receiver<(HashMap<String, bool>, Vec<String>)>>,
    /// An apply/revert thread is running; clicks are ignored meanwhile.
    work_rx: Option<mpsc::Receiver<((String, bool), Vec<(String, bool)>)>>,
    search: String,
    status: Option<(String, bool)>,
    /// The page shown last frame: a status line goes when its page is left.
    shown_page: Page,
    /// The page `run_work` was started on; its result shows only there.
    work_page: Page,
    confirm_optimize: bool,
    confirm_revert_all: bool,
    /// Tweaks whose last revert failed only because an adapter is not connected: Restore offers Forget for these.
    adapter_gone: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    confirm_forget: Option<usize>,
    ids_in_store: Vec<String>,
    checks: Option<Vec<Check>>,
    check_rx: Option<mpsc::Receiver<Vec<Check>>>,
    this_pc: Option<ThisPc>,
    pc_rx: Option<mpsc::Receiver<ThisPc>>,
    ram_hist: Vec<f32>,
    cpu_hist: Vec<f32>,
    gpu_hist: Vec<f32>,
    /// Sensor thread (GPU via NVML or PDH); sampled every 5 s, every 1 s while Home is shown.
    sensors: Option<mytune_sensors::Sensors>,
    gpu: Option<mytune_sensors::Snapshot>,
    last_sample: Option<Instant>,
    side_w: f32,
    anim_pill: Anim,
    /// When the pointer came to rest on the closed sidebar rail (it opens after `HOVER_OPEN`).
    hover_since: Option<Instant>,
    /// After a click the rail stays collapsed until the pointer leaves it.
    suppress_hover: bool,
    /// y centre of the selected nav item drawn this frame (0 = none visible).
    pill_y: f32,
    /// Closing the window hides it instead of exiting (set by main when a tray icon exists).
    pub keep_in_tray: bool,
}

/// Which page a catalog id belongs to (the dump has no page info).
fn page_of(id: &str) -> Option<Page> {
    // C# `Layout.cs`. Network tweaks are on the Internet page here; retired ones are on no page.
    use mytune_core::layout::{placement, Section};
    Some(match placement(id)?.section {
        Section::General | Section::Advanced => Page::Optimize,
        Section::Debloat => Page::Debloat,
        Section::Customize => Page::Customize,
        Section::Network => return None,
    })
}

/// The one notice after `Engine::recover` rolled back interrupted changes; green only when all were put back.
fn recovered_text(left: &[mytune_core::engine::Recovered]) -> Option<(String, bool)> {
    use mytune_core::layout::{placement, Section};
    let bad: Vec<_> = left.iter().filter(|r| !r.put_back).collect();
    let what = match left {
        [] => return None,
        [one] => one.title.clone(),
        _ => format!("{} settings", left.len()),
    };
    let end = match (left.len(), bad.as_slice()) {
        (1, []) => "It was put back.".to_string(),
        (_, []) => "They were put back.".to_string(),
        (_, [one]) => {
            let page = match placement(&one.id).map(|p| p.section) {
                Some(Section::General | Section::Advanced) => ", check it on Optimize",
                Some(Section::Network) => ", check it on Internet",
                Some(Section::Debloat) => ", check it on Debloat",
                Some(Section::Customize) => ", check it on Customize",
                None => "",
            };
            let it = if left.len() == 1 { "It".to_string() } else { one.title.clone() };
            format!("{it} could not be put back{page}.")
        }
        (_, many) => format!("{} could not be put back, see the log.", many.len()),
    };
    Some((format!("MyTune closed while changing {what}. {end}"), bad.is_empty()))
}

/// `Settings.AccentColor` from state.json (the C# app's own setting).
fn saved_accent(store: &Store) -> String {
    store
        .top("Settings")
        .and_then(|s| s.get("AccentColor"))
        .and_then(|v| v.as_str())
        .unwrap_or("#FF5A0A")
        .to_string()
}

impl App {
    pub fn new() -> App {
        let dir = mytune_core::util::default_dir();
        mytune_core::store::harden_dir(&dir); // before the state it trusts is read
        Self::with_store(Store::load(&dir))
    }

    pub fn with_store(store: Store) -> App {
        th::set_accent_value(&saved_accent(&store));
        let (itx, items_rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = itx.send(alltweaks::build(mytune_core::tweaks::system_cmd()).unwrap_or_default());
        });
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(pccheck::this_pc());
        });
        mytune_core::display::apply_saved(&store); // saved display colours, kept on by the colour worker
        let engine = Arc::new(Engine::new(store));
        let mut app = App {
            page: Page::Chat,
            internet: crate::internet::InternetPage::new(engine.clone()),
            tools: crate::toolspage::ToolsPage::new(engine.clone()),
            settings: crate::settingspage::SettingsPage::new(engine.clone()),
            mouse: crate::mouse::MousePage::new(),
            sound: crate::soundpage::SoundPage::new(),
            flyout: crate::soundpage::Flyout::new(),
            diagnostics: crate::diagnosticspage::DiagnosticsPage::new(engine.clone()),
            display: crate::displaypage::DisplayPage::new(engine.clone()),
            vibrance: crate::vibrancepage::VibrancePage::new(engine.clone()),
            crosshair: crate::crosshairpage::CrosshairPage::new(engine.clone()),
            games: crate::gamespage::GamesPage::new(engine.clone()),
            bios: Default::default(),
            #[cfg(feature = "taskbar-look")]
            taskbar: crate::taskbarcard::TaskbarCard::new(engine.clone()),
            chat: Default::default(),
            engine,
            tweaks: Arc::new(Vec::new()),
            items_rx: Some(items_rx),
            applied: HashMap::new(),
            states_loaded: false,
            states_rx: None,
            work_rx: None,
            search: String::new(),
            status: None,
            shown_page: Page::Chat,
            work_page: Page::Chat,
            confirm_optimize: false,
            confirm_revert_all: false,
            adapter_gone: Default::default(),
            confirm_forget: None,
            ids_in_store: vec![],
            checks: None,
            check_rx: None,
            this_pc: None,
            pc_rx: Some(rx),
            ram_hist: vec![],
            cpu_hist: vec![],
            gpu_hist: vec![],
            sensors: Some(mytune_sensors::Sensors::start(mytune_sensors::HwSource::new(true), mytune_sensors::Cadence::default())),
            gpu: None,
            last_sample: None,
            side_w: 248.0,
            anim_pill: Anim::new(0.0),
            hover_since: None,
            suppress_hover: false,
            pill_y: 0.0,
            keep_in_tray: false,
        };
        app.refresh();
        app
    }

    /// Starts the crosshair overlay thread (restores it when it was on); the hotkey saves and repaints.
    pub fn start_crosshair(&self, ctx: &eframe::egui::Context) {
        let initial = self.engine.with_store(|s| mytune_core::crosshair::Crosshair::load(s));
        let (engine, ctx) = (self.engine.clone(), ctx.clone());
        mytune_core::crosshair::start(initial, move |c| {
            let _ = engine.with_store(|s| c.save(s));
            ctx.request_repaint();
        });
    }

    pub fn goto(&mut self, page: Page) {
        self.page = page;
    }

    /// Shot tests: the Internet page in the middle of a speed test.
    #[doc(hidden)]
    pub fn fake_speed(&mut self, phase: mytune_core::speedtest::Phase, down: &[f64], up: &[f64]) {
        self.internet.fake_speed(phase, down, up);
    }

    /// Shot tests: the Internet page right after a finished speed test.
    #[doc(hidden)]
    pub fn fake_speed_done(&mut self, last: mytune_core::speedtest::LastSpeed) {
        self.internet.fake_speed_done(last);
    }

    /// Shot tests: the Connection health card with finished rows.
    #[doc(hidden)]
    pub fn fake_health(&mut self, rows: Vec<mytune_core::connhealth::Row>) {
        self.internet.fake_health(rows);
    }

    fn start_check(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(pccheck::run_all());
            ctx.request_repaint();
        });
        self.check_rx = Some(rx);
    }

    /// Reads every tweak's state on a thread (many shell out); `poll` picks the result up.
    fn refresh(&mut self) {
        let (tx, rx) = mpsc::channel();
        let (tweaks, engine) = (self.tweaks.clone(), self.engine.clone());
        std::thread::spawn(move || {
            let applied = tweaks.iter().map(|t| (t.id.clone(), t.is_applied().unwrap_or(false))).collect();
            let _ = tx.send((applied, engine.with_store(|s| s.applied_ids())));
        });
        self.states_rx = Some(rx);
    }

    /// Collects finished background work; keeps the UI polling while any is pending.
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(items) = self.items_rx.as_ref().and_then(|r| r.try_recv().ok()) {
            self.tweaks = Arc::new(items);
            self.items_rx = None;
            // First what a MyTune that died mid-change left half done is put back, then the states are read.
            let (tx, rx) = mpsc::channel();
            let (tweaks, engine, ctx) = (self.tweaks.clone(), self.engine.clone(), ctx.clone());
            std::thread::spawn(move || {
                // game tweaks and the PC check fix are not in the tweak list: their revert needs the backup alone
                let others = engine.with_store(|s| mytune_core::pergame::pending_reverts(s));
                let left = engine.recover(|id| tweaks.iter().find(|t| t.id == id).map(|t| t as &dyn Tweak).or_else(|| others.iter().find(|o| o.id() == id).map(|o| &**o)));
                let applied = tweaks.iter().map(|t| (t.id.clone(), t.is_applied().unwrap_or(false))).collect();
                let _ = tx.send((applied, engine.with_store(|s| s.applied_ids())));
                if let Some((text, ok)) = recovered_text(&left) {
                    // the bar counts its seconds from now: started in the tray, nobody would see it
                    while !window_shown() {
                        std::thread::sleep(Duration::from_secs(1));
                    }
                    crate::notice::post(&ctx, text, ok);
                    ctx.request_repaint();
                }
            });
            self.states_rx = Some(rx);
        }
        if let Some((applied, ids)) = self.states_rx.as_ref().and_then(|r| r.try_recv().ok()) {
            self.applied = applied;
            self.ids_in_store = ids;
            self.states_loaded = true;
            self.states_rx = None;
        }
        if let Some((st, touched)) = self.work_rx.as_ref().and_then(|r| r.try_recv().ok()) {
            self.status = (self.page == self.work_page).then_some(st);
            // the changed tweaks' real state now; the full re-read below takes seconds (many shell out)
            self.applied.extend(touched);
            self.work_rx = None;
            self.refresh();
            if self.page == Page::Diagnostics && self.checks.is_some() {
                self.start_check(ctx); // the PC check shows the new state
            }
        }
        if self.games.poll(ctx) {
            self.ids_in_store = self.engine.with_store(|s| s.applied_ids()); // a game profile came or went
        }
        if self.items_rx.is_some() || self.states_rx.is_some() || self.work_rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }

    /// Applies (`on`) or reverts each listed tweak on a thread; one status line at the end.
    fn run_work(&mut self, idxs: Vec<usize>, on: bool) {
        if self.work_rx.is_some() || idxs.is_empty() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let (tweaks, engine, gone) = (self.tweaks.clone(), self.engine.clone(), self.adapter_gone.clone());
        // a re-read started before this change would bring the old state back
        self.states_rx = None;
        let first = on && !mytune_core::sysreport::restore_point_done();
        self.status = Some((if first { "Making a restore point first, this can take a minute\u{2026}" } else { "Working\u{2026}" }.into(), true));
        self.work_page = self.page;
        // the switch moves at once; the thread's reading corrects it if the change failed
        for &i in &idxs {
            self.applied.insert(self.tweaks[i].id.clone(), on);
        }
        std::thread::spawn(move || {
            if on {
                mytune_core::sysreport::restore_point_once(&*mytune_core::tweaks::system_cmd());
            }
            let (mut ok, mut bad, mut last) = (0, 0, String::new());
            for &i in &idxs {
                let t = &tweaks[i];
                gone.lock().unwrap_or_else(|p| p.into_inner()).remove(&t.id); // Forget follows the latest result only
                // G-SYNC's revert puts back the state from before MyTune, usually "on": still on is then correct
                let gsync = (!on && t.id == "general.gsync").then(|| engine.with_store(|s| s.get_backup(&t.id).is_some_and(mytune_core::nvidia::has_gsync_state)));
                let r = if on {
                    engine.apply(t).map_err(|e| e.to_string())
                } else {
                    match engine.revert(t) {
                        Ok(mytune_core::engine::RevertOutcome::Reverted) => Ok(()),
                        Ok(mytune_core::engine::RevertOutcome::NoBackup) => Err("it was set outside MyTune, so there is nothing to undo".to_string()),
                        Err(e) => {
                            if mytune_core::special::only_missing_adapters(&e) {
                                gone.lock().unwrap_or_else(|p| p.into_inner()).insert(t.id.clone());
                            }
                            Err(e.to_string())
                        }
                    }
                };
                // "Ok" only means no error was raised: read the state back, and when it is not what was asked
                // for, say so with a reason and (turning on) undo the change so the switch stays off.
                let r = r.and_then(|()| match t.is_applied() {
                    Ok(now) if now == on || gsync == Some(true) => Ok(()),
                    checked => {
                        let why = match checked {
                            Ok(_) if gsync == Some(false) => "no earlier G-SYNC state was saved, so it was left as it is".to_string(),
                            Ok(_) if on => "Windows still reports it off, something reset it".to_string(),
                            Ok(_) => "Windows still reports it on".to_string(),
                            Err(e) => format!("couldn't check the result: {e}"),
                        };
                        mytune_core::log::error(&format!("{} not verified: {why}", t.id), None);
                        if on {
                            if let Err(e) = engine.rollback(t) {
                                mytune_core::log::error(&format!("{} undo after failed check", t.id), Some(&e));
                            }
                        }
                        Err(why)
                    }
                });
                match r {
                    Ok(()) => ok += 1,
                    Err(e) => {
                        bad += 1;
                        last = e;
                    }
                }
            }
            let msg = if idxs.len() == 1 {
                crate::notice::toggled(&tweaks[idxs[0]].title, on, (bad > 0).then_some(last.as_str()))
            } else if bad == 0 {
                crate::notice::toggled(&format!("{ok} tweaks"), on, None)
            } else {
                crate::notice::toggled(&format!("{bad} of {} tweaks", idxs.len()), on, Some("see the log"))
            };
            let touched = idxs.iter().map(|&i| (tweaks[i].id.clone(), tweaks[i].is_applied().unwrap_or(false))).collect();
            let _ = tx.send((msg, touched));
        });
        self.work_rx = Some(rx);
    }

    fn set(&mut self, idx: usize, on: bool) {
        if self.states_loaded {
            self.run_work(vec![idx], on);
        }
    }

    /// (size of the recommended set, indices not yet applied). Before the states are in, nothing is "todo".
    fn recommended_todo(&self) -> (usize, Vec<usize>) {
        let set: Vec<usize> = (0..self.tweaks.len())
            .filter(|&i| {
                let t = &self.tweaks[i];
                !t.removed && t.flags.recommended() && page_of(&t.id) == Some(Page::Optimize)
            })
            .collect();
        let todo = set.iter().copied().filter(|&i| !self.applied.get(&self.tweaks[i].id).copied().unwrap_or(false)).collect();
        (set.len(), todo)
    }

    /// Once a second while Home is shown.
    fn sample(&mut self) {
        if self.last_sample.is_some_and(|t| t.elapsed() < Duration::from_millis(950)) {
            return;
        }
        self.last_sample = Some(Instant::now());
        if let Some(s) = &self.sensors {
            [self.cpu_hist, self.gpu_hist, self.ram_hist] = s.history();
            if let Some(snap) = s.latest() {
                self.gpu = Some(snap);
            }
        }
    }

    pub fn ui(&mut self, root: &mut Ui) {
        let ctx = root.ctx().clone();
        let ctx = &ctx;
        frame_cap();
        perf_log(ctx);
        self.poll(ctx);
        self.flyout.show(ctx); // the tray speaker's volume window, also while this window is hidden
        self.chat.popouts(ctx); // the song and stream windows, also while this window is hidden
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            ctx.memory_mut(|m| m.request_focus(Id::new("search")));
        }
        // closing always hides to the tray; without a tray icon it exits
        if self.keep_in_tray && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_secs(2));
                mytune_core::process::trim_working_set();
            });
        }
        // public setup: no sidebar and no pages until a licence key is active on this PC
        #[cfg(feature = "license")]
        if mytune_core::license::status() != mytune_core::license::Status::Unlocked {
            th::backdrop(ctx, &ctx.layer_painter(egui::LayerId::background()), ctx.content_rect());
            self.title_bar(root, 248.0);
            crate::licensegate::ui(root);
            crate::fade::end_frame(ctx);
            return;
        }
        if let Some(rx) = &self.check_rx {
            if let Ok(v) = rx.try_recv() {
                self.checks = Some(v);
                self.check_rx = None;
            }
        }
        if let Some(rx) = &self.pc_rx {
            if let Ok(v) = rx.try_recv() {
                self.this_pc = Some(v);
                self.pc_rx = None;
            }
        }
        // Home's live tiles only while the window can be seen: in the tray this kept 1 s sensor reads and a frame a second
        let home = self.page == Page::Dashboard && window_shown();
        if let Some(s) = &self.sensors {
            s.set_watched(home); // 1 s samples only while Home is on screen
        }
        if home {
            self.sample();
            ctx.request_repaint_after(Duration::from_secs(1));
        }

        if self.page != Page::Mouse {
            self.mouse.cancel(); // stop listening when the page is left
        }
        if self.page != Page::Sound {
            self.sound.stop(); // no audio polling while the page is hidden
        }
        if self.page != Page::Diagnostics {
            self.diagnostics.stop(); // no sampling while the page is hidden
        }
        self.chat.call_flash(ctx);
        self.chat.notify(ctx);
        if self.page != Page::Chat {
            self.chat.hidden(); // messages arriving now stay unread
            self.chat.call_notice(ctx);
        }
        // Restore lives in Settings (its Restore tab), not the sidebar: Settings and Restore are the two tabs
        // the sidebar runs first, so a click on it shows the new page in this same frame (44 = title bar)
        self.sidebar(ctx, ctx.content_rect().top() + 44.0);
        let home = self.page == Page::Dashboard;
        th::backdrop(ctx, &ctx.layer_painter(egui::LayerId::background()), ctx.content_rect());
        let base_w = th::spring(ctx, Id::new("base-w"), if home { 248.0 } else { 64.0 }, th::TRAVEL, 1.0).round();
        self.title_bar(root, base_w);
        // Leaving Home, the page takes its final width at once and the folding sidebar uncovers it:
        // laying the page out again for every width in between cost frames.
        let gap = if home { base_w } else { 64.0 };
        egui::Panel::left("gap").exact_size(gap).resizable(false).frame(egui::Frame::new()).show_separator_line(false).show(root, |_| {});
        if self.page != self.shown_page {
            self.shown_page = self.page;
            self.status = None;
            self.internet.status = None;
            self.tools.status = None;
            self.settings.status = None;
            self.mouse.status = None;
            self.display.status = None;
            if self.page == Page::Tools {
                self.tools.reopened(ctx);
            }
        }
        // the chat draws its own opaque layers, so it keeps a solid base; every other page shows the backdrop
                egui::CentralPanel::default().frame(egui::Frame::new()).show(root, |ui| {
            // nothing shows through the glass sidebar while it still covers the page
            let mut clip = ui.clip_rect();
            clip.min.x = clip.min.x.max(self.side_w);
            ui.set_clip_rect(clip);
            if self.page == Page::Chat {
                // the chat sits in a rounded card inside the window: its square corners are covered with the base colour
                let mut card = ui.max_rect().shrink2(vec2(10.0, 8.0));
                card.max.y -= 8.0; // extra air under the card
                ui.painter().add(th::glass(card, th::R_PANEL, Color32::TRANSPARENT));
                ui.scope_builder(UiBuilder::new().max_rect(card), |ui| self.chat.ui(ui));
                let full = ctx.content_rect();
                th::round_corners(ui.painter(), card, th::R_PANEL, &|q| th::backdrop_at(full, q));
                th::sheen(ui.painter(), card, th::R_PANEL, th::Glass::Base);
                return;
            }
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                egui::Frame::new().inner_margin(Margin { left: 36, right: 36, top: 28, bottom: 36 }).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    {
                        let settings = matches!(self.page, Page::Settings | Page::Restore);
                        // the page's name, lined up with the text inside cards (card pad 3 + 10), user's choice
                        egui::Frame::new().outer_margin(Margin { left: 13, ..Default::default() }).show(ui, |ui| {
                            let page = if settings { Page::Settings } else { self.page };
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 10.0;
                                ui.label(ico(crate::nav::glyph_of(page), 22.0));
                                let (dot, _) = ui.allocate_exact_size(vec2(4.0, 4.0), Sense::hover());
                                ui.painter().circle_filled(dot.center() + vec2(0.0, 2.0), 2.0, th::text3()); // 2 px: optical middle of the title
                                ui.label(dsb(page.label(), 27.0));
                            });
                        });
                        ui.add_space(12.0);
                        if settings {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                for (page, name) in [(Page::Settings, "General"), (Page::Restore, "Restore")] {
                                    if th::tab(ui, name, self.page == page).clicked() {
                                        if self.page != page {
                                            let now = ui.input(|i| i.time);
                                            ui.data_mut(|d| d.insert_temp(egui::Id::new("settings-tab-at"), now));
                                        }
                                        self.page = page;
                                    }
                                }
                            });
                            ui.add_space(14.0);
                            // tab switch: the new tab's content fades in while rising 10 px
                            let at = ui.data(|d| d.get_temp::<f64>(egui::Id::new("settings-tab-at")));
                            let t = at.map_or(1.0, |at| ((ui.input(|i| i.time) - at) / 0.2).clamp(0.0, 1.0) as f32);
                            if t < 1.0 {
                                let k = egui::emath::easing::cubic_out(t);
                                ui.set_opacity(k);
                                ui.add_space((1.0 - k) * 10.0);
                                ui.ctx().request_repaint();
                            }
                        }
                        if let Some((msg, ok)) = self.status.take() {
                            crate::notice::post(ui.ctx(), msg, ok);
                        }
                        th::page_gap(ui);
                    }
                    match self.page {
                        Page::Dashboard => self.home(ui),
                        Page::Optimize | Page::Debloat | Page::Customize => self.tweak_list(ui),
                        Page::Restore => self.restore(ui), // reached only through Settings > Restore now
                        Page::Vpn => self.internet.ui(ui, &self.search),
                        Page::Tools => self.tools.ui(ui),
                        Page::Settings => self.settings.ui(ui),
                        Page::Mouse => self.mouse.ui(ui),
                        Page::Sound => {
                            // The switch hides Windows' volume icon through the taskbar DLL, so it exists only with it.
                            #[cfg(feature = "taskbar-look")]
                            {
                                self.sound.hide_win = Some(self.taskbar.hide_volume());
                            }
                            let _clicked = self.sound.ui(ui);
                            #[cfg(feature = "taskbar-look")]
                            if let Some(on) = _clicked {
                                self.taskbar.set_hide_volume(on);
                            }
                        }
                        Page::Music => crate::nowplaying::page(ui),
                        Page::Diagnostics => {
                            th::section(ui, "PC check");
                            self.pc_check(ui);
                            self.diagnostics.ui(ui)
                        }
                        Page::Display => self.display.ui(ui),
                        Page::Vibrance => self.vibrance.ui(ui),
                        Page::Crosshair => self.crosshair.ui(ui),
                        Page::Games => self.games.ui(ui),
                        Page::Bios => self.bios.ui(ui),
                        _ => th::card(ui, (10, 16, 16, 16), |ui| {
                            ui.label(rt("Not ported yet.", 14.0).color(text2()));
                        }),
                    }
                });
            });
        });
        if self.confirm_optimize {
            self.optimize_modal(ctx);
        }
        if self.confirm_revert_all {
            self.revert_all_modal(ctx);
        }
        if let Some(i) = self.confirm_forget {
            self.forget_adapter_modal(ctx, i);
        }
        self.games.modals(ctx);
        crate::notice::show(ctx, 44.0); // under the title bar
        crate::fade::end_frame(ctx);
        crate::sharp::scan(ctx);
    }

    // ------------------------------------------------------------ chrome

    fn title_bar(&mut self, root: &mut Ui, base_w: f32) {
        let ctx = root.ctx().clone();
        egui::Panel::top("title").exact_size(44.0).resizable(false).frame(egui::Frame::new()).show_separator_line(false).show(root, |ui| {
            let bar = ui.max_rect();
            let pal = p();
            let drag = ui.interact(bar, Id::new("drag"), Sense::click_and_drag());
            let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            if drag.drag_started() {
                ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            if drag.double_clicked() {
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
            // logo 18x18 at left 16, centred: accent to darker gradient, white M
            // centred over the nav icons
            let cx = 32.0; // same column as the nav icons, open or folded
            let logo = Rect::from_center_size(pos2(bar.left() + cx, bar.center().y), vec2(18.0, 18.0));
            if !pal.black {
                ui.painter().rect_filled(logo, 5.0, pal.accent);
            }
            let m = |x: f32, y: f32| logo.min + vec2(x, y) * (18.0 / 256.0);
            let pts: Vec<_> = [(65., 187.), (65., 69.), (90., 69.), (128., 123.), (166., 69.), (191., 69.), (191., 187.), (167., 187.), (167., 119.), (133., 157.), (123., 157.), (89., 119.), (89., 187.)]
                .iter()
                .map(|&(x, y)| m(x, y))
                .collect();
            // the M outline is concave: draw it as two stems and a V
            let _ = pts;
            let w = Stroke::new(2.0, pal.on_accent);
            ui.painter().line_segment([m(77., 187.), m(77., 69.)], w);
            ui.painter().line_segment([m(179., 187.), m(179., 69.)], w);
            ui.painter().line_segment([m(77., 75.), m(128., 140.)], w);
            ui.painter().line_segment([m(179., 75.), m(128., 140.)], w);
            if base_w > 120.0 {
                ui.painter().text(pos2(logo.right() + 10.0, bar.center().y), egui::Align2::LEFT_CENTER, "MyTune", FontId::new(13.0, th::semibold()), pal.side_text.gamma_multiply(((base_w - 120.0) / 128.0).clamp(0.0, 1.0)));
            }
            // caption buttons 46x32, glyphs 10 px
            let right = bar.right();
            let buttons = [
                ("\u{E8BB}", egui::ViewportCommand::Close, true),
                (if maximized { "\u{E923}" } else { "\u{E922}" }, egui::ViewportCommand::Maximized(!maximized), false),
                ("\u{E921}", egui::ViewportCommand::Minimized(true), false),
            ];
            for (i, (glyph, cmd, close)) in buttons.into_iter().enumerate() {
                let r = Rect::from_min_size(pos2(right - 46.0 * (i as f32 + 1.0), bar.top()), vec2(46.0, 32.0));
                let resp = ui.interact(r, Id::new(("cap", i)), Sense::click());
                let hov = th::anim(&ctx, resp.id.with("hover"), resp.hovered(), th::HOVER);
                if hov > 0.0 && !pal.black {
                    let tint = if close { pal.block[1] } else { Color32::from_white_alpha(0x24) };
                    ui.painter().rect_filled(r.shrink2(vec2(5.0, 3.0)), 8.0, tint.gamma_multiply(hov));
                }
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, glyph, FontId::new(10.0, th::icons()), if pal.black { th::text2().lerp_to_gamma(Color32::WHITE, hov) } else if close { pal.side_text.lerp_to_gamma(Color32::WHITE, hov) } else { pal.side_text });
                if resp.clicked() {
                    ctx.send_viewport_cmd(cmd);
                }
            }
        });
    }

    fn sidebar(&mut self, ctx: &egui::Context, top: f32) {
        let pal = p();
        let screen = ctx.content_rect();
        let home = self.page == Page::Dashboard;
        // On the chat page the open sidebar would cover the server icons right of it, so it stays open only
        // while the pointer is on the 64 px rail itself.
        let reach = if self.page == Page::Chat { 64.0 } else { self.side_w };
        let in_rail = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|q| q.y > top && q.x < reach);
        if !in_rail {
            self.suppress_hover = false;
        }
        // the rail opens after the pointer rests on it for 1 s (passing over it to reach something does nothing);
        // once open it stays open while the pointer is on it, and closes at once when it leaves
        let resting = home && in_rail && !self.suppress_hover; // the wide sidebar belongs to Home only
        if !resting {
            self.hover_since = None;
        } else if self.hover_since.is_none() {
            self.hover_since = Some(Instant::now());
        }
        let open = self.side_w > 64.5;
        let waited = self.hover_since.map_or(Duration::ZERO, |t| t.elapsed());
        let hovered = resting && (open || waited >= HOVER_OPEN);
        if resting && !hovered {
            ctx.request_repaint_after(HOVER_OPEN - waited);
        }
        // a spring: it can be turned round half way (pointer leaves while it opens) without a jump. Not rounded:
        // 1 px steps stutter.
        let w = th::spring(ctx, Id::new("side-w"), if home || hovered { 248.0 } else { 64.0 }, th::TRAVEL, 1.0);
        self.side_w = w;
        let rect = Rect::from_min_size(pos2(0.0, top), vec2(w, screen.height() - top));
        let fade = ((w - 64.0) / 184.0).clamp(0.0, 1.0);
        let prev_pill = self.pill_y;
        self.pill_y = 0.0;
        let pill = self.anim_pill.update(ctx, prev_pill, 0.22).round();
        egui::Area::new(Id::new("sidebar")).fixed_pos(rect.min).order(egui::Order::Middle).show(ctx, |ui| {
            ui.allocate_rect(rect, Sense::hover());
            let paint = ui.painter().clone();
            paint.add(th::glass(Rect::from_min_max(rect.min + vec2(6.0, 4.0), rect.max - vec2(6.0, 12.0)), th::R_PANEL, pal.side));
            let inner = Rect::from_min_max(rect.min + vec2(8.0, 4.0), rect.max - vec2(8.0, 20.0));
            ui.scope_builder(UiBuilder::new().max_rect(inner), |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                let scroll_h = (inner.height() - 40.0).max(0.0);
                egui::ScrollArea::vertical().max_height(scroll_h).auto_shrink([false, true]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
                    ui.set_width(inner.width());
                    // Same row in the rail and the full sidebar, so the icons below never move.
                    ui.add_space(2.0);
                    let r = Rect::from_min_size(ui.cursor().min, vec2(inner.width(), ROW));
                    ui.allocate_space(vec2(inner.width(), ROW));
                    let focused = ui.memory(|m| m.has_focus(Id::new("search")));
                    // same row as every nav item: hover tint, icon in the same column; an accent edge while typing
                    if focused {
                        ui.painter().rect_stroke(r, ROW_R, Stroke::new(1.0, pal.accent), StrokeKind::Inside);
                    }
                    if ui.rect_contains_pointer(r) {
                        ui.painter().rect_filled(r, ROW_R, Color32::from_white_alpha(22));
                    }
                    let gx = r.left() + 24.0; // fixed column: the icon never moves while the sidebar opens or folds
                    ui.painter().text(pos2(gx, r.center().y.round()), egui::Align2::CENTER_CENTER, "\u{E721}", FontId::new(16.0, th::icons()), pal.side_muted);
                    if w <= 150.0 {
                        let resp = ui.interact(r, Id::new("search-icon"), Sense::click());
                        if resp.on_hover_text("Search every setting (Ctrl+F)").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            self.suppress_hover = false;
                            ui.memory_mut(|m| m.request_focus(Id::new("search")));
                        }
                    } else {
                        // a child ui, not `ui.put`: put moved the cursor up to the text box's bottom, so every
                        // row below sat a few px higher while the sidebar was open and dropped when it folded
                        let field = Rect::from_min_max(r.min + vec2(46.0, 1.0), r.max - vec2(8.0, 1.0)); // text column of the nav labels
                        ui.new_child(UiBuilder::new().max_rect(field).layout(egui::Layout::centered_and_justified(egui::Direction::LeftToRight))).add(
                            egui::TextEdit::singleline(&mut self.search)
                                .id(Id::new("search"))
                                .frame(egui::Frame::NONE)
                                .font(FontId::proportional(14.0))
                                .text_color(pal.side_text)
                                .hint_text(rt("Search", 14.0).color(pal.side_text))
                                .vertical_align(Align::Center)
                                .desired_width(f32::INFINITY),
                        );
                    }
                    ui.add_space(2.0);
                    let q = self.search.to_lowercase();
                    for g in GROUPS {
                        let items: Vec<&Item> = g.items.iter().filter(|i| q.is_empty() || i.label.to_lowercase().contains(&q) || i.tip.to_lowercase().contains(&q)).collect();
                        if items.is_empty() {
                            continue;
                        }
                        if let Some(t) = g.title {
                            ui.add_space(12.0);
                            // painted, not a label: a label kept the sidebar layer ~117 px wide while folded,
                            // so its invisible part ate clicks meant for the chat page's server icons
                            let (tr, _) = ui.allocate_exact_size(vec2(inner.width(), 16.0), Sense::hover());
                            ui.painter().with_clip_rect(tr).text(pos2(tr.left() + 14.0, tr.center().y), egui::Align2::LEFT_CENTER, t, FontId::new(12.0, th::medium()), th::text3().gamma_multiply(fade));
                            // folded, the names are gone: a short line keeps the groups apart
                            if fade < 1.0 {
                                let y = tr.center().y.round();
                                ui.painter().line_segment([pos2(tr.left() + 16.0, y), pos2(tr.left() + 32.0, y)], Stroke::new(th::px(), Color32::from_white_alpha((36.0 * (1.0 - fade)) as u8)));
                            }
                            ui.add_space(2.0);
                        }
                        for i in items {
                            self.nav_item(ui, i, inner.width(), w);
                        }
                    }
                });
                let y = inner.bottom() - 40.0;
                ui.scope_builder(UiBuilder::new().max_rect(Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), 40.0))), |ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                    self.nav_item(ui, &SETTINGS, inner.width(), w);
                });
            });
            let _ = pill; // the selected item shows as a glass plate; no marker bar
        });
    }

    fn nav_item(&mut self, ui: &mut Ui, item: &Item, width: f32, w: f32) {
        let pal = p();
        ui.add_space(1.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(width, ROW), Sense::click());
        ui.add_space(1.0);
        let sel = self.page == item.page || (item.page == Page::Settings && self.page == Page::Restore); // Restore is a Settings tab
        let paint = ui.painter();
        // Same springs as `th::button`: the plate under the pointer fades in, gives way 3% while pressed, and
        // the selected plate grows in from 94%. Only the plate moves, icon and name stay put.
        let ctx = ui.ctx();
        let hov = th::anim(ctx, resp.id.with("hover"), resp.hovered(), th::HOVER);
        let down = th::anim(ctx, resp.id.with("down"), resp.is_pointer_button_down_on(), th::PRESS);
        let on = th::anim(ctx, resp.id.with("sel"), sel, 0.24);
        let plate = |k: f32| Rect::from_center_size(rect.center(), rect.size() * k);
        if on > 0.0 {
            // raised glass on the sidebar's base glass
            th::plate(paint, plate(0.94 + 0.06 * on - 0.03 * down), ROW_R, pal.side_selected.gamma_multiply(on), (0x44 as f32 * on) as u8);
        }
        if sel {
            self.pill_y = rect.center().y;
        }
        if hov > 0.0 || down > 0.0 {
            paint.rect_filled(plate(1.0 - 0.03 * down), ROW_R, Color32::from_white_alpha((20.0 * hov * (1.0 - on) + 14.0 * down) as u8));
        }
        let glyph_col = if sel || (pal.black && resp.hovered()) { Color32::WHITE } else { pal.side_muted };
        let text_col = if sel { Color32::WHITE } else { th::hover_text(pal.side_text, resp.hovered()) };
        let gx = rect.left() + 24.0; // fixed column: icon and name stay put, the sidebar edge uncovers the name
        paint.text(pos2(gx, rect.center().y.round()), egui::Align2::CENTER_CENTER, item.glyph, FontId::new(16.0, th::icons()), glyph_col);
        if w > 120.0 {
            let font = FontId::new(14.0, if sel { th::semibold() } else { th::medium() });
            paint.with_clip_rect(rect).text(pos2(rect.left() + 46.0, rect.center().y.round()), egui::Align2::LEFT_CENTER, item.label, font, text_col.gamma_multiply(((w - 120.0) / 128.0).clamp(0.0, 1.0)));
        }
        if resp.on_hover_text(item.tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.page = item.page;
            self.suppress_hover = true;
        }
    }

    // ------------------------------------------------------------ Home

    /// Two columns (one under the other in a narrow window): left what to do, right what the PC is doing now,
    /// ending at the same height; this PC's parts across the full width below.
    fn home(&mut self, ui: &mut Ui) {
        let (gap, full) = (th::GAP, ui.available_width());
        if full < 720.0 {
            self.home_main(ui);
            self.home_side(ui, GRAPH_MIN, false);
            self.home_pc(ui);
            return;
        }
        // the graphs take the left column's spare height, measured on the previous frame
        let key = Id::new("home-cols");
        let (left_h, right_h, graph_was) = ui.ctx().data(|d| d.get_temp::<(f32, f32, f32)>(key)).unwrap_or((0.0, 0.0, GRAPH_MIN));
        let natural = right_h - 3.0 * (graph_was - GRAPH_MIN);
        let graph = (GRAPH_MIN + (left_h - natural) / 3.0).max(GRAPH_MIN).round();
        let side = ((full - gap) * 0.42).floor();
        let mut heights = [0.0; 2];
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for (i, width) in [full - gap - side, side].into_iter().enumerate() {
                heights[i] = ui
                    .allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
                        ui.set_width(width);
                        if i == 0 { self.home_main(ui) } else { self.home_side(ui, graph, true) }
                    })
                    .response
                    .rect
                    .height();
            }
        });
        ui.ctx().data_mut(|d| d.insert_temp(key, (heights[0], heights[1], graph)));
        if (graph - graph_was).abs() > 0.5 {
            ui.ctx().request_repaint();
        }
        self.home_pc(ui);
    }

    /// Left column of Home: the recommended settings, what fills the system drive, where to go next.
    fn home_main(&mut self, ui: &mut Ui) {
        let pal = p();
        let (total, todo) = self.recommended_todo();
        let ready = self.states_loaded;
        let all_set = ready && todo.is_empty();
        let (mut optimize, mut see_all) = (false, false);
        home_heading(ui, "Gaming setup", true);
        th::card(ui, (13, 16, 16, 16), |ui| {
            let title = if !ready { "Checking your settings\u{2026}".to_string() } else if all_set { "Your PC is set up for gaming".to_string() } else { format!("{} of {total} recommended settings are still off", todo.len()) };
            ui.label(dsb(title, 19.0));
            ui.add_space(3.0);
            ui.label(rt(if !ready { "This takes a moment." } else if all_set { "Every recommended setting is on." } else { "Optimize now switches them on. You see the list first." }, 13.0).color(home_sub()));
            // one tick per recommended setting, filled for each one that is on, outlined for each one still off
            if total > 0 {
                ui.add_space(11.0);
                let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 15.0), Sense::hover());
                let lit = if ready { total - todo.len() } else { 0 };
                let w = (bar.width() - 3.0 * (total - 1) as f32) / total as f32;
                // a light runs along the ticks at constant speed and leaves after the last one that is on (like a
                // tail light); it only reaches further once another recommended setting is applied
                const SPEED: f64 = 9.0; // ticks a second, constant: no slowing at the end
                const DELAY: f64 = 0.4; // short rest before it starts again
                let end = lit.saturating_sub(1) as f32;
                const REACH: f32 = 2.5; // the light spans this many ticks each side, then is exactly zero (no endless tail)
                let run = (end as f64 + 2.0 * REACH as f64) / SPEED; // from fully left of the first to fully right of the last
                // time the frame will be on screen, not when it started: even steps at any frame rate
                let t = if lit > 0 { ui.input(|i| i.time + i.predicted_dt as f64) % (run + DELAY) } else { 0.0 };
                let pos = -REACH + (t.min(run) * SPEED) as f32;
                // every edge on a whole device pixel: sizes that land between pixels shimmer as the light moves
                let ppp = ui.ctx().pixels_per_point();
                let snap = |v: f32| (v * ppp).round() / ppp;
                let cy = snap(bar.center().y);
                for i in 0..total {
                    let d = ((i as f32 - pos).abs() / REACH).min(1.0);
                    let k = if i >= lit { 0.0 } else { 0.5 * (1.0 + (std::f32::consts::PI * d).cos()) };
                    let h = ((8.0 * (1.0 + 0.6 * k) * ppp) / 2.0).round() * 2.0 / ppp; // even pixel count keeps it centred
                    let (x0, x1) = (snap(bar.left() + i as f32 * (w + 3.0)), snap(bar.left() + i as f32 * (w + 3.0) + w));
                    let tick = Rect::from_min_max(pos2(x0, cy - h / 2.0), pos2(x1, cy + h / 2.0));
                    if i < lit {
                        ui.painter().rect_filled(tick, 2.0, pal.accent.gamma_multiply(0.6 + 0.4 * k));
                    } else {
                        ui.painter().rect_stroke(tick, 2.0, Stroke::new(1.0, Color32::from_white_alpha(0x40)), StrokeKind::Inside);
                    }
                }
                // a window behind a fullscreen game still counts as shown, so focus decides too
                if lit > 0 && window_shown() && ui.input(|i| i.viewport().focused == Some(true)) {
                    if t < run {
                        ui.ctx().request_repaint();
                    } else {
                        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(run + DELAY - t));
                    }
                }
            }
            ui.add_space(11.0);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if ready && !all_set && th::button(ui, "Optimize now", true, th::BTN).clicked() {
                    optimize = true;
                }
                if th::button(ui, "See all settings", false, th::BTN).clicked() {
                    see_all = true;
                }
            });
        });
        if optimize {
            self.confirm_optimize = true;
        }
        if see_all {
            self.page = Page::Optimize;
        }

        home_heading(ui, "Storage", false);
        crate::storagecard::ui(ui, &self.engine, window_shown());

        home_heading(ui, "What next", false);
        let mut go = None;
        th::group(ui, |ui| {
            let rows = [
                ("\u{E7FC}", "Add your games", "Per-game settings and the game booster", Page::Games),
                ("\u{E839}", "Test your internet", "Speed, ping and connection repair", Page::Vpn),
                ("\u{E74D}", "Free up space", "Junk cleaner and startup programs", Page::Tools),
            ];
            for (i, (glyph, title, sub, page)) in rows.into_iter().enumerate() {
                let slot = ui.painter().add(egui::Shape::Noop); // the hover tint goes under the row's text
                let r = ui.scope(|ui| {
                    th::card(ui, (10, 11, 14, 11), |ui| {
                        ui.horizontal(|ui| {
                            ui.label(ico(glyph, 18.0).color(th::TEXT));
                            ui.add_space(6.0);
                            ui.vertical(|ui| {
                                ui.label(th::md(title, 14.0));
                                ui.add_space(1.0);
                                ui.label(rt(sub, 13.0).color(home_sub()));
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(ico("\u{E76C}", 11.0).color(text3())));
                        });
                    });
                });
                let resp = ui.interact(r.response.rect, Id::new(("tile", i)), Sense::click());
                let hov = th::anim(ui.ctx(), resp.id.with("hover"), resp.hovered(), th::HOVER);
                if hov > 0.0 {
                    ui.painter().set(slot, egui::Shape::rect_filled(r.response.rect.shrink(3.0), th::nested(th::R_CARD, 3.0), Color32::from_white_alpha((16.0 * hov) as u8)));
                }
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    go = Some(page);
                }
            }
        });
        if let Some(pg) = go {
            self.page = pg;
        }
    }

    /// Right column of Home: the live readings on one plate, each with a history graph `graph` high.
    fn home_side(&mut self, ui: &mut Ui, graph: f32, first: bool) {
        home_heading(ui, "Right now", first);
        let dash = || "\u{2014}".to_string();
        let gpu = self.gpu;
        // label, the big value, two lines under it, history
        let mut rows: Vec<(&str, String, String, String, &[f32])> = Vec::with_capacity(3);
        let glyphs = ["\u{E950}", "\u{F211}", "\u{E964}"]; // chip, graphics card, memory module
        let temp = gpu.and_then(|g| g.cpu_temp).map(|t| match gpu.and_then(|g| g.cpu_power_w) {
            Some(p) => format!("{t:.0} °C, {p:.0} W"),
            None => format!("{t:.0} °C"),
        });
        rows.push(("Processor", self.cpu_hist.last().map_or_else(dash, |c| format!("{c:.0}%")), "Load across all cores".into(), temp.unwrap_or_else(|| "Temperature unavailable".into()), &self.cpu_hist));
        rows.push(match gpu.and_then(|g| g.gpu_load.map(|l| (g, l))) {
            Some((g, load)) => {
                let parts: Vec<String> = [g.gpu_temp.map(|t| format!("{t:.0} °C")), g.gpu_clock_mhz.map(|c| format!("{c:.0} MHz")), g.gpu_power_w.map(|p| format!("{p:.0} W"))].into_iter().flatten().collect();
                let memory = match (g.vram_used_mb, g.vram_total_mb) {
                    (Some(u), Some(t)) => format!("{:.1} / {:.1} GB video memory", u / 1024.0, t / 1024.0),
                    _ => String::new(),
                };
                ("Graphics", format!("{load:.0}%"), if parts.is_empty() { "Graphics load".into() } else { parts.join(", ") }, memory, &self.gpu_hist[..])
            }
            None => ("Graphics", dash(), "Reading the graphics card\u{2026}".into(), String::new(), &[]),
        });
        rows.push(match pccheck::memory_now() {
            Some((used, tot, free, load)) => ("Memory", format!("{load}%"), format!("{used:.1} / {tot:.1} GB in use"), format!("{free:.1} GB free"), &self.ram_hist),
            None => ("Memory", dash(), String::new(), String::new(), &[]),
        });
        th::group(ui, |ui| {
            for ((label, value, l1, l2, hist), glyph) in rows.into_iter().zip(glyphs) {
                th::card(ui, (10, 11, 14, 10), |ui| {
                    ui.horizontal(|ui| {
                        ui.label(ico(glyph, 18.0).color(th::TEXT));
                        ui.add_space(2.0);
                        ui.label(th::md(label, 14.0).color(th::TEXT));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(dsb(value, 24.0)));
                    });
                    for line in [l1, l2] {
                        if !line.is_empty() {
                            ui.label(rt(line, 13.0).color(home_sub()));
                        }
                    }
                    ui.add_space(8.0);
                    th::spark_filled(ui, hist, graph);
                });
            }
        });
    }

    /// Full width under Home's columns: this PC's parts on one plate, three cells a row. Long names wrap.
    fn home_pc(&mut self, ui: &mut Ui) {
        home_heading(ui, "This PC", false);
        let pc = self.this_pc.clone().unwrap_or_default();
        let rows = [
            [("Processor", short_part_name(&pc.cpu)), ("Cores", pc.cores), ("Memory", pc.memory)],
            [("Graphics", short_part_name(&pc.gpu)), ("Windows", pc.windows), ("Power plan", pc.power_plan)],
        ];
        th::group(ui, |ui| {
            for row in rows {
                th::card(ui, (10, 12, 14, 12), |ui| {
                    let (gap, full) = (24.0, ui.available_width());
                    let cell = ((full - 2.0 * gap) / 3.0).floor();
                    let r = ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for (key, val) in row {
                            ui.allocate_ui_with_layout(vec2(cell, 0.0), Layout::top_down(Align::Min), |ui| {
                                ui.set_width(cell);
                                ui.label(rt(key, 13.0).color(home_sub()));
                                ui.add_space(3.0);
                                ui.add(egui::Label::new(rt(if val.is_empty() { "\u{2026}".into() } else { val }, 14.0)).wrap());
                            });
                        }
                    });
                    let rect = r.response.rect;
                    for k in 1..3 {
                        let x = rect.left() + k as f32 * (cell + gap) - gap / 2.0;
                        ui.painter().line_segment([pos2(x, rect.top() - 4.0), pos2(x, rect.bottom() + 4.0)], Stroke::new(th::px(), Color32::from_white_alpha(0x1C)));
                    }
                });
            }
        });
    }

    fn pc_check(&mut self, ui: &mut Ui) {
        let mut fix = None;
        let mut run = false;
        let busy = self.check_rx.is_some();
        // one plate: the summary with its button, then a row per finding
        th::group(ui, |ui| {
            th::card(ui, (10, 12, 14, 12), |ui| {
                let summary = match (&self.checks, busy) {
                    (_, true) => "Checking\u{2026}".to_string(),
                    (Some(c), _) => pccheck::summary(c),
                    _ => "Checks power plan, refresh rate, memory, temperatures, drive space, startup programs, background load and your connection. Takes about ten seconds.".to_string(),
                };
                // the text is laid out first so the row knows its height and the button can sit centred beside it
                let avail = ui.available_width();
                let btn = if busy { None } else { Some(th::BTN.0 + th::BTN.2 + ui.painter().layout_no_wrap("Run check".into(), FontId::new(14.0, th::medium()), th::TEXT).size().x) };
                let text_w = avail - btn.map_or(0.0, |b| b + 16.0);
                let galley = egui::WidgetText::from(rt(summary, 13.0).color(home_sub())).into_galley(ui, Some(egui::TextWrapMode::Wrap), text_w, egui::TextStyle::Body);
                let row_h = galley.size().y.max(if busy { 0.0 } else { 34.0 });
                ui.allocate_ui_with_layout(vec2(avail, row_h), Layout::right_to_left(Align::Center), |ui| {
                    ui.set_min_height(row_h);
                    if !busy && th::button(ui, "Run check", false, th::BTN).clicked() {
                        run = true;
                    }
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| ui.label(galley));
                });
            });
            for ch in self.checks.iter().flatten() {
                th::card(ui, (10, 12, 14, 12), |ui| {
                    ui.horizontal_top(|ui| {
                        let (txt, col, bg) = match ch.level {
                            Level::Ok => ("OK", th::SUCCESS, th::success_soft()),
                            Level::Tip => ("Tip", th::WARNING, th::warning_soft()),
                            Level::Warn => ("Check", th::WARNING, th::warning_soft()),
                        };
                        let (chip, _) = ui.allocate_exact_size(vec2(52.0, 20.0), Sense::hover());
                        ui.painter().rect_filled(chip, 6.0, bg);
                        ui.painter().text(chip.center(), egui::Align2::CENTER_CENTER, txt, FontId::new(12.0, th::semibold()), col);
                        ui.add_space(12.0);
                        let avail = ui.available_width();
                        ui.vertical(|ui| {
                            ui.set_max_width((avail - 150.0).max(120.0));
                            ui.label(th::md(ch.title, 14.0));
                            ui.add_space(2.0);
                            ui.label(rt(&ch.message, 13.0).color(home_sub()));
                        });
                        if let Some(f) = ch.fix {
                            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                                if th::button(ui, f.label(), false, th::BTN).clicked() {
                                    fix = Some(f);
                                }
                            });
                        }
                    });
                });
            }
        });
        if let (Some(f), true) = (fix, self.work_rx.is_none()) {
            // on a thread like every other change (a fix runs powercfg); `poll` shows the result and checks again
            let (tx, rx) = mpsc::channel();
            let engine = self.engine.clone();
            self.status = Some(("Working\u{2026}".into(), true));
            self.work_page = self.page;
            std::thread::spawn(move || {
                let st = match pccheck::run_fix(f, &engine) {
                    Ok(()) => ("Done.".to_string(), true),
                    Err(e) => (format!("{e}"), false),
                };
                let _ = tx.send((st, Default::default()));
            });
            self.work_rx = Some(rx);
        }
        if run {
            self.start_check(ui.ctx());
        }
    }

    fn optimize_modal(&mut self, ctx: &egui::Context) {
        let (_, todo) = self.recommended_todo();
        let m = egui::Modal::new(Id::new("opt")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(dsb("Apply these tweaks?", 20.0));
            ui.add_space(4.0);
            ui.label(rt("Each one is backed up first. You can undo them on the Restore page.", 12.0).color(text2()));
            ui.add_space(10.0);
            egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
                for &i in &todo {
                    ui.label(rt(format!("\u{2022}  {}", self.tweaks[i].title), 14.0));
                    ui.add_space(4.0);
                }
            });
            ui.add_space(10.0);
            let mut go = false;
            ui.horizontal(|ui| {
                go = th::button(ui, "Apply", true, (18.0, 8.0, 18.0, 9.0)).clicked();
                if th::button(ui, "Cancel", false, (14.0, 8.0, 14.0, 9.0)).clicked() {
                    ui.close();
                }
            });
            go
        });
        if m.inner {
            self.run_work(todo, true);
            self.confirm_optimize = false;
        } else if m.should_close() {
            self.confirm_optimize = false;
        }
    }

    /// Every tweak MyTune has a backup for, in the order the Restore list shows them.
    fn revert_all_modal(&mut self, ctx: &egui::Context) {
        let idxs: Vec<usize> = self.ids_in_store.iter().filter_map(|id| self.tweaks.iter().position(|t| &t.id == id)).collect();
        // game tweaks and the PC check fix are not in the tweak list
        let others: Vec<String> = self.ids_in_store.iter().filter(|id| !self.tweaks.iter().any(|t| &t.id == *id) && mytune_core::pergame::reverts_alone(id)).cloned().collect();
        let files = others.iter().any(|id| id.ends_with(":config"));
        let busy = self.work_rx.is_some() || self.games.busy();
        let m = egui::Modal::new(Id::new("revert_all")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(dsb("Revert all changes?", 20.0));
            ui.add_space(4.0);
            ui.label(rt(format!("{} tweaks go back to how they were before MyTune changed them.", idxs.len() + others.len()), 12.0).color(text2()));
            if files {
                ui.add_space(4.0);
                ui.label(rt("Game settings files go back to the copy from before MyTune's first save. Anything changed in those games since then is overwritten. A game that is running is skipped and stays as it is.", 12.0).color(th::WARNING));
            }
            ui.add_space(10.0);
            let mut go = false;
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    go = th::button(ui, "Revert all", true, (18.0, 8.0, 18.0, 9.0)).clicked();
                });
                if th::button(ui, "Cancel", false, (14.0, 8.0, 14.0, 9.0)).clicked() {
                    ui.close();
                }
            });
            go
        });
        if m.inner {
            self.run_work(idxs, false);
            self.games.revert_all(others);
            self.confirm_revert_all = false;
        } else if m.should_close() {
            self.confirm_revert_all = false;
        }
    }

    /// Only for an adapter tweak whose last revert failed because the adapter is not connected.
    fn forget_adapter_modal(&mut self, ctx: &egui::Context, i: usize) {
        let busy = self.work_rx.is_some();
        let title = self.tweaks[i].title.clone();
        let m = egui::Modal::new(Id::new("forget_adapter")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(dsb("Forget this adapter?", 20.0));
            ui.add_space(4.0);
            ui.label(rt(format!("{title}: MyTune tries to restore it once more. If the adapter is still not connected, its saved original settings are deleted. This cannot be undone. Use it only for an adapter you no longer have."), 12.0).color(text2()));
            ui.add_space(10.0);
            let mut go = false;
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!busy, |ui| {
                    go = th::button(ui, "Forget", true, (18.0, 8.0, 18.0, 9.0)).clicked();
                });
                if th::button(ui, "Cancel", false, (14.0, 8.0, 14.0, 9.0)).clicked() {
                    ui.close();
                }
            });
            go
        });
        if m.inner {
            self.confirm_forget = None;
            let (tx, rx) = mpsc::channel();
            let (tweaks, engine, gone) = (self.tweaks.clone(), self.engine.clone(), self.adapter_gone.clone());
            self.states_rx = None;
            self.status = Some(("Working\u{2026}".into(), true));
            self.work_page = self.page;
            std::thread::spawn(move || {
                let t = &tweaks[i];
                let r = mytune_core::special::forget_missing_adapter(&engine, t);
                gone.lock().unwrap_or_else(|p| p.into_inner()).remove(&t.id);
                let msg = match r {
                    Ok(true) => (format!("{}: saved adapter settings forgotten", t.title), true),
                    Ok(false) => (format!("{}: the adapter is back, original restored", t.title), true),
                    Err(e) => (format!("{}: {e}", t.title), false),
                };
                let _ = tx.send((msg, vec![(t.id.clone(), t.is_applied().unwrap_or(false))]));
            });
            self.work_rx = Some(rx);
        } else if m.should_close() {
            self.confirm_forget = None;
        }
    }

    // ------------------------------------------------------------ list pages

    fn tweak_list(&mut self, ui: &mut Ui) {
        let q = self.search.to_lowercase();
        let mut click = None;
        // Optimize is split into normal, risky (security risk) and experimental tweaks, in that order.
        let categorized = self.page == Page::Optimize;
        // Same order and group headings as C# `Layout.cs`.
        let mut order: Vec<usize> = (0..self.tweaks.len()).collect();
        order.sort_by_cached_key(|&i| mytune_core::layout::placement(&self.tweaks[i].id).map_or(usize::MAX, |p| p.order));
        #[cfg(feature = "taskbar-look")]
        if self.page == Page::Customize && (q.is_empty() || "taskbar look clear tinted transparent".contains(&q)) {
            self.taskbar.ui(ui);
        }
        if self.page == Page::Debloat && (q.is_empty() || "auto debloat remove uninstall preinstalled apps".contains(&q)) {
            crate::debloatapps::ui(ui);
        }
        // the tweaks in display order, cut into sections: each section is one plate of rows
        let mut sections: Vec<(&str, Vec<usize>)> = Vec::new();
        for cat in 0..3 {
            for &i in &order {
                let t = &self.tweaks[i];
                if t.removed || page_of(&t.id) != Some(self.page) {
                    continue;
                }
                if !q.is_empty() && !t.title.to_lowercase().contains(&q) && !t.description.to_lowercase().contains(&q) {
                    continue;
                }
                let c = if t.flags.has(TweakFlags::SECURITY_RISK) { 1 } else if t.flags.has(TweakFlags::EXPERIMENTAL) { 2 } else { 0 };
                if (categorized && c != cat) || (!categorized && cat > 0) {
                    continue;
                }
                let heading = if categorized { ["Normal tweaks", "Risky", "Experimental"][cat] } else { mytune_core::layout::placement(&t.id).map_or("", |p| p.group) };
                match sections.last_mut() {
                    Some((h, rows)) if *h == heading => rows.push(i),
                    _ => sections.push((heading, vec![i])),
                }
            }
        }
        let partly = self.engine.with_store(|s| s.partly_applied_ids());
        for (heading, rows) in sections {
            th::section_gap(ui, heading);
            th::group(ui, |ui| {
                for i in rows {
                    let t = &self.tweaks[i];
                    let on = self.applied.get(&t.id).copied().unwrap_or(false);
                    th::card(ui, (10, 13, 14, 13), |ui| {
                        ui.horizontal(|ui| {
                            let avail = ui.available_width();
                            ui.vertical(|ui| {
                                ui.set_max_width((avail - 64.0).max(120.0));
                                ui.label(th::md(&t.title, 14.0));
                                ui.add_space(2.0);
                                if partly.contains(&t.id) {
                                    th::chip(ui, "Partly applied, undo it in Restore", th::WARNING, th::warning_soft());
                                }
                                th::flag_chips(ui, t.flags);
                                th::more_info(ui, egui::Id::new(("more_info", &t.id)), &t.description);
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if th::switch(ui, on).on_hover_text(&t.details).clicked() {
                                    click = Some((i, !on));
                                }
                            });
                        });
                    });
                }
            });
        }
        if let Some((i, on)) = click {
            self.set(i, on);
        }
    }

    fn restore(&mut self, ui: &mut Ui) {
        if self.ids_in_store.is_empty() {
            th::card(ui, (10, 16, 16, 16), |ui| {
                ui.label(rt("Nothing to undo. MyTune has not changed anything.", 14.0).color(text2()));
            });
            return;
        }
        let (mut revert, mut game_undo, mut forget, mut forget_adapter) = (None, None, None, None);
        th::card(ui, (10, 16, 16, 16), |ui| {
            ui.horizontal(|ui| {
                ui.set_min_height(ROW_H);
                ui.label(sb(format!("{} changes by MyTune", self.ids_in_store.len()), 14.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if th::button(ui, "Revert all", true, th::BTN).clicked() {
                        self.confirm_revert_all = true;
                    }
                });
            });
        });
        ui.add_space(th::GAP);
        th::group(ui, |ui| {
            for id in &self.ids_in_store {
                let idx = self.tweaks.iter().position(|t| &t.id == id);
                // a per-game tweak or the PC check fix: not in the tweak list, its title is in the backup
                let game = mytune_core::pergame::reverts_alone(id).then(|| self.engine.with_store(|s| s.get_backup(id).map(|b| b.title.clone()))).flatten();
                th::card(ui, (10, 10, 14, 10), |ui| {
                    ui.horizontal(|ui| {
                        ui.set_min_height(ROW_H);
                        ui.label(th::md(idx.map(|i| self.tweaks[i].title.as_str()).or(game.as_deref()).unwrap_or(id), 14.0));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| match idx {
                            Some(i) => {
                                if th::button(ui, "Undo", false, th::BTN).clicked() {
                                    revert = Some(i);
                                }
                                // only after an Undo that failed because the adapter is not connected
                                let gone = self.adapter_gone.lock().unwrap_or_else(|p| p.into_inner()).contains(id);
                                if gone && th::button(ui, "Forget", false, th::BTN).on_hover_text("For an adapter you no longer have: deletes its saved original settings.").clicked() {
                                    forget_adapter = Some(i);
                                }
                            }
                            None if game.is_some() => {
                                if th::button(ui, "Undo", false, th::BTN).clicked() {
                                    game_undo = Some((id.clone(), game.clone().unwrap_or_default()));
                                }
                                // only after an Undo that could not reach the game's file
                                if self.games.is_stuck(id) && th::button(ui, "Forget", false, th::BTN).on_hover_text("For a game you uninstalled: deletes MyTune's copy and this entry.").clicked() {
                                    forget = Some((id.clone(), game.clone().unwrap_or_default()));
                                }
                            }
                            None => {
                                ui.label(rt("not available in this UI yet", 12.0).color(text3()));
                            }
                        });
                    });
                });
            }
        });
        if let Some(i) = revert {
            self.set(i, false);
        }
        if let Some((id, title)) = game_undo {
            self.games.revert(id, title);
        }
        if let Some((id, title)) = forget {
            self.games.ask_forget(id, title);
        }
        if forget_adapter.is_some() {
            self.confirm_forget = forget_adapter;
        }
    }
}

/// Height of a `th::BTN` button. A row that starts with a title and ends with a button takes this height first:
/// egui centres each item in the row as it stood when the item was added, so the title sat above the middle.
const ROW_H: f32 = 30.0;

/// Holds the frame rate at the display's refresh rate. With V-Sync forced off in the graphics driver (common
/// on a gaming PC) every mouse move and every animation frame drew at once: 500 to 800 frames a second and
/// half a CPU core, measured 2026-11-15. With V-Sync working this never waits.
fn frame_cap() {
    use std::{sync::Mutex, time::{Duration, Instant}};
    // (start of the last frame, frame time, when the refresh rate was last read)
    // env MYTUNE_NO_CAP: benchmarks measure the frame's own cost (with the cap, bench_scroll printed the display's period)
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *OFF.get_or_init(|| std::env::var_os("MYTUNE_NO_CAP").is_some()) {
        return;
    }
    static LAST: Mutex<Option<(Instant, Duration, Option<Instant>)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap_or_else(|p| p.into_inner());
    let now = Instant::now();
    // no `now - 60 s` here: an Instant before boot panics, and autostart runs this in the first minute
    let (prev, mut period, mut read) = last.unwrap_or((now, Duration::ZERO, None));
    if read.is_none_or(|r| now - r > Duration::from_secs(10)) {
        period = Duration::from_secs_f64(1.0 / refresh_hz() as f64);
        read = Some(now);
    }
    if let Some(wait) = period.checked_sub(now - prev) {
        std::thread::sleep(wait);
    }
    *last = Some((Instant::now(), period, read));
}

/// False while MyTune's window is hidden in the tray or minimised. True when no window is found (headless tests).
pub(crate) fn window_shown() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{FindWindowW, IsIconic, IsWindowVisible};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    unsafe {
        let w = FindWindowW(wide("Window Class").as_ptr(), wide("MyTune").as_ptr());
        w.is_null() || (IsWindowVisible(w) != 0 && IsIconic(w) == 0)
    }
}

/// The primary display's refresh rate, 60 when Windows does not say.
// shortcut: the primary display's rate, also when the window is on another one; read the window's monitor if rates differ
fn refresh_hz() -> u32 {
    use windows_sys::Win32::Graphics::Gdi::{EnumDisplaySettingsW, DEVMODEW, ENUM_CURRENT_SETTINGS};
    let mut dm: DEVMODEW = unsafe { std::mem::zeroed() };
    dm.dmSize = std::mem::size_of::<DEVMODEW>() as u16;
    let ok = unsafe { EnumDisplaySettingsW(std::ptr::null(), ENUM_CURRENT_SETTINGS, &mut dm) } != 0;
    if ok && (30..=1000).contains(&dm.dmDisplayFrequency) { dm.dmDisplayFrequency } else { 60 }
}

/// With env `MYTUNE_PERF=<file>`: every 2 s, appends how many frames were drawn and what asked for them
/// (the code line of each repaint request), to find a page that never rests. Costs nothing when unset.
fn perf_log(ctx: &egui::Context) {
    use std::{collections::BTreeMap, io::Write, sync::{Mutex, OnceLock}};
    static FILE: OnceLock<Option<String>> = OnceLock::new();
    static SEEN: Mutex<(f64, u32, BTreeMap<String, u32>)> = Mutex::new((0.0, 0, BTreeMap::new()));
    let Some(path) = FILE.get_or_init(|| std::env::var("MYTUNE_PERF").ok()) else { return };
    let mut s = SEEN.lock().unwrap();
    s.1 += 1;
    static COST: Mutex<(u64, u32)> = Mutex::new((0, 0)); // (sum, worst) of the frames' cost in microseconds
    let us = FRAME_US.load(std::sync::atomic::Ordering::Relaxed);
    let mut cost = COST.lock().unwrap();
    *cost = (cost.0 + us as u64, cost.1.max(us));
    for c in ctx.repaint_causes() {
        *s.2.entry(c.to_string()).or_default() += 1;
    }
    let now = ctx.input(|i| i.time);
    if now - s.0 >= 2.0 {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{:.0}s frames={} mean_ms={:.2} worst_ms={:.2} {:?}", now, s.1, cost.0 as f64 / s.1.max(1) as f64 / 1000.0, cost.1 as f64 / 1000.0, s.2);
        }
        *cost = (0, 0);
        *s = (now, 0, BTreeMap::new());
    }
}

/// The last frame's whole cost as eframe measured it (update, tessellation and painting, no V-Sync wait), in
/// microseconds; `perf_log` prints its mean and worst.
static FRAME_US: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        FRAME_US.store((_frame.info().cpu_usage.unwrap_or(0.0) * 1e6) as u32, std::sync::atomic::Ordering::Relaxed);
        self.ui(ui);
    }
}

/// Smallest height of a history graph on Home.
const GRAPH_MIN: f32 = 40.0;

/// Secondary text on Home: brighter than `text2` there, the page is read by people new to PCs.
fn home_sub() -> Color32 { Color32::from_white_alpha(0xD9) }

/// `th::heading` for Home: 13 px and as bright as `home_sub`.
fn home_heading(ui: &mut Ui, text: &str, first: bool) {
    // The headline text is no longer drawn (user's choice): only the gap between cards is left.
    let _ = text;
    if !first {
        ui.add_space(th::GAP);
    }
}

/// A processor or graphics card name without marks and filler: "13th Gen Intel(R) Core(TM) i5-13600KF" -> "Intel Core i5-13600KF".
fn short_part_name(name: &str) -> String {
    let mut s = name.split('@').next().unwrap_or("").to_string();
    for mark in ["(R)", "(r)", "(TM)", "(tm)", "\u{AE}", "\u{2122}"] {
        s = s.replace(mark, " ");
    }
    let words: Vec<&str> = s.split_whitespace().collect();
    let ordinal = |w: &str| {
        let digits = w.trim_end_matches(char::is_alphabetic);
        !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) && ["st", "nd", "rd", "th"].contains(&&w[digits.len()..])
    };
    let mut out = Vec::with_capacity(words.len());
    let mut i = 0;
    while i < words.len() {
        if ordinal(words[i]) && words.get(i + 1).is_some_and(|w| w.eq_ignore_ascii_case("gen")) {
            i += 2;
            continue;
        }
        out.push(words[i]);
        i += 1;
    }
    if out.last().is_some_and(|w| w.eq_ignore_ascii_case("cpu")) {
        out.pop();
    }
    out.join(" ")
}

#[cfg(test)]
mod tests {
    use super::short_part_name;

    #[test]
    fn short_part_names() {
        assert_eq!(short_part_name("13th Gen Intel(R) Core(TM) i5-13600KF"), "Intel Core i5-13600KF");
        assert_eq!(short_part_name("Intel(R) Core(TM) i7-8700 CPU @ 3.20GHz"), "Intel Core i7-8700");
        assert_eq!(short_part_name("NVIDIA GeForce RTX 3070 Ti"), "NVIDIA GeForce RTX 3070 Ti");
        assert_eq!(short_part_name("AMD Ryzen 7 5800X3D 8-Core Processor  "), "AMD Ryzen 7 5800X3D 8-Core Processor");
        assert_eq!(short_part_name("Intel(R) UHD Graphics 770"), "Intel UHD Graphics 770");
        assert_eq!(short_part_name(""), "");
    }
}
