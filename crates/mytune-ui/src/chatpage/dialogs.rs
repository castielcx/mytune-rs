//! The server dialogs the right-click menus open: Invite People, Leave Server (and group DM), the channel dialog
//! (create, duplicate, delete a channel or category) and Join a Server. The engine holds each one's state
//! (`AppState::invite`, `leave`, `channel_dialog`, `join`); these draw it and send its actions.

use super::*;
use mytune_chat::yapfast_core::application::{ChannelAsk, INVITE_AGES, INVITE_USES};
use mytune_chat::yapfast_core::chat::{ChannelKind, ChannelSettings};

/// What the dialogs keep between frames: the channel dialog's fields and the join field.
#[derive(Default)]
pub struct DialogsUi {
    name: String,
    topic: String,
    voice: bool,
    channel_open: bool,
    join_text: String,
    person_key: Option<(String, String)>,
    person_text: String,
    /// The server profile editor's texts (nickname, pronouns, about me) and whether they were filled from it.
    pe: [String; 3],
    pe_open: bool,
}

/// This account's profile in one server (the server menu's Edit Server Profile), over the chat.
pub fn server_profile(ctx: &egui::Context, d: &mut DialogsUi, s: &AppState, acts: &mut Vec<Action>) {
    use mytune_chat::yapfast_core::application::ProfileText;
    let Some(ed) = s.profile_editor.as_ref().filter(|e| e.server.is_some()) else {
        d.pe_open = false;
        return;
    };
    egui::Modal::new(egui::Id::new("server_profile")).show(ctx, |ui| {
        ui.set_width(380.0);
        ui.label(sb(format!("Edit server profile \u{2014} {}", tidy(&ed.server_name)), 16.0).color(TEXT));
        ui.add_space(8.0);
        if ed.loading() {
            ui.label(rt("Loading your profile\u{2026}", 13.0).color(MUTED));
        } else {
            if !d.pe_open {
                d.pe_open = true;
                d.pe = [ed.name.clone(), ed.pronouns.clone(), ed.bio.clone()];
            }
            for (i, (which, label, multi)) in [(ProfileText::Name, "Server nickname", false), (ProfileText::Pronouns, "Pronouns", false), (ProfileText::Bio, "About me (needs Nitro here)", true)].into_iter().enumerate() {
                ui.label(rt(label, 12.5).color(SECONDARY));
                ui.add_enabled_ui(ed.may_write(which), |ui| {
                    let te = if multi { egui::TextEdit::multiline(&mut d.pe[i]).desired_rows(3) } else { egui::TextEdit::singleline(&mut d.pe[i]) };
                    if ui.add(te.desired_width(360.0)).changed() {
                        acts.push(Action::WriteProfile { field: which, text: d.pe[i].clone() });
                    }
                });
                ui.add_space(6.0);
            }
            if !ed.may_rename {
                ui.label(rt("You cannot change your nickname in this server.", 12.0).color(MUTED));
            }
            if let Some(e) = &ed.error {
                ui.label(rt(e, 12.5).color(DANGER));
            }
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.add_enabled_ui(!ed.loading() && !ed.saving, |ui| {
                if pill(ui, if ed.saving { "Saving\u{2026}" } else { "Save" }, true).clicked() {
                    acts.push(Action::SaveProfile);
                }
            });
            if pill(ui, "Cancel", false).clicked() {
                acts.push(Action::CloseProfileEditor);
            }
        });
    });
}

fn danger_button(ui: &mut Ui, text: &str) -> bool {
    ui.add(egui::Button::new(sb(text, 13.0).color(Color32::WHITE)).fill(DANGER).corner_radius(8.0).min_size(vec2(100.0, 34.0))).clicked()
}

fn plain_button(ui: &mut Ui, text: &str) -> bool {
    ui.add(egui::Button::new(sb(text, 13.0).color(TEXT)).fill(voice::k_raised()).corner_radius(8.0).min_size(vec2(90.0, 34.0))).clicked()
}

fn age_text(s: u32) -> String {
    match s {
        0 => "Never".into(),
        s if s < 3600 => format!("{} minutes", s / 60),
        s if s < 86_400 => format!("{} hour{}", s / 3600, if s == 3600 { "" } else { "s" }),
        s => format!("{} day{}", s / 86_400, if s == 86_400 { "" } else { "s" }),
    }
}

fn uses_text(n: u32) -> String {
    if n == 0 { "No limit".into() } else { format!("{n} use{}", if n == 1 { "" } else { "s" }) }
}

