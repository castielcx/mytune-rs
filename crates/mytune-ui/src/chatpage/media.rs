//! What a message carries besides text: pictures, files, stickers, link previews (embeds), polls, and the
//! full-size picture viewer. Pictures come from the engine already decoded (`Attachment::preview`); a GIF shows
//! its first frame only (the engine can animate, this page does not yet).

use super::voice::{glyph, k_edge, k_raised};
use super::*;
use egui::{pos2, Stroke};
use mytune_chat::yapfast_core::chat::{Attachment, ChatMessage, Embed, Picture, Placement, Poll};

const MAX_W: f32 = 420.0;
const MAX_H: f32 = 320.0;

/// The texture of a decoded picture, made once per key.
fn pic_tex(ctx: &egui::Context, cache: &mut HashMap<String, TextureHandle>, key: String, pic: &Picture) -> egui::TextureId {
    touch(ctx, &key);
    if let Some(t) = cache.get(&key) {
        return t.id();
    }
    let img = egui::ColorImage::from_rgba_unmultiplied([pic.width as usize, pic.height as usize], &pic.rgba);
    let t = ctx.load_texture(key.clone(), img, crate::theme::SMOOTH);
    let id = t.id();
    cache.insert(key, t);
    id
}

/// A size that fits `max_w` by `max_h`, keeping the ratio and never enlarging past `from`.
fn fit(from: (f32, f32), max_w: f32, max_h: f32) -> egui::Vec2 {
    let (w, h) = (from.0.max(1.0), from.1.max(1.0));
    let k = (max_w / w).min(max_h / h).min(1.0);
    vec2((w * k).max(24.0), (h * k).max(24.0))
}

/// The size of a message picture. A sticker posted as its picture link (the Stickers tab of a local build, or
/// another client's fake stickers) fills a sticker's 160 box, enlarged when its source is small: the sources
/// are 32 to 320 px, and at their own size some showed tiny.
fn sticker_link(a: &Attachment) -> bool {
    matches!(a.placement, Placement::Message) && a.url.as_deref().is_some_and(|u| u.contains("/stickers/"))
}

fn picture_size(a: &Attachment, from: (f32, f32)) -> egui::Vec2 {
    if !sticker_link(a) {
        return fit(from, MAX_W, MAX_H);
    }
    let (w, h) = (from.0.max(1.0), from.1.max(1.0));
    let k = (160.0 / w).min(160.0 / h);
    vec2(w * k, h * k)
}

fn human_size(b: u64) -> String {
    match b {
        0..=1023 => format!("{b} B"),
        1024..=1_048_575 => format!("{:.1} KB", b as f64 / 1024.0),
        _ => format!("{:.1} MB", b as f64 / 1_048_576.0),
    }
}

/// Files, pictures, stickers, embeds and the poll of a message, under its text.
pub fn content(ui: &mut Ui, s: &AppState, m: &ChatMessage, tex: &mut HashMap<String, TextureHandle>, acts: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    for (i, a) in m.attachments.iter().enumerate() {
        if matches!(a.placement, Placement::Message | Placement::Sticker) {
            // a sticker-only message has no text above it to space from: its picture follows the row's gap alone
            ui.add_space(if matches!(a.placement, Placement::Sticker) || sticker_link(a) { 0.0 } else { 4.0 });
            let playing = s.video.as_ref().filter(|v| v.item == m.item && v.index == i);
            attachment(ui, &ctx, m, i, a, playing, tex, acts);
        }
    }
    for (i, e) in m.embeds.iter().enumerate() {
        ui.add_space(4.0);
        embed(ui, &ctx, m, i, e, tex, acts);
    }
    if let Some(p) = &m.poll {
        ui.add_space(4.0);
        poll_card(ui, m, p, acts);
    }
}

/// The right-click menu of a picture, video or file: Save (a save dialog, run by the chat page with
/// [`take_save`]), Copy Image, Copy Link, Open in Browser.
fn attachment_menu(resp: &egui::Response, ctx: &egui::Context, m: &ChatMessage, i: usize, a: &Attachment, acts: &mut Vec<Action>) {
    if resp.secondary_clicked() {
        // this right click is the attachment's: the message menu (menu::watch) stays shut
        let pass = ctx.cumulative_pass_nr(); // read outside data_mut: nested ctx locks deadlock
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("msg_menu_block"), pass));
    }
    let _ = acts;
    if resp.secondary_clicked() {
        let at = ctx.input(|i| i.pointer.interact_pos()).unwrap_or(resp.rect.center());
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("att_menu"), (m.item.clone(), i, at)));
    }
}

