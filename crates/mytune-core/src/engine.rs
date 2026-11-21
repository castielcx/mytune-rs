//! Apply / revert with backup. Mirrors C# `Engine.ApplyNow` / `RevertNow` (one at a time, under the `op` lock).
//!
//! Rules (do not weaken):
//! - A tweak captures originals into its `TweakBackup` BEFORE writing.
//! - A first apply that fails is undone. Its backup is dropped once the PC is back as it was, and kept (marked
//!   `partly_applied`) when the undo fails too, so the originals are never lost.
//! - Revert with no backup changes nothing (no guessed defaults).
//! - If revert fails (any value not restored) the error is returned and the backup is KEPT for a retry.
//!   It is removed only after a fully successful revert.
//!
//! Known difference from C# `RegistryTweak.Revert`: with a backup that holds no registry entries the
//! C# code writes the tweak's declared default values. Here that case restores nothing.

use crate::backup::TweakBackup;
use crate::error::{Error, Result};
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
pub(crate) const PENDING: &str = "Pending";

fn mark(store: &mut Store, t: &dyn Tweak, op: &str) {
    let mut p = store.top(PENDING).and_then(Value::as_object).cloned().unwrap_or_default();
    p.insert(t.id().into(), json!({ "Title": t.title(), "Op": op, "At": crate::util::iso_now(), "Pid": std::process::id() }));
    store.set_top(PENDING, Value::Object(p));
}

/// In memory only: the caller saves. Leaves `{}` rather than removing the key: a removed key would make the save
/// delete another MyTune's entries too. `save` drops an empty journal from the file.
fn unmark(store: &mut Store, id: &str) {
    let mut p = store.top(PENDING).and_then(Value::as_object).cloned().unwrap_or_default();
    p.remove(id);
    store.set_top(PENDING, Value::Object(p));
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
    /// MyTune runs under another account than the signed-in one (a field so tests can fake it).
    other_account: fn() -> bool,
}

/// True when `capture` still reads the same originals: the failed apply changed nothing.
fn unchanged(t: &dyn Tweak, b: &TweakBackup) -> bool {
    let mut now = TweakBackup::default();
    t.capture(&mut now).is_ok() && !now.is_empty() && now.registry == b.registry && now.data == b.data
}

impl Engine {
    pub fn new(store: Store) -> Engine {
        Engine { store: Mutex::new(store), op: Mutex::new(()), other_account: crate::process::shell_user_differs }
    }

    pub fn with_store<R>(&self, f: impl FnOnce(&mut Store) -> R) -> R {
        f(&mut self.store.lock().unwrap_or_else(|p| p.into_inner()))
    }

    /// Saves the journal entry, and `backup` when it holds anything, before the change. Nothing may be changed
    /// when this fails.
    fn begin(&self, t: &dyn Tweak, op: &str, backup: &TweakBackup) -> Result<()> {
        self.with_store(|s| {
            let prev = s.get_backup(t.id()).cloned();
            mark(s, t, op);
            let saved = if backup.is_empty() { s.save() } else { s.set_backup(t.id(), backup.clone()) };
            if let Err(e) = &saved {
                log::error(&format!("{op} {}: journal not saved, nothing changed", t.id()), Some(e));
                unmark(s, t.id());
                s.reset_backup(t.id(), prev);
            }
            saved
        })
    }

