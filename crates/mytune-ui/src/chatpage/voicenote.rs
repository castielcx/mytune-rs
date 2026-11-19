//! Discord voice messages: the composer's mic button records one (a bar over the composer shows the time and
//! level, with Send and Cancel), and a received `voice-message.ogg` gets a play button in place of a file chip.
//! Recording and playing run on the engine's threads (`yapfast_core::voice_note`); the state here lives on the
//! UI thread only.
// shortcut: no waveform on received messages (the engine does not keep it), a progress bar instead; parse
// `waveform` and `duration_secs` in yapfast-discord's attachment when the bars are wanted.

use super::voice::{glyph, k_edge, k_panel, k_raised};
use super::*;
use egui::{pos2, Stroke};
use mytune_chat::yapfast_core::chat::{Attachment, ChatMessage};
use mytune_chat::yapfast_core::voice_note::{seconds_of, Player, Recorder};
use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A recording shorter than this is thrown away on Send: a slipped click.
const SHORTEST: f32 = 0.5;
/// How long a clicked voice message may take to download before the wait is dropped.
const DOWNLOAD_WAIT: Duration = Duration::from_secs(20);

struct Playing {
    /// The message's item and the attachment's index.
    key: (String, usize),
    path: PathBuf,
    /// The file's size as Discord gave it, to know the download is whole.
    size: Option<u64>,
    /// None while the file downloads.
    player: Option<(Player, f32)>,
    since: Instant,
}

