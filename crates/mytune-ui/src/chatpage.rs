//! Home: the Yapfast Discord client (engine `yapfast-core` through `mytune-chat`), laid out like Discord:
//! server rail | conversation list | messages. Skipped for now: threads, forums, attachments, reactions.
//! The engine state sits behind a mutex, so each frame collects `Action`s while reading and dispatches after.

use crate::theme::{self as th, p, rt, sb, text2, text3};
use eframe::egui::{self, vec2, Align, Color32, Layout, Sense, TextureHandle, Ui};
use mytune_chat::yapfast_core::application::{Action, AppState, DiscordLink, Navigation};
use mytune_chat::Chat;
use std::collections::{HashMap, HashSet};

mod callmenu;
mod compose;
mod dialogs;
mod extras;
mod forum;
mod friends;
mod markdown;
mod header;
mod inbox;
mod media;
mod menu;
mod switcher;
mod threads;
mod userbtns;
mod toast;
mod picker;
mod placemenu;
mod poll;
mod popout;
mod serversettings;
mod channelsettings;
mod serverpages;
mod settings;
mod voice;
mod voicenote;

#[derive(Default)]
pub struct ChatPage {
    call: voice::CallUi,
    header: header::HeaderUi,
    forum: forum::ForumUi,
    settings: settings::SettingsUi,
    forward: menu::ForwardUi,
    picker: picker::PickerUi,
    plus: poll::PlusUi,
    dialogs: dialogs::DialogsUi,
    extras: extras::ExtrasUi,
    /// The layout last saved (restored from chat-settings.json on the first frame).
    layout: Option<mytune_chat::ChatLayout>,
    server_settings: serversettings::SettingsUi,
    channel_ui: channelsettings::ChannelUi,
    /// Where the composer was last drawn, for the picker to open above it.
    composer: Option<egui::Rect>,
    /// What brought the timeline into view last (room, unread jump count, jump count, had messages), so a
    /// change scrolls once: to the "New" divider, a jumped-to message, or the newest message.
    scrolled: Option<(String, u64, u64, bool, bool)>,
    scroll: Option<Scroll>,
    /// The room open and the message being edited in it, so the composer takes the engine's draft once when the
    /// room changes or editing starts or ends (the engine keeps a draft per room).
    editing: (Option<String>, Option<String>),
    /// How many of the newest messages the timeline draws beyond the first 150; grows as it scrolls to its top.
    extra: usize,
    /// The oldest message drawn and the timeline's height last frame: when older ones come in above, the view
    /// moves down by what they added, so it stays on the same message.
    top: Option<(String, f32)>,
    /// The messages in view as last told to the engine (`Action::VisibleMessages`).
    vis: Option<(String, Vec<String>, bool)>,
    /// When to look whether pictures in view are still missing, and how often the view was told again since
    /// it last changed (see `signed_in`).
    vis_retry: (f64, u8),
    chat: Option<Chat>,
    error: Option<String>,
    started: bool,
    draft: String,
    tex: HashMap<String, TextureHandle>,
    asked: HashSet<String>,
    qr: Option<(String, TextureHandle)>,
    /// The room and whether its newest message was last told to the engine as read (`Action::Reading`).
    reading: Option<(String, bool)>,
    /// Messages shown when the next page of a detached window was last asked for (once per page).
    later_at: Option<usize>,
}

thread_local! {
    /// When each picture texture was last drawn (seconds of app time), and when the cache was last pruned.
    static TEX_USED: std::cell::RefCell<(HashMap<String, f64>, f64)> = std::cell::RefCell::new((HashMap::new(), 0.0));
}

/// Notes that the texture `key` is in use this frame.
fn touch(ctx: &egui::Context, key: &str) {
    let now = ctx.input(|i| i.time);
    TEX_USED.with(|u| {
        let mut u = u.borrow_mut(); // one borrow: a second one inside a match on the first panics
        match u.0.get_mut(key) {
            Some(t) => *t = now,
            None => {
                u.0.insert(key.to_owned(), now);
            }
        }
    });
}

/// Every 10 s, frees the textures not drawn for a minute (avatars and pictures of rooms left behind: they
/// were kept for the whole session). One that shows again is made again, or asked for again.
fn prune_textures(ctx: &egui::Context, cache: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>) {
    let now = ctx.input(|i| i.time);
    TEX_USED.with(|u| {
        let mut u = u.borrow_mut();
        if now - u.1 < 10.0 {
            return;
        }
        u.1 = now;
        let used = &mut u.0;
        cache.retain(|key, _| {
            let keep = used.get(key).is_some_and(|t| now - t < 60.0);
            if !keep {
                used.remove(key);
                asked.remove(key);
            }
            keep
        });
    });
}

/// The texture of a loaded picture; unknown ones are queued in `want` (once).
fn texture(ctx: &egui::Context, cache: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, s: &AppState, url: &Option<String>, want: &mut Vec<String>) -> Option<egui::TextureId> {
    let url = url.as_ref()?;
    if let Some(t) = cache.get(url) {
        touch(ctx, url);
        return Some(t.id());
    }
    match s.pictures.get(url) {
        Some(pic) => {
            let img = egui::ColorImage::from_rgba_unmultiplied([pic.width as usize, pic.height as usize], &pic.rgba);
            let t = ctx.load_texture(url.clone(), img, th::SMOOTH);
            let id = t.id();
            crate::sharp::register(id, &pic.rgba, pic.width, pic.height); // small draws get exact-size copies
            touch(ctx, url);
            cache.insert(url.clone(), t);
            Some(id)
        }
        None => {
            if asked.insert(url.clone()) {
                want.push(url.clone());
            }
            None
        }
    }
}

/// A call's length as Discord writes it: `m:ss`, from an hour on `h:mm:ss`.
fn call_time(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Temp-data key of the open stream preview: (picture address, streamer's name, time opened).
const STREAM_PREVIEW: &str = "stream_preview";

/// A large picture of someone's live stream, opened from the LIVE pill in the channel list. The engine
/// asks Discord for it under the address `stream-preview:<stream key>#<time>`.
fn stream_preview(ctx: &egui::Context, s: &AppState, cache: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>) {
    let id = egui::Id::new(STREAM_PREVIEW);
    let Some((address, name, born)) = ctx.data(|d| d.get_temp::<(String, String, f64)>(id)) else { return };
    let mut open = true;
    egui::Window::new(format!("{name}'s stream")).id(id).open(&mut open).collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0)).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(vec2(720.0, 405.0), Sense::hover());
        match texture(ctx, cache, asked, s, &Some(address.clone()), want) {
            Some(t) => {
                ui.painter().image(t, rect, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            }
            None => {
                // the engine reports no failure: after a while, say there is none
                let waited = ctx.input(|i| i.time) - born;
                let text = if waited < 8.0 { "Loading preview\u{2026}" } else { "No preview available. The stream may have just started." };
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(14.0), MUTED);
                ctx.request_repaint_after(std::time::Duration::from_millis(500));
            }
        }
    });
    if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        ctx.data_mut(|d| d.remove::<(String, String, f64)>(id));
    }
}

// Yapfast's own dark palette (ui/theme.slint), so the chat reads as Yapfast inside MyTune.
/// Whether the small always-on-top song window is open, in egui's memory: the Music page's button flips it.
pub fn song_window_id() -> egui::Id {
    popout::song_id()
}

/// What plays on this PC with its buttons: the media bar's row (`pop`: with the button for its own window)
/// and the song window's.
fn media_row(ui: &mut Ui, now: &crate::nowplaying::Now, pop: bool) {
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            use crate::nowplaying::Cmd;
            let icon = |ui: &mut Ui, g: &str, tip: &str| ui.add(egui::Button::new(rt(g, 14.0).family(th::icons()).color(SECONDARY)).frame(false)).on_hover_text(tip).clicked();
            if pop && icon(ui, "\u{E8A7}", "Own window, always on top") {
                let id = popout::song_id();
                let open = ui.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
                ui.data_mut(|d| d.insert_temp(id, !open));
            }
            for (g, tip, cmd) in [("\u{E893}", "Next", Cmd::Next), (if now.playing { "\u{E769}" } else { "\u{E768}" }, if now.playing { "Pause" } else { "Play" }, Cmd::Toggle), ("\u{E892}", "Previous", Cmd::Previous)] {
                if icon(ui, g, tip) {
                    crate::nowplaying::send(cmd);
                }
            }
            ui.with_layout(Layout::top_down(Align::Min), |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.add(egui::Label::new(rt(if now.title.is_empty() { "Nothing playing" } else { &now.title }, 13.0).color(TEXT)).truncate());
                if !now.artist.is_empty() {
                    ui.add(egui::Label::new(rt(&now.artist, 11.5).color(MUTED)).truncate());
                }
            });
        });
    });
}

const TEXT: Color32 = Color32::from_rgb(0xee, 0xf0, 0xf4);
const SECONDARY: Color32 = Color32::from_rgb(0xb5, 0xb8, 0xc1);
const MUTED: Color32 = Color32::from_rgb(0x8d, 0x91, 0x9c);
const FAINT: Color32 = Color32::from_rgb(0x5c, 0x60, 0x6a);
const LIVE: Color32 = Color32::from_rgb(0x3e, 0xcf, 0x7e);
const DANGER: Color32 = Color32::from_rgb(0xf0, 0x54, 0x5e);

/// A round avatar: the picture, or the first letter on a soft circle.
fn avatar(ui: &Ui, rect: egui::Rect, tex: Option<egui::TextureId>, name: &str) {
    let r = rect.width() / 2.0;
    match tex {
        Some(id) => {
            egui::Image::new(egui::load::SizedTexture::new(id, rect.size())).corner_radius(r).paint_at(ui, rect);
        }
        None => {
            ui.painter().circle_filled(rect.center(), r, k_active());
            let c = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, c, egui::FontId::new(r * 0.95, th::semibold()), SECONDARY);
        }
    }
}

/// A server/DM button on the rail: a rounded square (rounder when selected) with an accent bar on its left.
fn rail_icon(ui: &mut Ui, tex: Option<egui::TextureId>, name: &str, selected: bool, unread: bool, pings: u64) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::click_and_drag()); // servers drag to reorder
    let pal = p();
    // circle to rounded square on hover or selection, and the left bar grows or shrinks, both eased
    let round = ui.ctx().animate_bool_with_time(resp.id.with("round"), selected || resp.hovered(), voice::QUICK);
    let radius = egui::lerp(24.0..=16.0, round);
    match tex {
        Some(id) => {
            // Black has no selection bar: the other icons are dimmed instead
            let tint = if pal.black && !selected && !resp.hovered() { Color32::from_gray(0x90) } else { Color32::WHITE };
            egui::Image::new(egui::load::SizedTexture::new(id, rect.size())).corner_radius(radius).tint(tint).paint_at(ui, rect);
        }
        None => {
            // the Direct messages tile is white when selected, whatever the accent (user's choice)
            let home = name == "@";
            if !pal.black {
                ui.painter().rect_filled(rect, radius, if selected && home { Color32::from_rgb(0xF2, 0xF2, 0xF2) } else if selected { pal.accent } else { k_active() });
            }
            let fg = if pal.black { th::hover_text(Color32::WHITE, selected || resp.hovered()) } else if selected && home { Color32::from_rgb(0x1C, 0x1C, 0x1C) } else if selected { pal.on_accent } else { TEXT };
            if name == "+" {
                // two 2 px bars on the exact centre: the font's "+" sits off its box
                let ppp = ui.ctx().pixels_per_point();
                let c = egui::pos2((rect.center().x * ppp).round() / ppp, (rect.center().y * ppp).round() / ppp);
                ui.painter().rect_filled(egui::Rect::from_center_size(c, vec2(14.0, 2.0)), 1.0, fg);
                ui.painter().rect_filled(egui::Rect::from_center_size(c, vec2(2.0, 14.0)), 1.0, fg);
            } else {
                let c = name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, c, egui::FontId::new(20.0, th::semibold()), fg);
            }
        }
    }
    // selected: a tall accent bar; unread: a short white pill on the left edge, as Discord does
    let target = if selected { if pal.black { 0.0 } else { 28.0 } } else if unread || pings > 0 { 8.0 } else { 0.0 };
    let h = ui.ctx().animate_value_with_time(resp.id.with("bar"), target, 0.16);
    if h > 0.5 {
        let bar = egui::Rect::from_center_size(egui::pos2(rect.left() - 12.0, rect.center().y), vec2(4.0, h));
        ui.painter().rect_filled(bar, 2.0, Color32::from_rgb(0xF2, 0xF2, 0xF2)); // white whatever the accent (user's choice)
    }
    if pings > 0 {
        let text = if pings > 99 { "99+".to_owned() } else { pings.to_string() };
        let w = 12.0 + 7.0 * text.len() as f32;
        let badge = egui::Rect::from_min_size(egui::pos2(rect.right() - w + 6.0, rect.bottom() - 16.0), vec2(w, 18.0));
        let red = Color32::from_rgb(0xF2, 0x3F, 0x42);
        ui.painter().rect_filled(badge.expand(2.0), 11.0, k_rail());
        // Black: the number in red on plain black, so it stays readable over the server's picture
        ui.painter().rect_filled(badge, 14.0, if pal.black { Color32::BLACK } else { red });
        ui.painter().text(badge.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::new(11.0, th::semibold()), if pal.black { red } else { Color32::WHITE });
    }
    resp
}

