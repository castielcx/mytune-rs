//! The rest of a server's settings pages: Engagement (system messages, default notifications, inactive channel),
//! Boost Perks (level, progress bar, banner, invite background), Emoji, Stickers and Safety Setup, and the dialogs
//! of Delete Server and Transfer Ownership. `serversettings.rs` picks the page; the engine holds each page's draft.

use super::voice::k_raised;
use super::*;
use mytune_chat::yapfast_core::application::{
    Action, Application, BoostPage, EmojiInput, EmojiPage, EngagementPage, Ownership, OwnershipInput, SafetyChoice, SafetyPage, ServerSettings, SettingsChoice, SettingsToggle,
    StickerInput, StickersPage, AFK_TIMEOUTS, BOOSTED, SETUP_TIPS, WELCOME, WELCOME_STICKER,
};
use mytune_chat::yapfast_core::chat::ServerPicture;

fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(sb(text, 20.0).color(TEXT));
    ui.add_space(10.0);
}

fn small(ui: &mut Ui, text: &str) {
    ui.add_space(8.0);
    ui.label(rt(text.to_uppercase(), 11.0).color(MUTED));
}

fn button(ui: &mut Ui, text: &str, danger: bool) -> bool {
    let b = egui::Button::new(sb(text, 12.5).color(if danger { Color32::WHITE } else { TEXT })).fill(if danger { DANGER } else { k_raised() }).corner_radius(6.0).min_size(vec2(0.0, 28.0));
    ui.add(b).clicked()
}

fn secs(s: u32) -> String {
    if s >= 3600 {
        format!("{} hour{}", s / 3600, if s / 3600 == 1 { "" } else { "s" })
    } else {
        format!("{} minutes", s / 60)
    }
}

/// A pick list of channels: "none" or one of `list` (id, name).
fn channel_combo(ui: &mut Ui, id: &str, none: &str, current: &Option<String>, list: &[(String, String)], pick: impl Fn(Option<String>) -> Action, acts: &mut Vec<Action>) {
    let text = match current {
        Some(c) => list.iter().find(|(i, _)| i == c).map(|(_, n)| tidy(n)).unwrap_or_else(|| "(unknown)".to_owned()),
        None => none.to_owned(),
    };
    egui::ComboBox::from_id_salt(id).selected_text(text).width(260.0).show_ui(ui, |ui| {
        if ui.selectable_label(current.is_none(), none).clicked() {
            acts.push(pick(None));
        }
        for (cid, name) in list {
            if ui.selectable_label(current.as_deref() == Some(cid.as_str()), tidy(name)).clicked() {
                acts.push(pick(Some(cid.clone())));
            }
        }
    });
}

pub fn engagement(ui: &mut Ui, app: &Application, ss: &ServerSettings, p: &EngagementPage, acts: &mut Vec<Action>) {
    heading(ui, "Engagement");
    let text = app.settings_channels(&ss.server, false);
    small(ui, "System messages channel");
    channel_combo(ui, "eng_sys", "No System Messages", &p.system_channel, &text, |c| Action::ChooseServerSetting(SettingsChoice::SystemChannel(c)), acts);
    ui.add_enabled_ui(p.system_channel.is_some(), |ui| {
        for (bit, label) in [(WELCOME, "Send a random welcome message when someone joins"), (WELCOME_STICKER, "Prompt members to reply to welcome messages with a sticker"), (BOOSTED, "Send a message when someone boosts this server"), (SETUP_TIPS, "Send helpful tips for server setup")] {
            let mut on = p.sends(bit);
            if ui.checkbox(&mut on, rt(label, 13.5).color(TEXT)).changed() {
                acts.push(Action::ToggleServerSetting(SettingsToggle::SystemMessage(bit)));
            }
        }
    });
    small(ui, "Default notification settings");
    for (n, label) in [(0u8, "All messages"), (1u8, "Only @mentions")] {
        if ui.radio(p.default_notifications == n, rt(label, 13.5).color(TEXT)).clicked() {
            acts.push(Action::ChooseServerSetting(SettingsChoice::Notifications(n)));
        }
    }
    small(ui, "Inactive channel");
    let voice = app.settings_channels(&ss.server, true);
    channel_combo(ui, "eng_afk", "No Inactive Channel", &p.afk_channel, &voice, |c| Action::ChooseServerSetting(SettingsChoice::AfkChannel(c)), acts);
    small(ui, "Inactive timeout");
    egui::ComboBox::from_id_salt("eng_afk_t").selected_text(secs(p.afk_timeout)).show_ui(ui, |ui| {
        for t in AFK_TIMEOUTS {
            if ui.selectable_label(p.afk_timeout == t, secs(t)).clicked() {
                acts.push(Action::ChooseServerSetting(SettingsChoice::AfkTimeout(t)));
            }
        }
    });
}

