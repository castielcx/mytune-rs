//! Discord threads: the chip under a message that started one, the Threads list from the conversation bar, and
//! the New Thread form (from a message's menu or the list's Create button).

use super::header::{popover, HeaderUi};
use super::voice::{k_edge, k_raised};
use super::*;
use mytune_chat::yapfast_core::application::{Application, ThreadFrom};
use mytune_chat::yapfast_core::chat::{ChatMessage, ListedThread};

/// "Thread name, N messages" under the message that started one; a click opens it.
pub fn chip(ui: &mut Ui, m: &ChatMessage, acts: &mut Vec<Action>) {
    let Some(t) = &m.thread else { return };
    ui.add_space(4.0);
    let text = match t.messages {
        Some(n) => format!("{}  \u{B7}  {n} message{}", t.name, if n == 1 { "" } else { "s" }),
        None => t.name.clone(),
    };
    let w = ui.painter().layout_no_wrap(text.clone(), egui::FontId::new(13.0, th::semibold()), TEXT).size().x;
    let (r, resp) = ui.allocate_exact_size(vec2((w + 60.0).min(420.0), 32.0), Sense::click());
    let hov = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), voice::QUICK);
    ui.painter().rect(r, 8.0, k_raised().lerp_to_gamma(k_active(), hov), egui::Stroke::new(1.0, k_edge()), egui::StrokeKind::Inside);
    voice::glyph(ui.painter(), egui::pos2(r.left() + 18.0, r.center().y), "\u{E8BD}", 14.0, SECONDARY, false);
    let clip = ui.painter().with_clip_rect(egui::Rect::from_min_max(egui::pos2(r.left() + 34.0, r.top()), egui::pos2(r.right() - 26.0, r.bottom())));
    clip.text(egui::pos2(r.left() + 34.0, r.center().y), egui::Align2::LEFT_CENTER, text, egui::FontId::new(13.0, th::semibold()), TEXT);
    voice::glyph(ui.painter(), egui::pos2(r.right() - 14.0, r.center().y), "\u{E76C}", 11.0, MUTED, false);
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        acts.push(Action::Open(t.room.clone()));
    }
}

fn row(ui: &mut Ui, t: &ListedThread, acts: &mut Vec<Action>) {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, 12.0, k_active());
    }
    voice::glyph(ui.painter(), egui::pos2(r.left() + 16.0, r.center().y), if t.private { "\u{E72E}" } else { "\u{E8BD}" }, 14.0, SECONDARY, false);
    let clip = ui.painter().with_clip_rect(egui::Rect::from_min_max(egui::pos2(r.left() + 32.0, r.top()), egui::pos2(r.right() - 70.0, r.bottom())));
    clip.text(egui::pos2(r.left() + 32.0, r.center().y - 8.0), egui::Align2::LEFT_CENTER, &t.name, egui::FontId::new(13.5, th::semibold()), crate::theme::hover_text(TEXT, resp.hovered()));
    let sub = if t.archived { "Archived".to_owned() } else { format!("{} member{}", t.members, if t.members == 1 { "" } else { "s" }) };
    clip.text(egui::pos2(r.left() + 32.0, r.center().y + 10.0), egui::Align2::LEFT_CENTER, sub, egui::FontId::proportional(12.0), MUTED);
    // Join / Leave at the row's right, as Discord's thread browser has them
    if !t.archived {
        let b = egui::Rect::from_center_size(egui::pos2(r.right() - 34.0, r.center().y), vec2(56.0, 26.0));
        let jr = ui.interact(b, resp.id.with("join"), Sense::click());
        ui.painter().rect_filled(b, 10.0, if jr.hovered() { k_hover() } else { k_raised() });
        ui.painter().text(b.center(), egui::Align2::CENTER_CENTER, if t.joined { "Leave" } else { "Join" }, egui::FontId::new(12.5, th::semibold()), if t.joined { SECONDARY } else { TEXT });
        if jr.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            acts.push(Action::JoinThread { room: t.room.clone(), join: !t.joined, location: "Thread Browser" });
            return;
        }
    }
    if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        acts.push(Action::Open(t.room.clone()));
        acts.push(Action::ShowThreads(false));
    }
}

