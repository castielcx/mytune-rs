//! A message's right-click menu, after Discord's and Yapfast's: quick reactions, Reply, Forward, Edit, Copy
//! Text, Pin, Copy Message Link, Copy Message ID, Delete. Also what they open: reactions under a message, the
//! reply or edit bar over the composer, the delete confirmation and the forward dialog. Discord's Bookmark,
//! Reminder, Apps, Mark Unread, Speak and Report have no engine support and are left out.

use super::voice::{glyph, k_panel, QUICK, k_raised};
use super::*;
use mytune_chat::yapfast_core::application::{network_link, Compose};
use mytune_chat::yapfast_core::chat::ChatMessage;

/// The four quick reactions at the menu's top, as Discord offers frequent ones.
const QUICK_REACTIONS: [&str; 4] = ["\u{2764}\u{FE0F}", "\u{1F480}", "\u{1F44D}", "\u{1F64F}"];

fn item(ui: &mut Ui, g: &str, text: &str, danger: bool) -> bool {
    let c = if danger { DANGER } else { SECONDARY };
    let (r, resp) = ui.allocate_exact_size(vec2(210.0, 32.0), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), QUICK);
    if t > 0.0 {
        ui.painter().rect_filled(r, 4.0, (if danger { DANGER } else { p().accent }).gamma_multiply(0.25 * t));
    }
    glyph(ui.painter(), pos2(r.left() + 16.0, r.center().y), g, 14.0, c.lerp_to_gamma(TEXT, t), false);
    ui.painter().text(pos2(r.left() + 36.0, r.center().y), egui::Align2::LEFT_CENTER, text, egui::FontId::proportional(14.0), c.lerp_to_gamma(TEXT, t));
    resp.clicked()
}

fn sep(ui: &mut Ui) {
    ui.add_space(3.0);
    let (r, _) = ui.allocate_exact_size(vec2(210.0, 1.0), Sense::hover());
    ui.painter().rect_filled(r.shrink2(vec2(8.0, 0.0)), 0.0, k_line());
    ui.add_space(3.0);
}

/// Notes a right click on a message's row (`rect`); the menu then shows at the pointer. Hit-testing is by
/// rectangle so the row never takes clicks from its name, avatar or selectable text.
pub fn watch(ui: &Ui, rect: egui::Rect, item: &str) {
    // an attachment or reaction in the row that took this right click for its own menu blocks this one
    let taken = ui.ctx().data(|d| d.get_temp::<u64>(egui::Id::new("msg_menu_block"))) == Some(ui.ctx().cumulative_pass_nr());
    if !taken && ui.input(|i| i.pointer.secondary_clicked()) && ui.rect_contains_pointer(rect) {
        let at = ui.input(|i| i.pointer.interact_pos()).unwrap_or(rect.left_top());
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("msg_menu"), (item.to_owned(), at)));
    }
}

