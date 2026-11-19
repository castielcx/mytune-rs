//! Discord features drawn over the chat: clicked Discord links (invite, event, gift, app authorization,
//! channel, message, profile) routed to the engine as Yapfast routes them; the New DM dialog; the Reactions
//! window (who reacted, who voted); the custom status dialog (emoji, text, when it clears); the app
//! authorization, scheduled event and gift dialogs; and the confirms for switching calls and for a channel
//! dragged into a category. The engine holds each one's state (`AppState::new_dm`, `reactions`,
//! `status_editor`, `oauth`, `scheduled_event`, `gift`, `switch_voice`, `move_sync`); these draw it.

use super::voice::{glyph, k_raised};
use super::*;
use mytune_chat::yapfast_core::application::ClearAfter;
use mytune_chat::yapfast_core::chat::Presence;
use mytune_chat::yapfast_core::discord::{self as dc, DiscordRoute};

/// What the dialogs keep between frames.
#[derive(Default)]
pub struct ExtrasUi {
    dm_query: String,
    status_open: bool,
    status_emoji: String,
    status_text: String,
    status_clear: usize,
    /// OpenStatusEditor was sent and the engine has not shown the editor yet.
    status_asked: bool,
}

fn link_id() -> egui::Id {
    egui::Id::new("clicked_link")
}

/// Opens a link clicked in a message: Discord's own links are handed to the engine on the next pass of
/// [`route_links`] (it knows the account and its servers), every other one opens in the browser there.
pub fn open_link(ctx: &egui::Context, url: &str) {
    ctx.data_mut(|d| d.insert_temp(link_id(), url.to_owned()));
}

/// Routes the link [`open_link`] noted, once.
pub fn route_links(ctx: &egui::Context, s: &AppState, acts: &mut Vec<Action>) {
    let Some(url) = ctx.data_mut(|d| d.remove_temp::<String>(link_id())) else { return };
    let route = dc::discord_route(&url).filter(|r| s.opens_discord_route(r));
    let Some(route) = route else {
        // web links only: an embed or attachment could carry file: or an app's own scheme
        if url.starts_with("https://") || url.starts_with("http://") {
            ctx.open_url(egui::OpenUrl::new_tab(url));
        }
        return;
    };
    acts.push(match route {
        DiscordRoute::Invite { code } => Action::OpenInviteLink(code),
        DiscordRoute::Authorize { query } => Action::OpenOAuthLink(query),
        DiscordRoute::Gift { code } => Action::OpenGiftLink(code),
        DiscordRoute::Event { guild, event } => Action::OpenEventLink { guild, event },
        DiscordRoute::DirectMessages => Action::Navigate(Navigation::DirectMessages),
        DiscordRoute::Server { guild } => Action::Navigate(Navigation::Server(dc::server_id(&guild))),
        DiscordRoute::Channel { guild, channel } => Action::Open(dc::room_id(&guild, &channel)),
        DiscordRoute::Message { guild, channel, message } => Action::JumpInRoom { room: dc::room_id(&guild, &channel), item: format!("d:{message}") },
        DiscordRoute::User { user } => {
            super::header::remember_profile_at(ctx);
            Action::ShowProfile { user, large: false }
        }
    });
}

/// A dialog window in the chat's style, closed by its X or Escape (`on_close`).
fn window(ctx: &egui::Context, id: &str, title: &str, width: f32, on_close: Action, acts: &mut Vec<Action>, body: impl FnOnce(&mut Ui, &mut Vec<Action>)) {
    let mut open = true;
    egui::Window::new(title).id(egui::Id::new(id)).open(&mut open).collapsible(false).resizable(false).default_width(width).anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0)).show(ctx, |ui| {
        ui.set_width(width - 20.0);
        body(ui, acts);
    });
    if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        acts.push(on_close);
    }
}

fn error(ui: &mut Ui, e: &Option<String>) {
    if let Some(e) = e {
        ui.add_space(4.0);
        ui.label(rt(e, 12.5).color(DANGER));
    }
}

