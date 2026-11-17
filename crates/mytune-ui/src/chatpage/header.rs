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
pub fn status_dot_k(ui: &Ui, avatar: Rect, p: Option<Presence>, _ring: Color32, k: f32) {
    let r = (avatar.width() * k).max(3.5);
    let c = avatar.right_bottom() - vec2(r * 0.8, r * 0.8);
    // one nearly opaque ring everywhere: the row colour passed in is see-through and changes on hover
    th::dot(ui.painter(), c, r + 2.0, Color32::from_black_alpha(0xE0));
    badge(ui.painter(), c, r, p);
}

/// Discord's status shape of radius `r` at `c`: a dot for online, a moon for Idle, a circle with a minus for
/// Do Not Disturb, a ring for offline and invisible. The cut-outs are real holes (a mask picture tinted with
/// the status colour): painting them in a "background" colour showed as a dark blob on see-through glass.
pub fn badge(p: &egui::Painter, c: egui::Pos2, r: f32, presence: Option<Presence>) {
    let color = presence_color(presence);
    let kind = match presence {
        Some(Presence::Online | Presence::Mobile) => return th::dot(p, c, r, color),
        Some(Presence::Idle) => 0,
        Some(Presence::DoNotDisturb) => 1,
        _ => 2,
    };
    let ctx = p.ctx();
    let id = egui::Id::new(("status-mask", kind));
    let tex = ctx.data(|d| d.get_temp::<TextureHandle>(id)).unwrap_or_else(|| {
        let t = ctx.load_texture(format!("status-mask-{kind}"), mask(kind), th::SMOOTH);
        ctx.data_mut(|d| d.insert_temp(id, t.clone()));
        t
    });
    // whole pixels, centred on a pixel corner, so the small shape stays sharp
    let ppp = p.pixels_per_point();
    let d = (2.0 * r * ppp / MASK_DISC).round() / ppp;
    let c = egui::pos2((c.x * ppp).round() / ppp, (c.y * ppp).round() / ppp);
    p.image(tex.id(), Rect::from_center_size(c, vec2(d, d)), Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), color);
}

/// Side of a mask picture in pixels, and the share of it the disc fills (the rest is a clear margin).
const MASK_PX: usize = 64;
const MASK_DISC: f32 = 0.9;

/// How much of the point (x, y) the shape covers, in units of the disc's radius: 0 moon, 1 minus, 2 ring.
fn in_shape(kind: u8, x: f32, y: f32) -> bool {
    if x * x + y * y > 1.0 {
        return false;
    }
    match kind {
        0 => (x + 0.45).hypot(y + 0.45) > 0.78,                 // a bite out of the top left
        1 => (x.abs() - 0.4).max(0.0).hypot(y) > 0.2,           // a bar with round ends
        _ => x.hypot(y) > 0.5,                                  // a hole in the middle
    }
}

/// A white picture of the shape whose alpha is its coverage (4x4 samples per pixel).
fn mask(kind: u8) -> egui::ColorImage {
    let n = MASK_PX;
    let mut px = Vec::with_capacity(n * n);
    for j in 0..n {
        for i in 0..n {
            let mut hit = 0;
            for sj in 0..4 {
                for si in 0..4 {
                    let u = |v: usize, s: usize| ((v as f32 + (s as f32 + 0.5) / 4.0) / n as f32 * 2.0 - 1.0) / MASK_DISC;
                    hit += in_shape(kind, u(i, si), u(j, sj)) as u32;
                }
            }
            let a = (hit * 255 / 16) as u8;
            px.push(Color32::from_rgba_premultiplied(a, a, a, a));
        }
    }
    egui::ColorImage::new([n, n], px)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_shapes_have_real_holes() {
        // moon: the top left is bitten out, the bottom right is there
        assert!(!in_shape(0, -0.5, -0.5) && in_shape(0, 0.5, 0.5));
        // minus: the middle is open, above it is filled
        assert!(!in_shape(1, 0.0, 0.0) && in_shape(1, 0.0, 0.6) && in_shape(1, 0.8, 0.0));
        // ring: the middle is open, the rim is filled
        assert!(!in_shape(2, 0.0, 0.0) && in_shape(2, 0.8, 0.0));
        // nothing outside the disc, and the picture's corner is clear
        assert!((0..3).all(|k| !in_shape(k, 0.9, 0.9)));
        assert_eq!(mask(0).pixels[0].a(), 0);
    }
}