/// The open attachment menu, drawn like the message menu: floating glass, hover rows, closes on a pick,
/// Escape or a click elsewhere.
pub fn attachment_menu_show(ctx: &egui::Context, s: &AppState, acts: &mut Vec<Action>) {
    let Some((item, index, at)) = ctx.data(|d| d.get_temp::<(String, usize, egui::Pos2)>(egui::Id::new("att_menu"))) else { return };
    let Some((m, a)) = s.message(&item).and_then(|m| m.attachments.get(index).map(|a| (m, a))) else {
        ctx.data_mut(|d| d.remove::<(String, usize, egui::Pos2)>(egui::Id::new("att_menu")));
        return;
    };
    let area_id = egui::Id::new("att_menu_area");
    let screen = ctx.content_rect();
    let size = ctx.memory(|mem| mem.area_rect(area_id)).map_or(egui::Vec2::ZERO, |r| r.size());
    let pos = pos2(if at.x + size.x > screen.right() { at.x - size.x } else { at.x }, if at.y + size.y > screen.bottom() { at.y - size.y } else { at.y });
    let mut close = false;
    let shown = egui::Area::new(area_id).order(egui::Order::Tooltip).fixed_pos(pos).show(ctx, |ui| {
        crate::theme::float(ui, crate::theme::R_MENU, 6, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            if super::menu::item(ui, "\u{E74E}", "Save\u{2026}", false) {
                ctx.data_mut(|d| d.insert_temp(egui::Id::new("save_attachment"), (m.item.clone(), index, a.name.clone())));
                close = true;
            }
            if a.image && !a.video && super::menu::item(ui, "\u{E8C8}", "Copy Image", false) {
                acts.push(Action::CopyAttachment { item: m.item.clone(), index });
                close = true;
            }
            if let Some(u) = &a.url {
                if super::menu::item(ui, "\u{E71B}", "Copy Link", false) {
                    ctx.copy_text(u.to_string());
                    close = true;
                }
                if super::menu::item(ui, "\u{E774}", "Open in Browser", false) {
                    super::extras::open_link(ctx, u); // web links only, checked there
                    close = true;
                }
            }
        });
    });
    if close || (shown.response.clicked_elsewhere() && !ctx.input(|i| i.pointer.secondary_clicked())) || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        ctx.data_mut(|d| d.remove::<(String, usize, egui::Pos2)>(egui::Id::new("att_menu")));
    }
}

/// An attachment Save asked for: (item, index, file name), once.
pub fn take_save(ctx: &egui::Context) -> Option<(String, usize, String)> {
    ctx.data_mut(|d| d.remove_temp(egui::Id::new("save_attachment")))
}

/// A playing video's newest frame over its still: click pauses or plays, the corner button mutes.
fn video(ui: &mut Ui, ctx: &egui::Context, rect: egui::Rect, v: &mytune_chat::yapfast_core::chat::Video, resp: &egui::Response, acts: &mut Vec<Action>) {
    use mytune_chat::yapfast_core::chat::VideoCommand;
    let key = egui::Id::new("video_frame");
    if let Some(frames) = &v.frames {
        let img = frames.take(|w, h, px| egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px[..w as usize * h as usize * 4]));
        let mut t: Option<TextureHandle> = ctx.data(|d| d.get_temp(key));
        if let Some(img) = img {
            match &mut t {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => t = Some(ctx.load_texture("video_frame", img, egui::TextureOptions::LINEAR)),
            }
            ctx.data_mut(|d| d.insert_temp(key, t.clone().unwrap()));
        }
        if let Some(t) = t {
            ui.painter().rect_filled(rect, 12.0, Color32::BLACK);
            let size = t.size_vec2();
            let inner = egui::Rect::from_center_size(rect.center(), fit((size.x, size.y), rect.width(), rect.height()));
            egui::Image::new(egui::load::SizedTexture::new(t.id(), inner.size())).paint_at(ui, inner);
        }
    }
    if v.playing {
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    } else {
        th::dot(ui.painter(), rect.center(), 24.0, Color32::from_black_alpha(150));
        glyph(ui.painter(), rect.center(), if v.loading { "\u{E895}" } else { "\u{E768}" }, 22.0, Color32::WHITE, false);
    }
    if let Some(e) = &v.error {
        ui.painter().text(rect.left_bottom() + vec2(10.0, -10.0), egui::Align2::LEFT_BOTTOM, e, egui::FontId::proportional(12.0), DANGER);
    }
    if v.sound {
        let b = egui::Rect::from_min_size(rect.right_bottom() - vec2(40.0, 40.0), vec2(32.0, 32.0));
        let mr = ui.interact(b, resp.id.with("mute"), Sense::click());
        ui.painter().rect_filled(b, 10.0, Color32::from_black_alpha(if mr.hovered() { 190 } else { 140 }));
        glyph(ui.painter(), b.center(), if v.muted { "\u{E74F}" } else { "\u{E767}" }, 14.0, Color32::WHITE, false);
        if mr.clicked() {
            acts.push(Action::Video(VideoCommand::Mute(!v.muted)));
            return;
        }
    }
    if resp.clicked() {
        acts.push(Action::Video(if v.playing { VideoCommand::Pause } else { VideoCommand::Play }));
    }
}

