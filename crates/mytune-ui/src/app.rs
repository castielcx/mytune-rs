use crate::nav::{Item, Page, GROUPS, SETTINGS};
use crate::theme::{self as th, bd, dbd, dsb, ico, p, rt, sb, text2, text3};
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
    diagnostics: crate::diagnosticspage::DiagnosticsPage,
    display: crate::displaypage::DisplayPage,
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
    last_cpu: Option<(u64, u64)>,
    last_sample: Option<Instant>,
    side_w: f32,
    anim_base: Anim,
    anim_side: Anim,
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
        Self::with_store(Store::load_default())
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
        let engine = Arc::new(Engine::new(store));
        let mut app = App {
            page: Page::Chat,
            internet: crate::internet::InternetPage::new(engine.clone()),
            tools: crate::toolspage::ToolsPage::new(engine.clone()),
            settings: crate::settingspage::SettingsPage::new(engine.clone()),
            mouse: crate::mouse::MousePage::new(),
            sound: crate::soundpage::SoundPage::new(),
            diagnostics: crate::diagnosticspage::DiagnosticsPage::new(engine.clone()),
            display: crate::displaypage::DisplayPage::new(engine.clone()),
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
            last_cpu: None,
            last_sample: None,
            side_w: 248.0,
            anim_base: Anim::new(248.0),
            anim_side: Anim::new(248.0),
            anim_pill: Anim::new(0.0),
            hover_since: None,
            suppress_hover: false,
            pill_y: 0.0,
            keep_in_tray: false,
        };
        app.refresh();
        app
    }

    pub fn goto(&mut self, page: Page) {
        self.page = page;
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
            self.refresh();
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
        let (tweaks, engine) = (self.tweaks.clone(), self.engine.clone());
        // a re-read started before this change would bring the old state back
        self.states_rx = None;
        self.status = Some(("Working\u{2026}".into(), true));
        self.work_page = self.page;
        // the switch moves at once; the thread's reading corrects it if the change failed
        for &i in &idxs {
            self.applied.insert(self.tweaks[i].id.clone(), on);
        }
        std::thread::spawn(move || {
            let (mut ok, mut bad, mut last) = (0, 0, String::new());
            for &i in &idxs {
                let t = &tweaks[i];
                let r = if on {
                    engine.apply(t).map_err(|e| e.to_string())
                } else {
                    match engine.revert(t) {
                        Ok(mytune_core::engine::RevertOutcome::Reverted) => Ok(()),
                        Ok(mytune_core::engine::RevertOutcome::NoBackup) => Err("set outside MyTune and its Windows default is unknown, nothing changed".to_string()),
                        Err(e) => Err(e.to_string()),
                    }
                };
                match r {
                    Ok(()) => ok += 1,
                    Err(e) => {
                        bad += 1;
                        last = format!("{}: {e}", t.title);
                    }
                }
            }
            let msg = if idxs.len() == 1 {
                if bad == 0 { (format!("{}: {}", tweaks[idxs[0]].title, if on { "applied" } else { "reverted" }), true) } else { (last, false) }
            } else {
                let tail = if bad > 0 { format!(", {bad} failed (see log)") } else { String::new() };
                (format!("{} {ok} tweaks{tail}", if on { "Applied" } else { "Reverted" }), bad == 0)
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
        if let Some((_, _, _, load)) = pccheck::memory_now() {
            self.ram_hist.push(load as f32);
        }
        if let Some((idle, total)) = pccheck::cpu_ticks() {
            if let Some((i0, t0)) = self.last_cpu {
                let dt = total.saturating_sub(t0) as f32;
                if dt > 0.0 {
                    self.cpu_hist.push((100.0 * (1.0 - idle.saturating_sub(i0) as f32 / dt)).clamp(0.0, 100.0));
                }
            }
            self.last_cpu = Some((idle, total));
        }
        if let Some(snap) = self.sensors.as_ref().and_then(|s| s.latest()) {
            if let Some(l) = snap.gpu_load {
                self.gpu_hist.push(l);
            }
            self.gpu = Some(snap);
        }
        for h in [&mut self.ram_hist, &mut self.cpu_hist, &mut self.gpu_hist] {
            if h.len() > 60 {
                h.remove(0);
            }
        }
    }

    pub fn ui(&mut self, root: &mut Ui) {
        let ctx = root.ctx().clone();
        let ctx = &ctx;
        frame_cap();
        perf_log(ctx);
        self.poll(ctx);
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::F)) {
            ctx.memory_mut(|m| m.request_focus(Id::new("search")));
        }
        // Settings > "Keep running in the tray when closed" (default on): off means closing exits.
        let tray_on = self.engine.with_store(|s| s.settings_flag("MinimizeToTray")).unwrap_or(true);
        if self.keep_in_tray && tray_on && ctx.input(|i| i.viewport().close_requested()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            std::thread::spawn(|| {
                std::thread::sleep(Duration::from_secs(2));
                mytune_core::process::trim_working_set();
            });
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
        if let Some(s) = &self.sensors {
            s.set_watched(self.page == Page::Dashboard); // 1 s samples only while Home is on screen
        }
        if self.page == Page::Dashboard {
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
        let base_w = self.anim_base.update(ctx, if home { 248.0 } else { 64.0 }, 0.22).round();
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
        }
        // the chat draws its own opaque layers, so it keeps a solid base; every other page shows the backdrop
                egui::CentralPanel::default().frame(egui::Frame::new()).show(root, |ui| {
            // nothing shows through the glass sidebar while it still covers the page
            let mut clip = ui.clip_rect();
            clip.min.x = clip.min.x.max(self.side_w);
            ui.set_clip_rect(clip);
            if self.page == Page::Chat {
                // the chat sits in a rounded card inside the window: its square corners are covered with the base colour
                let card = ui.max_rect().shrink2(vec2(10.0, 8.0));
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
                    if self.page != Page::Dashboard {
                        let settings = matches!(self.page, Page::Settings | Page::Restore);
                        ui.label(dbd(if settings { Page::Settings.label() } else { self.page.label() }, 30.0));
                        ui.add_space(12.0);
                        if settings {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                for (page, name) in [(Page::Settings, "General"), (Page::Restore, "Restore")] {
                                    if th::tab(ui, name, self.page == page).clicked() {
                                        self.page = page;
                                    }
                                }
                            });
                            ui.add_space(14.0);
                        }
                        if let Some((msg, ok)) = &self.status {
                            ui.label(rt(msg, 12.0).color(if *ok { th::SUCCESS } else { th::DANGER }));
                            ui.add_space(8.0);
                        }
                    }
                    match self.page {
                        Page::Dashboard => self.home(ui),
                        Page::Optimize | Page::Debloat | Page::Customize => self.tweak_list(ui),
                        Page::Restore => self.restore(ui), // reached only through Settings > Restore now
                        Page::Vpn => self.internet.ui(ui, &self.search),
                        Page::Tools => self.tools.ui(ui),
                        Page::Settings => self.settings.ui(ui),
                        Page::Mouse => self.mouse.ui(ui),
                        Page::Sound => self.sound.ui(ui),
                        Page::Diagnostics => self.diagnostics.ui(ui),
                        Page::Display => self.display.ui(ui),
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
        crate::fade::end_frame(ctx);
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
            ui.painter().rect_filled(logo, 6.0, pal.accent);
            ui.painter().rect_stroke(logo, 6.0, Stroke::new(1.0, Color32::from_white_alpha(0x50)), StrokeKind::Inside);
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
                ui.painter().text(pos2(logo.right() + 12.0, bar.center().y), egui::Align2::LEFT_CENTER, "MyTune", FontId::proportional(12.0), pal.side_text.gamma_multiply(((base_w - 120.0) / 128.0).clamp(0.0, 1.0)));
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
                if resp.hovered() {
                    th::capsule(ui.painter(), r.shrink2(vec2(5.0, 3.0)), if close { pal.block[1] } else { Color32::from_white_alpha(0x22) }, 0x26);
                }
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, glyph, FontId::new(10.0, th::icons()), if close && resp.hovered() { Color32::WHITE } else { pal.side_text });
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
        let w = self.anim_side.update(ctx, if home || hovered { 248.0 } else { 64.0 }, 0.22); // not rounded: 1 px steps stutter
        self.side_w = w;
        let rect = Rect::from_min_size(pos2(0.0, top), vec2(w, screen.height() - top));
        let fade = ((w - 64.0) / 184.0).clamp(0.0, 1.0);
        let prev_pill = self.pill_y;
        self.pill_y = 0.0;
        let pill = self.anim_pill.update(ctx, prev_pill, 0.22).round();
        egui::Area::new(Id::new("sidebar")).fixed_pos(rect.min).order(egui::Order::Middle).show(ctx, |ui| {
            ui.allocate_rect(rect, Sense::hover());
            let paint = ui.painter().clone();
            paint.add(th::glass(rect.shrink2(vec2(6.0, 4.0)), 24.0, pal.side));
            let inner = Rect::from_min_max(rect.min + vec2(8.0, 4.0), rect.max - vec2(8.0, 12.0));
            ui.scope_builder(UiBuilder::new().max_rect(inner), |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                let scroll_h = (inner.height() - 40.0).max(0.0);
                egui::ScrollArea::vertical().max_height(scroll_h).auto_shrink([false, true]).scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden).show(ui, |ui| {
                    ui.set_width(inner.width());
                    // Same row in the rail and the full sidebar, so the icons below never move.
                    ui.add_space(2.0);
                    let r = Rect::from_min_size(ui.cursor().min, vec2(inner.width(), 36.0));
                    ui.allocate_space(vec2(inner.width(), 36.0));
                    let focused = ui.memory(|m| m.has_focus(Id::new("search")));
                    // same row as every nav item: ink fill, light edge, hover tint, icon in the same column
                    if focused {
                        ui.painter().rect_stroke(r, 18.0, Stroke::new(1.0, pal.accent), StrokeKind::Inside);
                    }
                    if ui.rect_contains_pointer(r) {
                        ui.painter().rect_filled(r, 18.0, Color32::from_white_alpha(26));
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
                            ui.add_space(14.0);
                            // painted, not a label: a label kept the sidebar layer ~117 px wide while folded,
                            // so its invisible part ate clicks meant for the chat page's server icons
                            let (tr, _) = ui.allocate_exact_size(vec2(inner.width(), 14.0), Sense::hover());
                            ui.painter().with_clip_rect(tr).text(pos2(tr.left() + 14.0, tr.center().y), egui::Align2::LEFT_CENTER, t, FontId::new(10.5, th::display_bold()), pal.side_muted.gamma_multiply(fade));
                            ui.add_space(4.0);
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
        ui.add_space(2.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 36.0), Sense::click());
        ui.add_space(2.0);
        let sel = self.page == item.page || (item.page == Page::Settings && self.page == Page::Restore); // Restore is a Settings tab
        let paint = ui.painter();
        // Glass motion, same timings as `th::button`: the plate under the pointer fades in, sinks 4% while
        // pressed and the selected plate grows in from 90%. Only the plate moves, icon and name stay put.
        let ctx = ui.ctx();
        let ease = egui::emath::easing::cubic_out;
        let hov = ctx.animate_bool_with_time_and_easing(resp.id.with("hover"), resp.hovered(), 0.14, ease);
        let down = ctx.animate_bool_with_time_and_easing(resp.id.with("down"), resp.is_pointer_button_down_on(), 0.12, ease);
        let on = ctx.animate_bool_with_time_and_easing(resp.id.with("sel"), sel, 0.22, ease);
        let plate = |k: f32| Rect::from_center_size(rect.center(), rect.size() * k);
        if on > 0.0 {
            // raised glass on the sidebar's base glass
            th::capsule(paint, plate(0.9 + 0.1 * on - 0.04 * down), pal.side_selected.gamma_multiply(on), (38.0 * on) as u8);
        }
        if sel {
            self.pill_y = rect.center().y;
        }
        if hov > 0.0 || down > 0.0 {
            paint.rect_filled(plate(1.0 - 0.04 * down), 18.0, Color32::from_white_alpha((26.0 * hov + 22.0 * down) as u8));
        }
        let glyph_col = if sel { Color32::WHITE } else { pal.side_muted };
        let text_col = if sel { Color32::WHITE } else { pal.side_text };
        let gx = rect.left() + 24.0; // fixed column: icon and name stay put, the sidebar edge uncovers the name
        paint.text(pos2(gx, rect.center().y.round()), egui::Align2::CENTER_CENTER, item.glyph, FontId::new(16.0, th::icons()), glyph_col);
        if w > 120.0 {
            let font = if sel { FontId::new(14.0, th::semibold()) } else { FontId::proportional(14.0) };
            paint.with_clip_rect(rect).text(pos2(rect.left() + 46.0, rect.center().y.round()), egui::Align2::LEFT_CENTER, item.label, font, text_col.gamma_multiply(((w - 120.0) / 128.0).clamp(0.0, 1.0)));
        }
        if resp.on_hover_text(item.tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            self.page = item.page;
            self.suppress_hover = true;
        }
    }

    // ------------------------------------------------------------ Home

    fn home(&mut self, ui: &mut Ui) {
        let pal = p();
        // hero
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 150.0), Sense::hover());
        let paint = ui.painter();
        paint.add(th::glass(rect, 24.0, pal.card_bg));
        th::blob(paint, rect.right_center() - vec2(rect.height() * 0.62, 0.0), rect.height() * 0.5 - 2.0, pal.accent, 0.5);
        let g1 = paint.layout_no_wrap("HOME".into(), FontId::new(11.0, th::display_bold()), pal.side_muted);
        let g2 = paint.layout_no_wrap("Your PC, tuned.".into(), FontId::new(34.0, th::display_bold()), Color32::WHITE);
        let g3 = paint.layout_no_wrap("See how it is doing and set it up for gaming with one click.".into(), FontId::proportional(13.0), pal.side_text);
        let total = g1.size().y + 4.0 + g2.size().y + 4.0 + g3.size().y;
        let mut y = rect.center().y - total / 2.0;
        let x = rect.left() + 26.0;
        for (g, gap) in [(g1, 4.0), (g2, 4.0), (g3, 0.0)] {
            let h = g.size().y;
            paint.galley(pos2(x, y), g, Color32::WHITE);
            y += h + gap;
        }
        ui.add_space(8.0);

        // status card
        let (total, todo) = self.recommended_todo();
        let ready = self.states_loaded;
        let all_set = ready && todo.is_empty();
        let mut optimize = false;
        let mut see_all = false;
        th::card(ui, (16, 20, 22, 20), |ui| {
            ui.horizontal(|ui| {
                let (tile, _) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::hover());
                ui.painter().rect_filled(tile, th::nested(th::R_PANEL, 8.0), if all_set { th::success_soft() } else { pal.accent_soft });
                ui.painter().text(tile.center(), egui::Align2::CENTER_CENTER, if all_set { "\u{E73E}" } else { "\u{E945}" }, FontId::new(22.0, th::icons()), if all_set { th::SUCCESS } else { pal.accent });
                ui.add_space(10.0);
                let avail = ui.available_width();
                ui.vertical(|ui| {
                    ui.set_max_width((avail - 360.0).max(120.0));
                    let title = if !ready { "Checking your settings\u{2026}".to_string() } else if all_set { "Your PC is set up for gaming".to_string() } else { format!("{} of {total} recommended settings are still off", todo.len()) };
                    ui.label(dsb(title, 20.0));
                    ui.add_space(4.0);
                    ui.label(rt(if !ready { "This takes a moment." } else if all_set { "Every recommended setting is on." } else { "Optimize now switches them on. You see the list first." }, 12.0).color(text2()));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if th::button(ui, "See all settings", false, (14.0, 8.0, 14.0, 9.0)).clicked() {
                        see_all = true;
                    }
                    if ready && !all_set && th::button(ui, "Optimize now", true, (18.0, 8.0, 18.0, 9.0)).clicked() {
                        optimize = true;
                    }
                });
            });
        });
        if let Some((msg, ok)) = &self.status {
            ui.add_space(6.0);
            ui.label(rt(msg, 12.0).color(if *ok { th::SUCCESS } else { th::DANGER }));
        }
        if optimize {
            self.confirm_optimize = true;
        }
        if see_all {
            self.page = Page::Optimize;
        }

        th::section(ui, "Right now");
        let ram = pccheck::memory_now();
        let cpu = self.cpu_hist.last().copied();
        let (ram_h, cpu_h, gpu_h) = (self.ram_hist.clone(), self.cpu_hist.clone(), self.gpu_hist.clone());
        let gpu = self.gpu;
        grid3(ui, |ui, i| {
            th::card(ui, (12, 14, 18, 10), |ui| {
                let (glyph, label) = [("\u{E950}", "Processor"), ("\u{E7F4}", "Graphics"), ("\u{E964}", "Memory")][i];
                ui.horizontal(|ui| {
                    ui.label(ico(glyph, 16.0).color(th::TEXT));
                    ui.add_space(2.0);
                    ui.label(rt(label, 12.0).color(text2()));
                });
                ui.add_space(6.0);
                let dash = "\u{2014}".to_string();
                let (value, l1, l2, hist): (String, String, String, &[f32]) = match i {
                    0 => {
                        let temp = gpu.and_then(|g| g.cpu_temp).map(|t| {
                            let mut s = format!("{t:.0} °C");
                            if let Some(p) = gpu.and_then(|g| g.cpu_power_w) {
                                s.push_str(&format!(" · {p:.0} W"));
                            }
                            s
                        });
                        (cpu.map(|c| format!("{c:.0}%")).unwrap_or(dash), "Load across all cores".into(), temp.unwrap_or_else(|| "Temperature unavailable".into()), &cpu_h)
                    }
                    1 => match gpu.and_then(|g| g.gpu_load.map(|l| (g, l))) {
                        Some((g, load)) => {
                            let mut l1 = Vec::new();
                            if let Some(t) = g.gpu_temp {
                                l1.push(format!("{t:.0} °C"));
                            }
                            if let Some(c) = g.gpu_clock_mhz {
                                l1.push(format!("{c:.0} MHz"));
                            }
                            if let Some(p) = g.gpu_power_w {
                                l1.push(format!("{p:.0} W"));
                            }
                            let l2 = match (g.vram_used_mb, g.vram_total_mb) {
                                (Some(u), Some(t)) => format!("{:.1} / {:.1} GB video memory", u / 1024.0, t / 1024.0),
                                _ => String::new(),
                            };
                            (format!("{load:.0}%"), if l1.is_empty() { "Graphics load".into() } else { l1.join(" · ") }, l2, &gpu_h[..])
                        }
                        None => (dash, "Reading the graphics card…".into(), String::new(), &[]),
                    },
                    _ => match ram {
                        Some((used, tot, free, load)) => (format!("{load}%"), format!("{used:.1} / {tot:.1} GB in use"), format!("{free:.1} GB free"), &ram_h),
                        None => (dash, String::new(), String::new(), &[]),
                    },
                };
                ui.label(dsb(value, 30.0));
                ui.label(rt(l1, 12.0).color(text2()));
                if !l2.is_empty() {
                    ui.add_space(2.0);
                    ui.label(rt(l2, 12.0).color(text3()));
                }
                ui.add_space(8.0);
                th::spark(ui, hist, 34.0);
            });
        });

        th::section(ui, "PC check");
        self.pc_check(ui);

        th::section(ui, "What next");
        let mut go = None;
        grid3(ui, |ui, i| {
            let (glyph, title, sub, page) = [
                ("\u{E7FC}", "Add your games", "Per-game settings and the game booster", Page::Games),
                ("\u{E839}", "Test your internet", "Speed, ping and connection repair", Page::Vpn),
                ("\u{E74D}", "Free up space", "Junk cleaner and startup programs", Page::Tools),
            ][i];
            let r = ui.scope(|ui| {
                th::card(ui, (10, 14, 16, 14), |ui| {
                    ui.horizontal(|ui| {
                        ui.label(ico(glyph, 20.0).color(th::TEXT));
                        ui.add_space(6.0);
                        ui.vertical(|ui| {
                            ui.label(sb(title, 14.0));
                            ui.label(rt(sub, 12.0).color(text2()));
                        });
                    });
                });
            });
            let resp = ui.interact(r.response.rect, Id::new(("tile", i)), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(r.response.rect, 14.0, Color32::from_white_alpha(13));
            }
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                go = Some(page);
            }
        });
        if let Some(pg) = go {
            self.page = pg;
        }

        th::section(ui, "This PC");
        let pc = self.this_pc.clone().unwrap_or_default();
        th::card(ui, (14, 16, 20, 6), |ui| {
            let dash = |s: &str| if s.is_empty() { "\u{2026}".to_string() } else { s.to_string() };
            let cols = [
                [("Processor", dash(&pc.cpu)), ("Cores", dash(&pc.cores)), ("Graphics", dash(&pc.gpu))],
                [("Memory", dash(&pc.memory)), ("Windows", dash(&pc.windows)), ("Power plan", dash(&pc.power_plan))],
            ];
            // Two equal halves, a 90 px key column and a value that takes the rest (XAML: Auto | * | Auto | *).
            let half = ((ui.available_width() - 24.0) / 2.0).floor();
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 24.0;
                for col in cols {
                    ui.allocate_ui_with_layout(vec2(half, 0.0), Layout::top_down(Align::Min), |ui| {
                        ui.set_width(half);
                        for (key, val) in col {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                ui.allocate_ui_with_layout(vec2(90.0, 18.0), Layout::left_to_right(Align::Center), |ui| {
                                    ui.set_min_width(90.0);
                                    ui.label(rt(key, 12.0).color(text2()));
                                });
                                ui.add(egui::Label::new(rt(val, 14.0)).truncate());
                            });
                            ui.add_space(10.0);
                        }
                    });
                }
            });
        });
    }

    fn pc_check(&mut self, ui: &mut Ui) {
        let mut fix = None;
        let mut run = false;
        let busy = self.check_rx.is_some();
        th::card(ui, (10, 16, 16, 16), |ui| {
            ui.horizontal(|ui| {
                let summary = match (&self.checks, busy) {
                    (_, true) => "Checking\u{2026}".to_string(),
                    (Some(c), _) => pccheck::summary(c),
                    _ => "Checks power plan, refresh rate, memory, temperatures, drive space, startup programs, background load and your connection. Takes about ten seconds.".to_string(),
                };
                let avail = ui.available_width();
                ui.vertical(|ui| {
                    ui.set_max_width((avail - 200.0).max(120.0));
                    ui.label(rt(summary, 12.0).color(text2()));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if !busy && th::button(ui, "Run check", true, th::BTN).clicked() {
                        run = true;
                    }
                });
            });
            if let Some(list) = &self.checks {
                ui.add_space(14.0);
                for ch in list {
                    ui.horizontal_top(|ui| {
                        let (txt, col, bg) = match ch.level {
                            Level::Ok => ("OK", th::SUCCESS, th::success_soft()),
                            Level::Tip => ("Tip", th::WARNING, th::warning_soft()),
                            Level::Warn => ("Check", th::WARNING, th::warning_soft()),
                        };
                        let (chip, _) = ui.allocate_exact_size(vec2(56.0, 20.0), Sense::hover());
                        ui.painter().rect_filled(chip, chip.height() / 2.0, bg);
                        ui.painter().text(chip.center(), egui::Align2::CENTER_CENTER, txt, FontId::new(12.0, th::semibold()), col);
                        ui.add_space(14.0);
                        let avail = ui.available_width();
                        ui.vertical(|ui| {
                            ui.set_max_width((avail - 150.0).max(120.0));
                            ui.label(sb(ch.title, 14.0));
                            ui.label(rt(&ch.message, 12.0).color(text2()));
                        });
                        if let Some(f) = ch.fix {
                            ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                                if th::button(ui, f.label(), false, th::BTN).clicked() {
                                    fix = Some(f);
                                }
                            });
                        }
                    });
                    ui.add_space(12.0);
                }
            }
        });
        if let Some(f) = fix {
            self.status = Some(match pccheck::run_fix(f, &self.engine) {
                Ok(()) => ("Done.".to_string(), true),
                Err(e) => (format!("{e}"), false),
            });
            self.refresh();
            run = true;
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

    // ------------------------------------------------------------ list pages

    fn tweak_list(&mut self, ui: &mut Ui) {
        let q = self.search.to_lowercase();
        let mut click = None;
        // Optimize is split into normal, risky (security risk) and experimental tweaks, in that order.
        let categorized = self.page == Page::Optimize;
        // Same order and group headings as C# `Layout.cs`.
        let mut order: Vec<usize> = (0..self.tweaks.len()).collect();
        order.sort_by_key(|&i| mytune_core::layout::placement(&self.tweaks[i].id).map_or(usize::MAX, |p| p.order));
        let mut last_group = "";
        for cat in 0..3 {
        let mut heading_done = false;
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
            if categorized && !heading_done {
                heading_done = true;
                th::section(ui, ["Normal tweaks", "Risky", "Experimental"][cat]);
            }
            if !categorized {
                let g = mytune_core::layout::placement(&t.id).map_or("", |p| p.group);
                if g != last_group {
                    last_group = g;
                    th::section(ui, g);
                }
            }
            let on = self.applied.get(&t.id).copied().unwrap_or(false);
            th::card(ui, (10, 16, 16, 16), |ui| {
                ui.horizontal(|ui| {
                    let avail = ui.available_width();
                    ui.vertical(|ui| {
                        ui.set_max_width((avail - 64.0).max(120.0));
                        ui.label(sb(&t.title, 14.0));
                        ui.label(rt(&t.description, 12.0).color(text2()));
                        th::flag_chips(ui, t.flags);
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if th::switch(ui, on).on_hover_text(&t.details).clicked() {
                            click = Some((i, !on));
                        }
                    });
                });
            });
            ui.add_space(8.0);
        }
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
        let mut revert = None;
        for id in &self.ids_in_store {
            let idx = self.tweaks.iter().position(|t| &t.id == id);
            th::card(ui, (10, 16, 16, 16), |ui| {
                ui.horizontal(|ui| {
                    ui.label(sb(idx.map(|i| self.tweaks[i].title.as_str()).unwrap_or(id), 14.0));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| match idx {
                        Some(i) => {
                            if th::button(ui, "Undo", false, th::BTN).clicked() {
                                revert = Some(i);
                            }
                        }
                        None => {
                            ui.label(rt("not available in this UI yet", 12.0).color(text3()));
                        }
                    });
                });
            });
            ui.add_space(8.0);
        }
        if let Some(i) = revert {
            self.set(i, false);
        }
    }
}

