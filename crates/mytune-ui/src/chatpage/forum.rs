//! Discord forum channels, after Yapfast's `ui/forum.slint` plus Discord's sort and tag filter (those two work
//! on the posts already loaded; the engine has neither): a search bar with New Post, the sort and tag row, post
//! cards, the open post beside the forum (the engine's "aside"), and the New Post form.

use super::voice::{glyph, k_edge, k_panel, QUICK, k_raised};
use super::*;
use egui::{pos2, Rect, Stroke};
use mytune_chat::yapfast_core::application::{Application, ForumPost};
use mytune_chat::yapfast_core::chat::ShownEmoji;
use std::time::{SystemTime, UNIX_EPOCH};

const I_PIN: &str = "\u{E718}";
const I_CHAT: &str = "\u{E8BD}";
const I_SEARCH: &str = "\u{E721}";
const I_CLOSE: &str = "\u{E711}";
const I_EXPAND: &str = "\u{E740}";

/// What the forum view keeps between frames.
#[derive(Default)]
pub struct ForumUi {
    search: String,
    /// Sort by creation date (true) or by recent activity (false); `None` keeps the forum's own order.
    by_creation: Option<bool>,
    /// Tags whose posts show; empty shows all.
    tags: HashSet<String>,
    /// The forum the filter belongs to: another forum starts unfiltered.
    room: String,
    /// Cards shown when more posts were last asked for, so scrolling asks once per page.
    asked_at: usize,
    title: String,
    body: String,
    form_open: bool,
    aside_draft: String,
    aside_room: String,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// "4m ago", as Discord's forum shows a post's last activity.
pub fn ago(ms: u64, now: u64) -> String {
    let s = now.saturating_sub(ms) / 1000;
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86_399 => format!("{}h ago", s / 3600),
        86_400..=2_591_999 => format!("{}d ago", s / 86_400),
        _ => format!("{}mo ago", s / 2_592_000),
    }
}

fn rgb(c: u32) -> Color32 {
    Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)
}

/// A tag's or reaction's emoji: its picture when custom, else its text.
fn emoji(ui: &mut Ui, s: &AppState, e: &ShownEmoji, size: f32, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>) {
    let ctx = ui.ctx().clone();
    match texture(&ctx, tex, asked, s, &e.picture, want) {
        Some(t) => {
            ui.add(egui::Image::new(egui::load::SizedTexture::new(t, vec2(size, size))));
        }
        None if !e.text.is_empty() => {
            ui.label(rt(&e.text, size));
        }
        None => {}
    }
}

/// A rounded chip (tag filter, Sort & view, tag on a card); `on` shows it chosen.
fn chip(ui: &mut Ui, s: &AppState, e: Option<&ShownEmoji>, text: &str, on: bool, small: bool, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>) -> egui::Response {
    let h = if small { 20.0 } else { 32.0 };
    let r = egui::Frame::new()
        .fill(if on { p().accent.gamma_multiply(0.25) } else { k_raised() })
        .stroke(Stroke::new(1.0, if on { p().accent } else { k_line() }))
        .corner_radius(h / 2.0)
        .inner_margin(egui::Margin::symmetric(if small { 8 } else { 12 }, 0))
        .show(ui, |ui| {
            ui.set_height(h);
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                if let Some(e) = e {
                    emoji(ui, s, e, if small { 12.0 } else { 14.0 }, tex, asked, want);
                }
                ui.label(sb(text, if small { 11.5 } else { 13.0 }).color(if on { TEXT } else { SECONDARY }));
            });
        });
    ui.interact(r.response.rect, r.response.id.with("chip"), Sense::click())
}