#[allow(clippy::too_many_arguments)]
fn attachment(ui: &mut Ui, ctx: &egui::Context, m: &ChatMessage, i: usize, a: &Attachment, playing: Option<&mytune_chat::yapfast_core::chat::Video>, tex: &mut HashMap<String, TextureHandle>, acts: &mut Vec<Action>) {
    let sticker = matches!(a.placement, Placement::Sticker);
    if let (true, Some(pic)) = (a.image || a.video || sticker, &a.preview) {
        let dims = a.dimensions.map(|(w, h)| (w as f32, h as f32)).unwrap_or((pic.width as f32, pic.height as f32));
        let size = if sticker { vec2(160.0, 160.0) } else { picture_size(a, dims) };
        let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
        let spoiler_key = egui::Id::new(("spoiler", &m.item, i));
        let covered = a.spoiler && !ctx.data(|d| d.get_temp::<bool>(spoiler_key)).unwrap_or(false);
        if covered {
            ui.painter().rect_filled(rect, 12.0, k_active());
            ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "SPOILER", egui::FontId::new(13.0, th::bold()), SECONDARY);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                ctx.data_mut(|d| d.insert_temp(spoiler_key, true));
            }
            return;
        }
        let still = pic_tex(ctx, tex, format!("att:{}:{i}", m.item), pic);
        // a GIF moves only under the pointer (a click opens it large, where it plays); else its first frame
        // with a "GIF" tag
        let moves = pic.animation.as_ref().is_some_and(|a| a.delays.len() >= 2);
        let id = if moves && resp.hovered() { animated(ctx, &format!("att:{}:{i}", m.item), pic).unwrap_or(still) } else { still };
        egui::Image::new(egui::load::SizedTexture::new(id, rect.size())).corner_radius(if sticker { 0.0 } else { 8.0 }).paint_at(ui, rect);
        if moves && !resp.hovered() && !sticker {
            let tag = egui::Rect::from_min_size(rect.left_top() + vec2(8.0, 8.0), vec2(34.0, 20.0));
            ui.painter().rect_filled(tag, 10.0, Color32::from_black_alpha(170));
            ui.painter().text(tag.center(), egui::Align2::CENTER_CENTER, "GIF", egui::FontId::new(11.0, th::bold()), Color32::WHITE);
        }
        let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
        if !sticker {
            attachment_menu(&resp, ctx, m, i, a, acts);
        }
        if let (true, Some(v)) = (a.video, playing) {
            video(ui, ctx, rect, v, &resp, acts);
            return;
        }
        if a.video {
            th::dot(ui.painter(), rect.center(), 24.0, Color32::from_black_alpha(150));
            glyph(ui.painter(), rect.center(), "\u{E768}", 22.0, Color32::WHITE, false);
        }
        if resp.clicked() && !sticker {
            if a.image && !a.video {
                acts.push(Action::ViewImage { item: m.item.clone(), index: i });
            } else if a.video {
                // plays where it shows, as in Discord
                let ppp = ctx.pixels_per_point();
                acts.push(Action::PlayVideo { item: m.item.clone(), index: i, width: (rect.width() * ppp) as u32, height: (rect.height() * ppp) as u32 });
            } else if let Some(u) = &a.url {
                super::extras::open_link(ctx, u);
            }
        }
        return;
    }
    // a picture not loaded yet (or dropped from the engine's cache): a block of the final size, so the rows
    // below do not jump when it arrives while scrolling
    if let (true, None, false, Some((w, h))) = (a.image || a.video || sticker, &a.preview, a.preview_failed, a.dimensions) {
        let size = if sticker { vec2(160.0, 160.0) } else { picture_size(a, (w as f32, h as f32)) };
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        ui.painter().rect_filled(rect, if sticker { 0.0 } else { 8.0 }, k_raised());
        return;
    }
    // a file, or a picture not loaded (yet): a chip with its name and size
    let size = a.size.map(human_size).unwrap_or_default();
    let (rect, resp) = ui.allocate_exact_size(vec2(MAX_W.min(ui.available_width()), 52.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    ui.painter().rect(rect, 8.0, if resp.hovered() { k_active() } else { k_raised() }, Stroke::new(1.0, k_edge()), egui::StrokeKind::Inside);
    glyph(ui.painter(), pos2(rect.left() + 26.0, rect.center().y), if a.image { "\u{EB9F}" } else { "\u{E8A5}" }, 20.0, SECONDARY, false);
    let label = if a.image && a.preview.is_none() && !a.preview_failed { format!("{} (loading)", a.name) } else { a.name.clone() };
    ui.painter().text(pos2(rect.left() + 50.0, rect.center().y - 8.0), egui::Align2::LEFT_CENTER, label, egui::FontId::new(13.5, th::semibold()), TEXT);
    ui.painter().text(pos2(rect.left() + 50.0, rect.center().y + 10.0), egui::Align2::LEFT_CENTER, size, egui::FontId::new(11.5, egui::FontFamily::Proportional), MUTED);
    attachment_menu(&resp, ctx, m, i, a, acts);
    if resp.clicked() {
        if let Some(u) = &a.url {
            super::extras::open_link(ctx, u);
        }
    }
}

/// The embed's footer line, for the early exit while its picture is still a block.
fn return_footer(ui: &mut Ui, e: &Embed) {
    if !e.footer.is_empty() {
        ui.add_space(4.0);
        ui.label(rt(&e.footer, 11.5).color(MUTED));
    }
}

fn embed(ui: &mut Ui, ctx: &egui::Context, m: &ChatMessage, idx: usize, e: &Embed, tex: &mut HashMap<String, TextureHandle>, acts: &mut Vec<Action>) {
    let _ = acts;
    let find = |want: fn(&Placement, usize) -> bool| m.attachments.iter().enumerate().find(|(_, a)| want(&a.placement, idx) && a.preview.is_some());
    // the big picture reserves its final size (when Discord gives it) before it loads, so rows do not jump
    let big = find(|p, i| matches!(p, Placement::Embed(n) if *n == i)).or_else(|| m.attachments.iter().enumerate().find(|(_, a)| matches!(a.placement, Placement::Embed(n) if n == idx) && !a.preview_failed && a.dimensions.is_some()));
    let small = find(|p, i| matches!(p, Placement::Thumbnail(n) if *n == i));
    let bar = e.color.map(|c| Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)).unwrap_or(k_line());
    let width = (MAX_W + 20.0).min(ui.available_width());
    let frame = egui::Frame::new().fill(k_sidebar()).corner_radius(10.0).inner_margin(egui::Margin::symmetric(14, 10)).show(ui, |ui| {
        ui.set_width(width - 28.0);
        ui.horizontal_top(|ui| {
            ui.vertical(|ui| {
                ui.set_max_width(if small.is_some() { width - 28.0 - 90.0 } else { width - 28.0 });
                if !e.provider.is_empty() {
                    ui.label(rt(&e.provider, 11.5).color(MUTED));
                }
                if !e.author.is_empty() {
                    ui.label(sb(&e.author, 13.0).color(TEXT));
                }
                if !e.title.is_empty() {
                    if e.url.is_empty() {
                        ui.add(egui::Label::new(sb(&e.title, 14.5).color(TEXT)).wrap());
                    } else if ui.add(egui::Label::new(sb(&e.title, 14.5).color(Color32::from_rgb(0x6c, 0xb4, 0xff))).wrap().sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        super::extras::open_link(ctx, &e.url);
                    }
                }
                if !e.description.is_empty() {
                    ui.add(egui::Label::new(rt(&e.description, 13.0).color(SECONDARY)).wrap().selectable(true));
                }
                for f in &e.fields {
                    ui.add_space(4.0);
                    ui.label(sb(&f.name, 12.5).color(TEXT));
                    ui.add(egui::Label::new(rt(&f.value, 12.5).color(SECONDARY)).wrap().selectable(true));
                }
            });
            if let Some((i, a)) = small {
                let id = pic_tex(ctx, tex, format!("att:{}:{i}", m.item), a.preview.as_ref().unwrap());
                let (r, _) = ui.allocate_exact_size(vec2(72.0, 72.0), Sense::hover());
                egui::Image::new(egui::load::SizedTexture::new(id, r.size())).corner_radius(10.0).paint_at(ui, r);
            }
        });
        if let Some((i, a)) = big {
            ui.add_space(6.0);
            let size = match a.preview.as_ref() {
                Some(pic) => fit((pic.width as f32, pic.height as f32), width - 28.0, MAX_H),
                None => {
                    let (w, h) = a.dimensions.unwrap_or((1, 1));
                    fit((w as f32, h as f32), width - 28.0, MAX_H)
                }
            };
            let (r, resp) = ui.allocate_exact_size(size, Sense::click());
            let Some(pic) = a.preview.as_ref() else {
                ui.painter().rect_filled(r, 10.0, k_raised());
                return_footer(ui, e);
                return;
            };
            let id = pic_tex(ctx, tex, format!("att:{}:{i}", m.item), pic);
            egui::Image::new(egui::load::SizedTexture::new(id, r.size())).corner_radius(10.0).paint_at(ui, r);
            if e.video {
                ui.painter().circle_filled(r.center(), 24.0, Color32::from_black_alpha(150));
                glyph(ui.painter(), r.center(), "\u{E768}", 22.0, Color32::WHITE, false);
            }
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                if e.video && !e.url.is_empty() {
                    super::extras::open_link(ctx, &e.url);
                } else if a.image {
                    acts.push(Action::ViewImage { item: m.item.clone(), index: i });
                }
            }
        }
        if !e.footer.is_empty() {
            ui.add_space(4.0);
            ui.label(rt(&e.footer, 11.5).color(MUTED));
        }
    });
    let r = frame.response.rect;
    ui.painter().rect_filled(egui::Rect::from_min_max(r.min, pos2(r.min.x + 4.0, r.max.y)), 2.0, bar);
}

