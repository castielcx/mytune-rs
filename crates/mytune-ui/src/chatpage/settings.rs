//! Yapfast's settings screen (ui/settings.slint), inside the chat page: My account, Voice & video, Messages,
//! Notifications, Sounds, Discord, Game activity, Advanced. Values live in `mytune_chat::Settings` (MyTune's own
//! file) or in the engine (status, profile, soundboard volume, game sharing). Appearance and Yapfast's window
//! options are left out: MyTune's own theme, tray and autostart cover them.

use super::voice::{glyph, k_edge, k_panel, QUICK, k_raised};
use super::*;
use egui::{pos2, Rect, Stroke};
use mytune_chat::yapfast_core::application::{ClearAfter, ProfileText};
use mytune_chat::yapfast_core::noise::{Strength, Suppressor};
use mytune_chat::yapfast_core::picture::Feed;
use mytune_chat::yapfast_core::sounds::{Sound, SoundSet};
use mytune_chat::yapfast_core::voice::Command as Vc;
use mytune_chat::{InputMode, Settings};

/// Work on the engine that must wait until the page let go of its state lock.
pub type Later = Vec<Box<dyn FnOnce(&Chat)>>;

const PAGES: [&str; 8] = ["My account", "Voice & video", "Messages", "Notifications", "Sounds", "Discord", "Game activity", "Advanced"];
const I_CLOSE: &str = "\u{E711}";
const I_PLAY: &str = "\u{E768}";

/// The settings screen while it is open.
#[derive(Default)]
pub struct SettingsUi {
    pub open: bool,
    page: usize,
    custom_status: String,
    token: String,
    profile: [String; 4],
    profile_open: bool,
    game_name: String,
    game_exe: String,
    /// The microphone after suppression, as the input sensitivity slider shows it.
    meter: Option<mytune_chat::yapfast_core::discord_audio::GateMeter>,
    camera: Option<(mytune_chat::yapfast_core::camera::Camera, Feed)>,
    camera_tex: Option<TextureHandle>,
    camera_error: Option<String>,
}

impl SettingsUi {
    pub fn open(&mut self, page: usize) {
        self.open = true;
        self.page = page;
    }

    fn close(&mut self) {
        self.open = false;
        self.meter = None;
        self.camera = None;
        self.camera_tex = None;
    }
}

/// A section title with a thin line above it, as Yapfast's settings group their rows.
fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(18.0);
    let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(line, 0.0, k_line());
    ui.add_space(16.0);
    ui.label(sb(text.to_uppercase(), 11.5).color(MUTED));
    ui.add_space(10.0);
}

/// A label with an optional help line under it, `w` wide.
fn label_block(ui: &mut Ui, w: f32, label: &str, help: &str) {
    ui.vertical(|ui| {
        ui.set_width(w);
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(rt(label, 14.5).color(TEXT));
        if !help.is_empty() {
            ui.add(egui::Label::new(rt(help, 12.5).color(MUTED)).wrap());
        }
    });
}

/// A row with a label (and a line of help under it) and a switch at the column's right edge. True when switched.
fn toggle(ui: &mut Ui, label: &str, help: &str, on: &mut bool) -> bool {
    let mut changed = false;
    let w = ui.available_width();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        label_block(ui, w - 60.0, label, help);
        ui.add_space(14.0);
        let (rect, resp) = ui.allocate_exact_size(vec2(46.0, 26.0), Sense::click());
        let t = ui.ctx().animate_bool_with_time(resp.id, *on, QUICK);
        let h = ui.ctx().animate_bool_with_time(resp.id.with("h"), resp.hovered(), QUICK);
        ui.painter().rect_filled(rect, 13.0, k_raised().lerp_to_gamma(k_active(), h * (1.0 - t)).lerp_to_gamma(p().accent, t));
        let x = egui::lerp(rect.left() + 13.0..=rect.right() - 13.0, t);
        ui.painter().circle_filled(pos2(x, rect.center().y), 9.0, if t > 0.5 { p().on_accent } else { SECONDARY.lerp_to_gamma(TEXT, h) });
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            *on = !*on;
            changed = true;
        }
    });
    ui.add_space(14.0);
    changed
}

/// Segmented choices over the whole column, equal widths (Yapfast's `Choices`). True when one was picked.
fn choices<T: PartialEq + Copy>(ui: &mut Ui, options: &[(T, &str)], value: &mut T) -> bool {
    let mut changed = false;
    let w = ui.available_width();
    let (outer, _) = ui.allocate_exact_size(vec2(w, 36.0), Sense::hover());
    ui.painter().rect_filled(outer, 8.0, k_raised());
    let n = options.len().max(1) as f32;
    let seg = (outer.width() - 6.0) / n;
    for (i, (v, name)) in options.iter().enumerate() {
        let r = Rect::from_min_size(pos2(outer.left() + 3.0 + seg * i as f32, outer.top() + 3.0), vec2(seg, 30.0));
        let resp = ui.interact(r, ui.id().with(("choice", *name)), Sense::click());
        let on = *value == *v;
        let t = ui.ctx().animate_bool_with_time(resp.id, on, QUICK);
        let h = ui.ctx().animate_bool_with_time(resp.id.with("h"), resp.hovered(), QUICK);
        if t > 0.0 || h > 0.0 {
            ui.painter().rect_filled(r, 6.0, k_hover().gamma_multiply(h * (1.0 - t)).lerp_to_gamma(p().accent, t));
        }
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, *name, egui::FontId::new(13.0, th::semibold()), SECONDARY.lerp_to_gamma(TEXT, h).lerp_to_gamma(p().on_accent, t));
        if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() && !on {
            *value = *v;
            changed = true;
        }
    }
    ui.add_space(14.0);
    changed
}

