//! What plays on this PC and its buttons, through Windows' media session: a SoundCloud tab in a browser,
//! Spotify, or any other player Windows knows. One thread asks Windows every 2 s, only while something drew
//! it lately. The Music page (`page`) shows every player, the song's picture, its progress and the buttons.

use eframe::egui;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use windows::Media::Control::{GlobalSystemMediaTransportControlsSession as Session, GlobalSystemMediaTransportControlsSessionManager as Manager, GlobalSystemMediaTransportControlsSessionMediaProperties as Properties, GlobalSystemMediaTransportControlsSessionPlaybackControls as Controls, GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status};

#[derive(Clone, Default, PartialEq)]
pub struct Now {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    /// The player, as Windows names it ("chrome.exe", "SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify").
    pub app: String,
    /// Seconds played when it was read, and the song's length; 0 when the player does not say.
    pub position: f64,
    pub length: f64,
    /// Shuffle on or off, repeat (0 off, 1 this song, 2 the list) and the speed (1.0 normal); `None`
    /// when the player does not offer that control.
    pub shuffle: Option<bool>,
    pub repeat: Option<u8>,
    pub rate: Option<f64>,
}

pub enum Cmd {
    Toggle,
    Next,
    Previous,
    /// Show and control this player (its `Now::app`) instead of the one Windows calls current.
    Pick(String),
    Shuffle(bool),
    /// 0 off, 1 this song, 2 the list.
    Repeat(u8),
    Rate(f64),
}

struct Shared {
    /// Every player with a song, the shown one first.
    all: Mutex<Vec<Now>>,
    /// When `all` was read: the page counts the progress on from here.
    read_at: Mutex<Instant>,
    /// The shown song's picture, with the song it belongs to.
    cover: Mutex<Option<(String, egui::ColorImage)>>,
    /// When something last drew a song, in ms since `start`.
    seen: AtomicU64,
    start: Instant,
    tx: Mutex<mpsc::Sender<Cmd>>,
}

static SHARED: OnceLock<Shared> = OnceLock::new();

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Where the song is and how long it is, in seconds. Players report the position only now and then, with
/// the time they did: a playing song is counted on from there.
fn timeline(session: &Session, playing: bool) -> windows::core::Result<(f64, f64)> {
    let t = session.GetTimelineProperties()?;
    let length = (t.EndTime()?.Duration - t.StartTime()?.Duration) as f64 / 1e7;
    let mut position = t.Position()?.Duration as f64 / 1e7;
    if playing {
        // Windows counts 100 ns steps from 1601, Unix seconds from 1970
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0) + 11_644_473_600.0;
        let since = now - t.LastUpdatedTime()?.UniversalTime as f64 / 1e7;
        if since > 0.0 && since < length {
            position += since;
        }
    }
    Ok((position.clamp(0.0, length.max(0.0)), length.max(0.0)))
}

fn read(session: &Session) -> windows::core::Result<(Now, Properties)> {
    let p = session.TryGetMediaPropertiesAsync()?.join()?;
    let info = session.GetPlaybackInfo()?;
    let playing = info.PlaybackStatus()? == Status::Playing;
    let (position, length) = timeline(session, playing).unwrap_or((0.0, 0.0));
    // each of these only where the player says it can be changed
    let can = info.Controls().ok();
    let offers = |f: fn(&Controls) -> windows::core::Result<bool>| can.as_ref().is_some_and(|c| f(c).unwrap_or(false));
    let shuffle = offers(Controls::IsShuffleEnabled).then(|| info.IsShuffleActive().and_then(|r| r.Value()).unwrap_or(false));
    let repeat = offers(Controls::IsRepeatEnabled).then(|| info.AutoRepeatMode().and_then(|r| r.Value()).map(|m| m.0 as u8).unwrap_or(0));
    let rate = offers(Controls::IsPlaybackRateEnabled).then(|| info.PlaybackRate().and_then(|r| r.Value()).unwrap_or(1.0));
    Ok((Now { title: p.Title()?.to_string(), artist: p.Artist()?.to_string(), playing, app: session.SourceAppUserModelId()?.to_string(), position, length, shuffle, repeat, rate }, p))
}

