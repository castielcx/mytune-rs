//! SoundCloud on the Music page: search, the account's likes and playlists, played by Windows' own
//! `MediaPlayer`. The player shows in Windows' media controls like any other, so the now-playing card above
//! and the chat's media strip pick it up. A track's stream address is asked for only when it is about to
//! play (`MediaBinder`): they are signed and expire.

use eframe::egui;
use mytune_core::soundcloud::{self as sc, Playlist, Track};
use std::sync::{mpsc, Mutex, OnceLock};
use windows::core::HSTRING;
use windows::Foundation::{TypedEventHandler, Uri};
use windows::Media::Core::{MediaBinder, MediaBindingEventArgs, MediaSource};
use windows::Media::MediaPlaybackType;
use windows::Media::Playback::{MediaPlaybackItem, MediaPlaybackList, MediaPlaybackState, MediaPlayer, MediaPlayerFailedEventArgs};
use windows::Media::Streaming::Adaptive::{AdaptiveMediaSource, AdaptiveMediaSourceCreationStatus};
use windows::Storage::Streams::RandomAccessStreamReference;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

// ---------------------------------------------------------------- artwork thumbnails

enum Art {
    Loading,
    Ready(egui::ColorImage),
    Failed,
}

fn art_cache() -> &'static Mutex<std::collections::HashMap<String, Art>> {
    static C: OnceLock<Mutex<std::collections::HashMap<String, Art>>> = OnceLock::new();
    C.get_or_init(Default::default)
}

/// Fetch a small artwork and decode it; SoundCloud serves a 120 px variant.
fn fetch_art(url: &str) -> Option<egui::ColorImage> {
    use std::io::Read;
    let small = url.replace("t500x500", "t120x120").replace("-large.", "-t120x120.");
    let resp = ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(10)).build().get(&small).call().ok()?;
    let mut bytes = Vec::new();
    resp.into_reader().take(8 * 1024 * 1024).read_to_end(&mut bytes).ok()?;
    let pic = mytune_chat::yapfast_core::chat::decode_picture(&bytes, 120, 120)?;
    Some(egui::ColorImage::from_rgba_unmultiplied([pic.width as usize, pic.height as usize], &pic.rgba))
}

/// The artwork texture for a URL, loaded once in the background and cached; `None` until it is ready.
fn art_texture(ctx: &egui::Context, url: &str) -> Option<egui::TextureHandle> {
    let id = egui::Id::new(("scart", url));
    if let Some(t) = ctx.data(|d| d.get_temp::<egui::TextureHandle>(id)) {
        return Some(t);
    }
    let mut cache = lock(art_cache());
    match cache.get(url) {
        Some(Art::Ready(img)) => {
            let tex = ctx.load_texture(format!("scart:{url}"), img.clone(), crate::theme::SMOOTH);
            ctx.data_mut(|d| d.insert_temp(id, tex.clone()));
            Some(tex)
        }
        Some(_) => None, // loading or failed
        None => {
            cache.insert(url.to_string(), Art::Loading);
            let (url, ctx) = (url.to_string(), ctx.clone());
            std::thread::spawn(move || {
                let art = fetch_art(&url).map_or(Art::Failed, Art::Ready);
                lock(art_cache()).insert(url, art);
                ctx.request_repaint();
            });
            None
        }
    }
}

// ---------------------------------------------------------------- player

// Toggle/Next/Previous/Shuffle/Repeat are driven from the top now-playing card now, not this card, but the
// player thread still answers them.
#[allow(dead_code)]
enum Cmd {
    Play(Vec<Track>, usize, Option<String>),
    Toggle,
    Next,
    Previous,
    Shuffle(bool),
    /// 0 off, 1 this track, 2 the list.
    Repeat(u8),
}