/// The slider track alone, the column's width: a thin track, filled in the accent up to a round knob that grows
/// on hover. (changed, let go): the file is written when the knob is let go.
fn track(ui: &mut Ui, value: &mut f32, range: std::ops::RangeInclusive<f32>) -> (bool, bool) {
    let (lo, hi) = (*range.start(), *range.end());
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::click_and_drag());
    let inner = rect.shrink2(vec2(9.0, 0.0));
    let mut changed = false;
    if resp.dragged() || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let v = (lo + ((p.x - inner.left()) / inner.width()).clamp(0.0, 1.0) * (hi - lo)).round();
            if v != *value {
                *value = v;
                changed = true;
            }
        }
    }
    let t = ((*value - lo) / (hi - lo)).clamp(0.0, 1.0);
    let x = inner.left() + inner.width() * t;
    let cy = rect.center().y;
    ui.painter().rect_filled(Rect::from_min_max(pos2(inner.left(), cy - 2.5), pos2(inner.right(), cy + 2.5)), 2.5, k_raised());
    ui.painter().rect_filled(Rect::from_min_max(pos2(inner.left(), cy - 2.5), pos2(x, cy + 2.5)), 2.5, p().accent);
    let h = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() || resp.dragged(), QUICK);
    ui.painter().circle_filled(pos2(x, cy), 8.0 + 2.0 * h, Color32::WHITE);
    ui.painter().circle_stroke(pos2(x, cy), 8.0 + 2.0 * h, Stroke::new(1.0, Color32::from_black_alpha(90)));
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    (changed, resp.drag_stopped() || resp.clicked())
}

/// A labelled slider: the label on the left and the value on the right, the track under them.
fn slider(ui: &mut Ui, label: &str, value: &mut f32, range: std::ops::RangeInclusive<f32>, suffix: &str) -> (bool, bool) {
    let w = ui.available_width();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        label_block(ui, w - 70.0, label, "");
        ui.allocate_ui_with_layout(vec2(70.0, 20.0), Layout::right_to_left(Align::Center), |ui| {
            ui.label(sb(format!("{:.0}{suffix}", *value), 13.5).color(SECONDARY));
        });
    });
    ui.add_space(2.0);
    let r = track(ui, value, range);
    ui.add_space(12.0);
    r
}

/// A button of the settings' size: 34 px high, rounded 8; primary in the accent.
fn btn(ui: &mut Ui, text: &str, primary: bool) -> egui::Response {
    let (fill, fg) = if primary { (p().accent, p().on_accent) } else { (k_raised(), TEXT) };
    ui.add(egui::Button::new(sb(text, 13.0).color(fg)).fill(fill).corner_radius(8.0).stroke(Stroke::NONE).min_size(vec2(96.0, 34.0)))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A text field in the settings' look: a dark rounded box, `w` wide (the column's when infinite).
fn field(ui: &mut Ui, text: &mut String, hint: &str, w: f32, rows: usize, password: bool) -> egui::Response {
    let w = w.min(ui.available_width());
    egui::Frame::new()
        .fill(k_rail())
        .stroke(Stroke::new(1.0, k_line()))
        .corner_radius(8.0)
        .inner_margin(egui::Margin::symmetric(10, 8))
        .show(ui, |ui| {
            ui.set_width(w - 22.0);
            let te = if rows > 1 { egui::TextEdit::multiline(text).desired_rows(rows) } else { egui::TextEdit::singleline(text).password(password) };
            ui.add(te.frame(egui::Frame::NONE).desired_width(f32::INFINITY).hint_text(rt(hint, 13.5).color(FAINT)).font(egui::FontId::proportional(14.0)))
        })
        .inner
}

/// A device dropdown over the whole column, in the field look.
fn device_pick(ui: &mut Ui, label: &str, list: &[mytune_chat::yapfast_core::voice::Device], cur: &Option<String>, make: fn(String) -> Vc, acts: &mut Vec<Action>) {
    ui.label(rt(label, 14.5).color(TEXT));
    ui.add_space(4.0);
    let shown = list.iter().find(|d| Some(&d.id) == cur.as_ref()).map(|d| d.name.as_str()).unwrap_or("System default");
    let w = ui.available_width();
    ui.scope(|ui| {
        let v = &mut ui.style_mut().visuals.widgets;
        for wv in [&mut v.inactive, &mut v.hovered, &mut v.active, &mut v.open] {
            wv.corner_radius = 8.into();
            wv.weak_bg_fill = k_rail();
            wv.bg_fill = k_rail();
            wv.bg_stroke = Stroke::new(1.0, k_line());
            wv.fg_stroke.color = TEXT;
        }
        v.hovered.bg_stroke = Stroke::new(1.0, k_active());
        v.hovered.weak_bg_fill = k_hover();
        ui.spacing_mut().interact_size.y = 36.0;
        ui.spacing_mut().button_padding = vec2(12.0, 8.0);
        egui::ComboBox::from_id_salt(label).width(w).height(320.0).selected_text(rt(shown, 14.0).color(TEXT)).show_ui(ui, |ui| {
            for d in list {
                if ui.selectable_label(Some(&d.id) == cur.as_ref(), rt(&d.name, 14.0)).clicked() {
                    acts.push(Action::Voice(make(d.id.clone())));
                }
            }
        });
    });
    ui.add_space(14.0);
}

