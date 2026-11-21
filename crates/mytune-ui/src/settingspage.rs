//! Settings page, laid out like `Views/SettingsPage.xaml`: App, While gaming, Updates, Appearance, Backup,
//! Help, Game booster, About. Not here yet: Profiles and the GPU overclock card.
//!
//! The toggles write `state.json` like the C# app. The game booster itself is not hosted in this window (run
//! `cargo run --release --example booster_service`, or the C# app), so these switches only take effect there.

use crate::theme::{self as th, rt, text2, TEXT};
use eframe::egui::{self, Align, Color32, Layout, Rect, Sense, Stroke, StrokeKind, Ui};
use mytune_core::autostart;
use mytune_core::alltweaks::{registry_items, TweakItem};
use mytune_core::engine::{Engine, Tweak};
use mytune_core::netrepair::{net_tweaks, NetTweak};
use mytune_core::sysreport;
use mytune_core::tweaks::system_cmd;
#[cfg(feature = "license")]
use mytune_core::updater;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

const ACCENTS: [(&str, &str); 10] = [
    ("Orange", "#FF5A0A"), ("Red", "#F23B1F"), ("Amber", "#FF9A00"), ("Yellow", "#FFC800"), ("Pink", "#FF4F81"),
    ("Lime", "#B6E02D"), ("Cyan", "#22D3EE"), ("Violet", "#A78BFA"), ("White", "#F2F2F2"), ("Black", "#000000"),
];

enum Msg {
    Status(String, bool),
    Autostart(bool),
    Copy(String),
}

#[derive(Clone, Copy)]
enum Report {
    Save,
    Copy,
    Support,
}

pub struct SettingsPage {
    engine: Arc<Engine>,
    tweaks: Arc<Vec<TweakItem>>,
    net: Arc<Vec<NetTweak>>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    pub(crate) status: Option<(String, bool)>,
    started: bool,
    autostart: Option<bool>,
    autostart_busy: bool,
}

impl SettingsPage {
    pub fn new(engine: Arc<Engine>) -> SettingsPage {
        let (tx, rx) = channel();
        SettingsPage {
            engine,
            tweaks: Arc::new(registry_items().unwrap_or_default()),
            net: Arc::new(net_tweaks(system_cmd()).unwrap_or_default()),
            tx,
            rx,
            status: None,
            started: false,
            autostart: None,
            autostart_busy: false,
        }
    }

    fn flag(&self, name: &str, default: bool) -> bool {
        self.engine.with_store(|s| s.settings_flag(name)).unwrap_or(default)
    }

    /// `title` is the switch's label, shown in the status bar.
    fn set_flag(&mut self, name: &str, title: &str, value: bool) {
        let r = self.engine.with_store(|s| s.set_settings_flag(name, value)).map_err(|e| e.to_string());
        self.status = Some(crate::notice::toggled(title, value, r.err().as_deref()));
    }

