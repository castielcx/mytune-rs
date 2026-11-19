//! Apply / revert with backup. Mirrors C# `Engine.ApplyNow` / `RevertNow` (one at a time, under the `op` lock).
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
use serde_json::{json, Value};
use std::sync::Mutex;

pub trait Tweak {
    fn id(&self) -> &str;
    fn title(&self) -> &str;
    fn is_applied(&self) -> Result<bool>;
    /// Records the originals `apply` would change, changing nothing, so the engine can save them before the
    /// change: a crash during `apply` then still has its backup. Tweaks that cannot know them up front
    /// (commands, driver calls) leave this empty and are covered by the journal entry alone.
    fn capture(&self, _backup: &mut TweakBackup) -> Result<()> {
        Ok(())
    }
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

/// One interrupted change that `Engine::recover` dealt with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recovered {
    pub id: String,
    pub title: String,
    /// False: it could not be put back (no backup and no known default, the tweak is gone, or the revert failed).
    pub put_back: bool,
}

/// Top-level key of state.json: tweak id -> `{Title, Op, At, Pid}` of the apply or revert in flight. Saved before
/// the change and removed after it, so an entry found at start is a change that was cut off.
const PENDING: &str = "Pending";

fn mark(store: &mut Store, t: &dyn Tweak, op: &str) {
    let mut p = store.top(PENDING).and_then(Value::as_object).cloned().unwrap_or_default();
    p.insert(t.id().into(), json!({ "Title": t.title(), "Op": op, "At": crate::util::iso_now(), "Pid": std::process::id() }));
    store.set_top(PENDING, Value::Object(p));
}

/// In memory only: the caller saves.
fn unmark(store: &mut Store, id: &str) {
    let mut p = store.top(PENDING).and_then(Value::as_object).cloned().unwrap_or_default();
    p.remove(id);
    if p.is_empty() {
        store.remove_top(PENDING);
    } else {
        store.set_top(PENDING, Value::Object(p));
    }
}

fn alive(pid: u32) -> bool {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut code = 0;
        let live = GetExitCodeProcess(h, &mut code).is_ok() && code == 259; // STILL_ACTIVE
        let _ = CloseHandle(h);
        live
    }
}

pub struct Engine {
    store: Mutex<Store>,
    /// One apply or revert at a time. The store lock is NOT held while a tweak's commands run (they take
    /// up to 2 min), or every reader of a setting, the UI each frame among them, would wait that long.
    op: Mutex<()>,
}

impl Engine {
    pub fn new(store: Store) -> Engine {
        Engine { store: Mutex::new(store), op: Mutex::new(()) }
    }

    pub fn with_store<R>(&self, f: impl FnOnce(&mut Store) -> R) -> R {
        f(&mut self.store.lock().unwrap_or_else(|p| p.into_inner()))
    }

    /// Saves the journal entry, and `backup` when it holds anything, before the change. Nothing may be changed
    /// when this fails.
    fn begin(&self, t: &dyn Tweak, op: &str, backup: &TweakBackup) -> Result<()> {
        self.with_store(|s| {
            mark(s, t, op);
            let saved = if backup.is_empty() { s.save() } else { s.set_backup(t.id(), backup.clone()) };
            if let Err(e) = &saved {
                log::error(&format!("{op} {}: journal not saved, nothing changed", t.id()), Some(e));
                unmark(s, t.id());
            }
            saved
        })
    }

    pub fn apply(&self, t: &dyn Tweak) -> Result<()> {
        let _op = self.op.lock().unwrap_or_else(|p| p.into_inner());
        let mut backup = self.with_store(|s| s.get_backup(t.id()).cloned()).unwrap_or_else(|| TweakBackup::new(t.title()));
        t.capture(&mut backup)?;
        self.begin(t, "apply", &backup)?;
        let result = t.apply(&mut backup);
        if let Err(e) = &result {
            log::error(&format!("apply {} failed", t.id()), Some(e));
        }
        let keep = !backup.is_empty() || t.is_applied().unwrap_or(false);
        self.with_store(|store| {
            unmark(store, t.id());
            // one save: the backup goes in and the journal entry goes out together
            let saved = if keep { store.set_backup(t.id(), backup) } else { store.save() };
            if let Err(e) = saved {
                log::error(&format!("saving backup for {} failed", t.id()), Some(&e));
                // apply's own error wins; otherwise the lost backup is the error
                return result.and(Err(e));
            }
            if result.is_ok() {
                crate::stability::record(store.dir(), "on", t.id(), t.title());
            }
            result.and_then(|()| t.on_applied(store))
        })
    }

