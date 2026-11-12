//! The conversation bar, as Discord and Yapfast show it (ui/yapfast.slint, ui/header.slint): avatar with status
//! dot and name for a DM, `#` and name for a channel; then call, video call, pinned messages, add friends,
//! members, profile and a search box. Also the panels they open: pins and add-people popovers, and the search
//! results, profile and member list docked on the right. Data and network work stay in the engine.

use super::voice::{glyph, k_edge, k_panel, QUICK, k_raised};
use super::*;
use egui::{pos2, Rect, Stroke};
use mytune_chat::yapfast_core::chat::{ChatRoom, Presence};
use mytune_chat::yapfast_core::voice::{Command as Vc, Phase};
use std::time::Instant;

// Segoe Fluent Icons glyphs.
const I_PHONE: &str = "\u{E717}";
const I_VIDEO: &str = "\u{E714}";
const I_PIN: &str = "\u{E718}";
const I_ADD_FRIEND: &str = "\u{E8FA}";
const I_PEOPLE: &str = "\u{E716}";
const I_PROFILE: &str = "\u{E77B}";
const I_SEARCH: &str = "\u{E721}";
const I_CLOSE: &str = "\u{E711}";
const I_CHECK: &str = "\u{E73E}";
const IDLE: Color32 = Color32::from_rgb(0xf0, 0xb2, 0x32);
const PANEL_W: f32 = 300.0;

/// What the bar keeps between frames.
pub struct HeaderUi {
    search: String,
    /// The member list beside a channel (a layout choice in Yapfast, not engine state).
    pub members_open: bool,
    /// A video call asked for: the camera turns on once that call connects.
    video_pending: Option<(String, Instant)>,
    add_query: String,
    pin_button: Rect,
    pub(super) thread_button: Rect,
    add_button: Rect,
    /// The last of Search and Aside shown, which share the wide side panel, so it can slide out with its content.
    pub wide: Side,
}

impl Default for HeaderUi {
    fn default() -> Self {
        Self { search: String::new(), members_open: true, video_pending: None, add_query: String::new(), pin_button: Rect::NOTHING, thread_button: Rect::NOTHING, add_button: Rect::NOTHING, wide: Side::Search }
    }
}

pub fn presence_color(p: Option<Presence>) -> Color32 {
    match p {
        Some(Presence::Online | Presence::Mobile) => LIVE,
        Some(Presence::Idle) => IDLE,
        Some(Presence::DoNotDisturb) => DANGER,
        _ => FAINT,
    }
}

/// A status dot at an avatar's bottom right, ringed in the background colour.
pub fn status_dot(ui: &Ui, avatar: Rect, p: Option<Presence>, ring: Color32) {
    status_dot_k(ui, avatar, p, ring, 0.14);
}

/// The same with the dot's radius as `k` times the avatar's width.
pub fn status_dot_k(ui: &Ui, avatar: Rect, p: Option<Presence>, ring: Color32, k: f32) {
    let r = (avatar.width() * k).max(3.5);
    let c = avatar.right_bottom() - vec2(r * 0.8, r * 0.8);
    th::dot(ui.painter(), c, r + 2.0, ring);
    th::dot(ui.painter(), c, r, presence_color(p));
}

/// A header icon button: no fill until hovered, brighter when its panel is open.
fn hbtn(ui: &mut Ui, g: &str, active: bool, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), QUICK);
    if t > 0.0 {
        ui.painter().rect_filled(rect, 6.0, k_hover().gamma_multiply(t));
    }
    let c = if active { TEXT } else { SECONDARY.lerp_to_gamma(TEXT, t) };
    glyph(ui.painter(), rect.center(), g, 18.0, c, false);
    resp.on_hover_text(tip)
}

