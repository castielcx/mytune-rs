//! The Friends screen of the Discord home: Online, All, Pending, Blocked and Add Friend tabs, as Discord has them.
//! The engine owns the lists (`friends`, `friend_requests`, `discord_blocked`) and the actions; the tab, the search
//! text and the unfriend confirmation live here, in egui's per-frame memory.

use super::header::status_dot;
use super::voice::{glyph, k_edge, k_raised};
use super::*;
use mytune_chat::yapfast_core::application::DmScreen;
use mytune_chat::yapfast_core::chat::{Presence, RelationshipChange};

#[derive(Clone, Default)]
struct Fr {
    /// 0 Online, 1 All, 2 Blocked, 3 Add Friend (Pending is the engine's Requests screen).
    tab: u8,
    query: String,
    add_name: String,
    /// Someone about to be removed: id and name.
    confirm: Option<(String, String)>,
    /// The Requests screen shows message requests (DMs from people who aren't friends) rather than friend requests.
    msgs: bool,
}

fn st(ctx: &egui::Context) -> Fr {
    ctx.data(|d| d.get_temp::<Fr>(egui::Id::new("friends_state"))).unwrap_or_default()
}

fn put(ctx: &egui::Context, f: Fr) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("friends_state"), f));
}

/// Friends, Message requests and Requests above the conversations of the DM list, then a line under them.
pub fn nav_rows(ui: &mut Ui, s: &AppState, acts: &mut Vec<Action>) {
    let waiting = s.friend_requests.iter().filter(|r| r.incoming && !r.spam).count();
    let messages = message_requests_of(s).filter(|r| &*r.category != SPAM).count();
    let mut f = st(ui.ctx());
    let rows = [("Friends", "\u{E716}", DmScreen::Friends, false, 0), ("Message requests", "\u{E8BD}", DmScreen::Requests, true, messages), ("Requests", "\u{E8FA}", DmScreen::Requests, false, waiting)];
    for (label, g, screen, msgs, badge) in rows {
        let on = s.dm_screen == Some(screen) && (screen == DmScreen::Friends || f.msgs == msgs);
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width() - 16.0, 38.0), Sense::click());
        let r = r.translate(vec2(8.0, 0.0));
        let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), voice::QUICK);
        if on || t > 0.0 {
            ui.painter().rect_filled(r, 12.0, if on { k_active() } else { k_active().gamma_multiply(t) });
        }
        let lit = on || (p().black && resp.hovered());
        glyph(ui.painter(), egui::pos2(r.left() + 24.0, r.center().y), g, 17.0, if lit { TEXT } else { SECONDARY }, false);
        ui.painter().text(egui::pos2(r.left() + 48.0, r.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::new(14.5, th::semibold()), if lit { TEXT } else { SECONDARY });
        if badge > 0 {
            let c = egui::pos2(r.right() - 20.0, r.center().y);
            if !p().black {
                ui.painter().circle_filled(c, 10.0, DANGER);
            }
            ui.painter().text(c, egui::Align2::CENTER_CENTER, badge.to_string(), egui::FontId::new(11.0, th::bold()), if p().black { DANGER } else { Color32::WHITE });
        }
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            f.msgs = msgs;
            acts.push(Action::OpenDmScreen(screen));
        }
    }
    put(ui.ctx(), f);
    // a line between these rows and the conversations, so they read as two groups
    ui.add_space(6.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(r.shrink2(vec2(16.0, 0.0)), 0.0, k_line());
    ui.add_space(6.0);
}

/// Discord's spam folder for DMs, the engine's category for them.
const SPAM: &str = "Spam";

/// DMs from people who aren't friends, waiting to be accepted or ignored (spam ones too).
pub fn message_requests_of(s: &AppState) -> impl Iterator<Item = &mytune_chat::yapfast_core::chat::ChatRoom> {
    s.rooms.iter().map(|r| &**r).filter(|r| r.direct && r.request)
}