/// The track playing (its id) and the last thing that went wrong while playing.
static PLAYING: Mutex<Option<u64>> = Mutex::new(None);
/// Shuffle and repeat (0 off, 1 this track, 2 the list), kept so a new list starts the same way.
static SHUFFLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static REPEAT: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
static PLAY_ERROR: Mutex<Option<String>> = Mutex::new(None);
static PLAYER: OnceLock<(Mutex<mpsc::Sender<Cmd>>, egui::Context)> = OnceLock::new();

fn send(ctx: &egui::Context, cmd: Cmd) {
    let (tx, _) = PLAYER.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        std::thread::Builder::new().name("soundcloud-player".into()).spawn(move || run(rx)).ok();
        (Mutex::new(tx), ctx.clone())
    });
    let _ = lock(tx).send(cmd);
}

fn repaint() {
    if let Some((_, ctx)) = PLAYER.get() {
        ctx.request_repaint();
    }
}

fn failed(msg: String) {
    mytune_core::log::info(&format!("SoundCloud: {msg}"));
    *lock(&PLAY_ERROR) = Some(msg);
    repaint();
}

fn run(rx: mpsc::Receiver<Cmd>) {
    let _ = unsafe { windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED) };
    let player = match MediaPlayer::new() {
        Ok(p) => p,
        Err(e) => return failed(format!("Windows' media player did not start: {e}")),
    };
    let _ = player.MediaFailed(&TypedEventHandler::<MediaPlayer, MediaPlayerFailedEventArgs>::new(|_, args| {
        failed(format!("Could not play: {}", args.ok()?.ErrorMessage()?));
        Ok(())
    }));
    let mut list: Option<MediaPlaybackList> = None;
    while let Ok(cmd) = rx.recv() {
        let done = match cmd {
            Cmd::Play(tracks, start, token) => play(&player, tracks, start, token).map(|l| list = Some(l)),
            Cmd::Toggle => player.PlaybackSession().and_then(|s| s.PlaybackState()).and_then(|s| if s == MediaPlaybackState::Playing { player.Pause() } else { player.Play() }),
            Cmd::Next => list.as_ref().map_or(Ok(()), |l| l.MoveNext().map(|_| ())),
            Cmd::Previous => list.as_ref().map_or(Ok(()), |l| l.MovePrevious().map(|_| ())),
            Cmd::Shuffle(on) => {
                SHUFFLE.store(on, std::sync::atomic::Ordering::Relaxed);
                list.as_ref().map_or(Ok(()), |l| l.SetShuffleEnabled(on))
            }
            Cmd::Repeat(mode) => {
                REPEAT.store(mode, std::sync::atomic::Ordering::Relaxed);
                apply_repeat(&player, list.as_ref(), mode)
            }
        };
        if let Err(e) = done {
            failed(e.to_string());
        }
    }
}

/// Repeat the list (`AutoRepeatEnabled`) or just this track (the player loops the current item).
fn apply_repeat(player: &MediaPlayer, list: Option<&MediaPlaybackList>, mode: u8) -> windows::core::Result<()> {
    player.SetIsLoopingEnabled(mode == 1)?;
    if let Some(l) = list {
        l.SetAutoRepeatEnabled(mode == 2)?;
    }
    Ok(())
}

