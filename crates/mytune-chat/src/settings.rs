//! The chat's own settings, as Yapfast keeps them in its `layout.json` (src/ui.rs `Layout`), but in MyTune's
//! folder (`%ProgramData%\MyTune\chat-settings.json`), so the installed Yapfast app's file is never touched.
//! `apply` hands the audio ones to the engine; the rest are read where they matter (sounds, typing, the page).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use yapfast_core::noise::{Strength, Suppressor};
use yapfast_core::sounds::{Sound, SoundSet};

/// How the microphone opens.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputMode {
    #[default]
    VoiceActivity,
    PushToTalk,
}

/// The push-to-talk key: its scan code (with 0xE000 for extended keys, as Yapfast stores it) and its name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keybind {
    pub scancode: u32,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    // Voice & video
    pub microphone_volume: u8,
    pub speaker_volume: u8,
    pub unmute_on_join: bool,
    pub deafen_on_join: bool,
    pub level_voice: bool,
    pub noise_suppression: Suppressor,
    pub noise_strength: Strength,
    pub reduce_room_echo: bool,
    pub input_sensitivity: i32,
    pub input_mode: InputMode,
    pub push_to_talk_key: Option<Keybind>,
    pub push_to_talk_release_delay: u32,
    // Messages
    pub twelve_hour_clock: bool,
    pub clock_seconds: bool,
    /// 0 small, 1 normal, 2 large.
    pub text_size: u8,
    pub link_previews: bool,
    pub reply_ping: bool,
    pub strip_tracking: bool,
    pub share_typing: bool,
    pub hide_blocked: bool,
    // Notifications
    pub notifications: bool,
    pub notification_text: bool,
    pub flash_taskbar: bool,
    // Sounds
    pub sound_set: SoundSet,
    pub sound_volume: u8,
    pub adjust_each_sound: bool,
    pub each_sound: BTreeMap<Sound, u8>,
    /// How loud the message and mention sounds are, on top of the master volume (0-200).
    pub notification_volume: u8,
    /// Says aloud who joins and leaves the call this account is in.
    pub narrate_calls: bool,
    // Game activity: (name, executable)
    pub game_executables: Vec<(String, String)>,
    // Advanced
    pub gpu_encoding: bool,
    /// DMs pinned to the top of the DM list, by room id (Yapfast keeps them per device too).
    pub pinned_dms: Vec<String>,
    /// How the chat was laid out when last left: restored at start.
    pub layout: ChatLayout,
    /// How loud each person is in calls for this account, by user id: (voice gain, muted, soundboard
    /// muted). Only people changed from the default are listed.
    pub person_sound: BTreeMap<String, (f32, bool, bool)>,
    /// How loud each person's stream (Go Live) is for this account, by user id: (gain, muted). Only
    /// streams changed from the default are listed.
    pub person_stream: BTreeMap<String, (f32, bool)>,
}

/// Streams changed from the default, by user id: (gain chosen, muted). The engine has a stream volume but
/// no stream mute, so a muted stream is handed to it as gain 0 and the chosen gain is kept here.
static STREAM_SOUND: std::sync::Mutex<BTreeMap<String, (f32, bool)>> = std::sync::Mutex::new(BTreeMap::new());

/// How loud `person`'s stream is, as chosen, and whether it is muted.
pub fn stream_sound(person: &str) -> (f32, bool) {
    let map = STREAM_SOUND.lock().unwrap_or_else(|e| e.into_inner());
    map.get(person).copied().unwrap_or((yapfast_core::volume::Kind::Stream.default_gain(), false))
}

/// Sets how loud `person`'s stream is and whether it is muted; the engine plays it so at once.
pub fn set_stream_sound(person: &str, gain: f32, muted: bool) {
    use yapfast_core::volume::{self, Kind};
    let plain = !muted && (gain - Kind::Stream.default_gain()).abs() < f32::EPSILON;
    {
        let mut map = STREAM_SOUND.lock().unwrap_or_else(|e| e.into_inner());
        if plain {
            map.remove(person);
        } else {
            map.insert(person.to_owned(), (gain, muted));
        }
    }
    volume::set(Kind::Stream, person, if muted { 0.0 } else { gain });
}

/// The chat page's layout, kept between runs: the member list and profile panel shown or not, the server
/// rail and conversation list hidden or not, and the width of each panel dragged to a size (by panel key).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChatLayout {
    pub members_open: bool,
    /// The profile beside a one-to-one DM; `None` until it was toggled once.
    pub profile_panel: Option<bool>,
    pub lists_hidden: bool,
    pub widths: BTreeMap<String, f32>,
    /// The servers on the rail in the order they were dragged to, by id (this device only: the engine
    /// cannot write Discord's own order). Servers not listed come first.
    pub server_order: Vec<String>,
}