/// The forum in place of messages: search and New Post, sort and tags, then the cards (more load on scroll).
#[allow(clippy::too_many_arguments)]
pub fn view(ui: &mut Ui, fu: &mut ForumUi, app: &Application, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let s = &app.state;
    let Some(forum) = s.forum() else { return };
    if fu.room != forum.list.room {
        *fu = ForumUi { room: forum.list.room.clone(), aside_draft: std::mem::take(&mut fu.aside_draft), aside_room: std::mem::take(&mut fu.aside_room), ..Default::default() };
    }
    // shortcut: forum_posts() builds the card list each frame (tens of posts); cache it if long forums get slow.
    let Some(posts) = app.forum_posts() else { return };
    let info = app.forum_info(&forum.list.room).unwrap_or_default();
    ui.add_space(14.0);
    // search or create a post
    ui.horizontal(|ui| {
        let w = ui.available_width();
        egui::Frame::new().fill(k_field()).corner_radius(8.0).stroke(Stroke::new(1.0, k_line())).inner_margin(egui::Margin::symmetric(12, 9)).show(ui, |ui| {
            ui.set_width(w - if app.can_post() { 130.0 } else { 26.0 });
            ui.horizontal(|ui| {
                let (ir, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                glyph(ui.painter(), ir.center(), I_SEARCH, 15.0, MUTED, false);
                let te = ui.add(egui::TextEdit::singleline(&mut fu.search).frame(egui::Frame::NONE).desired_width(ui.available_width() - 28.0).hint_text(rt("Search or create a post\u{2026}", 15.0).color(FAINT)).font(egui::FontId::proportional(15.0)));
                if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    acts.push(Action::SearchPosts(fu.search.trim().to_owned()));
                }
                if !fu.search.is_empty() {
                    let (cr, cresp) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::click());
                    glyph(ui.painter(), cr.center(), I_CLOSE, 11.0, MUTED, false);
                    if cresp.clicked() {
                        fu.search.clear();
                        acts.push(Action::SearchPosts(String::new()));
                    }
                }
            });
        });
        if app.can_post() && pill(ui, "  New Post  ", true).clicked() {
            fu.form_open = false; // the form takes its text from the engine when it opens
            acts.push(Action::NewPost);
        }
    });
    ui.add_space(12.0);
    // sort and tag filter
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        let by_creation = fu.by_creation.unwrap_or(info.by_creation);
        let sort = chip(ui, s, None, if by_creation { "Sort: Date Posted" } else { "Sort: Recently Active" }, false, false, tex, asked, want);
        egui::Popup::menu(&sort).show(|ui| {
            ui.label(rt("Sort by", 11.0).color(MUTED));
            if ui.selectable_label(!by_creation, "Recently Active").clicked() {
                fu.by_creation = Some(false);
            }
            if ui.selectable_label(by_creation, "Date Posted").clicked() {
                fu.by_creation = Some(true);
            }
        });
        if !info.tags.is_empty() {
            let (line, _) = ui.allocate_exact_size(vec2(1.0, 24.0), Sense::hover());
            ui.painter().rect_filled(line, 0.0, k_line());
        }
        for t in &info.tags {
            let on = fu.tags.contains(&t.id);
            if chip(ui, s, t.emoji.as_ref(), &t.name, on, false, tex, asked, want).clicked() {
                if on {
                    fu.tags.remove(&t.id);
                } else {
                    fu.tags.insert(t.id.clone());
                }
            }
        }
        if !fu.tags.is_empty() && chip(ui, s, None, "Clear", false, false, tex, asked, want).clicked() {
            fu.tags.clear();
        }
    });
    ui.add_space(12.0);
    // the cards
    let searching = posts.found.is_some();
    let mut list: Vec<&ForumPost> = match &posts.found {
        Some(Some(Ok(found))) => found.iter().collect(),
        Some(None) => {
            ui.label(rt("Searching\u{2026}", 13.0).color(MUTED));
            vec![]
        }
        Some(Some(Err(e))) => {
            ui.label(rt(e, 13.0).color(DANGER));
            vec![]
        }
        None => posts.active.iter().chain(posts.older.iter()).collect(),
    };
    let cards = list.len();
    if !fu.tags.is_empty() {
        list.retain(|p| p.tags.iter().any(|t| fu.tags.contains(&t.id)));
    }
    if let Some(by_creation) = fu.by_creation.filter(|b| *b != info.by_creation) {
        let key = |p: &&ForumPost| if by_creation { p.thread.created_ms } else { p.thread.active_ms.max(p.thread.created_ms) };
        list.sort_by(|a, b| b.thread.pinned.cmp(&a.thread.pinned).then(key(b).cmp(&key(a))));
    }
    let now = now_ms();
    let out = egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        if list.is_empty() && !forum.loading() && posts.found != Some(None) {
            ui.add_space(40.0);
            ui.vertical_centered(|ui| ui.label(rt(if searching || !fu.tags.is_empty() { "No results found." } else { "There are no posts yet." }, 14.0).color(MUTED)));
        }
        for post in &list {
            card(ui, s, post, now, tex, asked, want, acts);
        }
        if forum.loading() {
            ui.label(rt("Loading\u{2026}", 13.0).color(MUTED));
        }
        ui.add_space(12.0);
    });
    // within 200 px of the end, the next page (once per page)
    let near_end = out.state.offset().y + out.inner_rect.height() >= out.content_size.y - 200.0;
    if near_end && !forum.loading() && forum.wants_more(cards) && fu.asked_at != cards {
        fu.asked_at = cards;
        acts.push(Action::MorePosts);
    }
}

