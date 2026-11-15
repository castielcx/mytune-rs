//! The inbox (button in the brand strip): every channel and DM with unread messages, by server, mentions first.
//! A row opens its chat; a server's "Mark read" reads the whole server. Muted places list only when they mention
//! this account. The engine has no list of the mentioning messages themselves, so none is shown.

use super::*;
use mytune_chat::yapfast_core::chat::ChatRoom;

const OPEN: &str = "inbox_open";
const AT: &str = "inbox_at";
const W: f32 = 340.0;
const RED: Color32 = Color32::from_rgb(0xED, 0x42, 0x45);

/// The button; a red dot on it while anything mentions this account.
pub fn button(ui: &mut Ui, s: &AppState, at: egui::Rect) {
    let ctx = ui.ctx().clone();
    let r = ui.interact(at, egui::Id::new("inbox_button"), Sense::click());
    if r.hovered() {
        ui.painter().rect_filled(at, 10.0, p().side_selected);
    }
    voice::glyph(ui.painter(), at.center(), "\u{E715}", 15.0, p().side_text, false);
    if s.rooms.iter().any(|r| r.mentions > 0) {
        ui.painter().circle_filled(at.right_top() + vec2(-6.0, 6.0), 3.5, RED);
    }
    if r.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Inbox: unread chats and mentions").clicked() {
        let open: bool = ctx.data(|d| d.get_temp(egui::Id::new(OPEN))).unwrap_or(false);
        ctx.data_mut(|d| d.insert_temp(egui::Id::new(OPEN), !open));
    }
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(AT), at));
}

/// Whether a room belongs in the inbox: unread, readable, and not muted unless it mentions this account.
fn listed(r: &ChatRoom, server_muted: bool) -> bool {
    wanted(r.unread, r.mentions, r.locked, r.muted || server_muted)
}

fn wanted(unread: u64, mentions: u64, locked: bool, muted: bool) -> bool {
    (unread > 0 || mentions > 0) && !locked && (mentions > 0 || !muted)
}

/// The open inbox, under its button.
pub fn show(ctx: &egui::Context, s: &AppState, acts: &mut Vec<Action>) {
    let oid = egui::Id::new(OPEN);
    if !ctx.data(|d| d.get_temp::<bool>(oid)).unwrap_or(false) {
        return;
    }
    let Some(at) = ctx.data(|d| d.get_temp::<egui::Rect>(egui::Id::new(AT))) else { return };
    // (title, server id, rooms): DMs, then each server; groups and rooms that mention come first
    let by_id: HashMap<&str, &ChatRoom> = s.rooms.iter().map(|r| (r.id.as_str(), &**r)).collect();
    let mut groups: Vec<(String, Option<String>, Vec<&ChatRoom>)> = vec![];
    let dms: Vec<&ChatRoom> = s.rooms.iter().map(|r| &**r).filter(|r| r.direct && listed(r, false)).collect();
    if !dms.is_empty() {
        groups.push(("Direct messages".to_owned(), None, dms));
    }
    for sv in s.servers.iter() {
        let rooms: Vec<&ChatRoom> = sv.channels.iter().filter_map(|c| by_id.get(c.as_str()).copied()).filter(|r| listed(r, sv.muted)).collect();
        if !rooms.is_empty() {
            groups.push((sv.name.to_string(), Some(sv.id.to_string()), rooms));
        }
    }
    for g in &mut groups {
        g.2.sort_by_key(|r| r.mentions == 0);
    }
    groups.sort_by_key(|g| g.2.iter().all(|r| r.mentions == 0));

    let screen = ctx.content_rect();
    let pos = egui::pos2((at.right() - W).max(screen.left() + 8.0), at.bottom() + 6.0);
    let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let area = egui::Area::new(egui::Id::new("inbox_area")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        crate::theme::float(ui, crate::theme::R_MENU, 10, |ui| {
            ui.set_width(W);
            ui.horizontal(|ui| {
                ui.label(sb("Inbox", 15.0).color(TEXT));
                if groups.is_empty() {
                    return;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let r = ui.add(egui::Label::new(rt("Read all", 12.0).color(SECONDARY)).sense(Sense::click()));
                    if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        for (_, server, rooms) in &groups {
                            match server {
                                Some(id) => acts.push(Action::MarkRead(id.clone())),
                                None => acts.extend(rooms.iter().map(|r| Action::MarkRead(r.id.clone()))),
                            }
                        }
                    }
                });
            });
            ui.add_space(6.0);
            if groups.is_empty() {
                ui.label(rt("You're all caught up.", 13.0).color(MUTED));
                return;
            }
            egui::ScrollArea::vertical().max_height((screen.bottom() - pos.y - 80.0).clamp(160.0, 520.0)).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (title, server, rooms) in &groups {
                    // group heading, with Mark read for a server
                    let (head, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
                    let mark = egui::Rect::from_min_max(egui::pos2(head.right() - 70.0, head.top()), head.max);
                    ui.painter().with_clip_rect(head.with_max_x(mark.left() - 6.0)).text(head.left_center() + vec2(8.0, 0.0), egui::Align2::LEFT_CENTER, title.to_uppercase(), egui::FontId::proportional(11.0), MUTED);
                    if let Some(id) = server {
                        let r = ui.interact(mark, ui.id().with(("inbox_mark", id)), Sense::click());
                        ui.painter().text(mark.right_center() - vec2(8.0, 0.0), egui::Align2::RIGHT_CENTER, "Mark read", egui::FontId::proportional(11.5), if r.hovered() { TEXT } else { SECONDARY });
                        if r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            acts.push(Action::MarkRead(id.clone()));
                        }
                    }
                    for r in rooms {
                        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
                        if resp.hovered() {
                            ui.painter().rect_filled(rect, 8.0, k_hover());
                        }
                        let name = if r.direct { r.name.clone() } else { format!("# {}", r.name) };
                        ui.painter().with_clip_rect(rect.with_max_x(rect.right() - 44.0)).text(rect.left_center() + vec2(10.0, 0.0), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(14.0), TEXT);
                        let c = egui::pos2(rect.right() - 20.0, rect.center().y);
                        if r.mentions > 0 {
                            let n = if r.mentions > 99 { "99+".to_owned() } else { r.mentions.to_string() };
                            ui.painter().rect_filled(egui::Rect::from_center_size(c, vec2(if n.len() > 1 { 26.0 } else { 18.0 }, 18.0)), 9.0, RED);
                            ui.painter().text(c, egui::Align2::CENTER_CENTER, n, egui::FontId::proportional(11.0), Color32::WHITE);
                        } else {
                            ui.painter().circle_filled(c, 4.0, TEXT);
                        }
                        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            acts.push(Action::Open(r.id.clone()));
                            close = true;
                        }
                    }
                    ui.add_space(6.0);
                }
            });
        });
    });
    // a click outside closes it; one on the button is the button's own toggle
    if area.response.clicked_elsewhere() && !ctx.input(|i| i.pointer.interact_pos().is_some_and(|at_ptr| at.contains(at_ptr))) {
        close = true;
    }
    if close {
        ctx.data_mut(|d| d.insert_temp(oid, false));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn muted_lists_only_when_mentioned() {
        assert!(wanted(3, 0, false, false));
        assert!(!wanted(0, 0, false, false));
        assert!(!wanted(3, 0, false, true));
        assert!(wanted(3, 1, false, true));
        assert!(!wanted(1, 1, true, false));
    }
}
