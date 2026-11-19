//! The right-click menu of someone's tile in a call, as Discord's: the DM's rows (Mark as Read, Pin, Close DM,
//! Mute) when the call is their DM, Profile, Add Note, Add Friend Nickname, User Volume, Mute and Mute
//! Soundboard (this account's ears only, `yapfast_core::volume`), Invite to Server, the friend rows, Ignore,
//! Block, Copy User ID and Copy Channel ID. Left out, because the engine has nothing for them: Add to
//! Favourites, Apps, Disable Video, View Verification Code, Pop Out User, Show Non-Video Participants.

use super::placemenu::{arrow_row, flyout, left_sub, mute_text, row, sep, sub_row};
use super::*;
use mytune_chat::yapfast_core::application::{network_id, PersonAsk, Relation, MUTE_TIMES};
use mytune_chat::yapfast_core::chat::{Muting, RelationshipChange};
use mytune_chat::yapfast_core::volume::{self, Kind};

#[derive(Clone, Copy, PartialEq)]
enum Sub {
    Invite,
    Mute,
}

/// The open menu: whose tile (participant id), where, which submenu is unfolded, when it opened.
#[derive(Clone)]
struct Open {
    who: String,
    at: egui::Pos2,
    sub: Option<Sub>,
    born: f64,
}

fn key() -> egui::Id {
    egui::Id::new("call_menu")
}

/// Opens the menu of the call participant `who` at the pointer.
pub fn open(ctx: &egui::Context, who: &str) {
    // read before `data_mut`: the context's lock is not re-entrant, input inside it froze the app
    let (at, born) = ctx.input(|i| (i.pointer.interact_pos().unwrap_or(egui::pos2(100.0, 100.0)), i.time));
    ctx.data_mut(|d| d.insert_temp(key(), Open { who: who.to_owned(), at, sub: None, born }));
}

/// Temp-data key: the person whose volume or mutes the menu just changed, for the page to save.
pub const SOUND_CHANGED: &str = "person_sound_changed";

/// Stream Volume slider and Mute Stream tick for `person`'s stream (Go Live): its sound alone, apart from
/// their voice, for this account's ears only.
fn stream_rows(ui: &mut Ui, person: &str) {
    use mytune_chat::settings::{set_stream_sound, stream_sound};
    let (gain, muted) = stream_sound(person);
    let mut vol = f64::from(volume::to_slider(gain).round());
    let mut changed = false;
    let (head, _) = ui.allocate_exact_size(vec2(240.0, 26.0), Sense::hover());
    ui.painter().text(egui::pos2(head.left() + 14.0, head.center().y), egui::Align2::LEFT_CENTER, "Stream Volume", egui::FontId::proportional(13.0), SECONDARY);
    ui.painter().text(egui::pos2(head.right() - 14.0, head.center().y), egui::Align2::RIGHT_CENTER, format!("{vol:.0}%"), egui::FontId::proportional(12.0), MUTED);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        if crate::widgets::slider(ui, key().with("stream_vol"), &mut vol, 0.0, 200.0, 216.0, true).changed {
            set_stream_sound(person, volume::from_slider(vol.round() as f32), muted);
            changed = true;
        }
    });
    ui.add_space(6.0);
    sep(ui);
    if row(ui, "\u{E74F}", "Mute Stream", false, false, Some(muted)) {
        set_stream_sound(person, stream_sound(person).0, !muted);
        changed = true;
    }
    if changed {
        ui.data_mut(|d| d.insert_temp(egui::Id::new(SOUND_CHANGED), person.to_owned()));
    }
}

/// The small menu of a watched stream in fullscreen and in its own window: one per OS window.
fn stream_key(ctx: &egui::Context) -> egui::Id {
    egui::Id::new("stream_menu").with(ctx.viewport_id())
}

/// Opens the stream menu of `who`'s stream at the pointer, in the window `ctx` is drawing.
pub fn open_stream(ctx: &egui::Context, who: &str) {
    let (at, born) = ctx.input(|i| (i.pointer.interact_pos().unwrap_or(egui::pos2(100.0, 100.0)), i.time));
    let k = stream_key(ctx);
    ctx.data_mut(|d| d.insert_temp(k, Open { who: who.to_owned(), at, sub: None, born }));
}

