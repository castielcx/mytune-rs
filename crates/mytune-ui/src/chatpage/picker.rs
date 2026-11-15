//! The emoji and GIF picker, after Discord's: emoji from the servers this account may use (the engine's
//! `usable_emoji`, so Nitro rules hold) above the standard ones, a search, and a GIFs tab driven by the engine's
//! GIF picker. In the composer an emoji goes into the text (`:name:` for a server's, which the engine sends as
//! Discord's `<:name:id>`) and a GIF is sent; for a reaction the emoji reacts to the message.

use super::voice::{k_edge, k_panel, QUICK, k_raised};
use super::*;
use egui::{pos2, Rect, Stroke};
use mytune_chat::yapfast_core::application::{Application, GifPicture, GifView};

/// What the picker is for.
#[derive(Clone, PartialEq)]
pub enum For {
    Composer,
    /// Reacting to this message (by item).
    Reaction(String),
}

#[derive(Default)]
pub struct PickerUi {
    pub open: Option<For>,
    gifs: bool,
    /// The Stickers tab shows (feature `local-stickers` only).
    stickers: bool,
    query: String,
    gif_query: String,
    at: Option<egui::Pos2>,
    /// Opened this frame: the click that opened it is not a click elsewhere.
    fresh: bool,
    gif_tex: HashMap<String, TextureHandle>,
}

impl PickerUi {
    pub fn toggle(&mut self, what: For, gifs: bool, at: Option<egui::Pos2>, acts: &mut Vec<Action>) {
        if self.open.as_ref() == Some(&what) && self.gifs == gifs {
            self.close(acts);
            return;
        }
        if self.gifs && !gifs {
            acts.push(Action::CloseGifs);
        }
        if gifs && !self.gifs {
            acts.push(Action::OpenGifs);
        }
        self.open = Some(what);
        self.gifs = gifs;
        self.at = at;
        self.fresh = true;
        self.query.clear();
    }

    /// The GIFs tab shows.
    pub fn gifs(&self) -> bool {
        self.gifs
    }

    pub fn close(&mut self, acts: &mut Vec<Action>) {
        if self.gifs {
            acts.push(Action::CloseGifs);
        }
        self.open = None;
        self.gifs = false;
        self.stickers = false;
        self.gif_tex.clear();
    }
}

