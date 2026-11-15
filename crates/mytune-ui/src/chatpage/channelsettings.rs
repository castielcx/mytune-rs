//! A Discord channel's or category's settings over the chat, after Discord's: Overview (name, topic, slowmode,
//! age restriction), Permissions (the overwrites for roles and members, three choices per permission, private
//! channel, sync with the category) and Delete. The engine holds the page and its draft
//! (`AppState::channel_settings`); this draws it and sends `ChannelInput`s.

use super::voice::{k_panel, k_raised, QUICK};
use super::*;
use mytune_chat::yapfast_core::application::{
    channel_groups, channel_permission, Action, Application, ChannelInput, ChannelSection, ChannelSettingsScreen, OverwriteConfirm, OverwriteState, Overwrites, SLOWMODES,
};
use mytune_chat::yapfast_core::chat::ChannelKind;

/// What the screen keeps between frames: the picker's search text.
#[derive(Default)]
pub struct ChannelUi {
    find: String,
}

fn slow(sec: u32) -> String {
    match sec {
        0 => "Off".to_owned(),
        1..=59 => format!("{sec}s"),
        60..=3599 => format!("{}m", sec / 60),
        _ => format!("{}h", sec / 3600),
    }
}

fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(4.0);
    ui.label(sb(text, 20.0).color(TEXT));
    ui.add_space(10.0);
}

fn small(ui: &mut Ui, text: &str) {
    ui.label(rt(text.to_uppercase(), 11.0).color(MUTED));
}

fn button(ui: &mut Ui, text: &str, danger: bool) -> bool {
    crate::theme::button_tint(ui, text, false, danger.then_some(DANGER), (12.0, 3.0, 12.0, 4.0)).clicked()
}

fn input(acts: &mut Vec<Action>, i: ChannelInput) {
    acts.push(Action::ChannelSettings(i));
}

pub fn screen(ui: &mut Ui, cu: &mut ChannelUi, app: &Application, cs: &ChannelSettingsScreen, acts: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    let room = &cs.room;
    let title = app.channel_title(room);
    let channel = if cs.kind == ChannelKind::Text { format!("#{title}") } else { title };
    if cs.permissions.confirm.is_none() && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        input(acts, ChannelInput::Close);
    }
    egui::Panel::left("cs_nav").exact_size(230.0).frame(egui::Frame::new().fill(super::k_sidebar()).inner_margin(egui::Margin::symmetric(10, 14))).show_separator_line(false).show_inside(ui, |ui| {
        ui.label(sb(tidy(&channel).to_uppercase(), 12.0).color(MUTED));
        ui.add_space(8.0);
        for sec in app.channel_sections(room) {
            let on = sec == cs.section;
            let name = match sec {
                ChannelSection::Overview => "Overview",
                ChannelSection::Permissions => "Permissions",
            };
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
            let t = ctx.animate_bool_with_time(resp.id, resp.hovered(), QUICK);
            if on || t > 0.0 {
                ui.painter().rect_filled(r, 10.0, if on { super::k_active() } else { super::k_hover().gamma_multiply(t) });
            }
            ui.painter().text(egui::pos2(r.left() + 12.0, r.center().y), egui::Align2::LEFT_CENTER, name, egui::FontId::proportional(14.0), if on { TEXT } else { SECONDARY });
            if resp.clicked() {
                input(acts, ChannelInput::Section(sec));
            }
        }
        if app.may_delete_channel(room) {
            ui.add_space(8.0);
            if button(ui, if cs.kind == ChannelKind::Category { "Delete Category" } else { "Delete Channel" }, true) {
                input(acts, ChannelInput::Delete);
            }
        }
        ui.add_space(12.0);
        if button(ui, "Close  (Esc)", false) {
            input(acts, ChannelInput::Close);
        }
        if cs.nudge > 0 && cs.changed() {
            ui.add_space(6.0);
            ui.label(rt("Save or reset your changes first.", 12.0).color(th::WARNING));
        }
    });
    if cs.changed() {
        egui::Panel::bottom("cs_bar").frame(egui::Frame::new().fill(k_panel()).inner_margin(egui::Margin::symmetric(20, 10))).show_separator_line(false).show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(rt("Careful \u{2014} you have unsaved changes!", 13.5).color(TEXT));
                if let Some(p) = cs.problem() {
                    ui.label(rt(p, 12.5).color(DANGER));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_enabled_ui(cs.ready() && app.state.discord_online, |ui| {
                        if pill(ui, if cs.saving { "Saving\u{2026}" } else { "Save Changes" }, true).clicked() {
                            input(acts, ChannelInput::Save);
                        }
                    });
                    if button(ui, "Reset", false) {
                        input(acts, ChannelInput::Undo);
                    }
                });
            });
        });
    }
    egui::CentralPanel::default().frame(egui::Frame::new().fill(super::k_chat()).inner_margin(egui::Margin::symmetric(28, 16))).show_inside(ui, |ui| {
        if let Some(e) = &cs.error {
            ui.label(rt(e, 13.0).color(DANGER));
        }
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match cs.section {
            ChannelSection::Overview => overview(ui, cs, acts),
            ChannelSection::Permissions => permissions(ui, cu, app, cs, acts),
        });
    });
    confirm(&ctx, app, cs, acts);
}