/// The rail order: servers by their place in `order` (dragged, this device only); unlisted ones first, in
/// the engine's order.
fn rail_order(ids: &[&str], order: &[String]) -> Vec<usize> {
    let mut at: Vec<usize> = (0..ids.len()).collect();
    at.sort_by_key(|&i| order.iter().position(|o| o == ids[i]).map(|p| p + 1).unwrap_or(0));
    at
}

/// One line of the server hover card: an icon, up to five avatars and "+N" for the rest.
fn voice_row(ui: &mut Ui, icon: &str, people: &[&mytune_chat::yapfast_core::chat::VoicePerson], mut tex_of: impl FnMut(&Option<String>) -> Option<egui::TextureId>) {
    let n = people.len().min(5);
    let more = people.len() - n;
    let (rect, _) = ui.allocate_exact_size(vec2(28.0 + n as f32 * 24.0 + if more > 0 { 30.0 } else { 0.0 }, 24.0), Sense::hover());
    voice::glyph(ui.painter(), egui::pos2(rect.left() + 9.0, rect.center().y), icon, 15.0, MUTED, false);
    for (i, who) in people.iter().take(n).enumerate() {
        let r = egui::Rect::from_min_size(egui::pos2(rect.left() + 28.0 + i as f32 * 24.0, rect.top() + 1.0), vec2(22.0, 22.0));
        avatar(ui, r, tex_of(&who.avatar), &who.name);
    }
    if more > 0 {
        ui.painter().text(egui::pos2(rect.right() - 26.0, rect.center().y), egui::Align2::LEFT_CENTER, format!("+{more}"), egui::FontId::new(12.0, th::semibold()), SECONDARY);
    }
}

const ICON_VOICE: &str = "\u{E767}";
const ICON_SCREEN: &str = "\u{E7F4}";

/// `text` cut with an ellipsis so it fits `max_w` in `font`, as Discord does.
fn ellipsize(ui: &Ui, text: &str, font: &egui::FontId, max_w: f32) -> String {
    let width = |t: &str| ui.painter().layout_no_wrap(t.to_owned(), font.clone(), Color32::WHITE).size().x;
    if width(text) <= max_w {
        return text.to_owned();
    }
    let mut chars: Vec<char> = text.chars().collect();
    while chars.pop().is_some() {
        let t: String = chars.iter().collect::<String>().trim_end().to_owned() + "\u{2026}";
        if width(&t) <= max_w {
            return t;
        }
    }
    "\u{2026}".to_owned()
}

/// A small rounded button.
/// A button that destroys something at once acts on its second click: the first turns its label into "Sure?"
/// for 4 s. `draw` draws the button with the label given and says whether it was clicked.
fn sure(ui: &mut Ui, salt: impl std::hash::Hash + std::fmt::Debug, label: &str, draw: impl FnOnce(&mut Ui, &str) -> bool) -> bool {
    let (key, id) = (egui::Id::new("armed_button"), egui::Id::new(("sure", salt)));
    let now = ui.input(|i| i.time);
    let armed = ui.data(|d| d.get_temp::<(egui::Id, f64)>(key)).is_some_and(|(a, t)| a == id && now - t < 4.0);
    if !draw(ui, if armed { "Sure?" } else { label }) {
        return false;
    }
    if armed {
        ui.data_mut(|d| d.remove::<(egui::Id, f64)>(key));
    } else {
        ui.data_mut(|d| d.insert_temp(key, (id, now)));
        ui.ctx().request_repaint_after(std::time::Duration::from_secs(4));
    }
    armed
}

fn pill(ui: &mut Ui, text: &str, primary: bool) -> egui::Response {
    th::button(ui, text, primary, (12.0, 3.0, 12.0, 4.0))
}

/// Messages, Discord style: a run by one author within 7 minutes shares one header; only the last 150 are drawn.
/// How messages show, from the chat settings: clock and text size.
#[derive(Clone, Copy)]
pub struct Look {
    pub twelve_hour: bool,
    pub seconds: bool,
    pub text: f32,
}

impl Look {
    fn of(chat: &Chat) -> Look {
        chat.with_settings(|st| Look { twelve_hour: st.twelve_hour_clock, seconds: st.clock_seconds, text: [13.0, 14.5, 16.5][usize::from(st.text_size.min(2))] })
    }
}

/// A message's text with colour emoji (Twemoji) and its server emoji (`:name:`) as pictures, as Discord shows
/// them; a message of only emoji shows them large. Plain text stays one selectable label.
fn rich_body(ui: &mut Ui, s: &AppState, m: &mytune_chat::yapfast_core::chat::ChatMessage, size: f32, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    if markdown::has_markup(&m.body) || !m.mentions.is_empty() {
        markdown::body(ui, s, m, size, tex, asked, want, acts);
        return;
    }
    let color = Color32::from_rgb(0xDD, 0xDF, 0xE5);
    let custom = !m.emoji.is_empty() && m.body.contains(':');
    if !custom && !crate::emoji::has_emoji(&m.body) {
        ui.add(egui::Label::new(rt(&m.body, size).color(color)).wrap().selectable(true));
        return;
    }
    // pieces: text, a standard emoji, or a server emoji's picture address
    enum P<'a> {
        T(&'a str),
        E(&'a str),
        C(&'a str, &'a str),
    }
    let mut pieces: Vec<P> = Vec::new();
    for piece in crate::emoji::split(&m.body) {
        match piece {
            crate::emoji::Piece::Emoji(e) => pieces.push(P::E(e)),
            crate::emoji::Piece::Text(mut t) => {
                // server emoji show as their `:name:` in the text
                // (an empty name would match for ever)
                while let Some((at, c)) = m.emoji.iter().filter(|c| custom && !c.text.is_empty()).filter_map(|c| t.find(c.text.as_str()).map(|at| (at, c))).min_by_key(|(at, _)| *at) {
                    if at > 0 {
                        pieces.push(P::T(&t[..at]));
                    }
                    pieces.push(P::C(&c.text, &c.picture));
                    t = &t[at + c.text.len()..];
                }
                if !t.is_empty() {
                    pieces.push(P::T(t));
                }
            }
        }
    }
    // no emoji after all (smart quotes, dashes, other scripts): one label, not one per word
    if pieces.iter().all(|p| matches!(p, P::T(_))) {
        ui.add(egui::Label::new(rt(&m.body, size).color(color)).wrap().selectable(true));
        return;
    }
    let only = pieces.len() <= 27 && pieces.iter().all(|p| !matches!(p, P::T(t) if !t.trim().is_empty()));
    let es = if only { 44.0 } else { (size * 1.35).round() };
    let ctx = ui.ctx().clone();
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 2.0);
        for p in &pieces {
            match p {
                P::T(t) => {
                    for (i, line) in t.split('\n').enumerate() {
                        if i > 0 {
                            ui.end_row();
                        }
                        for word in line.split_inclusive(' ') {
                            ui.add(egui::Label::new(rt(word, size).color(color)).selectable(true));
                        }
                    }
                }
                P::E(e) => match crate::emoji::texture(&ctx, e) {
                    Some(t) => {
                        ui.add(egui::Image::new(egui::load::SizedTexture::new(t, vec2(es, es)))).on_hover_text(*e);
                    }
                    None => {
                        ui.label(rt(*e, size).color(color));
                    }
                },
                P::C(name, url) => match texture(&ctx, tex, asked, s, &Some((*url).to_owned()), want) {
                    Some(t) => {
                        ui.add(egui::Image::new(egui::load::SizedTexture::new(t, vec2(es, es)))).on_hover_text(*name);
                    }
                    None => {
                        ui.label(rt(*name, size).color(color));
                    }
                },
            }
        }
    });
}

/// What `stamp` a message row was drawn with: when any of it changes, its remembered height is not used.
type RowStamp = (usize, usize, usize, usize, bool, bool, bool);

thread_local! {
    /// The height each message row had when last drawn, by item, with the width and stamp it was drawn at.
    static HEIGHTS: std::cell::RefCell<HashMap<String, (f32, RowStamp, f32)>> = std::cell::RefCell::new(HashMap::new());
}

/// The newest messages the timeline draws when a room opens, and how many more each time the view nears the
/// top. Every one is laid out in the frame it first shows: 150 at once was a visible hitch on opening a room.
const DRAWN: usize = 50;

/// Where the timeline should scroll once.
pub enum Scroll {
    /// This message to the top (the first unread, or one jumped to).
    To(String),
    /// The newest message.
    Bottom,
}

/// Placeholder rows while a room's first messages load: grey avatar, name and text bars in the timeline's
/// own measures (40 px avatar, 12 px gap, 16 px between rows), with a slow pulse.
fn skeleton(ui: &mut Ui) {
    // inside a scroll area the free height is endless: only what is on screen
    let area = ui.available_rect_before_wrap().intersect(ui.clip_rect());
    let pulse = (ui.input(|i| i.time) * 2.2).sin() as f32 * 0.5 + 0.5;
    let fill = Color32::from_white_alpha((9.0 + 9.0 * pulse) as u8);
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    let p = ui.painter().with_clip_rect(area);
    let wide = (area.width() - 92.0).max(80.0);
    let (mut y, mut i) = (area.top() + 16.0, 0usize);
    while y < area.bottom() {
        let x = area.left() + 20.0;
        p.circle_filled(egui::pos2(x + 20.0, y + 20.0), 20.0, fill);
        let tx = x + 52.0;
        p.rect_filled(egui::Rect::from_min_size(egui::pos2(tx, y + 2.0), vec2(90.0 + (i * 37 % 70) as f32, 12.0)), 6.0, fill);
        // one to three text lines of uneven length, the same every frame
        let lines = 1 + i * 7 % 3;
        for l in 0..lines {
            let part = [0.82, 0.55, 0.68, 0.36, 0.74][(i * 3 + l) % 5];
            p.rect_filled(egui::Rect::from_min_size(egui::pos2(tx, y + 24.0 + l as f32 * 20.0), vec2(wide * part, 12.0)), 6.0, fill);
        }
        y += 24.0 + lines as f32 * 20.0 + 16.0;
        i += 1;
    }
    ui.allocate_rect(area, Sense::hover());
}

