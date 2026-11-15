//! Runs the Yapfast engine (`yapfast-core`, Discord only) for MyTune's Home page.
//! The engine's `Application` sits behind a mutex: UI code reads `state` and calls `dispatch`; one thread per
//! backend pumps events into it. The reference driver is `vendor-src/yapfast/src/ui.rs` (`execute`).
//! Skipped on purpose: desktop toasts (Yapfast has none on Windows either), camera preview in calls.

pub mod ptt;
pub mod settings;
pub use settings::{ChatLayout, InputMode, Keybind, Settings};

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use yapfast_core::application::{Action, Application, Effect};
use yapfast_core::backend::Backend;
use yapfast_core::chat::Start;
use yapfast_core::runtime::{BackgroundRuntime, BackgroundTask};

pub use yapfast_core::application::{Action as ChatAction, AppState};

struct Inner {
    app: Mutex<Application>,
    backend: Mutex<Option<Backend>>,
    link: Mutex<Option<BackgroundTask<()>>>,
    runtime: Arc<BackgroundRuntime>,
    root: PathBuf,
    /// Bumped by every Start/link so a stale pump thread stops applying events.
    run: Mutex<u64>,
    /// The ring of a DM call that rings this account; dropped to stop it.
    ringer: Mutex<Option<yapfast_core::ring::Ringer>>,
    settings: Mutex<Settings>,
    /// The push-to-talk keyboard hook, while push-to-talk is on or a key is being recorded.
    hook: Mutex<Option<ptt::Hook>>,
    /// The call's phase last seen, to unmute or deafen once a call connects, as the settings say.
    phase: Mutex<yapfast_core::voice::Phase>,
    /// A message came that should flash the taskbar; the UI takes it.
    attention: std::sync::atomic::AtomicBool,
    /// The mic was muted when deafening began, so undeafening leaves it muted (else it opens the mic again).
    mic_was_muted: std::sync::atomic::AtomicBool,
    /// Message alerts (who wrote where, and the text) the engine raised, for the UI to show as toasts.
    alerts: Mutex<Vec<yapfast_core::chat::Alert>>,
    repaint: Box<dyn Fn() + Send + Sync>,
}

#[derive(Clone)]
pub struct Chat(Arc<Inner>);

/// The engine mutes the mic on deafen and leaves it muted on undeafen. As Discord: undeafening gives the mic
/// back unless it was muted before (the engine's unmute also undeafens).
fn deafen_command(on: bool, muted: bool, was_muted: &std::sync::atomic::AtomicBool) -> yapfast_core::voice::Command {
    use std::sync::atomic::Ordering::Relaxed;
    use yapfast_core::voice::Command;
    if on {
        was_muted.store(muted, Relaxed);
        Command::Deafen(true)
    } else if was_muted.load(Relaxed) {
        Command::Deafen(false)
    } else {
        Command::Mute(false)
    }
}