/// The bar for the open room. Called inside the header panel.
#[allow(clippy::too_many_arguments)]
pub fn bar(ui: &mut Ui, h: &mut HeaderUi, s: &AppState, r: &ChatRoom, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    let v = &s.voice;
    let here = v.active() && v.room.as_ref() == Some(&r.id);
    // a video call turns the camera on once it connects (20 s at most)
    if let Some((room, at)) = &h.video_pending {
        if v.room.as_ref() == Some(room) && v.phase == Phase::Connected {
            if v.can_film {
                acts.push(Action::Voice(Vc::Video(true)));
            }
            h.video_pending = None;
        } else if at.elapsed().as_secs() > 20 {
            h.video_pending = None;
        }
    }
    ui.horizontal(|ui| {
        ui.set_min_height(32.0);
        if r.direct {
            let (a, _) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::hover());
            let t = texture(&ctx, tex, asked, s, &r.avatar, want);
            avatar(ui, a, t, &r.name);
            if r.person.is_some() {
                status_dot(ui, a, r.presence, k_chat());
            }
            ui.add_space(4.0);
        } else {
            ui.label(rt(if r.voice { "\u{266A}" } else { "#" }, 18.0).color(MUTED));
        }
        ui.label(sb(tidy(&r.name), 16.0).color(TEXT));
        if let Some(act) = r.activity.as_ref().filter(|_| r.direct) {
            ui.add(egui::Label::new(rt(act, 12.5).color(MUTED)).truncate());
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if r.capabilities.search && !r.voice {
                search_box(ui, h, s, r, acts);
                ui.add_space(4.0);
            }
            if s.one_to_one().is_some_and(|o| o.id == r.id) && hbtn(ui, I_PROFILE, s.profile_panel, if s.profile_panel { "Hide profile" } else { "Show profile" }).clicked() {
                acts.push(Action::ShowProfilePanel(!s.profile_panel));
            }
            if s.lists_members() && hbtn(ui, I_PEOPLE, h.members_open, if h.members_open { "Hide member list" } else { "Show member list" }).clicked() {
                h.members_open = !h.members_open;
            }
            if r.direct && r.capabilities.group_dms {
                let tip = if r.person.is_some() { "Add friends to DM" } else { "Add friends to the group" };
                let open = s.add_people.is_some();
                let b = hbtn(ui, I_ADD_FRIEND, open, tip);
                h.add_button = b.rect;
                if b.clicked() {
                    acts.push(if open { Action::CloseAddPeople } else { Action::OpenAddPeople });
                    h.add_query.clear();
                }
            }
            if !r.direct && !r.voice && s.threads_channel().is_some_and(|c| c == r.id) {
                let open = s.threads.is_some();
                let b = hbtn(ui, "\u{E8BD}", open, "Threads");
                h.thread_button = b.rect;
                if b.clicked() {
                    acts.push(Action::ShowThreads(!open));
                }
            }
            if r.capabilities.pins && !r.voice {
                let open = s.pins.as_ref().is_some_and(|p| p.room == r.id);
                let b = hbtn(ui, I_PIN, open, "Pinned messages");
                h.pin_button = b.rect;
                if b.clicked() {
                    acts.push(Action::ShowPins(!open));
                }
            }
            if r.direct && !here && s.can_join_voice(&r.id) {
                if hbtn(ui, I_VIDEO, false, "Start a video call").clicked() {
                    acts.push(Action::RequestJoinVoice(r.id.clone()));
                    h.video_pending = Some((r.id.clone(), Instant::now()));
                }
                let tip = if r.people.is_empty() { "Start a call" } else { "Join the call" };
                if hbtn(ui, I_PHONE, false, tip).clicked() {
                    acts.push(Action::RequestJoinVoice(r.id.clone()));
                }
            } else if r.voice && !here && s.can_join_voice(&r.id) && pill(ui, "Join voice", true).clicked() {
                acts.push(Action::RequestJoinVoice(r.id.clone()));
            }
        });
    });
}