#[allow(clippy::too_many_arguments)]
fn message_list(ui: &mut Ui, s: &AppState, msgs: &[std::sync::Arc<mytune_chat::yapfast_core::chat::ChatMessage>], tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>, look: Look, first_unread: Option<&str>, scroll: &mut Option<Scroll>, visible: &mut Vec<String>, depth: usize) {
    let ctx = ui.ctx().clone();
    ctx.request_repaint_after(std::time::Duration::from_secs(60)); // Today / Yesterday labels move on at midnight
    ui.spacing_mut().item_spacing.y = 0.0;
    let n = msgs.len();
    // what is above the oldest message: a fixed 28 px, so the list does not jump as its text changes
    let (slot, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    let above = |text: &str| ui.painter().text(slot.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(12.5), MUTED);
    if depth == usize::MAX {
        // a thread beside the room: it loads its own history
    } else if s.history_failed {
        above("Older messages did not load. Click to try again.");
        if ui.interact(slot, ui.id().with("earlier_retry"), Sense::click()).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            acts.push(Action::Earlier);
        }
    } else if s.loading_history {
        above("Loading older messages\u{2026}");
    } else if s.history_start && n <= depth {
        above("This is the start of the conversation.");
    }
    let mut prev: Option<(&str, u64)> = None;
    let mut prev_day = None;
    for m in msgs.iter().skip(n.saturating_sub(depth)).filter(|m| !m.deleted) {
        // in or near the viewport: the engine loads the pictures of these messages
        let (top, clip) = (ui.cursor().top(), ui.clip_rect());
        if top > clip.top() - 900.0 && top < clip.bottom() + 900.0 {
            visible.push(m.item.clone());
        }
        // A message well outside the view takes the height it had when last drawn and is not laid out: laying
        // out every message each frame is what made scrolling slow (worst in a call, which repaints all the time).
        let stamp = (m.body.len(), m.reactions.len(), m.attachments.len(), m.embeds.len(), m.edited, m.reply.is_some(), first_unread == Some(m.item.as_str()));
        let width = ui.available_width().round();
        let target = matches!(scroll, Some(Scroll::To(item)) if *item == m.item);
        let known = HEIGHTS.with(|c| c.borrow().get(&m.item).filter(|k| k.0 == width && k.1 == stamp).map(|k| k.2));
        if let Some(h) = known.filter(|h| !target && (top + h < clip.top() - 400.0 || top > clip.bottom() + 400.0)) {
            ui.allocate_exact_size(vec2(width, h), Sense::hover());
            let day = mytune_chat::yapfast_core::clock::date(m.timestamp);
            if day.is_some() && day != prev_day {
                prev_day = day;
            }
            prev = if m.notice.is_some() { None } else { Some((m.sender.as_str(), m.timestamp)) };
            continue;
        }
        let remember = |ui: &Ui| {
            HEIGHTS.with(|c| {
                let mut c = c.borrow_mut();
                if c.len() > 4000 {
                    c.clear();
                }
                c.insert(m.item.clone(), (width, stamp, ui.cursor().top() - top));
            })
        };
        // a run by one author within 7 minutes shares one header
        let new = first_unread == Some(m.item.as_str());
        // a divider with the full date where the day changes, as Discord shows it
        let day = mytune_chat::yapfast_core::clock::date(m.timestamp);
        let day_break = day.is_some() && day != prev_day;
        if day_break {
            // Today and Yesterday by the calendar now (recomputed each frame, so they move on at midnight), else the date
            let label = {
                let c = mytune_chat::yapfast_core::clock::date(mytune_chat::yapfast_core::clock::now_ms() / 1000);
                match (day, c) {
                    (Some(d), Some(t)) if d == t => "Today".to_string(),
                    (Some(d), Some(t)) if t.pred_opt() == Some(d) => "Yesterday".to_string(),
                    _ => mytune_chat::yapfast_core::clock::day(m.timestamp),
                }
            };
            ui.add_space(14.0);
            let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
            let y = row.center().y;
            let font = egui::FontId::new(11.5, th::semibold());
            let w = ui.painter().layout_no_wrap(label.clone(), font.clone(), MUTED).size().x + 16.0;
            let mid = egui::pos2((row.left() + row.right() - 12.0) / 2.0, y);
            for (x0, x1) in [(row.left() + 12.0, mid.x - w / 2.0), (mid.x + w / 2.0, row.right() - 24.0)] {
                ui.painter().line_segment([egui::pos2(x0, y), egui::pos2(x1, y)], egui::Stroke::new(1.0, k_line()));
            }
            ui.painter().text(mid, egui::Align2::CENTER_CENTER, label, font, MUTED);
            ui.add_space(4.0);
        }
        if day.is_some() {
            prev_day = day;
        }
        // the "New" divider above the first unread message, which starts a fresh header
        if new {
            ui.add_space(10.0);
            let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 14.0), Sense::hover());
            let y = line.center().y;
            let tag = egui::Rect::from_min_size(egui::pos2(line.right() - 40.0, y - 7.0), vec2(36.0, 14.0));
            if !p().black {
                ui.painter().line_segment([egui::pos2(line.left() + 8.0, y), egui::pos2(line.right() - 44.0, y)], egui::Stroke::new(1.0, DANGER));
                ui.painter().rect_filled(tag, 10.0, DANGER);
            }
            ui.painter().text(tag.center(), egui::Align2::CENTER_CENTER, "NEW", egui::FontId::new(9.5, th::bold()), if p().black { DANGER } else { Color32::WHITE });
            if matches!(scroll, Some(Scroll::To(item)) if *item == m.item) {
                ui.scroll_to_rect(line, Some(Align::TOP));
                *scroll = None;
            }
        }
        // a notice (call, join, pin, thread, poll result): one line with an icon, as Discord shows them
        if let Some(nt) = &m.notice {
            use mytune_chat::yapfast_core::chat::NoticeKind;
            let (g, c) = match nt.kind {
                NoticeKind::Call => ("\u{E717}", LIVE),
                NoticeKind::Joined => ("\u{E72A}", LIVE),
                NoticeKind::Boosted => ("\u{E734}", th::WARNING),
                NoticeKind::Pinned(_) => ("\u{E718}", SECONDARY),
                NoticeKind::ThreadStarted => ("\u{E8BD}", SECONDARY),
                NoticeKind::PollResult => ("\u{E9D9}", SECONDARY),
            };
            ui.add_space(10.0);
            let row = ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let (ir, _) = ui.allocate_exact_size(vec2(60.0, 20.0), Sense::hover());
                voice::glyph(ui.painter(), egui::pos2(ir.left() + 28.0, ir.center().y), g, 15.0, c, false);
                ui.label(sb(&m.author, 14.5).color(TEXT));
                ui.label(rt(format!(" {}", nt.text.trim_start()), 14.5).color(SECONDARY));
                ui.label(rt(format!("  {}", m.time), 11.0).color(MUTED));
            });
            let mut rect = row.response.rect;
            rect.max.x = ui.max_rect().right();
            if nt.kind != NoticeKind::Call {
                menu::watch(ui, rect, &m.item); // call notices have no message menu
            }
            if let NoticeKind::Pinned(Some(item)) = &nt.kind {
                if ui.interact(rect, ui.id().with(("pinned", &m.item)), Sense::click()).on_hover_text("Jump to the pinned message").clicked() {
                    acts.push(Action::Jump(item.clone()));
                }
            }
            if matches!(scroll, Some(Scroll::To(item)) if *item == m.item) {
                ui.scroll_to_rect(rect, Some(Align::TOP));
                *scroll = None;
            }
            prev = None;
            remember(ui);
            continue;
        }
        let grouped = !new && !day_break && m.reply.is_none() && prev.is_some_and(|(who, t)| who == m.sender && m.timestamp.saturating_sub(t) < 420);
        prev = Some((m.sender.as_str(), m.timestamp));
        let bg = ui.painter().add(egui::Shape::Noop);
        // where the row above ended: this row's hover band starts here, so bands never overlap or leave gaps
        let band_top = ui.cursor().top() - ui.spacing().item_spacing.y;
        ui.add_space(if grouped { 5.0 } else { 16.0 });
        let row_top = ui.cursor().top(); // above the reply line, so the highlight covers it too
        // a reply: a line with the replied-to author's small avatar, "@name" and the start of their text above the
        // message, joined to this author's picture by a curve; a click jumps to the original, as in Discord
        if let Some(rp) = &m.reply {
            // not below zero: with no width left (window shrunk to nothing) egui stopped the app on the negative
            // size ("Negative child size: [-8.0 20.0]", the crash of 2026-11-19)
            let (rr, resp) = ui.allocate_exact_size(vec2((ui.available_width() - 8.0).max(0.0), 20.0), Sense::click());
            let (x0, y) = (rr.left() + 28.0, rr.center().y);
            let st = egui::Stroke::new(2.0, FAINT);
            ui.painter().add(egui::Shape::line(vec![egui::pos2(x0, rr.bottom() + 2.0), egui::pos2(x0, y + 6.0), egui::pos2(x0 + 2.0, y + 2.5), egui::pos2(x0 + 6.0, y), egui::pos2(x0 + 18.0, y)], st));
            let t = texture(&ctx, tex, asked, s, &rp.avatar, want);
            avatar(ui, egui::Rect::from_center_size(egui::pos2(x0 + 30.0, y), vec2(16.0, 16.0)), t, &rp.author);
            let name = format!("@{}", rp.author);
            let nf = egui::FontId::new(12.5, th::semibold());
            let nw = ui.painter().layout_no_wrap(name.clone(), nf.clone(), SECONDARY).size().x;
            let tx = x0 + 44.0;
            let clip = ui.painter().with_clip_rect(egui::Rect::from_min_max(egui::pos2(tx, rr.top()), egui::pos2(rr.right(), rr.bottom())));
            clip.text(egui::pos2(tx, y), egui::Align2::LEFT_CENTER, name, nf, SECONDARY);
            let snippet = if rp.body.trim().is_empty() { "Click to see attachment".to_owned() } else { rp.body.replace('\n', " ") };
            clip.text(egui::pos2(tx + nw + 6.0, y), egui::Align2::LEFT_CENTER, snippet, egui::FontId::proportional(12.5), MUTED);
            let resp = if rp.item.is_some() { resp.on_hover_cursor(egui::CursorIcon::PointingHand) } else { resp };
            if let (true, Some(item)) = (resp.clicked(), &rp.item) {
                acts.push(Action::Jump(item.clone()));
            }
        }
        let row = ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add_space(8.0);
            if grouped {
                ui.add_space(52.0);
            } else {
                let t = texture(&ctx, tex, asked, s, &m.avatar, want);
                let (r, resp) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::click());
                avatar(ui, r, t, &m.author);
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    header::remember_profile_at(&ctx);
                    acts.push(Action::ShowProfile { user: m.sender.clone(), large: false });
                }
                ui.add_space(12.0);
            }
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                if !grouped {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let c = m.color.map(|c| Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)).unwrap_or(TEXT);
                        if ui.add(egui::Label::new(sb(&m.author, 15.0).color(c)).sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            header::remember_profile_at(&ctx);
                            acts.push(Action::ShowProfile { user: m.sender.clone(), large: false });
                        }
                        let time = if look.twelve_hour || look.seconds { mytune_chat::yapfast_core::clock::time_with(m.timestamp, look.twelve_hour, look.seconds) } else { m.time.clone() };
                        ui.label(rt(time, 11.0).color(MUTED));
                    });
                }
                rich_body(ui, s, m, look.text, tex, asked, want, acts);
                if m.edited {
                    ui.label(rt("(edited)", 10.5).color(FAINT));
                }
                // a message still on its way, or one Discord refused: say so, and offer to send it again
                {
                    use mytune_chat::yapfast_core::chat::Delivery;
                    match m.delivery {
                        Delivery::Sent => {}
                        Delivery::Sending => {
                            ui.label(rt("Sending\u{2026}", 11.0).color(FAINT));
                        }
                        Delivery::Waiting => {
                            ui.label(rt("Waiting for the connection\u{2026}", 11.0).color(FAINT));
                        }
                        Delivery::Failed => {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                ui.label(rt("Not sent.", 12.0).color(DANGER));
                                if ui.link(rt("Try again", 12.0)).clicked() {
                                    acts.push(Action::Retry(m.item.clone()));
                                }
                                if ui.link(rt("Discard", 12.0)).clicked() {
                                    acts.push(Action::CancelSend(m.item.clone()));
                                }
                            });
                        }
                    }
                }
                media::content(ui, s, m, tex, acts);
                threads::chip(ui, m, acts);
                menu::reactions(ui, s, m, tex, asked, want, acts);
            });
        });
        let mut rect = row.response.rect;
        rect.min.y = row_top;
        rect.max.x = ui.max_rect().right();
        menu::watch(ui, rect, &m.item);
        if matches!(scroll, Some(Scroll::To(item)) if *item == m.item) {
            ui.scroll_to_rect(rect, Some(Align::TOP));
            *scroll = None;
        }
        // the highlight takes 2 px of the 5 px gap on each side: it never reaches the line above or below. The pointer
        // is tested against the band from the row above's end to this row's end, which exactly one row owns
        let band = egui::Rect::from_min_max(egui::pos2(rect.left(), band_top), egui::pos2(rect.right(), rect.bottom() - 0.01));
        rect = rect.expand2(vec2(0.0, 2.0));
        if m.highlighted(new) {
            // it mentions this account (or is a new reply to it): a warm tint with a bar on its left, as Discord's
            let warm = Color32::from_rgb(0xF0, 0xB2, 0x32);
            ui.painter().set(bg, egui::Shape::rect_filled(rect, 10.0, warm.gamma_multiply(0.10)));
            ui.painter().rect_filled(egui::Rect::from_min_max(rect.min, egui::pos2(rect.left() + 2.0, rect.bottom())), 1.0, warm);
        } else if ui.rect_contains_pointer(band) {
            ui.painter().set(bg, egui::Shape::rect_filled(rect, 12.0, k_row_hover()));
        }
        remember(ui);
    }
    ui.add_space(12.0);
    // no unread one (or it is older than the messages drawn): the newest message, at the bottom
    // (kept until the bottom is reached: pictures and late pages still grow the list)
    if !msgs.is_empty() && scroll.is_some() {
        ui.scroll_to_cursor(Some(Align::BOTTOM));
        *scroll = Some(Scroll::Bottom);
    }
}

impl ChatPage {
    /// A DM call rings this account while another MyTune page is in front: the same card as on the chat page.
    pub fn call_notice(&mut self, ctx: &egui::Context) {
        let Some(chat) = self.chat.clone() else { return };
        let (mut acts, mut want) = (Vec::new(), Vec::new());
        let (tex, asked, cu) = (&mut self.tex, &mut self.asked, &mut self.call);
        chat.with_state(|s| voice::incoming_card(ctx, cu, s, tex, asked, &mut want, &mut acts, true));
        if !want.is_empty() {
            acts.push(Action::LoadPictures(want));
        }
        for a in acts {
            chat.dispatch(a);
        }
    }

