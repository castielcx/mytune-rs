//! The stats panel beside Start and Search (`cust.searchstats`). `mytune_core::searchstats` watches Windows and
//! places the window; this keeps one hidden viewport and draws Home's cards in it.

use crate::theme::{self as th, md, rt, sb};
use eframe::egui::{self, vec2, Align, Layout, Rect, Ui, UiBuilder};
use mytune_core::{pccheck, searchstats as ss};
use mytune_sensors::{IoNow, IoSampler, Sensors};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::Thread;
use std::time::Duration;

/// What the panel shows; `None` is drawn as "--".
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Stats {
    pub cpu: Option<f32>,
    pub cpu_temp: Option<f32>,
    pub gpu: Option<f32>,
    pub gpu_temp: Option<f32>,
    pub ram: Option<f32>,
    pub ram_gb: Option<f64>,
    pub io: IoNow,
}

/// Disk and network counters, on their own thread and only while the panel is open.
struct IoThread {
    run: Arc<AtomicBool>,
    now: Arc<Mutex<IoNow>>,
    thread: Thread,
}

impl IoThread {
    fn start() -> Option<IoThread> {
        let (run, now) = (Arc::new(AtomicBool::new(true)), Arc::new(Mutex::new(IoNow::default())));
        let (r, n) = (run.clone(), now.clone());
        let handle = std::thread::Builder::new()
            .name("search-stats-io".into())
            .spawn(move || {
                let Some(s) = IoSampler::new() else { return };
                while r.load(Ordering::Relaxed) {
                    std::thread::park_timeout(Duration::from_secs(1));
                    if r.load(Ordering::Relaxed) {
                        *n.lock().unwrap_or_else(|p| p.into_inner()) = s.read();
                    }
                }
            })
            .ok()?;
        Some(IoThread { run, now, thread: handle.thread().clone() })
    }
}

impl Drop for IoThread {
    fn drop(&mut self) {
        self.run.store(false, Ordering::Relaxed);
        self.thread.unpark();
    }
}

#[derive(Default)]
pub struct StatsPanel {
    io: Option<IoThread>,
    was_open: bool,
    /// The window is on screen as far as eframe knows, so it keeps running frames with the main window hidden.
    live: bool,
    wake_set: bool,
}

impl StatsPanel {
    /// While the main window is hidden eframe skips `App::ui`, so `App::logic` asks for a frame until this is false.
    pub fn needs_frame(&self) -> bool {
        ss::is_open() && !self.live
    }

    /// Call early in every frame. True when this frame exists only for the panel, if the main window is hidden.
    pub fn show(&mut self, ctx: &egui::Context, sensors: Option<&Sensors>) -> bool {
        if !self.wake_set {
            let c = ctx.clone();
            ss::set_wake(move || c.request_repaint());
            self.wake_set = true;
        }
        let open = ss::is_open();
        let changed = open != self.was_open;
        self.was_open = open;
        if changed {
            if let Some(s) = sensors {
                s.set_watched(open); // the page's own rule takes over in the next full frame
            }
            self.io = if open { IoThread::start() } else { None };
        }
        if !open {
            self.live = false;
        }
        // Tests embed viewports: there is no window to keep.
        if !ss::enabled() || ctx.embed_viewports() {
            return open || changed;
        }
        let stats = if open { read(self.io.as_ref(), sensors) } else { Stats::default() };
        let builder = egui::ViewportBuilder::default()
            .with_title(ss::TITLE)
            .with_decorations(false)
            .with_resizable(false)
            .with_taskbar(false)
            .with_always_on_top()
            .with_active(false)
            .with_mouse_passthrough(true)
            .with_inner_size(vec2(ss::WIDTH as f32, 600.0))
            .with_visible(false); // `searchstats` shows and hides it by handle, without a frame and without focus
        let mut live = false;
        ctx.show_viewport_immediate(egui::ViewportId::from_hash_of("search-stats"), builder, |ui, _| {
            ss::adopt_panel();
            live = ui.ctx().input(|i| i.viewport().visible()) == Some(true);
            if open {
                th::glow_textures(ui.ctx()); // this window's pixel grid; `App::ui` sets the main window's back after it
                panel(ui, &stats);
            }
        });
        self.live = open && live;
        if open {
            ctx.request_repaint_after(Duration::from_secs(1));
        }
        open || changed
    }
}