/// The open message menu, if a message was right-clicked; closes on a pick, Escape or a click elsewhere.
pub fn message_menu(ctx: &egui::Context, s: &AppState, pk: &mut super::picker::PickerUi, acts: &mut Vec<Action>) {
    let Some((item_id, at)) = ctx.data(|d| d.get_temp::<(String, egui::Pos2)>(egui::Id::new("msg_menu"))) else { return };
    let Some(m) = s.messages.iter().find(|m| m.item == item_id).cloned() else {
        ctx.data_mut(|d| d.remove::<(String, egui::Pos2)>(egui::Id::new("msg_menu")));
        return;
    };
    let m = &m;
    let room = s.selected.clone().unwrap_or_default();
    // The menu's corner sits on the pointer: below and right of it, or flipped above/left when it would not fit.
    // One area per message, so its size is measured (first, invisible frame) before it is placed.
    let area_id = egui::Id::new(("msg_menu_area", &item_id));
    let screen = ctx.content_rect();
    let size = ctx.memory(|mem| mem.area_rect(area_id)).map_or(egui::Vec2::ZERO, |r| r.size());
    let pos = pos2(if at.x + size.x > screen.right() { at.x - size.x } else { at.x }, if at.y + size.y > screen.bottom() { at.y - size.y } else { at.y });
    let mut close = false;
    let shown = egui::Area::new(area_id).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
      egui::Frame::new().fill(k_panel()).stroke(egui::Stroke::new(1.0, super::voice::k_edge())).corner_radius(8.0).inner_margin(egui::Margin::same(6)).shadow(egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(0x66) }).show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        if m.can_react {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                for e in QUICK_REACTIONS {
                    let (r, er) = ui.allocate_exact_size(vec2(45.0, 40.0), Sense::click());
                    let t = ui.ctx().animate_bool_with_time(er.id, er.hovered(), QUICK);
                    ui.painter().rect_filled(r, 6.0, k_raised().lerp_to_gamma(k_active(), t));
                    match crate::emoji::texture(ui.ctx(), e) {
                        Some(t) => egui::Image::new(egui::load::SizedTexture::new(t, vec2(24.0, 24.0))).paint_at(ui, egui::Rect::from_center_size(r.center(), vec2(24.0, 24.0))),
                        None => {
                            ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, e, egui::FontId::proportional(20.0), TEXT);
                        }
                    }
                    if er.clicked() {
                        acts.push(Action::React { item: m.item.clone(), key: e.to_owned() });
                        close = true;
                    }
                }
            });
            if item(ui, "\u{E76E}", "Add Reaction", false) {
                pk.toggle(super::picker::For::Reaction(m.item.clone()), false, Some(at), acts);
                close = true;
            }
            sep(ui);
        }
        if !m.reactions.is_empty() && item(ui, "\u{E8F2}", "View Reactions", false) {
            acts.push(Action::ShowReactions { item: m.item.clone(), key: None });
            close = true;
        }
        if item(ui, "\u{E97A}", "Reply", false) {
            acts.push(Action::Reply(m.item.clone()));
            ui.ctx().memory_mut(|mem| mem.request_focus(super::compose::edit_id()));
            close = true;
        }
        if item(ui, "\u{E72D}", "Forward", false) {
            acts.push(Action::StartForward(m.item.clone()));
            close = true;
        }
        if m.can_edit && item(ui, "\u{E70F}", "Edit Message", false) {
            acts.push(Action::Edit(m.item.clone()));
            ui.ctx().memory_mut(|mem| mem.request_focus(super::compose::edit_id()));
            close = true;
        }
        sep(ui);
        if item(ui, "\u{E8C8}", "Copy Text", false) {
            ui.ctx().copy_text(m.body.clone());
            close = true;
        }
        if m.can_pin && item(ui, "\u{E718}", if m.pinned { "Unpin Message" } else { "Pin Message" }, false) {
            acts.push(Action::PinMessage(m.item.clone()));
            close = true;
        }
        if m.thread.is_none() && s.threads_channel().is_some() && item(ui, "\u{E8BD}", "Create Thread", false) {
            acts.push(Action::NewThread { message: Some(m.item.clone()), from: mytune_chat::yapfast_core::application::ThreadFrom::Message });
            close = true;
        }
        let snowflake = m.item.strip_prefix("d:");
        if let Some(id) = snowflake {
            if item(ui, "\u{E71B}", "Copy Message Link", false) {
                ui.ctx().copy_text(format!("{}/{id}", network_link(&room)));
                close = true;
            }
        }
        if m.can_delete {
            sep(ui);
            if item(ui, "\u{E74D}", "Delete Message", true) {
                acts.push(Action::RequestDelete(m.item.clone()));
                close = true;
            }
        }
        if let Some(id) = snowflake {
            sep(ui);
            if item(ui, "\u{E8EC}", "Copy Message ID", false) {
                ui.ctx().copy_text(id.to_owned());
                close = true;
            }
        }
      });
    });
    let scrolled = ctx.input(|i| i.smooth_scroll_delta != egui::Vec2::ZERO || i.raw.events.iter().any(|e| matches!(e, egui::Event::MouseWheel { .. })));
    if close || scrolled || (shown.response.clicked_elsewhere() && !ctx.input(|i| i.pointer.secondary_clicked())) || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        ctx.data_mut(|d| d.remove::<(String, egui::Pos2)>(egui::Id::new("msg_menu")));
    }
}

