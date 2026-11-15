//! The right-click menu of a server on the rail, a channel in the list, or a conversation. The engine decides
//! what it offers (`Application::menu_choices`, as Yapfast's own menus do); this draws those choices and turns
//! a pick into an engine action: Mark As Read, Invite People, Mute for a time, notification levels and the
//! server switches, Server Settings, channel create/edit/duplicate/delete, Leave Server, copy link and ID.
//! Submenus (mute times, levels) open beside their row, as Discord's do (`arrow_row`, `flyout`).

use super::voice::{glyph, k_edge, k_panel, k_raised, QUICK};
use super::*;
use mytune_chat::yapfast_core::application::{network_id, network_link, Application, ChannelAsk, MenuChoice, MUTE_TIMES};
use mytune_chat::yapfast_core::chat::{ChannelKind, Muting, NotificationChange, NotifyLevel};

#[derive(Clone, Copy, PartialEq)]
enum Sub {
    Mute,
    Levels,
}

#[derive(Clone)]
struct Open {
    id: String,
    at: egui::Pos2,
    sub: Option<Sub>,
}

fn key() -> egui::Id {
    egui::Id::new("place_menu")
}

/// Opens the menu of a place (server id, room id) at the pointer.
pub fn open(ctx: &egui::Context, id: &str) {
    let at = ctx.input(|i| i.pointer.interact_pos()).unwrap_or(egui::pos2(100.0, 100.0));
    ctx.data_mut(|d| d.insert_temp(key(), Open { id: id.to_owned(), at, sub: None }));
}

fn close(ctx: &egui::Context) {
    ctx.data_mut(|d| d.remove::<Open>(key()));
}

const W: f32 = 240.0;

pub(super) fn row(ui: &mut Ui, g: &str, text: &str, danger: bool, chevron: bool, checked: Option<bool>) -> bool {
    row_resp(ui, g, text, danger, chevron, checked, false).clicked()
}

/// A row with an arrow that has a submenu beside it; it stays lit while that submenu is `open`.
pub(super) fn arrow_row(ui: &mut Ui, g: &str, text: &str, open: bool) -> egui::Response {
    row_resp(ui, g, text, false, true, None, open)
}

/// The submenu of an `arrow_row`: its own panel beside the menu, level with the row; left of the menu when
/// the right has no room. Returns where it is, so the menu can count clicks in it as its own.
pub(super) fn flyout(ctx: &egui::Context, id: egui::Id, row: egui::Rect, menu: egui::Rect, add: impl FnOnce(&mut Ui)) -> egui::Rect {
    let w = W + 12.0;
    let x = if menu.right() + 4.0 + w <= ctx.content_rect().right() { menu.right() + 4.0 } else { menu.left() - 4.0 - w };
    let shown = egui::Area::new(id).order(egui::Order::Foreground).fixed_pos(egui::pos2(x, row.top() - 6.0)).constrain(true).show(ctx, |ui| {
        crate::theme::float(ui, crate::theme::R_MENU, 6, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui);
        });
    });
    ctx.move_to_top(shown.response.layer_id);
    shown.response.rect
}

/// True when the pointer rests on another row of the menu: the submenu closes then. Crossing other rows on
/// the way to the submenu keeps it open.
pub(super) fn left_sub(ctx: &egui::Context, menu: egui::Rect, row: egui::Rect, sub: egui::Rect) -> bool {
    ctx.input(|i| {
        i.pointer.hover_pos().is_some_and(|p| {
            let toward = i.pointer.velocity().x * (sub.center().x - menu.center().x).signum();
            menu.contains(p) && !sub.contains(p) && !row.expand2(vec2(8.0, 0.0)).contains(p) && toward < 20.0
        })
    })
}

