//! A Discord server's settings, after Discord's: a list of pages on the left, the open page on the right, and
//! the "unsaved changes" bar. The engine holds the page and its draft (`AppState::server_settings`); this draws
//! Overview (name, description, icon), Members (with Timeout, Kick, Ban), Roles (list, editor, permissions),
//! Invites, Audit Log and Bans, and sends the engine's actions. Engagement, Boost Perks, Emoji, Stickers and
//! Safety Setup are not drawn; they are left out of the page list.

use super::voice::{k_panel, k_raised, QUICK};
use super::*;
use mytune_chat::yapfast_core::application::{
    Action, Application, AuditInput, AuditSpan, BanInput, InviteInput, MemberInput, MenuChoice, PersonAsk, RoleConfirm, RoleInput, RoleTab,
    ServerSettings, SettingsPage, SettingsText, BANS_PER_PAGE, DESCRIPTION_LIMIT, MEMBER_PAGE_SIZES, ROLE_COLORS, ROLE_NAME_LIMIT, ROLE_PERMISSIONS,
};
use mytune_chat::yapfast_core::chat::{ServerPicture, SettingsSection};
use mytune_chat::yapfast_core::clock;

/// What the screen keeps between frames: text boxes whose engine call waits for Enter.
#[derive(Default)]
pub struct SettingsUi {
    member_query: String,
    ban_query: String,
    perm_query: String,
    emoji_names: HashMap<String, String>,
}

fn name_of(sec: SettingsSection) -> Option<&'static str> {
    Some(match sec {
        SettingsSection::Profile => "Overview",
        SettingsSection::Engagement => "Engagement",
        SettingsSection::BoostPerks => "Boost Perks",
        SettingsSection::Emoji => "Emoji",
        SettingsSection::Stickers => "Stickers",
        SettingsSection::Safety => "Safety Setup",
        SettingsSection::Members => "Members",
        SettingsSection::Roles => "Roles",
        SettingsSection::Invites => "Invites",
        SettingsSection::AuditLog => "Audit Log",
        SettingsSection::Bans => "Bans",
        _ => return None,
    })
}

fn when(sec: u64) -> String {
    format!("{}, {}", clock::day(sec), clock::time_on(sec, false))
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

fn person_row_icon(ui: &mut Ui, ctx: &egui::Context, url: &str, name: &str, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>) {
    let (r, _) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::hover());
    let u = if url.is_empty() { None } else { Some(url.to_owned()) };
    let t = texture(ctx, tex, asked, s, &u, want);
    avatar(ui, r, t, name);
}

