//! Data-driven registry tweaks, loaded from `data/catalog.json`.
//!
//! `data/catalog.json` is NOT hand-written: it is a dump of the real C# `Catalog.All` / `Catalog.Removed`
//! (every tweak's id, title, flags and registry `RegSet` list), produced by a throwaway C# console project
//! that compiles `Core\`, `Tweaks\` and `Services\` without starting the app. Regenerate it after the C#
//! catalog changes; `catalog_matches_expected_shape` in the tests will tell if the shape drifts.
//!
//! Ported here: every plain `RegistryTweak` (57 active + 9 Removed stubs that exist only so old
//! backups can still be reverted). Not here (separate code): tweaks that run commands/COM/NVAPI, `adv.msi`
//! (devices enumerated at runtime) and `TimerResolutionTweak` (also writes an app setting).
//!
//! Differences from C# `RegistryTweak` (revert is never weaker):
//! - `Default` values are written when a tweak is turned off without any backup (set outside MyTune), so off is
//!   the opposite of on; C# writes them when a backup exists but holds no registry entries, here that case
//!   restores nothing. `AfterFallbackRevert` (DeleteTree for classic menu / take ownership)
//!   is dropped for the same reason: the captured `CreatedKeys` already remove keys the tweak created.
//! - `is_applied` is false when no set is applicable (all `OnlyIfKeyExists` keys missing); C# is vacuously true.
//! - The Explorer restart that the C# `Engine` does for `RestartsExplorer` tweaks is done here, after
//!   apply and revert, through `Hooks`.

use crate::backup::TweakBackup;
use crate::engine::Tweak;
use crate::error::{Error, Result};
use crate::reg::{self, Hive, RegValue};
use crate::util::b64_decode;
use serde::Deserialize;
use serde_json::Value;
use std::sync::Arc;

const CATALOG_JSON: &str = include_str!("../data/catalog.json");

// ---------------------------------------------------------------- flags

/// Same bit values as C# `TweakFlags`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TweakFlags(pub u32);

impl TweakFlags {
    pub const RESTART: u32 = 1;
    pub const SIGN_OUT: u32 = 2;
    pub const RESTARTS_EXPLORER: u32 = 4;
    pub const SECURITY_RISK: u32 = 8;
    pub const EXPERIMENTAL: u32 = 16;
    pub const NETWORK_BLIP: u32 = 32;
    pub const NEW: u32 = 64;

    /// Parses C# `Flags.ToString()`: `"None"` or `"Restart, SignOut"`.
    pub fn parse(s: &str) -> Result<TweakFlags> {
        let mut bits = 0;
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            bits |= match part {
                "None" => 0,
                "Restart" => Self::RESTART,
                "SignOut" => Self::SIGN_OUT,
                "RestartsExplorer" => Self::RESTARTS_EXPLORER,
                "SecurityRisk" => Self::SECURITY_RISK,
                "Experimental" => Self::EXPERIMENTAL,
                "NetworkBlip" => Self::NETWORK_BLIP,
                "New" => Self::NEW,
                other => return Err(Error::Catalog(format!("unknown flag {other}"))),
            };
        }
        Ok(TweakFlags(bits))
    }
    pub fn has(self, bit: u32) -> bool {
        self.0 & bit != 0
    }
    /// Not part of the recommended setup when risky.
    pub fn recommended(self) -> bool {
        !self.has(Self::SECURITY_RISK | Self::EXPERIMENTAL)
    }
}

// ---------------------------------------------------------------- hooks

/// Side effects that are not registry values. Tests use a recording fake.
pub trait Hooks: Send + Sync {
    /// WM_SETTINGCHANGE to all windows (apps pick up user settings).
    fn broadcast_setting_change(&self);
    /// Re-reads the DragFullWindows registry value live (SPI_SETDRAGFULLWINDOWS), then broadcasts.
    fn apply_drag_full_windows(&self);
    fn restart_explorer(&self);
}

pub struct SystemHooks;