    pub fn revert(&self, t: &dyn Tweak) -> Result<RevertOutcome> {
        let _op = self.op.lock().unwrap_or_else(|p| p.into_inner());
        self.revert_locked(t)
    }

    /// Rolls back every apply or revert that a MyTune which died (crash, kill, power loss) left in the journal:
    /// with a backup the normal revert runs, without one the tweak's Windows defaults are written when it knows
    /// them, else nothing is touched. Each entry is then removed. Call once at start, off the UI thread, with a
    /// lookup into the built tweak list.
    pub fn recover<'a>(&self, find: impl Fn(&str) -> Option<&'a dyn Tweak>) -> Vec<Recovered> {
        // held throughout: under it, an entry of this process cannot be an operation still running
        let _op = self.op.lock().unwrap_or_else(|p| p.into_inner());
        let left = self.with_store(|s| s.top(PENDING).and_then(Value::as_object).cloned()).unwrap_or_default();
        let mut out = Vec::new();
        for (id, e) in left {
            let pid = e["Pid"].as_u64().unwrap_or(0) as u32;
            // a second MyTune (the comparison copy) is in the middle of this one
            if pid != std::process::id() && alive(pid) {
                continue;
            }
            let title = e["Title"].as_str().unwrap_or(&id).to_string();
            let reverting = e["Op"].as_str() == Some("revert");
            let has_backup = self.with_store(|s| s.get_backup(&id).is_some());
            let put_back = if reverting && !has_backup {
                // the revert had finished (its backup goes in the same save as its entry): only the entry is stale
                None
            } else {
                Some(match find(&id).map(|t| self.revert_locked(t)) {
                    Some(Ok(RevertOutcome::Reverted)) => true,
                    Some(Ok(RevertOutcome::NoBackup)) => {
                        log::info(&format!("recover {id}: no backup and no known default, left as it is"));
                        false
                    }
                    Some(Err(_)) => false, // logged by revert, backup kept
                    None => {
                        log::info(&format!("recover {id}: tweak not in the list, left as it is"));
                        false
                    }
                })
            };
            log::info(&format!("recover {id}: interrupted {} from {}: {}", if reverting { "revert" } else { "apply" }, e["At"].as_str().unwrap_or("?"), match put_back {
                Some(true) => "put back",
                Some(false) => "not put back",
                None => "had finished",
            }));
            self.with_store(|s| {
                unmark(s, &id);
                if let Err(e) = s.save() {
                    log::error(&format!("recover {id}: journal entry not removed"), Some(&e));
                }
            });
            if let Some(put_back) = put_back {
                out.push(Recovered { id, title, put_back });
            }
        }
        out
    }

    fn revert_locked(&self, t: &dyn Tweak) -> Result<RevertOutcome> {
        let Some(backup) = self.with_store(|s| s.get_backup(t.id()).cloned()) else {
            // set outside MyTune: turning it off writes the Windows defaults when the catalog knows them
            if !t.revert_default()? {
                log::info(&format!("revert {}: no backup, nothing changed", t.id()));
                return Ok(RevertOutcome::NoBackup);
            }
            log::info(&format!("revert {}: no backup, Windows defaults written", t.id()));
            return self.with_store(|store| {
                crate::stability::record(store.dir(), "off", t.id(), t.title());
                t.on_reverted(store)?;
                Ok(RevertOutcome::Reverted)
            });
        };
        self.begin(t, "revert", &TweakBackup::default())?;
        if let Err(e) = t.revert(&backup) {
            log::error(&format!("revert {} failed, backup kept", t.id()), Some(&e));
            self.with_store(|store| {
                unmark(store, t.id());
                let _ = store.save();
            });
            return Err(e);
        }
        self.with_store(|store| {
            unmark(store, t.id());
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
    fn capture(&self, backup: &mut TweakBackup) -> Result<()> {
        self.sets.iter().try_for_each(|s| backup.capture(s.hive, &s.path, &s.name))
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

    /// A tweak whose process "dies" (panics) in the middle of the change, after its write.
    struct Dying {
        inner: RegistryTweak,
        captures: bool,
        default: Option<u32>,
        in_revert: bool,
    }
    impl Tweak for Dying {
        fn id(&self) -> &str {
            self.inner.id()
        }
        fn title(&self) -> &str {
            self.inner.title()
        }
        fn is_applied(&self) -> Result<bool> {
            self.inner.is_applied()
        }
        fn capture(&self, b: &mut TweakBackup) -> Result<()> {
            if self.captures {
                self.inner.capture(b)?;
            }
            Ok(())
        }
        fn apply(&self, b: &mut TweakBackup) -> Result<()> {
            if self.in_revert {
                return self.inner.apply(b);
            }
            let s = &self.inner.sets[0];
            reg::set(s.hive, &s.path, &s.name, &s.value)?;
            panic!("killed during apply");
        }
        fn revert(&self, b: &TweakBackup) -> Result<()> {
            if self.in_revert {
                panic!("killed during revert");
            }
            b.restore_registry()
        }
        fn revert_default(&self) -> Result<bool> {
            let Some(v) = self.default else { return Ok(false) };
            let s = &self.inner.sets[0];
            reg::set(s.hive, &s.path, &s.name, &RegValue::Dword(v))?;
            Ok(true)
        }
    }

    fn killed(f: impl FnOnce()) {
        assert!(std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err());
    }

    fn pending(dir: &std::path::Path) -> Value {
        let v: Value = serde_json::from_slice(&std::fs::read(dir.join("state.json")).unwrap()).unwrap();
        v["Pending"].clone()
    }

    /// state.json as a killed process left it, then a new start: (what `recover` reports, the engine).
    fn restart(dir: &std::path::Path, t: &dyn Tweak) -> (Vec<Recovered>, Engine) {
        let e = engine(dir);
        let r = e.recover(|id| (id == t.id()).then_some(t));
        (r, e)
    }

    #[test]
    fn interrupted_apply_with_captured_backup_is_rolled_back() {
        let s = Scratch::new("crash-cap");
        let d = tempfile::tempdir().unwrap();
        reg::set(Hive::Hkcu, &format!("{}\\sub", s.0), "V", &RegValue::Dword(7)).unwrap();
        let t = Dying { inner: s.tweak(1), captures: true, default: None, in_revert: false };
        let e = engine(d.path());
        killed(|| drop(e.apply(&t)));
        assert_eq!(s.get(), Some(RegValue::Dword(1)), "changed");
        assert_eq!(pending(d.path())["t.one"]["Op"], "apply");
        assert_eq!(pending(d.path())["t.one"]["Title"], "Test");
        assert!(Store::load(d.path()).get_backup("t.one").is_some(), "the original was on disk before the change");
        let (r, e2) = restart(d.path(), &t);
        assert_eq!(r, vec![Recovered { id: "t.one".into(), title: "Test".into(), put_back: true }]);
        assert_eq!(s.get(), Some(RegValue::Dword(7)));
        assert!(pending(d.path()).is_null(), "entry cleared");
        assert!(e2.with_store(|st| st.get_backup("t.one").is_none()));
        assert!(e2.recover(|_| None).is_empty(), "told once");
    }

    #[test]
    fn interrupted_apply_without_backup_uses_the_default_or_leaves_it() {
        let s = Scratch::new("crash-nocap");
        let d = tempfile::tempdir().unwrap();
        reg::set(Hive::Hkcu, &format!("{}\\sub", s.0), "V", &RegValue::Dword(7)).unwrap();
        // no default known: nothing is touched, and it says so
        let t = Dying { inner: s.tweak(1), captures: false, default: None, in_revert: false };
        killed(|| drop(engine(d.path()).apply(&t)));
        assert!(Store::load(d.path()).get_backup("t.one").is_none());
        let (r, _) = restart(d.path(), &t);
        assert_eq!(r, vec![Recovered { id: "t.one".into(), title: "Test".into(), put_back: false }]);
        assert_eq!(s.get(), Some(RegValue::Dword(1)), "left as it is");
        assert!(pending(d.path()).is_null(), "entry cleared");
        // default known: it is written
        let t = Dying { default: Some(3), ..t };
        killed(|| drop(engine(d.path()).apply(&t)));
        let (r, _) = restart(d.path(), &t);
        assert!(r[0].put_back);
        assert_eq!(s.get(), Some(RegValue::Dword(3)));
        assert!(pending(d.path()).is_null());
        // a tweak that is no longer in the list: entry cleared, reported
        killed(|| drop(engine(d.path()).apply(&t)));
        let r = engine(d.path()).recover(|_| None);
        assert!(!r[0].put_back);
        assert!(pending(d.path()).is_null());
    }

    #[test]
    fn interrupted_revert_is_finished() {
        let s = Scratch::new("crash-rev");
        let d = tempfile::tempdir().unwrap();
        reg::set(Hive::Hkcu, &format!("{}\\sub", s.0), "V", &RegValue::Dword(7)).unwrap();
        let t = Dying { inner: s.tweak(1), captures: true, default: None, in_revert: true };
        let e = engine(d.path());
        e.apply(&t).unwrap();
        assert!(pending(d.path()).is_null(), "a finished apply leaves no entry");
        killed(|| drop(e.revert(&t)));
        assert_eq!(pending(d.path())["t.one"]["Op"], "revert");
        let plain = s.tweak(1);
        let (r, e2) = restart(d.path(), &plain);
        assert!(r[0].put_back);
        assert_eq!(s.get(), Some(RegValue::Dword(7)));
        assert!(pending(d.path()).is_null());
        assert!(e2.with_store(|st| st.get_backup("t.one").is_none()));
    }

    #[test]
    fn failed_operations_leave_no_entry() {
        let s = Scratch::new("crash-fail");
        let d = tempfile::tempdir().unwrap();
        let e = engine(d.path());
        assert!(e.apply(&HalfTweak(&s)).is_err());
        assert!(pending(d.path()).is_null());
        e.with_store(|st| {
            let mut b = st.get_backup("t.half").unwrap().clone();
            b.registry[0].hive = "HKXX".into();
            st.set_backup("t.half", b).unwrap();
        });
        assert!(e.revert(&HalfTweak(&s)).is_err());
        assert!(pending(d.path()).is_null());
        assert!(e.with_store(|st| st.get_backup("t.half").is_some()), "backup kept");
    }

    #[test]
    fn entry_of_a_running_mytune_is_left_alone() {
        let s = Scratch::new("crash-live");
        let d = tempfile::tempdir().unwrap();
        // stays alive until its input closes
        let mut other = std::process::Command::new("cmd").args(["/c", "pause"]).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).spawn().unwrap();
        std::fs::write(d.path().join("state.json"), json!({ "Pending": { "t.one": { "Title": "Test", "Op": "apply", "At": "x", "Pid": other.id() } } }).to_string()).unwrap();
        let t = s.tweak(1);
        let (r, _) = restart(d.path(), &t);
        assert!(r.is_empty());
        assert_eq!(pending(d.path())["t.one"]["Op"], "apply");
        other.kill().unwrap();
        other.wait().unwrap();
        let (r, _) = restart(d.path(), &t);
        assert_eq!(r.len(), 1, "its process is gone: now it is a leftover");
        assert!(pending(d.path()).is_null());
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