/// The whole screen: the section list on the left, the section on the right, a close button (Escape too).
#[allow(clippy::too_many_arguments)]
pub fn screen(ui: &mut Ui, su: &mut SettingsUi, chat: &Chat, later: &mut Later, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) && s.profile_editor.is_none() {
        su.close();
        return;
    }
    let mut st = chat.settings();
    let before = st.clone();
    let mut save = false;
    egui::Panel::left("settings_nav").exact_size(232.0).frame(egui::Frame::new().fill(k_sidebar()).inner_margin(egui::Margin { left: 16, right: 12, top: 40, bottom: 12 })).show_separator_line(false).show_inside(ui, |ui| {
        ui.label(rt("USER SETTINGS", 11.0).color(MUTED));
        ui.add_space(6.0);
        for (i, name) in PAGES.iter().enumerate() {
            let on = su.page == i;
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
            let t = ctx.animate_bool_with_time(resp.id, resp.hovered(), QUICK);
            ui.painter().rect_filled(r, 6.0, if on { k_active() } else { k_hover().gamma_multiply(t) });
            ui.painter().text(pos2(r.left() + 10.0, r.center().y), egui::Align2::LEFT_CENTER, *name, egui::FontId::proportional(14.5), if on { TEXT } else { SECONDARY });
            if resp.clicked() && !on {
                su.page = i;
                su.meter = None;
                su.camera = None;
                if i == 1 {
                    acts.push(Action::Voice(Vc::Devices));
                }
            }
            ui.add_space(2.0);
        }
        ui.add_space(8.0);
        let (line, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
        ui.painter().rect_filled(line, 0.0, k_line());
        ui.add_space(8.0);
        let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
        let t = ctx.animate_bool_with_time(resp.id, resp.hovered(), QUICK);
        ui.painter().rect_filled(r, 6.0, k_hover().gamma_multiply(t));
        ui.painter().text(pos2(r.left() + 10.0, r.center().y), egui::Align2::LEFT_CENTER, "Log out", egui::FontId::proportional(14.5), DANGER);
        if resp.clicked() {
            su.close();
            later.push(Box::new(|c: &Chat| c.log_in_again()));
        }
    });
    egui::CentralPanel::default().frame(egui::Frame::new().fill(k_chat())).show_inside(ui, |ui| {
        // close button, top right, with "ESC" under it
        let area = ui.max_rect();
        let c = pos2(area.right() - 48.0, area.top() + 52.0);
        let resp = ui.interact(Rect::from_center_size(c, vec2(36.0, 36.0)), egui::Id::new("settings_close"), Sense::click());
        let t = ctx.animate_bool_with_time(resp.id, resp.hovered(), QUICK);
        ui.painter().circle_stroke(c, 18.0, Stroke::new(2.0, SECONDARY.lerp_to_gamma(TEXT, t)));
        glyph(ui.painter(), c, I_CLOSE, 13.0, SECONDARY.lerp_to_gamma(TEXT, t), false);
        ui.painter().text(c + vec2(0.0, 30.0), egui::Align2::CENTER_CENTER, "ESC", egui::FontId::new(11.0, th::semibold()), MUTED);
        if resp.clicked() {
            su.close();
            return;
        }
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            egui::Frame::new().inner_margin(egui::Margin { left: 40, right: 96, top: 40, bottom: 40 }).show(ui, |ui| {
                ui.set_width(ui.available_width().min(600.0));
                ui.label(sb(PAGES[su.page], 20.0).color(TEXT));
                match su.page {
                    0 => account(ui, su, chat, s, tex, asked, want, acts),
                    1 => save |= voice(ui, su, later, s, &mut st, acts),
                    2 => save |= messages(ui, &mut st),
                    3 => save |= notifications(ui, &mut st),
                    4 => save |= sounds(ui, later, &mut st),
                    5 => discord(ui, su, later, s, acts),
                    6 => save |= games(ui, su, s, &mut st, acts),
                    _ => save |= advanced(ui, &mut st),
                }
            });
        });
    });
    // saved at once, except while a slider is held (then when it is let go)
    let dragging = ctx.input(|i| i.pointer.any_down());
    if st != before {
        later.push(Box::new(move |c: &Chat| c.set_settings(st, !dragging)));
    } else if save {
        st.save();
    }
}