/// A small pick list as a row of choices; returns the picked value.
fn choices<T: Copy + PartialEq>(ui: &mut Ui, label: &str, now: T, all: &[T], text: impl Fn(T) -> String) -> Option<T> {
    let mut picked = None;
    ui.label(rt(label, 12.0).color(MUTED));
    ui.horizontal_wrapped(|ui| {
        for &v in all {
            let on = v == now;
            let b = egui::Button::new(rt(text(v), 12.5).color(if on { Color32::WHITE } else { SECONDARY })).fill(if on { p().accent } else { voice::k_raised() }).corner_radius(6.0);
            if ui.add(b).clicked() && !on {
                picked = Some(v);
            }
        }
    });
    picked
}

pub fn invite(ctx: &egui::Context, s: &AppState, acts: &mut Vec<Action>) {
    let Some(d) = &s.invite else { return };
    let mut open = true;
    egui::Window::new("Invite People").id(egui::Id::new("invite_dlg")).open(&mut open).collapsible(false).resizable(false).default_width(420.0).show(ctx, |ui| {
        ui.set_width(400.0);
        ui.label(rt(format!("Invite people to {}", d.server), 14.0).color(TEXT));
        ui.label(rt(format!("They land in {}{}", if d.voice { "" } else { "#" }, tidy(&d.channel)), 12.5).color(MUTED));
        ui.add_space(8.0);
        match &d.link {
            Some(link) => {
                egui::Frame::new().fill(k_field()).corner_radius(8.0).inner_margin(egui::Margin::symmetric(10, 8)).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(rt(link, 13.5).color(TEXT));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if pill(ui, "Copy", true).clicked() {
                                ui.ctx().copy_text(link.clone());
                            }
                        });
                    });
                });
            }
            None => {
                ui.label(rt(if d.error.is_some() { "No link yet." } else { "Making a link\u{2026}" }, 13.0).color(MUTED));
            }
        }
        if let Some(e) = &d.error {
            ui.label(rt(e, 12.5).color(DANGER));
        }
        ui.add_space(8.0);
        let new_age = choices(ui, "Expire after", d.max_age, &INVITE_AGES, age_text);
        let new_uses = choices(ui, "Max number of uses", d.max_uses, &INVITE_USES, uses_text);
        if new_age.is_some() || new_uses.is_some() {
            acts.push(Action::MakeInvite { max_age: new_age.unwrap_or(d.max_age), max_uses: new_uses.unwrap_or(d.max_uses) });
        }
        if d.making {
            ui.add_space(4.0);
            ui.label(rt("Making a new link\u{2026}", 12.0).color(MUTED));
        }
    });
    if !open {
        acts.push(Action::CloseInvite);
    }
}

pub fn leave(ctx: &egui::Context, s: &AppState, acts: &mut Vec<Action>) {
    let Some(d) = &s.leave else { return };
    egui::Modal::new(egui::Id::new("leave_dlg")).show(ctx, |ui| {
        ui.set_width(400.0);
        ui.label(sb(if d.group { format!("Leave {}", d.name) } else { format!("Leave '{}'", d.name) }, 17.0).color(TEXT));
        ui.add_space(4.0);
        ui.label(rt(if d.group { "You will stop getting messages from this group." } else { "Are you sure you want to leave? You won't be able to rejoin unless you are re-invited." }, 13.5).color(SECONDARY));
        if d.group {
            ui.add_space(6.0);
            let mut silent = d.silent;
            if ui.checkbox(&mut silent, rt("Leave without telling the other members", 13.0)).changed() {
                acts.push(Action::ToggleSilentLeave);
            }
        }
        if let Some(e) = &d.error {
            ui.add_space(4.0);
            ui.label(rt(e, 12.5).color(DANGER));
        }
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_enabled_ui(!d.leaving, |ui| {
                if danger_button(ui, if d.group { "Leave Group" } else { "Leave Server" }) {
                    acts.push(Action::Leave);
                }
            });
            if plain_button(ui, "Cancel") {
                acts.push(Action::CloseLeave);
            }
        });
    });
}