pub fn boost(ui: &mut Ui, chat: &Chat, p: &BoostPage, acts: &mut Vec<Action>) {
    heading(ui, "Server Boost Perks");
    ui.label(rt(format!("Boost level {}", p.original.premium_tier), 14.0).color(TEXT));
    ui.label(rt("The banner and invite background unlock at higher boost levels.", 12.5).color(MUTED));
    let mut bar = p.progress_bar;
    if ui.checkbox(&mut bar, rt("Show the boost progress bar", 13.5).color(TEXT)).changed() {
        acts.push(Action::ToggleServerSetting(SettingsToggle::ProgressBar));
    }
    for (kind, name) in [(ServerPicture::Banner, "Server banner"), (ServerPicture::Splash, "Invite background")] {
        small(ui, name);
        if !p.unlocked(kind) {
            ui.label(rt("Not unlocked at this boost level.", 12.5).color(MUTED));
            continue;
        }
        let has = p.picture(kind).0.is_some() || p.picture(kind).1.is_some();
        ui.label(rt(if has { "A picture is set." } else { "No picture." }, 12.5).color(SECONDARY));
        ui.horizontal(|ui| {
            if button(ui, "Choose picture\u{2026}", false) {
                let chat = chat.clone();
                std::thread::spawn(move || {
                    if let Some(path) = rfd::FileDialog::new().set_title(name).add_filter("Pictures", &["png", "jpg", "jpeg", "gif", "webp"]).pick_file() {
                        chat.dispatch(Action::PickServerPicture { kind, path });
                    }
                });
            }
            if has && button(ui, "Remove", false) {
                acts.push(Action::RemoveServerPicture(kind));
            }
        });
    }
}

pub fn safety(ui: &mut Ui, app: &Application, ss: &ServerSettings, p: &SafetyPage, acts: &mut Vec<Action>) {
    heading(ui, "Safety Setup");
    small(ui, "Verification level");
    ui.label(rt("What members must have done before they can send messages or DM members.", 12.0).color(MUTED));
    for (n, label) in [(0u8, "None"), (1, "Low: verified email"), (2, "Medium: registered on Discord for 5 minutes"), (3, "High: a member of this server for 10 minutes"), (4, "Highest: a verified phone number")] {
        ui.add_enabled_ui(p.offers_verification(n), |ui| {
            if ui.radio(p.verification() == n, rt(label, 13.5).color(TEXT)).clicked() {
                acts.push(Action::ChooseServerSetting(SettingsChoice::Safety(SafetyChoice::Verification(n))));
            }
        });
    }
    small(ui, "Explicit media content filter");
    for (n, label) in [(0u8, "Don't scan any media content"), (1, "Scan media from members without roles"), (2, "Scan media from all members")] {
        ui.add_enabled_ui(p.offers_filter(n), |ui| {
            if ui.radio(p.content_filter() == n, rt(label, 13.5).color(TEXT)).clicked() {
                acts.push(Action::ChooseServerSetting(SettingsChoice::Safety(SafetyChoice::ContentFilter(n))));
            }
        });
    }
    small(ui, "Moderator two-factor authentication");
    let lock = p.mfa_lock(app.mfa_rule(&ss.server));
    ui.add_enabled_ui(lock.is_none(), |ui| {
        let mut on = p.requires_mfa();
        if ui.checkbox(&mut on, rt("Require 2FA for moderator actions", 13.5).color(TEXT)).changed() {
            acts.push(Action::ChooseServerSetting(SettingsChoice::Safety(SafetyChoice::RequireMfa(on))));
        }
    });
    if let Some(l) = lock {
        ui.label(rt(l, 12.0).color(th::WARNING));
    }
}