fn overview(ui: &mut Ui, cs: &ChannelSettingsScreen, acts: &mut Vec<Action>) {
    heading(ui, "Overview");
    let Some((_, draft)) = &cs.overview else {
        ui.label(rt("Loading\u{2026}", 14.0).color(MUTED));
        return;
    };
    small(ui, if cs.kind == ChannelKind::Category { "Category name" } else { "Channel name" });
    let mut name = draft.name.clone();
    if ui.add(egui::TextEdit::singleline(&mut name).desired_width(420.0)).changed() {
        input(acts, ChannelInput::Name(name));
    }
    if cs.kind == ChannelKind::Category {
        return;
    }
    if cs.kind == ChannelKind::Text {
        ui.add_space(10.0);
        small(ui, "Channel topic");
        let mut topic = draft.topic.clone();
        if ui.add(egui::TextEdit::multiline(&mut topic).desired_width(420.0).desired_rows(3)).changed() {
            input(acts, ChannelInput::Topic(topic));
        }
        ui.add_space(10.0);
        small(ui, "Slowmode");
        ui.label(rt("Members wait this long between messages. Moderators and people with Manage Channel are not held back.", 12.0).color(MUTED));
        ui.horizontal_wrapped(|ui| {
            for &sec in SLOWMODES.iter() {
                let on = sec == draft.slowmode;
                let text = slow(sec);
                let b = crate::theme::Choice::new(&text, on).min_size(vec2(0.0, 28.0));
                if ui.add(b).clicked() {
                    input(acts, ChannelInput::Slowmode(sec));
                }
            }
        });
    }
    ui.add_space(10.0);
    let mut nsfw = draft.nsfw;
    if ui.checkbox(&mut nsfw, rt("Age-restricted channel", 13.5).color(TEXT)).changed() {
        input(acts, ChannelInput::Nsfw(nsfw));
    }
}