/// A header icon button: no fill until hovered, brighter when its panel is open.
fn hbtn(ui: &mut Ui, g: &str, active: bool, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), QUICK);
    if t > 0.0 {
        ui.painter().rect_filled(rect, 10.0, k_hover().gamma_multiply(t));
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
        // the buttons are laid out first, from the right, so a long name is cut short instead of running under them
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            if r.capabilities.search && !r.voice {
                search_box(ui, h, s, r, acts);
                ui.add_space(4.0);
            }
            // one switch for the right-hand panel in every chat: the profile in a DM, the member list in a group or
            // server; turning it on or off in one chat does it in all of them
            let mut flip = None;
            if s.one_to_one().is_some_and(|o| o.id == r.id) && hbtn(ui, I_PROFILE, s.profile_panel, if s.profile_panel { "Hide profile" } else { "Show profile" }).clicked() {
                flip = Some(!s.profile_panel);
            }
            if s.lists_members() && hbtn(ui, I_PEOPLE, h.members_open, if h.members_open { "Hide member list" } else { "Show member list" }).clicked() {
                flip = Some(!h.members_open);
            }
            if let Some(on) = flip {
                h.members_open = on;
                acts.push(Action::ShowProfilePanel(on));
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
            ui.add_space(8.0);
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let person = r.direct && r.person.is_some();
                if r.direct && r.person.is_none() && r.group_avatars.len() >= 2 {
                    // group DM: two members' pictures, as in the DM list
                    let (a, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
                    for (k, url) in r.group_avatars.iter().take(2).enumerate() {
                        let av = Rect::from_min_size(if k == 0 { a.min + vec2(10.0, 0.0) } else { a.min + vec2(0.0, 10.0) }, vec2(20.0, 20.0));
                        let t = texture(&ctx, tex, asked, s, &Some(url.clone()), want);
                        avatar(ui, av, t, &r.name);
                    }
                    ui.add_space(4.0);
                } else if r.direct {
                    let (a, pic) = ui.allocate_exact_size(vec2(26.0, 26.0), if person { Sense::click() } else { Sense::hover() });
                    let t = texture(&ctx, tex, asked, s, &r.avatar, want);
                    avatar(ui, a, t, &r.name);
                    if person {
                        status_dot(ui, a, r.presence, k_chat());
                        if pic.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            acts.push(Action::ShowDmProfile(r.id.clone()));
                        }
                    }
                    ui.add_space(4.0);
                } else {
                    ui.label(rt(if r.voice { "\u{266A}" } else { "#" }, 18.0).color(MUTED));
                }
                let name = egui::Label::new(sb(tidy(&r.name), 16.0).color(TEXT)).truncate();
                // a one-to-one DM: the picture and the name open the person's full profile
                if person {
                    if ui.add(name.sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        acts.push(Action::ShowDmProfile(r.id.clone()));
                    }
                } else {
                    ui.add(name);
                }
                if let Some(act) = r.activity.as_ref().filter(|_| r.direct) {
                    ui.add(egui::Label::new(rt(act, 12.5).color(MUTED)).truncate());
                }
            });
        });
    });
}