/// The search field: Enter searches the DM (or the channel's server), the cross clears it.
fn search_box(ui: &mut Ui, h: &mut HeaderUi, s: &AppState, r: &ChatRoom, acts: &mut Vec<Action>) {
    if s.search.is_none() && !h.search.is_empty() && !ui.memory(|m| m.has_focus(egui::Id::new("chat_search"))) {
        h.search.clear(); // the panel was closed
    }
    egui::Frame::new().fill(k_rail()).corner_radius(6.0).inner_margin(egui::Margin { left: 8, right: 6, top: 4, bottom: 4 }).show(ui, |ui| {
        ui.set_width(210.0);
        ui.horizontal(|ui| {
            let hint = if r.direct { format!("Search {}", r.name) } else { "Search".into() };
            let te = ui.add(egui::TextEdit::singleline(&mut h.search).id(egui::Id::new("chat_search")).frame(egui::Frame::NONE).desired_width(170.0).hint_text(rt(hint, 13.0).color(FAINT)).font(egui::FontId::proportional(13.0)));
            if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                acts.push(Action::Search(h.search.trim().to_owned()));
            }
            let (ir, ires) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::click());
            let clear = !h.search.is_empty();
            glyph(ui.painter(), ir.center(), if clear { I_CLOSE } else { I_SEARCH }, 12.0, MUTED, false);
            if clear && ires.clicked() {
                h.search.clear();
                acts.push(Action::Search(String::new()));
            }
        });
    });
}

/// A found or pinned message, as Yapfast's FoundCard: avatar, author and time, the text.
fn message_card(ui: &mut Ui, s: &AppState, m: &mytune_chat::yapfast_core::chat::ChatMessage, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>) -> bool {
    let ctx = ui.ctx().clone();
    let bg = ui.painter().add(egui::Shape::Noop);
    let inner = egui::Frame::new().inner_margin(egui::Margin::same(10)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            let (a, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
            let t = texture(&ctx, tex, asked, s, &m.avatar, want);
            avatar(ui, a, t, &m.author);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.horizontal(|ui| {
                    let c = m.color.map(|c| Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)).unwrap_or(TEXT);
                    ui.label(sb(&m.author, 14.0).color(c));
                    ui.label(rt(&m.time, 11.0).color(MUTED));
                });
                let body: String = m.body.chars().take(400).collect();
                ui.add(egui::Label::new(rt(body, 13.5).color(SECONDARY)).wrap());
            });
        });
    });
    let resp = ui.interact(inner.response.rect, ui.id().with(("card", &m.item)), Sense::click());
    let t = ctx.animate_bool_with_time(resp.id, resp.hovered(), QUICK);
    ui.painter().set(bg, egui::Shape::rect_filled(inner.response.rect, 8.0, k_raised().lerp_to_gamma(k_active(), t)));
    ui.add_space(6.0);
    resp.on_hover_text("Jump to message").clicked()
}

/// A popover under a header button; closes on Escape or a click elsewhere. Returns true when it should close.
pub(super) fn popover(ctx: &egui::Context, id: &str, under: Rect, width: f32, content: impl FnOnce(&mut Ui)) -> bool {
    let x = (under.right() - width).max(8.0);
    let resp = egui::Area::new(egui::Id::new(id)).order(egui::Order::Foreground).fixed_pos(pos2(x, under.bottom() + 8.0)).show(ctx, |ui| {
        egui::Frame::new()
            .fill(k_panel())
            .stroke(Stroke::new(1.0, k_edge()))
            .corner_radius(10.0)
            .shadow(egui::Shadow { offset: [0, 10], blur: 28, spread: 0, color: Color32::from_black_alpha(0x66) })
            .inner_margin(egui::Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(width - 24.0);
                content(ui);
            });
    });
    let outside = resp.response.clicked_elsewhere() && !ctx.input(|i| i.pointer.interact_pos().is_some_and(|p| under.contains(p)));
    outside || ctx.input(|i| i.key_pressed(egui::Key::Escape))
}

/// The pinned messages popover (420 px), newest first; a click jumps to the message.
pub fn pins(ctx: &egui::Context, h: &HeaderUi, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let Some(p) = &s.pins else { return };
    if s.selected.as_ref() != Some(&p.room) {
        return;
    }
    let close = popover(ctx, "chat_pins", h.pin_button, 420.0, |ui| {
        ui.label(sb("Pinned Messages", 15.0).color(TEXT));
        ui.add_space(8.0);
        match &p.messages {
            None => {
                ui.label(rt("Loading\u{2026}", 13.0).color(MUTED));
            }
            Some(Err(e)) => {
                ui.label(rt(e, 13.0).color(DANGER));
            }
            Some(Ok(list)) if list.is_empty() => {
                ui.label(rt("This conversation has no pinned messages yet.", 13.0).color(MUTED));
            }
            Some(Ok(list)) => {
                egui::ScrollArea::vertical().max_height(520.0).show(ui, |ui| {
                    for m in list {
                        if message_card(ui, s, m, tex, asked, want) {
                            acts.push(Action::Jump(m.item.clone()));
                            acts.push(Action::ShowPins(false));
                        }
                    }
                });
            }
        }
    });
    if close {
        acts.push(Action::ShowPins(false));
    }
}