#[allow(clippy::too_many_arguments)]
fn account(ui: &mut Ui, su: &mut SettingsUi, chat: &Chat, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    ui.add_space(16.0);
    egui::Frame::new().fill(k_panel()).corner_radius(10.0).inner_margin(egui::Margin::same(16)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (a, _) = ui.allocate_exact_size(vec2(72.0, 72.0), Sense::hover());
            let t = texture(&ctx, tex, asked, s, &s.account_avatar, want);
            avatar(ui, a, t, s.account_name.as_deref().unwrap_or("?"));
            ui.add_space(10.0);
            ui.vertical(|ui| {
                ui.label(sb(s.account_name.as_deref().unwrap_or("Not signed in"), 20.0).color(TEXT));
                if let Some(cs) = &s.custom_status {
                    ui.label(rt(format!("{}{}", cs.emoji.as_deref().map(|e| format!("{e} ")).unwrap_or_default(), cs.text), 13.5).color(SECONDARY));
                }
                ui.label(rt(if s.discord_online { "Online on Discord" } else { "Offline" }, 12.5).color(MUTED));
            });
        });
    });
    if !s.discord_linked {
        return;
    }
    heading(ui, "Status");
    let cur = s.discord_presence.as_deref().unwrap_or("online");
    // Four equal fixed-size tiles painted by hand: egui's Button grows on hover and sizes to its text.
    let gap = 8.0;
    let w = ((ui.available_width() - gap * 3.0) / 4.0).clamp(120.0, 170.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = gap;
        for (key, name, color) in [("online", "Online", LIVE), ("idle", "Idle", Color32::from_rgb(0xf0, 0xb2, 0x32)), ("dnd", "Do Not Disturb", DANGER), ("invisible", "Invisible", FAINT)] {
            let on = cur == key;
            let (rect, r) = ui.allocate_exact_size(vec2(w, 34.0), Sense::click());
            let hover = ui.ctx().animate_bool_with_time(r.id, r.hovered(), 0.12);
            let fill = if on { k_active() } else { k_raised() };
            let p = ui.painter();
            p.rect_filled(rect, 6.0, fill);
            if hover > 0.0 {
                p.rect_filled(rect, 6.0, Color32::from_white_alpha((14.0 * hover) as u8));
            }
            p.rect_stroke(rect, 6.0, Stroke::new(1.0, if on { color } else { k_line() }), egui::StrokeKind::Inside);
            let fg = if on { TEXT } else { SECONDARY };
            let galley = p.layout_no_wrap(name.to_owned(), egui::FontId::proportional(13.0), fg);
            let icon = 16.0;
            let x0 = rect.center().x - (icon + 8.0 + galley.size().x) / 2.0;
            let c = egui::pos2(x0 + icon / 2.0, rect.center().y);
            match key {
                "idle" => {
                    let g = p.layout_no_wrap("\u{E708}".into(), egui::FontId::new(15.0, crate::theme::icons()), color);
                    p.galley(c - g.size() / 2.0, g, color);
                }
                "dnd" => {
                    p.circle_filled(c, 7.0, color);
                    p.rect_filled(egui::Rect::from_center_size(c, vec2(8.0, 2.0)), 1.0, Color32::WHITE);
                }
                "invisible" => {
                    p.circle_stroke(c, 5.5, Stroke::new(2.0, color));
                }
                _ => {
                    p.circle_filled(c, 5.5, color);
                }
            }
            p.galley(egui::pos2(x0 + icon + 8.0, rect.center().y - galley.size().y / 2.0), galley, fg);
            if r.clicked() && !on {
                acts.push(Action::SetStatus(key.into()));
            }
        }
    });
    heading(ui, "Custom status");
    ui.horizontal(|ui| {
        field(ui, &mut su.custom_status, "What's on your mind?", 360.0, 1, false);
        if btn(ui, "Set", true).clicked() && !su.custom_status.trim().is_empty() {
            acts.push(Action::SetCustomStatus { emoji: None, text: su.custom_status.trim().to_owned(), clear_after: ClearAfter::Never });
        }
        if s.custom_status.is_some() && btn(ui, "Clear", false).clicked() {
            acts.push(Action::ClearCustomStatus);
            su.custom_status.clear();
        }
        // emoji and when it clears: the full dialog (extras::status_editor)
        if btn(ui, "Emoji and clear time\u{2026}", false).clicked() {
            ctx.data_mut(|d| d.insert_temp(egui::Id::new("open_status_editor"), true));
        }
    });
    heading(ui, "Profile");
    match &s.profile_editor {
        None => {
            su.profile_open = false;
            if btn(ui, "Edit profile", true).clicked() {
                acts.push(Action::EditProfile);
            }
        }
        Some(ed) if ed.loading() => {
            ui.label(rt("Loading your profile\u{2026}", 13.0).color(MUTED));
        }
        Some(ed) => {
            if !su.profile_open {
                su.profile_open = true;
                su.profile = [ed.name.clone(), ed.pronouns.clone(), ed.bio.clone(), ed.colour.clone()];
            }
            let fields = [(ProfileText::Name, "Display name", false), (ProfileText::Pronouns, "Pronouns", false), (ProfileText::Bio, "About me", true), (ProfileText::Colour, "Banner colour (#rrggbb)", false)];
            for (i, (which, label, multi)) in fields.into_iter().enumerate() {
                ui.label(rt(label, 13.0).color(SECONDARY));
                if field(ui, &mut su.profile[i], label, f32::INFINITY, if multi { 4 } else { 1 }, false).changed() {
                    acts.push(Action::WriteProfile { field: which, text: su.profile[i].clone() });
                }
                ui.add_space(6.0);
            }
            // avatar and banner: pick a picture (a file dialog on its own thread, as uploads do) or remove it
            {
                use mytune_chat::yapfast_core::application::PictureEdit;
                use mytune_chat::yapfast_core::chat::ProfilePicture;
                for (kind, label, now) in [(ProfilePicture::Avatar, "Avatar", &ed.avatar), (ProfilePicture::Banner, "Banner", &ed.banner)] {
                    ui.horizontal(|ui| {
                        ui.label(rt(label, 13.0).color(SECONDARY));
                        ui.add_space(8.0);
                        let state = match now {
                            PictureEdit::Keep => "unchanged",
                            PictureEdit::Reset => "removed (saved on Save changes)",
                            PictureEdit::New(_) => "new picture (saved on Save changes)",
                        };
                        let reading = ed.reading.as_ref().is_some_and(|(k, _)| *k == kind);
                        ui.label(rt(if reading { "reading the picture\u{2026}" } else { state }, 12.5).color(MUTED));
                        if btn(ui, "Change\u{2026}", false).clicked() {
                            let chat = chat.clone();
                            std::thread::spawn(move || {
                                if let Some(path) = rfd::FileDialog::new().set_title(format!("Choose a {}", label.to_lowercase())).add_filter("Pictures", &["png", "jpg", "jpeg", "gif", "webp"]).pick_file() {
                                    chat.dispatch(Action::PickProfilePicture { kind, path });
                                }
                            });
                        }
                        if btn(ui, "Remove", false).clicked() {
                            acts.push(Action::ResetProfilePicture(kind));
                        }
                    });
                    ui.add_space(4.0);
                }
                if btn(ui, "Undo changes", false).clicked() {
                    acts.push(Action::UndoProfileEdits);
                    su.profile_open = false; // fields re-read from the editor
                }
                ui.add_space(6.0);
            }
            if let Some(e) = &ed.error {
                ui.label(rt(e, 12.5).color(DANGER));
            }
            ui.horizontal(|ui| {
                ui.add_enabled_ui(!ed.saving, |ui| {
                    if btn(ui, if ed.saving { "Saving\u{2026}" } else { "Save changes" }, true).clicked() {
                        acts.push(Action::SaveProfile);
                    }
                });
                if btn(ui, "Cancel", false).clicked() {
                    acts.push(Action::CloseProfileEditor);
                }
            });
        }
    }
}

