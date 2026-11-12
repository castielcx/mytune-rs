//! Settings > Backup, export and import (C# `SettingsBackup`): which optimizations are on, plus display, VPN
//! and app settings, as one JSON file (PascalCase keys, same shape as the C# file).
//!
//! Like C#, import copies only five settings (MinimizeToTray, GameTimer, ThrottleBackground, QuietDuringGames,
//! HeatWarning); the others are exported but not restored. Applying the tweaks is up to the caller (it holds
//! the tweak objects and the engine); `apply_settings` does the rest.

use crate::catalog;
use crate::display::{self, DisplayProfile};
use crate::error::{Error, Result};
use crate::log;
use crate::store::Store;
use crate::vpn;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    #[serde(rename = "Version", default = "one")]
    pub version: i32,
    #[serde(rename = "Tweaks", default)]
    pub tweaks: Vec<String>,
    #[serde(rename = "Displays", default)]
    pub displays: BTreeMap<String, DisplayProfile>,
    #[serde(rename = "Vpns", default)]
    pub vpns: BTreeMap<String, bool>,
    #[serde(rename = "Settings", default)]
    pub settings: Value,
}

fn one() -> i32 {
    1
}

/// C# `AppSettings` with its defaults.
pub fn default_settings() -> Map<String, Value> {
    let stick = json!({ "Inner": 5, "Outer": 100, "Shape": 0 });
    let Value::Object(m) = json!({
        "MinimizeToTray": true, "GameTimer": false, "GpuOc": false, "GpuOcCore": 0, "GpuOcMemory": 0, "GpuOcPower": 0,
        "ThrottleBackground": false, "QuietDuringGames": false, "HeatWarning": true, "AutoPowerPlan": false,
        "CheckForUpdates": true, "AccentColor": "#FF5A0A", "ControllerLeft": stick, "ControllerRight": stick,
        "ControllerEmulate": false, "ControllerLowLatency": false,
    }) else {
        unreachable!()
    };
    m
}

const RESTORED: [&str; 5] = ["MinimizeToTray", "GameTimer", "ThrottleBackground", "QuietDuringGames", "HeatWarning"];

/// Writes the snapshot of the current state; returns how many tweaks it holds. Per-game ids (`game:`) and
/// anything that is not an active catalog tweak are left out.
pub fn export(store: &Store, path: &Path) -> Result<usize> {
    let known: Vec<String> = catalog::entries()?.into_iter().filter(|e| !e.removed).map(|e| e.id).collect();
    let mut tweaks: Vec<String> = store.applied_ids().into_iter().filter(|id| known.contains(id)).collect();
    tweaks.sort();
    let mut settings = default_settings();
    if let Some(Value::Object(raw)) = store.top("Settings") {
        settings.extend(raw.clone());
    }
    let snap = Snapshot {
        version: 1,
        displays: store.top("Displays").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default(),
        vpns: vpn::wanted(store).into_iter().collect(),
        settings: Value::Object(settings),
        tweaks,
    };
    std::fs::write(path, serde_json::to_string_pretty(&snap)?)?;
    log::info(&format!("settings exported to {}", path.display()));
    Ok(snap.tweaks.len())
}

pub fn read(path: &Path) -> std::result::Result<Snapshot, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    serde_json::from_slice(&bytes).map_err(|_| "This isn't a MyTune settings file.".to_string())
}

/// Copies the five restored settings, registers (and applies) the display profiles and remembers the VPN
/// states, then saves. Display and VPN failures are logged, not fatal.
pub fn apply_settings(store: &mut Store, snap: &Snapshot) -> Result<()> {
    let defaults = default_settings();
    for key in RESTORED {
        let v = snap.settings.get(key).or_else(|| defaults.get(key)).cloned().unwrap_or(Value::Null);
        store.set_settings_value(key, v);
    }
    let attached: Vec<String> = display::monitors().into_iter().map(|m| m.device.to_lowercase()).collect();
    for (dev, profile) in &snap.displays {
        display::register(store, dev, Some(profile));
        if !attached.contains(&dev.to_lowercase()) {
            continue;
        }
        let mut p = profile.clone();
        let r = display::apply_ramp(dev, &p).and_then(|()| display::apply_vibrance(dev, &mut p));
        if let Err(e) = r {
            log::error(&format!("import display {dev}"), Some(&e));
        }
    }
    for (id, on) in &snap.vpns {
        vpn::remember(store, id, *on)?; // the VPN watcher switches them
    }
    store.save()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backup::TweakBackup;

    #[test]
    fn export_has_csharp_shape_and_leaves_out_unknown_ids() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("state.json"), r#"{"Settings":{"HeatWarning":false,"Extra":1},"Vpns":{"ras:a":true}}"#).unwrap();
        let mut st = Store::load(d.path());
        st.set_backup("general.gamebar", TweakBackup::new("Game Bar")).unwrap();
        st.set_backup("game:123:priority", TweakBackup::new("per game")).unwrap();
        st.set_backup("nope.unknown", TweakBackup::new("x")).unwrap();
        let file = d.path().join("out.json");
        assert_eq!(export(&st, &file).unwrap(), 1);
        let v: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
        assert_eq!(v["Version"], 1);
        assert_eq!(v["Tweaks"], json!(["general.gamebar"]));
        assert_eq!(v["Vpns"]["ras:a"], true);
        assert_eq!(v["Settings"]["HeatWarning"], false, "saved value wins over the default");
        assert_eq!(v["Settings"]["MinimizeToTray"], true, "missing keys get C# defaults");
        assert_eq!(v["Settings"]["Extra"], 1);
        assert_eq!(v["Settings"]["ControllerLeft"]["Outer"], 100);
    }

    #[test]
    fn import_copies_five_settings_displays_and_vpns() {
        let d = tempfile::tempdir().unwrap();
        let mut st = Store::load(d.path());
        st.set_settings_value("AccentColor", json!("#22D3EE"));
        let snap: Snapshot = serde_json::from_str(
            r##"{"Version":1,"Tweaks":["a"],"Displays":{"\\\\.\\DISPLAY9":{"Gamma":1.5}},"Vpns":{"ras:x":false},
                "Settings":{"HeatWarning":false,"ThrottleBackground":true,"AccentColor":"#FF0000"}}"##,
        )
        .unwrap();
        apply_settings(&mut st, &snap).unwrap();
        assert_eq!(st.settings_flag("HeatWarning"), Some(false));
        assert_eq!(st.settings_flag("ThrottleBackground"), Some(true));
        assert_eq!(st.settings_flag("MinimizeToTray"), Some(true), "absent in the file = default");
        assert_eq!(st.settings_str("AccentColor").as_deref(), Some("#22D3EE"), "accent is not restored, like C#");
        assert_eq!(display::get_profile(&st, r"\\.\DISPLAY9").gamma, 1.5);
        assert_eq!(vpn::wanted(&st), vec![("ras:x".to_string(), false)]);
        let again = Store::load(d.path());
        assert_eq!(again.settings_flag("HeatWarning"), Some(false), "saved to disk");
    }

    #[test]
    fn not_a_settings_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.json");
        std::fs::write(&p, "hello").unwrap();
        assert_eq!(read(&p).unwrap_err(), "This isn't a MyTune settings file.");
        std::fs::write(&p, "{}").unwrap();
        assert!(read(&p).unwrap().tweaks.is_empty());
    }
}
