//! NVIDIA driver-profile tweaks (port of C# `NvidiaTweaks.cs`): global low-latency settings on NVIDIA's
//! Base Profile, the G-SYNC/VRR switch, and the per-game profile.
//!
//! Backup data, same as C#: `nv:<settingId>` = previous user value as decimal, or `default` when the profile
//! had no user value (so revert removes it again). Per game also `createdProfile` / `existingProfile`.
//!
//! `Target` lets tests aim the very same code at a throwaway profile instead of the Base Profile.
//!
//! Differences from C# (never weaker):
//! - Revert reports failures (collected, backup kept) where C# swallowed every error. Restoring a setting to
//!   its default counts as done if the setting is not set / not supported (nothing to restore).
//! - A previous user value that cannot be written back is a failure; C# deleted the setting instead, which
//!   silently changes it to the default.
//! - Deleting a created game profile that fails is an error; C# only logged it.

use crate::backup::TweakBackup;
use crate::engine::Tweak;
use crate::error::{Error, Result};
use crate::log;
use crate::nvapi::{self, Profile, Session, SettingValue, NOT_SUPPORTED, SETTING_NOT_FOUND};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

pub const MAX_PRE_RENDERED_FRAMES: u32 = 8102046;
pub const PREFERRED_REFRESH_RATE: u32 = 6600001;
pub const VERTICAL_SYNC: u32 = 11041231;
pub const POWER_MANAGEMENT_MODE: u32 = 274197361;
pub const TEXTURE_FILTERING_QUALITY: u32 = 13510289;
pub const TRILINEAR_OPTIMIZATION: u32 = 3066610;
pub const ANISO_FILTER_OPTIMIZATION: u32 = 8703344;
pub const ANISO_SAMPLE_OPTIMIZATION: u32 = 15151633;
pub const SHADER_CACHE_SIZE: u32 = 11306135;
pub const ANSEL: u32 = 276158834;
pub const TRIPLE_BUFFERING: u32 = 553505273;
pub const FXAA: u32 = 276089202;
pub const THREADED_OPTIMIZATION: u32 = 549528094;
/// "Frame Rate Limiter": plain fps, 0 = off.
pub const FRAME_RATE_LIMITER: u32 = 0x10835002;
const VSYNC_FORCE_OFF: u32 = 0x08416747;

pub type Setting = (u32, u32, &'static str);

/// Global low-latency settings (Base Profile).
pub const GLOBAL_SETTINGS: [Setting; 12] = [
    (POWER_MANAGEMENT_MODE, 1, "Power management: prefer maximum performance"),
    (MAX_PRE_RENDERED_FRAMES, 1, "Low latency mode: on (1 pre-rendered frame)"),
    (VERTICAL_SYNC, VSYNC_FORCE_OFF, "Vertical sync: off"),
    (TRIPLE_BUFFERING, 0, "Triple buffering: off"),
    (PREFERRED_REFRESH_RATE, 1, "Preferred refresh rate: highest available"),
    (TEXTURE_FILTERING_QUALITY, 20, "Texture filtering quality: high performance"),
    (TRILINEAR_OPTIMIZATION, 1, "Trilinear optimization: on"),
    (ANISO_FILTER_OPTIMIZATION, 1, "Anisotropic filter optimization: on"),
    (ANISO_SAMPLE_OPTIMIZATION, 1, "Anisotropic sample optimization: on"),
    (SHADER_CACHE_SIZE, 0xFFFF_FFFF, "Shader cache size: unlimited"),
    (FXAA, 0, "FXAA: off"),
    (ANSEL, 0, "Ansel: off (older drivers only)"),
];

/// Per-game settings: the global ones plus threaded optimization.
pub fn game_settings() -> Vec<Setting> {
    let mut v = GLOBAL_SETTINGS.to_vec();
    v.push((THREADED_OPTIMIZATION, 1, "Threaded optimization: on"));
    v
}

/// Driver-level G-SYNC / variable refresh switches (0 = off, default on).
pub const GSYNC_IDS: [u32; 3] = [0x1194F158, 0x1094F157, 0x1094F1F7];

/// Which profile the tweak edits. `Named` exists for tests (a throwaway profile).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    Base,
    Named(String),
}

fn profile_of(s: &Session, t: &Target) -> Result<Profile> {
    match t {
        Target::Base => s.base_profile(),
        Target::Named(n) => s.find_profile(n)?.ok_or_else(|| Error::Catalog(format!("NVIDIA profile `{n}` not found"))),
    }
}

pub fn available() -> bool {
    nvapi::available()
}

pub const UNAVAILABLE: &str = "No NVIDIA graphics driver found.";