fn row_resp(ui: &mut Ui, g: &str, text: &str, danger: bool, chevron: bool, checked: Option<bool>, lit: bool) -> egui::Response {
    let c = if danger { DANGER } else { SECONDARY };
    let (r, resp) = ui.allocate_exact_size(vec2(W, 32.0), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() || lit, QUICK);
    if t > 0.0 {
        ui.painter().rect_filled(r, 10.0, (if danger { DANGER } else { p().accent }).gamma_multiply(0.25 * t));
    }
    let col = c.lerp_to_gamma(TEXT, t);
    glyph(ui.painter(), egui::pos2(r.left() + 16.0, r.center().y), g, 14.0, col, false);
    ui.painter().text(egui::pos2(r.left() + 36.0, r.center().y), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(14.0), col);
    if chevron {
        glyph(ui.painter(), egui::pos2(r.right() - 14.0, r.center().y), "\u{E76C}", 11.0, col, false);
    }
    if let Some(on) = checked {
        let b = egui::Rect::from_center_size(egui::pos2(r.right() - 18.0, r.center().y), vec2(16.0, 16.0));
        ui.painter().rect(b, 3.0, if on { p().accent } else { Color32::TRANSPARENT }, egui::Stroke::new(1.5, if on { p().accent } else { MUTED }), egui::StrokeKind::Inside);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A row of a submenu, with a tick on the chosen one.
pub(super) fn sub_row(ui: &mut Ui, text: &str, note: &str, on: bool) -> bool {
    let (r, resp) = ui.allocate_exact_size(vec2(W, if note.is_empty() { 28.0 } else { 40.0 }), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), QUICK);
    if t > 0.0 {
        ui.painter().rect_filled(r, 10.0, p().accent.gamma_multiply(0.2 * t));
    }
    let y = if note.is_empty() { r.center().y } else { r.top() + 14.0 };
    ui.painter().text(egui::pos2(r.left() + 14.0, y), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(13.5), if on { TEXT } else { SECONDARY });
    if !note.is_empty() {
        ui.painter().text(egui::pos2(r.left() + 14.0, r.top() + 29.0), egui::Align2::LEFT_CENTER, note, egui::FontId::proportional(11.5), MUTED);
    }
    if on {
        glyph(ui.painter(), egui::pos2(r.right() - 18.0, r.center().y), "\u{E73E}", 13.0, p().accent, false);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

pub(super) fn sep(ui: &mut Ui) {
    ui.add_space(3.0);
    let (r, _) = ui.allocate_exact_size(vec2(W, 1.0), Sense::hover());
    ui.painter().rect_filled(r.shrink2(vec2(8.0, 0.0)), 0.0, super::k_line());
    ui.add_space(3.0);
}

pub(super) fn mute_text(m: Muting) -> String {
    match m {
        Muting::Minutes(15) => "For 15 minutes".into(),
        Muting::Minutes(60) => "For 1 hour".into(),
        Muting::Minutes(n) if n % 60 == 0 => format!("For {} hours", n / 60),
        Muting::Minutes(n) => format!("For {n} minutes"),
        Muting::Always | Muting::Off => "Until I turn it back on".into(),
    }
}

fn level_text(l: NotifyLevel) -> &'static str {
    match l {
        NotifyLevel::All => "All messages",
        NotifyLevel::Mentions => "Only @mentions",
        NotifyLevel::Nothing => "Nothing",
    }
}

/// Draws the open menu, if any. Picks push engine actions into `acts`.
pub fn show(ctx: &egui::Context, app: &Application, acts: &mut Vec<Action>) {
    let Some(mut st) = ctx.data(|d| d.get_temp::<Open>(key())) else { return };
    let choices = app.menu_choices(&st.id);
    if choices.is_empty() {
        close(ctx);
        return;
    }
    let id = st.id.clone();
    let server = app.state.servers.iter().any(|s| s.id == id);
    let dm = app.state.rooms.iter().any(|r| r.id == id && r.direct);
    let screen = ctx.content_rect();
    let pos = egui::pos2(st.at.x.min(screen.right() - W - 24.0), st.at.y.min(screen.bottom() - 520.0).max(screen.top()));
    let mut done = false;
    let mut sub_at = None; // the arrow row whose submenu is open
    let notes = app.notifications(&id);
    let snotes = if server { app.server_notifications(&id) } else { None };
    let shown = egui::Area::new(egui::Id::new("place_menu_area")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        crate::theme::float(ui, crate::theme::R_MENU, 6, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut last_group = 0;
            // UI-only row: a text channel or DM other than the open one shows beside it
            let st8 = &app.state;
            if st8.selected.as_ref().is_some_and(|sel| *sel != id) && st8.rooms.iter().any(|r| r.id == id && !r.forum && !r.voice && !r.locked) {
                if row(ui, "\u{E90D}", "Open Side by Side", false, false, None) {
                    acts.push(Action::OpenBeside(id.clone()));
                    done = true;
                }
                sep(ui);
            }
            for c in &choices {
                // groups, so separators fall where Discord's menus have them
                let group = match c {
                    MenuChoice::MarkRead | MenuChoice::JoinVoice | MenuChoice::OpenChat | MenuChoice::Profile | MenuChoice::Call => 1,
                    MenuChoice::Invite | MenuChoice::Pin | MenuChoice::Unpin => 2,
                    MenuChoice::MuteServer | MenuChoice::UnmuteServer | MenuChoice::ChannelMute | MenuChoice::Mute | MenuChoice::Unmute | MenuChoice::ServerNotifications | MenuChoice::ChannelNotifications | MenuChoice::HideMuted | MenuChoice::ShowAllChannels => 3,
                    MenuChoice::ServerSettings | MenuChoice::EditServerProfile | MenuChoice::CreateChannel | MenuChoice::CreateCategory | MenuChoice::EditChannel | MenuChoice::DuplicateChannel | MenuChoice::PinChannel | MenuChoice::UnpinChannel | MenuChoice::AddToChannelList | MenuChoice::RemoveFromChannelList | MenuChoice::UnfollowCategory => 4,
                    MenuChoice::LeaveServer | MenuChoice::DeleteChannel | MenuChoice::LeaveGroup | MenuChoice::CloseDm => 5,
                    _ => 6,
                };
                if matches!(c, MenuChoice::CopyLink | MenuChoice::CopyId) && last_group != 6 && last_group != 0 {
                    sep(ui);
                    last_group = 6;
                } else if group != last_group && last_group != 0 && group != 6 {
                    sep(ui);
                    last_group = group;
                } else if last_group == 0 {
                    last_group = group;
                }
                match c {
                    MenuChoice::MarkRead => {
                        if row(ui, "\u{E73E}", "Mark As Read", false, false, None) {
                            acts.push(Action::MarkRead(id.clone()));
                            done = true;
                        }
                    }
                    MenuChoice::JoinVoice => {
                        if row(ui, "\u{E720}", "Join Voice", false, false, None) {
                            acts.push(Action::RequestJoinVoice(id.clone()));
                            done = true;
                        }
                    }
                    MenuChoice::Call => {
                        if row(ui, "\u{E717}", "Call", false, false, None) {
                            acts.push(Action::RequestJoinVoice(id.clone()));
                            done = true;
                        }
                    }
                    MenuChoice::OpenChat => {
                        if row(ui, "\u{E8BD}", "Open Chat", false, false, None) {
                            acts.push(Action::OpenChat(id.clone()));
                            done = true;
                        }
                    }
                    MenuChoice::Profile => {
                        if row(ui, "\u{E77B}", "Profile", false, false, None) {
                            acts.push(Action::ShowDmProfile(id.clone()));
                            done = true;
                        }
                    }
                    MenuChoice::Invite => {
                        if row(ui, "\u{E8FA}", "Invite People", false, false, None) {
                            acts.push(Action::OpenInvite(id.clone()));
                            done = true;
                        }
                    }
                    MenuChoice::Pin | MenuChoice::Unpin => {
                        let pin = *c == MenuChoice::Pin;
                        if row(ui, "\u{E718}", if pin { "Pin DM" } else { "Unpin DM" }, false, false, None) {
                            acts.push(Action::Pin { room: id.clone(), pinned: pin });
                            done = true;
                        }
                    }
                    MenuChoice::MuteServer | MenuChoice::ChannelMute | MenuChoice::Mute => {
                        let muted_already = *c == MenuChoice::ChannelMute && notes.is_some_and(|n| n.muted);
                        let what = if server { "Server" } else if dm { "Conversation" } else { "Channel" };
                        if muted_already {
                            if row(ui, "\u{E74F}", &format!("Unmute {what}"), false, false, None) {
                                acts.push(Action::ChangeNotifications { place: id.clone(), change: NotificationChange::Mute(Muting::Off) });
                                done = true;
                            }
                        } else {
                            let r = arrow_row(ui, "\u{E7ED}", &format!("Mute {what}"), st.sub == Some(Sub::Mute));
                            if r.hovered() || r.clicked() {
                                st.sub = Some(Sub::Mute);
                            }
                            if st.sub == Some(Sub::Mute) {
                                sub_at = Some(r.rect);
                            }
                        }
                    }
                    MenuChoice::UnmuteServer => {
                        if row(ui, "\u{E74F}", "Unmute Server", false, false, None) {
                            acts.push(Action::ChangeNotifications { place: id.clone(), change: NotificationChange::Mute(Muting::Off) });
                            done = true;
                        }
                    }
                    MenuChoice::Unmute => {
                        if row(ui, "\u{E74F}", "Unmute Conversation", false, false, None) {
                            acts.push(Action::MuteDm { room: id.clone(), muting: Muting::Off });
                            done = true;
                        }
                    }
                    MenuChoice::ServerNotifications | MenuChoice::ChannelNotifications => {
                        let r = arrow_row(ui, "\u{E7E7}", "Notification Settings", st.sub == Some(Sub::Levels));
                        if r.hovered() || r.clicked() {
                            st.sub = Some(Sub::Levels);
                        }
                        if st.sub == Some(Sub::Levels) {
                            sub_at = Some(r.rect);
                        }
                    }
                    MenuChoice::HideMuted => {
                        let on = snotes.is_some_and(|n| n.hide_muted);
                        if row(ui, "\u{E7B3}", "Hide Muted Channels", false, false, Some(on)) {
                            acts.push(Action::ChangeNotifications { place: id.clone(), change: NotificationChange::HideMuted(!on) });
                            done = true;
                        }
                    }
                    MenuChoice::ShowAllChannels => {
                        let on = snotes.and_then(|n| n.show_all).unwrap_or(true);
                        if row(ui, "\u{E8FD}", "Show All Channels", false, false, Some(on)) {
                            acts.push(Action::ShowAllChannels { server: id.clone(), show: !on });
                            done = true;
                        }
                    }
                    MenuChoice::ServerSettings => {
                        if row(ui, "\u{E713}", "Server Settings", false, false, None) {
                            acts.push(Action::OpenServerSettings(id.clone()));
                            done = true;
                        }
                    }
                    MenuChoice::EditServerProfile => {
                        if row(ui, "\u{E77B}", "Edit Server Profile", false, false, None) {
                            acts.push(Action::EditServerProfile(id.clone()));
                            done = true;
                        }
                    }
                    MenuChoice::CreateChannel => {
                        if row(ui, "\u{E710}", "Create Channel", false, false, None) {
                            acts.push(Action::NewChannel { place: id.clone(), kind: ChannelKind::Text });
                            done = true;
                        }
                    }
                    MenuChoice::CreateCategory => {
                        if row(ui, "\u{E8F4}", "Create Category", false, false, None) {
                            acts.push(Action::NewChannel { place: id.clone(), kind: ChannelKind::Category });
                            done = true;
                        }
                    }
                    MenuChoice::EditChannel => {
                        if row(ui, "\u{E713}", "Edit Channel", false, false, None) {
                            acts.push(Action::OpenChannelSettings(id.clone()));
                            done = true;
                        }
                    }
                    MenuChoice::DuplicateChannel => {
                        if row(ui, "\u{E8C8}", "Duplicate Channel", false, false, None) {
                            acts.push(Action::AskChannel(ChannelAsk::Duplicate(id.clone())));
                            done = true;
                        }
                    }
                    MenuChoice::DeleteChannel => {
                        if row(ui, "\u{E74D}", "Delete Channel", true, false, None) {
                            acts.push(Action::AskChannel(ChannelAsk::Delete(id.clone())));
                            done = true;
                        }
                    }
                    MenuChoice::PinChannel | MenuChoice::UnpinChannel => {
                        let pin = *c == MenuChoice::PinChannel;
                        if row(ui, "\u{E718}", if pin { "Pin Channel" } else { "Unpin Channel" }, false, false, None) {
                            acts.push(Action::PinChannel { room: id.clone(), pinned: pin });
                            done = true;
                        }
                    }
                    MenuChoice::AddToChannelList | MenuChoice::RemoveFromChannelList => {
                        let add = *c == MenuChoice::AddToChannelList;
                        if row(ui, if add { "\u{E710}" } else { "\u{E738}" }, if add { "Add To Channel List" } else { "Remove From Channel List" }, false, false, None) {
                            acts.push(Action::FollowChannel { room: id.clone(), follow: add });
                            done = true;
                        }
                    }
                    MenuChoice::UnfollowCategory => {
                        if row(ui, "\u{E738}", "Unfollow Category", false, false, None) {
                            if let Some(cat) = app.category_of(&id) {
                                acts.push(Action::FollowChannel { room: cat, follow: false });
                            }
                            done = true;
                        }
                    }
                    MenuChoice::LeaveServer => {
                        if row(ui, "\u{E7E8}", "Leave Server", true, false, None) {
                            acts.push(Action::AskLeaveServer(id.clone()));
                            done = true;
                        }
                    }
                    MenuChoice::LeaveGroup => {
                        if row(ui, "\u{E7E8}", "Leave Group", true, false, None) {
                            acts.push(Action::AskLeaveGroup(id.clone()));
                            done = true;
                        }
                    }
                    MenuChoice::CloseDm => {
                        if row(ui, "\u{E711}", "Close DM", false, false, None) {
                            acts.push(Action::CloseDm(id.clone()));
                            done = true;
                        }
                    }
                    MenuChoice::CopyLink => {
                        if row(ui, "\u{E71B}", "Copy Link", false, false, None) {
                            ui.ctx().copy_text(network_link(&id));
                            done = true;
                        }
                    }
                    MenuChoice::CopyId => {
                        if row(ui, "\u{E8EC}", "Copy ID", false, false, None) {
                            ui.ctx().copy_text(network_id(&id).to_owned());
                            done = true;
                        }
                    }
                    _ => {}
                }
            }
        });
    });
    let _ = k_raised;
    // closes on a click outside it (any button, though a new right click just moves it) and on scrolling
    let rect = shown.response.rect;
    let mut sub_rect = None;
    if let (Some(sub), Some(at)) = (st.sub, sub_at) {
        let r = flyout(ctx, key().with(sub as u8), at, rect, |ui| match sub {
            Sub::Mute => {
                for m in MUTE_TIMES {
                    if sub_row(ui, &mute_text(m), "", false) {
                        acts.push(if dm { Action::MuteDm { room: id.clone(), muting: m } } else { Action::ChangeNotifications { place: id.clone(), change: NotificationChange::Mute(m) } });
                        done = true;
                    }
                }
            }
            Sub::Levels => {
                if server {
                    if let Some(sn) = snotes {
                        let current = sn.own.level.unwrap_or(sn.own.inherited);
                        for l in [NotifyLevel::All, NotifyLevel::Mentions, NotifyLevel::Nothing] {
                            if sub_row(ui, level_text(l), "", l == current) {
                                acts.push(Action::ChangeNotifications { place: id.clone(), change: NotificationChange::Level(Some(l)) });
                                done = true;
                            }
                        }
                        sep(ui);
                        if sub_row(ui, "Suppress @everyone and @here", "", sn.suppress_everyone) {
                            acts.push(Action::ChangeNotifications { place: id.clone(), change: NotificationChange::SuppressEveryone(!sn.suppress_everyone) });
                            done = true;
                        }
                        if sub_row(ui, "Suppress all role @mentions", "", sn.suppress_roles) {
                            acts.push(Action::ChangeNotifications { place: id.clone(), change: NotificationChange::SuppressRoles(!sn.suppress_roles) });
                            done = true;
                        }
                    }
                } else if let Some(n) = notes {
                    let default = if app.category_of(&id).is_some() { "Use category default" } else { "Use server default" };
                    if sub_row(ui, default, level_text(n.inherited), n.level.is_none()) {
                        acts.push(Action::ChangeNotifications { place: id.clone(), change: NotificationChange::Level(None) });
                        done = true;
                    }
                    for l in [NotifyLevel::All, NotifyLevel::Mentions, NotifyLevel::Nothing] {
                        if sub_row(ui, level_text(l), "", n.level == Some(l)) {
                            acts.push(Action::ChangeNotifications { place: id.clone(), change: NotificationChange::Level(Some(l)) });
                            done = true;
                        }
                    }
                }
            }
        });
        if left_sub(ctx, rect, at, r) {
            st.sub = None;
        }
        sub_rect = Some(r);
    }
    let clicked_away = ctx.input(|i| {
        let outside = i.pointer.interact_pos().is_none_or(|p| !rect.contains(p) && !sub_rect.is_some_and(|r| r.contains(p)));
        (i.pointer.any_pressed() && outside) || i.smooth_scroll_delta != egui::Vec2::ZERO || i.raw.events.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. }))
    });
    if done || clicked_away || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        close(ctx);
    } else {
        ctx.data_mut(|d| d.insert_temp(key(), st));
    }
}