/// The search field: Enter searches the DM (or the channel's server), the cross clears it.
fn search_box(ui: &mut Ui, h: &mut HeaderUi, s: &AppState, r: &ChatRoom, acts: &mut Vec<Action>) {
    if s.search.is_none() && !h.search.is_empty() && !ui.memory(|m| m.has_focus(egui::Id::new("chat_search"))) {
        h.search.clear(); // the panel was closed
    }
    egui::Frame::new().fill(k_rail()).corner_radius(10.0).inner_margin(egui::Margin { left: 8, right: 6, top: 4, bottom: 4 }).show(ui, |ui| {
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
    ui.painter().set(bg, egui::Shape::rect_filled(inner.response.rect, 12.0, k_raised().lerp_to_gamma(k_active(), t)));
    ui.add_space(6.0);
    resp.on_hover_text("Jump to message").clicked()
}

/// A popover under a header button; closes on Escape or a click elsewhere. Returns true when it should close.
pub(super) fn popover(ctx: &egui::Context, id: &str, under: Rect, width: f32, content: impl FnOnce(&mut Ui)) -> bool {
    let x = (under.right() - width).max(8.0);
    let resp = egui::Area::new(egui::Id::new(id)).order(egui::Order::Foreground).fixed_pos(pos2(x, under.bottom() + 8.0)).show(ctx, |ui| {
        crate::theme::float(ui, crate::theme::R_MENU, 12, |ui| {
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
        egui::Frame::new().fill(k_rail()).corner_radius(10.0).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
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
                    ui.painter().rect_filled(row, 10.0, k_hover().gamma_multiply(t));
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
    } else if let Some(a) = &s.aside {
        Some(if a.chat { Side::Split } else { Side::Aside })
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
    /// A second channel or DM side by side with the open one.
    Split,
    Profile,
    Members,
}

impl Side {
    pub fn width(self) -> f32 {
        match self {
            Side::Search | Side::Aside => 420.0,
            // only names its panel id and saved width; it opens at half the chat area
            Side::Split => 600.0,
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

/// The DM's profile beside the chat, laid out like Discord's (see `small_profile`).
pub fn profile_panel(ui: &mut Ui, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    if let Some(pv) = &s.panel_profile {
        small_profile(ui, s, pv, false, tex, asked, want, acts);
    }
}

/// Where a profile card opens: where its name or picture was clicked.
pub fn remember_profile_at(ctx: &egui::Context) {
    if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("profile_at"), p));
    }
}

/// A member's profile card, opened by clicking their name or picture (`small_profile`), or the full profile in
/// the middle of the window (`full_profile`). Closes on Escape or a click elsewhere.
pub fn profile_card(ctx: &egui::Context, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    // the "..." menu is its own layer: a click in it must not count as a click outside the card
    let menu_open = ctx.data(|d| d.get_temp::<More>(egui::Id::new(MORE)).is_some());
    profile_menu(ctx, s, acts);
    // The card over the user bar (`settings::own_card`) loads this account's profile through the same engine
    // state and draws it itself: without this, a second card of the same profile opened in the middle. The
    // profile stays hidden until the engine drops it: that card closes a frame or more before the engine does,
    // and in that gap this card flashed up.
    let hid = egui::Id::new("own_card_hides_profile");
    let Some(pv) = &s.profile else {
        ctx.data_mut(|d| d.remove::<bool>(hid));
        return;
    };
    let own_card = ctx.data(|d| d.get_temp::<bool>(egui::Id::new("own_card"))).unwrap_or(false);
    let hidden = ctx.data(|d| d.get_temp::<bool>(hid)).unwrap_or(false);
    if !pv.large && Some(&pv.profile.id) == s.discord_user.as_ref() && (own_card || hidden) {
        ctx.data_mut(|d| d.insert_temp(hid, true));
        if !own_card {
            acts.push(Action::CloseProfile); // that card is gone and left its profile open
        }
        return;
    }
    ctx.data_mut(|d| d.remove::<bool>(hid));
    let screen = ctx.content_rect();
    let at = ctx.data(|d| d.get_temp::<egui::Pos2>(egui::Id::new("profile_at"))).unwrap_or(screen.center());
    let (w, h) = if pv.large { ((screen.width() - 64.0).min(940.0), (screen.height() - 64.0).min(660.0)) } else { (320.0_f32, (screen.height() - 16.0).min(560.0)) };
    let pos = if pv.large {
        (screen.center() - vec2(w, h) / 2.0).round()
    } else {
        pos2((at.x + 12.0).min(screen.right() - w - 8.0), (at.y - 40.0).clamp(screen.top() + 8.0, (screen.bottom() - h - 8.0).max(screen.top() + 8.0))).round()
    };
    let resp = egui::Area::new(egui::Id::new(("profile_card", pv.large))).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        egui::Frame::new()
            .fill(crate::theme::Glass::Float.fill())
            .stroke(Stroke::new(1.0, k_edge()))
            .corner_radius(if pv.large { 20.0 } else { 16.0 })
            .shadow(egui::Shadow { offset: [0, 10], blur: 28, spread: 0, color: Color32::from_black_alpha(0x66) })
            .show(ui, |ui| {
                if pv.large {
                    let (outer, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
                    full_profile(ui, outer, s, pv, tex, asked, want, acts);
                } else {
                    ui.set_width(w);
                    ui.set_max_height(h);
                    small_profile(ui, s, pv, true, tex, asked, want, acts);
                }
            });
    });
    // the enlarged picture and the menu sit over the card: their clicks and Escape are their own
    let viewing = ctx.data(|d| d.get_temp::<String>(egui::Id::new(PFP_VIEW)).is_some());
    if !viewing && !menu_open && (resp.response.clicked_elsewhere() || ctx.input(|i| i.key_pressed(egui::Key::Escape))) {
        acts.push(Action::CloseProfile);
    }
}

const PFP_VIEW: &str = "pfp_view";

/// A profile picture enlarged over the window, opened by clicking it in a profile. A click or Escape closes it.
pub fn pfp_viewer(ctx: &egui::Context, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>) {
    let id = egui::Id::new(PFP_VIEW);
    let Some(url) = ctx.data(|d| d.get_temp::<String>(id)) else { return };
    // Discord serves the same picture larger; the small one shows until it comes
    let big = Some(url.replace("size=128", "size=512"));
    let t = texture(ctx, tex, asked, s, &big, want).or_else(|| texture(ctx, tex, asked, s, &Some(url), want));
    let side = (ctx.content_rect().height() - 120.0).clamp(160.0, 420.0);
    let modal = egui::Modal::new(egui::Id::new("pfp_modal")).frame(egui::Frame::new()).show(ctx, |ui| {
        let (r, resp) = ui.allocate_exact_size(vec2(side, side), Sense::click());
        if let Some(t) = t {
            egui::Image::new(egui::load::SizedTexture::new(t, r.size())).corner_radius(24.0).paint_at(ui, r);
        }
        resp.clicked()
    });
    if modal.inner || modal.should_close() {
        ctx.data_mut(|d| d.remove::<String>(id));
    }
}

fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

type Profile = mytune_chat::yapfast_core::chat::Profile;
type ProfileView = mytune_chat::yapfast_core::application::ProfileView;
type Activity = mytune_chat::yapfast_core::chat::Activity;

/// The top of a profile, as in Discord: the banner (picture, else the profile colour), the avatar over its lower
/// edge with the status shape, and the custom status as a thought bubble beside it. `back` is the colour of the
/// surface under it (the ring round the avatar). Returns the banner's rectangle.
#[allow(clippy::too_many_arguments)]
fn profile_head(ui: &mut Ui, s: &AppState, pr: &Profile, bh: f32, av: f32, round: u8, back: Color32, view_pic: bool, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>) -> Rect {
    let ctx = ui.ctx().clone();
    let (banner, _) = ui.allocate_exact_size(vec2(ui.available_width(), bh), Sense::hover());
    let top = egui::CornerRadius { nw: round, ne: round, sw: 0, se: 0 };
    match texture(&ctx, tex, asked, s, &pr.banner, want) {
        Some(t) => egui::Image::new(egui::load::SizedTexture::new(t, banner.size())).corner_radius(top).paint_at(ui, banner),
        None => {
            ui.painter().rect_filled(banner, top, pr.accent.map(rgb).unwrap_or(th::tone(0.24)));
        }
    }
    let a = Rect::from_min_size(pos2(banner.left() + 16.0, banner.bottom() - av / 2.0), vec2(av, av));
    th::dot(ui.painter(), a.center(), av / 2.0 + 6.0, back);
    let t = texture(&ctx, tex, asked, s, &pr.avatar, want);
    avatar(ui, a, t, &pr.name);
    status_dot_k(ui, a, pr.presence, back, 0.13);
    if let Some(url) = pr.avatar.as_ref().filter(|_| view_pic) {
        if ui.interact(a, ui.id().with("pfp"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("View picture").clicked() {
            ctx.data_mut(|d| d.insert_temp(egui::Id::new(PFP_VIEW), url.clone()));
        }
    }
    if let Some(text) = pr.activity.as_deref().filter(|t| !t.trim().is_empty()) {
        let font = egui::FontId::proportional(13.0);
        let line = ellipsize(ui, text, &font, (banner.right() - a.right() - 46.0).max(40.0));
        let g = ui.painter().layout_no_wrap(line, font, TEXT);
        let b = Rect::from_min_size(pos2(a.right() + 12.0, banner.bottom() - 12.0), vec2(g.size().x + 26.0, 32.0));
        let paper = th::tone(0.2);
        th::dot(ui.painter(), b.left_top() + vec2(-5.0, -7.0), 3.0, paper);
        th::dot(ui.painter(), b.left_top() + vec2(6.0, -1.0), 6.0, paper);
        ui.painter().rect_filled(b, 16.0, paper);
        ui.painter().galley(pos2(b.left() + 13.0, b.center().y - g.size().y / 2.0), g, TEXT);
    }
    ui.add_space(av / 2.0 + 12.0);
    banner
}

/// A round button drawn on a banner.
/// One button of the full profile's row, as Discord's: all the same height and corners, so they line up.
/// With `text` it is the accent one (icon and words), without it a square icon button.
fn profile_btn(ui: &mut Ui, g: &str, text: Option<&str>, tip: &str) -> bool {
    const H: f32 = 34.0;
    let font = egui::FontId::new(13.5, th::semibold());
    let words = text.map(|t| ui.painter().layout_no_wrap(t.to_owned(), font, p().on_accent));
    let w = words.as_ref().map_or(H, |w| 12.0 + 16.0 + 7.0 + w.size().x + 14.0);
    let (r, resp) = ui.allocate_exact_size(vec2(w, H), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), QUICK);
    match words {
        Some(words) => {
            ui.painter().rect_filled(r, 8.0, p().accent.lerp_to_gamma(Color32::WHITE, 0.12 * t));
            glyph(ui.painter(), pos2(r.left() + 20.0, r.center().y), g, 15.0, p().on_accent, false);
            ui.painter().galley(pos2(r.left() + 35.0, r.center().y - words.size().y / 2.0), words, p().on_accent);
        }
        None => {
            ui.painter().rect_filled(r, 8.0, k_raised().lerp_to_gamma(Color32::from_white_alpha(0x2C), t));
            glyph(ui.painter(), r.center(), g, 15.0, TEXT, false);
        }
    }
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    if tip.is_empty() { resp.clicked() } else { resp.on_hover_text(tip).clicked() }
}

fn banner_btn(ui: &mut Ui, r: Rect, g: &str, tip: &str) -> bool {
    let resp = ui.interact(r, ui.id().with(("banner_btn", g)), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), QUICK);
    ui.painter().circle_filled(r.center(), r.width() / 2.0, Color32::from_black_alpha(0x8C).lerp_to_gamma(Color32::from_black_alpha(0xD0), t));
    glyph(ui.painter(), r.center(), g, 14.0, Color32::WHITE, false);
    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
}

/// The friend entry that fits how this account stands to `user`: add, accept, cancel or remove.
fn friend_act(s: &AppState, user: &str) -> Option<super::userbtns::UserAct> {
    super::userbtns::list(s, user).into_iter().find(|b| matches!(b.label, "Remove Friend" | "Accept Friend" | "Cancel Request" | "Add Friend"))
}

/// How long ago `start` (Unix ms) was, as Discord's running clock: 2:37:44 or 4:05.
fn elapsed(start: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(start);
    clock(now.saturating_sub(start) / 1000)
}

fn clock(secs: u64) -> String {
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

/// One activity as Discord's card: what kind, its picture, its name, its lines and the running clock.
#[allow(clippy::too_many_arguments)]
fn activity_card(ui: &mut Ui, s: &AppState, a: &Activity, side: f32, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>) {
    use mytune_chat::yapfast_core::chat::ActivityKind as K;
    let ctx = ui.ctx().clone();
    #[allow(unreachable_patterns)]
    let kind = match a.kind {
        K::Playing => "Playing",
        K::Streaming => "Streaming",
        K::Listening => "Listening to",
        K::Watching => "Watching",
        K::Competing => "Competing in",
        K::Hang => "In voice",
        _ => "Activity",
    };
    egui::Frame::new().fill(k_raised()).corner_radius(12.0).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(sb(kind, 12.0).color(TEXT));
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            let (r, _) = ui.allocate_exact_size(vec2(side, side), Sense::hover());
            let url = a.large_image.as_deref().filter(|u| u.starts_with("http")).map(str::to_owned);
            match texture(&ctx, tex, asked, s, &url, want) {
                Some(t) => egui::Image::new(egui::load::SizedTexture::new(t, r.size())).corner_radius(10.0).paint_at(ui, r),
                None => {
                    ui.painter().rect_filled(r, 10.0, Color32::from_white_alpha(0x14));
                    glyph(ui.painter(), r.center(), "\u{E7FC}", side * 0.4, MUTED, false);
                }
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 3.0;
                ui.add_space(((side - 60.0) / 2.0).max(0.0));
                ui.add(egui::Label::new(sb(&*a.name, 14.0).color(TEXT)).truncate());
                for line in [&a.details, &a.state].into_iter().flatten() {
                    ui.add(egui::Label::new(rt(&**line, 12.5).color(SECONDARY)).truncate());
                }
                if let Some(start) = a.start {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        let (g, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                        glyph(ui.painter(), g.center(), "\u{E7FC}", 16.0, LIVE, false);
                        ui.label(egui::RichText::new(elapsed(start)).font(egui::FontId::monospace(12.5)).color(LIVE));
                    });
                    ctx.request_repaint_after(std::time::Duration::from_secs(1));
                }
            });
        });
    });
}