/// Ids the installed driver rejects FOR THIS PROFILE (e.g. Ansel is refused on the Base Profile but accepted
/// on a custom one). Found once per target by trying every game setting in a session that is never saved
/// (its own session, so call it BEFORE opening the session you will save).
fn unsupported_ids(t: &Target) -> Result<HashSet<u32>> {
    static CACHE: Mutex<Option<HashMap<Target, HashSet<u32>>>> = Mutex::new(None);
    let mut c = CACHE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(set) = c.as_ref().and_then(|m| m.get(t)) {
        return Ok(set.clone());
    }
    let s = Session::open()?;
    let p = profile_of(&s, t)?;
    let mut set = HashSet::new();
    for (id, value, _) in game_settings() {
        match s.set(p, id, value) {
            Err(e) if e.nvapi_status() == Some(NOT_SUPPORTED) => {
                set.insert(id);
            }
            _ => {}
        }
    }
    if !set.is_empty() {
        log::info(&format!("NVIDIA driver doesn't support settings: {set:?}"));
    }
    c.get_or_insert_with(HashMap::new).insert(t.clone(), set.clone());
    Ok(set)
}

fn current(s: &Session, p: Profile, id: u32) -> Result<Option<u32>> {
    Ok(s.get(p, id)?.map(|v: SettingValue| v.value))
}