/// A person row: avatar, name and a second line; returns the row's response (click).
#[allow(clippy::too_many_arguments)]
fn person_row(ui: &mut Ui, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, avatar_url: Option<String>, name: &str, line: &str) -> egui::Response {
    let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(r, 10.0, k_hover());
    }
    let ctx = ui.ctx().clone();
    let t = texture(&ctx, tex, asked, s, &avatar_url, want);
    avatar(ui, egui::Rect::from_center_size(egui::pos2(r.left() + 22.0, r.center().y), vec2(32.0, 32.0)), t, name);
    ui.painter().text(egui::pos2(r.left() + 46.0, r.center().y - if line.is_empty() { 0.0 } else { 8.0 }), egui::Align2::LEFT_CENTER, name, egui::FontId::new(14.0, th::semibold()), TEXT);
    if !line.is_empty() {
        ui.painter().text(egui::pos2(r.left() + 46.0, r.center().y + 9.0), egui::Align2::LEFT_CENTER, line, egui::FontId::proportional(12.0), MUTED);
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// New DM: search people (Discord friends and anyone the engine finds), click one to open or start the DM.
pub fn new_dm(ctx: &egui::Context, eu: &mut ExtrasUi, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let Some(nd) = &s.new_dm else {
        eu.dm_query.clear();
        return;
    };
    window(ctx, "new_dm_dlg", "New Message", 440.0, Action::CloseNewDm, acts, |ui, acts| {
        ui.label(rt("Find a friend by their name or username.", 13.0).color(SECONDARY));
        ui.add_space(6.0);
        let te = ui.add(egui::TextEdit::singleline(&mut eu.dm_query).desired_width(f32::INFINITY).hint_text("Type a name"));
        if te.changed() {
            acts.push(Action::NewDmQuery(eu.dm_query.clone()));
        }
        if !te.has_focus() && eu.dm_query.is_empty() {
            te.request_focus();
        }
        ui.add_space(6.0);
        if nd.starting {
            ui.label(rt("Opening the conversation\u{2026}", 13.0).color(MUTED));
        }
        error(ui, &nd.error);
        egui::ScrollArea::vertical().max_height(340.0).show(ui, |ui| {
            if nd.discord.is_empty() && nd.matrix.is_empty() && !eu.dm_query.trim().is_empty() {
                ui.label(rt("No one found.", 13.0).color(MUTED));
            }
            for c in nd.discord.iter().chain(&nd.matrix) {
                if person_row(ui, s, tex, asked, want, c.avatar.clone(), &c.name, &c.detail).clicked() && !nd.starting {
                    acts.push(Action::StartDm(c.clone()));
                }
            }
        });
    });
}

/// The Reactions window: one tab per reaction (or the poll's voters), the people, a remove button for one's own
/// reaction or, for a moderator, anyone's, and More while Discord has more.
pub fn reactions(ctx: &egui::Context, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let Some(w) = &s.reactions else { return };
    let msg = s.messages.iter().find(|m| m.item == w.item);
    window(ctx, "reactions_dlg", if w.poll { "Votes" } else { "Reactions" }, 420.0, Action::CloseReactions, acts, |ui, acts| {
        if !w.poll {
            if let Some(m) = msg {
                ui.horizontal_wrapped(|ui| {
                    for r in &m.reactions {
                        let on = r.key == w.key;
                        let label = if r.key.contains(':') { format!(":{}: {}", r.key.trim_matches(':').split(':').next().unwrap_or(""), r.count) } else { format!("{} {}", r.key, r.count) };
                        if ui.add(crate::theme::Choice::new(label, on)).clicked() && !on {
                            acts.push(Action::ShowReactors(r.key.clone()));
                        }
                    }
                });
                ui.add_space(6.0);
            }
        }
        egui::ScrollArea::vertical().max_height(360.0).show(ui, |ui| {
            for person in &w.people {
                let pic = (!person.avatar.is_empty()).then(|| person.avatar.clone());
                let row = ui.horizontal(|ui| {
                    let line = if person.username.is_empty() { String::new() } else { person.username.clone() };
                    let resp = person_row(ui, s, tex, asked, want, pic, &person.name, &line);
                    if resp.clicked() {
                        super::header::remember_profile_at(ui.ctx());
                        acts.push(Action::ShowProfile { user: person.id.clone(), large: false });
                    }
                    resp.rect
                });
                if !w.poll && (person.me || w.can_remove) {
                    let r = row.inner;
                    let b = egui::Rect::from_center_size(egui::pos2(r.right() - 22.0, r.center().y), vec2(28.0, 28.0));
                    let resp = ui.interact(b, ui.id().with(("unreact", &person.id)), Sense::click());
                    glyph(ui.painter(), b.center(), "\u{E711}", 12.0, if resp.hovered() { DANGER } else { MUTED }, false);
                    if resp.on_hover_text("Remove reaction").clicked() {
                        acts.push(Action::RemoveReactor(person.id.clone()));
                    }
                }
            }
            if w.loading {
                ui.label(rt("Loading\u{2026}", 13.0).color(MUTED));
            } else if w.error.is_none() && !w.people.is_empty() && pill(ui, "More", false).clicked() {
                acts.push(Action::MoreReactors);
            }
        });
        error(ui, &w.error);
    });
}

const CLEAR_AFTER: [(&str, ClearAfter); 5] = [("Today", ClearAfter::Today), ("4 hours", ClearAfter::Minutes(240)), ("1 hour", ClearAfter::Minutes(60)), ("30 minutes", ClearAfter::Minutes(30)), ("Don't clear", ClearAfter::Never)];

/// Opens the custom status dialog (from the status menu or the account page).
pub fn open_status(eu: &mut ExtrasUi, s: &AppState, acts: &mut Vec<Action>) {
    eu.status_open = true;
    eu.status_asked = true;
    eu.status_emoji = s.custom_status.as_ref().and_then(|c| c.emoji.clone()).unwrap_or_default();
    eu.status_text = s.custom_status.as_ref().map(|c| c.text.clone()).unwrap_or_default();
    acts.push(Action::OpenStatusEditor);
}

/// The custom status dialog: emoji (a Unicode one, or a server emoji's `:name:` with Nitro), text, when it clears.
pub fn status_editor(ctx: &egui::Context, eu: &mut ExtrasUi, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    if !eu.status_open {
        return;
    }
    let Some(ed) = &s.status_editor else {
        // once shown, the engine closing it means it saved or was closed elsewhere
        if !eu.status_asked {
            eu.status_open = false;
        }
        return;
    };
    eu.status_asked = false;
    let mut close = false;
    // Laid out like Discord's "Set your status": a preview of how the status shows on the profile, the status
    // field with its emoji, then when it clears (left) and Save (right).
    window(ctx, "status_dlg", "Set your status", 440.0, Action::CloseStatusEditor, acts, |ui, acts| {
        let name = s.account_name.as_deref().unwrap_or("");
        let presence = match s.discord_presence.as_deref() {
            Some("idle") => Some(Presence::Idle),
            Some("dnd") => Some(Presence::DoNotDisturb),
            Some("invisible") => Some(Presence::Offline),
            _ if s.discord_online => Some(Presence::Online),
            _ => None,
        };
        let back = ui.visuals().window_fill;
        ui.add_space(6.0);
        // preview card: banner, avatar with its status shape, the status as a thought bubble, the name
        let (card, _) = ui.allocate_exact_size(vec2(ui.available_width(), 138.0), Sense::hover());
        let banner = egui::Rect::from_min_size(card.min, vec2(card.width(), 64.0));
        ui.painter().rect_filled(card, 14.0, k_field());
        ui.painter().rect_filled(banner, egui::CornerRadius { nw: 14, ne: 14, sw: 0, se: 0 }, th::tone(0.04));
        let av = egui::Rect::from_center_size(egui::pos2(card.left() + 16.0 + 32.0, banner.bottom()), vec2(64.0, 64.0));
        th::dot(ui.painter(), av.center(), 36.0, back);
        let t = texture(ctx, tex, asked, s, &s.account_avatar, want);
        avatar(ui, av, t, name);
        super::header::status_dot(ui, av, presence, back);
        let shown = format!("{} {}", eu.status_emoji.trim(), eu.status_text.trim()).trim().to_owned();
        let (line, ink) = if shown.is_empty() { ("What's happening?".to_owned(), MUTED) } else { (shown, TEXT) };
        let font = egui::FontId::proportional(13.0);
        let line = ellipsize(ui, &line, &font, card.right() - av.right() - 48.0);
        let g = ui.painter().layout_no_wrap(line, font, ink);
        let bubble = egui::Rect::from_min_size(egui::pos2(av.right() + 14.0, banner.bottom() - 18.0), vec2(g.size().x + 26.0, 32.0));
        let paper = th::tone(0.2);
        th::dot(ui.painter(), bubble.left_top() + vec2(-6.0, -6.0), 3.0, paper);
        th::dot(ui.painter(), bubble.left_top() + vec2(5.0, -1.0), 6.0, paper);
        ui.painter().rect_filled(bubble, 16.0, paper);
        ui.painter().galley(egui::pos2(bubble.left() + 13.0, bubble.center().y - g.size().y / 2.0), g, ink);
        ui.painter().text(egui::pos2(card.left() + 16.0, av.bottom() + 6.0), egui::Align2::LEFT_TOP, name, egui::FontId::new(16.0, th::semibold()), TEXT);

        ui.add_space(18.0);
        ui.label(sb("Status", 13.5).color(TEXT));
        ui.add_space(8.0);
        // one field: the emoji and the text share a frame
        egui::Frame::new().fill(k_field()).corner_radius(10.0).stroke(egui::Stroke::new(1.0, crate::theme::p().accent)).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let f = egui::FontId::proportional(14.0);
                ui.add(egui::TextEdit::singleline(&mut eu.status_emoji).frame(egui::Frame::NONE).font(f.clone()).desired_width(26.0).horizontal_align(Align::Center).hint_text("\u{1F600}"));
                ui.add(egui::TextEdit::singleline(&mut eu.status_text).frame(egui::Frame::NONE).font(f).desired_width(f32::INFINITY).hint_text("What's happening?").char_limit(128));
            });
        });
        error(ui, &ed.error);
        ui.add_space(22.0);
        // a row of its own height: `with_layout` took all the height the window had left and centred Save in it
        ui.allocate_ui_with_layout(vec2(ui.available_width(), 32.0), Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.add_enabled_ui(!ed.saving, |ui| {
                let empty = eu.status_text.trim().is_empty() && eu.status_emoji.trim().is_empty();
                if pill(ui, if ed.saving { "Saving\u{2026}" } else { "Save" }, true).clicked() && !empty {
                    let emoji = Some(eu.status_emoji.trim().to_owned()).filter(|e| !e.is_empty());
                    acts.push(Action::SetCustomStatus { emoji, text: eu.status_text.trim().to_owned(), clear_after: CLEAR_AFTER[eu.status_clear].1 });
                }
                if s.custom_status.is_some() && pill(ui, "Clear status", false).clicked() {
                    acts.push(Action::ClearCustomStatus);
                    close = true;
                }
            });
            // when it clears, at the row's left as in Discord
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let label = |i: usize| match CLEAR_AFTER[i].1 {
                    ClearAfter::Today => "Clear today".to_owned(),
                    ClearAfter::Never => "Don't clear".to_owned(),
                    ClearAfter::Minutes(_) => format!("Clear after {}", CLEAR_AFTER[i].0),
                };
                egui::ComboBox::from_id_salt("status_clear").selected_text(rt(label(eu.status_clear), 13.5).color(TEXT)).show_ui(ui, |ui| {
                    for i in 0..CLEAR_AFTER.len() {
                        ui.selectable_value(&mut eu.status_clear, i, label(i));
                    }
                });
            });
        });
        ui.add_space(4.0);
    });
    if close {
        acts.push(Action::CloseStatusEditor);
        eu.status_open = false;
    }
}

