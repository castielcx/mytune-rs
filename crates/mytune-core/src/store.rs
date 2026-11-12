//! `state.json` in `%ProgramData%\MyTune`, shared with the C# app.
//!
//! Only `Applied` (tweak id -> `TweakBackup`) is typed. Every other top-level key (Profiles,
//! Settings, Games, Benchmarks, ...) is kept as raw JSON and written back unchanged, so a Rust
//! save never drops C# data. Keys are PascalCase like the C# file.
//!
//! Save: write `state.json.tmp` (fsync), then `ReplaceFileW` onto `state.json` with `state.json.bak`
//! as the previous generation (atomic, same as C# `File.Replace`). 3 attempts, 100 ms apart.
//! Load: `state.json`; if missing -> `.bak`; if unreadable -> moved to `.corrupt-<yyyyMMddHHmmss>`, then `.bak`.

use crate::backup::TweakBackup;
use crate::error::Result;
use crate::util::{default_dir, local_compact};
use crate::log;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::thread::sleep;
use std::time::Duration;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{ReplaceFileW, REPLACE_FILE_FLAGS};

fn null_as_default<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<BTreeMap<String, TweakBackup>, D::Error> {
    Ok(Option::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct State {
    #[serde(rename = "Applied", default, deserialize_with = "null_as_default")]
    applied: BTreeMap<String, TweakBackup>,
    #[serde(flatten)]
    other: Map<String, Value>,
}

enum Read {
    Missing,
    Bad,
    Ok(State),
}

pub struct Store {
    dir: PathBuf,
    state: State,
}

fn read_state(p: &Path) -> Read {
    let mut bytes = None;
    for attempt in 0..10 {
        match fs::read(p) {
            Ok(b) => {
                bytes = Some(b);
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Read::Missing,
            // 32/33 = file in use by another process (the C# app may be saving)
            Err(e) if matches!(e.raw_os_error(), Some(32) | Some(33)) && attempt < 9 => sleep(Duration::from_millis(300)),
            Err(_) => return Read::Bad,
        }
    }
    let Some(b) = bytes else { return Read::Bad };
    let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&b);
    match serde_json::from_slice::<State>(b) {
        Ok(s) => Read::Ok(s),
        Err(_) => Read::Bad,
    }
}

fn sibling(p: &Path, suffix: &str) -> PathBuf {
    let mut s = p.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

impl Store {
    pub fn load_default() -> Store {
        Store::load(&default_dir())
    }

    pub fn load(dir: &Path) -> Store {
        let path = dir.join("state.json");
        let bak = sibling(&path, ".bak");
        let state = match read_state(&path) {
            Read::Ok(s) => s,
            Read::Missing => match read_state(&bak) {
                Read::Ok(s) => {
                    log::info("state.json missing, loaded state.json.bak");
                    s
                }
                _ => State::default(),
            },
            Read::Bad => {
                // Move (not copy): the next Save would otherwise rotate the bad file over the good .bak.
                let corrupt = sibling(&path, &format!(".corrupt-{}", local_compact()));
                if fs::rename(&path, &corrupt).is_err() {
                    let _ = fs::copy(&path, &corrupt);
                }
                log::error(&format!("state.json unreadable, kept as {}", corrupt.display()), None);
                match read_state(&bak) {
                    Read::Ok(s) => {
                        log::info("loaded state.json.bak");
                        s
                    }
                    _ => State::default(),
                }
            }
        };
        Store { dir: dir.to_path_buf(), state }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn get_backup(&self, id: &str) -> Option<&TweakBackup> {
        self.state.applied.get(id)
    }

    pub fn applied_ids(&self) -> Vec<String> {
        self.state.applied.keys().cloned().collect()
    }

    pub fn set_backup(&mut self, id: &str, b: TweakBackup) -> Result<()> {
        self.state.applied.insert(id.to_string(), b);
        self.save()
    }

    pub fn remove_backup(&mut self, id: &str) -> Result<()> {
        if self.state.applied.remove(id).is_some() {
            self.save()?;
        }
        Ok(())
    }

    /// Raw JSON of a key this crate does not own (for tests and tools).
    pub fn other(&self, key: &str) -> Option<&Value> {
        self.state.other.get(key)
    }

    /// Any top-level key the C# app owns (`Games`, `QuietActive`, `PowerPrevious`, ...), as raw JSON.
    pub fn top(&self, key: &str) -> Option<&Value> {
        self.state.other.get(key)
    }

    /// Replaces a top-level key (in memory; call `save`). `Applied` is not reachable this way.
    pub fn set_top(&mut self, key: &str, value: Value) {
        if key != "Applied" {
            self.state.other.insert(key.to_string(), value);
        }
    }

    /// A boolean in the C# `Settings` object (e.g. `GameTimer`); `None` if missing or not a bool.
    pub fn settings_flag(&self, name: &str) -> Option<bool> {
        self.state.other.get("Settings")?.get(name)?.as_bool()
    }

    /// Sets a boolean in `Settings` (creating the object if needed) and saves. All other keys stay as they are.
    pub fn set_settings_flag(&mut self, name: &str, value: bool) -> Result<()> {
        let settings = self.state.other.entry("Settings").or_insert_with(|| Value::Object(Map::new()));
        match settings {
            Value::Object(m) => {
                m.insert(name.to_string(), Value::Bool(value));
            }
            other => *other = serde_json::json!({ name: value }),
        }
        self.save()
    }

    /// A string in `Settings` (e.g. `AccentColor`); `None` if missing or not a string.
    pub fn settings_str(&self, name: &str) -> Option<String> {
        self.state.other.get("Settings")?.get(name)?.as_str().map(String::from)
    }

    /// Sets a string in `Settings` and saves (e.g. `AccentColor`).
    pub fn set_settings_str(&mut self, name: &str, value: &str) -> Result<()> {
        self.set_settings_value(name, Value::String(value.to_string()));
        self.save()
    }

    /// Sets any value in `Settings` (creating the object if needed) without saving. Other keys stay as they are.
    pub fn set_settings_value(&mut self, name: &str, value: Value) {
        let settings = self.state.other.entry("Settings").or_insert_with(|| Value::Object(Map::new()));
        match settings {
            Value::Object(m) => {
                m.insert(name.to_string(), value);
            }
            other => *other = serde_json::json!({ name: value }),
        }
    }

    pub fn save(&self) -> Result<()> {
        let json = serde_json::to_vec_pretty(&self.state)?;
        let mut last = None;
        for attempt in 0..3 {
            match self.write_once(&json) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    log::error(&format!("state.json save attempt {} failed", attempt + 1), Some(&e));
                    last = Some(e);
                    if attempt < 2 {
                        sleep(Duration::from_millis(100));
                    }
                }
            }
        }
        Err(last.unwrap().into())
    }

    fn write_once(&self, json: &[u8]) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let path = self.dir.join("state.json");
        let tmp = sibling(&path, ".tmp");
        {
            let mut f = File::create(&tmp)?;
            f.write_all(json)?;
            f.sync_all()?;
        }
        if path.exists() {
            let w = |p: &Path| -> Vec<u16> { p.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect() };
            let (a, b, c) = (w(&path), w(&tmp), w(&sibling(&path, ".bak")));
            unsafe { ReplaceFileW(PCWSTR(a.as_ptr()), PCWSTR(b.as_ptr()), PCWSTR(c.as_ptr()), REPLACE_FILE_FLAGS(0), None, None) }
                .map_err(|e| std::io::Error::from_raw_os_error((e.code().0 & 0xFFFF) as i32))?;
        } else {
            fs::rename(&tmp, &path)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CS_JSON: &str = r#"{
  "Profiles": [],
  "Settings": {"Theme": "dark", "Big": 18446744073709551615, "F": 1.5},
  "Applied": {
    "adv.timer": {
      "Title": "t", "AppliedAt": "2026-11-09T23:28:34.9960836+01:00",
      "Registry": [{"Hive":"HKLM","Path":"p","Name":"n","Existed":true,"Kind":"DWord","Value":"1","Multi":null,"CreatedKeys":[]}],
      "Data": {"enabled": "1"}
    }
  },
  "Games": [{"Name": "x"}]
}"#;

    fn bk(title: &str) -> TweakBackup {
        TweakBackup::new(title)
    }

    #[test]
    fn reads_csharp_format_bom_and_keeps_unknown_keys() {
        let d = tempfile::tempdir().unwrap();
        let mut raw = vec![0xEF, 0xBB, 0xBF];
        raw.extend_from_slice(CS_JSON.as_bytes());
        fs::write(d.path().join("state.json"), raw).unwrap();
        let mut s = Store::load(d.path());
        assert_eq!(s.get_backup("adv.timer").unwrap().registry[0].value.as_deref(), Some("1"));
        s.set_backup("other", bk("o")).unwrap();
        let v: Value = serde_json::from_slice(&fs::read(d.path().join("state.json")).unwrap()).unwrap();
        assert_eq!(v["Settings"]["Big"], serde_json::json!(18446744073709551615u64));
        assert_eq!(v["Settings"]["F"], serde_json::json!(1.5));
        assert_eq!(v["Games"][0]["Name"], "x");
        assert_eq!(v["Applied"]["adv.timer"]["Registry"][0]["Kind"], "DWord");
        assert!(v["Applied"]["other"].is_object());
    }

    #[test]
    fn save_keeps_previous_generation_as_bak() {
        let d = tempfile::tempdir().unwrap();
        let mut s = Store::load(d.path());
        s.set_backup("a", bk("first")).unwrap();
        assert!(!d.path().join("state.json.bak").exists());
        s.set_backup("b", bk("second")).unwrap();
        let bak: Value = serde_json::from_slice(&fs::read(d.path().join("state.json.bak")).unwrap()).unwrap();
        assert!(bak["Applied"]["a"].is_object() && bak["Applied"]["b"].is_null());
        assert!(!d.path().join("state.json.tmp").exists());
        s.remove_backup("a").unwrap();
        assert_eq!(Store::load(d.path()).applied_ids(), vec!["b".to_string()]);
    }

    #[test]
    fn settings_flag_roundtrip_keeps_other_keys() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("state.json"), r#"{"Settings":{"Theme":"dark","GameTimer":false},"Games":[1]}"#).unwrap();
        let mut s = Store::load(d.path());
        assert_eq!(s.settings_flag("GameTimer"), Some(false));
        assert_eq!(s.settings_flag("Nope"), None);
        s.set_settings_flag("GameTimer", true).unwrap();
        let s2 = Store::load(d.path());
        assert_eq!(s2.settings_flag("GameTimer"), Some(true));
        assert_eq!(s2.other("Settings").unwrap()["Theme"], "dark");
        assert_eq!(s2.other("Games").unwrap()[0], 1);
        let empty = tempfile::tempdir().unwrap();
        let mut s3 = Store::load(empty.path());
        s3.set_settings_flag("GameTimer", true).unwrap();
        assert_eq!(Store::load(empty.path()).settings_flag("GameTimer"), Some(true));
    }

    #[test]
    fn null_applied_is_empty() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("state.json"), r#"{"Applied": null, "X": 1}"#).unwrap();
        let s = Store::load(d.path());
        assert!(s.applied_ids().is_empty());
        assert_eq!(s.other("X"), Some(&serde_json::json!(1)));
    }

    #[test]
    fn falls_back_to_bak_when_state_missing() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("state.json.bak"), CS_JSON).unwrap();
        assert!(Store::load(d.path()).get_backup("adv.timer").is_some());
    }

    #[test]
    fn corrupt_state_is_kept_and_bak_used() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("state.json"), "{ not json").unwrap();
        fs::write(d.path().join("state.json.bak"), CS_JSON).unwrap();
        let mut s = Store::load(d.path());
        assert!(s.get_backup("adv.timer").is_some());
        let corrupt: Vec<_> = fs::read_dir(d.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with("state.json.corrupt-"))
            .collect();
        assert_eq!(corrupt.len(), 1);
        assert_eq!(fs::read_to_string(corrupt[0].path()).unwrap(), "{ not json");
        // next save must not rotate the corrupt file over the good .bak
        s.set_backup("n", bk("n")).unwrap();
        assert!(fs::read_to_string(d.path().join("state.json")).unwrap().contains("adv.timer"));
    }

    #[test]
    fn empty_file_is_corrupt_and_no_bak_gives_empty_state() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("state.json"), "").unwrap();
        let s = Store::load(d.path());
        assert!(s.applied_ids().is_empty());
        assert!(!d.path().join("state.json").exists());
        assert!(fs::read_dir(d.path()).unwrap().any(|e| e.unwrap().file_name().to_string_lossy().contains(".corrupt-")));
    }
}