/// One post card: tags, title (bright when new or unread, with a NEW badge), the first message, then reaction,
/// replies and last activity; its picture on the right. Click opens it beside the forum, Shift-click in place.
#[allow(clippy::too_many_arguments)]
fn card(ui: &mut Ui, s: &AppState, post: &ForumPost, now: u64, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    let id = ui.id().with(("post", &post.thread.room));
    let bg = ui.painter().add(egui::Shape::Noop);
    let w = ui.available_width();
    let r = egui::Frame::new().inner_margin(egui::Margin { left: 16, right: 12, top: 12, bottom: 12 }).show(ui, |ui| {
        ui.set_width(w - 28.0);
        ui.horizontal_top(|ui| {
            let starter = post.starter.as_ref().and_then(|x| x.as_ref());
            let pic = starter.and_then(|x| x.picture.clone());
            let text_w = ui.available_width() - if pic.is_some() { 88.0 } else { 0.0 };
            ui.vertical(|ui| {
                ui.set_width(text_w);
                ui.spacing_mut().item_spacing.y = 6.0;
                if post.thread.pinned || !post.tags.is_empty() {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        if post.thread.pinned {
                            let (pr, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                            ui.painter().circle_filled(pr.center(), 10.0, p().accent);
                            glyph(ui.painter(), pr.center(), I_PIN, 11.0, p().on_accent, false);
                        }
                        for t in post.tags.iter().take(3) {
                            chip(ui, s, t.emoji.as_ref(), &t.name, false, true, tex, asked, want);
                        }
                        if post.tags.len() > 3 {
                            chip(ui, s, None, &format!("+{}", post.tags.len() - 3), false, true, tex, asked, want);
                        }
                    });
                }
                ui.horizontal(|ui| {
                    let bright = post.new || post.thread.unread;
                    ui.add(egui::Label::new(sb(&post.thread.name, 16.0).color(if bright { TEXT } else { SECONDARY })).truncate());
                    if post.new {
                        egui::Frame::new().fill(p().accent).corner_radius(8.0).inner_margin(egui::Margin::symmetric(6, 1)).show(ui, |ui| {
                            ui.label(sb("NEW", 10.0).color(p().on_accent));
                        });
                    }
                });
                match starter {
                    Some(st) => {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.0;
                            ui.label(sb(format!("{}: ", st.author.name), 14.0).color(st.author.color.map(rgb).unwrap_or(TEXT)));
                            let line: String = st.text.lines().collect::<Vec<_>>().join(" ");
                            ui.add(egui::Label::new(rt(line, 14.0).color(MUTED)).truncate());
                        });
                    }
                    None if post.starter.is_some() => {
                        ui.label(rt("Original message was deleted", 13.0).color(FAINT).italics());
                    }
                    None => {
                        ui.label(rt("\u{2026}", 13.0).color(FAINT));
                    }
                }
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    if let Some((e, n)) = starter.and_then(|x| x.reaction.as_ref()) {
                        egui::Frame::new().fill(k_raised()).stroke(Stroke::new(1.0, k_line())).corner_radius(6.0).inner_margin(egui::Margin::symmetric(6, 2)).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                emoji(ui, s, e, 14.0, tex, asked, want);
                                ui.label(rt(n.to_string(), 13.0).color(SECONDARY));
                            });
                        });
                    }
                    let (ir, _) = ui.allocate_exact_size(vec2(16.0, 18.0), Sense::hover());
                    glyph(ui.painter(), ir.center(), I_CHAT, 13.0, MUTED, false);
                    ui.label(rt(post.thread.replies.to_string(), 13.0).color(SECONDARY));
                    ui.label(rt("\u{2022}", 13.0).color(FAINT));
                    ui.label(rt(ago(post.thread.active_ms.max(post.thread.created_ms), now), 13.0).color(MUTED));
                });
            });
            if let Some(url) = pic {
                let (pr, _) = ui.allocate_exact_size(vec2(72.0, 72.0), Sense::hover());
                match texture(&ctx, tex, asked, s, &Some(url), want) {
                    Some(t) => {
                        let size = tex.values().find(|h| h.id() == t).map_or(vec2(1.0, 1.0), |h| h.size_vec2());
                        let (ra, ta) = (1.0, size.x / size.y.max(1.0));
                        let uv = if ta > ra { let w = ra / ta; Rect::from_min_max(pos2((1.0 - w) / 2.0, 0.0), pos2((1.0 + w) / 2.0, 1.0)) } else { let h = ta / ra; Rect::from_min_max(pos2(0.0, (1.0 - h) / 2.0), pos2(1.0, (1.0 + h) / 2.0)) };
                        egui::Image::new(egui::load::SizedTexture::new(t, pr.size())).uv(uv).corner_radius(8.0).paint_at(ui, pr);
                    }
                    None => {
                        ui.painter().rect_filled(pr, 8.0, k_raised());
                    }
                }
                if starter.is_some_and(|x| x.spoiler) {
                    ui.painter().rect_filled(pr, 8.0, Color32::from_black_alpha(200));
                    ui.painter().text(pr.center(), egui::Align2::CENTER_CENTER, "SPOILER", egui::FontId::new(10.0, th::semibold()), TEXT);
                }
            }
        });
    });
    let rect = r.response.rect;
    let resp = ui.interact(rect, id, Sense::click());
    let t = ctx.animate_bool_with_time(id.with("hover"), resp.hovered(), QUICK);
    ui.painter().set(bg, egui::epaint::RectShape::new(rect, 8.0, k_hover().lerp_to_gamma(k_raised(), t), Stroke::new(1.0, k_line().lerp_to_gamma(k_active(), t)), egui::StrokeKind::Inside));
    if resp.clicked() {
        acts.push(if ui.input(|i| i.modifiers.shift) { Action::Open(post.thread.room.clone()) } else { Action::OpenAside(post.thread.room.clone()) });
    }
}