/// Common emoji with search words, as the standard part of the picker (the engine ships no emoji list).
const EMOJI: &[(&str, &str)] = &[
    ("😀", "grinning smile"), ("😃", "smiley"), ("😄", "smile"), ("😁", "grin"), ("😆", "laughing"), ("😅", "sweat smile"), ("🤣", "rofl"), ("😂", "joy laugh"),
    ("🙂", "slight smile"), ("🙃", "upside down"), ("😉", "wink"), ("😊", "blush"), ("😇", "innocent"), ("🥰", "love hearts"), ("😍", "heart eyes"), ("🤩", "star struck"),
    ("😘", "kiss"), ("😋", "yum"), ("😛", "tongue"), ("😜", "wink tongue"), ("🤪", "zany"), ("🤑", "money"), ("🤗", "hug"), ("🤭", "hand over mouth"),
    ("🤫", "shush"), ("🤔", "thinking"), ("🤐", "zipper"), ("🤨", "raised eyebrow"), ("😐", "neutral"), ("😑", "expressionless"), ("😶", "no mouth"), ("😏", "smirk"),
    ("😒", "unamused"), ("🙄", "eye roll"), ("😬", "grimace"), ("😌", "relieved"), ("😔", "pensive"), ("😪", "sleepy"), ("😴", "sleeping"), ("😷", "mask"),
    ("🤒", "sick"), ("🤢", "nauseated"), ("🤮", "vomit"), ("🥵", "hot"), ("🥶", "cold"), ("🥴", "woozy"), ("😵", "dizzy"), ("🤯", "mind blown"),
    ("🤠", "cowboy"), ("🥳", "party"), ("😎", "cool sunglasses"), ("🤓", "nerd"), ("🧐", "monocle"), ("😕", "confused"), ("😟", "worried"), ("🙁", "frown"),
    ("😮", "open mouth"), ("😲", "astonished"), ("😳", "flushed"), ("🥺", "pleading"), ("😦", "frowning"), ("😨", "fearful"), ("😰", "anxious"), ("😢", "cry"),
    ("😭", "sob"), ("😱", "scream"), ("😖", "confounded"), ("😣", "persevere"), ("😞", "disappointed"), ("😓", "sweat"), ("😩", "weary"), ("😫", "tired"),
    ("🥱", "yawn"), ("😤", "triumph"), ("😡", "rage"), ("😠", "angry"), ("🤬", "cursing"), ("😈", "devil smile"), ("👿", "imp"), ("💀", "skull dead"),
    ("☠️", "skull crossbones"), ("💩", "poop"), ("🤡", "clown"), ("👻", "ghost"), ("👽", "alien"), ("🤖", "robot"), ("😺", "cat smile"), ("🙈", "see no evil"),
    ("👋", "wave"), ("🤚", "raised back hand"), ("✋", "hand"), ("👌", "ok"), ("🤌", "pinched"), ("✌️", "peace"), ("🤞", "crossed fingers"), ("🤟", "love you"),
    ("🤘", "rock"), ("🤙", "call me"), ("👈", "point left"), ("👉", "point right"), ("👆", "point up"), ("👇", "point down"), ("👍", "thumbs up like"), ("👎", "thumbs down"),
    ("✊", "fist"), ("👊", "punch"), ("👏", "clap"), ("🙌", "raised hands"), ("🫶", "heart hands"), ("🤝", "handshake"), ("🙏", "pray please thanks"), ("💪", "muscle strong"),
    ("👀", "eyes"), ("🧠", "brain"), ("❤️", "heart red love"), ("🧡", "orange heart"), ("💛", "yellow heart"), ("💚", "green heart"), ("💙", "blue heart"), ("💜", "purple heart"),
    ("🖤", "black heart"), ("🤍", "white heart"), ("💔", "broken heart"), ("💕", "two hearts"), ("💯", "100 hundred"), ("💢", "anger"), ("💥", "boom"), ("💫", "dizzy star"),
    ("💦", "sweat drops"), ("💤", "zzz"), ("🔥", "fire lit"), ("✨", "sparkles"), ("⭐", "star"), ("🌟", "glowing star"), ("⚡", "zap"), ("🎉", "tada party"),
    ("🎊", "confetti"), ("🎁", "gift"), ("🏆", "trophy"), ("🥇", "gold medal first"), ("🎮", "video game controller"), ("🕹️", "joystick"), ("🎧", "headphones"), ("🎵", "music note"),
    ("👑", "crown"), ("💎", "gem diamond"), ("💰", "money bag"), ("💸", "money wings"), ("📈", "chart up"), ("📉", "chart down"), ("✅", "check yes"), ("❌", "x no cross"),
    ("⚠️", "warning"), ("❓", "question"), ("❗", "exclamation"), ("🚀", "rocket"), ("🍕", "pizza"), ("🍔", "burger"), ("🍺", "beer"), ("☕", "coffee"),
    ("🐐", "goat"), ("🐶", "dog"), ("🐱", "cat"), ("🐸", "frog"), ("🦀", "crab rust"), ("🌈", "rainbow"), ("☀️", "sun"), ("🌙", "moon"),
];