/// The message requests screen: each request with its latest message; open it to read it, accept or ignore it.
#[allow(clippy::too_many_arguments)]
fn message_requests(ui: &mut Ui, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let all: Vec<_> = message_requests_of(s).collect();
    if all.is_empty() {
        ui.add_space(70.0);
        ui.vertical_centered(|ui| {
            ui.label(sb("No message requests", 18.0).color(TEXT));
            ui.label(rt("Messages from people who aren't your friends show up here.", 13.5).color(MUTED));
        });
        return;
    }
    for (title, spam) in [("MESSAGE REQUESTS", false), ("SPAM", true)] {
        let list: Vec<_> = all.iter().filter(|r| (&*r.category == SPAM) == spam).collect();
        if list.is_empty() {
            continue;
        }
        heading(ui, &format!("{title} \u{2014} {}", list.len()));
        for r in list {
            let id = r.id.clone();
            let line = r.preview.as_ref().map_or_else(String::new, |p| p.text.clone());
            row(ui, tex, asked, want, s, &r.avatar, &r.name, &line, None, |ui| {
                // right to left: the last one added sits leftmost. Accept and Ignore answer the open room, so it opens first.
                if icon_btn(ui, "\u{E711}", "Ignore", true) {
                    acts.push(Action::Open(id.clone()));
                    acts.push(Action::AnswerRequest(false));
                }
                if icon_btn(ui, "\u{E73E}", "Accept", false) {
                    acts.push(Action::Open(id.clone()));
                    acts.push(Action::AnswerRequest(true));
                }
                if icon_btn(ui, "\u{E8BD}", "Open", false) {
                    acts.push(Action::Open(id.clone()));
                }
            });
        }
    }
}

pub(super) fn icon_btn(ui: &mut Ui, g: &str, tip: &str, danger: bool) -> bool {
    let (r, mut resp) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::click_and_drag());
    let r = crate::theme::pulled(r, crate::theme::pull(ui, &mut resp));
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), voice::QUICK);
    ui.painter().circle_filled(r.center(), 17.0 + t, k_raised().lerp_to_gamma(k_hover(), t)); // grows a little on hover
    let col = if danger && resp.hovered() { DANGER } else if resp.hovered() { TEXT } else { SECONDARY };
    glyph(ui.painter(), r.center(), g, 15.0, col, false);
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

fn chip(ui: &mut Ui, text: &str, on: bool, accent: bool) -> bool {
    let pal = p();
    let g = ui.painter().layout_no_wrap(text.to_owned(), egui::FontId::new(13.5, th::semibold()), TEXT);
    let (r, mut resp) = ui.allocate_exact_size(vec2(g.size().x + 26.0, 30.0), Sense::click_and_drag());
    let r = crate::theme::pulled(r, crate::theme::pull(ui, &mut resp));
    let fill = if accent && pal.black { Color32::TRANSPARENT } else if accent { pal.accent } else if on { k_active() } else if resp.hovered() { k_hover() } else { Color32::TRANSPARENT };
    ui.painter().rect_filled(r, crate::theme::R_CTL, fill);
    let fg = if accent { pal.on_accent } else if on { TEXT } else { SECONDARY };
    ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::new(13.5, th::semibold()), fg);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// One person's row: avatar with status, name, a line under it, and the buttons `buttons` draws on the right.
/// True when the avatar or the name was clicked (the caller opens the profile).
#[allow(clippy::too_many_arguments)]
fn row(ui: &mut Ui, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, s: &AppState, avatar_url: &Option<String>, name: &str, line: &str, presence: Option<Option<Presence>>, buttons: impl FnOnce(&mut Ui)) -> bool {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 60.0), Sense::hover());
    // contains_pointer: the clickable name on top would end `hovered`
    let over = resp.contains_pointer();
    if over {
        ui.painter().rect_filled(r, 12.0, k_row_hover());
    }
    ui.painter().line_segment([egui::pos2(r.left() + 8.0, r.bottom()), egui::pos2(r.right() - 8.0, r.bottom())], egui::Stroke::new(1.0, k_line()));
    let ctx = ui.ctx().clone();
    let t = texture(&ctx, tex, asked, s, avatar_url, want);
    let a = egui::Rect::from_center_size(egui::pos2(r.left() + 30.0, r.center().y), vec2(38.0, 38.0));
    avatar(ui, a, t, name);
    if let Some(p) = presence {
        status_dot(ui, a, p, if over { k_row_hover() } else { k_chat() });
    }
    ui.painter().text(egui::pos2(r.left() + 58.0, r.center().y - 9.0), egui::Align2::LEFT_CENTER, name, egui::FontId::new(14.5, th::semibold()), th::hover_text(TEXT, over));
    ui.painter().text(egui::pos2(r.left() + 58.0, r.center().y + 10.0), egui::Align2::LEFT_CENTER, line, egui::FontId::proportional(12.5), MUTED);
    // the whole row opens the profile; the buttons are added after it, so they sit on top and keep their clicks
    let clicked = ui.interact(r, resp.id.with("profile"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked();
    if clicked {
        super::header::remember_profile_at(ui.ctx()); // the card opens beside the click
    }
    let bw = 160.0;
    let slot = egui::Rect::from_min_size(egui::pos2(r.right() - bw - 8.0, r.center().y - 17.0), vec2(bw, 34.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(slot).layout(Layout::right_to_left(Align::Center)), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        buttons(ui);
    });
    // the button scope moved the cursor up to the middle of the row: put it back under the whole row
    ui.advance_cursor_after_rect(r);
    clicked
}

fn status_text(f: &mytune_chat::yapfast_core::chat::Friend) -> String {
    if let Some(a) = &f.activity {
        if !a.is_empty() {
            return a.clone();
        }
    }
    match f.presence {
        Some(Presence::Online) => "Online",
        Some(Presence::Mobile) => "Online on mobile",
        Some(Presence::Idle) => "Idle",
        Some(Presence::DoNotDisturb) => "Do Not Disturb",
        _ => "Offline",
    }
    .to_owned()
}

fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(14.0);
    ui.label(sb(text, 11.5).color(MUTED));
    ui.add_space(4.0);
}