/// Draws the open stream menu; call it in the window that opened it, after the stream's picture.
pub fn show_stream(ctx: &egui::Context) {
    let k = stream_key(ctx);
    let Some(m) = ctx.data(|d| d.get_temp::<Open>(k)) else { return };
    let area = k.with("area");
    let screen = ctx.content_rect();
    let size = ctx.memory(|mem| mem.area_rect(area)).map_or(egui::Vec2::ZERO, |r| r.size());
    let pos = egui::pos2(m.at.x.min(screen.right() - size.x - 8.0), m.at.y.min(screen.bottom() - size.y - 8.0)).max(screen.min + vec2(8.0, 8.0)).round();
    let resp = egui::Area::new(area).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        th::float(ui, th::R_MENU, 6, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            stream_rows(ui, &m.who);
        });
    });
    ctx.move_to_top(resp.response.layer_id);
    let (fresh, esc) = ctx.input(|i| (i.time == m.born, i.key_pressed(egui::Key::Escape)));
    if !fresh && (resp.response.clicked_elsewhere() || esc) {
        ctx.data_mut(|d| d.remove::<Open>(k));
    }
}

pub fn show(ctx: &egui::Context, s: &AppState, acts: &mut Vec<Action>) {
    let Some(mut m) = ctx.data(|d| d.get_temp::<Open>(key())) else { return };
    // they left the call: nothing to act on
    let Some(pe) = s.voice.participants.iter().find(|p| p.id == m.who) else {
        ctx.data_mut(|d| d.remove::<Open>(key()));
        return;
    };
    let user = super::userbtns::raw(&pe.id).to_owned();
    let room = s.voice.room.clone().unwrap_or_default();
    // the call is this person's DM: the menu also has the conversation's rows
    let dm = s.rooms.iter().find(|r| r.id == room && r.direct && r.person.as_deref() == Some(user.as_str())).filter(|_| !pe.local);
    let list = || if pe.local { vec![] } else { super::userbtns::list(s, &user) }; // actions cannot be copied
    let other = !list().is_empty(); // a Discord user who is not this account
    let area = key().with("area");
    let screen = ctx.content_rect();
    let size = ctx.memory(|mem| mem.area_rect(area)).map_or(egui::Vec2::ZERO, |r| r.size());
    let pos = egui::pos2(m.at.x.min(screen.right() - size.x - 8.0), m.at.y.min(screen.bottom() - size.y - 8.0)).max(screen.min + vec2(8.0, 8.0)).round();
    let mut close = false;
    let mut sub_at = None; // the arrow row whose submenu is open
    let mut arrow = |ui: &mut Ui, m: &mut Open, sub: Sub, g: &str, text: &str| {
        let r = arrow_row(ui, g, text, m.sub == Some(sub));
        if r.hovered() || r.clicked() {
            m.sub = Some(sub);
        }
        if m.sub == Some(sub) {
            sub_at = Some(r.rect);
        }
    };
    let resp = egui::Area::new(area).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        th::float(ui, th::R_MENU, 6, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            if let Some(r) = dm {
                if r.unread > 0 && row(ui, "\u{E8C3}", "Mark as Read", false, false, None) {
                    acts.push(Action::MarkRead(r.id.clone()));
                    close = true;
                }
                let pinned = s.pinned.contains(&r.id);
                if row(ui, "\u{E718}", if pinned { "Unpin" } else { "Pin" }, false, false, None) {
                    acts.push(Action::Pin { room: r.id.clone(), pinned: !pinned });
                    close = true;
                }
                sep(ui);
            }
            if row(ui, "\u{E77B}", "Profile", false, false, None) {
                super::header::remember_profile_at(ctx);
                acts.push(Action::ShowProfile { user: pe.id.clone(), large: false });
                close = true;
            }
            if other {
                if row(ui, "\u{E70B}", "Add Note", false, false, None) {
                    acts.push(Action::AskPerson { user: user.clone(), ask: PersonAsk::Note });
                    close = true;
                }
                if s.relation(&user) == Relation::Friend && row(ui, "\u{E8AC}", "Add Friend Nickname", false, false, None) {
                    acts.push(Action::AskPerson { user: user.clone(), ask: PersonAsk::Nickname });
                    close = true;
                }
            }
            if let Some(r) = dm {
                if row(ui, "\u{E711}", "Close DM", false, false, None) {
                    acts.push(Action::CloseDm(r.id.clone()));
                    close = true;
                }
            }
            if !pe.local {
                sep(ui);
                // how loud they are for this account alone, on Discord's 0 to 200 scale
                let mut changed_sound = false;
                let mut vol = f64::from(volume::to_slider(volume::get(Kind::Voice, &pe.person)).round());
                let (head, _) = ui.allocate_exact_size(vec2(240.0, 26.0), Sense::hover());
                ui.painter().text(egui::pos2(head.left() + 14.0, head.center().y), egui::Align2::LEFT_CENTER, "User Volume", egui::FontId::proportional(13.0), SECONDARY);
                ui.painter().text(egui::pos2(head.right() - 14.0, head.center().y), egui::Align2::RIGHT_CENTER, format!("{vol:.0}%"), egui::FontId::proportional(12.0), MUTED);
                ui.horizontal(|ui| {
                    ui.add_space(12.0);
                    if crate::widgets::slider(ui, key().with("vol"), &mut vol, 0.0, 200.0, 216.0, true).changed {
                        volume::set(Kind::Voice, &pe.person, volume::from_slider(vol.round() as f32));
                        changed_sound = true;
                    }
                });
                ui.add_space(6.0);
                sep(ui);
                // ticks: they stay open, as Discord's do
                let muted = volume::muted(&pe.person);
                if row(ui, "\u{E74F}", "Mute", false, false, Some(muted)) {
                    volume::mute(&pe.person, !muted);
                    changed_sound = true;
                }
                let quiet = volume::soundboard_muted(&pe.person);
                if row(ui, "\u{E7F6}", "Mute Soundboard", false, false, Some(quiet)) {
                    volume::mute_soundboard(&pe.person, !quiet);
                    changed_sound = true;
                }
                if changed_sound {
                    ui.data_mut(|d| d.insert_temp(egui::Id::new(SOUND_CHANGED), pe.person.clone()));
                }
            }
            // Tests: MYTUNE_STREAM_ROWS shows the rows on any tile (the offline preview has no streamer).
            if (pe.streaming && !pe.local) || std::env::var_os("MYTUNE_STREAM_ROWS").is_some() {
                sep(ui);
                stream_rows(ui, &pe.person);
            }
            if other {
                if !s.servers.is_empty() {
                    arrow(ui, &mut m, Sub::Invite, "\u{E8FA}", "Invite to Server");
                }
                for b in list().into_iter().filter(|b| matches!(b.label, "Remove Friend" | "Add Friend" | "Accept Friend" | "Cancel Request")) {
                    if row(ui, b.glyph, b.label, false, false, None) {
                        acts.extend(b.acts);
                        close = true;
                    }
                }
                let ignored = s.discord_ignored.iter().any(|u| *u == user);
                if row(ui, "\u{E7B3}", if ignored { "Unignore" } else { "Ignore" }, false, false, None) {
                    acts.push(Action::ChangeRelationship { user: user.clone(), change: if ignored { RelationshipChange::Unignore } else { RelationshipChange::Ignore } });
                    close = true;
                }
                for b in list().into_iter().filter(|b| matches!(b.label, "Block" | "Unblock")) {
                    if row(ui, b.glyph, b.label, b.danger, false, None) {
                        acts.extend(b.acts);
                        close = true;
                    }
                }
            }
            if let Some(r) = dm {
                sep(ui);
                if r.muted {
                    if row(ui, "\u{E7ED}", &format!("Unmute @{}", pe.user), false, false, None) {
                        acts.push(Action::MuteDm { room: r.id.clone(), muting: Muting::Off });
                        close = true;
                    }
                } else {
                    arrow(ui, &mut m, Sub::Mute, "\u{E7ED}", &format!("Mute @{}", pe.user));
                }
            }
            sep(ui);
            if row(ui, "\u{E8C8}", "Copy User ID", false, false, None) {
                ctx.copy_text(user.clone());
                close = true;
            }
            if !room.is_empty() && row(ui, "\u{E8C8}", "Copy Channel ID", false, false, None) {
                ctx.copy_text(network_id(&room).to_owned());
                close = true;
            }
        });
    });
    ctx.move_to_top(resp.response.layer_id);
    let menu = resp.response.rect;
    // the submenu beside its row; a click in it is not a click elsewhere
    let mut in_sub = false;
    if let (Some(sub), Some(at)) = (m.sub, sub_at) {
        let r = flyout(ctx, key().with(sub as u8), at, menu, |ui| match sub {
            Sub::Invite => {
                egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                    for v in s.servers.iter() {
                        if sub_row(ui, &v.name, "", false) {
                            acts.push(Action::InviteToServer { user: user.clone(), server: v.id.clone() });
                            close = true;
                        }
                    }
                });
            }
            Sub::Mute => {
                for t in MUTE_TIMES {
                    if sub_row(ui, &mute_text(t), "", false) {
                        acts.push(Action::MuteDm { room: room.clone(), muting: t });
                        close = true;
                    }
                }
            }
        });
        if left_sub(ctx, menu, at, r) {
            m.sub = None;
        }
        in_sub = ctx.input(|i| i.pointer.interact_pos().is_some_and(|p| r.contains(p)));
    }
    // the right click that opened it is "elsewhere" too: it only counts from the next frame on
    let fresh = ctx.input(|i| i.time) == m.born;
    if close || (!fresh && ((resp.response.clicked_elsewhere() && !in_sub) || ctx.input(|i| i.key_pressed(egui::Key::Escape)))) {
        ctx.data_mut(|d| d.remove::<Open>(key()));
    } else {
        ctx.data_mut(|d| d.insert_temp(key(), m));
    }
}