fn poll_card(ui: &mut Ui, m: &ChatMessage, q: &Poll, acts: &mut Vec<Action>) {
    let total: usize = q.answers.iter().map(|a| a.votes).sum();
    let pal = p();
    egui::Frame::new().fill(k_sidebar()).corner_radius(12.0).inner_margin(egui::Margin::same(14)).show(ui, |ui| {
        ui.set_width((MAX_W - 20.0).min(ui.available_width()));
        ui.add(egui::Label::new(sb(&q.question, 15.0).color(TEXT)).wrap());
        ui.label(rt(if q.finalized { "Final results" } else if q.multiselect { "Select one or more answers" } else { "Select one answer" }, 11.5).color(MUTED));
        ui.add_space(6.0);
        for a in &q.answers {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
            ui.painter().rect_filled(rect, 10.0, k_raised());
            if total > 0 && a.votes > 0 {
                let mut bar = rect;
                bar.set_width(rect.width() * a.votes as f32 / total as f32);
                ui.painter().rect_filled(bar, 10.0, if a.mine { pal.accent.gamma_multiply(0.45) } else { k_active() });
            }
            if a.mine {
                ui.painter().rect_stroke(rect, 6.0, Stroke::new(1.5, pal.accent), egui::StrokeKind::Inside);
            }
            let label = match &a.emoji {
                Some(e) if !e.starts_with(':') => format!("{e} {}", a.text),
                _ => a.text.clone(),
            };
            ui.painter().text(pos2(rect.left() + 12.0, rect.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::new(13.5, egui::FontFamily::Proportional), TEXT);
            let pct = if total > 0 { a.votes * 100 / total } else { 0 };
            ui.painter().text(pos2(rect.right() - 12.0, rect.center().y), egui::Align2::RIGHT_CENTER, format!("{} · {pct}%", a.votes), egui::FontId::new(12.0, egui::FontFamily::Proportional), SECONDARY);
            if !q.finalized && resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                let mut picked: Vec<u64> = if q.multiselect { q.answers.iter().filter(|x| x.mine).map(|x| x.id).collect() } else { Vec::new() };
                if a.mine {
                    picked.retain(|x| *x != a.id);
                } else {
                    picked.push(a.id);
                }
                acts.push(Action::Vote { item: m.item.clone(), answers: picked });
            }
            ui.add_space(3.0);
        }
        ui.horizontal(|ui| {
            ui.label(rt(format!("{total} vote{}", if total == 1 { "" } else { "s" }), 11.5).color(MUTED));
            // its author may close it early (asks once more: it cannot be reopened)
            if q.can_end && !q.finalized && super::sure(ui, ("end_poll", &m.item), "End poll", |ui, l| ui.link(rt(l, 11.5)).clicked()) {
                acts.push(Action::AskEndPoll(m.item.clone()));
                acts.push(Action::EndPoll);
            }
        });
    });
}

