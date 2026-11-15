//! The quick switcher (Ctrl+K): type a name, pick a conversation, channel or server with the arrow keys and Enter.
//! The engine searches and ranks (`switcher_places`, most used first); this draws the list and keeps the row.

use super::voice::{k_edge, k_panel};
use super::*;
use mytune_chat::yapfast_core::application::Place;

/// Ctrl+K opens and closes it; while it is open, the arrows, Enter and Esc are its own.
#[allow(clippy::too_many_arguments)]
pub fn show(ctx: &egui::Context, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::K)) {
        acts.push(Action::ToggleSwitcher);
        ctx.data_mut(|d| d.insert_temp(egui::Id::new("switch_index"), 0usize));
        return;
    }
    let Some(query) = &s.switcher else { return };
    // the nine drawn: arrows and Enter must not reach a place that is not shown
    let places: Vec<Place> = s.switcher_places().into_iter().take(9).collect();
    let idx_id = egui::Id::new("switch_index");
    let mut index: usize = ctx.data(|d| d.get_temp(idx_id)).unwrap_or(0).min(places.len().saturating_sub(1));
    let key = |k| ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k));
    if key(egui::Key::ArrowDown) && !places.is_empty() {
        index = (index + 1) % places.len();
    }
    if key(egui::Key::ArrowUp) && !places.is_empty() {
        index = (index + places.len() - 1) % places.len();
    }
    let mut go: Option<String> = None;
    if key(egui::Key::Enter) {
        go = places.get(index).map(|p| match p {
            Place::Room(r) => r.id.clone(),
            Place::Server(sv) => sv.id.clone(),
        });
    }
    let mut q = query.clone();
    let modal = egui::Modal::new(egui::Id::new("quick_switcher")).frame(egui::Frame::new().fill(crate::theme::Glass::Float.fill()).shadow(crate::theme::Glass::Float.shadow()).corner_radius(crate::theme::R_MENU).inner_margin(egui::Margin::same(12))).show(ctx, |ui| {
        ui.set_width(520.0);
        let te = egui::TextEdit::singleline(&mut q).id(egui::Id::new("switcher_edit")).desired_width(f32::INFINITY).font(egui::FontId::proportional(16.0)).hint_text(rt("Where would you like to go?", 16.0).color(FAINT));
        let r = ui.add(te);
        r.request_focus();
        if r.changed() {
            ctx.data_mut(|d| d.insert_temp(idx_id, 0usize));
            index = 0;
        }
        ui.add_space(8.0);
        if places.is_empty() {
            ui.label(rt("Nothing matches that.", 13.5).color(MUTED));
        }
        for (i, p) in places.iter().take(9).enumerate() {
            let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::click());
            if i == index || resp.hovered() {
                ui.painter().rect_filled(rect, 12.0, k_active());
            }
            let (name, kind, pic, id) = match p {
                Place::Room(r) => (r.name.clone(), if r.direct { "Direct message".to_owned() } else if r.voice { "Voice channel".to_owned() } else { "Channel".to_owned() }, r.avatar.clone(), r.id.clone()),
                Place::Server(sv) => (sv.name.clone(), "Server".to_owned(), sv.icon.clone(), sv.id.clone()),
            };
            let t = texture(ctx, tex, asked, s, &pic, want);
            avatar(ui, egui::Rect::from_center_size(egui::pos2(rect.left() + 24.0, rect.center().y), vec2(28.0, 28.0)), t, &name);
            ui.painter().text(egui::pos2(rect.left() + 48.0, rect.center().y), egui::Align2::LEFT_CENTER, tidy(&name), egui::FontId::new(14.5, th::semibold()), TEXT);
            ui.painter().text(egui::pos2(rect.right() - 12.0, rect.center().y), egui::Align2::RIGHT_CENTER, kind, egui::FontId::proportional(12.0), MUTED);
            if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                go = Some(id);
            }
        }
        ui.add_space(6.0);
        ui.label(rt("Arrow keys to move, Enter to go, Esc to close. Start with @ for people, # for channels, ! for voice.", 11.5).color(MUTED));
    });
    ctx.data_mut(|d| d.insert_temp(idx_id, index));
    if q != *query {
        acts.push(Action::SearchSwitcher(q));
    }
    if let Some(id) = go {
        acts.push(Action::SwitchTo(id));
    } else if modal.should_close() {
        acts.push(Action::CloseSwitcher);
    }
}