pub fn channel(ctx: &egui::Context, du: &mut DialogsUi, s: &AppState, acts: &mut Vec<Action>) {
    let Some(d) = &s.channel_dialog else {
        du.channel_open = false;
        return;
    };
    // fields start from the channel's, once, as the dialog opens
    if !du.channel_open {
        du.channel_open = true;
        let st = d.settings.clone().unwrap_or_default();
        du.name = st.name;
        du.topic = st.topic;
        du.voice = d.kind == ChannelKind::Voice;
    }
    let category = d.kind == ChannelKind::Category;
    let thing = if category { "category" } else { "channel" };
    let (title, detail, confirm, deleting) = match &d.ask {
        ChannelAsk::Create { .. } => (format!("Create {thing}"), if d.name.is_empty() { format!("In {}", d.server_name) } else { format!("In {}", d.name) }, format!("Create {thing}"), false),
        ChannelAsk::Duplicate(_) => ("Duplicate channel".to_owned(), format!("A copy of #{}, with its settings and permissions, in the same category.", d.name), "Create channel".to_owned(), false),
        ChannelAsk::Delete(_) => (format!("Delete {thing}"), format!("Are you sure you want to delete {}{}? This cannot be undone.", if d.kind == ChannelKind::Text { "#" } else { "" }, d.name), format!("Delete {thing}"), true),
    };
    egui::Modal::new(egui::Id::new("channel_dlg")).show(ctx, |ui| {
        ui.set_width(420.0);
        ui.label(sb(title, 17.0).color(TEXT));
        ui.add_space(4.0);
        ui.label(rt(detail, 13.5).color(SECONDARY));
        if !deleting {
            ui.add_space(10.0);
            if matches!(d.ask, ChannelAsk::Create { .. }) && !category {
                ui.horizontal(|ui| {
                    for (v, t) in [(false, "Text"), (true, "Voice")] {
                        let on = du.voice == v;
                        if ui.add(egui::Button::new(rt(t, 13.0).color(if on { Color32::WHITE } else { SECONDARY })).fill(if on { p().accent } else { voice::k_raised() }).corner_radius(6.0)).clicked() {
                            du.voice = v;
                        }
                    }
                });
                ui.add_space(6.0);
            }
            ui.label(rt(if category { "CATEGORY NAME" } else { "CHANNEL NAME" }, 11.0).color(MUTED));
            ui.add(egui::TextEdit::singleline(&mut du.name).desired_width(f32::INFINITY));
        }
        if let Some(e) = &d.error {
            ui.add_space(4.0);
            ui.label(rt(e, 12.5).color(DANGER));
        }
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let go = ui.add_enabled_ui(!d.saving, |ui| if deleting { danger_button(ui, &confirm) } else { pill(ui, &confirm, true).clicked() }).inner;
            if go {
                let kind = if category { ChannelKind::Category } else if du.voice { ChannelKind::Voice } else { ChannelKind::Text };
                acts.push(Action::SaveChannel { kind, settings: ChannelSettings { name: du.name.clone(), topic: du.topic.clone(), ..d.settings.clone().unwrap_or_default() } });
            }
            if plain_button(ui, "Cancel") {
                acts.push(Action::CloseChannelDialog);
            }
        });
    });
}

pub fn join(ctx: &egui::Context, du: &mut DialogsUi, s: &AppState, acts: &mut Vec<Action>) {
    let Some(d) = &s.join else {
        du.join_text.clear();
        return;
    };
    let mut open = true;
    egui::Window::new("Join a Server").id(egui::Id::new("join_dlg")).open(&mut open).collapsible(false).resizable(false).default_width(420.0).show(ctx, |ui| {
        ui.set_width(400.0);
        if !d.from_link {
            ui.label(rt("Enter an invite link to join a server.", 13.5).color(SECONDARY));
            ui.add_space(6.0);
            let te = ui.add(egui::TextEdit::singleline(&mut du.join_text).desired_width(f32::INFINITY).hint_text("https://discord.gg/yapfast"));
            if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) || te.changed() && du.join_text.contains("discord") && du.join_text.len() > 14 {
                acts.push(Action::FindInvite(du.join_text.clone()));
            }
        }
        if d.resolving {
            ui.label(rt("Looking up the invite\u{2026}", 13.0).color(MUTED));
        }
        if let Some(pv) = &d.preview {
            ui.add_space(8.0);
            egui::Frame::new().fill(k_field()).corner_radius(8.0).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                ui.label(sb(&pv.name, 15.0).color(TEXT));
                let mut line = String::new();
                if let Some(o) = pv.online {
                    line.push_str(&format!("{o} online  "));
                }
                if let Some(m) = pv.members {
                    line.push_str(&format!("{m} members"));
                }
                if !line.is_empty() {
                    ui.label(rt(line, 12.5).color(MUTED));
                }
            });
        }
        if let Some(e) = &d.error {
            ui.add_space(4.0);
            ui.label(rt(e, 12.5).color(DANGER));
        }
        if let Some(url) = &d.captcha {
            ui.add_space(4.0);
            ui.label(rt("Discord asks for a captcha first. Finish joining in the browser.", 12.5).color(th::WARNING));
            ui.label(rt(url, 11.5).color(MUTED));
        }
        if d.form.is_some() {
            ui.add_space(4.0);
            if pill(ui, "Finish the server's form in the browser", false).clicked() {
                acts.push(Action::OpenJoinForm);
            }
        }
        ui.add_space(10.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_enabled_ui(d.preview.is_some() && !d.joining, |ui| {
                if pill(ui, if d.member { "Open Server" } else { "Join Server" }, true).clicked() {
                    acts.push(Action::JoinServer);
                }
            });
        });
    });
    if !open {
        acts.push(Action::CloseJoin);
    }
}

