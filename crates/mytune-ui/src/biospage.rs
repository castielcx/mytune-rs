//! BIOS page: what Windows can see of the BIOS settings, with a verdict per setting. Reads only; the one
//! action is restarting into the BIOS after a confirm.

use crate::theme::{self as th, dsb, rt, text2, text3};
use eframe::egui::{self, Align, Id, Layout, Ui};
use mytune_core::bios::{self, Level, Report};
use std::sync::mpsc;

#[derive(Default)]
pub struct BiosPage {
    report: Option<Report>,
    rx: Option<mpsc::Receiver<Report>>,
    confirm: bool,
    error: Option<String>,
}

impl BiosPage {
    fn start(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(bios::read());
            ctx.request_repaint();
        });
        self.rx = Some(rx);
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        if self.report.is_none() && self.rx.is_none() {
            self.start(ui.ctx());
        }
        if let Some(r) = self.rx.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.report = Some(r);
            self.rx = None;
        }
        // Same shape as the Diagnostics cards: text left, buttons right, rows below, chip on the right.
        let rep = self.report.clone();
        let (mut restart, mut again) = (false, false);
        th::card(ui, (10, 16, 16, 16), |ui| {
            ui.horizontal(|ui| {
                let avail = ui.available_width();
                ui.vertical(|ui| {
                    ui.set_max_width((avail - 290.0).max(120.0));
                    ui.label(rt("MyTune cannot change BIOS settings. This page shows what to change and why. Before you change anything, save your BIOS settings as a profile and have your BitLocker recovery key ready: Windows may ask for it once.", 12.0).color(text2()));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| match &rep {
                    None => {
                        ui.label(rt("Reading\u{2026}", 12.0).color(text3()));
                    }
                    Some(rep) => {
                        again = th::button(ui, "Check again", false, th::BTN).clicked();
                        restart = rep.uefi && th::button(ui, "Restart into BIOS", true, th::BTN).clicked();
                    }
                });
            });
            if let Some(rep) = &rep {
                ui.add_space(12.0);
                ui.add(egui::Label::new(rt(if rep.board.is_empty() { "Unknown board" } else { &rep.board }, 14.0)).wrap());
                ui.label(rt(format!("BIOS version {}, dated {}", rep.bios_version, rep.bios_date), 12.0).color(text3()));
            }
            if let Some(e) = &self.error {
                ui.add_space(8.0);
                ui.label(rt(e, 12.0).color(th::WARNING));
            }
        });
        if let Some(rep) = &rep {
            ui.add_space(16.0);
            th::card(ui, (10, 16, 16, 6), |ui| {
                for r in &rep.rows {
                    ui.horizontal(|ui| {
                        let avail = ui.available_width();
                        ui.vertical(|ui| {
                            ui.set_max_width((avail - 120.0).max(120.0));
                            ui.label(rt(r.title, 14.0));
                            ui.add(egui::Label::new(rt(&r.message, 12.0).color(text2())).wrap());
                            if !r.where_.is_empty() {
                                ui.add(egui::Label::new(rt(r.where_, 12.0).color(text3())).wrap());
                            }
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let (txt, fg, bg) = match r.level {
                                Level::Ok => ("Fine", th::SUCCESS, th::success_soft()),
                                Level::Change => ("Change", th::WARNING, th::warning_soft()),
                                Level::Choice => ("Your choice", text2(), th::control_bg()),
                                Level::Info => ("Info", text2(), th::control_bg()),
                            };
                            th::chip(ui, txt, fg, bg);
                        });
                    });
                    ui.add_space(10.0);
                }
            });
        }
        if restart {
            self.confirm = true;
            self.error = None;
        }
        if again {
            self.report = None;
            self.start(ui.ctx());
        }
        if self.confirm {
            self.confirm_modal(ui.ctx());
        }
    }

    fn confirm_modal(&mut self, ctx: &egui::Context) {
        let m = egui::Modal::new(Id::new("bios-restart")).show(ctx, |ui| {
            ui.set_width(460.0);
            ui.label(dsb("Restart now and open the BIOS?", 20.0));
            ui.add_space(4.0);
            ui.label(rt("Save your work first: every open program closes.", 12.0).color(text2()));
            ui.add_space(10.0);
            let mut go = false;
            ui.horizontal(|ui| {
                go = th::button(ui, "Restart", true, (18.0, 8.0, 18.0, 9.0)).clicked();
                if th::button(ui, "Cancel", false, (14.0, 8.0, 14.0, 9.0)).clicked() {
                    ui.close();
                }
            });
            go
        });
        if m.inner {
            self.confirm = false;
            if let Err(e) = bios::restart_to_firmware() {
                self.error = Some(format!("Could not restart into the BIOS: {e}"));
            }
        } else if m.should_close() {
            self.confirm = false;
        }
    }
}