    /// A DM call starts ringing this account while MyTune is behind other windows: its taskbar button flashes (once per call).
    pub fn call_flash(&mut self, ctx: &egui::Context) {
        let Some(chat) = self.chat.clone() else { return };
        let ringing = chat.with_state(|s| s.rooms.iter().find(|r| r.incoming && !(s.voice.active() && s.voice.room.as_ref() == Some(&r.id))).map(|r| r.id.clone()));
        let key = egui::Id::new("call_flash");
        let last: Option<String> = ctx.data(|d| d.get_temp(key)).flatten();
        if ringing != last {
            if ringing.is_some() && !ctx.input(|i| i.focused) {
                ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Critical));
            }
            ctx.data_mut(|d| d.insert_temp(key, ringing));
        }
    }

    /// A call is running or ringing here. False while the chat has not started.
    #[cfg(feature = "license")]
    pub fn call_busy(&self) -> bool {
        self.chat.as_ref().is_some_and(|chat| chat.with_state(|s| s.voice.active() || s.rooms.iter().any(|r| r.incoming)))
    }

    /// A call is really running, not only ringing. False while the chat has not started.
    #[cfg(feature = "license")]
    pub fn in_call(&self) -> bool {
        self.chat.as_ref().is_some_and(|chat| chat.with_state(|s| s.voice.active()))
    }

    /// Another MyTune page is in front: new messages are no longer read as they come.
    pub fn hidden(&mut self) {
        if let (Some(chat), Some((room, true))) = (&self.chat, &self.reading) {
            self.reading = Some((room.clone(), false));
            chat.dispatch(Action::Reading(false));
        }
    }

    fn start(&mut self, ctx: &egui::Context) {
        self.started = true;
        let c = ctx.clone();
        match Chat::new(move || c.request_repaint()) {
            Ok(chat) => {
                // Screenshot tests: Yapfast's offline preview (sample rooms, a fake call) in place of Discord.
                match std::env::var("MYTUNE_CHAT_PREVIEW") {
                    Ok(room) => {
                        chat.dispatch(Action::Preview);
                        if !room.is_empty() {
                            chat.dispatch(Action::Open(room.clone()));
                            chat.dispatch(Action::RequestJoinVoice(room));
                        }
                    }
                    Err(_) => chat.restore(),
                }
                self.chat = Some(chat);
            }
            Err(e) => self.error = Some(e.to_string()),
        }
    }

    pub fn ui(&mut self, ui: &mut Ui) {
        if !self.started {
            self.start(ui.ctx());
        }
        let Some(chat) = self.chat.clone() else {
            ui.add_space(24.0);
            ui.label(rt(self.error.clone().unwrap_or_else(|| "Chat is not available.".into()), 14.0).color(th::DANGER));
            return;
        };
        let mut acts: Vec<Action> = Vec::new();
        let signed_in = chat.with_state(|s| s.account.is_some() || s.preview);
        if signed_in {
            // Screenshot tests: open a settings page by number.
            if let Some(page) = std::env::var("MYTUNE_CHAT_SETTINGS").ok().and_then(|p| p.parse().ok()) {
                std::env::remove_var("MYTUNE_CHAT_SETTINGS");
                self.settings.open(page);
            }
            // Screenshot tests: open the Friends or Requests screen once the account is up.
            if let Ok(screen) = std::env::var("MYTUNE_CHAT_SCREEN") {
                if chat.with_state(|s| s.discord_user.is_some()) {
                    std::env::remove_var("MYTUNE_CHAT_SCREEN");
                    acts.push(Action::Navigate(Navigation::DirectMessages));
                    acts.push(Action::OpenDmScreen(if screen == "requests" { mytune_chat::yapfast_core::application::DmScreen::Requests } else { mytune_chat::yapfast_core::application::DmScreen::Friends }));
                }
            }
            // Screenshot tests: open the channel with this name once it is listed.
            if let Ok(name) = std::env::var("MYTUNE_CHAT_OPEN") {
                if let Some(id) = chat.with_state(|s| s.rooms.iter().find(|r| r.name.contains(name.as_str()) && s.selected.as_ref() != Some(&r.id)).map(|r| r.id.clone())) {
                    std::env::remove_var("MYTUNE_CHAT_OPEN");
                    acts.push(Action::Open(id));
                }
            }
            self.signed_in(ui, &chat, &mut acts);
        } else {
            self.sign_in(ui, &chat, &mut acts);
        }
        for a in acts {
            chat.dispatch(a);
        }
        // an attachment's Save: the save dialog on its own thread (on the UI thread it would freeze the app)
        if let Some((item, index, name)) = media::take_save(ui.ctx()) {
            let chat = chat.clone();
            std::thread::spawn(move || {
                if let Some(path) = rfd::FileDialog::new().set_title("Save attachment").set_file_name(&name).save_file() {
                    chat.dispatch(Action::SaveAttachment { item, index, path });
                }
            });
        }
        // Edit Group > Change Icon: the same, for the picture file
        if let Some(room) = dialogs::take_group_icon(ui.ctx()) {
            let chat = chat.clone();
            std::thread::spawn(move || {
                if let Some(path) = rfd::FileDialog::new().set_title("Choose a group icon").add_filter("Pictures", &["png", "jpg", "jpeg", "gif", "webp"]).pick_file() {
                    chat.dispatch(Action::ChangeGroup { room, change: mytune_chat::yapfast_core::chat::GroupChange::Icon(Some(path)) });
                }
            });
        }
    }

    fn sign_in(&mut self, ui: &mut Ui, chat: &Chat, acts: &mut Vec<Action>) {
        // the saved login is connecting: a wordmark and three dots in place of the sign-in page
        if chat.starting() && chat.with_state(|s| s.discord_link.is_none()) {
            let c = ui.max_rect().center();
            let fade = ui.ctx().animate_bool_with_time(egui::Id::new("chat_loading"), true, 0.4);
            let t = ui.input(|i| i.time) as f32;
            ui.painter().text(c - vec2(0.0, 14.0), egui::Align2::CENTER_CENTER, "MyTune", egui::FontId::new(26.0, th::display_semibold()), Color32::WHITE.gamma_multiply(fade));
            for i in 0..3 {
                let wave = (t * 4.5 - i as f32 * 0.7).sin().max(0.0);
                th::dot(ui.painter(), c + vec2((i as f32 - 1.0) * 14.0, 18.0), 3.0, p().accent.gamma_multiply(fade * (0.25 + 0.75 * wave)));
            }
            ui.ctx().request_repaint();
            return;
        }
        let (mut relogin, mut reconnect) = (false, false);
        ui.add_space(40.0);
        ui.vertical_centered(|ui| {
            ui.label(th::dsb("Yapfast", 30.0));
            ui.add_space(6.0);
            let link = chat.with_state(|s| s.discord_link.clone());
            let notice = chat.with_state(|s| s.notice.clone());
            match link {
                None => {
                    ui.label(rt("Link your Discord account to chat here.", 14.0).color(text2()));
                    ui.add_space(12.0);
                    if th::button(ui, "Link Discord with a QR code", true, (18.0, 8.0, 18.0, 9.0)).clicked() {
                        relogin = true;
                    }
                    ui.add_space(8.0);
                    if th::button(ui, "Reconnect with the saved login", false, (14.0, 8.0, 14.0, 9.0)).clicked() {
                        reconnect = true;
                    }
                }
                Some(DiscordLink::Starting) | Some(DiscordLink::Checking) => {
                    ui.label(rt("Asking Discord for a code\u{2026}", 14.0).color(text2()));
                }
                Some(DiscordLink::Code(addr)) => {
                    ui.label(rt("Open Discord on your phone, go to Settings > Scan QR Code, and scan this.", 14.0).color(text2()));
                    ui.add_space(12.0);
                    self.draw_qr(ui, &addr);
                }
                Some(DiscordLink::Scanned(user)) => {
                    ui.label(rt(format!("{user} scanned it. Confirm on your phone."), 14.0).color(text2()));
                }
                Some(DiscordLink::Captcha(_)) => {
                    ui.label(rt("Discord wants a captcha solved in your browser.", 14.0).color(text2()));
                }
                Some(DiscordLink::Failed(e)) => {
                    ui.label(rt(format!("Linking failed: {e}"), 14.0).color(th::DANGER));
                    ui.add_space(8.0);
                    if th::button(ui, "Try again", true, (18.0, 8.0, 18.0, 9.0)).clicked() {
                        relogin = true;
                    }
                }
            }
            if let Some(n) = notice {
                ui.add_space(10.0);
                ui.label(rt(format!("{n}"), 12.0).color(text3()));
            }
        });
        if relogin {
            chat.log_in_again();
            acts.push(Action::LinkDiscord);
        }
        if reconnect {
            chat.reconnect();
        }
    }

    fn draw_qr(&mut self, ui: &mut Ui, addr: &str) {
        let Some((w, dark)) = mytune_chat::qr(addr) else { return };
        let scale = 6usize;
        let quiet = 2usize;
        let side = (w + quiet * 2) * scale;
        let (rect, _) = ui.allocate_exact_size(vec2(side as f32, side as f32), Sense::hover());
        ui.painter().rect_filled(rect, 0.0, Color32::WHITE);
        for y in 0..w {
            for x in 0..w {
                if dark[y * w + x] {
                    let o = rect.min + vec2(((x + quiet) * scale) as f32, ((y + quiet) * scale) as f32);
                    ui.painter().rect_filled(egui::Rect::from_min_size(o, vec2(scale as f32, scale as f32)), 0.0, Color32::BLACK);
                }
            }
        }
    }

    fn signed_in(&mut self, ui: &mut Ui, chat: &Chat, acts: &mut Vec<Action>) {
        let ctx = ui.ctx().clone();
        // the layout as last left (member list, profile panel, hidden lists, panel widths), restored once
        let first = self.layout.is_none();
        let saved = match &self.layout {
            Some(l) => l.clone(),
            None => {
                let l = chat.with_settings(|st| st.layout.clone());
                ctx.data_mut(|d| d.insert_temp(egui::Id::new("chat_focus"), l.lists_hidden));
                // one switch for the right-hand panel (profile in DMs, member list elsewhere): open only when both were
                let on = l.members_open && l.profile_panel.unwrap_or(true);
                self.header.members_open = on;
                acts.push(Action::ShowProfilePanel(on));
                self.layout = Some(l.clone());
                l
            }
        };
        let mut widths = saved.widths.clone();
        prune_textures(&ctx, &mut self.tex, &mut self.asked);
        let mut order = saved.server_order.clone();
        let mut want: Vec<String> = Vec::new();
        let mut at_end = false;
        let (mut relogin, mut reconnect) = (false, false);
        let (tex, asked, draft, cu, hd, fo, su, fw, editing, pk, composer_rect, scrolled, scroll, pu) = (&mut self.tex, &mut self.asked, &mut self.draft, &mut self.call, &mut self.header, &mut self.forum, &mut self.settings, &mut self.forward, &mut self.editing, &mut self.picker, &mut self.composer, &mut self.scrolled, &mut self.scroll, &mut self.plus);
        let (extra, top, vis, vis_retry) = (&mut self.extra, &mut self.top, &mut self.vis, &mut self.vis_retry);
        let look = Look::of(chat);
        let dlg = &mut self.dialogs;
        let ex = &mut self.extras;
        let sset = &mut self.server_settings;
        let csu = &mut self.channel_ui;
        let mut later: settings::Later = Vec::new();
        chat.with_app(|app| {
            // the ten DMs at the top open without a wait even on the first click (ten requests, once a run)
            if app.state.discord_online {
                for r in app.state.rooms.iter().filter(|r| r.direct && !r.voice).take(10) {
                    chat.warm(&r.id);
                }
            }
            let s = &app.state;
            // switching between the chat and a settings screen fades the new screen in over 150 ms
            let screen: u8 = if s.channel_settings.is_some() { 1 } else if s.server_settings.is_some() { 2 } else if su.open { 3 } else { 0 };
            let (now, sid) = (ui.input(|i| i.time), egui::Id::new("chat_screen"));
            let since = match ui.ctx().data(|d| d.get_temp::<(u8, f64)>(sid)) {
                Some((shown, since)) if shown == screen => since,
                old => {
                    let since = if old.is_some() { now } else { f64::MIN }; // nothing to fade from on the first frame
                    ui.ctx().data_mut(|d| d.insert_temp(sid, (screen, since)));
                    since
                }
            };
            let f = ((now - since) / 0.15).clamp(0.0, 1.0) as f32;
            if f < 1.0 {
                ui.ctx().request_repaint();
            }
            ui.multiply_opacity(1.0 - (1.0 - f).powi(3));
            if let Some(cs) = &s.channel_settings {
                channelsettings::screen(ui, csu, app, cs, acts);
                return;
            }
            if let Some(ss) = &s.server_settings {
                serversettings::screen(ui, sset, chat, app, ss, tex, asked, &mut want, acts);
                dialogs::invite(&ctx, s, acts);
                dialogs::person(&ctx, dlg, s, acts);
                dialogs::leave(&ctx, s, acts);
                dialogs::mfa(&ctx, s, acts);
                serverpages::ownership(&ctx, ss, acts);
                if !want.is_empty() {
                    acts.push(Action::LoadPictures(std::mem::take(&mut want)));
                }
                return;
            }
            if su.open {
                settings::screen(ui, su, chat, &mut later, s, tex, asked, &mut want, acts);
                return;
            }
            // the server rail and list can be hidden (Ctrl+B or the button in the strip) to give the chat the room
            let focus_id = egui::Id::new("chat_focus");
            let mut focus: bool = ctx.data(|d| d.get_temp(focus_id)).unwrap_or(false);
            if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::B)) {
                focus = !focus;
                ctx.data_mut(|d| d.insert_temp(focus_id, focus));
            }
            // brand strip and left edge: the chat is a space of its own inside MyTune
            egui::Panel::top("chat_brand").exact_size(30.0).resizable(false).frame(egui::Frame::new().fill(p().side).inner_margin(egui::Margin::symmetric(14, 0))).show_separator_line(false).show_inside(ui, |ui| {
                let r = ui.max_rect();
                voice::glyph(ui.painter(), egui::pos2(r.left() + 10.0, r.center().y), "\u{E8BD}", 15.0, p().side_text, false);
                ui.painter().text(egui::pos2(r.left() + 28.0, r.center().y), egui::Align2::LEFT_CENTER, "DISCORD", egui::FontId::new(12.0, th::bold()), p().side_text);
                let tg = egui::Rect::from_center_size(egui::pos2(r.right() - 6.0, r.center().y), vec2(30.0, 24.0));
                let te = ui.interact(tg, egui::Id::new("focus_toggle"), Sense::click());
                if te.hovered() {
                    ui.painter().rect_filled(tg, 10.0, p().side_selected);
                }
                voice::glyph(ui.painter(), tg.center(), "\u{E700}", 15.0, p().side_text, false);
                if te.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Hide or show the server and conversation lists (Ctrl+B)").clicked() {
                    focus = !focus;
                    ctx.data_mut(|d| d.insert_temp(focus_id, focus));
                }
                inbox::button(ui, s, tg.translate(vec2(-34.0, 0.0)));
            });
            // connection bar, only while offline (the user bar already shows the name and status): it says the
            // messages are stale and offers to reconnect
            if !s.discord_online {
            egui::Panel::top("chat_status").frame(egui::Frame::new().fill(k_rail()).inner_margin(egui::Margin::symmetric(14, 7))).show_separator_line(false).show_inside(ui, |ui| {
                ui.horizontal(|ui| {
                    let online = s.discord_online;
                    let text = s.account_status.clone().unwrap_or_else(|| "Connecting to Discord...".into());
                    let (dot, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                    th::dot(ui.painter(), dot.center(), 4.0, if online { LIVE } else { th::WARNING });
                    ui.add_space(2.0);
                    ui.label(rt(if online { format!("Online as {}", s.account_name.clone().unwrap_or_default()) } else { format!("Offline: {}. Messages shown are the last saved ones.", text.trim_end_matches('.')) }, 12.0).color(SECONDARY));
                    if !online {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if pill(ui, "Log in again", false).clicked() {
                                relogin = true;
                            }
                            if pill(ui, "Reconnect", true).clicked() {
                                reconnect = true;
                            }
                        });
                    }
                });
            });
            }
            // server rail
            egui::Panel::left("chat_rail").exact_size(72.0).resizable(false).frame(egui::Frame::new().fill(k_sidebar())).show_separator_line(false).show_collapsible(ui, &mut !focus, |ui| {
                // Join a server: pinned under the rail, centred in it and on the same line as the user bar and
                // the message bar (their centres sit 41 px above the bottom: 18 + 46/2 and 24 + 34/2)
                egui::Panel::bottom("rail_join").exact_size(76.0).frame(egui::Frame::new()).show_separator_line(false).show_inside(ui, |ui| {
                    let r = ui.max_rect();
                    let spot = egui::Rect::from_center_size(egui::pos2(r.center().x, r.bottom() - 41.0), vec2(48.0, 48.0));
                    ui.scope_builder(egui::UiBuilder::new().max_rect(spot), |ui| {
                        if rail_icon(ui, None, "+", false, false, 0).on_hover_text("Join a server").clicked() {
                            acts.push(Action::OpenJoinServer);
                        }
                    });
                });
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(10.0);
                    ui.spacing_mut().item_spacing.y = 10.0;
                    ui.vertical_centered(|ui| {
                        let dm = matches!(s.navigation, Navigation::DirectMessages);
                        let rooms: HashMap<&str, (u64, u64, bool)> = s.rooms.iter().map(|r| (r.id.as_str(), (r.unread, r.mentions, r.muted))).collect();
                        let (mut dm_unread, mut dm_pings) = (0u64, 0u64);
                        for r in s.rooms.iter().filter(|r| r.direct) {
                            if !r.muted {
                                dm_unread += r.unread;
                            }
                            dm_pings += r.mentions;
                        }
                        if rail_icon(ui, None, "@", dm, dm_unread > 0, dm_pings).on_hover_text("Direct messages").clicked() {
                            acts.push(Action::Navigate(Navigation::DirectMessages));
                        }
                        let (line, _) = ui.allocate_exact_size(vec2(28.0, 2.0), Sense::hover());
                        ui.painter().rect_filled(line, 1.0, k_line());
                        let in_voice: HashMap<&str, &Vec<_>> = s.rooms.iter().filter(|r| r.voice && !r.people.is_empty()).map(|r| (r.id.as_str(), &r.people)).collect();
                        let drag_id = egui::Id::new("srv_drag");
                        let dragging: Option<String> = ctx.data(|d| d.get_temp(drag_id));
                        let ids: Vec<&str> = s.servers.iter().map(|sv| &*sv.id).collect();
                        let mut spots: Vec<(&str, egui::Rect)> = vec![];
                        for sv in rail_order(&ids, &order).into_iter().map(|i| &s.servers[i]) {
                            let on = matches!(&s.navigation, Navigation::Server(id) if *id == sv.id);
                            let t = texture(&ctx, tex, asked, s, &sv.icon, &mut want);
                            let talk: Vec<_> = sv.channels.iter().filter_map(|c| in_voice.get(c.as_str())).flat_map(|p| p.iter()).collect();
                            let live: Vec<_> = talk.iter().copied().filter(|p| p.streaming).collect();
                            let (mut un, mut pings) = (false, 0u64);
                            for c in &sv.channels {
                                if let Some(&(u, m, muted)) = rooms.get(c.as_str()) {
                                    un |= u > 0 && !muted && !sv.muted;
                                    pings += m;
                                }
                            }
                            let ri = rail_icon(ui, t, &sv.name, on, un, pings);
                            spots.push((&sv.id, ri.rect));
                            // someone in voice: a speaker at the top right; someone streaming: a screen
                            if !talk.is_empty() {
                                let c = egui::pos2(ri.rect.right() - 4.0, ri.rect.top() + 4.0);
                                ui.painter().circle_filled(c, 10.0, th::tone(0.13));
                                voice::glyph(ui.painter(), c, if live.is_empty() { ICON_VOICE } else { ICON_SCREEN }, 11.0, TEXT, false);
                            }
                            if dragging.is_none() && ri.hovered() {
                                // the name, then who is in voice and who is streaming, as Discord's hover card;
                                // shown at once, to the right of the icon (an egui tooltip waits and sits underneath)
                                let card = egui::Area::new(egui::Id::new("srv_card")).order(egui::Order::Tooltip).interactable(false);
                                card.fixed_pos(ri.rect.right_center() + vec2(16.0, 0.0)).pivot(egui::Align2::LEFT_CENTER).show(&ctx, |ui| th::float(ui, th::R_MENU, 10, |ui| {
                                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend); // the name on one line
                                    ui.label(egui::RichText::new(&*sv.name).color(TEXT).font(egui::FontId::new(14.0, th::semibold())));
                                    if !talk.is_empty() {
                                        voice_row(ui, ICON_VOICE, &talk, |u| texture(&ctx, tex, asked, s, u, &mut want));
                                    }
                                    if !live.is_empty() {
                                        voice_row(ui, ICON_SCREEN, &live, |u| texture(&ctx, tex, asked, s, u, &mut want));
                                    }
                                }));
                            }
                            if ri.drag_started() {
                                ctx.data_mut(|d| d.insert_temp(drag_id, sv.id.to_string()));
                            }
                            if ri.clicked() {
                                acts.push(Action::Navigate(Navigation::Server(sv.id.clone())));
                            }
                            if ri.secondary_clicked() {
                                placemenu::open(&ctx, &sv.id);
                            }
                        }
                        // a dragged server lands in the gap under the pointer (a white line marks it)
                        if let Some(d) = dragging {
                            let others: Vec<(&str, egui::Rect)> = spots.iter().copied().filter(|(id, _)| *id != d).collect();
                            let y = ctx.input(|i| i.pointer.interact_pos()).map(|p| p.y);
                            let gap = y.map(|y| others.iter().filter(|(_, r)| r.center().y < y).count());
                            if let (Some(k), false) = (gap, others.is_empty()) {
                                let ly = if k == 0 { others[0].1.top() - 5.0 } else { others[k - 1].1.bottom() + 5.0 };
                                let line = egui::Rect::from_center_size(egui::pos2(others[0].1.center().x, ly), vec2(40.0, 3.0));
                                ui.painter().rect_filled(line, 1.5, Color32::from_rgb(0xF2, 0xF2, 0xF2));
                            }
                            if !ctx.input(|i| i.pointer.primary_down()) {
                                if let Some(k) = gap {
                                    let mut new: Vec<String> = others.iter().map(|(id, _)| id.to_string()).collect();
                                    new.insert(k, d);
                                    order = new;
                                }
                                ctx.data_mut(|dd| dd.remove::<String>(drag_id));
                            }
                        }
                    });
                });
            });
            // conversations / channels, at the width it was dragged to last time (egui keeps the size meanwhile)
            if let Some(ps) = egui::PanelState::load(&ctx, ui.scope_id().with_salt(egui::IdSalt::new("chat_list"))) {
                widths.insert("chat_list".into(), ps.size().x);
            }
            egui::Panel::left("chat_list").resizable(true).default_size(saved.widths.get("chat_list").copied().unwrap_or(264.0)).size_range(180.0..=460.0).frame(egui::Frame::new().fill(k_sidebar())).show_separator_line(false).show_collapsible(ui, &mut !focus, |ui| {
                let title = match &s.navigation {
                    Navigation::DirectMessages => "Direct messages".to_string(),
                    Navigation::Server(id) => s.servers.iter().find(|x| &x.id == id).map(|x| x.name.clone()).unwrap_or_default(),
                };
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    ui.add_space(16.0);
                    ui.label(sb(title, 16.0).color(TEXT));
                    if matches!(s.navigation, Navigation::DirectMessages) {
                        // New Message: start a DM or group DM, as Discord's + beside Direct Messages
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.add_space(14.0);
                            if ui.add(egui::Button::new(rt("\u{E710}", 14.0).family(th::icons()).color(SECONDARY)).frame(false)).on_hover_text("New message").clicked() {
                                acts.push(Action::OpenNewDm);
                            }
                        });
                    }
                });
                ui.add_space(10.0);
                if matches!(s.navigation, Navigation::DirectMessages) {
                    friends::nav_rows(ui, s, acts);
                } else if let Navigation::Server(id) = &s.navigation {
                    extras::server_rows(ui, s, id, acts);
                }
                egui::Panel::bottom("chat_user").frame(egui::Frame::new().inner_margin(egui::Margin { left: 8, right: 8, top: 6, bottom: 24 })).show_separator_line(false).show_inside(ui, |ui| {
                    settings::user_bar(ui, chat, &mut later, su, s, tex, asked, &mut want, acts);
                });
                if s.voice.active() {
                    egui::Panel::bottom("chat_call").frame(egui::Frame::new()).show_separator_line(false).show_inside(ui, |ui| {
                        voice::dock(ui, cu, s, acts);
                    });
                }
                let sel = s.selected_room().map(|r| r.id.clone());
                egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    let mut last_cat: &str = "";
                    // DMs keep the engine's order, Discord's own (newest message first, kept current); re-sorting
                    // here by previews still loading made rows jump when a chat was opened. Message requests have
                    // their own screen (friends::nav_rows), as in Discord. Pinned DMs (right-click > Pin) come first,
                    // in the order they were pinned.
                    let listed: Vec<_> = s.scoped_rooms().filter(|r| !(r.direct && r.request)).collect();
                    let pinned = |r: &&mytune_chat::yapfast_core::chat::ChatRoom| r.direct && s.pinned.contains(&r.id);
                    let mut order: Vec<_> = s.pinned.iter().filter_map(|id| listed.iter().find(|r| &r.id == id && r.direct).copied()).collect();
                    order.extend(listed.iter().filter(|r| !pinned(r)).copied());
                    // dragging a channel or category to move it (Manage Channels): the rows as shown, by room id,
                    // and where each is, for `Application::landing`
                    let drag_id = egui::Id::new("chan_drag");
                    let dragging: Option<usize> = ctx.data(|d| d.get_temp(drag_id));
                    let (mut shown, mut spots): (Vec<String>, Vec<egui::Rect>) = (vec![], vec![]);
                    for r in order {
                        if !r.direct && r.category != last_cat {
                            last_cat = &r.category;
                            if !last_cat.is_empty() {
                                ui.add_space(8.0);
                                let head = ui.horizontal(|ui| {
                                    ui.add_space(16.0);
                                    ui.label(rt(last_cat.to_uppercase(), 11.0).color(MUTED));
                                });
                                // right-click a category: Mark As Read, mute, create channel...
                                if let Some(cid) = &r.category_id {
                                    let sense = if app.draggable(cid) { Sense::click_and_drag() } else { Sense::click() };
                                    let resp = ui.interact(head.response.rect, ui.id().with(("cat", &**cid)), sense);
                                    shown.push(cid.to_string());
                                    spots.push(head.response.rect);
                                    if resp.drag_started() {
                                        ctx.data_mut(|d| d.insert_temp(drag_id, shown.len() - 1));
                                    }
                                    if resp.secondary_clicked() {
                                        placemenu::open(&ctx, cid);
                                    }
                                }
                            }
                        }
                        let on = sel.as_deref() == Some(r.id.as_str());
                        let unread = r.unread > 0 || r.mentions > 0;
                        let w = ui.available_width() - 16.0;
                        // the click area is the row as drawn: 8 px in from both sides
                        let (full, _) = ui.allocate_exact_size(vec2(w + 16.0, 44.0), Sense::hover());
                        let rect = full.shrink2(vec2(8.0, 0.0));
                        let drags = !r.direct && app.draggable(&r.id);
                        let resp = ui.interact(rect, ui.id().with(("room", &r.id)), if drags { Sense::click_and_drag() } else { Sense::click() });
                        if !r.direct {
                            shown.push(r.id.clone());
                            spots.push(rect);
                            if resp.drag_started() {
                                ctx.data_mut(|d| d.insert_temp(drag_id, shown.len() - 1));
                            }
                        }
                        if on || resp.hovered() {
                            ui.painter().rect_filled(rect, 12.0, if on { k_active() } else { k_hover() });
                        }
                        let mut x = rect.left() + 10.0;
                        let row_bg = if on { k_active() } else if resp.hovered() { k_hover() } else { k_sidebar() };
                        if r.direct && r.person.is_none() && r.group_avatars.len() >= 2 {
                            // group DM, as Discord draws it: two members' pictures, the front one bottom left with a
                            // ring of the row colour cutting into the one behind
                            let top = rect.center().y - 16.0;
                            for (k, url) in r.group_avatars.iter().take(2).enumerate() {
                                let av = egui::Rect::from_min_size(if k == 0 { egui::pos2(x + 10.0, top) } else { egui::pos2(x, top + 10.0) }, vec2(22.0, 22.0));
                                let t = texture(&ctx, tex, asked, s, &Some(url.clone()), &mut want);
                                avatar(ui, av, t, &r.name);
                                // a light ring: a near-black picture read as a blob (the row colour is see-through)
                                ui.painter().circle_stroke(av.center(), 11.0, egui::Stroke::new(1.0, Color32::from_white_alpha(0x50)));
                            }
                            x += 42.0;
                        } else if r.direct {
                            let t = texture(&ctx, tex, asked, s, &r.avatar, &mut want);
                            let av = egui::Rect::from_min_size(egui::pos2(x, rect.center().y - 16.0), vec2(32.0, 32.0));
                            avatar(ui, av, t, &r.name);
                            // online, idle, do not disturb or offline, for one-person DMs (not groups)
                            if r.person.is_some() {
                                header::status_dot_k(ui, av, r.presence, row_bg, 0.14);
                            }
                            x += 42.0;
                        } else {
                            ui.painter().text(egui::pos2(x + 2.0, rect.center().y), egui::Align2::LEFT_CENTER, if r.voice { "\u{266A}" } else { "#" }, egui::FontId::proportional(17.0), MUTED);
                            x += 22.0;
                        }
                        let color = if on || unread || (p().black && resp.hovered()) { TEXT } else { SECONDARY };
                        let font = if unread { th::semibold() } else { egui::FontFamily::Proportional };
                        // under a DM's name (never the last message, by the user's choice): the voice channel
                        // they are in (speaker icon), else the game they play (green gamepad), else their custom status
                        let sub: Option<(Option<(&str, Color32)>, String)> = if r.direct {
                            // a call in this DM: "In call" with the speaker (user's choice 2026-11-17, replaces
                            // the rows of people under the DM)
                            let in_call = (!r.people.is_empty()).then(|| ("In call".to_owned(), r.people.iter().any(|x| x.streaming)));
                            let in_voice = in_call.or_else(|| match &r.person {
                                Some(p) => s.rooms.iter().find(|v| !v.direct && v.voice && v.people.iter().any(|x| &x.id == p)).map(|v| (tidy(&v.name), v.people.iter().any(|x| &x.id == p && x.streaming))),
                                None => None,
                            });
                            let game =|| r.activities.iter().find(|a| a.kind == mytune_chat::yapfast_core::chat::ActivityKind::Playing).map(|a| a.name.to_string());
                            in_voice
                                // green speaker; sharing their screen: the red LIVE pill instead
                                .map(|(v, live)| (Some((if live { "LIVE" } else { "\u{E767}" }, LIVE)), v))
                                .or_else(|| game().map(|g| (Some(("\u{E7FC}", LIVE)), g)))
                                .or_else(|| r.activity.as_deref().map(|a| (None, markdown::plain(a))))
                                // a group DM: how many are in it, this account included
                                .or_else(|| r.person.is_none().then(|| (None, format!("{} Members", r.members))))
                                .filter(|(_, t)| !t.trim().is_empty())
                        } else {
                            None
                        };
                        let dy = if sub.is_some() { -8.0 } else { 0.0 };
                        // a server voice channel with people in it: how long its call has been going, at the right.
                        // Discord's start time when the engine has it, else since this app first saw the call.
                        let mut timer_w = 0.0;
                        if r.voice && !r.direct {
                            let seen = egui::Id::new(("vc_seen", &r.id));
                            if r.people.is_empty() {
                                ui.data_mut(|d| d.remove::<u64>(seen));
                            } else {
                                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                                let first = ui.data_mut(|d| *d.get_temp_mut_or(seen, now));
                                let start = r.id.rsplit(':').next().and_then(mytune_chat::yapfast_core::yapfast_discord::voice_start::get).unwrap_or(first);
                                let g = ui.painter().layout_no_wrap(call_time(now.saturating_sub(start)), egui::FontId::proportional(11.5), MUTED);
                                timer_w = g.size().x + 8.0;
                                ui.painter().galley(egui::pos2(rect.right() - 26.0 - g.size().x, rect.center().y - g.size().y / 2.0), g, MUTED);
                                ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
                            }
                        }
                        let room_w = (rect.right() - x - 36.0 - timer_w).max(40.0);
                        let nf = egui::FontId::new(14.0, font);
                        let name = ellipsize(ui, &tidy(&r.name), &nf, room_w);
                        ui.painter().text(egui::pos2(x, rect.center().y + dy), egui::Align2::LEFT_CENTER, name, nf, color);
                        if let Some((icon, t)) = sub {
                            let f = egui::FontId::new(11.5, egui::FontFamily::Proportional);
                            let mut tx = x;
                            if let Some(("LIVE", _)) = icon {
                                let g = ui.painter().layout_no_wrap("LIVE".to_owned(), egui::FontId::new(9.5, th::bold()), Color32::WHITE);
                                let pill = egui::Rect::from_min_size(egui::pos2(x, rect.center().y + 2.0), vec2(g.size().x + 10.0, 14.0));
                                if !p().black {
                                    ui.painter().rect_filled(pill, 4.0, DANGER);
                                }
                                ui.painter().galley_with_override_text_color(pill.center() - g.size() / 2.0, g, if p().black { DANGER } else { Color32::WHITE });
                                tx += pill.width() + 4.0;
                            } else if let Some((g, c)) = icon {
                                // Segoe Fluent icons are drawn for 16 px; smaller sizes blur
                                // 2 px under the text's box centre: measured on a capture, the icon's ink sits high in
                                // its box and the letters low in theirs
                                voice::glyph(ui.painter(), egui::pos2(x + 8.0, rect.center().y + 11.0), g, 16.0, c, false);
                                tx += 20.0;
                            }
                            // as in Discord: icon, a dot, then the text
                            let t = if icon.is_some() { format!("\u{00B7} {t}") } else { t };
                            let text = ellipsize(ui, &t.chars().take(120).collect::<String>(), &f, room_w - (tx - x));
                            ui.painter().text(egui::pos2(tx, rect.center().y + 9.0), egui::Align2::LEFT_CENTER, text, f, if unread { SECONDARY } else { MUTED });
                        }
                        // someone is typing in this room: three rising dots at the row's right
                        if s.typing.get(&r.id).is_some_and(|v| !v.is_empty()) {
                            let t = ui.input(|i| i.time) as f32;
                            for k in 0..3 {
                                let a = 0.35 + 0.65 * (((t * 3.0 - k as f32 * 0.7).sin() + 1.0) / 2.0);
                                th::dot(ui.painter(), egui::pos2(rect.right() - 50.0 + k as f32 * 8.0, rect.center().y), 2.5, LIVE.gamma_multiply(a));
                            }
                            ui.ctx().request_repaint_after(std::time::Duration::from_millis(60));
                        }
                        if r.mentions > 0 {
                            let c = egui::pos2(rect.right() - 16.0, rect.center().y);
                            th::dot(ui.painter(), c, 9.0, DANGER);
                            ui.painter().text(c, egui::Align2::CENTER_CENTER, r.mentions.to_string(), egui::FontId::proportional(11.0), Color32::WHITE);
                        } else if unread {
                            th::dot(ui.painter(), egui::pos2(rect.right() - 14.0, rect.center().y), 4.0, TEXT);
                        } else if r.direct && s.pinned.contains(&r.id) {
                            voice::glyph(ui.painter(), egui::pos2(rect.right() - 16.0, rect.center().y), "\u{E718}", 11.0, FAINT, false);
                        }
                        if r.incoming {
                            ui.painter().circle_filled(egui::pos2(rect.right() - 34.0, rect.center().y), 4.0, LIVE);
                        }
                        if resp.secondary_clicked() {
                            placemenu::open(&ctx, &r.id);
                        }
                        // the pointer is on its way to a click: its messages load now, not after it
                        if resp.hovered() && s.discord_online && !r.voice && !r.forum && !r.locked {
                            chat.warm(&r.id);
                        }
                        if resp.clicked() {
                            // a voice channel asks before joining (extras::voice_join); the one already joined just opens
                            if r.voice && !(s.voice.active() && s.voice.room.as_ref() == Some(&r.id)) {
                                let at = (resp.rect, ctx.input(|i| i.time));
                                ctx.data_mut(|d| {
                                    d.insert_temp(egui::Id::new(extras::JOIN_ASK), r.id.clone());
                                    d.insert_temp(egui::Id::new(extras::JOIN_ASK).with("at"), at);
                                });
                            } else if r.voice {
                                // the call over the channel's text chat, as a DM call shows
                                acts.push(Action::OpenChat(r.id.clone()));
                            } else {
                                acts.push(Action::Open(r.id.clone()));
                            }
                        }
                        // who is in a voice channel, known without joining (a DM call shows "In call" instead)
                        for pe in r.people.iter().filter(|_| !r.direct).take(12) {
                            let (pr, _) = ui.allocate_exact_size(vec2(w, 24.0), Sense::hover());
                            let av = egui::Rect::from_min_size(egui::pos2(pr.left() + 40.0, pr.center().y - 9.0), vec2(18.0, 18.0));
                            let t = texture(&ctx, tex, asked, s, &pe.avatar, &mut want);
                            avatar(ui, av, t, &pe.name);
                            // icons at the row's right, as Discord: streaming (screen), deafened (struck headphones),
                            // muted (struck microphone)
                            let mut ix = pr.right() - 16.0;
                            if pe.streaming {
                                // the red LIVE pill of the call tile, small
                                let g = ui.painter().layout_no_wrap("LIVE".to_owned(), egui::FontId::new(9.5, th::bold()), Color32::WHITE);
                                let pill = egui::Rect::from_min_size(egui::pos2(pr.right() - 8.0 - g.size().x - 10.0, pr.center().y - 8.0), vec2(g.size().x + 10.0, 16.0));
                                if !p().black {
                                    ui.painter().rect_filled(pill, 5.0, DANGER);
                                }
                                ui.painter().galley_with_override_text_color(pill.center() - g.size() / 2.0, g, if p().black { DANGER } else { Color32::WHITE });
                                ix = pill.left() - 12.0;
                                // a click shows what they stream, without joining
                                let hit = ui.interact(pill.expand(3.0), ui.id().with(("live_preview", &r.id, &pe.id)), Sense::click());
                                if hit.on_hover_text("View stream preview").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                    let mut parts = r.id.trim_start_matches("discord:").splitn(2, ':');
                                    if let (Some(g), Some(c)) = (parts.next(), parts.next()) {
                                        let now = ctx.input(|i| i.time);
                                        let key = mytune_chat::yapfast_core::yapfast_discord::stream::key(g, c, &pe.id);
                                        ctx.data_mut(|d| d.insert_temp(egui::Id::new(STREAM_PREVIEW), (format!("stream-preview:{key}#{now}"), pe.name.clone(), now)));
                                    }
                                }
                            }
                            for (show, g, c, slash) in [(pe.deafened, "\u{E7F6}", FAINT, true), (pe.muted && !pe.deafened, "\u{E720}", FAINT, true)] {
                                if show {
                                    voice::glyph(ui.painter(), egui::pos2(ix, pr.center().y), g, 13.0, c, slash);
                                    ix -= 18.0;
                                }
                            }
                            // the name last: it ends before the pill and icons instead of running under them
                            let font = egui::FontId::proportional(13.0);
                            let name = ellipsize(ui, &pe.name, &font, ix + 6.0 - (av.right() + 8.0));
                            ui.painter().text(egui::pos2(av.right() + 8.0, pr.center().y), egui::Align2::LEFT_CENTER, name, font, MUTED);
                        }
                    }
                    // a drag in progress: an accent line where the row lands (where the engine says it may go);
                    // letting go moves it
                    if let Some(d) = dragging.filter(|&d| d < shown.len()) {
                        let pointer = ctx.input(|i| i.pointer.interact_pos());
                        let onto = pointer.and_then(|p| spots.iter().position(|r| p.y >= r.top() && p.y < r.bottom()));
                        let landing = onto.filter(|&o| o != d).and_then(|o| app.landing(&shown, d, o));
                        if let (Some(o), Some(_)) = (onto, &landing) {
                            let r = spots[o];
                            let y = if o > d { r.bottom() } else { r.top() };
                            ui.painter().rect_filled(egui::Rect::from_min_max(egui::pos2(r.left(), y - 1.0), egui::pos2(r.right(), y + 1.0)), 1.0, p().accent);
                        }
                        ui.painter().rect_stroke(spots[d], 8.0, egui::Stroke::new(1.0, p().accent), egui::StrokeKind::Inside);
                        if !ctx.input(|i| i.pointer.primary_down()) {
                            if let Some(l) = landing {
                                acts.push(Action::MoveChannel { room: shown[d].clone(), landing: l });
                            }
                            ctx.data_mut(|dd| dd.remove::<usize>(drag_id));
                        }
                    }
                });
            });
            // messages + composer
            let room = s.selected_room().map(|r| (r.name.clone(), r.direct, r.read_only || r.locked));
            let in_forum = s.forum().is_some();
            let call = s.selected_room().map(|r| (r.id.clone(), s.can_join_voice(&r.id), s.voice.active() && s.voice.room.as_ref() == Some(&r.id)));
            voice::windows(&ctx, cu, s, acts);
            voice::incoming_card(&ctx, cu, s, tex, asked, &mut want, acts, false);
            // the conversation bar spans the chat and the panel docked beside it, as in Discord
            if s.dm_screen.is_none() || !matches!(s.navigation, Navigation::DirectMessages) {
            egui::Panel::top("chat_header").frame(egui::Frame::new().fill(k_chat()).inner_margin(egui::Margin::symmetric(16, 8)).stroke(egui::Stroke::new(1.0, voice::k_edge()))).show_separator_line(false).show_inside(ui, |ui| {
                match s.selected_room() {
                    Some(r) => header::bar(ui, hd, s, r, tex, asked, &mut want, acts),
                    None => {
                        ui.set_min_height(32.0);
                        ui.label(sb("Yapfast", 16.0).color(TEXT));
                    }
                }
            });
            }
            header::pins(&ctx, hd, s, tex, asked, &mut want, acts);
            threads::list(&ctx, hd, app, acts);
            threads::new_thread(&ctx, s, acts);
            header::add_people(&ctx, hd, s, tex, asked, &mut want, acts);
            header::profile_card(&ctx, s, tex, asked, &mut want, acts);
            // The engine's notice (a DM that could not start, a failed send) had no place while a room with
            // messages was open, so a refusal looked like nothing happened. A click dismisses it.
            if let Some(n) = s.notice.as_ref().filter(|_| !s.messages.is_empty() || s.selected.is_none()) {
                egui::Area::new(egui::Id::new("chat_notice")).order(egui::Order::Foreground).anchor(egui::Align2::CENTER_TOP, vec2(0.0, 56.0)).show(&ctx, |ui| {
                    th::float(ui, th::R_MENU, 12, |ui| {
                        ui.set_max_width(520.0);
                        ui.add(egui::Label::new(rt(n, 13.5).color(th::WARNING)).wrap());
                    });
                    if ui.interact(ui.min_rect(), ui.id().with("dismiss"), Sense::click()).on_hover_text("Dismiss").clicked() {
                        acts.push(Action::DismissNotice);
                    }
                });
            }
            voicenote::frame(s);
            menu::message_menu(&ctx, s, pk, acts);
            placemenu::show(&ctx, app, acts);
            callmenu::show(&ctx, s, acts);
            dialogs::invite(&ctx, s, acts);
            dialogs::leave(&ctx, s, acts);
            dialogs::group(&ctx, dlg, s, acts);
            dialogs::person(&ctx, dlg, s, acts);
            dialogs::channel(&ctx, dlg, s, acts);
            dialogs::join(&ctx, dlg, s, acts);
            dialogs::mfa(&ctx, s, acts);
            extras::route_links(&ctx, s, acts);
            if ctx.data_mut(|d| d.remove_temp::<bool>(egui::Id::new("open_status_editor"))).is_some() {
                extras::open_status(ex, s, acts);
            }
            extras::new_dm(&ctx, ex, s, tex, asked, &mut want, acts);
            extras::reactions(&ctx, s, tex, asked, &mut want, acts);
            extras::status_editor(&ctx, ex, s, tex, asked, &mut want, acts);
            extras::oauth(&ctx, s, tex, asked, &mut want, acts);
            extras::event(&ctx, s, acts);
            extras::gift(&ctx, s, acts);
            extras::voice_switch(&ctx, s, acts);
            extras::voice_join(&ctx, s, acts);
            extras::move_sync(&ctx, s, acts);
            dialogs::server_profile(&ctx, dlg, s, acts);
            menu::confirm_delete(&ctx, s, acts);
            media::viewer(&ctx, s, tex, acts);
            media::attachment_menu_show(&ctx, s, acts);
            header::pfp_viewer(&ctx, s, tex, asked, &mut want);
            switcher::show(&ctx, s, tex, asked, &mut want, acts);
            inbox::show(&ctx, s, acts);
            menu::forward(&ctx, fw, s, tex, asked, &mut want, acts);
            forum::new_post(&ctx, fo, app, tex, asked, &mut want, acts);
            if s.selected_room().is_some() {
                // Each kind of side panel has its own id (one per width, so each keeps its own size); the open one is
                // there at once, with no slide, as in Discord when another chat opens.
                let open = header::side_panel(hd, s);
                if let Some(w @ (header::Side::Search | header::Side::Aside)) = open {
                    hd.wide = w;
                }
                let chat_w = ui.available_width();
                for side in [hd.wide, header::Side::Split, header::Side::Profile, header::Side::Members] {
                    let pid = egui::Id::new(("chat_side", side.width() as i32));
                    let key = format!("side_{}", side.width() as i32);
                    if let Some(ps) = egui::PanelState::load(&ctx, ui.scope_id().with_salt(egui::IdSalt::new(pid))) {
                        widths.insert(key.clone(), ps.size().x);
                    }
                    if open != Some(side) {
                        continue;
                    }
                    // a chat side by side opens at half the chat area and may take all but 320 px of it
                    let split = side == header::Side::Split;
                    let (default, range) = if split { (chat_w / 2.0, 280.0..=(chat_w - 320.0).max(280.0)) } else { (side.width(), 200.0..=480.0) };
                    egui::Panel::right(pid).resizable(true).drag_to_open(false).default_size(saved.widths.get(&key).copied().unwrap_or(default)).size_range(range).frame(egui::Frame::new().fill(k_sidebar()).inner_margin(if matches!(side, header::Side::Profile | header::Side::Aside | header::Side::Split) { egui::Margin::ZERO } else { egui::Margin::same(12) })).show_separator_line(false).show_inside(ui, |ui| match side {
                        header::Side::Search => header::search_panel(ui, hd, s, tex, asked, &mut want, acts),
                        header::Side::Aside | header::Side::Split => forum::aside(ui, fo, look, s, tex, asked, &mut want, acts),
                        header::Side::Profile => header::profile_panel(ui, s, tex, asked, &mut want, acts),
                        header::Side::Members => header::members_panel(ui, s, s.selected.as_deref().filter(|r| app.owns_group(r)), tex, asked, &mut want, acts),
                    });
                }
            }
            egui::Panel::bottom("chat_composer").frame(egui::Frame::new().fill(k_chat()).inner_margin(egui::Margin { left: 20, right: 20, top: 6, bottom: 18 })).show_separator_line(false).show_inside(ui, |ui| {
                if s.detached && !in_forum {
                    egui::Frame::new().fill(k_field()).corner_radius(12.0).inner_margin(egui::Margin::symmetric(12, 6)).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(rt("You're viewing older messages", 13.0).color(SECONDARY));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if pill(ui, "Jump to present", true).clicked() {
                                    acts.push(Action::JumpToPresent);
                                    *scroll = Some(Scroll::Bottom);
                                }
                            });
                        });
                    });
                    ui.add_space(6.0);
                }
                match &room {
                    _ if in_forum => {}
                    Some((name, _, false)) => {
                        // editing a message: the composer shows its text once, as the engine set the draft
                        let now = (s.selected.clone(), match s.compose() {
                            Some(mytune_chat::yapfast_core::application::Compose::Edit { item, .. }) => Some(item.clone()),
                            _ => None,
                        });
                        if now != *editing {
                            *draft = s.draft().to_owned();
                            *editing = now;
                        }
                        // files waiting to go with the next message, each with a remove button
                        let staged = s.staged();
                        if !staged.is_empty() {
                            ui.horizontal_wrapped(|ui| {
                                for (i, f) in staged.iter().enumerate() {
                                    egui::Frame::new().fill(k_field()).corner_radius(12.0).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(rt(if f.image { "\u{EB9F}" } else { "\u{E8A5}" }, 14.0).family(th::icons()).color(SECONDARY));
                                            ui.label(rt(&f.name, 13.0).color(TEXT));
                                            ui.label(rt(format!("{:.1} MB", f.size as f64 / 1_048_576.0), 11.5).color(MUTED));
                                            if s.can_mark_spoilers() && ui.add(egui::Button::new(rt("\u{ED1A}", 12.0).family(th::icons()).color(if f.spoiler { p().accent } else { MUTED })).frame(false)).on_hover_text(if f.spoiler { "Marked as spoiler: click to show it openly" } else { "Mark as spoiler" }).clicked() {
                                                acts.push(Action::ToggleSpoiler(i));
                                            }
                                            if ui.add(egui::Button::new(rt("\u{E711}", 10.0).family(th::icons()).color(MUTED)).frame(false)).on_hover_text("Remove").clicked() {
                                                acts.push(Action::Unstage(i));
                                            }
                                        });
                                    });
                                }
                            });
                            ui.add_space(6.0);
                        }
                        compose::typing_line(ui, s);
                        // the reply/edit bar sits flush on the field
                        let gap = std::mem::replace(&mut ui.spacing_mut().item_spacing.y, 0.0);
                        if !voicenote::bar(ui, acts) {
                            menu::compose_bar(ui, s, acts);
                        }
                        let field =egui::Frame::new().fill(k_field()).corner_radius(12.0).inner_margin(egui::Margin { left: 8, right: 8, top: 6, bottom: 6 }).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                let icon = |ui: &mut Ui, g: &str, tip: &str, on: bool| {
                                    let (r, resp) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::click());
                                    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() || on, voice::QUICK);
                                    let font = if g == "GIF" { egui::FontId::new(11.0, th::bold()) } else { egui::FontId::new(18.0, th::icons()) };
                                    let color = SECONDARY.lerp_to_gamma(TEXT, t);
                                    if g == "sticker" {
                                        picker::sticker_icon(ui.painter(), r.center(), color);
                                    } else {
                                        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, g, font, color);
                                    }
                                    resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
                                };
                                if icon(ui, "\u{E710}", "Upload a file, create a poll", pu.open) {
                                    pu.toggle();
                                }
                                let right_icons = if cfg!(feature = "local-stickers") { 4.0 } else { 3.0 };
                                let w = ui.available_width() - right_icons * 40.0;
                                compose::files(&ctx, acts);
                                compose::before_edit(&ctx, s, draft, acts);
                                let out = egui::TextEdit::singleline(draft).id(compose::edit_id()).frame(egui::Frame::NONE).desired_width(w).hint_text(rt(format!("Message {name}"), 14.0).color(FAINT)).font(egui::FontId::proportional(14.0)).show(ui);
                                compose::after_edit(&ctx, &out, draft, acts);
                                let te = out.response;
                                if compose::take_focus(&ctx) {
                                    te.request_focus();
                                }
                                // Up in an empty composer edits this account's last message, as in Discord
                                if te.has_focus() && draft.is_empty() && ui.input(|i| i.key_pressed(egui::Key::ArrowUp)) {
                                    acts.push(Action::EditLast);
                                }
                                if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                                    // Enter on an empty field sends nothing and keeps the focus
                                    if !draft.trim().is_empty() || !staged.is_empty() {
                                        acts.push(Action::SetDraft(std::mem::take(draft)));
                                        acts.push(Action::Send);
                                    }
                                    te.request_focus();
                                }
                                let at = ui.input(|i| i.pointer.interact_pos());
                                let open = pk.open == Some(picker::For::Composer);
                                let rec = voicenote::recording();
                                if icon(ui, "\u{E720}", if rec { "Stop and throw the recording away" } else { "Record a voice message" }, rec) {
                                    if let Some(room) = &s.selected {
                                        voicenote::toggle(room);
                                    }
                                }
                                if icon(ui, "GIF", "GIFs", open && pk.gifs()) {
                                    pk.toggle(picker::For::Composer, true, at, acts);
                                }
                                if cfg!(feature = "local-stickers") && icon(ui, "sticker", "Stickers", open && pk.stickers()) {
                                    pk.toggle_stickers(at, acts);
                                }
                                if icon(ui, "\u{E76E}", "Emoji", open && !pk.gifs() && !pk.stickers()) {
                                    pk.toggle(picker::For::Composer, false, at, acts);
                                }
                            });
                        });
                        ui.spacing_mut().item_spacing.y = gap;
                        *composer_rect = Some(field.response.rect);
                    }
                    Some((_, _, true)) if s.selected_room().is_some_and(|r| r.request) => {
                        // a message request: replying waits until it is accepted
                        ui.horizontal(|ui| {
                            ui.label(rt("This is a message request.", 13.0).color(MUTED));
                            ui.add_space(8.0);
                            if crate::theme::button(ui, "Accept", true, (14.0, 5.0, 14.0, 6.0)).clicked() {
                                acts.push(Action::AnswerRequest(true));
                            }
                            if crate::theme::button(ui, "Ignore", false, (14.0, 5.0, 14.0, 6.0)).clicked() {
                                acts.push(Action::AnswerRequest(false));
                            }
                        });
                    }
                    Some((_, _, true)) => {
                        ui.label(rt("You cannot write here.", 13.0).color(MUTED));
                    }
                    None => {}
                }
            });
            picker::show(&ctx, pk, app, draft, composer_rect.unwrap_or(egui::Rect::NOTHING), tex, asked, &mut want, acts);
            compose::popup(&ctx, s, composer_rect.unwrap_or(egui::Rect::NOTHING), draft, tex, asked, &mut want, acts);
            poll::menu(&ctx, pu, app, chat, composer_rect.unwrap_or(egui::Rect::NOTHING), acts);
            poll::dialog(&ctx, pu, s, acts);
            // a room opened, or its messages came, or a jump: bring the right message into view once
            if let Some(r) = s.selected.clone() {
                let key = (r, s.unread_jump, s.jump.as_ref().map_or(0, |j| j.1), !s.messages.is_empty(), s.detached);
                if scrolled.as_ref().map(|k| &k.0) != Some(&key.0) {
                    (*extra, *top) = (0, None); // another room: back to its newest 150
                }
                if scrolled.as_ref() != Some(&key) {
                    // leaving a window of older messages (Jump to present, or the newest reached) goes to the bottom
                    let to_present = scrolled.as_ref().is_some_and(|k| k.0 == key.0 && k.4 && !key.4);
                    *scroll = Some(match (&s.jump, &s.first_unread) {
                        _ if to_present => Scroll::Bottom,
                        (Some((item, _)), _) => Scroll::To(item.clone()),
                        (None, Some(item)) => Scroll::To(item.clone()),
                        _ => Scroll::Bottom,
                    });
                    *scrolled = Some(key);
                }
            }
            egui::CentralPanel::default().frame(egui::Frame::new().fill(k_chat()).inner_margin(egui::Margin { left: 12, right: 12, top: 0, bottom: 0 })).show_inside(ui, |ui| {
                // the call stage (or a DM's call invite) takes the top of the message area, the chat stays below
                let here = call.as_ref().is_some_and(|c| c.2);
                let invite = s.selected_room().filter(|r| !here && r.direct && (r.incoming || !r.people.is_empty()));
                if here || invite.is_some() {
                    let full = ui.max_rect();
                    let h = if here { (full.height() * 0.64).max(260.0_f32.min(full.height() * 0.8)) } else { 230.0 };
                    let (slot, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
                    // an inset rounded card, not a full-bleed band
                    let rect = slot.shrink2(vec2(0.0, 8.0));
                    match invite {
                        Some(r) => voice::invite(ui, rect, s, r, tex, asked, &mut want, acts, cu),
                        None => voice::stage(ui, rect, cu, s, tex, asked, &mut want, acts),
                    }
                    // the card's inner scope moved the cursor up to its content: put it back below the slot
                    ui.advance_cursor_after_rect(slot);
                }
                if s.dm_screen.is_some() && matches!(s.navigation, Navigation::DirectMessages) {
                    friends::screen(ui, s, tex, asked, &mut want, acts);
                    return;
                }
                if s.browse.as_ref().is_some_and(|b| s.navigation == Navigation::Server(b.server.clone())) {
                    extras::browse(ui, s, acts);
                    return;
                }
                if in_forum {
                    forum::view(ui, fo, app, tex, asked, &mut want, acts);
                    return;
                }
                if room.is_none() {
                    ui.add_space(60.0);
                    ui.vertical_centered(|ui| ui.label(rt("Pick a conversation to start.", 15.0).color(MUTED)));
                    return;
                }
                // never a blank room: why it is empty (Discord's refusal), still loading, or truly empty
                if s.messages.is_empty() {
                    if s.notice.is_none() && !s.room_ready {
                        skeleton(ui);
                        return;
                    }
                    let (text, color) = match &s.notice {
                        Some(n) => (format!("{n}\nDiscord lets you see this channel but not read its messages."), th::WARNING),
                        None => ("No messages to show.".to_owned(), MUTED),
                    };
                    ui.add_space(60.0);
                    ui.vertical_centered(|ui| ui.label(rt(text, 14.0).color(color)));
                    return;
                }
                let mut seen = Vec::new();
                // opening a room or jumping lands at once, with no fast scroll across the history
                ui.style_mut().scroll_animation = egui::style::ScrollAnimation::none();
                let n = s.messages.len();
                // a message to bring into view that is older than those drawn: draw back to it
                if let Some(Scroll::To(item)) = scroll {
                    if let Some(at) = s.messages.iter().position(|m| m.item == *item) {
                        *extra = (*extra).max((n - at).saturating_sub(DRAWN));
                    }
                }
                let depth = DRAWN + *extra;
                let shift_id = egui::Id::new("timeline_shift");
                let mut area = egui::ScrollArea::vertical().auto_shrink(false).stick_to_bottom(true);
                if let Some(y) = ui.data_mut(|d| d.remove_temp::<f32>(shift_id)) {
                    area = area.vertical_scroll_offset(y);
                }
                let out = area.show(ui, |ui| {
                    message_list(ui, s, &s.messages, tex, asked, &mut want, acts, look, s.first_unread.as_deref(), scroll, &mut seen, depth);
                });
                at_end = out.state.offset().y + out.inner_rect.height() >= out.content_size.y - 24.0;
                // Older messages: at the top of the timeline more of the loaded ones are drawn, then the engine
                // loads the page before them, as Yapfast's timeline does.
                let first = s.messages.iter().skip(n.saturating_sub(depth)).find(|m| !m.deleted).map(|m| m.item.clone());
                if let (Some(first), None) = (first, &*scroll) {
                    match top.as_ref() {
                        Some((was, h)) if *was != first => {
                            // they came in above: stay on the message that was in view
                            let y = out.state.offset().y + (out.content_size.y - h).max(0.0);
                            ui.data_mut(|d| d.insert_temp(shift_id, y));
                            ui.ctx().request_repaint();
                        }
                        _ if out.state.offset().y < 300.0 => {
                            if n > depth {
                                *extra += DRAWN;
                            } else if s.room_ready && !s.history_start && !s.loading_history && !s.history_failed {
                                *extra += DRAWN;
                                acts.push(Action::Earlier);
                            }
                        }
                        _ => {}
                    }
                    *top = Some((first, out.content_size.y));
                } else {
                    *top = None; // being scrolled into place: nothing to hold on to yet
                }
                if let Some(r) = s.selected.clone() {
                    // only when it changed: every dispatch asks for a repaint, and sent each frame the page never rested
                    let now = (r, seen, at_end);
                    let time = ctx.input(|i| i.time);
                    if vis.as_ref() != Some(&now) {
                        acts.push(Action::VisibleMessages { room: now.0.clone(), items: now.1.clone(), at_end });
                        *vis = Some(now);
                        *vis_retry = (time + 2.5, 0);
                        ctx.request_repaint_after(std::time::Duration::from_millis(2600));
                    } else if time >= vis_retry.0 && vis_retry.1 < 2 {
                        // The engine loads pictures only for the view it was told, and that message can be lost
                        // while a room opens (its channel is not open yet): pictures then stayed blank until a
                        // scroll changed the view. Still missing after a while: tell it again. It ignores the
                        // same view twice, so an empty one goes first. Twice at most: a slow download is not a loss.
                        let missing = s.messages.iter().rev().take(depth).filter(|m| now.1.contains(&m.item)).any(|m| m.attachments.iter().any(|a| (a.image || a.video) && a.preview.is_none() && !a.preview_failed));
                        *vis_retry = (time + 4.0, vis_retry.1 + 1);
                        if missing {
                            acts.push(Action::VisibleMessages { room: now.0.clone(), items: vec![], at_end: false });
                            acts.push(Action::VisibleMessages { room: now.0.clone(), items: now.1.clone(), at_end });
                            ctx.request_repaint_after(std::time::Duration::from_millis(4100));
                        }
                    }
                }
                // reached the bottom, or the user scrolls: stop pulling the view down
                if matches!(scroll, Some(Scroll::Bottom)) && (at_end || ui.input(|i| i.smooth_scroll_delta.y != 0.0)) {
                    *scroll = None;
                }
            });
        });
        for f in later {
            f(chat);
        }
        // a message while MyTune is behind other windows flashes its taskbar button, as Yapfast does
        if chat.take_attention() && !ctx.input(|i| i.focused) {
            ctx.send_viewport_cmd(egui::ViewportCommand::RequestUserAttention(egui::UserAttentionType::Informational));
        }
        self.call.end_frame(&ctx);
        // The newest message is read while it is in view with the window in front: the engine then marks the
        // room read (here and on Discord) and its unread dot goes.
        // Opened on a window of older messages (unread ones, around the last read): scrolled to its end, the
        // next page loads, as Yapfast's timeline does, until it reaches the newest and the room can be read.
        let (detached, count) = chat.with_state(|s| (s.detached, s.messages.len()));
        if detached && at_end && self.later_at != Some(count) {
            self.later_at = Some(count);
            acts.push(Action::Later);
        }
        let room = chat.with_state(|s| s.selected.clone());
        let focused = ctx.input(|i| i.focused);
        if let Some(room) = room {
            let now = at_end && focused;
            if self.reading.as_ref() != Some(&(room.clone(), now)) {
                self.reading = Some((room, now));
                acts.push(Action::Reading(now));
            }
        }
        chat.with_state(|s| stream_preview(&ctx, s, &mut self.tex, &mut self.asked, &mut want));
        if !want.is_empty() {
            acts.push(Action::LoadPictures(want));
        }
        if relogin {
            chat.log_in_again();
        }
        if reconnect {
            chat.reconnect();
        }
        // save the layout when it changed, once no drag is going on (a drag ends with the final width); not on the
        // first frame, where the restored profile panel is only on its way to the engine
        let now = mytune_chat::ChatLayout {
            members_open: self.header.members_open,
            profile_panel: Some(chat.with_state(|st| st.profile_panel)),
            lists_hidden: ctx.data(|d| d.get_temp(egui::Id::new("chat_focus"))).unwrap_or(false),
            widths: widths.into_iter().map(|(k, v)| (k, v.round())).collect(),
            server_order: order,
        };
        if !first && now != saved && !ctx.input(|i| i.pointer.any_down()) {
            chat.set_layout(now.clone());
            self.layout = Some(now);
        }
        // a person's volume or mutes changed in the call menu: kept between runs once the slider is let go
        if !ctx.input(|i| i.pointer.any_down()) {
            if let Some(person) = ctx.data_mut(|d| d.remove_temp::<String>(egui::Id::new(callmenu::SOUND_CHANGED))) {
                chat.keep_person_sound(&person);
            }
        }
    }
}