pub fn screen(ui: &mut Ui, su: &mut SettingsUi, chat: &Chat, app: &Application, ss: &ServerSettings, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    let s = &app.state;
    let sections: Vec<SettingsSection> = app.settings_sections(&ss.server).into_iter().filter(|x| name_of(*x).is_some()).collect();
    let server_name = s.servers.iter().find(|x| x.id == ss.server).map(|x| x.name.clone()).unwrap_or_default();
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && ss.ownership.is_none() {
        acts.push(Action::CloseServerSettings);
    }
    egui::Panel::left("ss_nav").exact_size(230.0).frame(egui::Frame::new().fill(super::k_sidebar()).inner_margin(egui::Margin::symmetric(10, 14))).show_separator_line(false).show_inside(ui, |ui| {
        ui.label(sb(tidy(&server_name).to_uppercase(), 12.0).color(MUTED));
        ui.add_space(8.0);
        for sec in &sections {
            let on = *sec == ss.section;
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
            let t = ctx.animate_bool_with_time(resp.id, resp.hovered(), QUICK);
            if on || t > 0.0 {
                ui.painter().rect_filled(r, 10.0, if on { super::k_active() } else { super::k_hover().gamma_multiply(t) });
            }
            ui.painter().text(egui::pos2(r.left() + 12.0, r.center().y), egui::Align2::LEFT_CENTER, name_of(*sec).unwrap_or(""), egui::FontId::proportional(14.0), if on || (crate::theme::p().black && resp.hovered()) { TEXT } else { SECONDARY });
            if resp.clicked() {
                acts.push(Action::ServerSettingsSection(*sec));
            }
        }
        if app.may_give_up(&ss.server) {
            ui.add_space(8.0);
            if button(ui, "Delete Server", true) {
                acts.push(Action::Ownership(mytune_chat::yapfast_core::application::OwnershipInput::Delete));
            }
        }
        ui.add_space(12.0);
        if button(ui, "Close  (Esc)", false) {
            acts.push(Action::CloseServerSettings);
        }
        if ss.nudge > 0 && ss.changed() {
            ui.add_space(6.0);
            ui.label(rt("Save or reset your changes first.", 12.0).color(th::WARNING));
        }
    });
    // the bar for pages that save as a whole
    if !ss.immediate() && ss.changed() {
        egui::Panel::bottom("ss_bar").frame(egui::Frame::new().fill(k_panel()).inner_margin(egui::Margin::symmetric(20, 10))).show_separator_line(false).show_inside(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(rt("Careful \u{2014} you have unsaved changes!", 13.5).color(TEXT));
                if let Some(p) = ss.problem() {
                    ui.label(rt(p, 12.5).color(DANGER));
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_enabled_ui(ss.ready(), |ui| {
                        if pill(ui, if ss.saving { "Saving\u{2026}" } else { "Save Changes" }, true).clicked() {
                            acts.push(Action::SaveServerSettings);
                        }
                    });
                    if button(ui, "Reset", false) {
                        acts.push(Action::UndoServerSettings);
                    }
                });
            });
        });
    }
    egui::CentralPanel::default().frame(egui::Frame::new().fill(super::k_chat()).inner_margin(egui::Margin::symmetric(28, 16))).show_inside(ui, |ui| {
        if let Some(e) = &ss.error {
            ui.label(rt(e, 13.0).color(DANGER));
        }
        let Some(page) = &ss.page else {
            ui.label(rt("Loading\u{2026}", 14.0).color(MUTED));
            return;
        };
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| match page {
            SettingsPage::Profile(p) => {
                heading(ui, "Server Overview");
                small(ui, "Server name");
                let mut name = p.name.clone();
                if ui.add(egui::TextEdit::singleline(&mut name).desired_width(420.0)).changed() {
                    acts.push(Action::WriteServerSetting { field: SettingsText::Name, text: name });
                }
                ui.add_space(10.0);
                small(ui, &format!("Description ({} max)", DESCRIPTION_LIMIT));
                let mut d = p.description.clone();
                if ui.add(egui::TextEdit::multiline(&mut d).desired_width(420.0).desired_rows(3)).changed() {
                    acts.push(Action::WriteServerSetting { field: SettingsText::Description, text: d });
                }
                ui.add_space(10.0);
                small(ui, "Server icon");
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(64.0, 64.0), Sense::hover());
                    let t = texture(&ctx, tex, asked, s, &p.original.icon, want);
                    avatar(ui, r, t, &p.name);
                    ui.vertical(|ui| {
                        if button(ui, "Change icon\u{2026}", false) {
                            let chat = chat.clone();
                            std::thread::spawn(move || {
                                if let Some(path) = rfd::FileDialog::new().set_title("Server icon").add_filter("Pictures", &["png", "jpg", "jpeg", "gif", "webp"]).pick_file() {
                                    chat.dispatch(Action::PickServerPicture { kind: ServerPicture::Icon, path });
                                }
                            });
                        }
                        if button(ui, "Remove icon", false) {
                            acts.push(Action::RemoveServerPicture(ServerPicture::Icon));
                        }
                    });
                });
            }
            SettingsPage::Engagement(p) => super::serverpages::engagement(ui, app, ss, p, acts),
            SettingsPage::BoostPerks(p) => super::serverpages::boost(ui, chat, p, acts),
            SettingsPage::Safety(p) => super::serverpages::safety(ui, app, ss, p, acts),
            SettingsPage::Emoji(p) => super::serverpages::emoji(ui, &ctx, chat, s, p, &mut su.emoji_names, tex, asked, want, acts),
            SettingsPage::Stickers(p) => super::serverpages::stickers(ui, &ctx, chat, s, p, tex, asked, want, acts),
            SettingsPage::Members(m) => {
                heading(ui, "Members");
                ui.horizontal(|ui| {
                    let te = ui.add(egui::TextEdit::singleline(&mut su.member_query).desired_width(300.0).hint_text("Search by username or nickname"));
                    if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        acts.push(Action::Members(MemberInput::Search(su.member_query.clone())));
                    }
                    if let Some(total) = m.total {
                        ui.label(rt(format!("{total} members"), 12.5).color(MUTED));
                    }
                    if m.fetching() {
                        ui.label(rt("Loading\u{2026}", 12.5).color(MUTED));
                    }
                });
                ui.add_space(8.0);
                let start = m.page * m.page_size;
                for lm in m.members.iter().skip(start).take(m.page_size) {
                    ui.horizontal(|ui| {
                        person_row_icon(ui, &ctx, &lm.person.avatar, &lm.person.name, s, tex, asked, want);
                        ui.vertical(|ui| {
                            ui.label(sb(&lm.person.name, 14.0).color(TEXT));
                            let mut sub = String::new();
                            if let Some(u) = &lm.person.username {
                                sub.push_str(u);
                                sub.push_str("  \u{2022}  ");
                            }
                            sub.push_str(&format!("joined {}", when(lm.joined / 1000)));
                            if lm.timed_out_until > clock::now_ms() {
                                sub.push_str("  \u{2022}  timed out");
                            }
                            ui.label(rt(sub, 12.0).color(MUTED));
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let id = &lm.person.id;
                            let c = app.person_choices(id);
                            for (choice, ask, text, danger) in [(MenuChoice::Ban, PersonAsk::Ban, "Ban", true), (MenuChoice::Kick, PersonAsk::Kick, "Kick", true), (MenuChoice::Timeout, PersonAsk::Timeout, "Timeout", false)] {
                                if c.contains(&choice) && button(ui, text, danger) {
                                    acts.push(Action::AskMember { place: ss.server.clone(), user: id.clone(), ask });
                                }
                            }
                            if app.may_give_up(&ss.server) && button(ui, "Make owner", false) {
                                acts.push(Action::Ownership(mytune_chat::yapfast_core::application::OwnershipInput::Transfer(id.clone())));
                            }
                            if c.contains(&MenuChoice::RemoveTimeout) && button(ui, "End timeout", false) {
                                acts.push(Action::ChangeMember { place: ss.server.clone(), user: id.clone(), change: mytune_chat::yapfast_core::chat::MemberChange::Timeout { until_ms: None, reason: String::new() } });
                            }
                        });
                    });
                    ui.add_space(4.0);
                }
                ui.horizontal(|ui| {
                    if m.page > 0 && button(ui, "Previous", false) {
                        acts.push(Action::Members(MemberInput::Page(m.page - 1)));
                    }
                    let have = m.members.len();
                    if (start + m.page_size < have || m.more) && button(ui, "Next", false) {
                        acts.push(Action::Members(MemberInput::Page(m.page + 1)));
                    }
                    ui.label(rt(format!("Page {}", m.page + 1), 12.0).color(MUTED));
                    for n in MEMBER_PAGE_SIZES {
                        if n != m.page_size && button(ui, &format!("{n} per page"), false) {
                            acts.push(Action::Members(MemberInput::PageSize(n)));
                        }
                    }
                });
            }
            SettingsPage::Roles(r) => {
                match &r.open {
                    None => {
                        ui.horizontal(|ui| {
                            ui.label(sb("Roles", 20.0).color(TEXT));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if pill(ui, "Create Role", true).clicked() {
                                    acts.push(Action::Roles(RoleInput::Create));
                                }
                            });
                        });
                        ui.add_space(8.0);
                        for id in &r.order {
                            let Some(role) = r.original.roles.iter().find(|x| &x.id == id) else { continue };
                            let (row, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
                            let t = ctx.animate_bool_with_time(resp.id, resp.hovered(), QUICK);
                            ui.painter().rect_filled(row, 10.0, super::k_hover().gamma_multiply(t));
                            let col = if role.color == 0 { MUTED } else { Color32::from_rgb((role.color >> 16) as u8, (role.color >> 8) as u8, role.color as u8) };
                            ui.painter().circle_filled(egui::pos2(row.left() + 16.0, row.center().y), 6.0, col);
                            ui.painter().text(egui::pos2(row.left() + 32.0, row.center().y), egui::Align2::LEFT_CENTER, &role.name, egui::FontId::proportional(14.0), crate::theme::hover_text(TEXT, resp.hovered()));
                            ui.painter().text(egui::pos2(row.right() - 12.0, row.center().y), egui::Align2::RIGHT_CENTER, format!("{} members", role.members), egui::FontId::proportional(12.0), MUTED);
                            if resp.clicked() {
                                acts.push(Action::Roles(RoleInput::Open(Some(role.id.clone()))));
                            }
                        }
                    }
                    Some(d) => {
                        if button(ui, "\u{2190} All roles", false) {
                            acts.push(Action::Roles(RoleInput::Open(None)));
                        }
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            for (tab, t) in [(RoleTab::Display, "Display"), (RoleTab::Permissions, "Permissions"), (RoleTab::Members, "Members")] {
                                let on = d.tab == tab;
                                if ui.add(crate::theme::Choice::new(t, on)).clicked() {
                                    acts.push(Action::Roles(RoleInput::Tab(tab)));
                                }
                            }
                        });
                        ui.add_space(10.0);
                        match d.tab {
                            RoleTab::Display => {
                                small(ui, &format!("Role name ({ROLE_NAME_LIMIT} max)"));
                                let mut n = d.name.clone();
                                if ui.add(egui::TextEdit::singleline(&mut n).desired_width(380.0)).changed() {
                                    acts.push(Action::Roles(RoleInput::Name(n)));
                                }
                                ui.add_space(10.0);
                                small(ui, "Role colour");
                                ui.horizontal_wrapped(|ui| {
                                    for c in ROLE_COLORS {
                                        let (r, resp) = ui.allocate_exact_size(vec2(26.0, 26.0), Sense::click());
                                        let col = Color32::from_rgb((c >> 16) as u8, (c >> 8) as u8, c as u8);
                                        ui.painter().rect_filled(r, 5.0, col);
                                        if d.color == c {
                                            ui.painter().rect_stroke(r.expand(2.0), 6.0, egui::Stroke::new(2.0, TEXT), egui::StrokeKind::Outside);
                                        }
                                        if resp.clicked() {
                                            acts.push(Action::Roles(RoleInput::Color(c)));
                                        }
                                    }
                                });
                                ui.add_space(10.0);
                                let mut h = d.hoist;
                                if ui.checkbox(&mut h, rt("Display role members separately from online members", 13.0)).changed() {
                                    acts.push(Action::Roles(RoleInput::Hoist));
                                }
                                let mut mn = d.mentionable;
                                if ui.checkbox(&mut mn, rt("Allow anyone to @mention this role", 13.0)).changed() {
                                    acts.push(Action::Roles(RoleInput::Mentionable));
                                }
                                ui.add_space(14.0);
                                if button(ui, "Delete role", true) {
                                    acts.push(Action::Roles(RoleInput::Delete(d.id.clone())));
                                }
                            }
                            RoleTab::Permissions => {
                                ui.add(egui::TextEdit::singleline(&mut su.perm_query).desired_width(300.0).hint_text("Search permissions"));
                                let q = su.perm_query.to_lowercase();
                                for g in ROLE_PERMISSIONS.iter() {
                                    let shown: Vec<_> = g.permissions.iter().filter(|pi| (!pi.community || r.original.community) && (q.is_empty() || pi.title.to_lowercase().contains(&q))).collect();
                                    if shown.is_empty() {
                                        continue;
                                    }
                                    ui.add_space(10.0);
                                    small(ui, g.heading);
                                    for pi in shown {
                                        let mask = 1u64 << pi.bit;
                                        let mut on = d.permissions & mask != 0;
                                        if ui.checkbox(&mut on, rt(pi.title, 13.5)).on_hover_text(pi.description).changed() {
                                            acts.push(Action::Roles(RoleInput::Permission(mask)));
                                        }
                                    }
                                }
                            }
                            RoleTab::Members => {
                                if button(ui, "Add members\u{2026}", false) {
                                    acts.push(Action::Roles(RoleInput::AddMembers));
                                }
                                ui.add_space(6.0);
                                match &d.members {
                                    None => {
                                        ui.label(rt("Loading\u{2026}", 13.0).color(MUTED));
                                    }
                                    Some(Err(e)) => {
                                        ui.label(rt(e, 13.0).color(DANGER));
                                    }
                                    Some(Ok(list)) => {
                                        for pm in list {
                                            ui.horizontal(|ui| {
                                                person_row_icon(ui, &ctx, &pm.avatar, &pm.name, s, tex, asked, want);
                                                ui.label(rt(&pm.name, 14.0).color(TEXT));
                                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                                    if button(ui, "Remove", false) {
                                                        acts.push(Action::Roles(RoleInput::RemoveMember(pm.id.clone())));
                                                    }
                                                });
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            SettingsPage::Invites(iv) => {
                ui.horizontal(|ui| {
                    ui.label(sb("Invites", 20.0).color(TEXT));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if pill(ui, "Create Invite Link", true).clicked() {
                            acts.push(Action::Invites(InviteInput::Create));
                        }
                    });
                });
                ui.add_space(8.0);
                if iv.invites.is_empty() {
                    ui.label(rt("No active invites.", 13.5).color(MUTED));
                }
                for i in &iv.invites {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.label(sb(format!("discord.gg/{}", i.code), 14.0).color(TEXT));
                            let by = i.inviter.as_ref().map(|x| x.name.clone()).unwrap_or_else(|| "unknown".into());
                            let uses = if i.max_uses == 0 { format!("{} uses", i.uses) } else { format!("{} / {} uses", i.uses, i.max_uses) };
                            let exp = i.expires.map(|e| format!("expires {}", when(e))).unwrap_or_else(|| "never expires".into());
                            ui.label(rt(format!("{} \u{2022} by {by} \u{2022} {uses} \u{2022} {exp}", i.channel), 12.0).color(MUTED));
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if super::sure(ui, ("invite", &i.code), "Revoke", |ui, l| button(ui, l, true)) {
                                acts.push(Action::Invites(InviteInput::Revoke(i.code.clone())));
                            }
                            if button(ui, "Copy", false) {
                                ctx.copy_text(format!("https://discord.gg/{}", i.code));
                            }
                        });
                    });
                    ui.add_space(6.0);
                }
            }
            SettingsPage::AuditLog(a) => {
                heading(ui, "Audit Log");
                if a.failed {
                    ui.label(rt("Discord did not send the newest entries.", 13.0).color(th::WARNING));
                }
                for (i, row) in a.rows.iter().enumerate() {
                    let open = a.open == Some(i);
                    let who = row.actor.and_then(|k| a.actors.get(k as usize)).map(|(_, u)| format!("{u:?}"));
                    let _ = who;
                    let resp = egui::Frame::new().fill(if open { super::k_hover() } else { Color32::TRANSPARENT }).corner_radius(10.0).inner_margin(egui::Margin::symmetric(8, 5)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal_wrapped(|ui| {
                            for sp in row.title.spans() {
                                match sp {
                                    AuditSpan::Text(t) => {
                                        ui.label(rt(t, 13.5).color(SECONDARY));
                                    }
                                    AuditSpan::Bold(t) => {
                                        ui.label(sb(t, 13.5).color(TEXT));
                                    }
                                    AuditSpan::Time(t) => {
                                        ui.label(rt(when(t), 13.5).color(MUTED));
                                    }
                                }
                            }
                        });
                        ui.label(rt(when(row.start), 11.5).color(MUTED));
                        if open {
                            for line in &row.lines {
                                ui.horizontal_wrapped(|ui| {
                                    for sp in line.words.spans() {
                                        match sp {
                                            AuditSpan::Text(t) => ui.label(rt(t, 12.5).color(SECONDARY)),
                                            AuditSpan::Bold(t) => ui.label(sb(t, 12.5).color(TEXT)),
                                            AuditSpan::Time(t) => ui.label(rt(when(t), 12.5).color(MUTED)),
                                        };
                                    }
                                });
                            }
                        }
                    });
                    if resp.response.interact(Sense::click()).clicked() {
                        acts.push(Action::AuditLog(AuditInput::Toggle(i)));
                    }
                }
                if a.fetching() {
                    ui.label(rt("Loading\u{2026}", 13.0).color(MUTED));
                } else if a.older && button(ui, "Load older entries", false) {
                    acts.push(Action::AuditLog(AuditInput::More));
                }
            }
            SettingsPage::Bans(b) => {
                heading(ui, "Bans");
                ui.horizontal(|ui| {
                    let te = ui.add(egui::TextEdit::singleline(&mut su.ban_query).desired_width(300.0).hint_text("Search by username or ID"));
                    if te.changed() {
                        acts.push(Action::Bans(BanInput::Query(su.ban_query.clone())));
                    }
                    if te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        acts.push(Action::Bans(BanInput::Search));
                    }
                    if b.fetching {
                        ui.label(rt("Loading\u{2026}", 12.5).color(MUTED));
                    }
                });
                ui.add_space(8.0);
                let shown = b.shown();
                if shown.is_empty() && !b.fetching {
                    ui.label(rt("No bans.", 13.5).color(MUTED));
                }
                for ban in shown.iter().skip(b.page * BANS_PER_PAGE).take(BANS_PER_PAGE) {
                    let open = b.open.as_deref() == Some(ban.user.id.as_str());
                    ui.horizontal(|ui| {
                        person_row_icon(ui, &ctx, &ban.user.avatar, &ban.user.name, s, tex, asked, want);
                        ui.vertical(|ui| {
                            ui.label(sb(&ban.user.name, 14.0).color(TEXT));
                            if let Some(r) = &ban.reason {
                                ui.label(rt(format!("Reason: {r}"), 12.0).color(MUTED));
                            }
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if open {
                                if button(ui, if b.unbanning { "Revoking\u{2026}" } else { "Revoke Ban" }, true) && !b.unbanning {
                                    acts.push(Action::Bans(BanInput::Unban));
                                }
                                if button(ui, "Cancel", false) {
                                    acts.push(Action::Bans(BanInput::Close));
                                }
                            } else if button(ui, "Revoke\u{2026}", false) {
                                acts.push(Action::Bans(BanInput::Open(ban.user.id.clone())));
                            }
                        });
                    });
                    if open {
                        if let Some(e) = &b.open_error {
                            ui.label(rt(e, 12.5).color(DANGER));
                        }
                    }
                    ui.add_space(4.0);
                }
                ui.horizontal(|ui| {
                    if b.page > 0 && button(ui, "Previous", false) {
                        acts.push(Action::Bans(BanInput::Page(b.page - 1)));
                    }
                    if (b.page + 1) * BANS_PER_PAGE < shown.len() || b.more {
                        if button(ui, "Next", false) {
                            acts.push(Action::Bans(BanInput::Page(b.page + 1)));
                        }
                    }
                });
            }
            _ => {
                ui.label(rt("This page is not available in MyTune yet.", 14.0).color(MUTED));
            }
        });
    });
    // what Roles asked before doing it
    if let Some(SettingsPage::Roles(r)) = &ss.page {
        if let Some(c) = &r.confirm {
            egui::Modal::new(egui::Id::new("role_confirm")).show(&ctx, |ui| {
                ui.set_width(380.0);
                ui.label(sb(match c { RoleConfirm::Delete(_) => "Delete role?", RoleConfirm::RemoveMember(_) => "Remove member from role?" }, 16.0).color(TEXT));
                ui.add_space(12.0);
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if button(ui, "Yes", true) {
                        acts.push(Action::Roles(RoleInput::Confirm));
                    }
                    if button(ui, "No", false) {
                        acts.push(Action::Roles(RoleInput::Cancel));
                    }
                });
            });
        }
    }
}