fn voice(ui: &mut Ui, su: &mut SettingsUi, later: &mut Later, s: &AppState, st: &mut Settings, acts: &mut Vec<Action>) -> bool {
    let ctx = ui.ctx().clone();
    let v = &s.voice;
    let mut save = false;
    heading(ui, "Devices");
    if v.microphones.is_empty() && v.speakers.is_empty() {
        if btn(ui, "List devices", false).clicked() {
            acts.push(Action::Voice(Vc::Devices));
        }
        ui.add_space(12.0);
    }
    device_pick(ui, "Microphone", &v.microphones, &v.microphone, Vc::Microphone, acts);
    let mut mv = f32::from(st.microphone_volume);
    let (ch, done) = slider(ui, "Microphone volume", &mut mv, 0.0..=200.0, "%");
    if ch {
        st.microphone_volume = mv as u8;
    }
    save |= done;
    device_pick(ui, "Speakers", &v.speakers, &v.speaker, Vc::Speaker, acts);
    let mut sv = f32::from(st.speaker_volume);
    let (ch, done) = slider(ui, "Speaker volume", &mut sv, 0.0..=200.0, "%");
    if ch {
        st.speaker_volume = sv as u8;
    }
    save |= done;
    if btn(ui, "Refresh devices", false).clicked() {
        acts.push(Action::Voice(Vc::Devices));
    }
    heading(ui, "Video");
    device_pick(ui, "Camera", &v.cameras, &v.camera, Vc::Camera, acts);
    let testing = su.camera.is_some();
    if btn(ui, if testing { "Stop video" } else { "Test video" }, !testing).clicked() {
        if testing {
            su.camera = None;
            su.camera_tex = None;
        } else {
            let feed = Feed::new();
            match mytune_chat::yapfast_core::camera::Camera::open(v.camera.as_deref(), Box::new(feed.mirrored_sink())) {
                Ok(cam) => {
                    su.camera = Some((cam, feed));
                    su.camera_error = None;
                }
                Err(e) => su.camera_error = Some(e.to_string()),
            }
        }
    }
    if let Some(e) = &su.camera_error {
        ui.label(rt(e, 12.5).color(DANGER));
    }
    if let Some((_, feed)) = &su.camera {
        let (r, _) = ui.allocate_exact_size(vec2(400.0, 225.0), Sense::hover());
        let ppp = ctx.pixels_per_point();
        feed.show_at(Some(((400.0 * ppp) as u32 & !1, (225.0 * ppp) as u32 & !1)));
        if let Some(img) = feed.take(|fb| mytune_chat::frame_pixels(fb).map(|((w, h), px)| egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], px))).flatten() {
            match &mut su.camera_tex {
                Some(t) => t.set(img, egui::TextureOptions::LINEAR),
                None => su.camera_tex = Some(ctx.load_texture("camera_test", img, egui::TextureOptions::LINEAR)),
            }
        }
        match &su.camera_tex {
            Some(t) => {
                egui::Image::new(egui::load::SizedTexture::new(t.id(), r.size())).corner_radius(8.0).paint_at(ui, r);
            }
            None => {
                ui.painter().rect_filled(r, 8.0, Color32::BLACK);
                ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, "Starting camera\u{2026}", egui::FontId::proportional(13.0), MUTED);
            }
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(33));
    }
    heading(ui, "Voice");
    toggle(ui, "Turn on your microphone when you join a call", "Off: you join muted.", &mut st.unmute_on_join);
    toggle(ui, "Deafen when you join a call", "", &mut st.deafen_on_join);
    toggle(ui, "Even out your voice", "Raises a quiet voice and lowers a loud one, so you're heard the same.", &mut st.level_voice);
    ui.label(rt("Noise suppression in Discord calls", 14.5).color(TEXT));
    choices(ui, &[(Suppressor::DeepFilterNet, "DeepFilterNet"), (Suppressor::RNNoise, "RNNoise"), (Suppressor::Off, "Off")], &mut st.noise_suppression);
    if st.noise_suppression != Suppressor::Off {
        ui.label(rt("Noise suppression strength", 14.5).color(TEXT));
        choices(ui, &[(Strength::Full, "Full"), (Strength::Medium, "Medium"), (Strength::Light, "Light")], &mut st.noise_strength);
    }
    toggle(ui, "Take out your room's echo in Discord calls", "", &mut st.reduce_room_echo);
    let mut sb_vol = s.soundboard_volume;
    let (_, done) = slider(ui, "Soundboard volume", &mut sb_vol, 0.0..=100.0, "%");
    if done {
        acts.push(Action::SoundboardVolume(sb_vol));
    }
    join_sounds(ui, s, acts);
    heading(ui, "Input mode");
    choices(ui, &[(InputMode::VoiceActivity, "Voice activity"), (InputMode::PushToTalk, "Push to talk")], &mut st.input_mode);
    if st.input_mode == InputMode::PushToTalk {
        su.meter = None;
        ui.label(rt("Shortcut", 14.5).color(TEXT));
        ui.horizontal(|ui| {
            let name = st.push_to_talk_key.as_ref().map_or("No key yet", |k| k.name.as_str());
            egui::Frame::new().fill(k_raised()).corner_radius(6.0).inner_margin(egui::Margin::symmetric(12, 6)).show(ui, |ui| {
                ui.label(sb(name, 14.0).color(TEXT));
            });
            let recording = mytune_chat::ptt::recording();
            if btn(ui, if recording { "Press a key\u{2026}" } else { "Record keybind" }, !recording).clicked() {
                let (ctx, on) = (ctx.clone(), !recording);
                later.push(Box::new(move |c: &Chat| record_key(&ctx, c, on)));
            }
        });
        ui.label(rt("Works in every app, games included.", 12.5).color(MUTED));
        ui.add_space(8.0);
        let mut d = st.push_to_talk_release_delay as f32;
        let (ch, done) = slider(ui, "Release delay", &mut d, 0.0..=2000.0, " ms");
        if ch {
            st.push_to_talk_release_delay = d as u32;
        }
        save |= done;
    } else {
        // the live level after suppression, against the threshold, as Discord's sensitivity bar shows it
        if su.meter.is_none() {
            su.meter = Some(mytune_chat::yapfast_core::discord_audio::GateMeter::start(v.microphone.as_deref()));
        }
        let mut sens = st.input_sensitivity as f32;
        let (ch, done) = slider(ui, "Input sensitivity", &mut sens, -80.0..=0.0, " dB");
        if ch {
            st.input_sensitivity = sens as i32;
        }
        save |= done;
        if let Some(m) = &su.meter {
            let level = m.level().clamp(0.0, 1.0);
            let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width().min(420.0), 8.0), Sense::hover());
            ui.painter().rect_filled(bar, 4.0, k_raised());
            let threshold = (sens + 80.0) / 80.0;
            let fill = Rect::from_min_size(bar.min, vec2(bar.width() * level, bar.height()));
            ui.painter().rect_filled(fill, 4.0, if level >= threshold { LIVE } else { th::WARNING });
            let x = bar.left() + bar.width() * threshold;
            ui.painter().line_segment([pos2(x, bar.top() - 3.0), pos2(x, bar.bottom() + 3.0)], Stroke::new(2.0, TEXT));
            ui.label(rt("Speak: the bar turns green where your voice is sent.", 12.5).color(MUTED));
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
    save
}