// The chat's surfaces come from the MyTune palette (accent hue, same brightness steps as the layers and cards
// of the other pages), so the chat matches MyTune; the header strip and divider keep it a space of its own.
// Glass chat (2026-11-15): the layers are dark veils and light films over the backdrop, not opaque tones.
fn k_rail() -> Color32 { Color32::from_black_alpha(0x70) }
fn k_sidebar() -> Color32 { Color32::from_black_alpha(0x50) }
fn k_chat() -> Color32 { Color32::from_black_alpha(0x30) }
/// The call window sits a shade darker than the chat under it, as in Discord.
fn k_stage() -> Color32 { Color32::from_black_alpha(0x50) }
fn k_field() -> Color32 { Color32::from_white_alpha(0x14) }
fn k_hover() -> Color32 { Color32::from_white_alpha(0x0E) }
fn k_row_hover() -> Color32 { Color32::from_white_alpha(0x10) }
fn k_active() -> Color32 { Color32::from_white_alpha(0x1C) }
fn k_line() -> Color32 { Color32::from_white_alpha(0x1E) }

/// A channel name as drawn: the vertical dashes server owners put after an emoji (U+FE31, U+FE32, U+2502, U+23D0)
/// are not in the fonts and show as boxes, so they become a plain bar.
fn tidy(name: &str) -> String {
    name.chars().map(|c| if matches!(c as u32, 0x2500..=0x257F | 0xFE30..=0xFE4F | 0xFF5C | 0x2758..=0x275A | 0x23D0 | 0x2016 | 0x01C0) { '|' } else if c as u32 == 0x30FB || c as u32 == 0xFF65 { '\u{B7}' } else { c }).filter(|c| !matches!(*c as u32, 0x200B..=0x200F | 0x2060 | 0xFEFF)).collect()
}

#[cfg(test)]
mod rail_tests {
    #[test]
    fn dragged_order_wins_and_new_servers_come_first() {
        let order = vec!["c".to_owned(), "a".to_owned()];
        assert_eq!(super::rail_order(&["a", "new", "c", "new2"], &order), vec![1, 3, 2, 0]);
        assert_eq!(super::rail_order(&["a", "b"], &[]), vec![0, 1]);
    }
}