/// The dialog a clicked app authorization link opens: the app, what it asks for, the server to add it to.
pub fn oauth(ctx: &egui::Context, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let Some(d) = &s.oauth else { return };
    window(ctx, "oauth_dlg", "Authorize an app", 440.0, Action::CloseOAuth, acts, |ui, acts| {
        if d.resolving {
            ui.label(rt("Looking up the app\u{2026}", 13.0).color(MUTED));
        }
        if let Some(pv) = &d.preview {
            ui.horizontal(|ui| {
                let (a, _) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::hover());
                let t = texture(&ui.ctx().clone(), tex, asked, s, &pv.icon, want);
                avatar(ui, a, t, &pv.name);
                ui.vertical(|ui| {
                    ui.label(sb(&pv.name, 16.0).color(TEXT));
                    if let Some(b) = &pv.bot {
                        ui.label(rt(format!("Adds the bot {b}"), 12.5).color(MUTED));
                    }
                });
            });
            if let Some(a) = &pv.about {
                ui.label(rt(a, 12.5).color(SECONDARY));
            }
            if !pv.wants.is_empty() {
                ui.add_space(6.0);
                ui.label(sb("THIS WILL ALLOW THE APP TO", 11.0).color(MUTED));
                for w in &pv.wants {
                    ui.label(rt(format!("\u{2022} {w}"), 13.0).color(SECONDARY));
                }
            }
            if pv.controls_client {
                ui.label(rt("It asks to control your Discord client.", 12.5).color(th::WARNING));
            }
            if pv.guild_install {
                ui.add_space(6.0);
                ui.label(sb("ADD TO SERVER", 11.0).color(MUTED));
                egui::ScrollArea::vertical().max_height(160.0).show(ui, |ui| {
                    for g in pv.guilds.iter().filter(|g| g.can_add) {
                        let on = d.guild.as_deref() == Some(g.id.as_str());
                        let locked = pv.guild_locked && pv.wanted_guild.as_deref() != Some(g.id.as_str());
                        ui.add_enabled_ui(!locked, |ui| {
                            if ui.add(egui::Button::new(rt(&g.name, 13.0).color(if on { TEXT } else { SECONDARY })).fill(if on { k_active() } else { k_raised() }).min_size(vec2(ui.available_width(), 28.0))).clicked() {
                                acts.push(Action::ChooseOAuthGuild(g.id.clone()));
                            }
                        });
                    }
                });
            }
        }
        if d.done {
            ui.label(rt("Authorized.", 13.0).color(LIVE));
        }
        if let Some(n) = &d.next {
            ui.label(rt(format!("The app continues at {n}"), 12.0).color(MUTED));
        }
        error(ui, &d.error);
        ui.add_space(10.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let ready = d.preview.as_ref().is_some_and(|pv| !pv.guild_install || d.guild.is_some());
            ui.add_enabled_ui(ready && !d.authorizing && !d.done, |ui| {
                if pill(ui, if d.authorizing { "Authorizing\u{2026}" } else { "Authorize" }, true).clicked() {
                    acts.push(Action::AuthorizeOAuth);
                }
            });
        });
    });
}