/// The add-friends popover (440 px): who may still join, a search, friends with a tick, then Add.
pub fn add_people(ctx: &egui::Context, h: &mut HeaderUi, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let Some(ap) = &s.add_people else { return };
    let one_to_one = s.rooms.iter().any(|r| r.id == ap.room && r.person.is_some());
    let close = popover(ctx, "chat_add_people", h.add_button, 440.0, |ui| {
        ui.label(sb("Select Friends", 15.0).color(TEXT));
        let left = s.group_room().saturating_sub(ap.chosen.len());
        ui.label(rt(format!("You can add {left} more friend{}.", if left == 1 { "" } else { "s" }), 12.5).color(MUTED));
        ui.add_space(8.0);
        egui::Frame::new().fill(k_rail()).corner_radius(6.0).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
            let te = ui.add(egui::TextEdit::singleline(&mut h.add_query).frame(egui::Frame::NONE).desired_width(f32::INFINITY).hint_text(rt("Type the username of a friend", 13.0).color(FAINT)));
            if te.changed() {
                acts.push(Action::AddPeopleQuery(h.add_query.clone()));
            }
        });
        ui.add_space(6.0);
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            let friends = s.addable_friends();
            if friends.is_empty() {
                ui.label(rt("No friends to add.", 13.0).color(MUTED));
            }
            for f in friends {
                let chosen = ap.chosen.contains(&f.id);
                let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
                let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), QUICK);
                if t > 0.0 {
                    ui.painter().rect_filled(row, 6.0, k_hover().gamma_multiply(t));
                }
                let a = Rect::from_min_size(pos2(row.left() + 8.0, row.center().y - 16.0), vec2(32.0, 32.0));
                let tx = texture(ui.ctx(), tex, asked, s, &f.avatar, want);
                avatar(ui, a, tx, &f.name);
                status_dot(ui, a, f.presence, k_panel());
                ui.painter().text(pos2(a.right() + 10.0, row.center().y), egui::Align2::LEFT_CENTER, &f.name, egui::FontId::proportional(14.0), TEXT);
                let b = Rect::from_center_size(pos2(row.right() - 18.0, row.center().y), vec2(20.0, 20.0));
                ui.painter().rect(b, 5.0, if chosen { p().accent } else { Color32::TRANSPARENT }, Stroke::new(1.5, if chosen { p().accent } else { MUTED }), egui::StrokeKind::Inside);
                if chosen {
                    glyph(ui.painter(), b.center(), I_CHECK, 12.0, p().on_accent, false);
                }
                if resp.clicked() {
                    acts.push(Action::ChoosePerson(f.id.clone()));
                }
            }
        });
        if let Some(e) = &ap.error {
            ui.label(rt(e, 12.5).color(DANGER));
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let label = if one_to_one { "Create Group DM" } else { "Add" };
                ui.add_enabled_ui(!ap.chosen.is_empty() && !ap.adding, |ui| {
                    if pill(ui, label, true).clicked() {
                        acts.push(Action::AddPeople);
                    }
                });
                if pill(ui, "Cancel", false).clicked() {
                    acts.push(Action::CloseAddPeople);
                }
            });
        });
    });
    if close {
        acts.push(Action::CloseAddPeople);
    }
}

