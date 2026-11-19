//! The licence screen of the public setup (feature `license`): shown by `App::ui` in place of the sidebar
//! and the pages until `mytune_core::license` says Unlocked.

use crate::theme::{self as th, rt, sb, text2, text3};
use eframe::egui::{self, vec2, Rect, Ui, UiBuilder};
use mytune_core::license::{self, Reject, Status};
use std::time::Duration;

thread_local! {
    /// The key being typed (UI thread only).
    static KEY: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

pub fn ui(root: &mut Ui) {
    let status = license::status();
    egui::CentralPanel::default().frame(egui::Frame::new()).show(root, |ui| {
        let area = ui.max_rect();
        let card = Rect::from_center_size(area.center(), vec2(400.0_f32.min(area.width() - 32.0), 230.0_f32.min(area.height())));
        ui.scope_builder(UiBuilder::new().max_rect(card), |ui| {
            th::card(ui, (20, 18, 20, 18), |ui| {
                ui.set_width(ui.available_width());
                let Status::Locked(why) = status else {
                    ui.label(rt("Checking licence\u{2026}", 14.0).color(text2()));
                    ui.ctx().request_repaint_after(Duration::from_millis(200)); // the answer comes from a thread
                    return;
                };
                ui.label(sb("Activate MyTune", 17.0).color(th::TEXT));
                ui.add_space(6.0);
                ui.label(rt("Enter your licence key. It works on one PC.", 13.5).color(text2()));
                ui.add_space(12.0);
                let mut key = KEY.with(|k| k.borrow().clone());
                let field = ui.add(egui::TextEdit::singleline(&mut key).desired_width(f32::INFINITY).char_limit(40).hint_text(rt("XXXXX-XXXXX-XXXXX-XXXXX", 13.5).color(text3())));
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if let Some(why) = why {
                    ui.add_space(8.0);
                    let text = match why {
                        Reject::Invalid => "This key is wrong. Check it and try again.",
                        Reject::Revoked => "This key was switched off.",
                        Reject::Bound => "This key is used on another PC.",
                        Reject::Network => "No connection to the licence server. Check your internet and try again.",
                    };
                    ui.label(rt(text, 13.0).color(th::DANGER));
                }
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    let go = th::button(ui, "Activate", true, th::BTN).clicked() || enter;
                    if go {
                        // an empty field retries the saved key (a PC that was offline need not retype it)
                        if !key.trim().is_empty() {
                            license::submit(key.clone());
                        } else if let Some(saved) = license::cached_key() {
                            license::submit(saved);
                        }
                    }
                    // the owner asks for this to tell the PCs of one buyer apart
                    ui.label(rt(format!("This PC: {}", &license::device_id()[..8]), 12.0).color(text3()));
                });
                KEY.with(|k| *k.borrow_mut() = key);
            });
        });
    });
}