/// The lines every profile has under its name: About me, Member Since, the server join date, roles, and what
/// Discord said when it gave no more.
fn profile_about(ui: &mut Ui, pv: &ProfileView) {
    let pr = &pv.profile;
    if !pr.bio.is_empty() {
        ui.add_space(12.0);
        ui.add(egui::Label::new(rt(&pr.bio, 13.0).color(SECONDARY)).wrap());
    }
    if pr.created.is_some() || pr.joined.is_some() {
        ui.add_space(14.0);
        ui.label(sb("Member Since", 12.0).color(TEXT));
        ui.add_space(2.0);
    }
    if let Some(c) = &pr.created {
        ui.label(rt(c, 13.0).color(SECONDARY));
    }
    if let Some(j) = &pr.joined {
        ui.label(rt(format!("Joined this server {j}"), 13.0).color(SECONDARY));
    }
    if !pr.roles.is_empty() {
        ui.add_space(14.0);
        ui.label(sb("Roles", 12.0).color(TEXT));
        ui.add_space(4.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for r in &pr.roles {
                egui::Frame::new().fill(Color32::from_white_alpha(18)).corner_radius(255.0).inner_margin(egui::Margin::symmetric(8, 3)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        let (d, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                        ui.painter().circle_filled(d.center(), 5.0, r.color.map(rgb).unwrap_or(MUTED));
                        ui.label(rt(&r.name, 12.0).color(SECONDARY));
                    });
                });
            }
        });
    }
    if pv.loading {
        ui.add_space(10.0);
        ui.label(rt("Loading the rest of the profile\u{2026}", 12.0).color(MUTED));
    }
    if let Some(n) = &pv.note {
        ui.add_space(6.0);
        ui.add(egui::Label::new(rt(n, 12.0).color(MUTED)).wrap());
    }
}