    fn spawn(&self, ctx: &egui::Context, f: impl FnOnce(&Sender<Msg>) + Send + 'static) {
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            f(&tx);
            ctx.request_repaint();
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Status(t, ok) => self.status = Some((t, ok)),
                Msg::Autostart(on) => {
                    self.autostart = Some(on);
                    self.autostart_busy = false;
                }
                Msg::Copy(text) => ctx.copy_text(text),
            }
        }
    }

    /// `"ON "`, `"off"` or `"?  "` for the report; only registry and network tweaks have objects here.
    fn states(tweaks: &[TweakItem], net: &[NetTweak]) -> HashMap<String, String> {
        let mut m = HashMap::new();
        let s = |r: mytune_core::Result<bool>| match r {
            Ok(true) => "ON ",
            Ok(false) => "off",
            Err(_) => "?  ",
        };
        for t in tweaks.iter().filter(|t| !t.removed) {
            m.insert(t.id.clone(), s(t.is_applied()).to_string());
        }
        for n in net {
            m.insert(n.tweak.id().to_string(), s(n.tweak.is_applied()).to_string());
        }
        m
    }

    fn report(&self, ctx: &egui::Context, what: Report) {
        let (tweaks, net, eng) = (self.tweaks.clone(), self.net.clone(), self.engine.clone());
        self.spawn(ctx, move |tx| {
            let say = |t: &str, ok: bool| {
                let _ = tx.send(Msg::Status(t.to_string(), ok));
            };
            let path = match what {
                Report::Save => {
                    let day = mytune_core::stability::iso(mytune_core::stability::now());
                    let Some(p) = rfd::FileDialog::new().set_title("Save system report").add_filter("Text file", &["txt"]).set_file_name(format!("mytune-report-{}.txt", &day[..10])).save_file() else { return };
                    Some(p)
                }
                Report::Support => {
                    let desktop = PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join("Desktop");
                    Some(desktop.join("MyTune-support.txt"))
                }
                Report::Copy => None,
            };
            say(if matches!(what, Report::Support) { "Building the support file, this takes a few seconds…" } else { "Building the report, this takes a few seconds…" }, true);
            let map = SettingsPage::states(&tweaks, &net);
            let states = |id: &str| map.get(id).cloned().unwrap_or_else(|| "?  ".to_string());
            let cmd = system_cmd();
            let text = if matches!(what, Report::Support) { eng.with_store(|s| sysreport::build_support(&*cmd, s, &states)) } else { sysreport::build(&*cmd, &states) };
            match (what, path) {
                (Report::Copy, _) => {
                    let _ = tx.send(Msg::Copy(text));
                    say("Report copied. Paste it into your message.", true);
                }
                (Report::Save, Some(p)) => match std::fs::write(&p, text) {
                    Ok(()) => say("Report saved.", true),
                    Err(e) => say(&format!("Couldn't save the report: {e}"), false),
                },
                (Report::Support, Some(p)) => match std::fs::write(&p, text) {
                    Ok(()) => {
                        let _ = std::process::Command::new("explorer.exe").arg(format!("/select,{}", p.display())).spawn();
                        say("Saved MyTune-support.txt on your desktop. Send that file.", true);
                    }
                    Err(e) => say(&format!("Couldn't save the support file: {e}"), false),
                },
                _ => {}
            }
        });
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        if !self.started {
            self.started = true;
            self.spawn(&ctx, |tx| {
                let _ = tx.send(Msg::Autostart(autostart::is_enabled()));
            });
        }
        self.poll(&ctx);
        if let Some((m, ok)) = self.status.take() {
            crate::notice::post(&ctx, m, ok);
        }

        heading(ui, "App", 24.0);
        let on = self.autostart.unwrap_or(false);
        let busy = self.autostart_busy || self.autostart.is_none();
        if row(ui, "Start with Windows", "Launches MyTune hidden in the tray when you sign in, without an admin prompt.", 80.0, |ui| {
            ui.add_enabled_ui(!busy, |ui| th::switch(ui, on).clicked()).inner
        }) && !busy
        {
            self.autostart_busy = true;
            let want = !on;
            self.spawn(&ctx, move |tx| {
                let ok = if want { autostart::enable() } else { autostart::disable() };
                let _ = tx.send(Msg::Autostart(if ok { want } else { !want }));
                let (t, good) = crate::notice::toggled("Start with Windows", want, (!ok).then_some("see the log"));
                let _ = tx.send(Msg::Status(t, good));
            });
        }

        heading(ui, "While gaming", 20.0);
        for (key, default, title, desc) in [
            ("QuietDuringGames", false, "Quiet mode", "Hides Windows pop-up notifications while a Game Hub game is running and brings them back afterwards."),
            ("AutoPowerPlan", false, "Performance power plan only while gaming", "Switches to the performance plan when a Game Hub game starts and back to your previous plan when it closes. Saves power and heat the rest of the time."),
            ("HeatWarning", true, "Heat warning", "Shows a tray notice when the CPU (95 °C) or GPU (88 °C) stays that hot while you play."),
        ] {
            let v = self.flag(key, default);
            if row(ui, title, desc, 80.0, |ui| th::switch(ui, v).clicked()) {
                self.set_flag(key, title, !v);
            }
        }

        #[cfg(feature = "license")]
        {
            heading(ui, "Updates", 20.0);
            let mut check = false;
            row(ui, "Check for updates", "MyTune looks for a new version when it starts and once a day. Nothing about you is sent.", 260.0, |ui| {
                check = th::button(ui, "Check now", false, th::BTN).clicked();
                false
            });
            if check {
                self.status = Some(("Checking…".into(), true));
                self.spawn(&ctx, |tx| {
                    let (t, ok) = match updater::check() {
                        Ok(None) => ("You're on the latest version.".to_string(), true),
                        Ok(Some(u)) => (format!("Version {} is available. Use Update now at the top of the window.", u.version), true),
                        Err(e) => (format!("Couldn't check for updates: {e}"), false),
                    };
                    let _ = tx.send(Msg::Status(t, ok));
                });
            }
        }

        heading(ui, "Appearance", 20.0);
        let mut accent = None;
        row(ui, "Accent color", "Used for buttons, toggles, sliders and the sidebar marker.", 330.0, |ui| {
            for (name, hex) in ACCENTS.iter().rev() {
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), Sense::click());
                let c = Color32::from_hex(hex).unwrap_or(Color32::WHITE);
                ui.painter().rect_filled(rect, 10.0, c);
                ui.painter().rect_stroke(rect, 10.0, Stroke::new(1.0, Color32::from_white_alpha(0x40)), StrokeKind::Inside);
                if c == Color32::BLACK && th::p().black {
                    // the one ring left in the Black look: without it this swatch cannot be seen on the page
                    ui.painter().circle_stroke(rect.center(), 10.5, Stroke::new(1.0, Color32::from_white_alpha(0x90)));
                }
                ui.add_space(0.0);
                if resp.on_hover_text(*name).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    accent = Some(*hex);
                }
            }
            false
        });
        if let Some(hex) = accent {
            let was_black = th::p().black;
            th::set_accent_value(hex);
            if th::p().black != was_black {
                th::apply_visuals(&ctx); // the panel and selection colours follow at once, not after a restart
            }
            let r = self.engine.with_store(|s| s.set_settings_str("AccentColor", hex));
            if let Err(e) = r {
                self.status = Some((format!("Couldn't save the accent: {e}"), false));
            }
        }

        heading(ui, "Backup", 20.0);
        let mut restore = false;
        row(ui, "Windows restore point", "Creates a restore point now, in case you want to roll Windows back. MyTune also makes one once per run, before the first tweak you turn on from a tweak page.", 260.0, |ui| {
            restore = th::button(ui, "Create now", false, th::BTN).clicked();
            false
        });
        if restore {
            self.status = Some(("Creating a restore point…".into(), true));
            self.spawn(&ctx, |tx| {
                let r = sysreport::create_restore_point(&*system_cmd(), "MyTune manual restore point");
                let _ = tx.send(match r {
                    None => Msg::Status("Restore point created.".into(), true),
                    Some(e) => Msg::Status(format!("Couldn't create a restore point: {e}"), false),
                });
            });
        }

        heading(ui, "Help", 20.0);
        let mut rep = None;
        row(ui, "System report", "Your hardware, Windows build, main drivers and which tweaks are on, as plain text for asking for help. No username, serial numbers, IP addresses or file paths.", 330.0, |ui| {
            if th::button(ui, "Copy", false, th::BTN).clicked() {
                rep = Some(Report::Copy);
            }
            if th::button(ui, "Save…", false, th::BTN).clicked() {
                rep = Some(Report::Save);
            }
            if th::button(ui, "Support file", false, th::BTN).on_hover_text("Saves MyTune-support.txt on your desktop: the report plus network adapters and the last 150 log lines, for when something doesn't work.").clicked() {
                rep = Some(Report::Support);
            }
            false
        });
        if let Some(r) = rep {
            self.report(&ctx, r);
        }

        heading(ui, "Game booster", 20.0);
        let thr = self.flag("ThrottleBackground", false);
        if row(
            ui,
            "Lower background programs while gaming",
            "While a Game Hub game runs, browsers, game launchers and updaters get low priority (and efficiency mode where Windows allows it) so they stay out of the game's way. Nothing is closed or paused, and everything goes back when the game exits. Steam, Discord and the Riot client are left alone.",
            80.0,
            |ui| th::switch(ui, thr).clicked(),
        ) {
            self.set_flag("ThrottleBackground", "Lower background programs while gaming", !thr);
        }
    }
}

/// Headings are hidden (user's choice) and add no space: the rows' own `th::GAP` is the gap between groups too,
/// so every box on every page sits the same distance apart.
fn heading(_ui: &mut Ui, _text: &str, _top: f32) {}

/// One settings card: title and caption left (leaving `right_margin` px), controls right. `controls`
/// returns true when the thing it drew was clicked (the first control's click is what `row` reports).
fn row(ui: &mut Ui, title: &str, desc: &str, right_margin: f32, controls: impl FnOnce(&mut Ui) -> bool) -> bool {
    let mut clicked = false;
    th::card(ui, (12, 14, 18, 14), |ui| {
        ui.horizontal(|ui| {
            let avail = ui.available_width();
            ui.vertical(|ui| {
                ui.set_max_width((avail - right_margin).max(120.0));
                ui.label(rt(title, 14.0).color(TEXT));
                ui.add_space(2.0);
                ui.label(rt(desc, 12.0).color(text2()));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                clicked = controls(ui);
            });
        });
    });
    ui.add_space(th::GAP);
    clicked
}

#[allow(dead_code)]
fn _unused(_: Rect) {}