pub fn emoji(ui: &mut Ui, ctx: &egui::Context, chat: &Chat, s: &AppState, p: &EmojiPage, names: &mut std::collections::HashMap<String, String>, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    heading(ui, "Emoji");
    ui.horizontal(|ui| {
        ui.label(rt(format!("{} of {} static slots used, {} animated", p.count(false), p.slots(), p.count(true)), 12.5).color(MUTED));
        if button(ui, "Upload Emoji\u{2026}", false) {
            let chat = chat.clone();
            std::thread::spawn(move || {
                if let Some(path) = rfd::FileDialog::new().set_title("Emoji").add_filter("Pictures", &["png", "jpg", "jpeg", "gif", "webp"]).pick_file() {
                    chat.dispatch(Action::PickServerPicture { kind: ServerPicture::Emoji, path });
                }
            });
        }
    });
    ui.add_space(8.0);
    for e in &p.emojis {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
            let t = texture(ctx, tex, asked, s, &Some(e.picture.clone()), want);
            avatar(ui, r, t, &e.name);
            let entry = names.entry(e.id.clone()).or_insert_with(|| e.name.clone());
            let te = ui.add(egui::TextEdit::singleline(entry).desired_width(200.0));
            if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && *entry != e.name {
                acts.push(Action::Emoji(EmojiInput::Rename { id: e.id.clone(), name: entry.clone() }));
            }
            if let Some(u) = &e.user {
                ui.label(rt(format!("by {}", tidy(&u.name)), 12.0).color(MUTED));
            }
            if !e.available {
                ui.label(rt("unavailable", 12.0).color(th::WARNING));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if button(ui, "Delete", true) {
                    acts.push(Action::Emoji(EmojiInput::Delete(e.id.clone())));
                }
            });
        });
        ui.add_space(2.0);
    }
    if let Some(u) = &p.upload {
        egui::Modal::new(egui::Id::new("emoji_upload")).show(ctx, |ui| {
            ui.set_width(340.0);
            ui.label(sb("Upload Emoji", 16.0).color(TEXT));
            ui.add_space(8.0);
            small(ui, "Name");
            let mut name = u.name.clone();
            if ui.add(egui::TextEdit::singleline(&mut name).desired_width(300.0)).changed() {
                acts.push(Action::Emoji(EmojiInput::Name(name)));
            }
            if let Some(e) = &u.error {
                ui.label(rt(e, 12.5).color(DANGER));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if pill(ui, if u.saving { "Uploading\u{2026}" } else { "Add emoji" }, true).clicked() && !u.saving {
                    acts.push(Action::Emoji(EmojiInput::Finish));
                }
                if pill(ui, "Cancel", false).clicked() {
                    acts.push(Action::Emoji(EmojiInput::Cancel));
                }
            });
        });
    }
}

pub fn stickers(ui: &mut Ui, ctx: &egui::Context, chat: &Chat, s: &AppState, p: &StickersPage, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    heading(ui, "Stickers");
    if button(ui, "Upload Sticker", false) {
        acts.push(Action::Stickers(StickerInput::Upload));
    }
    ui.add_space(8.0);
    for st in &p.stickers {
        ui.horizontal(|ui| {
            let (r, _) = ui.allocate_exact_size(vec2(48.0, 48.0), Sense::hover());
            let t = texture(ctx, tex, asked, s, &st.picture, want);
            avatar(ui, r, t, &st.form.name);
            ui.vertical(|ui| {
                ui.label(sb(&st.form.name, 14.0).color(TEXT));
                ui.label(rt(format!("{}  \u{2022}  {}", st.form.tags, st.form.description), 12.0).color(MUTED));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if button(ui, "Delete", true) {
                    acts.push(Action::Stickers(StickerInput::Delete(st.id.clone())));
                }
                if button(ui, "Edit", false) {
                    acts.push(Action::Stickers(StickerInput::Edit(st.id.clone())));
                }
            });
        });
        ui.add_space(4.0);
    }
    if let Some(d) = &p.draft {
        egui::Modal::new(egui::Id::new("sticker_form")).show(ctx, |ui| {
            ui.set_width(380.0);
            ui.label(sb(if d.editing.is_some() { "Edit Sticker" } else { "Upload Sticker" }, 16.0).color(TEXT));
            if d.editing.is_none() {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if button(ui, "Choose file\u{2026}", false) {
                        let chat = chat.clone();
                        std::thread::spawn(move || {
                            if let Some(path) = rfd::FileDialog::new().set_title("Sticker").add_filter("Pictures", &["png", "apng", "gif"]).pick_file() {
                                chat.dispatch(Action::PickServerPicture { kind: ServerPicture::Sticker, path });
                            }
                        });
                    }
                    ui.label(rt(if d.file_name.is_empty() { "No file chosen" } else { &d.file_name }, 12.5).color(MUTED));
                });
            }
            small(ui, "Name");
            let mut name = d.form.name.clone();
            if ui.add(egui::TextEdit::singleline(&mut name).desired_width(340.0)).changed() {
                acts.push(Action::Stickers(StickerInput::Name(name)));
            }
            small(ui, "Related emoji (name or keyword)");
            let mut tags = d.form.tags.clone();
            if ui.add(egui::TextEdit::singleline(&mut tags).desired_width(340.0)).changed() {
                acts.push(Action::Stickers(StickerInput::Tags(tags)));
            }
            small(ui, "Description (optional)");
            let mut desc = d.form.description.clone();
            if ui.add(egui::TextEdit::singleline(&mut desc).desired_width(340.0)).changed() {
                acts.push(Action::Stickers(StickerInput::Description(desc)));
            }
            if let Some(e) = &d.error {
                ui.label(rt(e, 12.5).color(DANGER));
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.add_enabled_ui(d.ready(), |ui| {
                    if pill(ui, if d.saving { "Saving\u{2026}" } else { "Save" }, true).clicked() {
                        acts.push(Action::Stickers(StickerInput::Save));
                    }
                });
                if pill(ui, "Cancel", false).clicked() {
                    acts.push(Action::Stickers(StickerInput::Cancel));
                }
            });
        });
    }
}