/// The screen: tab strip, then the list for the tab.
#[allow(clippy::too_many_arguments)]
pub fn screen(ui: &mut Ui, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    let mut f = st(&ctx);
    let pending = s.dm_screen == Some(DmScreen::Requests);
    if pending && f.msgs {
        egui::Frame::new().fill(k_chat()).inner_margin(egui::Margin::symmetric(18, 10)).stroke(egui::Stroke::new(1.0, k_edge())).show(ui, |ui| {
            ui.horizontal(|ui| {
                glyph(ui.painter(), egui::pos2(ui.cursor().left() + 10.0, ui.cursor().center().y + 2.0), "\u{E8BD}", 18.0, SECONDARY, false);
                ui.add_space(26.0);
                ui.label(sb("Message requests", 16.0).color(TEXT));
            });
        });
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(24, 8)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                message_requests(ui, s, tex, asked, want, acts);
            });
        });
        return;
    }
    // header strip
    egui::Frame::new().fill(k_chat()).inner_margin(egui::Margin::symmetric(18, 10)).stroke(egui::Stroke::new(1.0, k_edge())).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            glyph(ui.painter(), egui::pos2(ui.cursor().left() + 10.0, ui.cursor().center().y + 2.0), "\u{E716}", 18.0, SECONDARY, false);
            ui.add_space(26.0);
            ui.label(sb("Friends", 16.0).color(TEXT));
            ui.add_space(14.0);
            let waiting = s.friend_requests.iter().filter(|r| r.incoming && !r.spam).count();
            let tabs = [("Online", 0u8), ("All", 1)];
            for (name, id) in tabs {
                if chip(ui, name, !pending && f.tab == id, false) {
                    f.tab = id;
                    if pending {
                        acts.push(Action::OpenDmScreen(DmScreen::Friends));
                    }
                }
            }
            if chip(ui, &if waiting > 0 { format!("Pending ({waiting})") } else { "Pending".to_owned() }, pending, false) && !pending {
                acts.push(Action::OpenDmScreen(DmScreen::Requests));
            }
            if chip(ui, "Blocked", !pending && f.tab == 2, false) {
                f.tab = 2;
                if pending {
                    acts.push(Action::OpenDmScreen(DmScreen::Friends));
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if chip(ui, "Add Friend", false, true) {
                    f.tab = 3;
                    if pending {
                        acts.push(Action::OpenDmScreen(DmScreen::Friends));
                    }
                }
            });
        });
    });
    let adding = !pending && f.tab == 3;
    if adding && s.add_friend.is_none() {
        acts.push(Action::OpenAddFriend);
    } else if !adding && s.add_friend.is_some() {
        acts.push(Action::CloseAddFriend);
    }
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(24, 8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            if adding {
                add_friend(ui, s, &mut f, acts);
            } else if pending {
                requests(ui, s, tex, asked, want, acts);
            } else if f.tab == 2 {
                blocked(ui, s, tex, asked, want, acts);
            } else {
                friends(ui, s, &mut f, tex, asked, want, acts);
            }
        });
    });
    confirm(&ctx, &mut f, acts);
    put(&ctx, f);
}