/// The song's picture as the player gave it to Windows, at most 512 px.
fn cover(p: &Properties) -> Option<egui::ColorImage> {
    use windows::Storage::Streams::DataReader;
    let stream = p.Thumbnail().ok()?.OpenReadAsync().ok()?.join().ok()?;
    let size = stream.Size().ok()?;
    if size == 0 || size > 16 * 1024 * 1024 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size as u32).ok()?.join().ok()?;
    let mut data = vec![0u8; size as usize];
    reader.ReadBytes(&mut data).ok()?;
    let pic = mytune_chat::yapfast_core::chat::decode_picture(&data, 512, 512)?;
    Some(egui::ColorImage::from_rgba_unmultiplied([pic.width as usize, pic.height as usize], &pic.rgba))
}

fn run(ctx: egui::Context, rx: mpsc::Receiver<Cmd>) {
    let shared = SHARED.get().expect("set before the thread starts");
    // WinRT needs the thread in an apartment; failing only means every call below fails and nothing shows
    let _ = unsafe { windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED) };
    let manager = Manager::RequestAsync().and_then(|m| m.join());
    let mut pick: Option<String> = None;
    loop {
        let cmd = match rx.recv_timeout(Duration::from_secs(2)) {
            Ok(cmd) => Some(cmd),
            Err(mpsc::RecvTimeoutError::Timeout) => None,
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        };
        let idle = shared.start.elapsed().as_millis() as u64 - shared.seen.load(Ordering::Relaxed) > 5000;
        if cmd.is_none() && idle {
            continue;
        }
        let Ok(manager) = &manager else { continue };
        if let Some(Cmd::Pick(app)) = &cmd {
            pick = Some(app.clone());
        }
        // every player Windows knows; the picked one first, else the one Windows calls current
        let mut sessions: Vec<Session> = manager.GetSessions().map(|v| v.into_iter().collect()).unwrap_or_default();
        let app_of = |s: &Session| s.SourceAppUserModelId().map(|a| a.to_string()).unwrap_or_default();
        let first = pick.clone().filter(|p| sessions.iter().any(|s| &app_of(s) == p)).or_else(|| manager.GetCurrentSession().ok().map(|s| app_of(&s)));
        if let Some(first) = &first {
            sessions.sort_by_key(|s| &app_of(s) != first);
        }
        if let (Some(s), Some(cmd)) = (sessions.first(), &cmd) {
            let sent = match cmd {
                Cmd::Toggle => Some(s.TryTogglePlayPauseAsync()),
                Cmd::Next => Some(s.TrySkipNextAsync()),
                Cmd::Previous => Some(s.TrySkipPreviousAsync()),
                Cmd::Shuffle(on) => Some(s.TryChangeShuffleActiveAsync(*on)),
                Cmd::Repeat(mode) => Some(s.TryChangeAutoRepeatModeAsync(windows::Media::MediaPlaybackAutoRepeatMode(i32::from(*mode)))),
                Cmd::Rate(rate) => Some(s.TryChangePlaybackRateAsync(*rate)),
                Cmd::Pick(_) => None,
            };
            if let Some(sent) = sent {
                let _ = sent.and_then(|a| a.join());
                // the player takes a moment to report the new song or state
                std::thread::sleep(Duration::from_millis(300));
            }
        }
        let read: Vec<(Now, Properties)> = sessions.iter().filter_map(|s| read(s).ok()).filter(|(n, _)| !n.title.is_empty()).collect();
        // the picture is read only when the shown song changes
        let key = read.first().map(|(n, _)| song_key(n));
        if lock(&shared.cover).as_ref().map(|(k, _)| k) != key.as_ref() {
            let new = read.first().and_then(|(_, p)| cover(p)).zip(key.clone()).map(|(img, k)| (k, img));
            // a new song's picture may come a moment after its title: without one, ask again next round
            if new.is_some() || key.is_none() {
                *lock(&shared.cover) = new;
                ctx.request_repaint();
            }
        }
        let all: Vec<Now> = read.into_iter().map(|(n, _)| n).collect();
        let mut held = lock(&shared.all);
        if *held != all {
            *held = all;
            *lock(&shared.read_at) = Instant::now();
            ctx.request_repaint();
        }
    }
}