#[test]
fn undeafen_restores_mic() {
    use yapfast_core::voice::Command;
    let was = std::sync::atomic::AtomicBool::new(false);
    assert!(matches!(deafen_command(true, false, &was), Command::Deafen(true)));
    assert!(matches!(deafen_command(false, true, &was), Command::Mute(false)));
    assert!(matches!(deafen_command(true, true, &was), Command::Deafen(true)));
    assert!(matches!(deafen_command(false, true, &was), Command::Deafen(false)));
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Chat {
    /// `repaint` is called whenever the state changed. Call `restore` to sign in with a saved link.
    pub fn new(repaint: impl Fn() + Send + Sync + 'static) -> anyhow::Result<Chat> {
        let _ = yapfast_core::init_tls();
        use_readable_frames();
        let root = yapfast_core::default_directory()?;
        std::fs::create_dir_all(&root)?;
        yapfast_core::keep_discord_files(&root);
        let chat = Chat(Arc::new(Inner {
            app: Mutex::new(Application::default()),
            backend: Mutex::new(None),
            link: Mutex::new(None),
            runtime: Arc::new(BackgroundRuntime::new()?),
            root,
            run: Mutex::new(0),
            ringer: Mutex::new(None),
            settings: Mutex::new(Settings::default()),
            hook: Mutex::new(None),
            phase: Mutex::new(Default::default()),
            attention: std::sync::atomic::AtomicBool::new(false),
            mic_was_muted: std::sync::atomic::AtomicBool::new(false),
            alerts: Mutex::new(Vec::new()),
            repaint: Box::new(repaint),
        }));
        chat.set_settings(Settings::load(), false);
        Ok(chat)
    }

    /// A message wants the taskbar flashed (once; the UI flashes only when MyTune is not in front).
    pub fn take_attention(&self) -> bool {
        self.0.attention.swap(false, std::sync::atomic::Ordering::Relaxed)
    }

    /// The message alerts raised since the last call (newest last).
    pub fn take_alerts(&self) -> Vec<yapfast_core::chat::Alert> {
        std::mem::take(&mut *lock(&self.0.alerts))
    }

    /// Reads the settings without copying them.
    pub fn with_settings<R>(&self, f: impl FnOnce(&Settings) -> R) -> R {
        f(&lock(&self.0.settings))
    }

    pub fn settings(&self) -> Settings {
        lock(&self.0.settings).clone()
    }

    /// Saves the chat's layout (only when it changed).
    pub fn set_layout(&self, layout: ChatLayout) {
        let mut s = lock(&self.0.settings);
        if s.layout != layout {
            s.layout = layout;
            s.save();
        }
    }

    /// Keeps a person's call volume and mutes (as the engine holds them now) between runs.
    pub fn keep_person_sound(&self, person: &str) {
        use yapfast_core::volume::{self, Kind};
        let now = (volume::get(Kind::Voice, person), volume::muted(person), volume::soundboard_muted(person));
        let plain = now == (Kind::Voice.default_gain(), false, false);
        let mut s = lock(&self.0.settings);
        let changed = if plain { s.person_sound.remove(person).is_some() } else { s.person_sound.insert(person.to_owned(), now) != Some(now) };
        if changed {
            s.save();
        }
    }

    /// Takes new settings: hands them to the engine, starts or stops the push-to-talk hook, and saves them.
    pub fn set_settings(&self, mut new: Settings, save: bool) {
        if save {
            // the settings screen edits an older copy: people's volumes are changed only in the call menu
            new.person_sound = lock(&self.0.settings).person_sound.clone();
        }
        let old = std::mem::replace(&mut *lock(&self.0.settings), new.clone());
        new.apply();
        let first = !save;
        let mut acts = vec![];
        if first || old.reply_ping != new.reply_ping {
            acts.push(Action::ReplyPing(new.reply_ping));
        }
        if first || old.strip_tracking != new.strip_tracking {
            acts.push(Action::StripTracking(new.strip_tracking));
        }
        if first || old.hide_blocked != new.hide_blocked {
            acts.push(Action::HideBlocked(new.hide_blocked));
        }
        if first {
            acts.push(Action::SetPinned(new.pinned_dms.clone()));
            // without this the engine marks rooms read only in memory and they come back unread on restart
            acts.push(Action::SyncDiscordReads(true));
        }
        for a in acts {
            let effects = lock(&self.0.app).dispatch(a);
            self.execute(effects);
        }
        self.watch_keys();
        if save {
            new.save();
        }
        self.changed();
    }

    /// The hook runs while push-to-talk is on or a key is being recorded.
    fn watch_keys(&self) {
        let s = lock(&self.0.settings);
        let ptt_on = s.input_mode == InputMode::PushToTalk;
        ptt::set_key(if ptt_on { s.push_to_talk_key.as_ref().map(|k| k.scancode) } else { None });
        let want = ptt_on || ptt::recording();
        let mut hook = lock(&self.0.hook);
        if want && hook.is_none() {
            *hook = ptt::Hook::start();
        } else if !want {
            *hook = None;
        }
    }

    /// Records the next key pressed anywhere as the push-to-talk key; poll [`Chat::recorded_key`].
    pub fn record_key(&self, on: bool) {
        ptt::record(on);
        self.watch_keys();
    }

    /// The key recorded, once pressed: saved as the push-to-talk key.
    pub fn recorded_key(&self) -> Option<Keybind> {
        let (scancode, name) = ptt::recorded()?;
        let mut s = self.settings();
        s.push_to_talk_key = Some(Keybind { scancode, name });
        self.set_settings(s.clone(), true);
        s.push_to_talk_key
    }

    /// Plays one of Yapfast's sounds as set, to preview it.
    pub fn preview_sound(&self, sound: yapfast_core::sounds::Sound) {
        let speaker = lock(&self.0.app).state.voice.speaker.clone();
        let s = lock(&self.0.settings).clone();
        yapfast_core::sounds::play(s.sound_set, sound, s.sound_gain(sound), speaker.as_deref());
    }

    /// Reads the state (a short lock: do not hold it across UI calls that block).
    pub fn with_state<R>(&self, f: impl FnOnce(&AppState) -> R) -> R {
        f(&lock(&self.0.app).state)
    }

    /// Reads through the engine itself, for views it builds (`forum_posts`, `forum_info`). Same short lock.
    pub fn with_app<R>(&self, f: impl FnOnce(&Application) -> R) -> R {
        f(&lock(&self.0.app))
    }

    pub fn dispatch(&self, action: Action) {
        let pin = matches!(action, Action::Pin { .. });
        let action = match action {
            Action::Voice(yapfast_core::voice::Command::Deafen(on)) => {
                let muted = lock(&self.0.app).state.voice.muted;
                Action::Voice(deafen_command(on, muted, &self.0.mic_was_muted))
            }
            a => a,
        };
        let effects = lock(&self.0.app).dispatch(action);
        self.execute(effects);
        if pin {
            // pinned DMs are kept in chat-settings.json, as Yapfast keeps them per device
            let pinned = lock(&self.0.app).state.pinned.clone();
            let mut s = lock(&self.0.settings);
            if s.pinned_dms != pinned {
                s.pinned_dms = pinned;
                s.save();
            }
        }
        self.changed();
    }

    /// Rings while a DM call rings this account (not in Do Not Disturb, not in that call), then repaints.
    fn changed(&self) {
        // A call that just connected starts unmuted or deafened as the settings say (it joins muted).
        // one lock for all it reads: the UI holds it for whole frames
        let (now, ring, speaker) = {
            let app = lock(&self.0.app);
            let s = &app.state;
            let v = &s.voice;
            (v.phase, !s.do_not_disturb() && s.rooms.iter().any(|r| r.incoming && !(v.active() && v.room.as_ref() == Some(&r.id))), v.speaker.clone())
        };
        let was = std::mem::replace(&mut *lock(&self.0.phase), now);
        if now == yapfast_core::voice::Phase::Connected && was != now {
            let s = self.settings();
            let act = if s.deafen_on_join { Some(Action::DeafenOnJoin) } else if s.unmute_on_join { Some(Action::UnmuteOnJoin) } else { None };
            if let Some(a) = act {
                // a call joins muted: undeafening later opens the mic only if it would have been unmuted
                self.0.mic_was_muted.store(!s.unmute_on_join, std::sync::atomic::Ordering::Relaxed);
                let effects = lock(&self.0.app).dispatch(a);
                self.execute(effects);
            }
        }
        let mut ringer = lock(&self.0.ringer);
        if !ring {
            *ringer = None;
        } else if ringer.is_none() {
            *ringer = yapfast_core::ring::Ringer::start(yapfast_core::ring::Tone::Incoming, speaker.as_deref()).ok();
        }
        drop(ringer);
        (self.0.repaint)();
    }

    /// Plays one of Yapfast's sounds on the call's speakers; while deafened only deafening's own sounds play.
    fn sound(&self, sound: yapfast_core::sounds::Sound) {
        use yapfast_core::sounds::Sound;
        let (deaf, speaker) = {
            let app = lock(&self.0.app);
            let v = &app.state.voice;
            (v.active() && v.deafened, v.speaker.clone())
        };
        if deaf && !matches!(sound, Sound::Deafen | Sound::Undeafen) {
            return;
        }
        let s = lock(&self.0.settings).clone();
        yapfast_core::sounds::play(s.sound_set, sound, s.sound_gain(sound), speaker.as_deref());
    }

    /// Connects with the Discord login saved on this PC (a no-op error event when there is none).
    pub fn restore(&self) {
        self.execute(vec![Effect::Start(Start::Restore)]);
    }

    /// True while a connection is running or being set up (false again once it ended, as with no saved login).
    pub fn starting(&self) -> bool {
        lock(&self.0.backend).is_some()
    }

    /// Drops the connection and connects again with the saved login (when Discord went offline or the page is stale).
    pub fn reconnect(&self) {
        self.start(Start::Restore);
        self.changed();
    }

    /// Forgets the saved login and returns to the sign-in page, where a new QR code can be linked.
    pub fn log_in_again(&self) {
        *lock(&self.0.run) += 1;
        *lock(&self.0.backend) = None;
        *lock(&self.0.link) = None;
        let _ = yapfast_core::yapfast_discord::token::forget();
        *lock(&self.0.app) = Application::default();
        self.changed();
    }

    fn execute(&self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Start(start) => self.start(start),
                // "Let others see when you're typing" off: the typing signal never leaves
                Effect::Backend(yapfast_core::chat::Command::Typing { .. }) if !lock(&self.0.settings).share_typing => {}
                Effect::Backend(command) => {
                    let sent = match lock(&self.0.backend).as_ref() {
                        Some(b) => b.send(command),
                        None => Err(command),
                    };
                    if let Err(command) = sent {
                        lock(&self.0.app).backend_failed(&command);
                    }
                }
                Effect::LinkDiscord => self.link(None),
                Effect::LinkDiscordToken(t) => self.link(Some(t)),
                Effect::StopDiscordLink => {
                    *lock(&self.0.link) = None;
                }
                Effect::Sound(sound) => self.sound(sound),
                Effect::Notify(alert) => {
                    // A message sounds unless it is in the open conversation; a call rings instead.
                    let reading = lock(&self.0.app).state.selected.as_ref() == Some(&alert.room);
                    if !alert.call && lock(&self.0.settings).flash_taskbar {
                        self.0.attention.store(true, std::sync::atomic::Ordering::Relaxed);
                    }
                    if !alert.call {
                        let mut alerts = lock(&self.0.alerts);
                        alerts.push(alert.clone());
                        // a UI that never drains them (chat page never shown) must not grow this
                        let extra = alerts.len().saturating_sub(16);
                        alerts.drain(..extra);
                    }
                    if !alert.call && !reading {
                        use yapfast_core::sounds::Sound;
                        self.sound(if alert.mention { Sound::Mention } else { Sound::Message });
                    }
                }
                _ => {}
            }
        }
    }

    fn start(&self, start: Start) {
        let run = {
            let mut r = lock(&self.0.run);
            *r += 1;
            *r
        };
        *lock(&self.0.backend) = None;
        let (backend, mut events) = Backend::start(self.0.runtime.clone(), self.0.root.clone(), start);
        *lock(&self.0.backend) = Some(backend);
        let me = self.clone();
        std::thread::spawn(move || {
            while let Some(event) = events.blocking_recv() {
                if *lock(&me.0.run) != run {
                    return;
                }
                // A call sends events in bursts, and the UI holds this lock for each whole frame: one event per
                // lock (then three more locks in `changed`) fell behind while the page kept repainting, as when
                // scrolling. All that wait are applied under one lock, with one repaint after.
                let mut effects = Vec::new();
                let narrate = lock(&me.0.settings).narrate_calls;
                let mut lines = Vec::new();
                {
                    let mut app = lock(&me.0.app);
                    let before = if narrate { in_call(&app.state.voice) } else { None };
                    effects.extend(app.apply(event));
                    for _ in 0..256 {
                        match events.try_recv() {
                            Ok(event) => effects.extend(app.apply(event)),
                            Err(_) => break,
                        }
                    }
                    if let (Some(before), Some(after)) = (before, in_call(&app.state.voice)) {
                        lines = narration(&before, &after);
                    }
                }
                if !lines.is_empty() {
                    say(lines.join(". "), lock(&me.0.settings).sound_volume);
                }
                me.execute(effects);
                me.changed();
            }
            if *lock(&me.0.run) == run {
                lock(&me.0.app).stopped();
                *lock(&me.0.backend) = None;
                me.changed();
            }
        });
    }

    fn link(&self, token: Option<String>) {
        let (task, mut progress) = yapfast_core::discord_link::start(&self.0.runtime, token);
        *lock(&self.0.link) = Some(task);
        let me = self.clone();
        std::thread::spawn(move || {
            while let Some(step) = progress.blocking_recv() {
                let effects = lock(&me.0.app).linking(step);
                me.execute(effects);
                me.changed();
            }
        });
    }
}