/// The picker, as a floating panel: above the composer's right edge, or by the pointer for a reaction.
#[allow(clippy::too_many_arguments)]
pub fn show(ctx: &egui::Context, pk: &mut PickerUi, app: &Application, draft: &mut String, composer: Rect, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let Some(what) = pk.open.clone() else { return };
    let s = &app.state;
    let (w, h) = (440.0, 460.0);
    let screen = ctx.content_rect();
    let pos = match (&what, pk.at) {
        (For::Reaction(_), Some(at)) => pos2((at.x - w / 2.0).clamp(screen.left() + 8.0, screen.right() - w - 8.0), (at.y - h - 8.0).max(screen.top() + 8.0)),
        _ => pos2(composer.right() - w, composer.top() - h - 8.0),
    };
    let mut close = false;
    let area = egui::Area::new(egui::Id::new("emoji_picker")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        crate::theme::float(ui, crate::theme::R_MENU, 10, |ui| {
                ui.set_width(w - 20.0);
                ui.set_height(h - 20.0);
                // tabs: GIFs only where one can be sent
                ui.horizontal(|ui| {
                    if what == For::Composer {
                        let tabs: &[&str] = if cfg!(feature = "local-stickers") { &["GIFs", "Emoji", "Stickers"] } else { &["GIFs", "Emoji"] };
                        let now = if pk.gifs { 0 } else if pk.stickers { 2 } else { 1 };
                        for (tab, name) in tabs.iter().enumerate() {
                            let on = now == tab;
                            if ui.add(crate::theme::Choice::new(*name, on).min_size(vec2(70.0, 28.0))).clicked() && !on {
                                let at = pk.at;
                                if pk.gifs != (tab == 0) {
                                    pk.toggle(For::Composer, tab == 0, at, acts);
                                }
                                pk.stickers = tab == 2;
                                pk.query.clear();
                            }
                        }
                    } else {
                        ui.label(sb("Add Reaction", 14.0).color(TEXT));
                    }
                });
                ui.add_space(8.0);
                #[cfg(feature = "local-stickers")]
                let stickers_tab = pk.stickers && stickers(ui, pk, app, tex, asked, want, acts, &mut close);
                #[cfg(not(feature = "local-stickers"))]
                let stickers_tab = false;
                if stickers_tab {
                } else if pk.gifs {
                    gifs(ui, pk, s, acts, &mut close);
                } else {
                    emoji(ui, pk, app, &what, draft, tex, asked, want, acts, &mut close);
                }
            });
    });
    let fresh = std::mem::take(&mut pk.fresh);
    // A wheel turn outside the picker scrolls the chat under it: the picker would float over other messages.
    let scrolled_away = ctx.input(|i| {
        i.raw.events.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. })) && !i.pointer.latest_pos().is_some_and(|p| area.response.rect.contains(p))
    });
    if close || scrolled_away || (!fresh && area.response.clicked_elsewhere()) || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        pk.close(acts);
    }
}

#[allow(clippy::too_many_arguments)]
fn emoji(ui: &mut Ui, pk: &mut PickerUi, app: &Application, what: &For, draft: &mut String, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>, close: &mut bool) {
    let s = &app.state;
    let ctx = ui.ctx().clone();
    egui::Frame::new().fill(k_rail()).corner_radius(10.0).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.add(egui::TextEdit::singleline(&mut pk.query).frame(egui::Frame::NONE).desired_width(f32::INFINITY).hint_text("Find the perfect emoji"));
    });
    ui.add_space(6.0);
    let q = pk.query.trim().to_lowercase();
    // a pick: what the composer gets, and the reaction key
    let mut picked: Option<(String, String)> = None;
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        let tile = |ui: &mut Ui| {
            let (r, resp) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::click());
            let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), QUICK);
            ui.painter().rect_filled(r, 10.0, k_hover().gamma_multiply(t));
            (r, resp)
        };
        // shortcut: usable_emoji() is built while the picker shows (a few hundred entries); cache it if slow.
        for set in app.usable_emoji() {
            let list: Vec<_> = set.emoji.iter().filter(|e| q.is_empty() || e.text.to_lowercase().contains(&q)).collect();
            if list.is_empty() {
                continue;
            }
            ui.label(rt(set.server.to_uppercase(), 11.0).color(MUTED));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
                for e in list {
                    let (r, resp) = tile(ui);
                    match texture(&ctx, tex, asked, s, &Some(e.picture.clone()), want) {
                        Some(t) => {
                            egui::Image::new(egui::load::SizedTexture::new(t, vec2(30.0, 30.0))).paint_at(ui, Rect::from_center_size(r.center(), vec2(30.0, 30.0)));
                        }
                        None => {
                            ui.painter().rect_filled(Rect::from_center_size(r.center(), vec2(26.0, 26.0)), 10.0, k_raised());
                        }
                    }
                    if resp.on_hover_text(&e.text).clicked() {
                        let name = e.text.trim_matches(':');
                        picked = Some((e.text.clone(), format!("{name}:{}", e.id)));
                    }
                }
            });
            ui.add_space(6.0);
        }
        ui.label(rt("STANDARD", 11.0).color(MUTED));
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(2.0, 2.0);
            for (e, words) in EMOJI.iter().filter(|(_, words)| q.is_empty() || words.contains(&q)) {
                let (r, resp) = tile(ui);
                match crate::emoji::texture(&ctx, e) {
                    Some(t) => egui::Image::new(egui::load::SizedTexture::new(t, vec2(30.0, 30.0))).paint_at(ui, Rect::from_center_size(r.center(), vec2(30.0, 30.0))),
                    None => {
                        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, *e, egui::FontId::proportional(24.0), TEXT);
                    }
                }
                if resp.on_hover_text(*words).clicked() {
                    picked = Some(((*e).to_owned(), (*e).to_owned()));
                }
            }
        });
    });
    if let Some((text, key)) = picked {
        match what {
            For::Composer => {
                if !draft.is_empty() && !draft.ends_with(' ') {
                    draft.push(' ');
                }
                draft.push_str(&text);
                draft.push(' ');
                acts.push(Action::SetDraft(draft.clone())); // the engine keeps the draft per room
            }
            For::Reaction(item) => {
                acts.push(Action::React { item: item.clone(), key });
                *close = true;
            }
        }
    }
}

