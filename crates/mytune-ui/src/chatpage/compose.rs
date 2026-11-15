//! The composer's extras: the "X is typing..." line, the typing signal and draft saving (the engine does both once it
//! gets the draft on every change), and @mention suggestions with the arrow keys, Tab/Enter to take one, Esc to
//! dismiss. The engine owns the suggestions (`mention_query`, `mention_suggestions`); the caret and the chosen row
//! live here.

use super::voice::{k_edge, k_raised};
use super::*;
use mytune_chat::yapfast_core::mention;

/// The id of the composer's text field, so its caret can be read and moved.
pub fn edit_id() -> egui::Id {
    egui::Id::new("composer_edit")
}

/// Asks for the keyboard focus in the composer. The field takes it itself after it is drawn:
/// a direct `request_focus` from a menu click is dropped when the field is drawn later in the
/// same frame, because egui takes the focus from a field on a click outside it.
pub fn focus(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(edit_id().with("focus"), true));
    ctx.request_repaint();
}

/// True once after `focus` was called.
pub fn take_focus(ctx: &egui::Context) -> bool {
    ctx.data_mut(|d| d.remove_temp::<bool>(edit_id().with("focus"))).is_some()
}

#[derive(Clone, Default)]
struct Mention {
    /// The `@` word last told to the engine.
    query: Option<String>,
    /// The chosen suggestion.
    index: usize,
    /// The caret as a byte offset into the draft.
    caret: usize,
}

fn state(ctx: &egui::Context) -> Mention {
    ctx.data(|d| d.get_temp::<Mention>(egui::Id::new("mention_state"))).unwrap_or_default()
}

fn store(ctx: &egui::Context, m: Mention) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("mention_state"), m));
}

/// The line under the messages: who is typing. Always the same height so the messages do not jump.
pub fn typing_line(ui: &mut Ui, s: &AppState) {
    let line = s.typing_line();
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
    if line.is_empty() {
        return;
    }
    // three dots that rise one after the other
    let t = ui.input(|i| i.time) as f32;
    for k in 0..3 {
        let a = 0.35 + 0.65 * (((t * 3.0 - k as f32 * 0.7).sin() + 1.0) / 2.0);
        ui.painter().circle_filled(egui::pos2(r.left() + 8.0 + k as f32 * 7.0, r.center().y), 2.2, SECONDARY.gamma_multiply(a));
    }
    ui.painter().text(egui::pos2(r.left() + 32.0, r.center().y), egui::Align2::LEFT_CENTER, line, egui::FontId::new(12.5, th::semibold()), SECONDARY);
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(60));
}

/// Before the text field: with suggestions showing and the field focused, the arrow keys, Tab, Enter and Esc
/// belong to the list. Returns true when a suggestion was taken.
pub fn before_edit(ctx: &egui::Context, s: &AppState, draft: &mut String, acts: &mut Vec<Action>) -> bool {
    let list = s.mention_suggestions();
    if list.is_empty() || !ctx.memory(|m| m.has_focus(edit_id())) {
        return false;
    }
    let mut st = state(ctx);
    st.index = st.index.min(list.len() - 1);
    let key = |k| ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, k));
    let mut took = false;
    if key(egui::Key::ArrowDown) {
        st.index = (st.index + 1) % list.len();
    }
    if key(egui::Key::ArrowUp) {
        st.index = (st.index + list.len() - 1) % list.len();
    }
    if key(egui::Key::Tab) || key(egui::Key::Enter) {
        let i = st.index;
        store(ctx, st.clone());
        accept(ctx, s, draft, i, acts);
        took = true;
        st = state(ctx);
    }
    if key(egui::Key::Escape) {
        acts.push(Action::MentionQuery(None));
        st.query = None;
    }
    store(ctx, st);
    took
}

/// After the text field: a changed draft goes to the engine (typing signal, saved draft), and the `@` word at the
/// caret is told to it when it changed.
pub fn after_edit(ctx: &egui::Context, out: &egui::text_edit::TextEditOutput, draft: &str, acts: &mut Vec<Action>) {
    let mut st = state(ctx);
    if out.response.changed() {
        acts.push(Action::SetDraft(draft.to_owned()));
    }
    if let Some(range) = out.cursor_range {
        st.caret = draft.char_indices().nth(range.primary.index.0).map_or(draft.len(), |(b, _)| b);
    }
    let q = if out.response.has_focus() { mention::query(draft, st.caret).map(|(_, q)| q.to_owned()) } else { st.query.clone().filter(|_| out.response.has_focus()) };
    if q != st.query {
        st.query = q.clone();
        st.index = 0;
        acts.push(Action::MentionQuery(q));
    }
    store(ctx, st);
}