/// The dialog a clicked scheduled-event link opens: name, time, place, Interested.
pub fn event(ctx: &egui::Context, s: &AppState, acts: &mut Vec<Action>) {
    let Some(d) = &s.scheduled_event else { return };
    window(ctx, "event_dlg", "Event", 420.0, Action::CloseEvent, acts, |ui, acts| {
        if d.resolving {
            ui.label(rt("Looking up the event\u{2026}", 13.0).color(MUTED));
        }
        if let Some(pv) = &d.preview {
            use mytune_chat::yapfast_core::clock;
            let when = clock::event(pv.starts_at, pv.ends_at, clock::now_ms() / 1000, false);
            ui.label(rt(if pv.active { "Happening now".to_owned() } else if pv.over { "Ended".to_owned() } else { when }, 12.5).color(if pv.active { LIVE } else { MUTED }));
            ui.label(sb(&pv.name, 17.0).color(TEXT));
            if let Some(desc) = &pv.description {
                ui.label(rt(desc, 13.0).color(SECONDARY));
            }
            let place = pv.room.as_ref().and_then(|r| s.rooms.iter().find(|x| &x.id == r)).map(|r| format!("#{}", r.name)).or_else(|| pv.location.clone());
            if let Some(pl) = place {
                ui.label(rt(format!("\u{1F4CD} {pl}"), 13.0).color(SECONDARY));
            }
            let mut line = vec![];
            if let Some(c) = &pv.creator {
                line.push(format!("By {c}"));
            }
            if let Some(n) = pv.interested_count {
                line.push(format!("{n} interested"));
            }
            if !line.is_empty() {
                ui.label(rt(line.join("  \u{00B7}  "), 12.5).color(MUTED));
            }
            error(ui, &d.error);
            ui.add_space(10.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add_enabled_ui(!d.marking && !pv.over, |ui| {
                    if pill(ui, if pv.interested { "Interested \u{2713}" } else { "Interested" }, !pv.interested).clicked() {
                        acts.push(Action::ToggleEventInterest);
                    }
                });
            });
        } else {
            error(ui, &d.error);
        }
    });
}