fn plural(n: usize, one: &str) -> String {
    format!("{n} {one}{}", if n == 1 { "" } else { "s" })
}

/// Discord's small profile, for the panel beside a DM and for the card a clicked name opens: banner with the
/// friend and "..." buttons, name and handle, mutual friends and servers (a click opens the full profile),
/// About me, Member Since, what they are doing, and View Full Profile. The card also has the message field.
#[allow(clippy::too_many_arguments)]
fn small_profile(ui: &mut Ui, s: &AppState, pv: &ProfileView, card: bool, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let pr = &pv.profile;
    let ctx = ui.ctx().clone();
    let user = super::userbtns::raw(&pr.id).to_owned();
    let others = s.discord_user.as_ref() != Some(&user);
    let full = || if card { Action::EnlargeProfile } else { Action::ShowDmProfile(pr.id.clone()) };
    let back = if card { crate::theme::Glass::Float.fill() } else { th::tone(0.08) };
    let banner = profile_head(ui, s, pr, 106.0, 80.0, if card { 16 } else { 0 }, back, false, tex, asked, want);
    // the picture and the name open the full profile (same rectangle as profile_head's avatar)
    let pic = Rect::from_min_size(pos2(banner.left() + 16.0, banner.bottom() - 40.0), vec2(80.0, 80.0));
    if ui.interact(pic, ui.id().with("pfp_full"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
        acts.push(full());
    }
    let slot = |i: f32| Rect::from_min_size(pos2(banner.right() - 12.0 - 32.0 - i * 40.0, banner.top() + 10.0), vec2(32.0, 32.0));
    if banner_btn(ui, slot(0.0), "\u{E712}", "More") {
        let born = ctx.input(|i| i.time); // read before data_mut: a context lock inside another deadlocks
        ctx.data_mut(|d| d.insert_temp(egui::Id::new(MORE), More { user: user.clone(), at: slot(0.0).left_bottom() + vec2(-208.0, 6.0), invite: false, born }));
    }
    if let Some(b) = friend_act(s, &user) {
        if banner_btn(ui, slot(1.0), b.glyph, b.label) {
            acts.extend(b.acts);
        }
    }
    egui::ScrollArea::vertical().id_salt("small_profile").auto_shrink([false, true]).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 0)).show(ui, |ui| {
            if ui.add(egui::Label::new(sb(&pr.name, 20.0).color(TEXT)).truncate().sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                acts.push(full());
            }
            let handle = if pr.pronouns.is_empty() { pr.handle.clone() } else { format!("{} \u{2022} {}", pr.handle, pr.pronouns) };
            ui.label(rt(handle, 13.0).color(SECONDARY));
            if !pr.friends.is_empty() || !pr.servers.is_empty() {
                ui.add_space(10.0);
                let mut parts = vec![];
                if !pr.friends.is_empty() {
                    parts.push(plural(pr.friends.len(), "Mutual Friend"));
                }
                if !pr.servers.is_empty() {
                    parts.push(plural(pr.servers.len(), "Mutual Server"));
                }
                let clicked = ui
                    .horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        for f in pr.friends.iter().take(3) {
                            let (a, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                            let t = texture(&ctx, tex, asked, s, &f.avatar, want);
                            avatar(ui, a, t, &f.name);
                        }
                        ui.add_space(3.0);
                        ui.add(egui::Label::new(rt(parts.join(" \u{2022} "), 12.5).color(SECONDARY)).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
                    })
                    .inner;
                if clicked {
                    acts.push(full());
                }
            }
            profile_about(ui, pv);
            for a in pr.activities.iter() {
                ui.add_space(12.0);
                activity_card(ui, s, a, 60.0, tex, asked, want);
            }
            if card && others {
                // Discord's "Message @name" field: Enter sends it in their DM, which opens
                ui.add_space(14.0);
                let tid = egui::Id::new(("profile_msg", &user));
                let mut text: String = ui.ctx().data(|d| d.get_temp(tid)).unwrap_or_default();
                let te = ui.add(egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY).font(egui::FontId::proportional(14.0)).margin(egui::Margin::symmetric(12, 8)).hint_text(rt(format!("Message @{}", pr.name), 14.0).color(FAINT)));
                if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !text.trim().is_empty() {
                    acts.push(Action::MessageFromProfile { user: user.clone(), text: std::mem::take(&mut text) });
                    acts.push(Action::CloseProfile);
                }
                ui.ctx().data_mut(|d| d.insert_temp(tid, text));
            }
            ui.add_space(14.0);
        });
    });
}