impl Hooks for SystemHooks {
    fn broadcast_setting_change(&self) {
        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        use windows::Win32::UI::WindowsAndMessaging::{SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE};
        let wide = |s: &str| -> Vec<u16> { s.encode_utf16().chain(Some(0)).collect() };
        for text in [Some("ImmersiveColorSet"), Some("Policy"), None] {
            let w = text.map(wide);
            let lp = w.as_ref().map(|v| v.as_ptr() as isize).unwrap_or(0);
            unsafe {
                let _ = SendMessageTimeoutW(HWND_BROADCAST, WM_SETTINGCHANGE, WPARAM(0), LPARAM(lp), SMTO_ABORTIFHUNG, 1000, None);
            }
            let _: Option<HWND> = None;
        }
    }

    fn apply_drag_full_windows(&self) {
        use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_SETDRAGFULLWINDOWS, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS};
        // "0" means outline dragging, i.e. full-window drag off; missing means on.
        let outline = matches!(reg::get(Hive::Hkcu, "Control Panel\\Desktop", "DragFullWindows"), Ok(Some(RegValue::String(s))) if s == "0");
        unsafe {
            let _ = SystemParametersInfoW(SPI_SETDRAGFULLWINDOWS, if outline { 0 } else { 1 }, None, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0x03));
        }
        self.broadcast_setting_change();
    }

    fn restart_explorer(&self) {
        use crate::process::run;
        let _ = run("taskkill", &["/F", "/IM", "explorer.exe"]);
        std::thread::sleep(std::time::Duration::from_millis(800));
        let running = run("tasklist", &["/FI", "IMAGENAME eq explorer.exe", "/NH"]).map(|r| r.output.contains("explorer.exe")).unwrap_or(false);
        if !running {
            let win = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
            let _ = std::process::Command::new(std::path::PathBuf::from(win).join("explorer.exe")).spawn();
        }
    }
}

// ---------------------------------------------------------------- raw JSON

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: String,
    pub cls: String,
    pub removed: bool,
    pub title: String,
    pub description: Option<String>,
    pub details: Option<String>,
    pub flags: String,
    pub registry: bool,
    pub provider: bool,
    pub after_change: bool,
    pub sets: Option<Vec<SetDef>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetDef {
    pub hive: String,
    pub path: String,
    pub name: String,
    pub kind: String,
    pub value: Value,
    /// Windows default; null means the value does not exist by default.
    #[serde(default)]
    pub default: Value,
    pub only_if_key_exists: bool,
}

/// All tweaks of the C# catalog (active and Removed), as dumped.
pub fn entries() -> Result<Vec<Entry>> {
    serde_json::from_str(CATALOG_JSON).map_err(|e| Error::Catalog(e.to_string()))
}

fn to_reg_value(kind: &str, v: &Value) -> Result<RegValue> {
    let bad = || Error::Catalog(format!("bad {kind} value {v}"));
    Ok(match kind {
        "DWord" => RegValue::Dword(v.as_i64().ok_or_else(bad)? as u32),
        "QWord" => RegValue::Qword(v.as_i64().ok_or_else(bad)? as u64),
        "String" => RegValue::String(v.as_str().ok_or_else(bad)?.to_string()),
        "ExpandString" => RegValue::ExpandString(v.as_str().ok_or_else(bad)?.to_string()),
        "MultiString" => RegValue::MultiString(
            v.as_array().ok_or_else(bad)?.iter().map(|s| s.as_str().map(String::from).ok_or_else(bad)).collect::<Result<_>>()?,
        ),
        "Binary" => RegValue::Binary(b64_decode(v.as_str().ok_or_else(bad)?).ok_or_else(bad)?),
        _ => return Err(bad()),
    })
}

// ---------------------------------------------------------------- tweak

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum After {
    None,
    BroadcastSettingChange,
    DragFullWindows,
}

#[derive(Debug, Clone)]
pub struct DataSet {
    pub hive: Hive,
    pub path: String,
    pub name: String,
    pub value: RegValue,
    /// Skip this value when its key does not exist (e.g. a service that is not installed).
    pub only_if_key_exists: bool,
    /// Windows default for a revert without backup: `Some(None)` deletes the value, `None` means unknown.
    pub default: Option<Option<RegValue>>,
}