/// The dialog a clicked gift link opens: what it grants, Redeem, or the browser for gifts only Discord redeems.
pub fn gift(ctx: &egui::Context, s: &AppState, acts: &mut Vec<Action>) {
    let Some(d) = &s.gift else { return };
    window(ctx, "gift_dlg", "Gift", 400.0, Action::CloseGift, acts, |ui, acts| {
        if d.resolving {
            ui.label(rt("Looking up the gift\u{2026}", 13.0).color(MUTED));
        }
        if let Some(pv) = &d.preview {
            if let Some(g) = &pv.gifter {
                ui.label(rt(format!("{g} sent you a gift"), 12.5).color(MUTED));
            }
            ui.label(sb(pv.plan.as_deref().unwrap_or(&pv.name), 17.0).color(TEXT));
            let state = if d.redeemed || pv.redeemed { Some(("You redeemed this gift.", LIVE)) } else if pv.spent { Some(("This gift was already claimed.", MUTED)) } else { None };
            if let Some((t, c)) = state {
                ui.label(rt(t, 13.0).color(c));
            }
            error(ui, &d.error);
            ui.add_space(10.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if pv.in_discord {
                    if pill(ui, "Open in browser", true).clicked() {
                        acts.push(Action::OpenGiftInBrowser);
                    }
                } else if !(d.redeemed || pv.redeemed || pv.spent) {
                    let busy = s.redeeming_gifts.contains(&d.code);
                    ui.add_enabled_ui(!busy, |ui| {
                        if pill(ui, if busy { "Redeeming\u{2026}" } else { "Redeem" }, true).clicked() {
                            acts.push(Action::RedeemGift);
                        }
                    });
                }
            });
        } else {
            error(ui, &d.error);
        }
    });
}

