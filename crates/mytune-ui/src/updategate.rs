//! The forced update of the public setup (feature `license`): the screen `App::ui` shows in place of the sidebar
//! and the pages while `Pending::blocks` is set, and the title-bar banner once it is not.

use crate::theme::{self as th, rt, sb, text2};
use eframe::egui::{self, pos2, vec2, Align, Layout, Rect, Ui, UiBuilder};
use mytune_core::updater::{self, Pending, Phase, APP_VERSION};

fn percent(got: u64, total: Option<u64>) -> Option<u64> {
    total.filter(|t| *t > 0).map(|t| (got * 100 / t).min(100))
}

/// "42%", or "12 MB" when the server did not say how large the file is.
fn amount(got: u64, total: Option<u64>) -> String {
    percent(got, total).map_or_else(|| format!("{} MB", got >> 20), |p| format!("{p}%"))
}

/// True when **Revert all tweaks** was clicked.
pub fn ui(root: &mut Ui, p: &Pending, can_revert: bool) -> bool {
    let mut revert = false;
    egui::CentralPanel::default().frame(egui::Frame::new()).show(root, |ui| {
        let area = ui.max_rect();
        let height = if matches!(p.phase, Phase::Failed(_)) { 190.0 } else { 150.0_f32 };
        let card = Rect::from_center_size(area.center(), vec2(440.0_f32.min(area.width() - 32.0), height.min(area.height())));
        ui.scope_builder(UiBuilder::new().max_rect(card), |ui| {
            th::card(ui, (20, 18, 20, 18), |ui| {
                ui.set_width(ui.available_width());
                ui.label(sb("Update required", 17.0).color(th::TEXT));
                ui.add_space(6.0);
                ui.label(rt(format!("MyTune {} is ready. You have {APP_VERSION}.", p.info.version), 13.5).color(text2()));
                ui.add_space(14.0);
                match &p.phase {
                    Phase::Ready => {
                        if th::button(ui, "Update now", true, th::BTN).clicked() {
                            updater::install();
                        }
                    }
                    Phase::Downloading { got, total } => {
                        ui.label(rt(format!("Downloading... {}", amount(*got, *total)), 13.5).color(text2()));
                        if let Some(pct) = percent(*got, *total) {
                            ui.add_space(8.0);
                            th::progress(ui, pct as f32 / 100.0);
                        }
                    }
                    Phase::Installing => {
                        ui.label(rt("Installing. MyTune restarts by itself.", 13.5).color(text2()));
                    }
                    Phase::Failed(why) => {
                        ui.label(rt(why, 13.0).color(th::DANGER));
                        ui.add_space(14.0);
                        ui.horizontal_wrapped(|ui| {
                            if th::button(ui, "Retry", true, th::BTN).clicked() {
                                updater::install();
                            }
                            if th::button(ui, "Download manually", false, th::BTN).clicked() {
                                crate::diagnosticspage::open_url(&p.info.url);
                            }
                            ui.add_enabled_ui(can_revert, |ui| {
                                revert = th::button(ui, "Revert all tweaks", false, th::BTN).clicked();
                            });
                        });
                    }
                }
            });
        });
    });
    revert
}

/// A glass pill in the title bar, left of the caption buttons. It stays inside the bar, so no page moves.
pub fn banner(ui: &mut Ui, bar: Rect, p: &Pending) {
    const PAD: (f32, f32, f32, f32) = (10.0, 3.0, 10.0, 4.0);
    const RESTARTS: &str = "Restarts MyTune. A running call ends.";
    let s = 1.0_f32;
    let area = Rect::from_min_max(pos2(bar.left(), bar.top() + 6.0 * s), pos2(bar.right() - 46.0 * s * 3.0 - 8.0 * s, bar.bottom() - 6.0 * s));
    let plate = ui.painter().add(egui::Shape::Noop);
    let text = |t: String| egui::Label::new(rt(t, 13.0).color(text2())).selectable(false);
    let left = ui.scope_builder(UiBuilder::new().max_rect(area).layout(Layout::right_to_left(Align::Center)), |ui| {
        match &p.phase {
            Phase::Ready => {
                ui.add_space(4.0);
                if th::button(ui, "Update now", true, PAD).on_hover_text(RESTARTS).clicked() {
                    updater::install();
                }
                ui.add(text(format!("MyTune {} is ready", p.info.version)));
            }
            Phase::Downloading { got, total } => {
                ui.add_space(12.0);
                ui.add(text(format!("Downloading {}", amount(*got, *total))));
            }
            Phase::Installing => {
                ui.add_space(12.0);
                ui.add(text("Installing".into()));
            }
            Phase::Failed(why) => {
                ui.add_space(4.0);
                if th::button(ui, "Retry", true, PAD).on_hover_text(RESTARTS).clicked() {
                    updater::install();
                }
                ui.add(text("Update failed".into())).on_hover_text(why);
            }
        }
        ui.add_space(12.0);
        ui.min_rect().left()
    }).inner;
    let pill = Rect::from_min_max(pos2(left, area.top()), area.max);
    ui.painter().set(plate, th::glass(pill, pill.height() / 2.0, th::p().card_bg));
}