/// Records the next key pressed as push-to-talk, waiting on a thread so the page stays live.
fn record_key(ctx: &egui::Context, chat: &Chat, on: bool) {
    let chat = chat.clone();
    chat.record_key(on);
    if on {
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            for _ in 0..300 {
                std::thread::sleep(std::time::Duration::from_millis(50));
                if chat.recorded_key().is_some() || !mytune_chat::ptt::recording() {
                    break;
                }
            }
            chat.record_key(false);
            ctx.request_repaint();
        });
    }
}

fn messages(ui: &mut Ui, st: &mut Settings) -> bool {
    heading(ui, "Display");
    ui.label(rt("Text size", 14.5).color(TEXT));
    choices(ui, &[(0u8, "Small"), (1, "Normal"), (2, "Large")], &mut st.text_size);
    toggle(ui, "Show times on a 12-hour clock", "", &mut st.twelve_hour_clock);
    toggle(ui, "Show seconds in message times", "", &mut st.clock_seconds);
    toggle(ui, "Show link previews", "Embeds under links, where Discord sends them.", &mut st.link_previews);
    heading(ui, "Writing");
    toggle(ui, "Mention the author when you reply", "", &mut st.reply_ping);
    toggle(ui, "Take tracking out of links you send", "", &mut st.strip_tracking);
    toggle(ui, "Let others see when you're typing", "", &mut st.share_typing);
    heading(ui, "People");
    toggle(ui, "Hide messages from people you blocked", "Off: they show collapsed.", &mut st.hide_blocked);
    true
}

fn notifications(ui: &mut Ui, st: &mut Settings) -> bool {
    heading(ui, "Desktop");
    toggle(ui, "Show desktop notifications", "A Windows notification for a DM, a mention or a call while MyTune is behind other windows. Not in Do Not Disturb.", &mut st.notifications);
    ui.add_enabled_ui(st.notifications, |ui| {
        toggle(ui, "Show what messages say", "Not available yet: a notification says who wrote and how many messages.", &mut st.notification_text);
    });
    toggle(ui, "Flash the taskbar", "When a message comes while MyTune is not in front.", &mut st.flash_taskbar);
    true
}

const SETS: [(SoundSet, &str); 6] = [(SoundSet::NewKalimba, "New Kalimba"), (SoundSet::Felt, "Felt piano"), (SoundSet::Marimba, "Soft marimba"), (SoundSet::Rhodes, "Mellow Rhodes"), (SoundSet::Glow, "Glow"), (SoundSet::Flute, "Flute")];
const SOUNDS: [(Sound, &str); 12] = [
    (Sound::Join, "Someone joins the call"),
    (Sound::Leave, "Someone leaves the call"),
    (Sound::Mute, "Muting"),
    (Sound::Unmute, "Unmuting"),
    (Sound::Deafen, "Deafening"),
    (Sound::Undeafen, "Undeafening"),
    (Sound::Message, "A message"),
    (Sound::Mention, "A mention"),
    (Sound::ViewerJoin, "Someone starts watching a stream"),
    (Sound::ViewerLeave, "Someone stops watching a stream"),
    (Sound::StreamStart, "Someone starts streaming"),
    (Sound::StreamStop, "Someone stops streaming"),
];

fn play_button(ui: &mut Ui, later: &mut Later, sound: Sound) {
    let (r, resp) = ui.allocate_exact_size(vec2(28.0, 28.0), Sense::click());
    let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), QUICK);
    ui.painter().rect_filled(r, 14.0, k_raised().lerp_to_gamma(k_active(), t));
    glyph(ui.painter(), r.center(), I_PLAY, 11.0, TEXT, false);
    if resp.on_hover_text("Play").clicked() {
        later.push(Box::new(move |c: &Chat| c.preview_sound(sound)));
    }
}

fn sounds(ui: &mut Ui, later: &mut Later, st: &mut Settings) -> bool {
    let mut save = false;
    heading(ui, "Sound set");
    if choices(ui, &SETS, &mut st.sound_set) {
        let new = st.clone();
        later.push(Box::new(move |c: &Chat| {
            c.set_settings(new, true);
            c.preview_sound(Sound::Join);
        }));
    }
    let mut vol = f32::from(st.sound_volume);
    let (ch, done) = slider(ui, "Volume", &mut vol, 0.0..=200.0, "%");
    if ch {
        st.sound_volume = vol as u8;
    }
    if done {
        let new = st.clone();
        later.push(Box::new(move |c: &Chat| {
            c.set_settings(new, true);
            c.preview_sound(Sound::Message);
        }));
    }
    save |= done;
    toggle(ui, "Adjust each sound", "", &mut st.adjust_each_sound);
    for (sound, name) in SOUNDS {
        ui.horizontal(|ui| {
            play_button(ui, later, sound);
            ui.label(rt(name, 14.0).color(SECONDARY));
        });
        if st.adjust_each_sound {
            let mut own = f32::from(st.each_sound.get(&sound).copied().unwrap_or(100));
            let (ch, done) = track(ui, &mut own, 0.0..=200.0);
            if ch {
                if own as u8 == 100 {
                    st.each_sound.remove(&sound);
                } else {
                    st.each_sound.insert(sound, own as u8);
                }
            }
            save |= done;
        }
        ui.add_space(4.0);
    }
    save
}

fn discord(ui: &mut Ui, su: &mut SettingsUi, later: &mut Later, s: &AppState, acts: &mut Vec<Action>) {
    heading(ui, "Linked account");
    if s.discord_linked {
        ui.label(rt(format!("Linked as {}", s.account_name.as_deref().unwrap_or("?")), 14.5).color(TEXT));
        ui.label(rt("The login is kept in Windows' credential store, shared with the Yapfast app.", 12.5).color(MUTED));
        ui.add_space(8.0);
        if btn(ui, "Unlink Discord", false).clicked() {
            acts.push(Action::UnlinkDiscord);
        }
    } else {
        if btn(ui, "Link with a QR code", true).clicked() {
            su.close();
            later.push(Box::new(|c: &Chat| {
                c.log_in_again();
                c.dispatch(Action::LinkDiscord);
            }));
        }
        heading(ui, "Or with a token");
        ui.horizontal(|ui| {
            field(ui, &mut su.token, "Discord token", 360.0, 1, true);
            if btn(ui, "Link", false).clicked() && !su.token.trim().is_empty() {
                acts.push(Action::LinkDiscordToken(std::mem::take(&mut su.token).trim().to_owned()));
            }
        });
    }
}