fn song_key(n: &Now) -> String {
    format!("{}\n{}\n{}", n.app, n.title, n.artist)
}

fn shared(ctx: &egui::Context) -> &'static Shared {
    let shared = SHARED.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        let ctx = ctx.clone();
        std::thread::spawn(move || run(ctx, rx));
        Shared { all: Mutex::new(vec![]), read_at: Mutex::new(Instant::now()), cover: Mutex::new(None), seen: AtomicU64::new(0), start: Instant::now(), tx: Mutex::new(tx) }
    });
    shared.seen.store(shared.start.elapsed().as_millis() as u64, Ordering::Relaxed);
    shared
}

/// What plays now, if anything. Call each frame a song may show: it keeps the asking alive.
pub fn now(ctx: &egui::Context) -> Option<Now> {
    lock(&shared(ctx).all).first().cloned()
}

pub fn send(cmd: Cmd) {
    if let Some(shared) = SHARED.get() {
        let _ = lock(&shared.tx).send(cmd);
    }
}

/// A player's name for people: "Spotify" from "SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify", "Chrome" from
/// "chrome.exe".
fn app_name(app: &str) -> String {
    // a Store app is "<publisher>.<package>_<hash>!<entry>": the entry names it, unless it is just "App"
    let name = match app.split_once('!') {
        Some((package, "App")) => package.split('_').next().unwrap_or(package).rsplit('.').next().unwrap_or(package),
        Some((_, entry)) => entry,
        None => app,
    };
    let name = name.strip_suffix(".exe").or_else(|| name.strip_suffix(".EXE")).unwrap_or(name);
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Player".to_owned(),
    }
}

/// The speeds the Music page offers.
const SPEEDS: [f64; 6] = [1.0, 1.05, 1.10, 1.15, 1.20, 1.25];

/// Repeat goes off, the list, this song, off.
fn next_repeat(mode: u8) -> u8 {
    match mode {
        0 => 2,
        2 => 1,
        _ => 0,
    }
}