impl Default for ChatLayout {
    fn default() -> Self {
        Self { members_open: true, profile_panel: None, lists_hidden: false, widths: BTreeMap::new(), server_order: vec![] }
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            microphone_volume: 100,
            speaker_volume: 100,
            unmute_on_join: true,
            deafen_on_join: false,
            level_voice: true,
            noise_suppression: Suppressor::default(),
            noise_strength: Strength::default(),
            reduce_room_echo: false,
            input_sensitivity: -60,
            input_mode: InputMode::VoiceActivity,
            push_to_talk_key: None,
            push_to_talk_release_delay: 20,
            twelve_hour_clock: false,
            clock_seconds: false,
            text_size: 1,
            link_previews: true,
            reply_ping: true,
            strip_tracking: true,
            share_typing: true,
            hide_blocked: false,
            notifications: true,
            notification_text: true,
            flash_taskbar: true,
            sound_set: SoundSet::default(),
            sound_volume: 100,
            adjust_each_sound: false,
            each_sound: BTreeMap::new(),
            notification_volume: 100,
            narrate_calls: false,
            game_executables: vec![],
            gpu_encoding: true,
            pinned_dms: vec![],
            layout: ChatLayout::default(),
            person_sound: BTreeMap::new(),
            person_stream: BTreeMap::new(),
        }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        mytune_core::util::program_data().join("MyTune").join("chat-settings.json")
    }

    /// The saved settings, or the defaults when there are none or the file is unreadable.
    pub fn load() -> Self {
        std::fs::read(Self::path()).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            // write beside, then swap in, so a crash never leaves half a file
            let tmp = path.with_extension("json.tmp");
            if std::fs::write(&tmp, bytes).is_ok() {
                let _ = std::fs::rename(&tmp, &path);
            }
        }
    }

    /// Hands the audio and stream settings to the engine, which reads them on each audio block.
    pub fn apply(&self) {
        use yapfast_core::{leveling, noise, push_to_talk, stream_worker, volume};
        volume::set_microphone(volume::from_slider(f32::from(self.microphone_volume)));
        volume::set_speakers(volume::from_slider(f32::from(self.speaker_volume)));
        for (person, &(gain, muted, quiet)) in &self.person_sound {
            volume::set(volume::Kind::Voice, person, gain);
            volume::mute(person, muted);
            volume::mute_soundboard(person, quiet);
        }
        for (person, &(gain, muted)) in &self.person_stream {
            set_stream_sound(person, gain, muted);
        }
        leveling::set_enabled(self.level_voice);
        noise::set(self.noise_suppression);
        noise::set_strength(self.noise_strength);
        noise::set_dereverb(self.reduce_room_echo);
        noise::set_sensitivity(self.input_sensitivity.clamp(-80, 0) as f32);
        push_to_talk::set_on(self.input_mode == InputMode::PushToTalk);
        push_to_talk::set_release_delay(self.push_to_talk_release_delay.min(2000));
        stream_worker::set_hardware_encoding(self.gpu_encoding);
        yapfast_core::discord::set_custom_games(&self.game_executables);
    }

    /// How loud one of Yapfast's sounds plays: the master volume, then its own when adjusted one by one.
    pub fn sound_gain(&self, sound: Sound) -> f32 {
        let own = if self.adjust_each_sound { self.each_sound.get(&sound).copied().unwrap_or(100) } else { 100 };
        let note = if matches!(sound, Sound::Message | Sound::Mention) { self.notification_volume } else { 100 };
        yapfast_core::sounds::gain(self.sound_volume) * yapfast_core::sounds::gain(own) * yapfast_core::sounds::gain(note)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_and_fills_missing_fields() {
        let mut s = Settings { twelve_hour_clock: true, sound_volume: 150, ..Default::default() };
        s.each_sound.insert(Sound::Mention, 40);
        let back: Settings = serde_json::from_slice(&serde_json::to_vec(&s).unwrap()).unwrap();
        assert_eq!(back, s);
        let partial: Settings = serde_json::from_str(r#"{"share_typing": false}"#).unwrap();
        assert!(!partial.share_typing && partial.unmute_on_join && partial.sound_volume == 100);
    }

    #[test]
    fn old_settings_have_no_shortcuts() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert!(s.sound_shortcuts.is_empty());
    }

    #[test]
    fn muted_stream_is_silent_and_keeps_its_volume() {
        use yapfast_core::volume::{self, Kind};
        set_stream_sound("stream-test-1", 0.5, true);
        assert_eq!((stream_sound("stream-test-1"), volume::get(Kind::Stream, "stream-test-1")), ((0.5, true), 0.0));
        set_stream_sound("stream-test-1", 0.5, false);
        assert_eq!(volume::get(Kind::Stream, "stream-test-1"), 0.5);
        assert_eq!(volume::get(Kind::Voice, "stream-test-1"), 1.0); // the voice is untouched
    }

    #[test]
    fn each_sound_counts_only_while_adjusting_each() {
        let mut s = Settings::default();
        s.each_sound.insert(Sound::Join, 50);
        assert_eq!(s.sound_gain(Sound::Join), 1.0);
        s.adjust_each_sound = true;
        assert!(s.sound_gain(Sound::Join) < 0.3);
    }
}