fn matches(s: &Session, p: Profile, settings: &[Setting], unsupported: &HashSet<u32>) -> Result<bool> {
    for (id, value, _) in settings {
        if unsupported.contains(id) {
            continue;
        }
        if current(s, p, *id)? != Some(*value) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Sets the settings, recording each previous user value (or `default`) once.
fn apply_settings(s: &Session, p: Profile, settings: &[Setting], b: &mut TweakBackup, unsupported: &HashSet<u32>) -> Result<()> {
    for (id, value, name) in settings {
        if unsupported.contains(id) {
            continue;
        }
        let key = format!("nv:{id}");
        if !b.data.contains_key(&key) {
            let prev = match s.get(p, *id)? {
                Some(v) if v.location == SettingValue::LOCATION_CURRENT_PROFILE && !v.is_current_predefined => v.value.to_string(),
                _ => "default".to_string(),
            };
            b.data.insert(key.clone(), prev);
        }
        match s.set(p, *id, *value) {
            Ok(()) => {}
            Err(e) if e.nvapi_status() == Some(NOT_SUPPORTED) => {
                log::info(&format!("NVIDIA driver doesn't support '{name}' ({id}); skipped"));
                b.data.remove(&key);
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Puts settings back (`b` = None: all to default). Tries all, then returns every failure together.
fn restore_settings(s: &Session, p: Profile, ids: &[u32], b: Option<&TweakBackup>, unsupported: &HashSet<u32>) -> Result<()> {
    let mut failed = Vec::new();
    for id in ids {
        if unsupported.contains(id) {
            continue;
        }
        let prev = b.and_then(|b| b.data.get(&format!("nv:{id}"))).map(String::as_str).unwrap_or("default");
        let done = if prev == "default" {
            match s.restore_default(p, *id) {
                Ok(()) => Ok(()),
                Err(e) if matches!(e.nvapi_status(), Some(SETTING_NOT_FOUND) | Some(NOT_SUPPORTED)) => Ok(()),
                Err(_) => match s.delete_setting(p, *id) {
                    Err(e) if e.nvapi_status() != Some(SETTING_NOT_FOUND) => Err(e),
                    _ => Ok(()),
                },
            }
        } else {
            match prev.parse::<u32>() {
                Ok(v) => s.set(p, *id, v),
                Err(_) => Err(Error::BadBackup(format!("nv:{id} = `{prev}`"))),
            }
        };
        if let Err(e) = done {
            failed.push(format!("setting {id}: {e}"));
        }
    }
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Error::Revert(failed))
    }
}

fn ids_of(settings: &[Setting]) -> Vec<u32> {
    settings.iter().map(|s| s.0).collect()
}

// ---------------------------------------------------------------- global profile

pub struct NvidiaGlobalProfileTweak {
    pub target: Target,
}

impl NvidiaGlobalProfileTweak {
    pub fn base() -> Self {
        NvidiaGlobalProfileTweak { target: Target::Base }
    }
}

impl Tweak for NvidiaGlobalProfileTweak {
    fn id(&self) -> &str {
        "general.nvidia"
    }
    fn title(&self) -> &str {
        "NVIDIA global low-latency profile"
    }
    fn is_applied(&self) -> Result<bool> {
        if !available() {
            return Ok(false);
        }
        let un = unsupported_ids(&self.target)?;
        let s = Session::open()?;
        matches(&s, profile_of(&s, &self.target)?, &GLOBAL_SETTINGS, &un)
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        let un = unsupported_ids(&self.target)?;
        let s = Session::open()?;
        apply_settings(&s, profile_of(&s, &self.target)?, &GLOBAL_SETTINGS, b, &un)?;
        s.save()
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        let un = unsupported_ids(&self.target)?;
        let s = Session::open()?;
        let r = restore_settings(&s, profile_of(&s, &self.target)?, &ids_of(&GLOBAL_SETTINGS), Some(b), &un);
        s.save()?; // keep what was restored even if some settings failed
        r
    }
}

// ---------------------------------------------------------------- G-SYNC

/// On = G-SYNC / VRR enabled in the driver (NVIDIA's default); off = switched off. Revert is NOT "put the old
/// value back": off is the state the user picks on purpose (same as C#).
pub struct NvidiaGsyncTweak {
    pub target: Target,
}

impl NvidiaGsyncTweak {
    pub fn base() -> Self {
        NvidiaGsyncTweak { target: Target::Base }
    }
}

impl Tweak for NvidiaGsyncTweak {
    fn id(&self) -> &str {
        "general.gsync"
    }
    fn title(&self) -> &str {
        "NVIDIA G-SYNC / VRR in the driver"
    }
    fn is_applied(&self) -> Result<bool> {
        if !available() {
            return Ok(false);
        }
        let s = Session::open()?;
        let p = profile_of(&s, &self.target)?;
        for id in GSYNC_IDS {
            if current(&s, p, id)? == Some(0) {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn apply(&self, _: &mut TweakBackup) -> Result<()> {
        let s = Session::open()?;
        let r = restore_settings(&s, profile_of(&s, &self.target)?, &GSYNC_IDS, None, &HashSet::new());
        s.save()?;
        r
    }
    fn revert(&self, _: &TweakBackup) -> Result<()> {
        let s = Session::open()?;
        let off: Vec<Setting> = GSYNC_IDS.iter().map(|id| (*id, 0, "G-SYNC / VRR: off")).collect();
        apply_settings(&s, profile_of(&s, &self.target)?, &off, &mut TweakBackup::default(), &HashSet::new())?;
        s.save()
    }
}

// ---------------------------------------------------------------- per game

/// Low-latency profile for one game executable. `frame_cap` fps added to the profile; 0 = none.
pub struct NvidiaGameTweak {
    pub id: String,
    pub exe: String,
    pub game_name: String,
    pub frame_cap: u32,
}

impl NvidiaGameTweak {
    fn profile_name(&self) -> String {
        format!("MyTune - {}", self.game_name)
    }

    fn settings(&self) -> Vec<Setting> {
        let mut v = game_settings();
        if self.frame_cap > 0 {
            v.push((FRAME_RATE_LIMITER, self.frame_cap, "Frame rate limiter"));
        }
        v
    }
}

impl Tweak for NvidiaGameTweak {
    fn id(&self) -> &str {
        &self.id
    }
    fn title(&self) -> &str {
        &self.game_name
    }
    fn is_applied(&self) -> Result<bool> {
        if !available() {
            return Ok(false);
        }
        let un = unsupported_ids(&Target::Base)?;
        let s = Session::open()?;
        match s.find_application(&self.exe)? {
            Some(p) => matches(&s, p, &self.settings(), &un),
            None => Ok(false),
        }
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        let un = unsupported_ids(&Target::Base)?;
        let s = Session::open()?;
        let p = match s.find_application(&self.exe)? {
            Some(p) => {
                if !b.data.contains_key("createdProfile") {
                    b.data.insert("existingProfile".into(), s.profile_name(p)?);
                }
                p
            }
            None => {
                let p = s.create_profile(&self.profile_name())?;
                s.create_application(p, &self.exe)?;
                b.data.insert("createdProfile".into(), self.profile_name());
                p
            }
        };
        apply_settings(&s, p, &self.settings(), b, &un)?;
        s.save()
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        let un = unsupported_ids(&Target::Base)?;
        let s = Session::open()?;
        if let Some(created) = b.data.get("createdProfile") {
            if let Some(p) = s.find_profile(created)? {
                s.delete_profile(p)?;
            }
            return s.save();
        }
        let mut ids = ids_of(&game_settings());
        // the limiter is only restored when this tweak set it, so a cap the user set on an existing profile survives
        if b.data.contains_key(&format!("nv:{FRAME_RATE_LIMITER}")) {
            ids.push(FRAME_RATE_LIMITER);
        }
        let r = match s.find_application(&self.exe)? {
            Some(p) => restore_settings(&s, p, &ids, Some(b), &un),
            None => Ok(()),
        };
        s.save()?;
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_match_csharp() {
        assert_eq!(GLOBAL_SETTINGS.len(), 12);
        assert_eq!(game_settings().len(), 13);
        assert_eq!(GLOBAL_SETTINGS[2], (11041231, 0x08416747, "Vertical sync: off"));
        assert_eq!(GLOBAL_SETTINGS[9].1, u32::MAX);
        assert_eq!(GSYNC_IDS, [0x1194F158, 0x1094F157, 0x1094F1F7]);
        let ids: HashSet<_> = GLOBAL_SETTINGS.iter().map(|s| s.0).collect();
        assert_eq!(ids.len(), 12, "unique setting ids");
    }
}