/// The reactions under a message: emoji and count in a chip, outlined in the accent when they include this
/// account's; a click adds or takes back this account's.
pub fn reactions(ui: &mut Ui, s: &AppState, m: &ChatMessage, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    if m.reactions.is_empty() {
        return;
    }
    let ctx = ui.ctx().clone();
    ui.add_space(4.0);
    // rows broken by hand at the visible edge of the message area: egui does not wrap nested frames
    let w = (ui.clip_rect().right() - ui.cursor().left() - 12.0).max(120.0);
    struct Chip<'a> {
        r: &'a mytune_chat::yapfast_core::chat::Reaction,
        pic: Option<egui::TextureId>,
        text: Option<(String, f32)>,
        width: f32,
    }
    let measure = |t: &str, size: f32| ctx.fonts_mut(|f| f.layout_no_wrap(t.to_owned(), egui::FontId::proportional(size), TEXT).size().x);
    let chips: Vec<Chip> = m
        .reactions
        .iter()
        .map(|r| {
            // a custom emoji's key is "name:id": its picture from Discord, else its name
            // a server emoji's key is "name:id", or ":name:" as the message lists it with its picture
            let custom = r.key.split_once(':').filter(|(_, id)| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()));
            let listed = m.emoji.iter().find(|e| e.text == r.key);
            let custom = custom.or_else(|| listed.map(|e| (e.text.trim_matches(':'), e.id.as_str())));
            let url = match listed {
                Some(e) => Some(e.picture.clone()),
                None => custom.and_then(|(_, id)| mytune_chat::yapfast_core::discord::emoji_picture(id)),
            };
            let pic = url.as_ref().and_then(|u| texture(&ctx, tex, asked, s, &Some(u.clone()), want));
            let text = match (pic, custom) {
                (Some(_), _) => None,
                (None, Some((name, _))) => Some((format!(":{name}:"), 12.5)),
                (None, None) => Some((r.key.clone(), 15.0)),
            };
            let lead = text.as_ref().map_or(18.0, |(t, size)| measure(t, *size));
            let width = 14.0 + lead + 5.0 + measure(&r.count.to_string(), 12.5) + 4.0;
            Chip { r, pic, text, width }
        })
        .collect();
    let mut rows: Vec<Vec<&Chip>> = vec![vec![]];
    let mut used = 0.0;
    for c in &chips {
        if used + c.width > w && !rows.last().is_some_and(|r| r.is_empty()) {
            rows.push(vec![]);
            used = 0.0;
        }
        used += c.width + 4.0;
        rows.last_mut().map(|r| r.push(c));
    }
    for row in rows {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for c in row {
                let r = c.r;
                let resp = egui::Frame::new()
                    .fill(if r.mine { p().accent.gamma_multiply(0.18) } else { k_raised() })
                    .stroke(egui::Stroke::new(1.0, if r.mine { p().accent } else { Color32::TRANSPARENT }))
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(7, 2))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 5.0;
                            match (&c.pic, &c.text) {
                                (Some(t), _) => {
                                    ui.add(egui::Image::new(egui::load::SizedTexture::new(*t, vec2(18.0, 18.0))));
                                }
                                (None, Some((t, size))) => match crate::emoji::texture(&ctx, t).filter(|_| *size >= 15.0) {
                                    Some(e) => {
                                        ui.add(egui::Image::new(egui::load::SizedTexture::new(e, vec2(18.0, 18.0))));
                                    }
                                    None => {
                                        ui.label(rt(t, *size).color(if *size < 15.0 { SECONDARY } else { TEXT }));
                                    }
                                },
                                (None, None) => {}
                            }
                            ui.label(sb(r.count.to_string(), 12.5).color(if r.mine { TEXT } else { SECONDARY }));
                        });
                    })
                    .response;
                // click: add or take back this account's; right click: who reacted
                let chip = ui.interact(resp.rect, resp.id.with("react"), Sense::click()).on_hover_text("Right-click to see who reacted");
                if m.can_react && chip.clone().on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    acts.push(Action::React { item: m.item.clone(), key: r.key.clone() });
                }
                if chip.secondary_clicked() {
                    ctx.data_mut(|d| d.insert_temp(egui::Id::new("msg_menu_block"), ctx.cumulative_pass_nr()));
                    acts.push(Action::ShowReactions { item: m.item.clone(), key: Some(r.key.clone()) });
                }
            }
        });
    }
}

/// "Replying to …" (with the mention switch) or "Editing message" over the composer; Escape or × cancels.
/// Returns the draft the engine holds when editing starts, for the composer to show.
pub fn compose_bar(ui: &mut Ui, s: &AppState, acts: &mut Vec<Action>) {
    let Some(c) = s.compose() else { return };
    egui::Frame::new().fill(k_panel()).corner_radius(egui::CornerRadius { nw: 12, ne: 12, sw: 0, se: 0 }).inner_margin(egui::Margin::symmetric(14, 7)).show(ui, |ui| {
        ui.horizontal(|ui| {
            match c {
                Compose::Reply { author, ping, .. } => {
                    ui.label(rt("Replying to", 13.0).color(MUTED));
                    ui.label(sb(author, 13.0).color(TEXT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(egui::Button::new(rt("\u{E711}", 11.0).family(th::icons()).color(MUTED)).frame(false)).on_hover_text("Cancel").clicked() {
                            acts.push(Action::CancelCompose);
                        }
                        let label = if *ping { "@ ON" } else { "@ OFF" };
                        if ui.add(egui::Button::new(sb(label, 12.0).color(if *ping { p().accent } else { MUTED })).frame(false)).on_hover_text("Mention the author").clicked() {
                            acts.push(Action::ToggleReplyPing { aside: false });
                        }
                    });
                }
                Compose::Edit { .. } => {
                    ui.label(rt("Editing message", 13.0).color(MUTED));
                    ui.label(rt("Escape to cancel, Enter to save", 12.0).color(FAINT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(egui::Button::new(rt("\u{E711}", 11.0).family(th::icons()).color(MUTED)).frame(false)).clicked() {
                            acts.push(Action::CancelCompose);
                        }
                    });
                }
            }
        });
    });
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        acts.push(Action::CancelCompose);
    }
}

