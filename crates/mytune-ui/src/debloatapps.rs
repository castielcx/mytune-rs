//! Auto Debloat: the card at the top of the Debloat page and its review dialog. The user ticks apps, presses
//! Remove, confirms, and only then are they uninstalled (`mytune_core::apps`, on a thread, one at a time).

use crate::theme::{self as th, dsb, rt, text2, text3};
use crate::widgets as w;
use eframe::egui::{self, Align, Id, Layout, Ui};
use mytune_core::apps::{self, App};
use mytune_core::cleaner::fmt_size;
use mytune_core::tweaks::system_cmd;
use std::cell::RefCell;
use std::sync::mpsc::{self, Receiver, TryRecvError};

#[derive(Default, PartialEq, Clone, Copy)]
enum Step {
    #[default]
    Review,
    Confirm,
    Removing,
    Results,
}

type Outcome = (&'static str, Result<(), String>);

#[derive(Default)]
struct State {
    open: bool,
    step: Step,
    /// `None` while the installed apps are being read.
    list: Option<Result<Vec<App>, String>>,
    /// One tick per app in `list`.
    ticked: Vec<bool>,
    list_rx: Option<Receiver<Result<Vec<App>, String>>>,
    work_rx: Option<Receiver<Outcome>>,
    total: usize,
    results: Vec<Outcome>,
}

thread_local! {
    // only the UI thread draws the page, so the dialog's state lives here and app.rs needs one call
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// The Auto Debloat card; also draws the dialog while it is open.
pub fn ui(ui: &mut Ui) {
    STATE.with_borrow_mut(|s| s.ui(ui));
}

fn count(n: usize) -> String {
    format!("{n} app{}", if n == 1 { "" } else { "s" })
}

impl State {
    fn load(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(apps::list(&*system_cmd()));
            ctx.request_repaint();
        });
        self.list = None;
        self.list_rx = Some(rx);
    }

    fn remove(&mut self, ctx: &egui::Context) {
        let Some(Ok(list)) = &self.list else { return };
        let picks: Vec<(&'static str, String)> = list.iter().zip(&self.ticked).filter(|(_, t)| **t).map(|(a, _)| (a.title, a.full_name.clone())).collect();
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        self.total = picks.len();
        self.results.clear();
        self.work_rx = Some(rx);
        self.step = Step::Removing;
        std::thread::spawn(move || {
            let cmd = system_cmd();
            for (title, full_name) in picks {
                // a failure is reported and the next app still runs
                let _ = tx.send((title, apps::remove(&*cmd, &full_name)));
                ctx.request_repaint();
            }
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.list_rx {
            let got = match rx.try_recv() {
                Ok(r) => Some(r),
                Err(TryRecvError::Disconnected) => Some(Err("Could not read the installed apps.".to_string())),
                Err(TryRecvError::Empty) => None,
            };
            if let Some(r) = got {
                self.ticked = r.as_ref().map(|v| v.iter().map(App::suggested).collect()).unwrap_or_default();
                self.list = Some(r);
                self.list_rx = None;
            }
        }
        if let Some(rx) = &self.work_rx {
            let mut over = false;
            loop {
                match rx.try_recv() {
                    Ok(r) => self.results.push(r),
                    Err(TryRecvError::Disconnected) => {
                        over = true;
                        break;
                    }
                    Err(TryRecvError::Empty) => break,
                }
            }
            if over {
                self.work_rx = None;
                self.step = Step::Results;
                self.load(ctx); // what is still installed, for the next review
            }
        }
    }

    fn ui(&mut self, ui: &mut Ui) {
        let ctx = ui.ctx().clone();
        self.poll(&ctx);
        // same shape as the rows below: text left, control right
        th::card(ui, (10, 13, 14, 13), |ui| {
            ui.horizontal(|ui| {
                let avail = ui.available_width();
                ui.vertical(|ui| {
                    ui.set_max_width((avail - 150.0).max(120.0));
                    ui.label(th::md("Auto Debloat", 14.0));
                    ui.add_space(2.0);
                    ui.label(rt("Uninstalls the apps that came with Windows and that most people never open, like Clipchamp, Solitaire and the news apps. You choose which ones.", 12.5).color(text2()));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if th::button(ui, "Review apps", true, th::BTN).clicked() && !self.open {
                        self.open = true;
                        if self.step != Step::Removing {
                            self.step = Step::Review;
                            self.load(&ctx);
                        }
                    }
                });
            });
        });
        if !self.open {
            return;
        }
        let m = egui::Modal::new(Id::new("debloat-apps")).show(&ctx, |ui| {
            ui.set_width(520.0);
            match self.step {
                Step::Review => self.review(ui),
                Step::Confirm => self.confirm(ui),
                Step::Removing | Step::Results => self.progress(ui),
            }
        });
        // a click beside the dialog must not hide a removal that is running
        if m.should_close() && self.step != Step::Removing {
            self.open = false;
        }
    }

    fn review(&mut self, ui: &mut Ui) {
        ui.label(dsb("Remove preinstalled apps", 20.0));
        ui.add_space(4.0);
        ui.label(rt("Tick the apps to uninstall. Nothing is removed until you confirm.", 12.0).color(text2()));
        ui.add_space(10.0);
        let State { list, ticked, .. } = self;
        match list {
            None => {
                ui.label(rt("Looking for installed apps\u{2026}", 14.0).color(text3()));
            }
            Some(Err(e)) => {
                ui.label(rt(format!("Could not read the installed apps: {e}"), 14.0).color(text2()));
            }
            Some(Ok(apps)) if apps.is_empty() => {
                ui.label(rt("None of the usual preinstalled apps are installed. Nothing to remove.", 14.0).color(text2()));
            }
            Some(Ok(apps)) => {
                ui.horizontal(|ui| {
                    if th::subtle_button(ui, "Select suggested", (10.0, 5.0, 10.0, 5.0)).clicked() {
                        for (t, a) in ticked.iter_mut().zip(apps.iter()) {
                            *t = a.suggested();
                        }
                    }
                    if th::subtle_button(ui, "Clear", (10.0, 5.0, 10.0, 5.0)).clicked() {
                        ticked.fill(false);
                    }
                });
                ui.add_space(6.0);
                egui::ScrollArea::vertical().max_height(360.0).auto_shrink([false, true]).show(ui, |ui| {
                    let mut first = true;
                    for (label, suggested) in [("Suggested", true), ("Optional", false)] {
                        if !apps.iter().any(|a| a.suggested() == suggested) {
                            continue;
                        }
                        th::heading(ui, label, first);
                        first = false;
                        th::group(ui, |ui| {
                            for (i, a) in apps.iter().enumerate().filter(|(_, a)| a.suggested() == suggested) {
                                th::card(ui, (10, 10, 14, 10), |ui| {
                                    ui.horizontal(|ui| {
                                        let avail = ui.available_width();
                                        ui.vertical(|ui| {
                                            ui.set_max_width((avail - 90.0).max(120.0));
                                            w::checkbox(ui, Id::new(("debloat-app", &a.full_name)), &mut ticked[i], |ui| {
                                                ui.label(rt(a.title, 14.0));
                                                ui.label(rt(a.reason, 12.0).color(text3()));
                                                if !a.warning.is_empty() {
                                                    ui.label(rt(a.warning, 12.0).color(th::WARNING));
                                                }
                                            });
                                        });
                                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                            ui.label(rt(a.size.map(fmt_size).unwrap_or_default(), 13.0).color(text2()));
                                        });
                                    });
                                });
                            }
                        });
                    }
                });
            }
        }
        ui.add_space(12.0);
        let n = self.ticked.iter().filter(|t| **t).count();
        let mut next = false;
        ui.horizontal(|ui| {
            ui.add_enabled_ui(n > 0, |ui| {
                next = th::button(ui, &format!("Remove {}", count(n)), true, (18.0, 8.0, 18.0, 9.0)).clicked();
            });
            if th::button(ui, "Cancel", false, (14.0, 8.0, 14.0, 9.0)).clicked() {
                ui.close();
            }
        });
        if next {
            self.step = Step::Confirm;
        }
    }

    fn confirm(&mut self, ui: &mut Ui) {
        let n = self.ticked.iter().filter(|t| **t).count();
        ui.label(dsb(format!("Uninstall {}?", count(n)), 20.0));
        ui.add_space(4.0);
        ui.label(rt("These apps will be uninstalled. You can install them again from the Microsoft Store.", 12.0).color(text2()));
        ui.add_space(10.0);
        egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
            if let Some(Ok(apps)) = &self.list {
                for (a, _) in apps.iter().zip(&self.ticked).filter(|(_, t)| **t) {
                    ui.label(rt(format!("\u{2022}  {}", a.title), 14.0));
                    ui.add_space(4.0);
                }
            }
        });
        ui.add_space(10.0);
        let (mut go, mut back) = (false, false);
        ui.horizontal(|ui| {
            go = th::button(ui, "Uninstall", true, (18.0, 8.0, 18.0, 9.0)).clicked();
            back = th::button(ui, "Back", false, (14.0, 8.0, 14.0, 9.0)).clicked();
        });
        if go {
            self.remove(ui.ctx());
        } else if back {
            self.step = Step::Review;
        }
    }

    fn progress(&mut self, ui: &mut Ui) {
        let running = self.step == Step::Removing;
        let failed = self.results.iter().filter(|r| r.1.is_err()).count();
        ui.label(dsb(if running { "Removing apps\u{2026}" } else { "Done" }, 20.0));
        ui.add_space(4.0);
        let text = if running {
            format!("{} of {}", self.results.len(), self.total)
        } else if failed == 0 {
            format!("{} removed.", count(self.results.len()))
        } else {
            format!("{} removed, {} could not be removed.", count(self.results.len() - failed), failed)
        };
        ui.label(rt(text, 12.0).color(text2()));
        if running {
            ui.add_space(8.0);
            th::progress(ui, self.results.len() as f32 / self.total.max(1) as f32);
        }
        ui.add_space(10.0);
        egui::ScrollArea::vertical().max_height(300.0).auto_shrink([false, true]).show(ui, |ui| {
            for (title, r) in &self.results {
                ui.horizontal(|ui| {
                    ui.label(rt(*title, 14.0));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| match r {
                        Ok(()) => ui.label(rt("Removed", 12.0).color(th::SUCCESS)),
                        Err(_) => ui.label(rt("Not removed", 12.0).color(th::DANGER)),
                    });
                });
                if let Err(e) = r {
                    ui.label(rt(e, 12.0).color(text3()));
                }
                ui.add_space(6.0);
            }
        });
        if !running {
            ui.add_space(10.0);
            if th::button(ui, "Done", true, (18.0, 8.0, 18.0, 9.0)).clicked() {
                self.step = Step::Review;
            }
        }
    }
}