/// The Delete Server and Transfer Ownership dialogs.
pub fn ownership(ctx: &egui::Context, ss: &ServerSettings, acts: &mut Vec<Action>) {
    let Some(o) = &ss.ownership else { return };
    let send = |acts: &mut Vec<Action>, i: OwnershipInput| acts.push(Action::Ownership(i));
    egui::Modal::new(egui::Id::new("ownership")).show(ctx, |ui| {
        ui.set_width(380.0);
        match o {
            Ownership::Delete(d) => {
                ui.label(sb(format!("Delete {}", tidy(&d.name)), 16.0).color(DANGER));
                ui.label(rt("This cannot be undone. The server, its channels and its messages are gone for good.", 13.0).color(TEXT));
                if d.by_name {
                    small(ui, "Enter the server name to confirm");
                    let mut t = d.typed.clone();
                    if ui.add(egui::TextEdit::singleline(&mut t).desired_width(340.0)).changed() {
                        send(acts, OwnershipInput::Type(t));
                    }
                    if d.wrong {
                        ui.label(rt("You didn't enter the server name correctly.", 12.5).color(DANGER));
                    }
                } else {
                    small(ui, "Enter your 2FA code");
                    let mut t = d.typed.clone();
                    if ui.add(egui::TextEdit::singleline(&mut t).desired_width(340.0)).changed() {
                        send(acts, OwnershipInput::Type(t));
                    }
                }
                if let Some(e) = &d.error {
                    ui.label(rt(e, 12.5).color(DANGER));
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if button(ui, if d.sending { "Deleting\u{2026}" } else { "Delete Server" }, true) && !d.sending {
                        send(acts, OwnershipInput::Confirm);
                    }
                    if pill(ui, "Cancel", false).clicked() {
                        send(acts, OwnershipInput::Cancel);
                    }
                });
            }
            Ownership::Transfer(t) => {
                ui.label(sb(format!("Transfer ownership to {}", tidy(&t.to.name)), 16.0).color(TEXT));
                if t.verified {
                    ui.label(rt("A Verified or Partnered server cannot be given away.", 13.0).color(th::WARNING));
                }
                let mut agreed = t.agreed;
                if ui.checkbox(&mut agreed, rt("I understand they will own this server and I will lose that role.", 13.0).color(TEXT)).changed() {
                    send(acts, OwnershipInput::Agree);
                }
                if let Some(code) = &t.code {
                    small(ui, "Code emailed to you");
                    let mut c = code.clone();
                    if ui.add(egui::TextEdit::singleline(&mut c).desired_width(200.0)).changed() {
                        send(acts, OwnershipInput::Type(c));
                    }
                    if button(ui, if t.resent { "Code sent" } else { "Send a new code" }, false) && !t.resending {
                        send(acts, OwnershipInput::Resend);
                    }
                }
                if let Some(e) = &t.error {
                    ui.label(rt(e, 12.5).color(DANGER));
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    ui.add_enabled_ui(t.ready(), |ui| {
                        if button(ui, if t.sending { "Transferring\u{2026}" } else { "Transfer Ownership" }, true) {
                            send(acts, OwnershipInput::Confirm);
                        }
                    });
                    if pill(ui, "Cancel", false).clicked() {
                        send(acts, OwnershipInput::Cancel);
                    }
                });
            }
        }
    });
}