    pub fn apply(&self, t: &dyn Tweak) -> Result<()> {
        let _op = self.op.lock().unwrap_or_else(|p| p.into_inner());
        self.with_store(Store::refresh)?;
        let stored = self.with_store(|s| s.get_backup(t.id()).cloned());
        let mut had_backup = stored.is_some();
        let mut backup = stored.unwrap_or_else(|| TweakBackup::new(t.title()));
        t.capture(&mut backup)?;
        let other_account = (self.other_account)();
        // checked here too: when HKCU already holds the values no write fails, and Clean Start goes on to the pin file
        if other_account && backup.registry.iter().any(|r| r.hive.eq_ignore_ascii_case("HKCU")) {
            log::info(&format!("apply {} refused: MyTune runs under another account", t.id()));
            return Err(Error::Plain(reg::OTHER_ACCOUNT.into()));
        }
        self.begin(t, "apply", &backup)?;
        // another MyTune saved this tweak's backup in between: its capture holds the true originals
        if let Some(first) = self.with_store(|s| s.get_backup(t.id()).cloned()).filter(|b| !had_backup && *b != backup) {
            backup = first;
            had_backup = true;
        }
        let result = {
            let _deny = other_account.then(reg::deny_hkcu_writes);
            t.apply(&mut backup)
        };
        let mut undone = false;
        let result = result.map_err(|e| {
            log::error(&format!("apply {} failed", t.id()), Some(&e));
            // a re-apply keeps its backup: undoing it would also turn off the earlier, working apply
            if had_backup || backup.is_empty() {
                return e;
            }
            if unchanged(t, &backup) {
                undone = true;
                return e;
            }
            match t.revert(&backup) {
                Ok(()) => {
                    log::info(&format!("apply {}: undone after the failure", t.id()));
                    undone = true;
                    e
                }
                Err(re) => {
                    log::error(&format!("apply {}: undo failed, backup kept", t.id()), Some(&re));
                    backup.partly_applied = true;
                    Error::Plain(format!("{e}. Undoing it failed too, so it is partly applied: use Undo in Settings > Restore"))
                }
            }
        });
        if result.is_ok() {
            backup.partly_applied = false;
        }
        let keep = !undone && (!backup.is_empty() || t.is_applied().unwrap_or(false));
        self.with_store(|store| {
            unmark(store, t.id());
            // one save: the backup goes in and the journal entry goes out together
            let saved = if keep {
                store.set_backup(t.id(), backup)
            } else if !had_backup && store.get_backup(t.id()).is_some() {
                // only `begin`'s copy of this run's capture, and nothing was changed. An older backup is never dropped here.
                store.remove_backup(t.id())
            } else {
                store.save()
            };
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
        // without its backups in memory this would write Windows defaults over the originals
        self.with_store(Store::refresh)?;
        self.revert_locked(t, true)
    }

    /// Undoes an apply that did not hold. Only a saved backup is restored: without one the apply may have changed
    /// nothing, and writing the Windows defaults would be a change of its own.
    pub fn rollback(&self, t: &dyn Tweak) -> Result<RevertOutcome> {
        let _op = self.op.lock().unwrap_or_else(|p| p.into_inner());
        self.with_store(Store::refresh)?;
        self.revert_locked(t, false)
    }

    /// Rolls back every apply or revert that a MyTune which died (crash, kill, power loss) left in the journal:
    /// with a backup the normal revert runs, without one nothing is touched. Each entry is then removed, except
    /// one that belongs to another Windows account: that account finishes it at its next start. Call once at
    /// start, off the UI thread, with a lookup into the built tweak list.
    pub fn recover<'a>(&self, find: impl Fn(&str) -> Option<&'a dyn Tweak>) -> Vec<Recovered> {
        // held throughout: under it, an entry of this process cannot be an operation still running
        let _op = self.op.lock().unwrap_or_else(|p| p.into_inner());
        if self.with_store(Store::refresh).is_err() {
            return Vec::new(); // the journal stays on disk for the next start
        }
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
                let reverted = find(&id).map(|t| self.revert_locked(t, false));
                if matches!(&reverted, Some(Err(Error::Plain(m))) if m == reg::OTHER_ACCOUNT) {
                    continue; // entry and backup stay for the account they belong to
                }
                Some(match reverted {
                    Some(Ok(RevertOutcome::Reverted)) => true,
                    Some(Ok(RevertOutcome::NoBackup)) => {
                        log::info(&format!("recover {id}: no backup, left as it is"));
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

    /// `use_default`: only an explicit Off from the user may write Windows defaults when there is no backup.
    fn revert_locked(&self, t: &dyn Tweak, use_default: bool) -> Result<RevertOutcome> {
        let other_account = (self.other_account)();
        let stored = self.with_store(|s| s.get_backup(t.id()).cloned());
        // HKCU and the Start pins (`start2`, Clean Start) are the signed-in user's: they would land in this admin's
        // profile, be reported as restored, and the backup would be gone
        if other_account && stored.as_ref().is_some_and(|b| b.registry.iter().any(|r| r.hive.eq_ignore_ascii_case("HKCU")) || b.data.contains_key("start2")) {
            log::info(&format!("revert {} refused: MyTune runs under another account, backup kept", t.id()));
            return Err(Error::Plain(reg::OTHER_ACCOUNT.into()));
        }
        let Some(backup) = stored else {
            // set outside MyTune: turning it off writes the Windows defaults when the catalog knows them
            let written = use_default && {
                let _deny = other_account.then(reg::deny_hkcu_writes);
                t.revert_default()?
            };
            if !written {
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
        let reverted = {
            let _deny = other_account.then(reg::deny_hkcu_writes);
            t.revert(&backup)
        };
        if let Err(e) = reverted {
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
    use crate::backup::RegBackup;

    /// No system access: the "PC" is one number. `apply` changes it (when `writes`) and then fails.
    struct FakeFail {
        pc: std::sync::Arc<Mutex<u32>>,
        writes: bool,
        revert_fails: bool,
        reverts: std::sync::Arc<Mutex<u32>>,
        hkcu: bool,
    }
    impl FakeFail {
        fn new(writes: bool, revert_fails: bool) -> FakeFail {
            FakeFail { pc: Default::default(), writes, revert_fails, reverts: Default::default(), hkcu: false }
        }
    }
    impl Tweak for FakeFail {
        fn id(&self) -> &str {
            "t.fake"
        }
        fn title(&self) -> &str {
            "Fake"
        }
        fn is_applied(&self) -> Result<bool> {
            Ok(*self.pc.lock().unwrap() == 1)
        }
        fn capture(&self, b: &mut TweakBackup) -> Result<()> {
            b.data.entry("v".into()).or_insert(self.pc.lock().unwrap().to_string());
            if self.hkcu && b.registry.is_empty() {
                b.registry.push(RegBackup { hive: "HKCU".into(), path: "p".into(), name: "n".into(), existed: false, kind: None, value: None, multi: None, created_keys: vec![] });
            }
            Ok(())
        }
        fn apply(&self, b: &mut TweakBackup) -> Result<()> {
            self.capture(b)?;
            if self.writes {
                *self.pc.lock().unwrap() = 1;
            }
            Err(Error::Plain("simulated".into()))
        }
        fn revert(&self, b: &TweakBackup) -> Result<()> {
            *self.reverts.lock().unwrap() += 1;
            if self.revert_fails {
                return Err(Error::Plain("revert failed".into()));
            }
            *self.pc.lock().unwrap() = b.data["v"].parse().unwrap();
            Ok(())
        }
    }

    #[test]
    fn failed_first_apply_that_changed_nothing_drops_its_capture() {
        let d = tempfile::tempdir().unwrap();
        let e = engine(d.path());
        let t = FakeFail::new(false, false);
        assert!(e.apply(&t).is_err());
        assert_eq!(*t.reverts.lock().unwrap(), 0, "nothing to undo");
        assert!(Store::load(d.path()).get_backup("t.fake").is_none());
        assert!(pending(d.path()).is_null());
    }

    #[test]
    fn failed_apply_is_rolled_back() {
        let d = tempfile::tempdir().unwrap();
        let e = engine(d.path());
        let t = FakeFail::new(true, false);
        assert!(e.apply(&t).is_err());
        assert_eq!(*t.pc.lock().unwrap(), 0, "put back");
        assert!(Store::load(d.path()).get_backup("t.fake").is_none());
    }

    #[test]
    fn failed_rollback_keeps_the_backup_as_partly_applied() {
        let d = tempfile::tempdir().unwrap();
        let e = engine(d.path());
        let t = FakeFail::new(true, true);
        let err = e.apply(&t).unwrap_err().to_string();
        assert!(err.contains("partly applied"), "{err}");
        let s = Store::load(d.path());
        assert!(s.get_backup("t.fake").unwrap().partly_applied);
        assert_eq!(s.partly_applied_ids(), vec!["t.fake".to_string()]);
        assert!(pending(d.path()).is_null());
        // the undo works later: the mark goes with the backup
        let ok = FakeFail { revert_fails: false, ..t };
        assert_eq!(e.revert(&ok).unwrap(), RevertOutcome::Reverted);
        assert_eq!(*ok.pc.lock().unwrap(), 0);
        assert!(Store::load(d.path()).partly_applied_ids().is_empty());
    }

    #[test]
    fn failed_reapply_keeps_the_earlier_backup_without_undoing() {
        let d = tempfile::tempdir().unwrap();
        let e = engine(d.path());
        let mut old = TweakBackup::new("Fake");
        old.data.insert("v".into(), "7".into());
        e.with_store(|s| s.set_backup("t.fake", old)).unwrap();
        let t = FakeFail::new(true, false);
        assert!(e.apply(&t).is_err());
        assert_eq!(*t.reverts.lock().unwrap(), 0);
        assert_eq!(Store::load(d.path()).get_backup("t.fake").unwrap().data["v"], "7");
    }

    #[test]
    fn other_account_is_refused_for_hkcu_tweaks_and_may_still_revert() {
        let d = tempfile::tempdir().unwrap();
        let e = Engine { other_account: || true, ..engine(d.path()) };
        let t = FakeFail { hkcu: true, ..FakeFail::new(true, false) };
        assert!(e.apply(&t).unwrap_err().to_string().contains("different Windows account"));
        assert_eq!(*t.pc.lock().unwrap(), 0, "apply never ran");
        assert!(!d.path().join("state.json").exists(), "nothing saved");
        let mut b = TweakBackup::new("Fake");
        b.data.insert("v".into(), "0".into());
        e.with_store(|s| s.set_backup("t.fake", b)).unwrap();
        assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted);
    }

    #[test]
    fn other_account_revert_keeps_a_backup_with_hkcu_entries() {
        let d = tempfile::tempdir().unwrap();
        let e = Engine { other_account: || true, ..engine(d.path()) };
        let t = FakeFail::new(false, false);
        let mut b = TweakBackup::new("Fake");
        b.data.insert("v".into(), "0".into());
        b.registry.push(RegBackup { hive: "HKCU".into(), path: "p".into(), name: "n".into(), existed: false, kind: None, value: None, multi: None, created_keys: vec![] });
        e.with_store(|s| s.set_backup("t.fake", b)).unwrap();
        assert!(e.revert(&t).unwrap_err().to_string().contains("different Windows account"));
        assert_eq!(*t.reverts.lock().unwrap(), 0);
        assert!(Store::load(d.path()).get_backup("t.fake").is_some(), "backup kept");
        assert!(pending(d.path()).is_null());
        // cut off under the right account: its entry waits for that account
        e.with_store(|s| {
            mark(s, &t, "apply");
            s.save()
        })
        .unwrap();
        assert!(e.recover(|_| Some(&t as &dyn Tweak)).is_empty());
        assert_eq!(pending(d.path())["t.fake"]["Op"], "apply");
    }

    /// No system access: deleting a value under a key that does not exist changes nothing, and fails while denied.
    struct DefaultOnly(Mutex<u32>);
    impl Tweak for DefaultOnly {
        fn id(&self) -> &str {
            "t.default"
        }
        fn title(&self) -> &str {
            "Default"
        }
        fn is_applied(&self) -> Result<bool> {
            Ok(false)
        }
        fn apply(&self, _: &mut TweakBackup) -> Result<()> {
            Ok(())
        }
        fn revert(&self, _: &TweakBackup) -> Result<()> {
            Ok(())
        }
        fn revert_default(&self) -> Result<bool> {
            reg::delete_value(Hive::Hkcu, "Software\\MyTuneTest\\never", "V")?;
            *self.0.lock().unwrap() += 1;
            Ok(true)
        }
    }

    #[test]
    fn other_account_cannot_write_hkcu_defaults() {
        let d = tempfile::tempdir().unwrap();
        let e = Engine { other_account: || true, ..engine(d.path()) };
        let t = DefaultOnly(Mutex::new(0));
        assert!(e.revert(&t).unwrap_err().to_string().contains("different Windows account"));
        assert_eq!(*t.0.lock().unwrap(), 0);
    }

    #[test]
    fn only_an_explicit_off_writes_defaults() {
        let d = tempfile::tempdir().unwrap();
        let e = Engine { other_account: || false, ..engine(d.path()) };
        let t = DefaultOnly(Mutex::new(0));
        assert_eq!(e.rollback(&t).unwrap(), RevertOutcome::NoBackup);
        e.with_store(|s| {
            mark(s, &t, "apply");
            s.save()
        })
        .unwrap();
        let r = e.recover(|_| Some(&t as &dyn Tweak));
        assert_eq!(r, vec![Recovered { id: "t.default".into(), title: "Default".into(), put_back: false }]);
        assert!(pending(d.path()).is_null());
        assert_eq!(*t.0.lock().unwrap(), 0, "rollback and recovery wrote nothing");
        assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted);
        assert_eq!(*t.0.lock().unwrap(), 1);
    }

    /// No system access: the "PC" is one number, and 99 marks a write of the defaults.
    struct FakeOk(std::sync::Arc<Mutex<u32>>);
    impl Tweak for FakeOk {
        fn id(&self) -> &str {
            "t.ok"
        }
        fn title(&self) -> &str {
            "Ok"
        }
        fn is_applied(&self) -> Result<bool> {
            Ok(*self.0.lock().unwrap() == 1)
        }
        fn capture(&self, b: &mut TweakBackup) -> Result<()> {
            b.data.entry("v".into()).or_insert(self.0.lock().unwrap().to_string());
            Ok(())
        }
        fn apply(&self, b: &mut TweakBackup) -> Result<()> {
            self.capture(b)?;
            *self.0.lock().unwrap() = 1;
            Ok(())
        }
        fn revert(&self, b: &TweakBackup) -> Result<()> {
            *self.0.lock().unwrap() = b.data["v"].parse().unwrap();
            Ok(())
        }
        fn revert_default(&self) -> Result<bool> {
            *self.0.lock().unwrap() = 99;
            Ok(true)
        }
    }

    #[test]
    fn a_second_process_uses_the_first_ones_originals() {
        let d = tempfile::tempdir().unwrap();
        let (a, b) = (Engine { other_account: || false, ..engine(d.path()) }, Engine { other_account: || false, ..engine(d.path()) });
        let t = FakeOk(Default::default());
        *t.0.lock().unwrap() = 7;
        a.apply(&t).unwrap();
        b.apply(&t).unwrap(); // the PC reads 1 now: that is no original
        assert_eq!(Store::load(d.path()).get_backup("t.ok").unwrap().data["v"], "7");
        assert_eq!(b.revert(&t).unwrap(), RevertOutcome::Reverted);
        assert_eq!(*t.0.lock().unwrap(), 7, "restored, not the defaults");
        assert!(Store::load(d.path()).get_backup("t.ok").is_none());
        // the other way round: the process that never saw the backup reverts it
        a.apply(&t).unwrap();
        assert_eq!(b.revert(&t).unwrap(), RevertOutcome::Reverted);
        assert_eq!(*t.0.lock().unwrap(), 7);
    }

    #[test]
    fn hkcu_writes_fail_while_denied_on_this_thread_only() {
        let deny = reg::deny_hkcu_writes();
        let msg = reg::set(Hive::Hkcu, "Software\\MyTuneTest\\never", "V", &RegValue::Dword(1)).unwrap_err().to_string();
        assert!(msg.contains("different Windows account"));
        assert!(reg::delete_value(Hive::Hkcu, "Software\\MyTuneTest\\never", "V").is_err());
        drop(deny);
        assert!(reg::get(Hive::Hkcu, "Software\\MyTuneTest\\never", "V").unwrap().is_none());
    }

    #[test]
    fn apply_and_revert_refuse_while_state_cannot_be_read() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir(d.path().join("state.json")).unwrap();
        let e = engine(d.path());
        let t = FakeFail::new(true, false);
        assert!(e.apply(&t).is_err());
        assert_eq!(*t.pc.lock().unwrap(), 0, "apply never ran");
        assert!(e.revert(&t).is_err());
        assert_eq!(*t.reverts.lock().unwrap(), 0);
    }

    #[test]
    fn old_backup_without_the_new_field_loads() {
        let b: TweakBackup = serde_json::from_str(r#"{"Title":"t","AppliedAt":"x","Registry":[],"Data":{"k":"v"}}"#).unwrap();
        assert!(!b.partly_applied);
        assert!(!serde_json::to_string(&b).unwrap().contains("PartlyApplied"), "written only when set");
    }

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
    #[ignore = "real system: writes HKCU scratch key"]
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
    #[ignore = "real system: writes HKCU scratch key"]
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
    #[ignore = "real system: writes HKCU scratch key"]
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
    #[ignore = "real system: writes HKCU scratch key"]
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
    #[ignore = "real system: writes HKCU scratch key"]
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
    #[ignore = "real system: writes HKCU scratch key"]
    fn half_applied_tweak_is_rolled_back() {
        let s = Scratch::new("half");
        let d = tempfile::tempdir().unwrap();
        reg::set(Hive::Hkcu, &format!("{}\\sub", s.0), "V", &RegValue::Dword(7)).unwrap();
        let e = engine(d.path());
        let t = HalfTweak(&s);
        assert!(e.apply(&t).is_err());
        assert_eq!(s.get(), Some(RegValue::Dword(7)));
        assert!(e.with_store(|st| st.get_backup("t.half").is_none()));
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
    #[ignore = "real system: writes HKCU scratch key"]
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
    #[ignore = "real system: writes HKCU scratch key"]
    fn interrupted_apply_without_backup_is_left_as_it_is() {
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
        // default known: still not written, the apply may have changed nothing
        let t = Dying { default: Some(3), ..t };
        killed(|| drop(engine(d.path()).apply(&t)));
        let (r, _) = restart(d.path(), &t);
        assert!(!r[0].put_back);
        assert_eq!(s.get(), Some(RegValue::Dword(1)));
        assert!(pending(d.path()).is_null());
        // a tweak that is no longer in the list: entry cleared, reported
        killed(|| drop(engine(d.path()).apply(&t)));
        let r = engine(d.path()).recover(|_| None);
        assert!(!r[0].put_back);
        assert!(pending(d.path()).is_null());
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
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
    #[ignore = "real system: writes HKCU scratch key"]
    fn failed_operations_leave_no_entry() {
        let s = Scratch::new("crash-fail");
        let d = tempfile::tempdir().unwrap();
        let e = engine(d.path());
        assert!(e.apply(&HalfTweak(&s)).is_err());
        assert!(pending(d.path()).is_null());
        e.with_store(|st| {
            let mut b = TweakBackup::new("Half");
            b.capture(Hive::Hkcu, &format!("{}\\sub", s.0), "V").unwrap();
            b.registry[0].hive = "HKXX".into();
            st.set_backup("t.half", b).unwrap();
        });
        assert!(e.revert(&HalfTweak(&s)).is_err());
        assert!(pending(d.path()).is_null());
        assert!(e.with_store(|st| st.get_backup("t.half").is_some()), "backup kept");
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
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

    struct CapturesOnly;
    impl Tweak for CapturesOnly {
        fn id(&self) -> &str {
            "t.cap"
        }
        fn title(&self) -> &str {
            "Cap"
        }
        fn is_applied(&self) -> Result<bool> {
            Ok(false)
        }
        fn capture(&self, b: &mut TweakBackup) -> Result<()> {
            b.data.insert("k".into(), "v".into());
            Ok(())
        }
        fn apply(&self, _: &mut TweakBackup) -> Result<()> {
            panic!("must not run when the journal was not saved");
        }
        fn revert(&self, _: &TweakBackup) -> Result<()> {
            Ok(())
        }
    }

    #[test]
    fn failed_pre_change_save_leaves_no_backup_in_memory() {
        let d = tempfile::tempdir().unwrap();
        let e = engine(d.path());
        std::fs::create_dir(d.path().join("state.json.tmp")).unwrap(); // File::create on it fails
        assert!(e.apply(&CapturesOnly).is_err());
        assert!(e.with_store(|s| s.get_backup("t.cap").is_none()));
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
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