pub use yapfast_core;

/// The call this account is in and hears: its room and the other connected people (id, name).
type InCall = (Option<String>, Vec<(String, String)>);

fn in_call(v: &yapfast_core::voice::State) -> Option<InCall> {
    (v.active() && !v.deafened).then(|| (v.room.clone(), v.participants.iter().filter(|p| !p.local && p.connected).map(|p| (p.id.clone(), p.user.clone())).collect()))
}

/// What the narrator says for a change in one call: who joined, who left. Nothing when the call changed.
fn narration(before: &InCall, after: &InCall) -> Vec<String> {
    if before.0 != after.0 {
        return vec![];
    }
    let gone = |list: &InCall, id: &str| !list.1.iter().any(|(i, _)| i == id);
    let joined = after.1.iter().filter(|(id, _)| gone(before, id)).map(|(_, n)| format!("{n} joined"));
    let left = before.1.iter().filter(|(id, _)| gone(after, id)).map(|(_, n)| format!("{n} left"));
    joined.chain(left).collect()
}

/// Speaks a line with Windows' own voice, from a thread. The text is other people's names: it goes in as an
/// environment variable, never as part of the script.
fn say(line: String, volume: u8) {
    std::thread::spawn(move || {
        use std::os::windows::process::CommandExt;
        let script = "Add-Type -AssemblyName System.Speech; $s = New-Object System.Speech.Synthesis.SpeechSynthesizer; $s.Volume = [int]$env:MYTUNE_SAY_VOLUME; $s.Speak($env:MYTUNE_SAY)";
        let _ = std::process::Command::new("powershell").args(["-NoProfile", "-WindowStyle", "Hidden", "-Command", script]).env("MYTUNE_SAY", line.replace('\0', "")).env("MYTUNE_SAY_VOLUME", volume.min(100).to_string()).creation_flags(0x0800_0000).output();
    });
}