fn games(ui: &mut Ui, su: &mut SettingsUi, s: &AppState, st: &mut Settings, acts: &mut Vec<Action>) -> bool {
    let mut save = false;
    heading(ui, "Sharing");
    let mut show = s.discord_show_game;
    if toggle(ui, "Share the game you play", "Shows \"Playing ...\" on your Discord profile.", &mut show) {
        acts.push(Action::ShowGame(show));
    }
    heading(ui, "Your own games");
    ui.label(rt("Games Discord doesn't know, by their program.", 12.5).color(MUTED));
    let mut remove = None;
    for (i, (name, exe)) in st.game_executables.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.label(sb(name, 14.0).color(TEXT));
            ui.label(rt(exe, 12.5).color(MUTED));
            if btn(ui, "Remove", false).clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        st.game_executables.remove(i);
        save = true;
    }
    ui.horizontal(|ui| {
        field(ui, &mut su.game_name, "Name", 200.0, 1, false);
        field(ui, &mut su.game_exe, "game.exe", 220.0, 1, false);
        if btn(ui, "Add", true).clicked() && !su.game_name.trim().is_empty() && !su.game_exe.trim().is_empty() {
            st.game_executables.push((std::mem::take(&mut su.game_name).trim().to_owned(), std::mem::take(&mut su.game_exe).trim().to_owned()));
            save = true;
        }
    });
    save
}

fn advanced(ui: &mut Ui, st: &mut Settings) -> bool {
    heading(ui, "Streaming");
    toggle(ui, "Encode streams with the GPU", "Uses NVENC, AMF or Quick Sync when there is one; off encodes on the CPU.", &mut st.gpu_encoding);
    ui.add_space(6.0);
    ui.label(rt("Start with Windows, closing to the tray and the theme are in MyTune's own Settings.", 12.5).color(MUTED));
    true
}

/// Discord's user bar under the room list: avatar with status, name and custom status, then microphone and
/// headphones (the call's, or how the next call starts) and the gear that opens these settings.
#[allow(clippy::too_many_arguments)]
pub fn user_bar(ui: &mut Ui, chat: &Chat, later: &mut Later, su: &mut SettingsUi, s: &AppState, tex: &mut HashMap<String, TextureHandle>, asked: &mut HashSet<String>, want: &mut Vec<String>, acts: &mut Vec<Action>) {
    let ctx = ui.ctx().clone();
    let v = &s.voice;
    let (unmute, deafen) = chat.with_settings(|st| (st.unmute_on_join, st.deafen_on_join));
    let (muted, deafened) = if v.active() { (v.muted, v.deafened) } else { (!unmute || deafen, deafen) };
    let mut anchor = egui::Rect::NOTHING;
    ui.horizontal(|ui| {
        let (a, ar) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
        anchor = a;
        if ar.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_text("Set your status").clicked() {
            ctx.data_mut(|d| { let o = d.get_temp::<bool>(egui::Id::new("status_menu")).unwrap_or(false); d.insert_temp(egui::Id::new("status_menu"), !o) });
        }
        let t = texture(&ctx, tex, asked, s, &s.account_avatar, want);
        avatar(ui, a, t, s.account_name.as_deref().unwrap_or("?"));
        let presence = match s.discord_presence.as_deref() {
            Some("idle") => Some(mytune_chat::yapfast_core::chat::Presence::Idle),
            Some("dnd") => Some(mytune_chat::yapfast_core::chat::Presence::DoNotDisturb),
            Some("invisible") => Some(mytune_chat::yapfast_core::chat::Presence::Offline),
            _ if s.discord_online => Some(mytune_chat::yapfast_core::chat::Presence::Online),
            _ => None,
        };
        super::header::status_dot(ui, a, presence, k_panel());
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.set_width(ui.available_width() - 3.0 * 40.0 - 4.0);
            ui.add(egui::Label::new(sb(s.account_name.as_deref().unwrap_or(""), 13.5).color(TEXT)).truncate());
            let line = s.custom_status.as_ref().map(|c| c.text.clone()).unwrap_or_else(|| match s.discord_presence.as_deref() {
                Some("idle") => "Idle".into(),
                Some("dnd") => "Do Not Disturb".into(),
                Some("invisible") => "Invisible".into(),
                _ => "Online".into(),
            });
            ui.add(egui::Label::new(rt(line, 11.5).color(MUTED)).truncate());
        });
        let mut b = |ui: &mut Ui, g: &str, slash: bool, red: bool, tip: &str| {
            let (r, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), Sense::click());
            let t = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), QUICK);
            ui.painter().rect_filled(r, 6.0, k_hover().gamma_multiply(t));
            glyph(ui.painter(), r.center(), g, 16.0, if red { DANGER } else { SECONDARY.lerp_to_gamma(TEXT, t) }, slash);
            resp.on_hover_text(tip).clicked()
        };
        if b(ui, "\u{E720}", muted, muted, if muted { "Unmute" } else { "Mute" }) {
            if v.active() {
                acts.push(Action::Voice(Vc::Mute(!v.muted)));
            } else {
                // out of a call: how the next one starts, as Yapfast's toggle_on_join
                let mut st = chat.settings();
                if st.deafen_on_join {
                    st.deafen_on_join = false;
                    st.unmute_on_join = true;
                } else {
                    st.unmute_on_join = !st.unmute_on_join;
                }
                later.push(Box::new(move |c: &Chat| c.set_settings(st, true)));
            }
        }
        if b(ui, "\u{E7F6}", deafened, deafened, if deafened { "Undeafen" } else { "Deafen" }) {
            if v.active() {
                acts.push(Action::Voice(Vc::Deafen(!v.deafened)));
            } else {
                let mut st = chat.settings();
                st.deafen_on_join = !st.deafen_on_join;
                later.push(Box::new(move |c: &Chat| c.set_settings(st, true)));
            }
        }
        if b(ui, "\u{E713}", false, false, "User settings") {
            su.open(0);
        }
    });
    status_menu(&ctx, s, anchor, acts);
}