fn play(player: &MediaPlayer, tracks: Vec<Track>, start: usize, token: Option<String>) -> windows::core::Result<MediaPlaybackList> {
    *lock(&PLAY_ERROR) = None;
    let list = MediaPlaybackList::new()?;
    let items = list.Items()?;
    let mut first = None;
    for (i, t) in tracks.iter().enumerate() {
        let binder = MediaBinder::new()?;
        let (track, token) = (t.clone(), token.clone());
        binder.Binding(&TypedEventHandler::<MediaBinder, MediaBindingEventArgs>::new(move |_, args| {
            let args = args.ok()?;
            let deferral = args.GetDeferral()?;
            match sc::stream_url(&track, token.as_deref()) {
                Ok(url) => {
                    let uri = Uri::CreateUri(&HSTRING::from(url.as_str()))?;
                    // HLS needs the adaptive source; a plain MP3 address plays as it is
                    let hls = url.contains(".m3u8").then(|| AdaptiveMediaSource::CreateFromUriAsync(&uri).and_then(|a| a.join())).and_then(Result::ok);
                    match hls {
                        Some(r) if r.Status()? == AdaptiveMediaSourceCreationStatus::Success => args.SetAdaptiveMediaSource(&r.MediaSource()?)?,
                        _ => args.SetUri(&uri)?,
                    }
                }
                Err(e) => failed(format!("{}: {e}", track.title)),
            }
            deferral.Complete()
        }))?;
        let item = MediaPlaybackItem::Create(&MediaSource::CreateFromMediaBinder(&binder)?)?;
        // title, artist and picture for Windows' media controls (and so for the card above)
        let props = item.GetDisplayProperties()?;
        props.SetType(MediaPlaybackType::Music)?;
        let music = props.MusicProperties()?;
        music.SetTitle(&HSTRING::from(t.title.as_str()))?;
        music.SetArtist(&HSTRING::from(t.artist.as_str()))?;
        if let Some(art) = &t.artwork {
            props.SetThumbnail(&RandomAccessStreamReference::CreateFromUri(&Uri::CreateUri(&HSTRING::from(art.as_str()))?)?)?;
        }
        item.ApplyDisplayProperties(&props)?;
        items.Append(&item)?;
        if i == start {
            first = Some(item);
        }
    }
    let ids: Vec<u64> = tracks.iter().map(|t| t.id).collect();
    list.CurrentItemChanged(&TypedEventHandler::new(move |list: windows::core::Ref<MediaPlaybackList>, _| {
        let i = list.ok()?.CurrentItemIndex()? as usize;
        *lock(&PLAYING) = ids.get(i).copied();
        repaint();
        Ok(())
    }))?;
    if let Some(item) = &first {
        list.SetStartingItem(item)?;
    }
    *lock(&PLAYING) = tracks.get(start).map(|t| t.id);
    // keep the shuffle and repeat the user already chose
    list.SetShuffleEnabled(SHUFFLE.load(std::sync::atomic::Ordering::Relaxed))?;
    apply_repeat(player, Some(&list), REPEAT.load(std::sync::atomic::Ordering::Relaxed))?;
    player.SetSource(&list)?;
    player.Play()?;
    Ok(list)
}

// ---------------------------------------------------------------- page section

#[derive(Default, Clone, Copy, PartialEq)]
enum Tab {
    #[default]
    Search,
    Likes,
    Playlists,
}

#[derive(Default)]
struct State {
    tab: Tab,
    query: String,
    /// The saved login, read once (`None` until then).
    token: Option<Option<String>>,
    field: String,
    found: Vec<Track>,
    likes: Option<Vec<Track>>,
    lists: Option<Vec<Playlist>>,
    open: Option<(String, Vec<Track>)>,
    busy: bool,
    error: Option<String>,
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();

fn state() -> std::sync::MutexGuard<'static, State> {
    lock(STATE.get_or_init(Default::default))
}

/// Runs `work` on its own thread, then hands its result to `done` (or shows its error).
fn background<T: Send + 'static>(ctx: &egui::Context, work: impl FnOnce() -> Result<T, String> + Send + 'static, done: impl FnOnce(&mut State, T) + Send + 'static) {
    {
        let mut s = state();
        s.busy = true;
        s.error = None;
    }
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let r = work();
        let mut s = state();
        s.busy = false;
        match r {
            Ok(v) => done(&mut s, v),
            Err(e) => s.error = Some(e),
        }
        ctx.request_repaint();
    });
}

#[allow(dead_code)] // Player: kept for the thread command path, no longer raised from this card
enum Act {
    Search(String),
    Likes,
    Lists,
    Open(u64, String),
    Play(Vec<Track>, usize),
    Save(String),
    Login,
    SignOut,
    Player(Cmd),
}