/// Which panel docks on the right for the open room: search results, the DM's profile, or the member list.
pub fn side_panel(h: &HeaderUi, s: &AppState) -> Option<Side> {
    if s.search.is_some() {
        Some(Side::Search)
    } else if s.aside.is_some() {
        Some(Side::Aside)
    } else if s.profile_panel && s.one_to_one().is_some() && s.panel_profile.is_some() {
        Some(Side::Profile)
    } else if h.members_open && s.lists_members() {
        Some(Side::Members)
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Side {
    Search,
    /// A thread or forum post beside its channel.
    Aside,
    Profile,
    Members,
}

impl Side {
    pub fn width(self) -> f32 {
        match self {
            Side::Search | Side::Aside => 420.0,
            Side::Profile => PANEL_W,
            Side::Members => 240.0,
        }
    }
}

/// The search results panel: count, found messages (a server search heads each channel's run), Show more.
pub fn search_panel(ui: &mut Ui, h: &mut HeaderUi, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let Some(se) = &s.search else { return };
    ui.horizontal(|ui| {
        let title = if se.loading && se.results.is_empty() { "Searching\u{2026}".to_owned() } else { format!("{} Result{}", se.total, if se.total == 1 { "" } else { "s" }) };
        ui.label(sb(title, 15.0).color(TEXT));
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if hbtn(ui, I_CLOSE, false, "Close").clicked() {
                h.search.clear();
                acts.push(Action::Search(String::new()));
            }
        });
    });
    ui.add_space(8.0);
    if let Some(e) = &se.error {
        ui.label(rt(e, 13.0).color(DANGER));
    } else if !se.loading && se.results.is_empty() {
        ui.label(rt(format!("Nothing here matches \u{201C}{}\u{201D}.", se.query), 13.0).color(MUTED));
    }
    let server = s.rooms.iter().all(|r| r.id != se.place);
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        let mut last = "";
        for hit in &se.results {
            if server && hit.room != last {
                last = &hit.room;
                let name = s.rooms.iter().find(|r| r.id == hit.room).map(|r| r.name.as_str()).unwrap_or("");
                ui.label(rt(format!("# {name}"), 12.0).color(MUTED));
                ui.add_space(4.0);
            }
            if message_card(ui, s, &hit.message, tex, asked, want) {
                acts.push(Action::JumpInRoom { room: hit.room.clone(), item: hit.message.item.clone() });
            }
        }
        if se.loading && !se.results.is_empty() {
            ui.label(rt("Loading\u{2026}", 13.0).color(MUTED));
        } else if se.more() && pill(ui, "Show more", false).clicked() {
            acts.push(Action::MoreResults);
        }
    });
}

/// The DM's profile beside the chat: banner, avatar with status, name and handle, then About me, Member since,
/// mutual servers and mutual friends.
pub fn profile_panel(ui: &mut Ui, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>) {
    if let Some(pv) = &s.panel_profile {
        profile_view(ui, s, pv, tex, asked, want);
    }
}

/// Where a profile card opens: where its name or picture was clicked.
pub fn remember_profile_at(ctx: &egui::Context) {
    if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("profile_at"), p));
    }
}