/// UniformGrid with 3 columns and 8 px gaps; cells share the tallest height (kept from the last frame).
fn grid3(ui: &mut Ui, mut f: impl FnMut(&mut Ui, usize)) {
    let w = (ui.available_width() - 16.0) / 3.0;
    let id = ui.id().with(("grid3", ui.next_auto_id()));
    let stored: f32 = ui.data(|d| d.get_temp(id)).unwrap_or(0.0);
    let mut tallest = 0.0f32;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        for i in 0..3 {
            th::set_min_card_height(stored);
            let r = ui.allocate_ui_with_layout(vec2(w, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(w);
                f(ui, i);
            });
            tallest = tallest.max(r.response.rect.height());
        }
    });
    th::set_min_card_height(0.0);
    if (tallest - stored).abs() > 0.5 {
        ui.data_mut(|d| d.insert_temp(id, tallest));
        ui.ctx().request_repaint();
    }
}

/// Holds the frame rate at the display's refresh rate. With V-Sync forced off in the graphics driver (common
/// on a gaming PC) every mouse move and every animation frame drew at once: 500 to 800 frames a second and
/// half a CPU core, measured 2026-11-15. With V-Sync working this never waits.
fn frame_cap() {
    use std::{sync::Mutex, time::{Duration, Instant}};
    // (start of the last frame, frame time, when the refresh rate was last read)
    static LAST: Mutex<Option<(Instant, Duration, Instant)>> = Mutex::new(None);
    let mut last = LAST.lock().unwrap();
    let now = Instant::now();
    let (prev, mut period, mut read) = last.unwrap_or((now - Duration::from_secs(1), Duration::ZERO, now - Duration::from_secs(60)));
    if now - read > Duration::from_secs(10) {
        period = Duration::from_secs_f64(1.0 / refresh_hz() as f64);
        read = now;
    }
    if let Some(wait) = period.checked_sub(now - prev) {
        std::thread::sleep(wait);
    }
    *last = Some((Instant::now(), period, read));
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
    for c in ctx.repaint_causes() {
        *s.2.entry(c.to_string()).or_default() += 1;
    }
    let now = ctx.input(|i| i.time);
    if now - s.0 >= 2.0 {
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{:.0}s frames={} {:?}", now, s.1, s.2);
        }
        *s = (now, 0, BTreeMap::new());
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.ui(ui);
    }
}