/// The full-size picture over the page: click outside or Esc closes, arrow keys step through the message's pictures.
pub fn viewer(ctx: &egui::Context, s: &AppState, tex: &mut HashMap<String, TextureHandle>, acts: &mut Vec<Action>) {
    let Some(v) = &s.viewer else { return };
    let msg = s.message(&v.item);
    let att = msg.and_then(|m| m.attachments.get(v.index));
    let screen = ctx.content_rect();
    let modal = egui::Modal::new(egui::Id::new("image_viewer")).show(ctx, |ui| {
        match v.pictures.get(&v.index) {
            Some(Some(Ok(pic))) => {
                // small pictures are enlarged to the same box as big ones
                let (w, h) = (pic.width.max(1) as f32, pic.height.max(1) as f32);
                let k = (screen.width() * 0.8 / w).min(screen.height() * 0.78 / h);
                let size = vec2(w * k, h * k);
                let id = pic_tex(ctx, tex, format!("view:{}:{}", v.item, v.index), pic);
                let resp = ui.add(egui::Image::new(egui::load::SizedTexture::new(id, size)).corner_radius(10.0).sense(Sense::click()));
                if let (Some(m), Some(a)) = (msg, att) {
                    attachment_menu(&resp, ctx, m, v.index, a, acts);
                }
            }
            Some(Some(Err(why))) => {
                ui.label(rt(*why, 14.0).color(DANGER));
            }
            _ => {
                ui.add(egui::Spinner::new());
            }
        }
    });
    if ctx.input(|i| i.key_pressed(egui::Key::ArrowLeft)) {
        acts.push(Action::StepImage(-1));
    }
    if ctx.input(|i| i.key_pressed(egui::Key::ArrowRight)) {
        acts.push(Action::StepImage(1));
    }
    if modal.should_close() {
        acts.push(Action::CloseImage);
    }
}