/// The status menu above the user bar (a click on the avatar): Online, Idle, Do Not Disturb, Invisible, and a
/// custom status with a text that clears itself at midnight.
fn status_menu(ctx: &egui::Context, s: &AppState, anchor: egui::Rect, acts: &mut Vec<Action>) {
    use mytune_chat::yapfast_core::application::ClearAfter;
    let id = egui::Id::new("status_menu");
    if !ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false) || anchor == egui::Rect::NOTHING {
        return;
    }
    let text_id = egui::Id::new("status_text");
    let mut text: String = ctx.data(|d| d.get_temp(text_id)).unwrap_or_else(|| s.custom_status.as_ref().map(|c| c.text.clone()).unwrap_or_default());
    let mut close = false;
    let resp = egui::Area::new(egui::Id::new("status_menu_area")).order(egui::Order::Foreground).fixed_pos(egui::pos2(anchor.left(), anchor.top() - 12.0)).pivot(egui::Align2::LEFT_BOTTOM).show(ctx, |ui| {
        egui::Frame::new().fill(k_raised()).stroke(Stroke::new(1.0, k_edge())).corner_radius(10.0).shadow(egui::Shadow { offset: [0, 10], blur: 28, spread: 0, color: Color32::from_black_alpha(0x66) }).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
            ui.set_width(248.0);
            let cur = s.discord_presence.as_deref().unwrap_or("online");
            for (key, label, color) in [("online", "Online", LIVE), ("idle", "Idle", super::header::presence_color(Some(mytune_chat::yapfast_core::chat::Presence::Idle))), ("dnd", "Do Not Disturb", DANGER), ("invisible", "Invisible", FAINT)] {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
                if resp.hovered() || cur == key {
                    ui.painter().rect_filled(r, 6.0, k_active());
                }
                ui.painter().circle_filled(egui::pos2(r.left() + 18.0, r.center().y), 6.0, color);
                ui.painter().text(egui::pos2(r.left() + 36.0, r.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::new(13.5, th::semibold()), TEXT);
                if resp.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    acts.push(Action::SetStatus(key.to_owned()));
                    close = true;
                }
            }
            ui.add_space(6.0);
            ui.painter().hline(ui.max_rect().x_range(), ui.cursor().top(), Stroke::new(1.0, k_edge()));
            ui.add_space(8.0);
            ui.label(sb("CUSTOM STATUS", 11.0).color(MUTED));
            ui.add(egui::TextEdit::singleline(&mut text).desired_width(f32::INFINITY).hint_text(rt("What's happening?", 13.5).color(FAINT)));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if pill(ui, "Set", true).clicked() && !text.trim().is_empty() {
                    acts.push(Action::SetCustomStatus { emoji: None, text: text.trim().to_owned(), clear_after: ClearAfter::Today });
                    close = true;
                }
                if s.custom_status.is_some() && pill(ui, "Clear", false).clicked() {
                    acts.push(Action::ClearCustomStatus);
                    text.clear();
                    close = true;
                }
                // emoji and when it clears: the full dialog (extras::status_editor)
                if pill(ui, "More\u{2026}", false).clicked() {
                    ctx.data_mut(|d| d.insert_temp(egui::Id::new("open_status_editor"), true));
                    close = true;
                }
            });
        });
    });
    ctx.data_mut(|d| d.insert_temp(text_id, text));
    let outside = resp.response.clicked_elsewhere() && !ctx.input(|i| i.pointer.interact_pos().is_some_and(|p| anchor.contains(p)));
    if close || outside || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        ctx.data_mut(|d| d.insert_temp(id, false));
    }
}

/// Discord's join sound (Nitro): the soundboard sound that plays when this account joins a call, for every
/// server and for the server open now. The engine lists the sounds the account may use (`sound_catalog`).
fn join_sounds(ui: &mut Ui, s: &AppState, acts: &mut Vec<Action>) {
    heading(ui, "Join sound");
    let Some(catalog) = &s.sound_catalog else {
        if btn(ui, "Choose a join sound\u{2026}", false).clicked() {
            acts.push(Action::LoadSoundCatalog);
        }
        return;
    };
    let name_of = |id: &str| catalog.iter().flat_map(|g| &g.sounds).find(|x| x.id == id).map(|x| format!("{} {}", x.emoji, x.name).trim().to_owned()).unwrap_or_else(|| "a sound".to_owned());
    let here = match &s.navigation {
        Navigation::Server(id) => s.servers.iter().find(|x| &x.id == id).map(|x| (Some(id.clone()), format!("In {}", x.name))),
        _ => None,
    };
    for (server, label) in std::iter::once((None, "Every server".to_owned())).chain(here) {
        let now = s.join_sounds.iter().find(|j| j.server == server).map(|j| j.sound.clone());
        ui.horizontal(|ui| {
            ui.label(rt(&label, 13.5).color(SECONDARY));
            ui.add_space(8.0);
            egui::ComboBox::from_id_salt(("join_sound", &server)).selected_text(now.as_deref().map_or("None".to_owned(), name_of)).width(240.0).show_ui(ui, |ui| {
                if ui.selectable_label(now.is_none(), "None").clicked() {
                    acts.push(Action::SetJoinSound { server: server.clone(), sound: None });
                }
                for g in catalog.iter() {
                    ui.label(sb(&g.name, 11.5).color(MUTED));
                    for snd in &g.sounds {
                        let on = now.as_deref() == Some(snd.id.as_str());
                        if ui.selectable_label(on, format!("{} {}", snd.emoji, snd.name).trim()).clicked() && !on {
                            acts.push(Action::SetJoinSound { server: server.clone(), sound: Some(snd.id.clone()) });
                        }
                    }
                }
            });
        });
        ui.add_space(4.0);
    }
}