pub struct DataTweak {
    pub id: String,
    pub title: String,
    pub description: String,
    pub details: String,
    pub flags: TweakFlags,
    /// True for Removed tweaks: kept only so an old backup can still be reverted.
    pub removed: bool,
    pub sets: Vec<DataSet>,
    pub after: After,
    pub hooks: Arc<dyn Hooks>,
}

impl DataTweak {
    fn applicable(&self) -> Result<Vec<&DataSet>> {
        let mut v = Vec::new();
        for s in &self.sets {
            if !s.only_if_key_exists || reg::key_exists(s.hive, &s.path)? {
                v.push(s);
            }
        }
        Ok(v)
    }

    fn after_change(&self) {
        match self.after {
            After::None => {}
            After::BroadcastSettingChange => self.hooks.broadcast_setting_change(),
            After::DragFullWindows => self.hooks.apply_drag_full_windows(),
        }
        if self.flags.has(TweakFlags::RESTARTS_EXPLORER) {
            self.hooks.restart_explorer();
        }
    }
}

impl Tweak for DataTweak {
    fn id(&self) -> &str {
        &self.id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn is_applied(&self) -> Result<bool> {
        let sets = self.applicable()?;
        if sets.is_empty() {
            return Ok(false);
        }
        for s in sets {
            if reg::get(s.hive, &s.path, &s.name)?.as_ref() != Some(&s.value) {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        for s in self.applicable()? {
            b.capture(s.hive, &s.path, &s.name)?;
            if reg::get(s.hive, &s.path, &s.name)?.as_ref() != Some(&s.value) {
                reg::set(s.hive, &s.path, &s.name, &s.value)?;
            }
        }
        self.after_change();
        Ok(())
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        b.restore_registry()?;
        self.after_change();
        Ok(())
    }
    fn revert_default(&self) -> Result<bool> {
        let sets = self.applicable()?;
        if sets.is_empty() || sets.iter().any(|s| s.default.is_none()) {
            return Ok(false);
        }
        for s in sets {
            match s.default.as_ref().and_then(Option::as_ref) {
                Some(v) => reg::set(s.hive, &s.path, &s.name, v)?,
                None => reg::delete_value(s.hive, &s.path, &s.name)?,
            }
        }
        self.after_change();
        Ok(true)
    }
}

/// Builds one registry-style `DataTweak` from a catalog entry (also `TimerResolutionTweak`, which is plain values + a flag).
pub fn data_tweak(e: Entry, hooks: Arc<dyn Hooks>) -> Result<DataTweak> {
    let mut sets = Vec::new();
    for s in e.sets.unwrap_or_default() {
        sets.push(DataSet {
            hive: Hive::parse(&s.hive).ok_or_else(|| Error::Catalog(format!("{}: hive {}", e.id, s.hive)))?,
            path: s.path,
            name: s.name,
            value: to_reg_value(&s.kind, &s.value).map_err(|er| Error::Catalog(format!("{}: {er}", e.id)))?,
            default: Some(if s.default.is_null() { None } else { Some(to_reg_value(&s.kind, &s.default).map_err(|er| Error::Catalog(format!("{}: default {er}", e.id)))?) }),
            only_if_key_exists: s.only_if_key_exists,
        });
    }
    let after = if !e.after_change {
        After::None
    } else if e.id == "cust.dragoutline" {
        After::DragFullWindows
    } else {
        After::BroadcastSettingChange
    };
    Ok(DataTweak {
        flags: TweakFlags::parse(&e.flags)?,
        id: e.id,
        title: e.title,
        description: e.description.unwrap_or_default(),
        details: e.details.unwrap_or_default(),
        removed: e.removed,
        sets,
        after,
        hooks,
    })
}

/// Builds every plain registry tweak (active + Removed stubs) from the dump.
pub fn registry_tweaks(hooks: Arc<dyn Hooks>) -> Result<Vec<DataTweak>> {
    entries()?.into_iter().filter(|e| e.cls == "RegistryTweak" && !e.provider).map(|e| data_tweak(e, hooks.clone())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicU32, Ordering};

    #[test]
    fn revert_without_backup_writes_windows_defaults() {
        let path = format!("Software\\MyTuneTest\\cat-default-{}", std::process::id());
        let set = |name: &str, default| DataSet { hive: Hive::Hkcu, path: path.clone(), name: name.into(), value: RegValue::Dword(0), only_if_key_exists: false, default };
        let tweak = |sets| DataTweak { id: "t".into(), title: "t".into(), description: String::new(), details: String::new(), flags: TweakFlags(0), removed: false, sets, after: After::None, hooks: Arc::new(CountHooks::default()) };
        reg::set(Hive::Hkcu, &path, "A", &RegValue::Dword(0)).unwrap();
        reg::set(Hive::Hkcu, &path, "B", &RegValue::Dword(0)).unwrap();
        // one unknown default: nothing is touched
        let unknown = tweak(vec![set("A", Some(Some(RegValue::Dword(1)))), set("B", None)]);
        assert!(!unknown.revert_default().unwrap());
        assert_eq!(reg::get(Hive::Hkcu, &path, "A").unwrap(), Some(RegValue::Dword(0)));
        // known defaults: A gets its value back, B (absent by default) is deleted
        let known = tweak(vec![set("A", Some(Some(RegValue::Dword(1)))), set("B", Some(None))]);
        assert!(known.revert_default().unwrap());
        assert_eq!(reg::get(Hive::Hkcu, &path, "A").unwrap(), Some(RegValue::Dword(1)));
        assert_eq!(reg::get(Hive::Hkcu, &path, "B").unwrap(), None);
        assert!(!known.is_applied().unwrap());
        let _ = reg::delete_tree(Hive::Hkcu, &path);
        let _ = reg::delete_key_if_empty(Hive::Hkcu, "Software\\MyTuneTest");
    }

    #[derive(Default)]
    pub struct CountHooks {
        pub broadcast: AtomicU32,
        pub drag: AtomicU32,
        pub explorer: AtomicU32,
    }
    impl Hooks for CountHooks {
        fn broadcast_setting_change(&self) {
            self.broadcast.fetch_add(1, Ordering::SeqCst);
        }
        fn apply_drag_full_windows(&self) {
            self.drag.fetch_add(1, Ordering::SeqCst);
        }
        fn restart_explorer(&self) {
            self.explorer.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn catalog_matches_expected_shape() {
        let all = entries().unwrap();
        assert_eq!((all.len(), all.iter().filter(|e| e.removed).count()), (89, 16));
        let tweaks = registry_tweaks(Arc::new(CountHooks::default())).unwrap();
        assert_eq!(tweaks.len(), 66, "57 active + 9 removed stubs (adv.msi and TimerResolution excluded)");
        assert_eq!(tweaks.iter().filter(|t| t.removed).count(), 9);
        let ids: HashSet<_> = tweaks.iter().map(|t| t.id.clone()).collect();
        assert_eq!(ids.len(), tweaks.len(), "unique ids");
        assert_eq!(tweaks.iter().map(|t| t.sets.len()).sum::<usize>(), 122, "registry value count drifted from the C# dump");
    }

    #[test]
    fn spot_checks_against_known_csharp_values() {
        let t = registry_tweaks(Arc::new(CountHooks::default())).unwrap();
        let by = |id: &str| t.iter().find(|x| x.id == id).unwrap_or_else(|| panic!("{id}"));
        assert!(by("cust.numlock").sets.iter().any(|s| s.hive == Hive::Hku));
        assert!(by("general.display").sets.iter().any(|s| matches!(s.value, RegValue::Binary(_))));
        assert_eq!(by("cust.taskbarleft").sets[0].value, RegValue::Dword(0));
        assert!(by("general.telemetry").sets.iter().any(|s| s.only_if_key_exists));
        assert!(by("cust.dragoutline").after == After::DragFullWindows);
        assert!(by("adv.notifications").flags.has(TweakFlags::RESTARTS_EXPLORER));
        assert!(!by("general.windowsupdate").flags.recommended());
        assert_eq!(TweakFlags::parse("Restart, SignOut").unwrap().0, 3);
        assert!(TweakFlags::parse("Bogus").is_err());
    }

    #[test]
    fn reading_every_tweak_state_is_safe() {
        // read-only against the real registry: must never error or panic
        for t in registry_tweaks(Arc::new(CountHooks::default())).unwrap() {
            t.is_applied().unwrap_or_else(|e| panic!("{}: {e}", t.id));
        }
    }
}