/// One playing animation: its frames decoded one at a time as they show, on the UI thread, only while drawn.
struct Player {
    frames: Box<dyn Iterator<Item = Vec<u8>>>,
    frame: usize,
    next_at: f64,
    tex: TextureHandle,
    last_seen: f64,
}

thread_local! {
    static PLAYERS: std::cell::RefCell<HashMap<String, Player>> = std::cell::RefCell::new(HashMap::new());
}

/// The texture of an animated picture at this moment (GIF, APNG, animated WebP), moving on by itself; none for a
/// still or when the file cannot be read, and the caller shows the first frame then.
pub fn animated(ctx: &egui::Context, key: &str, pic: &Picture) -> Option<egui::TextureId> {
    let anim = pic.animation.as_ref()?;
    if anim.delays.len() < 2 || anim.delays.len() > 900 {
        return None;
    }
    let now = ctx.input(|i| i.time);
    PLAYERS.with(|cell| {
        let mut players = cell.borrow_mut();
        // players not drawn for a few seconds let go of their files
        players.retain(|_, p| now - p.last_seen < 3.0);
        if !players.contains_key(key) {
            let mut frames = pic.frames();
            let first = frames.next()?;
            let img = egui::ColorImage::from_rgba_unmultiplied([pic.width as usize, pic.height as usize], &first);
            let tex = ctx.load_texture(format!("anim:{key}"), img, egui::TextureOptions::LINEAR);
            players.insert(key.to_owned(), Player { frames, frame: 0, next_at: now + anim.delays[0].max(20) as f64 / 1000.0, tex, last_seen: now });
        }
        let p = players.get_mut(key)?;
        p.last_seen = now;
        if now >= p.next_at {
            let next = match p.frames.next() {
                Some(f) => Some((f, p.frame + 1)),
                None => {
                    // the loop ends: start over
                    p.frames = pic.frames();
                    p.frames.next().map(|f| (f, 0))
                }
            };
            if let Some((f, idx)) = next {
                let img = egui::ColorImage::from_rgba_unmultiplied([pic.width as usize, pic.height as usize], &f);
                p.tex.set(img, egui::TextureOptions::LINEAR);
                p.frame = idx;
            }
            let delay = anim.delays.get(p.frame).copied().unwrap_or(100).max(20) as f64 / 1000.0;
            p.next_at = now + delay;
        }
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((p.next_at - now).max(0.0)));
        Some(p.tex.id())
    })
}