/// Asks before deleting a message.
pub fn confirm_delete(ctx: &egui::Context, s: &AppState, acts: &mut Vec<Action>) {
    if s.confirm_delete.is_none() {
        return;
    }
    egui::Modal::new(egui::Id::new("confirm_delete")).show(ctx, |ui| {
        ui.set_width(400.0);
        ui.label(sb("Delete Message", 17.0).color(TEXT));
        ui.add_space(4.0);
        ui.label(rt("Are you sure you want to delete this message? This cannot be undone.", 13.5).color(SECONDARY));
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add(egui::Button::new(sb("Delete", 13.0).color(Color32::WHITE)).fill(DANGER).corner_radius(8.0).min_size(vec2(90.0, 34.0))).clicked() {
                acts.push(Action::ConfirmDelete);
            }
            if ui.add(egui::Button::new(sb("Cancel", 13.0).color(TEXT)).fill(k_raised()).corner_radius(8.0).min_size(vec2(90.0, 34.0))).clicked() {
                acts.push(Action::CancelDelete);
            }
        });
    });
}

/// What the forward dialog keeps between frames.
#[derive(Default)]
pub struct ForwardUi {
    query: String,
    note: String,
}

/// The forward dialog: the message, a search, conversations to tick, an optional note, Send.
pub fn forward(ctx: &egui::Context, fw: &mut ForwardUi, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let Some(f) = &s.forwarding else {
        fw.query.clear();
        fw.note.clear();
        return;
    };
    let mut open = true;
    egui::Window::new("Forward To").id(egui::Id::new("forward")).open(&mut open).collapsible(false).resizable(false).default_width(440.0).show(ctx, |ui| {
        ui.set_width(420.0);
        egui::Frame::new().fill(k_rail()).corner_radius(8.0).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
            ui.set_width(400.0);
            if ui.add(egui::TextEdit::singleline(&mut fw.query).frame(egui::Frame::NONE).desired_width(f32::INFINITY).hint_text("Search")).changed() {
                acts.push(Action::SearchForward(fw.query.clone()));
            }
        });
        ui.add_space(6.0);
        egui::ScrollArea::vertical().max_height(300.0).show(ui, |ui| {
            for r in s.forward_places() {
                let chosen = f.chosen.contains(&r.id);
                let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), Sense::click());
                let t = ctx.animate_bool_with_time(resp.id, resp.hovered(), QUICK);
                ui.painter().rect_filled(row, 6.0, k_hover().gamma_multiply(t));
                let a = egui::Rect::from_min_size(pos2(row.left() + 6.0, row.center().y - 14.0), vec2(28.0, 28.0));
                let tx = texture(ctx, tex, asked, s, &r.avatar, want);
                avatar(ui, a, tx, &r.name);
                let name = if r.direct { r.name.clone() } else { format!("# {}", r.name) };
                ui.painter().text(pos2(a.right() + 10.0, row.center().y), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(14.0), TEXT);
                let b = egui::Rect::from_center_size(pos2(row.right() - 16.0, row.center().y), vec2(18.0, 18.0));
                ui.painter().rect(b, 4.0, if chosen { p().accent } else { Color32::TRANSPARENT }, egui::Stroke::new(1.5, if chosen { p().accent } else { MUTED }), egui::StrokeKind::Inside);
                if resp.clicked() {
                    acts.push(Action::ChooseForward(r.id.clone()));
                }
            }
        });
        ui.add_space(8.0);
        ui.label(rt(format!("\u{201C}{}\u{201D}", f.quote.chars().take(120).collect::<String>()), 12.5).color(MUTED));
        ui.add_space(6.0);
        ui.add(egui::TextEdit::singleline(&mut fw.note).desired_width(f32::INFINITY).hint_text("Add an optional message\u{2026}"));
        if let Some(e) = &f.error {
            ui.label(rt(e, 12.5).color(DANGER));
        }
        ui.add_space(8.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_enabled_ui(!f.chosen.is_empty(), |ui| {
                if pill(ui, "Send", true).clicked() {
                    acts.push(Action::Forward(fw.note.clone()));
                }
            });
        });
    });
    if !open {
        acts.push(Action::CloseForward);
    }
}

use egui::pos2;