#[cfg(test)]
mod narrator_tests {
    use super::*;

    #[test]
    fn says_who_joined_and_left_one_call() {
        let p = |id: &str| (id.to_owned(), id.to_uppercase());
        let a = (Some("r".to_owned()), vec![p("a"), p("b")]);
        let b = (Some("r".to_owned()), vec![p("b"), p("c")]);
        assert_eq!(narration(&a, &b), ["C joined", "A left"]);
        assert!(narration(&a, &(Some("other".to_owned()), vec![])).is_empty());
    }
}

/// A video frame buffer the UI can read back (the engine's own one only offers `pixels_mut`).
struct Rgba {
    size: (u32, u32),
    px: Vec<u8>,
}

impl yapfast_core::picture::FrameBuffer for Rgba {
    fn size(&self) -> (u32, u32) {
        self.size
    }
    fn pixels_mut(&mut self) -> &mut [u8] {
        &mut self.px
    }
}

/// Makes every camera frame the engine writes readable by [`frame_pixels`]. Call once at startup.
pub fn use_readable_frames() {
    yapfast_core::picture::set_frame_buffers(|w, h| Box::new(Rgba { size: (w, h), px: vec![0; w as usize * h as usize * 4] }));
}

/// The size and RGBA pixels of a camera frame taken from a `picture::Feed`.
pub fn frame_pixels(fb: &dyn yapfast_core::picture::FrameBuffer) -> Option<((u32, u32), &[u8])> {
    let any: &dyn std::any::Any = fb;
    any.downcast_ref::<Rgba>().map(|f| (f.size, f.px.as_slice()))
}

/// Runs the Go Live / screen-picture child process when this exe was started as one; returns false otherwise.
/// Yapfast's engine starts the current exe with `--stream-worker`, so `main` must call this first.
pub fn run_stream_worker() -> bool {
    if std::env::args().nth(1).as_deref() != Some(yapfast_core::stream_worker::ARGUMENT) {
        return false;
    }
    let code = i32::from(yapfast_core::stream_worker::run().is_err());
    std::process::exit(code);
}

/// A QR code for `text`: its side length in modules and the dark ones, row by row.
pub fn qr(text: &str) -> Option<(usize, Vec<bool>)> {
    let code = qrcode::QrCode::new(text.as_bytes()).ok()?;
    let w = code.width();
    Some((w, code.to_colors().into_iter().map(|c| c == qrcode::Color::Dark).collect()))
}