fn search(ui: &mut Ui, f: &mut Fr, hint: &str) {
    egui::Frame::new().fill(k_rail()).corner_radius(12.0).inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.horizontal(|ui| {
            glyph(ui.painter(), egui::pos2(ui.cursor().left() + 8.0, ui.cursor().center().y + 2.0), "\u{E721}", 14.0, MUTED, false);
            ui.add_space(22.0);
            ui.add(egui::TextEdit::singleline(&mut f.query).frame(egui::Frame::NONE).desired_width(f32::INFINITY).hint_text(rt(hint, 14.0).color(FAINT)));
        });
    });
}

fn friends(ui: &mut Ui, s: &AppState, f: &mut Fr, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    ui.add_space(8.0);
    search(ui, f, "Search");
    let q = f.query.to_lowercase();
    let all = f.tab == 1;
    let list: Vec<&mytune_chat::yapfast_core::chat::Friend> = s.friends.iter().filter(|x| (all || !matches!(x.presence, None | Some(Presence::Offline))) && (q.is_empty() || x.name.to_lowercase().contains(&q))).collect();
    if s.friends.is_empty() {
        ui.add_space(70.0);
        ui.vertical_centered(|ui| {
            ui.label(sb("No friends yet", 18.0).color(TEXT));
            ui.label(rt("Add someone by their username with Add Friend.", 13.5).color(MUTED));
        });
        return;
    }
    heading(ui, &format!("{} \u{2014} {}", if all { "ALL FRIENDS" } else { "ONLINE" }, list.len()));
    if list.is_empty() {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| ui.label(rt(if q.is_empty() { "Nobody is online right now." } else { "No friend matches that." }, 13.5).color(MUTED)));
    }
    let f_confirm = std::cell::Cell::new(None);
    for x in list {
        let line = status_text(x);
        let (id, name) = (x.id.clone(), x.name.clone());
        if row(ui, tex, asked, want, s, &x.avatar, &x.name, &line, Some(x.presence), |ui| {
            if icon_btn(ui, "\u{E712}", "Remove friend", true) {
                f_confirm.set(Some((id.clone(), name.clone())));
            }
            if icon_btn(ui, "\u{E8BD}", "Message", false) {
                acts.push(Action::MessagePerson(id.clone()));
            }
        }) {
            acts.push(Action::ShowProfile { user: x.id.clone(), large: false });
        }
    }
    if let Some(c) = f_confirm.take() {
        f.confirm = Some(c);
    }
}

fn requests(ui: &mut Ui, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let incoming: Vec<_> = s.friend_requests.iter().filter(|r| r.incoming && !r.spam).collect();
    let sent: Vec<_> = s.friend_requests.iter().filter(|r| !r.incoming).collect();
    let spam: Vec<_> = s.friend_requests.iter().filter(|r| r.incoming && r.spam).collect();
    if incoming.is_empty() && sent.is_empty() && spam.is_empty() {
        ui.add_space(70.0);
        ui.vertical_centered(|ui| {
            ui.label(sb("No pending requests", 18.0).color(TEXT));
            ui.label(rt("Friend requests you send and receive show up here.", 13.5).color(MUTED));
        });
        return;
    }
    for (title, list, kind) in [("INCOMING", incoming, 0), ("OUTGOING", sent, 1), ("SPAM", spam, 2)] {
        if list.is_empty() {
            continue;
        }
        heading(ui, &format!("{title} \u{2014} {}", list.len()));
        for r in list {
            let id = r.id.clone();
            let line = match kind {
                0 => "Incoming Friend Request",
                1 => "Outgoing Friend Request",
                _ => "Likely spam",
            };
            if row(ui, tex, asked, want, s, &r.avatar, &r.name, line, None, |ui| {
                // right to left: the last one added sits leftmost
                if icon_btn(ui, "\u{E711}", if kind == 1 { "Cancel request" } else { "Ignore" }, true) {
                    acts.push(Action::RemoveFriendRequest(id.clone()));
                }
                if kind != 1 && icon_btn(ui, "\u{E73E}", "Accept", false) {
                    acts.push(Action::Befriend(id.clone()));
                }
            }) {
                acts.push(Action::ShowProfile { user: r.id.clone(), large: false });
            }
        }
    }
}