/// The Stickers tab: every server's stickers, whatever Nitro allows. A click posts the sticker's picture link as
/// its own message, which Discord shows as the picture (an animated APNG sticker as a still). Returns true: it drew.
#[cfg(feature = "local-stickers")]
#[allow(clippy::too_many_arguments)]
fn stickers(ui: &mut Ui, pk: &mut PickerUi, app: &Application, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>, close: &mut bool) -> bool {
    let s = &app.state;
    let ctx = ui.ctx().clone();
    egui::Frame::new().fill(k_rail()).corner_radius(10.0).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
        ui.add(egui::TextEdit::singleline(&mut pk.query).frame(egui::Frame::NONE).desired_width(f32::INFINITY).hint_text("Find the perfect sticker"));
    });
    ui.add_space(6.0);
    let q = pk.query.trim().to_lowercase();
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        for (server, list) in app.all_stickers() {
            let list: Vec<_> = list.iter().filter(|x| q.is_empty() || x[0].to_lowercase().contains(&q)).collect();
            if list.is_empty() {
                continue;
            }
            ui.label(rt(server.to_uppercase(), 11.0).color(MUTED));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
                for [name, picture, link] in list {
                    let (r, resp) = ui.allocate_exact_size(vec2(96.0, 96.0), Sense::click());
                    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), QUICK);
                    ui.painter().rect_filled(r, 10.0, k_hover().gamma_multiply(t));
                    let inner = Rect::from_center_size(r.center(), vec2(84.0, 84.0));
                    // pictures are asked for only once their tile is in view: a server can have 60 stickers
                    match ui.is_rect_visible(r).then(|| texture(&ctx, tex, asked, s, &Some(picture.clone()), want)).flatten() {
                        Some(t) => egui::Image::new(egui::load::SizedTexture::new(t, inner.size())).paint_at(ui, inner),
                        None => {
                            ui.painter().rect_filled(inner, 10.0, k_raised());
                        }
                    }
                    if resp.on_hover_text(name).clicked() {
                        acts.push(Action::SendText(link.clone()));
                        *close = true;
                    }
                }
            });
            ui.add_space(6.0);
        }
    });
    true
}