/// 103.4 -> "1:43".
fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The Music page: every player with a song, the shown one's picture, progress and buttons.
pub fn page(ui: &mut egui::Ui) {
    use crate::theme::{self as th, ico, rt, sb, text2};
    use egui::{pos2, vec2, Align, Layout, Rect, Sense};
    let ctx = ui.ctx().clone();
    let shared = shared(&ctx);
    let all = lock(&shared.all).clone();
    let Some(now) = all.first() else {
        th::card(ui, (18, 18, 18, 18), |ui| ui.label(rt("Nothing is playing. Start a song in any player and it shows here.", 14.0).color(text2())));
        crate::soundcloud::section(ui);
        return;
    };
    // more than one player: pick which one shows
    if all.len() > 1 {
        ui.horizontal(|ui| {
            for (i, n) in all.iter().enumerate() {
                if ui.selectable_label(i == 0, rt(app_name(&n.app), 13.0)).on_hover_text(&n.title).clicked() && i != 0 {
                    send(Cmd::Pick(n.app.clone()));
                }
            }
        });
        ui.add_space(10.0);
    }
    // the picture as a texture, loaded anew when the song changes
    let tex_id = egui::Id::new("music_cover");
    let mut tex: Option<(String, egui::TextureHandle)> = ctx.data(|d| d.get_temp(tex_id));
    {
        let cover = lock(&shared.cover);
        if cover.as_ref().map(|(k, _)| k) != tex.as_ref().map(|(k, _)| k) {
            tex = cover.as_ref().map(|(k, img)| (k.clone(), ctx.load_texture("music_cover", img.clone(), th::SMOOTH)));
            let keep = tex.clone();
            drop(cover);
            ctx.data_mut(|d| match keep {
                Some(t) => {
                    d.insert_temp(tex_id, t);
                }
                None => d.remove::<(String, egui::TextureHandle)>(tex_id),
            });
        }
    }
    th::card(ui, (24, 24, 24, 24), |ui| {
        let side = 240.0f32.min(ui.available_width() * 0.4);
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), side), Sense::hover());
        // right: the song's picture, whole, in a rounded square
        let square = Rect::from_min_size(pos2(rect.right() - side, rect.top()), vec2(side, side));
        ui.painter().rect_filled(square, 16.0, egui::Color32::from_white_alpha(0x10));
        match &tex {
            Some((_, t)) => {
                let fit = t.size_vec2() * (side / t.size_vec2().max_elem());
                egui::Image::from_texture((t.id(), fit)).corner_radius(16.0).paint_at(ui, Rect::from_center_size(square.center(), fit));
            }
            None => {
                ui.painter().text(square.center(), egui::Align2::CENTER_CENTER, "\u{E8D6}", egui::FontId::new(side * 0.3, th::icons()), text2());
            }
        }
        // top right corner: the small song window that stays on top of other windows
        let corner = Rect::from_min_size(pos2(square.right() - 42.0, square.top() + 10.0), vec2(32.0, 32.0));
        let pop = egui::Button::new(ico("\u{E8A7}", 15.0).color(th::TEXT)).fill(egui::Color32::from_black_alpha(0x8C)).corner_radius(16.0);
        if ui.put(corner, pop).on_hover_text("Small window, always on top").clicked() {
            let id = crate::chatpage::song_window_id();
            let open = ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
            ctx.data_mut(|d| d.insert_temp(id, !open));
        }
        // left: title, artist, progress, buttons
        let left = Rect::from_min_max(rect.min, pos2(square.left() - 28.0, rect.bottom()));
        ui.scope_builder(egui::UiBuilder::new().max_rect(left).layout(Layout::top_down(Align::Min)), |ui| {
            // semibold, not the display face: only it has the symbol and emoji fallbacks, and a song title
            // with a glyph the font lacks ("★") panics egui
            ui.add(egui::Label::new(sb(&now.title, 26.0).color(th::TEXT)).wrap());
            if !now.artist.is_empty() {
                ui.add_space(6.0);
                ui.add(egui::Label::new(rt(&now.artist, 16.0).color(text2())).truncate());
            }
            ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                // bottom up: the buttons, then the progress above them
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 20.0;
                    // a round button: a soft circle shows under the pointer, a greyed one the player does not offer,
                    // one that is on takes the accent and a dot under it
                    let round = |ui: &mut egui::Ui, text: &str, font: egui::FontId, idle: egui::Color32, base: f32, offered: bool, on: bool| {
                        let (rect, resp) = ui.allocate_exact_size(vec2(40.0, 40.0), if offered { Sense::click() } else { Sense::hover() });
                        let p = ui.painter();
                        let shade = if !offered { 0 } else if resp.is_pointer_button_down_on() { 0x2E } else if resp.hovered() { 0x1C } else { 0 };
                        p.circle_filled(rect.center(), 20.0, egui::Color32::from_white_alpha(base as u8 + shade));
                        let color = if !offered { text2().gamma_multiply(0.5) } else if on { th::p().accent } else { idle };
                        p.text(rect.center(), egui::Align2::CENTER_CENTER, text, font, color);
                        if on && offered {
                            th::dot(p, pos2(rect.center().x, rect.bottom() - 3.0), 2.0, th::p().accent);
                        }
                        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
                    };
                    let icon = |size: f32| egui::FontId::new(size, th::icons());
                    // shuffle, repeat and speed show only when the player offers them (SoundCloud offers none)
                    if let Some(shuffle) = now.shuffle {
                        if round(ui, "\u{E8B1}", icon(18.0), text2(), 0.0, true, shuffle).on_hover_text(if shuffle { "Shuffle is on" } else { "Shuffle" }).clicked() {
                            send(Cmd::Shuffle(!shuffle));
                        }
                    }
                    let play = if now.playing { ("\u{E769}", "Pause") } else { ("\u{E768}", "Play") };
                    for ((glyph, tip), cmd) in [(("\u{E892}", "Previous"), Cmd::Previous), (play, Cmd::Toggle), (("\u{E893}", "Next"), Cmd::Next)] {
                        if round(ui, glyph, icon(24.0), th::TEXT, 0.0, true, false).on_hover_text(tip).clicked() {
                            send(cmd);
                        }
                    }
                    // repeat: off, the list (accent), this song (accent and a small 1)
                    if let Some(repeat) = now.repeat {
                        let tip = match repeat {
                            1 => "Repeating this song",
                            2 => "Repeating the list",
                            _ => "Repeat",
                        };
                        let r = round(ui, "\u{E8EE}", icon(18.0), text2(), 0.0, true, repeat != 0);
                        if repeat == 1 {
                            let at = r.rect.center() + vec2(9.0, -9.0);
                            ui.painter().circle_filled(at, 6.0, th::p().accent);
                            ui.painter().text(at, egui::Align2::CENTER_CENTER, "1", egui::FontId::proportional(9.0), egui::Color32::BLACK);
                        }
                        if r.on_hover_text(tip).clicked() {
                            send(Cmd::Repeat(next_repeat(repeat)));
                        }
                    }
                    // speed: one round button with the current speed; its list opens beside it, downwards
                    if let Some(rate) = now.rate {
                        let speed_label = |s: f64| if (s - 1.0).abs() < 0.01 { "1x".to_owned() } else { format!("{s:.2}x") };
                        let chip = round(ui, &speed_label(rate), egui::FontId::proportional(12.0), th::TEXT, 26.0, true, false).on_hover_text("Speed");
                        egui::Popup::menu(&chip).align(egui::RectAlign::RIGHT_START).show(|ui| {
                            for speed in SPEEDS {
                                if ui.selectable_label((rate - speed).abs() < 0.01, rt(speed_label(speed), 13.0)).clicked() {
                                    send(Cmd::Rate(speed));
                                }
                            }
                        });
                    }
                });
                if now.length > 0.0 {
                    ui.add_space(18.0);
                    let played = if now.playing { now.position + lock(&shared.read_at).elapsed().as_secs_f64() } else { now.position }.min(now.length);
                    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::hover());
                    let font = egui::FontId::proportional(12.0);
                    ui.painter().text(row.left_center(), egui::Align2::LEFT_CENTER, clock(played), font.clone(), text2());
                    ui.painter().text(row.right_center(), egui::Align2::RIGHT_CENTER, clock(now.length), font, text2());
                    let bar = Rect::from_min_max(pos2(row.left() + 44.0, row.center().y - 3.0), pos2(row.right() - 44.0, row.center().y + 3.0));
                    ui.painter().rect_filled(bar, 3.0, egui::Color32::from_white_alpha(0x29));
                    ui.painter().rect_filled(bar.with_max_x(bar.left() + bar.width() * (played / now.length) as f32), 3.0, th::p().accent);
                    if now.playing {
                        ctx.request_repaint_after(Duration::from_millis(500));
                    }
                }
            });
        });
    });
    crate::soundcloud::section(ui);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_clock() {
        assert_eq!(app_name("SpotifyAB.SpotifyMusic_zpdnekdrzrea0!Spotify"), "Spotify");
        assert_eq!(app_name("chrome.exe"), "Chrome");
        assert_eq!(app_name("12345Pub.SoundCloud_abc123!App"), "SoundCloud");
        assert_eq!(app_name(""), "Player");
        assert_eq!(clock(103.4), "1:43");
        assert_eq!(clock(-1.0), "0:00");
        assert_eq!([0, 2, 1].map(next_repeat), [2, 1, 0]);
    }

    /// Asks Windows for real: prints every player's song (nothing playing is fine, a failing call is not).
    #[test]
    #[ignore]
    fn real_session() {
        let _ = unsafe { windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED) };
        let manager = Manager::RequestAsync().and_then(|m| m.join()).expect("media session manager");
        let sessions: Vec<Session> = manager.GetSessions().expect("sessions").into_iter().collect();
        println!("NOW players={}", sessions.len());
        for s in &sessions {
            let (n, p) = read(s).expect("media properties");
            let pic = cover(&p).map(|c| c.size);
            println!("NOW app={:?} title={:?} artist={:?} playing={} at={} of={} cover={:?} shuffle={:?} repeat={:?} rate={:?}", app_name(&n.app), n.title, n.artist, n.playing, clock(n.position), clock(n.length), pic, n.shuffle, n.repeat, n.rate);
        }
    }
}
