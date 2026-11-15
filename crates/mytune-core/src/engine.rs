//! Apply / revert with backup. Mirrors C# `Engine.ApplyNow` / `RevertNow` (one at a time, under one lock).
//!
//! Rules (do not weaken):
//! - A tweak captures originals into its `TweakBackup` BEFORE writing.
//! - The backup is saved even when `apply` fails half way, if anything was captured, so the
//!   originals are never lost.
//! - Revert with no backup changes nothing (no guessed defaults).
//! - If revert fails (any value not restored) the error is returned and the backup is KEPT for a retry.
//!   It is removed only after a fully successful revert.
//!
//! Known difference from C# `RegistryTweak.Revert`: with a backup that holds no registry entries the
//! C# code writes the tweak's declared default values. Here that case restores nothing.

use crate::backup::TweakBackup;
use crate::error::Result;
use crate::log;
use crate::reg::{self, Hive, RegValue};
use crate::store::Store;
use std::sync::Mutex;

pub trait Tweak {
    fn id(&self) -> &str;
    fn title(&self) -> &str;
    fn is_applied(&self) -> Result<bool>;
    /// Capture originals into `backup` before each write.
    fn apply(&self, backup: &mut TweakBackup) -> Result<()>;
    fn revert(&self, backup: &TweakBackup) -> Result<()>;
    /// Revert without a backup: write the Windows defaults. False when the defaults are not known (nothing touched).
    fn revert_default(&self) -> Result<bool> {
        Ok(false)
    }
    /// Runs under the engine lock after a successful apply and after its backup was saved
    /// (e.g. to set a flag in `state.json`).
    fn on_applied(&self, _store: &mut Store) -> Result<()> {
        Ok(())
    }
    /// Runs under the engine lock after a fully successful revert and after the backup was removed.
    fn on_reverted(&self, _store: &mut Store) -> Result<()> {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevertOutcome {
    Reverted,
    /// No backup stored: nothing was touched.
    NoBackup,
}

pub struct Engine {
    store: Mutex<Store>,
}

impl Engine {
    pub fn new(store: Store) -> Engine {
        Engine { store: Mutex::new(store) }
    }

    pub fn with_store<R>(&self, f: impl FnOnce(&mut Store) -> R) -> R {
        f(&mut self.store.lock().unwrap_or_else(|p| p.into_inner()))
    }

    pub fn apply(&self, t: &dyn Tweak) -> Result<()> {
        self.with_store(|store| {
            let mut backup = store.get_backup(t.id()).cloned().unwrap_or_else(|| TweakBackup::new(t.title()));
            let result = t.apply(&mut backup);
            if let Err(e) = &result {
                log::error(&format!("apply {} failed", t.id()), Some(e));
            }
            let keep = !backup.is_empty() || t.is_applied().unwrap_or(false);
            if keep {
                if let Err(e) = store.set_backup(t.id(), backup) {
                    log::error(&format!("saving backup for {} failed", t.id()), Some(&e));
                    // apply's own error wins; otherwise the lost backup is the error
                    return result.and(Err(e));
                }
            }
            if result.is_ok() {
                crate::stability::record(store.dir(), "on", t.id(), t.title());
            }
            result.and_then(|()| t.on_applied(store))
        })
    }

    pub fn revert(&self, t: &dyn Tweak) -> Result<RevertOutcome> {
        self.with_store(|store| {
            let Some(backup) = store.get_backup(t.id()).cloned() else {
                // set outside MyTune: turning it off writes the Windows defaults when the catalog knows them
                if !t.revert_default()? {
                    log::info(&format!("revert {}: no backup, nothing changed", t.id()));
                    return Ok(RevertOutcome::NoBackup);
                }
                log::info(&format!("revert {}: no backup, Windows defaults written", t.id()));
                crate::stability::record(store.dir(), "off", t.id(), t.title());
                t.on_reverted(store)?;
                return Ok(RevertOutcome::Reverted);
            };
            if let Err(e) = t.revert(&backup) {
                log::error(&format!("revert {} failed, backup kept", t.id()), Some(&e));
                return Err(e);
            }
            store.remove_backup(t.id())?;
            crate::stability::record(store.dir(), "off", t.id(), t.title());
            t.on_reverted(store)?;
            Ok(RevertOutcome::Reverted)
        })
    }
}

#[derive(Debug, Clone)]
pub struct RegSet {
    pub hive: Hive,
    pub path: String,
    pub name: String,
    pub value: RegValue,
}

/// A tweak made only of registry values.
pub struct RegistryTweak {
    pub id: String,
    pub title: String,
    pub sets: Vec<RegSet>,
}

impl Tweak for RegistryTweak {
    fn id(&self) -> &str {
        &self.id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn is_applied(&self) -> Result<bool> {
        for s in &self.sets {
            if reg::get(s.hive, &s.path, &s.name)?.as_ref() != Some(&s.value) {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn apply(&self, backup: &mut TweakBackup) -> Result<()> {
        for s in &self.sets {
            backup.capture(s.hive, &s.path, &s.name)?;
            if reg::get(s.hive, &s.path, &s.name)?.as_ref() != Some(&s.value) {
                reg::set(s.hive, &s.path, &s.name, &s.value)?;
            }
        }
        Ok(())
    }
    fn revert(&self, backup: &TweakBackup) -> Result<()> {
        backup.restore_registry()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Error;

    struct Scratch(String);
    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let p = format!("Software\\MyTuneTest\\eng-{tag}-{}", std::process::id());
            let _ = reg::delete_tree(Hive::Hkcu, &p);
            Scratch(p)
        }
        fn tweak(&self, v: u32) -> RegistryTweak {
            RegistryTweak {
                id: "t.one".into(),
                title: "Test".into(),
                sets: vec![RegSet { hive: Hive::Hkcu, path: format!("{}\\sub", self.0), name: "V".into(), value: RegValue::Dword(v) }],
            }
        }
        fn get(&self) -> Option<RegValue> {
            reg::get(Hive::Hkcu, &format!("{}\\sub", self.0), "V").unwrap()
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = reg::delete_tree(Hive::Hkcu, &self.0);
            let _ = reg::delete_key_if_empty(Hive::Hkcu, "Software\\MyTuneTest");
        }
    }

    fn engine(dir: &std::path::Path) -> Engine {
        Engine::new(Store::load(dir))
    }

    #[test]
    fn apply_then_revert_restores_original_value() {
        let s = Scratch::new("restore");
        let d = tempfile::tempdir().unwrap();
        reg::set(Hive::Hkcu, &format!("{}\\sub", s.0), "V", &RegValue::Dword(7)).unwrap();
        let e = engine(d.path());
        let t = s.tweak(1);
        e.apply(&t).unwrap();
        assert_eq!(s.get(), Some(RegValue::Dword(1)));
        assert!(t.is_applied().unwrap());
        assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted);
        assert_eq!(s.get(), Some(RegValue::Dword(7)));
        assert!(e.with_store(|st| st.get_backup("t.one").is_none()));
    }

    #[test]
    fn missing_original_is_deleted_with_created_keys() {
        let s = Scratch::new("missing");
        let d = tempfile::tempdir().unwrap();
        let e = engine(d.path());
        let t = s.tweak(1);
        e.apply(&t).unwrap();
        assert!(reg::key_exists(Hive::Hkcu, &s.0).unwrap());
        e.revert(&t).unwrap();
        assert_eq!(s.get(), None);
        assert!(!reg::key_exists(Hive::Hkcu, &s.0).unwrap(), "keys we created are removed");
    }

    #[test]
    fn revert_without_backup_changes_nothing() {
        let s = Scratch::new("nobackup");
        let d = tempfile::tempdir().unwrap();
        reg::set(Hive::Hkcu, &format!("{}\\sub", s.0), "V", &RegValue::Dword(9)).unwrap();
        let e = engine(d.path());
        assert_eq!(e.revert(&s.tweak(1)).unwrap(), RevertOutcome::NoBackup);
        assert_eq!(s.get(), Some(RegValue::Dword(9)));
        assert!(!d.path().join("state.json").exists(), "nothing saved either");
    }

    #[test]
    fn reapply_keeps_true_original() {
        let s = Scratch::new("reapply");
        let d = tempfile::tempdir().unwrap();
        reg::set(Hive::Hkcu, &format!("{}\\sub", s.0), "V", &RegValue::Dword(7)).unwrap();
        let e = engine(d.path());
        e.apply(&s.tweak(1)).unwrap();
        e.apply(&s.tweak(2)).unwrap();
        e.revert(&s.tweak(2)).unwrap();
        assert_eq!(s.get(), Some(RegValue::Dword(7)));
    }

    #[test]
    fn failed_revert_keeps_backup_and_retry_works() {
        let s = Scratch::new("fail");
        let d = tempfile::tempdir().unwrap();
        reg::set(Hive::Hkcu, &format!("{}\\sub", s.0), "V", &RegValue::Dword(7)).unwrap();
        let e = engine(d.path());
        let t = s.tweak(1);
        e.apply(&t).unwrap();
        // poison: add a broken entry captured "first", so it is restored last
        e.with_store(|st| {
            let mut b = st.get_backup("t.one").unwrap().clone();
            let mut bad = b.registry[0].clone();
            bad.hive = "HKXX".into();
            b.registry.insert(0, bad);
            st.set_backup("t.one", b).unwrap();
        });
        let err = e.revert(&t).unwrap_err();
        assert!(matches!(&err, Error::Revert(f) if f.len() == 1), "{err}");
        assert!(e.with_store(|st| st.get_backup("t.one").is_some()), "backup kept");
        assert_eq!(s.get(), Some(RegValue::Dword(7)), "the good value was still restored");
        // fix the entry and retry
        e.with_store(|st| {
            let mut b = st.get_backup("t.one").unwrap().clone();
            b.registry.remove(0);
            st.set_backup("t.one", b).unwrap();
        });
        assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted);
    }

    struct HalfTweak<'a>(&'a Scratch);
    impl Tweak for HalfTweak<'_> {
        fn id(&self) -> &str {
            "t.half"
        }
        fn title(&self) -> &str {
            "Half"
        }
        fn is_applied(&self) -> Result<bool> {
            Ok(false)
        }
        fn apply(&self, b: &mut TweakBackup) -> Result<()> {
            let p = format!("{}\\sub", self.0 .0);
            b.capture(Hive::Hkcu, &p, "V")?;
            reg::set(Hive::Hkcu, &p, "V", &RegValue::Dword(5))?;
            Err(Error::BadBackup("simulated failure after first write".into()))
        }
        fn revert(&self, b: &TweakBackup) -> Result<()> {
            b.restore_registry()
        }
    }

    #[test]
    fn half_applied_tweak_still_gets_a_backup() {
        let s = Scratch::new("half");
        let d = tempfile::tempdir().unwrap();
        reg::set(Hive::Hkcu, &format!("{}\\sub", s.0), "V", &RegValue::Dword(7)).unwrap();
        let e = engine(d.path());
        let t = HalfTweak(&s);
        assert!(e.apply(&t).is_err());
        assert_eq!(s.get(), Some(RegValue::Dword(5)));
        assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted);
        assert_eq!(s.get(), Some(RegValue::Dword(7)));
    }

    #[test]
    fn backup_survives_restart() {
        let s = Scratch::new("restart");
        let d = tempfile::tempdir().unwrap();
        reg::set(Hive::Hkcu, &format!("{}\\sub", s.0), "V", &RegValue::Dword(7)).unwrap();
        let t = s.tweak(1);
        engine(d.path()).apply(&t).unwrap();
        let e2 = engine(d.path()); // new process would load state.json
        e2.revert(&t).unwrap();
        assert_eq!(s.get(), Some(RegValue::Dword(7)));
    }
}