/// Discord's full profile inside `outer`: the profile on the left (banner, name, Message, friend and "..."
/// buttons, About me, Member Since, Note), and on the right the tabs Activity, Mutual Friends, Mutual Servers.
#[allow(clippy::too_many_arguments)]
fn full_profile(ui: &mut Ui, outer: Rect, s: &AppState, pv: &ProfileView, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let pr = &pv.profile;
    let ctx = ui.ctx().clone();
    let user = super::userbtns::raw(&pr.id).to_owned();
    let others = s.discord_user.as_ref() != Some(&user);
    let lw = (outer.width() * 0.44).clamp(280.0, 400.0);
    let left = Rect::from_min_size(outer.min + vec2(20.0, 20.0), vec2(lw, outer.height() - 40.0));
    let right = Rect::from_min_max(pos2(left.right() + 28.0, outer.top() + 20.0), outer.max - vec2(24.0, 20.0));
    let paper = th::tone(0.15);
    ui.painter().rect_filled(left, 16.0, paper);
    ui.scope_builder(egui::UiBuilder::new().max_rect(left), |ui| {
        ui.shrink_clip_rect(left);
        profile_head(ui, s, pr, 130.0, 104.0, 16, paper, true, tex, asked, want);
        egui::ScrollArea::vertical().id_salt("full_profile_left").auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::new().inner_margin(egui::Margin::symmetric(24, 0)).show(ui, |ui| {
                ui.add(egui::Label::new(sb(&pr.name, 24.0).color(TEXT)).truncate());
                let handle = if pr.pronouns.is_empty() { pr.handle.clone() } else { format!("{} \u{2022} {}", pr.handle, pr.pronouns) };
                ui.label(rt(handle, 13.5).color(SECONDARY));
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    if others && profile_btn(ui, "\u{E8BD}", Some("Message"), "") {
                        acts.push(Action::MessagePerson(user.clone()));
                        acts.push(Action::CloseProfile);
                    }
                    if let Some(b) = friend_act(s, &user) {
                        if profile_btn(ui, b.glyph, None, b.label) {
                            acts.extend(b.acts);
                        }
                    }
                    if profile_btn(ui, "\u{E712}", None, "More") {
                        let at = ctx.input(|i| i.pointer.interact_pos()).unwrap_or(left.center()) + vec2(12.0, 12.0);
                        let born = ctx.input(|i| i.time);
                        ctx.data_mut(|d| d.insert_temp(egui::Id::new(MORE), More { user: user.clone(), at, invite: false, born }));
                    }
                });
                profile_about(ui, pv);
                if others {
                    ui.add_space(14.0);
                    ui.label(sb("Note (only visible to you)", 12.0).color(TEXT));
                    ui.add_space(2.0);
                    if ui.add(egui::Label::new(rt("Click to add a note", 13.0).italics().color(MUTED)).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        acts.push(Action::AskPerson { user: user.clone(), ask: mytune_chat::yapfast_core::application::PersonAsk::Note });
                    }
                }
                ui.add_space(20.0);
            });
        });
    });
    // close, top right
    if banner_btn(ui, Rect::from_min_size(pos2(outer.right() - 44.0, outer.top() + 12.0), vec2(32.0, 32.0)), "\u{E711}", "Close") {
        acts.push(Action::CloseProfile);
    }
    ui.scope_builder(egui::UiBuilder::new().max_rect(right), |ui| {
        ui.shrink_clip_rect(right);
        let tid = egui::Id::new(("profile_tab", &user));
        let mut tab: usize = ctx.data(|d| d.get_temp(tid)).unwrap_or(0);
        let names = ["Activity".to_owned(), plural(pr.friends.len(), "Mutual Friend"), plural(pr.servers.len(), "Mutual Server")];
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (i, n) in names.iter().enumerate() {
                if th::tab(ui, n, tab == i).clicked() {
                    tab = i;
                }
            }
        });
        ctx.data_mut(|d| d.insert_temp(tid, tab));
        ui.add_space(14.0);
        egui::ScrollArea::vertical().id_salt(("full_profile_right", tab)).auto_shrink([false, false]).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let none = |ui: &mut Ui, text: &str| {
                ui.add_space(6.0);
                ui.label(rt(if pv.loading { "Loading\u{2026}" } else { text }, 13.0).color(MUTED));
            };
            // a row of the two lists: picture, name, a second line
            let mut row = |ui: &mut Ui, pic: &Option<String>, name: &str, line: &str, round: bool| -> bool {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(r, 10.0, Color32::from_white_alpha(0x0E));
                }
                let a = Rect::from_min_size(pos2(r.left() + 8.0, r.center().y - 20.0), vec2(40.0, 40.0));
                match texture(&ctx, tex, asked, s, pic, want) {
                    Some(t) if !round => egui::Image::new(egui::load::SizedTexture::new(t, a.size())).corner_radius(12.0).paint_at(ui, a),
                    t => avatar(ui, a, t, name),
                }
                let f = egui::FontId::new(15.0, th::semibold());
                let text = ellipsize(ui, name, &f, r.right() - a.right() - 24.0);
                ui.painter().text(pos2(a.right() + 12.0, r.center().y - if line.is_empty() { 0.0 } else { 8.0 }), egui::Align2::LEFT_CENTER, text, f, TEXT);
                if !line.is_empty() {
                    ui.painter().text(pos2(a.right() + 12.0, r.center().y + 10.0), egui::Align2::LEFT_CENTER, line, egui::FontId::proportional(12.0), MUTED);
                }
                resp.clicked()
            };
            match tab {
                0 => {
                    ui.label(rt("Current activity", 12.5).color(SECONDARY));
                    ui.add_space(8.0);
                    if pr.activities.is_empty() {
                        none(ui, "Nothing right now.");
                    }
                    for a in pr.activities.iter() {
                        activity_card(ui, s, a, 84.0, tex, asked, want);
                        ui.add_space(8.0);
                    }
                }
                1 => {
                    if pr.friends.is_empty() {
                        none(ui, "No mutual friends.");
                    }
                    for f in &pr.friends {
                        if row(ui, &f.avatar, &f.name, "", true) {
                            acts.push(Action::ShowFriendProfile { user: f.id.clone(), large: true });
                        }
                    }
                }
                _ => {
                    if pr.servers.is_empty() {
                        none(ui, "No mutual servers.");
                    }
                    for v in &pr.servers {
                        row(ui, &v.icon, &v.name, v.nick.as_deref().unwrap_or(""), false);
                    }
                }
            }
        });
    });
}