/// A member's profile card, opened by clicking their name or picture: the profile, then Send Message.
/// Closes on Escape or a click elsewhere.
pub fn profile_card(ctx: &egui::Context, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let Some(pv) = &s.profile else { return };
    let screen = ctx.content_rect();
    let at = ctx.data(|d| d.get_temp::<egui::Pos2>(egui::Id::new("profile_at"))).unwrap_or(screen.center());
    // the full profile (View Full Profile) is a larger card in the middle of the window
    let (w, h) = if pv.large { (520.0, 620.0) } else { (300.0, 480.0) };
    let pos = if pv.large {
        screen.center() - vec2(w, h) / 2.0
    } else {
        pos2((at.x + 12.0).min(screen.right() - w - 8.0), (at.y - 40.0).clamp(screen.top() + 8.0, (screen.bottom() - h - 8.0).max(screen.top() + 8.0)))
    };
    let resp = egui::Area::new(egui::Id::new(("profile_card", pv.large))).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        egui::Frame::new()
            .fill(k_sidebar())
            .stroke(Stroke::new(1.0, k_edge()))
            .corner_radius(10.0)
            .shadow(egui::Shadow { offset: [0, 10], blur: 28, spread: 0, color: Color32::from_black_alpha(0x66) })
            .show(ui, |ui| {
                ui.set_width(w);
                ui.set_max_height(h);
                profile_view(ui, s, pv, tex, asked, want);
                if s.discord_user.as_ref() != Some(&pv.profile.id) {
                    let user = pv.profile.id.clone();
                    egui::Frame::new().inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                        // Discord's "Message @name" field: Enter sends it in their DM, which opens
                        let tid = egui::Id::new(("profile_msg", &user));
                        let mut text: String = ui.ctx().data(|d| d.get_temp(tid)).unwrap_or_default();
                        let te = ui.add(egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY).hint_text(rt(format!("Message @{}", pv.profile.name), 13.5).color(FAINT)));
                        if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !text.trim().is_empty() {
                            acts.push(Action::MessageFromProfile { user: user.clone(), text: std::mem::take(&mut text) });
                            acts.push(Action::CloseProfile);
                        }
                        ui.ctx().data_mut(|d| d.insert_temp(tid, text));
                        ui.add_space(8.0);
                        ui.horizontal_wrapped(|ui| {
                            if pill(ui, "Message", true).clicked() {
                                acts.push(Action::MessagePerson(user.clone()));
                                acts.push(Action::CloseProfile);
                            }
                            if pill(ui, "Call", false).clicked() {
                                acts.push(Action::CallPerson(user.clone()));
                                acts.push(Action::CloseProfile);
                            }
                            if s.selected_room().is_some() && pill(ui, "Mention", false).on_hover_text("Put @name in the message bar").clicked() {
                                acts.push(Action::MentionPerson(user.clone()));
                                acts.push(Action::CloseProfile);
                                ui.ctx().memory_mut(|m| m.request_focus(super::compose::edit_id()));
                            }
                            if !pv.large && pill(ui, "Full profile", false).clicked() {
                                acts.push(Action::EnlargeProfile);
                            }
                        });
                    });
                }
            });
    });
    if resp.response.clicked_elsewhere() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        acts.push(Action::CloseProfile);
    }
}

fn profile_view(ui: &mut Ui, s: &AppState, pv: &mytune_chat::yapfast_core::application::ProfileView, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>) {
    let pr = &pv.profile;
    let ctx = ui.ctx().clone();
    let w = ui.available_width();
    let (banner, _) = ui.allocate_exact_size(vec2(w, 106.0), Sense::hover());
    let accent = pr.accent.map(|c| Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)).unwrap_or(k_raised());
    match texture(&ctx, tex, asked, s, &pr.banner, want) {
        Some(t) => egui::Image::new(egui::load::SizedTexture::new(t, banner.size())).paint_at(ui, banner),
        None => {
            ui.painter().rect_filled(banner, 10.0, accent);
        }
    }
    let a = Rect::from_min_size(pos2(banner.left() + 16.0, banner.bottom() - 40.0), vec2(80.0, 80.0));
    ui.painter().circle_filled(a.center(), 46.0, k_sidebar());
    let t = texture(&ctx, tex, asked, s, &pr.avatar, want);
    avatar(ui, a, t, &pr.name);
    status_dot(ui, a, pr.presence, k_sidebar());
    ui.add_space(48.0);
    egui::ScrollArea::vertical().auto_shrink([false, true]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 0)).show(ui, |ui| {
            ui.label(sb(&pr.name, 20.0).color(TEXT));
            let handle = if pr.pronouns.is_empty() { pr.handle.clone() } else { format!("{} \u{00B7} {}", pr.handle, pr.pronouns) };
            ui.label(rt(handle, 13.5).color(SECONDARY));
            if let Some(act) = &pr.activity {
                ui.add_space(4.0);
                ui.add(egui::Label::new(rt(act, 13.0).color(MUTED)).wrap());
            }
            ui.add_space(12.0);
            egui::Frame::new().fill(k_raised()).corner_radius(8.0).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                if !pr.bio.is_empty() {
                    ui.label(sb("About Me", 12.5).color(TEXT));
                    ui.add(egui::Label::new(rt(&pr.bio, 13.0).color(SECONDARY)).wrap());
                    ui.add_space(10.0);
                }
                if let Some(c) = &pr.created {
                    ui.label(sb("Member Since", 12.5).color(TEXT));
                    ui.label(rt(c, 13.0).color(SECONDARY));
                }
                if pv.loading {
                    ui.add_space(6.0);
                    ui.label(rt("Loading the rest of the profile\u{2026}", 12.0).color(MUTED));
                }
                if let Some(n) = &pv.note {
                    ui.label(rt(n, 12.0).color(MUTED));
                }
            });
            let mut list = |ui: &mut Ui, id: &str, title: String, rows: Vec<(&str, &Option<String>)>| {
                if rows.is_empty() {
                    return;
                }
                ui.add_space(10.0);
                egui::CollapsingHeader::new(rt(title, 12.5).color(TEXT)).id_salt(id).default_open(false).show(ui, |ui| {
                    for (name, pic) in rows {
                        ui.horizontal(|ui| {
                            let (a, _) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::hover());
                            let t = texture(&ctx, tex, asked, s, pic, want);
                            avatar(ui, a, t, name);
                            ui.label(rt(name, 13.5).color(SECONDARY));
                        });
                    }
                });
            };
            list(ui, "servers", format!("Mutual Servers \u{2014} {}", pr.servers.len()), pr.servers.iter().map(|x| (x.name.as_str(), &x.icon)).collect());
            list(ui, "friends", format!("Mutual Friends \u{2014} {}", pr.friends.len()), pr.friends.iter().map(|x| (x.name.as_str(), &x.avatar)).collect());
        });
    });
}