/// Joining a call while in another asks first, as Discord does.
pub fn voice_switch(ctx: &egui::Context, s: &AppState, acts: &mut Vec<Action>) {
    let Some(room) = &s.switch_voice else { return };
    let to = s.rooms.iter().find(|r| &r.id == room).map_or("this call".to_owned(), |r| if r.direct { r.name.clone() } else { format!("#{}", r.name) });
    window(ctx, "switch_voice_dlg", "Switch calls?", 380.0, Action::CancelVoiceSwitch, acts, |ui, acts| {
        ui.label(rt(format!("You're already in a call. Leave it and join {to}?"), 13.5).color(SECONDARY));
        ui.add_space(10.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if pill(ui, "Switch", true).clicked() {
                acts.push(Action::ConfirmVoiceSwitch);
            }
            if pill(ui, "Cancel", false).clicked() {
                acts.push(Action::CancelVoiceSwitch);
            }
        });
    });
}

/// Temp-data key: the voice channel a click in the channel list wants to join.
pub const JOIN_ASK: &str = "join_voice_ask";

/// A click on a voice channel asks before joining and says who is in it.
pub fn voice_join(ctx: &egui::Context, s: &AppState, acts: &mut Vec<Action>) {
    let key = egui::Id::new(JOIN_ASK);
    let Some(room) = ctx.data(|d| d.get_temp::<String>(key)) else { return };
    let Some(r) = s.rooms.iter().find(|r| r.id == room) else {
        ctx.data_mut(|d| d.remove::<String>(key));
        return;
    };
    // the clicked row and when: the popover sits beside the row, and the opening click must not close it
    let now = ctx.input(|i| i.time);
    let (row, opened) = ctx.data(|d| d.get_temp::<(egui::Rect, f64)>(key.with("at"))).unwrap_or((egui::Rect::ZERO, now));
    let mut join = false;
    let area = egui::Area::new(egui::Id::new("join_voice_pop")).order(egui::Order::Foreground).fixed_pos(row.right_top() + egui::vec2(8.0, 0.0)).show(ctx, |ui| {
        crate::theme::float(ui, crate::theme::R_MENU, 8, |ui| {
            ui.horizontal(|ui| {
                ui.label(rt("Join voice?", 13.0).color(SECONDARY));
                join = pill(ui, "Join", true).clicked();
            });
        });
    });
    if join {
        // the channel's text chat under the call, as a DM call shows (plain Open when it has none)
        acts.push(Action::OpenChat(room.clone()));
        acts.push(Action::RequestJoinVoice(room.clone()));
    }
    if join || (opened != now && area.response.clicked_elsewhere()) || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        ctx.data_mut(|d| d.remove::<String>(key));
    }
}

