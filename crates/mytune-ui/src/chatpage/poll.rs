//! The composer's + menu (Upload a File, Create Poll) and Discord's "Create a Poll" dialog, driven by the
//! engine's poll draft. Use Apps, Schedule Message and Record Voice Message have no engine support.

use super::voice::{glyph, k_edge, k_panel, QUICK, k_raised};
use super::*;
use egui::{pos2, Stroke};
use mytune_chat::yapfast_core::application::{Application, PollEdit, ANSWER_LIMIT, POLL_ANSWERS, POLL_HOURS, QUESTION_LIMIT};

#[derive(Default)]
pub struct PlusUi {
    pub open: bool,
    /// Opened this frame: its own click is not a click elsewhere.
    fresh: bool,
    question: String,
    answers: HashMap<u32, String>,
    poll_open: bool,
}

impl PlusUi {
    pub fn toggle(&mut self) {
        self.open = !self.open;
        self.fresh = self.open;
    }
}

/// Opens the system's file dialog on a thread of its own (on the UI thread it froze the window), then stages
/// the files picked to go with the next message.
pub fn upload(chat: &Chat) {
    let chat = chat.clone();
    std::thread::spawn(move || {
        if let Some(files) = rfd::FileDialog::new().set_title("Upload to Discord").pick_files() {
            chat.dispatch(Action::Stage(files));
        }
    });
}

/// The + menu over the composer's left edge.
pub fn menu(ctx: &egui::Context, pu: &mut PlusUi, app: &Application, chat: &Chat, composer: egui::Rect, acts: &mut Vec<Action>) {
    if !pu.open {
        return;
    }
    let polls = app.can_create_polls();
    let h = if polls { 2.0 * 38.0 + 12.0 } else { 38.0 + 12.0 };
    let pos = pos2(composer.left(), composer.top() - h - 8.0);
    let mut close = false;
    let area = egui::Area::new(egui::Id::new("plus_menu")).order(egui::Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        egui::Frame::new().fill(k_panel()).stroke(Stroke::new(1.0, k_edge())).corner_radius(8.0).inner_margin(egui::Margin::same(6)).shadow(egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(0x66) }).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            let mut item = |ui: &mut Ui, g: &str, text: &str| {
                let (r, resp) = ui.allocate_exact_size(vec2(220.0, 38.0), Sense::click());
                let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), QUICK);
                if t > 0.0 {
                    ui.painter().rect_filled(r, 4.0, p().accent.gamma_multiply(0.25 * t));
                }
                glyph(ui.painter(), pos2(r.left() + 18.0, r.center().y), g, 16.0, SECONDARY.lerp_to_gamma(TEXT, t), false);
                ui.painter().text(pos2(r.left() + 42.0, r.center().y), egui::Align2::LEFT_CENTER, text, egui::FontId::new(14.0, th::semibold()), SECONDARY.lerp_to_gamma(TEXT, t));
                resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
            };
            if item(ui, "\u{E898}", "Upload a File") {
                upload(chat);
                close = true;
            }
            if polls && item(ui, "\u{E9D9}", "Create Poll") {
                acts.push(Action::OpenPollDraft);
                close = true;
            }
        });
    });
    let fresh = std::mem::take(&mut pu.fresh);
    if close || (!fresh && area.response.clicked_elsewhere()) || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        pu.open = false;
    }
}

/// "Create a Poll": the question, up to ten answers, how long it runs, multiple answers, Post.
pub fn dialog(ctx: &egui::Context, pu: &mut PlusUi, s: &AppState, acts: &mut Vec<Action>) {
    let Some(d) = &s.poll_draft else {
        pu.poll_open = false;
        return;
    };
    if !pu.poll_open {
        pu.poll_open = true;
        pu.question = d.question.clone();
        pu.answers = d.answers.iter().map(|a| (a.id, a.text.clone())).collect();
    }
    let mut open = true;
    egui::Window::new("Create a Poll").id(egui::Id::new("poll_draft")).open(&mut open).collapsible(false).resizable(false).default_width(460.0).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.label(rt("QUESTION", 11.0).color(MUTED));
        if ui.add(egui::TextEdit::singleline(&mut pu.question).char_limit(QUESTION_LIMIT).desired_width(f32::INFINITY).hint_text("What question do you want to ask?")).changed() {
            acts.push(Action::EditPoll(PollEdit::Question(pu.question.clone())));
        }
        ui.add_space(10.0);
        ui.label(rt("ANSWERS", 11.0).color(MUTED));
        let many = d.answers.len() > 2;
        for (index, a) in d.answers.iter().enumerate() {
            ui.horizontal(|ui| {
                let text = pu.answers.entry(a.id).or_insert_with(|| a.text.clone());
                let w = ui.available_width() - if many { 34.0 } else { 0.0 };
                if ui.add(egui::TextEdit::singleline(text).char_limit(ANSWER_LIMIT).desired_width(w).hint_text("Type your answer")).changed() {
                    acts.push(Action::EditPoll(PollEdit::Answer { index, text: text.clone() }));
                }
                if many && ui.add(egui::Button::new(rt("\u{E74D}", 13.0).family(th::icons()).color(MUTED)).frame(false)).on_hover_text("Remove").clicked() {
                    acts.push(Action::EditPoll(PollEdit::RemoveAnswer(index)));
                }
            });
            ui.add_space(4.0);
        }
        if d.answers.len() < POLL_ANSWERS && ui.add(egui::Button::new(sb("+ Add another answer", 13.0).color(TEXT)).fill(k_raised()).corner_radius(6.0)).clicked() {
            acts.push(Action::EditPoll(PollEdit::AddAnswer));
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(rt("Duration", 13.0).color(SECONDARY));
            let label = |h: u32| if h < 24 { format!("{h} hour{}", if h == 1 { "" } else { "s" }) } else { format!("{} day{}", h / 24, if h == 24 { "" } else { "s" }) };
            egui::ComboBox::from_id_salt("poll_duration").selected_text(label(POLL_HOURS[d.duration.min(POLL_HOURS.len() - 1)])).show_ui(ui, |ui| {
                for (i, h) in POLL_HOURS.iter().enumerate() {
                    if ui.selectable_label(i == d.duration, label(*h)).clicked() {
                        acts.push(Action::EditPoll(PollEdit::Duration(i)));
                    }
                }
            });
            let mut multi = d.multiselect;
            if ui.checkbox(&mut multi, rt("Allow multiple answers", 13.0)).changed() {
                acts.push(Action::EditPoll(PollEdit::Multiselect(multi)));
            }
        });
        if let Some(e) = &d.error {
            ui.label(rt(e, 12.5).color(DANGER));
        }
        ui.add_space(10.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_enabled_ui(!d.posting, |ui| {
                if pill(ui, if d.posting { "Posting\u{2026}" } else { "Post" }, true).clicked() {
                    acts.push(Action::PostPoll);
                }
            });
        });
    });
    if !open {
        acts.push(Action::ClosePollDraft);
    }
}