fn span_text(s: u32) -> String {
    match s {
        0 => "Don't delete any".into(),
        s if s < 3600 => format!("{} seconds", s),
        s if s < 86_400 => format!("{} hour{}", s / 3600, if s == 3600 { "" } else { "s" }),
        s => format!("{} day{}", s / 86_400, if s == 86_400 { "" } else { "s" }),
    }
}

/// The dialog about a person: note, nickname, unfriend, block; and a server member's nickname, timeout,
/// kick and ban, with a reason.
pub fn person(ctx: &egui::Context, du: &mut DialogsUi, s: &AppState, acts: &mut Vec<Action>) {
    use mytune_chat::yapfast_core::application::{PersonAsk, BAN_HISTORY, TIMEOUT_TIMES};
    let Some(d) = &s.person_dialog else {
        du.person_key = None;
        return;
    };
    let key = (d.user.clone(), format!("{:?}", d.ask));
    if du.person_key.as_ref() != Some(&key) {
        du.person_key = Some(key);
        du.person_text = d.text.clone().unwrap_or_default();
    }
    if du.person_text.is_empty() {
        if let Some(t) = &d.text {
            if du.person_key.is_some() && !t.is_empty() {
                du.person_text = t.clone();
            }
        }
    }
    let (title, hint, confirm, danger) = match d.ask {
        PersonAsk::Note => (format!("Note for {}", d.name), "Click to add a note", "Done", false),
        PersonAsk::Nickname => (format!("Nickname for {}", d.name), "Nickname", "Save", false),
        PersonAsk::ServerNickname => (format!("Change nickname of {}", d.name), "Server nickname", "Save", false),
        PersonAsk::Unfriend => (format!("Remove '{}'", d.name), "", "Remove Friend", true),
        PersonAsk::Block => (format!("Block '{}'", d.name), "", "Block", true),
        PersonAsk::Timeout => (format!("Timeout {}", d.name), "Reason (optional)", "Timeout", false),
        PersonAsk::Kick => (format!("Kick {}", d.name), "Reason (optional)", "Kick", true),
        PersonAsk::Ban => (format!("Ban {}", d.name), "Reason (optional)", "Ban", true),
    };
    egui::Modal::new(egui::Id::new("person_dlg")).show(ctx, |ui| {
        ui.set_width(420.0);
        ui.label(sb(title, 17.0).color(TEXT));
        ui.add_space(8.0);
        if d.ask == PersonAsk::Timeout {
            let c = choices(ui, "Duration", d.choice, &(0..TIMEOUT_TIMES.len()).collect::<Vec<_>>(), |i| {
                let s = TIMEOUT_TIMES[i];
                if s < 3600 { format!("{} min", s / 60) } else if s < 86_400 { format!("{} hour", s / 3600) } else { format!("{} day", s / 86_400) }
            });
            if let Some(i) = c {
                acts.push(Action::ChoosePersonOption(i));
            }
            ui.add_space(6.0);
        }
        if d.ask == PersonAsk::Ban {
            let c = choices(ui, "Delete message history", d.choice, &(0..BAN_HISTORY.len()).collect::<Vec<_>>(), |i| span_text(BAN_HISTORY[i]));
            if let Some(i) = c {
                acts.push(Action::ChoosePersonOption(i));
            }
            ui.add_space(6.0);
        }
        if !hint.is_empty() {
            ui.add(egui::TextEdit::singleline(&mut du.person_text).desired_width(f32::INFINITY).hint_text(hint));
        }
        if let Some(e) = &d.error {
            ui.add_space(4.0);
            ui.label(rt(e, 12.5).color(DANGER));
        }
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let go = ui.add_enabled_ui(!d.saving, |ui| if danger { danger_button(ui, confirm) } else { pill(ui, confirm, true).clicked() }).inner;
            if go {
                acts.push(Action::AnswerPerson(du.person_text.clone()));
            }
            if plain_button(ui, "Cancel") {
                acts.push(Action::ClosePersonDialog);
            }
        });
    });
}