/// The track rows; returns the clicked one.
fn rows(ui: &mut egui::Ui, tracks: &[Track], playing: Option<u64>) -> Option<usize> {
    use crate::theme::{self as th, rt, sb, text2, text3};
    let ctx = ui.ctx().clone();
    let mut clicked = None;
    for (i, t) in tracks.iter().enumerate() {
        let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 56.0), egui::Sense::click());
        if !ui.is_rect_visible(rect) {
            continue;
        }
        let ok = t.transcoding.is_some();
        let now = playing == Some(t.id);
        if now {
            ui.painter().rect_filled(rect, 12.0, th::p().accent.gamma_multiply(0.22));
        } else if resp.hovered() && ok {
            ui.painter().rect_filled(rect, 12.0, egui::Color32::from_white_alpha(0x12));
        }
        // artwork thumbnail, or a note glyph while it loads or has none
        let thumb = egui::Rect::from_min_size(rect.left_top() + egui::vec2(8.0, 8.0), egui::vec2(40.0, 40.0));
        match t.artwork.as_deref().and_then(|u| art_texture(&ctx, u)) {
            Some(tex) => egui::Image::from_texture((tex.id(), thumb.size())).corner_radius(8.0).paint_at(ui, thumb),
            None => {
                ui.painter().rect_filled(thumb, 8.0, egui::Color32::from_white_alpha(0x10));
                ui.painter().text(thumb.center(), egui::Align2::CENTER_CENTER, "\u{E8D6}", egui::FontId::new(18.0, th::icons()), text3());
            }
        }
        let tx = thumb.right() + 12.0;
        let right = if t.preview { "Preview  " } else { "" }.to_owned() + &clock(t.duration);
        let right = if ok { right } else { "Not playable".into() };
        let font = egui::FontId::proportional(12.0);
        let r = ui.painter().text(rect.right_center() - egui::vec2(14.0, 0.0), egui::Align2::RIGHT_CENTER, right, font, text3());
        let width = (r.left() - tx - 12.0).max(40.0);
        let line = |text: egui::RichText| egui::WidgetText::from(text).into_galley(ui, Some(egui::TextWrapMode::Truncate), width, egui::FontSelection::Default);
        let title = line(sb(&t.title, 14.0).color(if ok { th::TEXT } else { text3() }));
        let artist = line(rt(&t.artist, 12.0).color(text2()));
        ui.painter().galley(egui::pos2(tx, rect.top() + 10.0), title, th::TEXT);
        ui.painter().galley(egui::pos2(tx, rect.top() + 31.0), artist, text2());
        if resp.on_hover_cursor(if ok { egui::CursorIcon::PointingHand } else { egui::CursorIcon::NotAllowed }).clicked() && ok {
            clicked = Some(i);
        }
    }
    clicked
}

fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// The SoundCloud card under the now-playing card.
pub fn section(ui: &mut egui::Ui) {
    use crate::theme::{self as th, rt, text2};
    let ctx = ui.ctx().clone();
    let mut act = None;
    let playing = *lock(&PLAYING);
    // a finished sign-in: reload the token or show why it failed
    if let Some(r) = crate::soundcloud_login::take_result() {
        let mut s = state();
        match r {
            Ok(()) => {
                s.token = Some(sc::load_token());
                (s.likes, s.lists, s.error) = (None, None, None);
            }
            Err(e) => s.error = Some(e),
        }
    }
    // heading: a cloud mark in SoundCloud orange, then the name
    ui.add_space(24.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        ui.label(th::ico("\u{E753}", 18.0).color(egui::Color32::from_rgb(0xFF, 0x55, 0x00)));
        ui.label(th::dsb("SoundCloud", 13.0).color(th::TEXT));
    });
    ui.add_space(8.0);
    th::card(ui, (18, 18, 18, 18), |ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        let mut s = state();
        if s.token.is_none() {
            s.token = Some(sc::load_token());
            // test switch: start with this search (tests/shot_music.rs)
            if let Ok(q) = std::env::var("MYTUNE_SC_SEARCH") {
                s.query = q.clone();
                act = Some(Act::Search(q));
            }
        }
        let token = s.token.clone().flatten();
        ui.horizontal(|ui| {
            for (tab, name) in [(Tab::Search, "Search"), (Tab::Likes, "Likes"), (Tab::Playlists, "Playlists")] {
                if th::tab(ui, name, s.tab == tab).clicked() {
                    s.tab = tab;
                }
            }
            // playback (play, skip, shuffle, repeat) is the top "what's playing" card's job once a track plays
            if s.busy {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spinner();
                });
            }
        });
        ui.add_space(6.0);
        if let Some(e) = s.error.clone().or_else(|| lock(&PLAY_ERROR).clone()) {
            ui.label(rt(e, 13.0).color(th::DANGER));
        }
        let needs_login = s.tab != Tab::Search && token.is_none();
        if needs_login {
            ui.label(rt("Likes and playlists need your SoundCloud login. Sign in once in the window that opens; MyTune keeps the login in the Windows credential store.", 13.0).color(text2()));
            ui.add_space(4.0);
            if crate::soundcloud_login::is_open() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(rt("Finish signing in in the window that opened.", 13.0).color(text2()));
                });
            } else if th::button(ui, "Sign in to SoundCloud", true, (18.0, 7.0, 18.0, 8.0)).clicked() {
                act = Some(Act::Login);
            }
            ui.add_space(10.0);
            ui.collapsing(rt("Or paste a token", 13.0).color(text2()), |ui| {
                ui.label(rt("soundcloud.com in a browser > F12 > Application (or Storage) > Cookies > https://soundcloud.com > copy oauth_token.", 12.0).color(text2()));
                ui.horizontal(|ui| {
                    let field = ui.add(egui::TextEdit::singleline(&mut s.field).password(true).hint_text("oauth_token").desired_width(320.0));
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (th::button(ui, "Save", true, (16.0, 6.0, 16.0, 7.0)).clicked() || enter) && !s.field.trim().is_empty() {
                        act = Some(Act::Save(sc::clean_token(&s.field)));
                    }
                });
            });
            return;
        }
        match s.tab {
            Tab::Search => {
                ui.horizontal(|ui| {
                    let field = ui.add(egui::TextEdit::singleline(&mut s.query).hint_text("Search SoundCloud").background_color(th::control_bg()).margin(egui::Margin::symmetric(10, 8)).desired_width(ui.available_width() - 110.0));
                    let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    if (th::button(ui, "Search", true, (16.0, 6.0, 16.0, 7.0)).clicked() || enter) && !s.query.trim().is_empty() && !s.busy {
                        act = Some(Act::Search(s.query.trim().to_owned()));
                    }
                });
                if let Some(i) = rows(ui, &s.found, playing) {
                    act = Some(Act::Play(s.found.clone(), i));
                }
            }
            Tab::Likes => {
                if s.likes.is_none() && !s.busy && s.error.is_none() {
                    act = Some(Act::Likes);
                }
                ui.horizontal(|ui| {
                    ui.label(rt(format!("{} liked tracks", s.likes.as_ref().map_or(0, Vec::len)), 13.0).color(text2()));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if th::subtle_button(ui, "Sign out", (10.0, 4.0, 10.0, 4.0)).clicked() {
                            act = Some(Act::SignOut);
                        }
                        if th::subtle_button(ui, "Refresh", (10.0, 4.0, 10.0, 4.0)).clicked() && !s.busy {
                            act = Some(Act::Likes);
                        }
                    });
                });
                if let Some(likes) = &s.likes {
                    if let Some(i) = rows(ui, likes, playing) {
                        act = Some(Act::Play(likes.clone(), i));
                    }
                }
            }
            Tab::Playlists => {
                if let Some((title, tracks)) = s.open.clone() {
                    ui.horizontal(|ui| {
                        if th::subtle_button(ui, "\u{2039}  Back", (10.0, 4.0, 10.0, 4.0)).clicked() {
                            s.open = None;
                        }
                        ui.label(th::sb(&title, 15.0));
                    });
                    if let Some(i) = rows(ui, &tracks, playing) {
                        act = Some(Act::Play(tracks, i));
                    }
                    return;
                }
                if s.lists.is_none() && !s.busy && s.error.is_none() {
                    act = Some(Act::Lists);
                }
                for p in s.lists.clone().unwrap_or_default() {
                    let label = format!("{}    {} tracks", p.title, p.count);
                    if th::subtle_button(ui, &label, (14.0, 8.0, 14.0, 8.0)).clicked() && !s.busy {
                        act = Some(Act::Open(p.id, p.title.clone()));
                    }
                }
            }
        }
    });
    let token = state().token.clone().flatten();
    match act {
        Some(Act::Search(q)) => {
            let t = token.clone();
            background(&ctx, move || sc::search(&q, t.as_deref()), |s, found| s.found = found);
        }
        Some(Act::Likes) => {
            let t = token.unwrap_or_default();
            background(&ctx, move || sc::likes(&t), |s, likes| s.likes = Some(likes));
        }
        Some(Act::Lists) => {
            let t = token.unwrap_or_default();
            background(&ctx, move || sc::playlists(&t), |s, lists| s.lists = Some(lists));
        }
        Some(Act::Open(id, title)) => {
            background(&ctx, move || sc::playlist_tracks(id, token.as_deref()), move |s, tracks| s.open = Some((title, tracks)));
        }
        Some(Act::Play(tracks, i)) => {
            // keep only playable tracks in the queue, so the list never stops on a DRM/no-stream one
            let id = tracks[i].id;
            let playable: Vec<Track> = tracks.into_iter().filter(|t| t.transcoding.is_some()).collect();
            let start = playable.iter().position(|t| t.id == id).unwrap_or(0);
            if !playable.is_empty() {
                send(&ctx, Cmd::Play(playable, start, token));
            }
        }
        Some(Act::Save(t)) => {
            let mut s = state();
            match sc::save_token(&t) {
                Ok(()) => {
                    s.token = Some(Some(t));
                    s.field.clear();
                    (s.likes, s.lists, s.error) = (None, None, None);
                }
                Err(e) => s.error = Some(e),
            }
        }
        Some(Act::SignOut) => {
            let _ = sc::save_token("");
            let mut s = state();
            s.token = Some(None);
            (s.likes, s.lists, s.open) = (None, None, None);
        }
        Some(Act::Login) => crate::soundcloud_login::open(&ctx),
        Some(Act::Player(cmd)) => send(&ctx, cmd),
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Plays the first playable search result, muted, through the real player: it must reach Playing
    /// without an error, and Windows' media sessions should list it (printed, not asserted).
    #[test]
    #[ignore]
    fn real_play() {
        let _ = unsafe { windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED) };
        let found = sc::search("lofi hip hop", None).expect("search");
        let start = found.iter().position(|t| t.transcoding.is_some()).expect("a playable track");
        let player = MediaPlayer::new().unwrap();
        player.SetIsMuted(true).unwrap();
        play(&player, found.clone(), start, None).expect("play");
        let mut state = MediaPlaybackState::None;
        for _ in 0..30 {
            std::thread::sleep(std::time::Duration::from_millis(500));
            state = player.PlaybackSession().unwrap().PlaybackState().unwrap();
            if state == MediaPlaybackState::Playing || lock(&PLAY_ERROR).is_some() {
                break;
            }
        }
        let pos = player.PlaybackSession().unwrap().Position().unwrap().Duration as f64 / 1e7;
        println!("SC state={state:?} pos={pos:.1} error={:?} title={:?}", lock(&PLAY_ERROR), found[start].title);
        use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager as M;
        if let Ok(m) = M::RequestAsync().and_then(|m| m.join()) {
            for s in m.GetSessions().unwrap() {
                let title = s.TryGetMediaPropertiesAsync().and_then(|p| p.join()).and_then(|p| p.Title()).unwrap_or_default();
                println!("SC session app={} title={title}", s.SourceAppUserModelId().unwrap_or_default());
            }
        }
        assert_eq!(state, MediaPlaybackState::Playing);
    }

    /// Shuffle and repeat take on a live list: the properties read back what was set.
    #[test]
    #[ignore]
    fn real_shuffle_repeat() {
        let _ = unsafe { windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED) };
        let found = sc::search("lofi", None).expect("search");
        let start = found.iter().position(|t| t.transcoding.is_some()).expect("a playable track");
        let player = MediaPlayer::new().unwrap();
        player.SetIsMuted(true).unwrap();
        SHUFFLE.store(true, std::sync::atomic::Ordering::Relaxed);
        REPEAT.store(2, std::sync::atomic::Ordering::Relaxed);
        let list = play(&player, found, start, None).expect("play");
        println!("SC applied shuffle={} auto_repeat={} loop={}", list.ShuffleEnabled().unwrap(), list.AutoRepeatEnabled().unwrap(), player.IsLoopingEnabled().unwrap());
        assert!(list.ShuffleEnabled().unwrap() && list.AutoRepeatEnabled().unwrap());
        apply_repeat(&player, Some(&list), 1).unwrap();
        println!("SC repeat-one loop={} auto_repeat={}", player.IsLoopingEnabled().unwrap(), list.AutoRepeatEnabled().unwrap());
        assert!(player.IsLoopingEnabled().unwrap() && !list.AutoRepeatEnabled().unwrap());
    }

    /// Does MyTune's own SMTC session expose shuffle/repeat (so the top now-playing card can drive it)?
    #[test]
    #[ignore]
    fn real_smtc() {
        use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager as M;
        let _ = unsafe { windows::Win32::System::WinRT::RoInitialize(windows::Win32::System::WinRT::RO_INIT_MULTITHREADED) };
        let found = sc::search("lofi", None).expect("search");
        let start = found.iter().position(|t| t.transcoding.is_some()).unwrap();
        let player = MediaPlayer::new().unwrap();
        player.SetIsMuted(true).unwrap();
        let _list = play(&player, found, start, None).expect("play");
        std::thread::sleep(std::time::Duration::from_millis(2500));
        let m = M::RequestAsync().and_then(|m| m.join()).unwrap();
        let me = format!("{}", std::process::id());
        for s in m.GetSessions().unwrap() {
            let app = s.SourceAppUserModelId().unwrap_or_default().to_string();
            if !app.contains("mytune") && !app.contains(&me) {
                continue;
            }
            let info = s.GetPlaybackInfo().unwrap();
            let c = info.Controls().unwrap();
            println!("SMTC app={app} shuffle_btn={} repeat_btn={} next={} prev={}", c.IsShuffleEnabled().unwrap(), c.IsRepeatEnabled().unwrap(), c.IsNextEnabled().unwrap(), c.IsPreviousEnabled().unwrap());
            // toggle from the SMTC (what the top card's button does) and see it reach the queue
            s.TryChangeShuffleActiveAsync(true).unwrap().join().unwrap();
            s.TryChangeAutoRepeatModeAsync(windows::Media::MediaPlaybackAutoRepeatMode::List).unwrap().join().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(600));
            println!("SMTC after toggle: list shuffle={} auto_repeat={}", _list.ShuffleEnabled().unwrap(), _list.AutoRepeatEnabled().unwrap());
            assert!(_list.ShuffleEnabled().unwrap() && _list.AutoRepeatEnabled().unwrap());
        }
    }
}