/// Puts suggestion `index` into the draft in place of the `@` word and moves the caret after it.
fn accept(ctx: &egui::Context, s: &AppState, draft: &mut String, index: usize, acts: &mut Vec<Action>) {
    let Some(c) = s.mention_suggestions().into_iter().nth(index) else { return };
    let mut st = state(ctx);
    // the caret is last frame's: the draft may have been sent or replaced since
    if !draft.is_char_boundary(st.caret) {
        return;
    }
    let Some((at, _)) = mention::query(draft, st.caret) else { return };
    let insert = format!("@{} ", c.name);
    draft.replace_range(at..st.caret, &insert);
    let end = at + insert.len();
    let chars = draft[..end].chars().count();
    let mut ts = egui::TextEdit::load_state(ctx, edit_id()).unwrap_or_default();
    ts.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(chars))));
    ts.store(ctx, edit_id());
    st.caret = end;
    st.query = None;
    st.index = 0;
    store(ctx, st);
    acts.push(Action::MentionQuery(None));
    acts.push(Action::SetDraft(draft.clone()));
    focus(ctx);
}

/// The suggestion list above the composer: avatar, name and username, the chosen row lit; a click takes one.
#[allow(clippy::too_many_arguments)]
pub fn popup(ctx: &egui::Context, s: &AppState, composer: egui::Rect, draft: &mut String, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let list = s.mention_suggestions();
    if list.is_empty() || composer == egui::Rect::NOTHING {
        return;
    }
    let st = state(ctx);
    let (row_h, w) = (40.0, composer.width().min(420.0));
    let h = list.len() as f32 * row_h + 28.0;
    let mut take = None;
    egui::Area::new(egui::Id::new("mention_popup")).order(egui::Order::Foreground).fixed_pos(egui::pos2(composer.left(), composer.top() - h - 8.0)).show(ctx, |ui| {
        crate::theme::float(ui, crate::theme::R_MENU, 6, |ui| {
            ui.set_width(w);
            ui.label(sb("MEMBERS", 11.0).color(MUTED));
            ui.add_space(2.0);
            for (i, c) in list.iter().enumerate() {
                let (r, resp) = ui.allocate_exact_size(vec2(w, row_h - 4.0), Sense::click());
                if i == st.index.min(list.len() - 1) || resp.hovered() {
                    ui.painter().rect_filled(r, 10.0, k_active());
                }
                let t = texture(ctx, tex, asked, s, &c.avatar, want);
                avatar(ui, egui::Rect::from_center_size(egui::pos2(r.left() + 20.0, r.center().y), vec2(26.0, 26.0)), t, &c.name);
                let col = c.color.map(|c| Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8)).unwrap_or(TEXT);
                let g = ui.painter().layout_no_wrap(c.name.clone(), egui::FontId::new(13.5, th::semibold()), col);
                let nx = r.left() + 40.0;
                ui.painter().galley(egui::pos2(nx, r.center().y - g.size().y / 2.0), g.clone(), col);
                if let Some(u) = &c.username {
                    ui.painter().text(egui::pos2(nx + g.size().x + 8.0, r.center().y), egui::Align2::LEFT_CENTER, u, egui::FontId::proportional(12.0), MUTED);
                }
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    take = Some(i);
                }
            }
        });
    });
    if let Some(i) = take {
        accept(ctx, s, draft, i, acts);
    }
}

/// Files from the clipboard (a picture or copied files) waiting to be attached.
static PASTED: std::sync::Mutex<Vec<std::path::PathBuf>> = std::sync::Mutex::new(Vec::new());

/// Files dropped on the window, or pasted with Ctrl+V while the composer has focus (a picture on the clipboard
/// is saved as a PNG in the temp folder; copied files are attached as they are), go to the next message.
pub fn files(ctx: &egui::Context, acts: &mut Vec<Action>) {
    let dropped: Vec<std::path::PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_owned()).collect());
    if !dropped.is_empty() {
        acts.push(Action::Stage(dropped));
    }
    let paste = ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Paste(_))) || (i.modifiers.command && i.key_pressed(egui::Key::V)));
    if paste && ctx.memory(|m| m.has_focus(edit_id())) {
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            use std::os::windows::process::CommandExt;
            let script = "Add-Type -AssemblyName System.Windows.Forms; Add-Type -AssemblyName System.Drawing; \
                if ([System.Windows.Forms.Clipboard]::ContainsFileDropList()) { foreach ($f in [System.Windows.Forms.Clipboard]::GetFileDropList()) { Write-Output $f } } \
                elseif ([System.Windows.Forms.Clipboard]::ContainsImage()) { $p = Join-Path $env:TEMP ('mytune-paste-' + [guid]::NewGuid().ToString('N') + '.png'); \
                [System.Windows.Forms.Clipboard]::GetImage().Save($p, [System.Drawing.Imaging.ImageFormat]::Png); Write-Output $p }";
            let out = std::process::Command::new("powershell").args(["-NoProfile", "-STA", "-Command", script]).creation_flags(0x0800_0000).output();
            if let Ok(o) = out {
                let found: Vec<std::path::PathBuf> = String::from_utf8_lossy(&o.stdout).lines().map(str::trim).filter(|l| !l.is_empty()).map(std::path::PathBuf::from).filter(|p| p.is_file()).collect();
                if !found.is_empty() {
                    PASTED.lock().unwrap_or_else(|e| e.into_inner()).extend(found);
                    ctx.request_repaint();
                }
            }
        });
    }
    let got: Vec<std::path::PathBuf> = std::mem::take(&mut *PASTED.lock().unwrap_or_else(|e| e.into_inner()));
    if !got.is_empty() {
        acts.push(Action::Stage(got));
    }
}