fn read(io: Option<&IoThread>, sensors: Option<&Sensors>) -> Stats {
    let snap = sensors.and_then(Sensors::latest).unwrap_or_default();
    let mem = pccheck::memory_now();
    Stats {
        cpu: snap.cpu_load,
        cpu_temp: snap.cpu_temp,
        gpu: snap.gpu_load,
        gpu_temp: snap.gpu_temp,
        ram: mem.map(|m| m.3 as f32).or(snap.ram_load),
        ram_gb: mem.map(|m| m.0).or(snap.ram_used_gb.map(f64::from)),
        io: io.map(|t| *t.now.lock().unwrap_or_else(|p| p.into_inner())).unwrap_or_default(),
    }
}

const PAD: (i8, i8, i8, i8) = (10, 8, 10, 8);

/// The panel's whole content: the page backdrop and five of Home's cards, spread over the height.
pub fn panel(ui: &mut Ui, s: &Stats) {
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0.0, th::tone(0.10)); // under the backdrop; the Black accent has no backdrop
    th::backdrop(ui.ctx(), ui.painter(), full);
    let area = full.shrink(th::GAP);
    let h = ((area.height() - 4.0 * th::GAP) / 5.0).floor();
    let slot = |i: usize| Rect::from_min_size(area.min + vec2(0.0, i as f32 * (h + th::GAP)), vec2(area.width(), h));
    let meter = |ui: &mut Ui, i: usize, label: &str, side: &str, big: String, right: String, v: Option<f32>| {
        card(ui, slot(i), |ui| {
            pair(ui, rt(label, 11.0).color(th::text2()), rt(side, 11.0).color(th::text2()));
            ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                th::progress(ui, v.filter(|x| x.is_finite()).unwrap_or(0.0) / 100.0);
                ui.add_space(5.0);
                pair(ui, sb(big, 19.0).color(th::TEXT), md(right, 12.0).color(th::TEXT));
            });
        });
    };
    meter(ui, 0, "CPU", "Temp", ss::percent(s.cpu), ss::temp(s.cpu_temp), s.cpu);
    meter(ui, 1, "GPU", "Temp", ss::percent(s.gpu), ss::temp(s.gpu_temp), s.gpu);
    meter(ui, 2, "RAM", "Used", ss::percent(s.ram), ss::gb(s.ram_gb), s.ram);
    meter(ui, 3, "Disk", "Active", ss::percent(s.io.disk_active), ss::speed(s.io.disk_bytes), s.io.disk_active);
    card(ui, slot(4), |ui| {
        ui.label(rt("Network", 11.0).color(th::text2()));
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            for (name, v) in [("Up", s.io.up_bytes), ("Down", s.io.down_bytes)] {
                pair(ui, rt(name, 12.0).color(th::text2()), md(ss::speed(v), 12.0).color(th::TEXT));
            }
        });
    });
}

fn card(ui: &mut Ui, rect: Rect, add: impl FnOnce(&mut Ui)) {
    ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
        th::card(ui, PAD, |ui| {
            ui.set_width(rect.width() - (PAD.0 + PAD.2) as f32);
            ui.set_height(rect.height() - (PAD.1 + PAD.3) as f32);
            ui.spacing_mut().item_spacing.y = 2.0;
            add(ui);
        });
    });
}

fn pair(ui: &mut Ui, left: egui::RichText, right: egui::RichText) {
    ui.horizontal(|ui| {
        ui.label(left);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(right));
    });
}