fn permissions(ui: &mut Ui, cu: &mut ChannelUi, app: &Application, cs: &ChannelSettingsScreen, acts: &mut Vec<Action>) {
    let room = &cs.room;
    let page = &cs.permissions;
    let category = cs.kind == ChannelKind::Category;
    heading(ui, if category { "Category Permissions" } else { "Channel Permissions" });
    let rules = app.overwrite_rules(room);
    let access: u64 = if cs.kind == ChannelKind::Voice { (1 << 10) | (1 << 20) } else { 1 << 10 };
    // private channel and sync with the category
    let mut private = page.private(access);
    if ui.checkbox(&mut private, rt(if category { "Private Category" } else { "Private Channel" }, 14.0).color(TEXT)).changed() {
        input(acts, ChannelInput::Private(private));
    }
    ui.label(rt(if private { "Only the roles and members below can view it." } else { "Everyone who can see the server can view it." }, 12.0).color(MUTED));
    if !category {
        if let Some((_, cat)) = app.sync_category(room) {
            ui.horizontal(|ui| {
                if app.synced(room) {
                    ui.label(rt(format!("Permissions are synced with the category {}.", tidy(&cat)), 12.5).color(SECONDARY));
                } else {
                    ui.label(rt(format!("Not synced with the category {}.", tidy(&cat)), 12.5).color(th::WARNING));
                    if button(ui, "Sync Now", false) {
                        input(acts, ChannelInput::Sync);
                    }
                }
            });
        }
    }
    ui.add_space(10.0);
    let listed = app.listed_overwrites();
    let roles = app.server_roles(room);
    ui.horizontal_top(|ui| {
        // who has an overwrite
        ui.vertical(|ui| {
            ui.set_width(210.0);
            small(ui, "Roles / Members");
            ui.add_space(4.0);
            for ow in &listed {
                let on = ow.id == page.selected;
                let name = app.overwrite_name(room, ow);
                let (r, resp) = ui.allocate_exact_size(vec2(210.0, 30.0), Sense::click());
                if on || resp.hovered() {
                    ui.painter().rect_filled(r, 10.0, if on { super::k_active() } else { super::k_hover() });
                }
                super::voice::glyph(ui.painter(), egui::pos2(r.left() + 16.0, r.center().y), if ow.member { "\u{E77B}" } else { "\u{E716}" }, 13.0, SECONDARY, false);
                let shown = super::ellipsize(ui, &tidy(&name), &egui::FontId::proportional(13.5), 160.0);
                ui.painter().text(egui::pos2(r.left() + 30.0, r.center().y), egui::Align2::LEFT_CENTER, shown, egui::FontId::proportional(13.5), if on { TEXT } else { SECONDARY });
                if resp.clicked() {
                    input(acts, ChannelInput::Select(ow.id.clone()));
                }
            }
            ui.add_space(6.0);
            if page.adding.is_none() {
                if button(ui, "Add role or member", false) {
                    cu.find.clear();
                    input(acts, ChannelInput::Adding(true));
                }
            } else {
                if ui.add(egui::TextEdit::singleline(&mut cu.find).desired_width(210.0).hint_text("Search roles and members")).changed() {
                    input(acts, ChannelInput::FindMembers(cu.find.clone()));
                }
                let q = cu.find.trim().to_lowercase();
                let free = |id: &str| !listed.iter().any(|k| k.id == id);
                egui::ScrollArea::vertical().max_height(220.0).id_salt("cs_add").show(ui, |ui| {
                    for role in roles.iter().filter(|r| free(&r.id) && (q.is_empty() || r.name.to_lowercase().contains(&q))) {
                        if ui.add(egui::Button::new(rt(format!("@{}", tidy(&role.name)), 13.0).color(TEXT)).fill(Color32::TRANSPARENT).min_size(vec2(200.0, 26.0))).clicked() {
                            input(acts, ChannelInput::Add(role.id.clone(), None));
                        }
                    }
                    if let Some(add) = &page.adding {
                        for m in add.found.iter().filter(|m| free(&m.id)) {
                            if ui.add(egui::Button::new(rt(tidy(&m.name), 13.0).color(TEXT)).fill(Color32::TRANSPARENT).min_size(vec2(200.0, 26.0))).clicked() {
                                input(acts, ChannelInput::Add(m.id.clone(), Some(m.clone())));
                            }
                        }
                    }
                });
                if button(ui, "Done", false) {
                    input(acts, ChannelInput::Adding(false));
                }
            }
        });
        ui.add_space(16.0);
        // the permissions of the shown overwrite
        ui.vertical(|ui| {
            let shown = page.shown();
            let everyone = page.is_everyone(&shown);
            ui.horizontal(|ui| {
                ui.label(sb(tidy(&app.overwrite_name(room, &shown)), 15.0).color(TEXT));
                if !everyone && page.has(&shown.id) && button(ui, "Remove", true) {
                    input(acts, ChannelInput::Remove);
                }
            });
            for (group, bits) in channel_groups(cs.kind) {
                ui.add_space(10.0);
                small(ui, group);
                for &bit in *bits {
                    let (name, desc) = channel_permission(bit, cs.kind, everyone);
                    let locked = rules.locked(bit);
                    let cur = Overwrites::state(&shown, bit);
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.set_width(380.0);
                            ui.label(sb(name, 13.5).color(TEXT));
                            ui.add(egui::Label::new(rt(desc, 11.5).color(MUTED)).wrap());
                            if locked {
                                ui.label(rt("You cannot modify this permission because none of your roles have it.", 11.0).color(th::WARNING));
                            }
                        });
                        ui.add_enabled_ui(!locked, |ui| {
                            for (st, glyph, col) in [(OverwriteState::Deny, "\u{2715}", DANGER), (OverwriteState::Neutral, "/", SECONDARY), (OverwriteState::Allow, "\u{2713}", LIVE)] {
                                let on = cur == st;
                                let b = egui::Button::new(sb(glyph, 14.0).color(if on { Color32::WHITE } else { col })).fill(if on { col.gamma_multiply(0.8) } else { k_raised() }).corner_radius(10.0).min_size(vec2(34.0, 28.0));
                                if ui.add(b).clicked() {
                                    input(acts, ChannelInput::Set(bit, st));
                                }
                            }
                        });
                    });
                }
            }
        });
    });
}

fn confirm(ctx: &egui::Context, app: &Application, cs: &ChannelSettingsScreen, acts: &mut Vec<Action>) {
    let Some(c) = &cs.permissions.confirm else { return };
    let (text, yes) = match c {
        OverwriteConfirm::Remove(id) => {
            let o = cs.permissions.draft().into_iter().find(|o| o.id == *id).unwrap_or(mytune_chat::yapfast_core::chat::ChannelOverwrite { id: id.clone(), ..Default::default() });
            (format!("Remove {} from this channel?", tidy(&app.overwrite_name(&cs.room, &o))), Some("Remove"))
        }
        OverwriteConfirm::Sync => ("Give this channel its category's permissions? Its own overwrites are replaced.".to_owned(), Some("Sync")),
        OverwriteConfirm::DenySelf(name) => (format!("That would take the permission away from you too, as {}.", tidy(name)), None),
    };
    egui::Modal::new(egui::Id::new("cs_confirm")).show(ctx, |ui| {
        ui.set_width(360.0);
        ui.label(rt(text, 14.0).color(TEXT));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if let Some(y) = yes {
                if pill(ui, y, true).clicked() {
                    input(acts, ChannelInput::Confirm);
                }
            }
            if pill(ui, if yes.is_some() { "Cancel" } else { "OK" }, false).clicked() {
                input(acts, ChannelInput::Cancel);
            }
        });
    });
}