/// The open post beside the forum: its name with Open-in-place and Close, its messages, a reply box.
#[allow(clippy::too_many_arguments)]
pub fn aside(ui: &mut Ui, fu: &mut ForumUi, look: Look, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let Some(a) = &s.aside else { return };
    if fu.aside_room != a.room {
        fu.aside_room = a.room.clone();
        fu.aside_draft = s.aside_draft().to_owned();
    }
    let name = s.aside_room().map_or("Post", |r| r.name.as_str());
    egui::Panel::top("aside_head").frame(egui::Frame::new().fill(k_panel()).inner_margin(egui::Margin::symmetric(12, 10)).stroke(Stroke::new(1.0, k_edge()))).show_separator_line(false).show_inside(ui, |ui| {
        ui.horizontal(|ui| {
            ui.add(egui::Label::new(sb(name, 15.0).color(TEXT)).truncate());
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let mut b = |ui: &mut Ui, g: &str, tip: &str| {
                    let (r, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
                    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), QUICK);
                    ui.painter().rect_filled(r, 6.0, k_hover().gamma_multiply(t));
                    glyph(ui.painter(), r.center(), g, 14.0, SECONDARY.lerp_to_gamma(TEXT, t), false);
                    resp.on_hover_text(tip).clicked()
                };
                if b(ui, I_CLOSE, "Close") {
                    acts.push(Action::CloseAside);
                }
                if b(ui, I_EXPAND, "Open in full view") {
                    acts.push(Action::ExpandAside);
                }
            });
        });
    });
    let writable = s.can_write_in(&a.room);
    egui::Panel::bottom("aside_composer").frame(egui::Frame::new().fill(k_sidebar()).inner_margin(egui::Margin::same(12))).show_separator_line(false).show_inside(ui, |ui| {
        if !writable {
            ui.label(rt("You cannot reply here.", 13.0).color(MUTED));
            return;
        }
        egui::Frame::new().fill(k_field()).corner_radius(10.0).inner_margin(egui::Margin::symmetric(12, 9)).show(ui, |ui| {
            let te = ui.add(egui::TextEdit::singleline(&mut fu.aside_draft).frame(egui::Frame::NONE).desired_width(f32::INFINITY).hint_text(rt(format!("Message {name}"), 13.5).color(FAINT)));
            if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !fu.aside_draft.trim().is_empty() && s.can_send_in(&a.room) {
                acts.push(Action::SetAsideDraft(std::mem::take(&mut fu.aside_draft)));
                acts.push(Action::SendAside);
                te.request_focus();
            }
        });
    });
    egui::CentralPanel::default().frame(egui::Frame::new().fill(k_sidebar())).show_inside(ui, |ui| {
        if !a.ready {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| ui.label(rt("Loading\u{2026}", 13.0).color(MUTED)));
            return;
        }
        egui::ScrollArea::vertical().auto_shrink(false).stick_to_bottom(true).show(ui, |ui| {
            if !a.history_start {
                ui.add_space(6.0);
                ui.vertical_centered(|ui| {
                    if a.loading_history {
                        ui.label(rt("Loading earlier messages\u{2026}", 12.0).color(MUTED));
                    } else if pill(ui, "Load earlier messages", false).clicked() {
                        acts.push(Action::AsideEarlier);
                    }
                });
            }
            let mut seen = Vec::new();
            message_list(ui, s, &a.messages, tex, asked, want, acts, look, None, &mut None, &mut seen);
            acts.push(Action::VisibleMessages { room: a.room.clone(), items: seen, at_end: true });
        });
    });
}