/// The Threads popover under its button in the conversation bar.
pub fn list(ctx: &egui::Context, h: &HeaderUi, app: &Application, acts: &mut Vec<Action>) {
    let s = &app.state;
    let Some(tl) = s.thread_list() else { return };
    let Some(sec) = app.thread_sections() else { return };
    let close = popover(ctx, "chat_threads", h.thread_button, 400.0, |ui| {
        ui.horizontal(|ui| {
            ui.label(sb("Threads", 15.0).color(TEXT));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if pill(ui, "Create", true).clicked() {
                    acts.push(Action::NewThread { message: None, from: ThreadFrom::List });
                    acts.push(Action::ShowThreads(false));
                }
            });
        });
        ui.add_space(8.0);
        // search the threads by name (the engine asks Discord); empty shows the list again
        let qid = egui::Id::new("thread_search");
        let mut q: String = ui.ctx().data(|d| d.get_temp(qid)).unwrap_or_default();
        if ui.add(egui::TextEdit::singleline(&mut q).desired_width(f32::INFINITY).hint_text(rt("Search for thread name", 13.0).color(FAINT))).changed() {
            acts.push(Action::SearchThreads(q.clone()));
        }
        ui.ctx().data_mut(|d| d.insert_temp(qid, q));
        ui.add_space(8.0);
        if let Some(found) = &tl.search {
            match &found.found {
                None => {
                    ui.label(rt("Searching\u{2026}", 13.0).color(MUTED));
                }
                Some(Err(e)) => {
                    ui.label(rt(e, 12.5).color(DANGER));
                }
                Some(Ok(list)) if list.is_empty() => {
                    ui.label(rt("No threads found.", 13.0).color(MUTED));
                }
                Some(Ok(list)) => {
                    egui::ScrollArea::vertical().max_height(460.0).show(ui, |ui| {
                        for t in list {
                            row(ui, t, acts);
                        }
                    });
                }
            }
            return;
        }
        if sec.is_empty() {
            ui.add_space(14.0);
            ui.vertical_centered(|ui| {
                ui.label(rt(if tl.loading { "Loading\u{2026}" } else { "There are no threads yet." }, 13.0).color(MUTED));
            });
            ui.add_space(14.0);
        }
        if let Some(e) = &tl.error {
            ui.label(rt(e, 12.5).color(DANGER));
        }
        egui::ScrollArea::vertical().max_height(460.0).show(ui, |ui| {
            for (title, list) in [("ACTIVE THREADS YOU ARE IN", &sec.joined), ("OTHER ACTIVE THREADS", &sec.other), ("ARCHIVED", &sec.older)] {
                if list.is_empty() {
                    continue;
                }
                ui.add_space(6.0);
                ui.label(sb(title, 11.0).color(MUTED));
                for t in list {
                    row(ui, t, acts);
                }
            }
            if tl.more && !tl.loading && pill(ui, "Load more", false).clicked() {
                acts.push(Action::MoreThreads);
            }
        });
    });
    if close {
        acts.push(Action::ShowThreads(false));
    }
}

/// The New Thread form: a name, a first message, private or not.
pub fn new_thread(ctx: &egui::Context, s: &AppState, acts: &mut Vec<Action>) {
    let Some(f) = s.new_thread() else { return };
    let mut open = true;
    egui::Window::new("New Thread").id(egui::Id::new("new_thread")).open(&mut open).collapsible(false).resizable(false).default_width(420.0).show(ctx, |ui| {
        ui.set_width(400.0);
        ui.label(sb("THREAD NAME", 11.0).color(MUTED));
        let mut name = f.name.clone();
        let hint = if f.default_name.is_empty() { "New Thread".to_owned() } else { f.default_name.clone() };
        if ui.add(egui::TextEdit::singleline(&mut name).desired_width(f32::INFINITY).hint_text(rt(hint, 14.0).color(FAINT))).changed() {
            acts.push(Action::WriteNewThread { name: Some(name), body: None });
        }
        if f.message.is_none() {
            ui.add_space(8.0);
            ui.label(sb("FIRST MESSAGE", 11.0).color(MUTED));
            let mut body = f.body.clone();
            if ui.add(egui::TextEdit::multiline(&mut body).desired_rows(3).desired_width(f32::INFINITY)).changed() {
                acts.push(Action::WriteNewThread { name: None, body: Some(body) });
            }
        }
        if f.can_private && !f.only_private {
            ui.add_space(8.0);
            let mut private = f.private;
            if ui.checkbox(&mut private, rt("Private thread", 13.5)).changed() {
                acts.push(Action::SetThreadPrivate(private));
            }
        }
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if pill(ui, "Create Thread", true).clicked() {
                acts.push(Action::CreateThread);
            }
            if pill(ui, "Cancel", false).clicked() {
                acts.push(Action::CloseNewThread);
            }
        });
    });
    if !open {
        acts.push(Action::CloseNewThread);
    }
}
