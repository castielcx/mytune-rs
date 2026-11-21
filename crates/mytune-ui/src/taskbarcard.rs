//! Customize > Taskbar: the look of the Windows taskbar (mytune-core `taskbar`).

use crate::theme::{self as th, rt, sb, text2, TEXT};
use crate::widgets as w;
use eframe::egui::{self, vec2, Align, Layout, Ui};
use mytune_core::engine::Engine;
use mytune_core::taskbar::{self, Config, Look, Rule};
use std::sync::{Arc, Mutex};

pub struct TaskbarCard {
    engine: Arc<Engine>,
    /// What the taskbar thread shows; changed here at once, so the taskbar follows while dragging.
    live: Arc<Mutex<Config>>,
    saved: Config,
}

impl TaskbarCard {
    pub fn new(engine: Arc<Engine>) -> TaskbarCard {
        let saved = engine.with_store(|s| taskbar::load(s));
        let live = Arc::new(Mutex::new(saved.clone()));
        taskbar::set_outline_select(engine.with_store(|s| s.settings_flag("OutlineSelect")) == Some(true));
        let (e, l) = (engine.clone(), live.clone());
        taskbar::start(live.clone(), move || {
            let c = l.lock().unwrap_or_else(|p| p.into_inner()).clone();
            let _ = e.with_store(|s| taskbar::save(s, &c));
            // Explorer keeps dying after the load: this switch alone would load the DLL again.
            let _ = e.revert(&taskbar::OutlineSelectTweak);
            taskbar::set_outline_select(false);
        });
        // The tweak `cust.clearstart` (Customize list): its switch, and its way out when Start keeps dying.
        // It was a tick in this card first (`Settings.Taskbar.start_clear`): carried over once.
        let ticked = engine.with_store(|s| {
            s.settings_flag("ClearStart").is_none() && s.top("Settings").and_then(|v| v.get("Taskbar")).and_then(|t| t.get("start_clear")).and_then(|v| v.as_bool()) == Some(true)
        });
        if ticked {
            // this runs on the UI thread, and this tweak only sets a flag: the restore point need not finish first
            std::thread::spawn(|| mytune_core::sysreport::restore_point_once(&*mytune_core::tweaks::system_cmd()));
            let _ = engine.apply(&taskbar::ClearStartTweak);
        }
        taskbar::set_start_clear(engine.with_store(|s| s.settings_flag("ClearStart")) == Some(true));
        let e = engine.clone();
        taskbar::start_menu(move || drop(e.revert(&taskbar::ClearStartTweak)));
        TaskbarCard { engine, live, saved }
    }

    /// Windows' own volume icon is hidden from the tray (Sound page switch).
    pub fn hide_volume(&self) -> bool {
        self.live.lock().unwrap_or_else(|p| p.into_inner()).hide_volume
    }

    pub fn set_hide_volume(&mut self, on: bool) {
        let c = {
            let mut live = self.live.lock().unwrap_or_else(|p| p.into_inner());
            live.hide_volume = on;
            live.clone()
        };
        let _ = self.engine.with_store(|s| taskbar::save(s, &c));
        self.saved = c;
        std::thread::spawn(taskbar::clear_volume_icon_policy); // its broadcast can wait 1 s per hung window
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        let before = self.live.lock().unwrap_or_else(|p| p.into_inner()).clone();
        let mut c = before.clone();
        th::section(ui, "Taskbar");
        th::card(ui, (10, 16, 16, 16), |ui| {
            ui.label(sb("Taskbar look", 14.0));
            ui.label(rt("Makes the Windows taskbar clear, tinted or one colour.", 12.0).color(text2()))
                .on_hover_text("MyTune loads a small helper into Windows Explorer for any look but Normal. The normal taskbar comes back when every look is Normal or MyTune is closed.");
            {
                ui.add_space(10.0);
                // Desktop is always a rule; its box is ticked while its look is not Normal.
                let (mut own, mut toggled) = (c.desktop.look != Look::Normal, false);
                let label = |ui: &mut Ui| {
                    toggled = w::checkbox(ui, egui::Id::new("taskbar-own-desktop"), &mut own, |ui| drop(ui.label(rt("Desktop", 14.0).color(TEXT))));
                };
                row(ui, "desktop", label, (c.desktop.look != Look::Normal).then_some(&mut c.desktop));
                if toggled {
                    c.desktop.look = if own { Look::Clear } else { Look::Normal };
                }
                for (id, text, rule) in [("max", "Maximized window", &mut c.maximized)] {
                    let (mut own, mut toggled) = (rule.is_some(), false);
                    let label = |ui: &mut Ui| {
                        toggled = w::checkbox(ui, egui::Id::new(("taskbar-own", id)), &mut own, |ui| drop(ui.label(rt(text, 14.0).color(TEXT))));
                    };
                    row(ui, id, label, rule.as_mut());
                    if toggled {
                        *rule = own.then(Rule::default);
                    }
                }
            }
            if let Some(problem) = taskbar::problem() {
                ui.add_space(6.0);
                ui.label(rt(problem, 12.0).color(th::WARNING));
            }
        });
        if c != before {
            c.on = c.wanted(); // no switch: all Normal is off
            *self.live.lock().unwrap_or_else(|p| p.into_inner()) = c.clone();
        }
        // saved once the pointer is up, not on every slider step
        if c != self.saved && !ui.input(|i| i.pointer.any_down()) {
            let _ = self.engine.with_store(|s| taskbar::save(s, &c));
            self.saved = c;
        }
    }
}

/// One situation: label column 230 px, then look, colour and opacity.
fn row(ui: &mut Ui, id: &str, label: impl FnOnce(&mut Ui), rule: Option<&mut Rule>) {
    ui.horizontal(|ui| {
        ui.set_min_height(30.0);
        ui.allocate_ui_with_layout(vec2(230.0, 30.0), Layout::left_to_right(Align::Center), |ui| {
            ui.set_min_width(230.0);
            label(ui);
        });
        let Some(rule) = rule else { return };
        let looks: Vec<(String, bool)> = Look::ALL.iter().map(|l| (l.name().to_string(), *l == rule.look)).collect();
        if let Some(i) = w::dropdown_text(ui, &format!("taskbar-{id}"), rule.look.name(), &looks) {
            rule.look = Look::ALL[i];
        }
        if matches!(rule.look, Look::Tinted | Look::Solid) {
            ui.color_edit_button_srgb(&mut rule.color);
        }
        if rule.look == Look::Tinted {
            let mut v = rule.opacity as f64;
            if w::slider(ui, egui::Id::new(("taskbar-opacity", id)), &mut v, 0.0, 100.0, 120.0, true).changed {
                rule.opacity = v.round() as u8;
            }
            ui.label(rt(format!("{}%", rule.opacity), 12.0).color(text2()));
        }
    });
    ui.add_space(6.0);
}