#[derive(Default)]
struct State {
    /// The recording and the room it is for.
    rec: Option<(Recorder, String)>,
    playing: Option<Playing>,
    microphone: Option<String>,
    speaker: Option<String>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

fn dir() -> PathBuf {
    let d = std::env::temp_dir().join("mytune-voice");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn clock(seconds: f32) -> String {
    let s = seconds.max(0.0) as u32;
    format!("{}:{:02}", s / 60, s % 60)
}

/// Once a frame: the chosen devices, and a recording ends when its room is left.
pub fn frame(s: &AppState) {
    STATE.with_borrow_mut(|st| {
        st.microphone = s.voice.microphone.clone();
        st.speaker = s.voice.speaker.clone();
        if st.rec.as_ref().is_some_and(|(_, room)| s.selected.as_ref() != Some(room)) {
            st.rec = None;
        }
    });
}

pub fn recording() -> bool {
    STATE.with_borrow(|st| st.rec.is_some())
}

/// The mic button: starts a recording for `room`, or throws the running one away.
pub fn toggle(room: &str) {
    STATE.with_borrow_mut(|st| {
        st.rec = match st.rec.take() {
            Some(_) => None,
            None => {
                st.playing = None;
                Some((Recorder::start(st.microphone.as_deref()), room.to_owned()))
            }
        };
    });
}

/// The bar over the composer while a voice message records. Returns whether it drew (the reply bar then stays out).
pub fn bar(ui: &mut Ui, acts: &mut Vec<Action>) -> bool {
    let Some((seconds, level, failed)) = STATE.with_borrow(|st| st.rec.as_ref().map(|(r, _)| (r.seconds(), r.level(), r.failed()))) else {
        return false;
    };
    let (mut send, mut cancel) = (false, false);
    egui::Frame::new().fill(k_panel()).corner_radius(egui::CornerRadius { nw: 12, ne: 12, sw: 0, se: 0 }).inner_margin(egui::Margin::symmetric(14, 7)).show(ui, |ui| {
        ui.horizontal(|ui| {
            if failed {
                ui.label(rt("The microphone could not be opened.", 13.0).color(DANGER));
            } else {
                let (dot, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                // the dot blinks once a second
                let on = ui.input(|i| i.time).fract() < 0.6;
                ui.painter().circle_filled(dot.center(), 4.5, if on { DANGER } else { DANGER.gamma_multiply(0.35) });
                ui.label(rt("Recording", 13.0).color(MUTED));
                ui.label(sb(clock(seconds), 13.0).color(TEXT));
                // how loud the microphone is
                let (r, _) = ui.allocate_exact_size(vec2(120.0, 6.0), Sense::hover());
                ui.painter().rect_filled(r, 3.0, k_raised());
                let mut lit = r;
                lit.set_width(r.width() * level.sqrt().clamp(0.0, 1.0));
                ui.painter().rect_filled(lit, 3.0, p().accent);
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !failed && pill(ui, "Send", true).clicked() {
                    send = true;
                }
                if ui.add(egui::Button::new(rt("\u{E711}", 11.0).family(th::icons()).color(MUTED)).frame(false)).on_hover_text("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
    });
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        cancel = true;
    }
    ui.ctx().request_repaint_after(Duration::from_millis(50));
    if send || cancel {
        let rec = STATE.with_borrow_mut(|st| st.rec.take());
        if let (true, Some((rec, _))) = (send, rec) {
            // stopping waits for the recorder's last 20 ms read
            if let Some(done) = rec.finish().filter(|d| d.seconds >= SHORTEST) {
                let millis = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis());
                let path = dir().join(format!("rec-{millis}.ogg"));
                if std::fs::write(&path, &done.ogg).is_ok() {
                    acts.push(Action::SendVoice { path, seconds: done.seconds, waveform: done.waveform });
                }
            }
        }
    }
    true
}

/// Whether an attachment is a voice message: Discord's clients always name the file so.
pub fn is_voice(a: &Attachment) -> bool {
    a.name == "voice-message.ogg" && !a.image && !a.video
}

/// A received voice message: play or stop, a progress bar and the time. The first click downloads the file.
pub fn chip(ui: &mut Ui, m: &ChatMessage, i: usize, a: &Attachment, acts: &mut Vec<Action>) -> egui::Response {
    let key = (m.item.clone(), i);
    let (rect, resp) = ui.allocate_exact_size(vec2(300.0_f32.min(ui.available_width()), 52.0), Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    ui.painter().rect(rect, 26.0, if resp.hovered() { k_active() } else { k_raised() }, Stroke::new(1.0, k_edge()), egui::StrokeKind::Inside);
    // (played, total) while it plays; None while it loads; not this one: idle
    let state = STATE.with_borrow_mut(|st| {
        let Some(pl) = st.playing.as_mut().filter(|pl| pl.key == key) else { return None };
        if pl.player.is_none() {
            let whole = std::fs::metadata(&pl.path).is_ok_and(|md| md.len() > 0 && pl.size.is_none_or(|size| md.len() == size));
            if whole {
                if let Ok(ogg) = std::fs::read(&pl.path) {
                    let total = seconds_of(&ogg);
                    pl.player = Some((Player::start(ogg, st.speaker.as_deref()), total));
                }
            } else if pl.since.elapsed() > DOWNLOAD_WAIT {
                st.playing = None;
                return None;
            }
        }
        let pl = st.playing.as_ref()?;
        match &pl.player {
            Some((player, _)) if player.ended() => {
                st.playing = None;
                None
            }
            Some((player, total)) => Some(Some((player.seconds(), *total))),
            None => Some(None),
        }
    });
    let active = state.is_some();
    let c = pos2(rect.left() + 26.0, rect.center().y);
    ui.painter().circle_filled(c, 16.0, p().accent);
    glyph(ui.painter(), c, if active { "\u{E71A}" } else { "\u{E768}" }, 13.0, Color32::BLACK, false);
    let track = egui::Rect::from_min_max(pos2(rect.left() + 52.0, rect.center().y - 2.0), pos2(rect.right() - 76.0, rect.center().y + 2.0));
    ui.painter().rect_filled(track, 2.0, k_edge());
    let text = match state {
        Some(Some((played, total))) => {
            let mut lit = track;
            lit.set_width(track.width() * if total > 0.0 { (played / total).clamp(0.0, 1.0) } else { 0.0 });
            ui.painter().rect_filled(lit, 2.0, p().accent);
            format!("{} / {}", clock(played), clock(total))
        }
        Some(None) => "Loading".to_owned(),
        None => "Voice".to_owned(),
    };
    ui.painter().text(pos2(rect.right() - 16.0, rect.center().y), egui::Align2::RIGHT_CENTER, text, egui::FontId::proportional(11.5), MUTED);
    if active {
        ui.ctx().request_repaint_after(Duration::from_millis(100));
    }
    if resp.clicked() {
        STATE.with_borrow_mut(|st| {
            if active {
                st.playing = None;
                return;
            }
            let path = dir().join(format!("{}-{i}.ogg", m.item.replace(|ch: char| !ch.is_ascii_alphanumeric(), "_")));
            if !path.exists() {
                acts.push(Action::SaveAttachment { item: m.item.clone(), index: i, path: path.clone() });
            }
            st.playing = Some(Playing { key, path, size: a.size, player: None, since: Instant::now() });
        });
    }
    resp
}