/// A channel dragged into a category: keep its own permissions or take the category's.
pub fn move_sync(ctx: &egui::Context, s: &AppState, acts: &mut Vec<Action>) {
    let Some(m) = &s.move_sync else { return };
    window(ctx, "move_sync_dlg", "Sync permissions?", 400.0, Action::AnswerMoveSync(None), acts, |ui, acts| {
        ui.label(rt(format!("Move #{} into {} and give it the category's permissions, or keep its own?", m.channel, m.category), 13.5).color(SECONDARY));
        ui.add_space(10.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if pill(ui, "Sync permissions", true).clicked() {
                acts.push(Action::AnswerMoveSync(Some(true)));
            }
            if pill(ui, "Keep its own", false).clicked() {
                acts.push(Action::AnswerMoveSync(Some(false)));
            }
        });
    });
}


/// Rows at the top of a server's channel list: Browse Channels (Community servers) and the switch that lists
/// channels this account can't see, locked.
pub fn server_rows(ui: &mut Ui, s: &AppState, server: &str, acts: &mut Vec<Action>) {
    let community = s.discord_online && s.servers.iter().any(|x| x.id == server && x.community);
    let rows: [(bool, &str, &str, bool); 2] = [
        (community, "\u{E8FD}", "Browse Channels", s.browse.is_some()),
        (true, "\u{E72E}", if s.show_hidden_channels { "Hide locked channels" } else { "Show locked channels" }, false),
    ];
    for (i, (show, g, label, on)) in rows.into_iter().enumerate() {
        if !show {
            continue;
        }
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width() - 16.0, 32.0), Sense::click());
        let r = r.translate(vec2(8.0, 0.0));
        if on || resp.hovered() {
            ui.painter().rect_filled(r, 10.0, if on { k_active() } else { k_hover() });
        }
        glyph(ui.painter(), egui::pos2(r.left() + 18.0, r.center().y), g, 14.0, SECONDARY, false);
        ui.painter().text(egui::pos2(r.left() + 38.0, r.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::new(13.5, th::semibold()), SECONDARY);
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            acts.push(if i == 0 { Action::BrowseChannels } else { Action::ShowHiddenChannels(!s.show_hidden_channels) });
        }
    }
    ui.add_space(4.0);
}