/// The channel's member list: Discord's group headings ("Online — 12"), then each member with a status dot,
/// their role colour and what they are doing.
pub fn members_panel(ui: &mut Ui, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let list = match &s.members {
        None => {
            ui.label(rt("Loading members\u{2026}", 13.0).color(MUTED));
            return;
        }
        Some(Err(e)) => {
            ui.label(rt(*e, 13.0).color(MUTED));
            return;
        }
        Some(Ok(l)) => l,
    };
    let ctx = ui.ctx().clone();
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        let mut last: Option<&str> = None;
        // shortcut: draws every member each frame; switch to show_rows if lists past ~1000 get slow.
        for m in list.iter() {
            if m.group.as_deref() != last {
                last = m.group.as_deref();
                if let Some(g) = last {
                    ui.add_space(12.0);
                    ui.label(rt(g.to_uppercase(), 11.0).color(MUTED));
                    ui.add_space(2.0);
                }
            }
            let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::click());
            if resp.clicked() {
                remember_profile_at(&ctx);
                acts.push(Action::ShowProfile { user: m.id.clone(), large: false });
            }
            let t = ctx.animate_bool_with_time(resp.id, resp.hovered(), QUICK);
            if t > 0.0 {
                ui.painter().rect_filled(row, 6.0, k_hover().gamma_multiply(t));
            }
            let a = Rect::from_min_size(pos2(row.left() + 6.0, row.center().y - 16.0), vec2(32.0, 32.0));
            let tx = texture(&ctx, tex, asked, s, &m.avatar, want);
            avatar(ui, a, tx, &m.name);
            status_dot(ui, a, m.presence, k_sidebar());
            let offline = matches!(m.presence, None | Some(Presence::Offline));
            let c = m.color.map(|c| Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)).unwrap_or(SECONDARY);
            let c = if offline { c.gamma_multiply(0.5) } else { c };
            let x = a.right() + 10.0;
            let clip = ui.painter().with_clip_rect(Rect::from_min_max(pos2(x, row.top()), pos2(row.right() - 4.0, row.bottom())));
            match &m.activity {
                Some(act) => {
                    clip.text(pos2(x, row.center().y - 8.0), egui::Align2::LEFT_CENTER, &m.name, egui::FontId::proportional(14.0), c);
                    clip.text(pos2(x, row.center().y + 9.0), egui::Align2::LEFT_CENTER, act, egui::FontId::proportional(11.5), MUTED);
                }
                None => {
                    clip.text(pos2(x, row.center().y), egui::Align2::LEFT_CENTER, &m.name, egui::FontId::proportional(14.0), c);
                }
            }
        }
    });
}