fn blocked(ui: &mut Ui, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    if s.discord_blocked.is_empty() {
        ui.add_space(70.0);
        ui.vertical_centered(|ui| {
            ui.label(sb("Nobody is blocked", 18.0).color(TEXT));
            ui.label(rt("People you block show up here, and you can unblock them.", 13.5).color(MUTED));
        });
        return;
    }
    heading(ui, &format!("BLOCKED \u{2014} {}", s.discord_blocked.len()));
    for b in &s.discord_blocked {
        let id = b.id.clone();
        if row(ui, tex, asked, want, s, &b.avatar, &b.name, b.username.as_deref().unwrap_or("Blocked"), None, |ui| {
            if icon_btn(ui, "\u{E8FB}", "Unblock", false) {
                acts.push(Action::ChangeRelationship { user: id.clone(), change: RelationshipChange::Unblock });
            }
        }) {
            acts.push(Action::ShowProfile { user: b.id.clone(), large: false });
        }
    }
}

fn add_friend(ui: &mut Ui, s: &AppState, f: &mut Fr, acts: &mut Vec<Action>) {
    ui.add_space(14.0);
    ui.label(sb("Add Friend", 18.0).color(TEXT));
    ui.label(rt("You can add friends with their Discord username.", 13.5).color(MUTED));
    ui.add_space(12.0);
    let sending = s.add_friend.as_ref().is_some_and(|a| a.sending);
    let mut go = false;
    egui::Frame::new().fill(k_rail()).corner_radius(14.0).inner_margin(egui::Margin::symmetric(14, 10)).show(ui, |ui| {
        ui.horizontal(|ui| {
            let w = ui.available_width() - 190.0;
            let te = ui.add(egui::TextEdit::singleline(&mut f.add_name).frame(egui::Frame::NONE).desired_width(w).hint_text(rt("You can add friends with their Discord username.", 14.0).color(FAINT)));
            if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                go = true;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let enabled = !f.add_name.trim().is_empty() && !sending;
                ui.add_enabled_ui(enabled, |ui| {
                    if pill(ui, "Send Friend Request", true).clicked() {
                        go = true;
                    }
                });
            });
        });
    });
    if go && !f.add_name.trim().is_empty() && !sending {
        acts.push(Action::AddFriend(f.add_name.clone()));
        ui.data_mut(|d| d.insert_temp(egui::Id::new("add_friend_sent"), true));
    }
    ui.add_space(8.0);
    match s.add_friend.as_ref().and_then(|a| a.outcome.as_ref()) {
        _ if sending => {
            ui.label(rt("Sending\u{2026}", 13.5).color(MUTED));
        }
        Some(Ok(name)) => {
            ui.label(rt(format!("Success! Your friend request to {name} was sent."), 13.5).color(LIVE));
            // once per request: the outcome stays until the next one, and the field must stay usable
            if ui.data_mut(|d| d.remove_temp::<bool>(egui::Id::new("add_friend_sent"))).is_some() {
                f.add_name.clear();
            }
        }
        Some(Err(why)) => {
            ui.label(rt(why, 13.5).color(DANGER));
        }
        None => {}
    }
}

/// Asks before removing a friend.
fn confirm(ctx: &egui::Context, f: &mut Fr, acts: &mut Vec<Action>) {
    let Some((id, name)) = f.confirm.clone() else { return };
    let mut close = false;
    let modal = egui::Modal::new(egui::Id::new("unfriend")).show(ctx, |ui| {
        ui.set_width(380.0);
        ui.label(sb(format!("Remove '{name}'"), 17.0).color(TEXT));
        ui.add_space(4.0);
        ui.label(rt(format!("Are you sure you want to permanently remove {name} from your friends?"), 13.5).color(SECONDARY));
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if crate::theme::button_tint(ui, "Remove Friend", false, Some(DANGER), (18.0, 8.0, 18.0, 9.0)).clicked() {
                acts.push(Action::ChangeRelationship { user: id.clone(), change: RelationshipChange::Unfriend });
                close = true;
            }
            if crate::theme::button(ui, "Cancel", false, (18.0, 8.0, 18.0, 9.0)).clicked() {
                close = true;
            }
        });
    });
    if close || modal.should_close() {
        f.confirm = None;
    }
}