/// The New Post form (a window over the forum): title, tags, message, Post; closing with text asks first.
#[allow(clippy::too_many_arguments)]
pub fn new_post(ctx: &egui::Context, fu: &mut ForumUi, app: &Application, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let s = &app.state;
    let Some(np) = s.new_post() else {
        fu.form_open = false;
        return;
    };
    if !fu.form_open {
        fu.form_open = true;
        fu.title = np.title.clone();
        fu.body = np.body.clone();
    }
    let mut open = true;
    let required = app.forum_info(&np.room).is_some_and(|i| i.require_tag);
    egui::Window::new("New Post").id(egui::Id::new("new_post")).open(&mut open).collapsible(false).resizable(false).default_width(560.0).show(ctx, |ui| {
        ui.set_width(540.0);
        ui.label(rt("POST TITLE", 11.0).color(MUTED));
        if ui.add(egui::TextEdit::singleline(&mut fu.title).char_limit(100).desired_width(f32::INFINITY).hint_text("Give your post a title")).changed() {
            acts.push(Action::WriteNewPost { title: Some(fu.title.clone()), body: None });
        }
        let tags = app.post_tags(&np.room);
        if !tags.is_empty() {
            ui.add_space(10.0);
            ui.label(rt(if required { "TAGS (REQUIRED)" } else { "TAGS" }, 11.0).color(MUTED));
            ui.label(rt("Select up to 5 tags", 11.5).color(FAINT));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for t in &tags {
                    if chip(ui, s, t.emoji.as_ref(), &t.name, np.tags.contains(&t.id), false, tex, asked, want).clicked() {
                        acts.push(Action::TogglePostTag(t.id.clone()));
                    }
                }
            });
        }
        ui.add_space(10.0);
        ui.label(rt("MESSAGE", 11.0).color(MUTED));
        if ui.add(egui::TextEdit::multiline(&mut fu.body).desired_rows(6).desired_width(f32::INFINITY).hint_text("What do you want to share?")).changed() {
            acts.push(Action::WriteNewPost { title: None, body: Some(fu.body.clone()) });
        }
        if let Some(e) = &np.error {
            ui.label(rt(e, 12.5).color(DANGER));
        }
        ui.add_space(10.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_enabled_ui(!np.posting, |ui| {
                if pill(ui, if np.posting { "Posting\u{2026}" } else { "Post" }, true).clicked() {
                    acts.push(Action::CreatePost);
                }
            });
        });
    });
    if !open {
        acts.push(Action::CloseNewPost);
    }
    if np.leaving {
        egui::Modal::new(egui::Id::new("leave_post")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(sb("Discard this post?", 16.0).color(TEXT));
            ui.label(rt("What you wrote will be lost.", 13.0).color(SECONDARY));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if pill(ui, "Discard", true).clicked() {
                    acts.push(Action::LeaveNewPost);
                }
                if pill(ui, "Keep editing", false).clicked() {
                    acts.push(Action::StayInNewPost);
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ago_reads_like_discord() {
        let now = 10_000_000_000;
        assert_eq!(super::ago(now - 30_000, now), "just now");
        assert_eq!(super::ago(now - 4 * 60_000, now), "4m ago");
        assert_eq!(super::ago(now - 3 * 3_600_000, now), "3h ago");
        assert_eq!(super::ago(now - 7 * 86_400_000, now), "7d ago");
    }
}