fn gifs(ui: &mut Ui, pk: &mut PickerUi, s: &AppState, acts: &mut Vec<Action>, close: &mut bool) {
    let ctx = ui.ctx().clone();
    let g = &s.gifs;
    ui.horizontal(|ui| {
        if g.view != GifView::Home && ui.add(egui::Button::new(rt("\u{E72B}", 12.0).family(th::icons())).frame(false)).on_hover_text("Back").clicked() {
            pk.gif_query.clear();
            acts.push(Action::ShowGifs(GifView::Home));
        }
        egui::Frame::new().fill(k_rail()).corner_radius(10.0).inner_margin(egui::Margin::symmetric(8, 6)).show(ui, |ui| {
            let te = ui.add(egui::TextEdit::singleline(&mut pk.gif_query).frame(egui::Frame::NONE).desired_width(f32::INFINITY).hint_text("Search Tenor"));
            if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !pk.gif_query.trim().is_empty() {
                acts.push(Action::SearchGifs(pk.gif_query.trim().to_owned()));
            }
        });
    });
    ui.add_space(6.0);
    if let Some(e) = g.error.as_ref().or(g.home_error.as_ref()) {
        ui.label(rt(e, 12.5).color(DANGER));
    }
    let col = (ui.available_width() - 6.0) / 2.0;
    let mut shown: Vec<(String, u32, u32)> = Vec::new();
    let ppp = ctx.pixels_per_point();
    let out = egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        let mut tile = |ui: &mut Ui, src: &str, label: Option<&str>, ratio: f32| -> bool {
            let h = (col / ratio.max(0.3)).clamp(60.0, 240.0);
            let (r, resp) = ui.allocate_exact_size(vec2(col, h), Sense::click());
            shown.push((src.to_owned(), (col * ppp) as u32, (h * ppp) as u32));
            let pic = match g.pictures.get(src).map(|a| &a.picture) {
                Some(GifPicture::Shown(p)) => Some(p.clone()),
                _ => None,
            };
            match pic {
                Some(p) => {
                    let t = pk.gif_tex.entry(src.to_owned()).or_insert_with(|| ctx.load_texture(format!("gif:{src}"), egui::ColorImage::from_rgba_unmultiplied([p.width as usize, p.height as usize], &p.rgba), crate::theme::SMOOTH));
                    // a GIF plays only under the pointer
                    let id = if resp.hovered() { super::media::animated(&ctx,&format!("gif:{src}"), &p).unwrap_or(t.id()) } else { t.id() };
                    egui::Image::new(egui::load::SizedTexture::new(id, r.size())).corner_radius(10.0).paint_at(ui, r);
                }
                None => {
                    ui.painter().rect_filled(r, 10.0, k_raised());
                }
            }
            if let Some(l) = label {
                ui.painter().rect_filled(r, 10.0, Color32::from_black_alpha(110));
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, l, egui::FontId::new(14.0, th::semibold()), Color32::WHITE);
            }
            let t = ctx.animate_bool_with_time(resp.id, resp.hovered(), QUICK);
            if t > 0.0 {
                ui.painter().rect_stroke(r, 6.0, Stroke::new(2.0, p().accent.gamma_multiply(t)), egui::StrokeKind::Inside);
            }
            resp.clicked()
        };
        let mut i = 0;
        let mut row = |ui: &mut Ui, cells: &mut dyn FnMut(&mut Ui)| {
            let _ = i;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                cells(ui);
            });
            ui.add_space(6.0);
            i += 1;
        };
        if g.view == GifView::Home {
            if g.home_loading {
                ui.label(rt("Loading\u{2026}", 13.0).color(MUTED));
            }
            let mut pending: Option<String> = None;
            let mut trending = false;
            let cats: Vec<_> = g.categories.iter().collect();
            if let Some(t) = &g.trending_preview {
                row(ui, &mut |ui| {
                    if tile(ui, &t.src, Some("Trending GIFs"), 1.6) {
                        trending = true;
                    }
                    if g.view == GifView::Home && tile(ui, &t.src, Some("Favorites"), 1.6) {
                        pending = Some(String::new());
                    }
                });
            }
            for pair in cats.chunks(2) {
                row(ui, &mut |ui| {
                    for c in pair {
                        if tile(ui, &c.src, Some(&c.name), 1.6) {
                            pending = Some(c.name.clone());
                        }
                    }
                });
            }
            if trending {
                acts.push(Action::ShowGifs(GifView::Trending));
            }
            match pending {
                Some(n) if n.is_empty() => acts.push(Action::ShowGifs(GifView::Favorites)),
                Some(n) => {
                    pk.gif_query = n.clone();
                    acts.push(Action::SearchGifCategory(n));
                }
                None => {}
            }
        } else {
            if g.loading && g.results.is_empty() {
                ui.label(rt("Loading\u{2026}", 13.0).color(MUTED));
            }
            let mut pick: Option<String> = None;
            for pair in g.results.chunks(2) {
                row(ui, &mut |ui| {
                    for gif in pair {
                        if tile(ui, &gif.src, None, gif.width as f32 / gif.height.max(1) as f32) {
                            pick = Some(gif.url.clone());
                        }
                    }
                });
            }
            if let Some(url) = pick {
                acts.push(Action::PickGif { url, insert: false });
                *close = true;
            }
        }
    });
    let near_end = out.state.offset().y + out.inner_rect.height() >= out.content_size.y - 300.0;
    // the engine loads the pictures shown, and the next page near the end (only when what shows changed)
    let key = egui::Id::new("gifs_shown");
    let sig = (shown.len(), near_end, shown.first().map(|x| x.0.clone()));
    if ctx.data(|d| d.get_temp::<(usize, bool, Option<String>)>(key)) != Some(sig.clone()) {
        ctx.data_mut(|d| d.insert_temp(key, sig));
        acts.push(Action::GifsShown { shown, near_end });
    }
}