const MORE: &str = "profile_more";

/// The open "..." menu of a profile: whose, where, whether its server list is unfolded, when it opened.
#[derive(Clone)]
struct More {
    user: String,
    at: egui::Pos2,
    invite: bool,
    born: f64,
}

/// A profile's "..." menu, as in Discord: Share Profile (copies the profile's link), Invite to Server, Ignore,
/// Block, Copy User ID; plus Call and Mention, which the old card had as buttons. Discord's Report is not
/// here: the engine cannot send one.
fn profile_menu(ctx: &egui::Context, s: &AppState, acts: &mut Vec<Action>) {
    use super::placemenu::{arrow_row, flyout, left_sub, row, sep, sub_row};
    let id = egui::Id::new(MORE);
    let Some(mut m) = ctx.data(|d| d.get_temp::<More>(id)) else { return };
    let screen = ctx.content_rect();
    let pos = pos2(m.at.x.min(screen.right() - 260.0).max(screen.left() + 8.0), m.at.y.min(screen.bottom() - 330.0).max(screen.top() + 8.0)).round();
    let mut close = false;
    let mut invite_at = None; // the Invite to Server row while its server list is open
    let user = m.user.clone();
    let resp = egui::Area::new(id.with("area")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        th::float(ui, th::R_MENU, 6, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let list = || super::userbtns::list(s, &user); // actions cannot be copied: a fresh list per use
            if !list().is_empty() {
                if row(ui, "\u{E72D}", "Share Profile", false, false, None) {
                    ctx.copy_text(format!("https://discord.com/users/{user}"));
                    close = true;
                }
                if !s.servers.is_empty() {
                    let r = arrow_row(ui, "\u{E8FA}", "Invite to Server", m.invite);
                    if r.hovered() || r.clicked() {
                        m.invite = true;
                    }
                    if m.invite {
                        invite_at = Some(r.rect);
                    }
                }
                sep(ui);
                for b in list().into_iter().filter(|b| matches!(b.label, "Call" | "Mention")) {
                    if row(ui, b.glyph, b.label, false, false, None) {
                        acts.extend(b.acts);
                        acts.push(Action::CloseProfile);
                        if b.label == "Mention" {
                            super::compose::focus(ctx);
                        }
                        close = true;
                    }
                }
                sep(ui);
                if row(ui, "\u{E7B3}", "Ignore", false, false, None) {
                    acts.push(Action::ChangeRelationship { user: user.clone(), change: mytune_chat::yapfast_core::chat::RelationshipChange::Ignore });
                    close = true;
                }
                for b in list().into_iter().filter(|b| matches!(b.label, "Block" | "Unblock")) {
                    if row(ui, b.glyph, b.label, b.danger, false, None) {
                        acts.extend(b.acts);
                        close = true;
                    }
                }
                sep(ui);
            }
            if row(ui, "\u{E8C8}", "Copy User ID", false, false, None) {
                ctx.copy_text(user.clone());
                close = true;
            }
        });
    });
    ctx.move_to_top(resp.response.layer_id);
    // the click that opened it is "elsewhere" too: it only counts from the next frame on
    let fresh = ctx.input(|i| i.time) == m.born;
    // the server list, beside the row; a click in it is not a click elsewhere
    let mut in_sub = false;
    if let Some(at) = invite_at {
        let menu = resp.response.rect;
        let r = flyout(ctx, id.with("invite"), at, menu, |ui| {
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for v in s.servers.iter() {
                    if sub_row(ui, &v.name, "", false) {
                        acts.push(Action::InviteToServer { user: user.clone(), server: v.id.clone() });
                        close = true;
                    }
                }
            });
        });
        m.invite = !left_sub(ctx, menu, at, r);
        in_sub = ctx.input(|i| i.pointer.interact_pos().is_some_and(|p| r.contains(p)));
    }
    if close || (!fresh && ((resp.response.clicked_elsewhere() && !in_sub) || ctx.input(|i| i.key_pressed(egui::Key::Escape)))) {
        ctx.data_mut(|d| d.remove::<More>(id));
    } else {
        ctx.data_mut(|d| d.insert_temp(id, m));
    }
}

#[cfg(test)]
mod profile_tests {
    #[test]
    fn clock_reads_like_discord() {
        assert_eq!(super::clock(245), "4:05");
        assert_eq!(super::clock(2 * 3600 + 37 * 60 + 44), "2:37:44");
        assert_eq!(super::clock(0), "0:00");
        assert_eq!(super::plural(1, "Mutual Friend"), "1 Mutual Friend");
        assert_eq!(super::plural(3, "Mutual Server"), "3 Mutual Servers");
    }
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
            let uid = m.id.clone();
            resp.context_menu(|ui| {
                if ui.button("Profile").clicked() {
                    remember_profile_at(ui.ctx());
                    acts.push(Action::ShowProfile { user: uid.clone(), large: false });
                    ui.close();
                }
                for b in super::userbtns::list(s, &uid) {
                    if ui.button(b.label).clicked() {
                        acts.extend(b.acts);
                        if b.label == "Mention" {
                            super::compose::focus(ui.ctx());
                        }
                        ui.close();
                    }
                }
            });
            let t = ctx.animate_bool_with_time(resp.id, resp.hovered(), QUICK);
            if t > 0.0 {
                ui.painter().rect_filled(row, 10.0, k_hover().gamma_multiply(t));
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