/// Browse Channels in place of the conversation: each category with its switch, each channel with its box;
/// a channel followed through its category changes only with the category.
pub fn browse(ui: &mut Ui, s: &AppState, acts: &mut Vec<Action>) {
    use mytune_chat::yapfast_core::application::BrowsedKind;
    let Some(b) = &s.browse else { return };
    egui::Frame::new().fill(k_chat()).inner_margin(egui::Margin::symmetric(18, 12)).show(ui, |ui| {
        ui.horizontal(|ui| {
            glyph(ui.painter(), egui::pos2(ui.cursor().left() + 10.0, ui.cursor().center().y + 2.0), "\u{E8FD}", 18.0, SECONDARY, false);
            ui.add_space(26.0);
            ui.label(sb("Browse Channels", 16.0).color(TEXT));
        });
        ui.label(rt("Tick the channels you want in your channel list.", 12.5).color(MUTED));
    });
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        egui::Frame::new().inner_margin(egui::Margin::symmetric(24, 8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            for row in &b.rows {
                if row.kind == BrowsedKind::Category {
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.label(sb(row.name.to_uppercase(), 11.5).color(MUTED));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let mut on = row.followed;
                            if ui.checkbox(&mut on, "Follow category").changed() {
                                acts.push(Action::BrowseFollow { room: row.id.clone(), follow: on });
                            }
                        });
                    });
                    continue;
                }
                let g = match row.kind {
                    BrowsedKind::Voice => "\u{E767}",
                    BrowsedKind::Forum => "\u{E8BD}",
                    BrowsedKind::Announcement => "\u{E789}",
                    _ => "#",
                };
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), if row.topic.is_empty() { 40.0 } else { 52.0 }), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(r, 10.0, k_hover());
                }
                if g == "#" {
                    ui.painter().text(egui::pos2(r.left() + 16.0, r.center().y), egui::Align2::CENTER_CENTER, "#", egui::FontId::new(18.0, th::semibold()), MUTED);
                } else {
                    glyph(ui.painter(), egui::pos2(r.left() + 16.0, r.center().y), g, 15.0, MUTED, false);
                }
                let dy = if row.topic.is_empty() { 0.0 } else { -8.0 };
                ui.painter().text(egui::pos2(r.left() + 36.0, r.center().y + dy), egui::Align2::LEFT_CENTER, &row.name, egui::FontId::new(14.0, th::semibold()), TEXT);
                if !row.topic.is_empty() {
                    let topic: String = row.topic.chars().take(110).collect();
                    ui.painter().text(egui::pos2(r.left() + 36.0, r.center().y + 10.0), egui::Align2::LEFT_CENTER, topic, egui::FontId::proportional(12.0), MUTED);
                }
                // the box: ticked when followed; one followed through its category shows it but can't change
                let b = egui::Rect::from_center_size(egui::pos2(r.right() - 20.0, r.center().y), vec2(18.0, 18.0));
                let lit = row.followed;
                ui.painter().rect(b, 4.0, if lit { p().accent } else { Color32::TRANSPARENT }, egui::Stroke::new(1.5, if lit { p().accent } else { MUTED }), egui::StrokeKind::Inside);
                if lit {
                    glyph(ui.painter(), b.center(), "\u{E73E}", 11.0, p().on_accent, false);
                }
                let resp = if row.by_category { resp.on_hover_text("Followed through its category") } else { resp.on_hover_cursor(egui::CursorIcon::PointingHand) };
                if resp.clicked() && !row.by_category {
                    acts.push(Action::BrowseFollow { room: row.id.clone(), follow: !row.followed });
                }
            }
        });
    });
}
