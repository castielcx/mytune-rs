//! Background throttle (port of C# `BackgroundThrottle`): while a game runs, browsers, launchers and updaters get
//! below-normal priority and efficiency mode; both are undone afterwards. Nothing is suspended or closed.
//!
//! Steam, Discord and the Riot client are never in the list (and there is a test that keeps it that way).
//! Originals are kept in memory only, like C#: after a crash those programs keep the lowered priority until
//! they restart. Differences from C#: `restore` returns the failures (and keeps those entries) instead of only
//! logging them, and a pid that exited is dropped so it cannot block a later process that reuses the pid.

use crate::error::{Error, Result};
use crate::log;
use crate::procs::{Held, Procs};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const BROWSERS: [&str; 6] = ["chrome", "msedge", "firefox", "brave", "opera", "vivaldi"];
pub const LAUNCHERS: [&str; 8] = ["EpicGamesLauncher", "EpicWebHelper", "Battle.net", "EADesktop", "UbisoftConnect", "upc", "UplayWebCore", "GalaxyClient"];
pub const UPDATERS: [&str; 4] = ["GoogleUpdate", "MicrosoftEdgeUpdate", "OneDrive", "Dropbox"];
/// Rescan interval while active: a browser or launcher helper can start mid-game.
pub const RESCAN_SECS: u64 = 10;

pub fn default_names() -> Vec<String> {
    BROWSERS.iter().chain(LAUNCHERS.iter()).chain(UPDATERS.iter()).map(|s| s.to_string()).collect()
}

pub struct BackgroundThrottle {
    procs: Arc<dyn Procs>,
    names: Vec<String>,
    held: Mutex<HashMap<u32, Held>>,
    active: Mutex<bool>,
}

impl BackgroundThrottle {
    pub fn new(procs: Arc<dyn Procs>) -> BackgroundThrottle {
        BackgroundThrottle::with_names(procs, default_names())
    }

    /// `names` are exact process names without `.exe`, compared case-insensitively.
    pub fn with_names(procs: Arc<dyn Procs>, names: Vec<String>) -> BackgroundThrottle {
        BackgroundThrottle { procs, names, held: Mutex::new(HashMap::new()), active: Mutex::new(false) }
    }

    pub fn is_active(&self) -> bool {
        *self.active.lock().unwrap()
    }

    pub fn held_count(&self) -> usize {
        self.held.lock().unwrap().len()
    }

    /// Turns throttling on (scans now) or off (restores everything).
    pub fn set(&self, on: bool) -> Result<()> {
        if on {
            let mut a = self.active.lock().unwrap();
            if *a {
                return Ok(());
            }
            *a = true;
            drop(a);
            self.scan()
        } else {
            let was_active = std::mem::replace(&mut *self.active.lock().unwrap(), false);
            if was_active || self.held_count() > 0 {
                self.restore()
            } else {
                Ok(())
            }
        }
    }

    /// Looks for new matching processes (call about every `RESCAN_SECS` while active).
    pub fn scan(&self) -> Result<()> {
        if !self.is_active() {
            return Ok(());
        }
        let list = self.procs.list()?;
        let mut held = self.held.lock().unwrap();
        held.retain(|pid, _| list.iter().any(|p| p.pid == *pid)); // exited: forget, the pid may be reused
        for p in list {
            if held.contains_key(&p.pid) || !self.names.iter().any(|n| n.eq_ignore_ascii_case(&p.name)) {
                continue;
            }
            match self.procs.throttle(p.pid) {
                Ok(Some(h)) => {
                    held.insert(p.pid, h);
                }
                Ok(None) => {}
                Err(e) => log::error(&format!("throttle {} ({})", p.name, p.pid), Some(&e)),
            }
        }
        Ok(())
    }

    fn restore(&self) -> Result<()> {
        let mut held = self.held.lock().unwrap();
        let mut failed = Vec::new();
        let total = held.len();
        let pids: Vec<u32> = held.keys().copied().collect();
        for pid in pids {
            let h = held[&pid];
            match self.procs.restore(pid, &h) {
                Ok(()) => {
                    held.remove(&pid);
                }
                Err(e) => failed.push(format!("{pid}: {e}")),
            }
        }
        log::info(&format!("booster: background programs restored ({}/{total})", total - failed.len()));
        if failed.is_empty() {
            Ok(())
        } else {
            Err(Error::Revert(failed))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procs::fake::FakeProcs;
    use crate::procs::ProcInfo;

    #[test]
    fn steam_discord_riot_are_never_in_the_lists() {
        for n in default_names() {
            let l = n.to_lowercase();
            assert!(!l.contains("steam") && !l.contains("discord") && !l.contains("riot") && !l.contains("vanguard"), "{n}");
        }
        assert_eq!(default_names().len(), 18);
    }

    #[test]
    fn throttles_matching_names_only_and_restores() {
        let p = FakeProcs::new(&[(1, "Chrome"), (2, "steam"), (3, "OneDrive"), (4, "game"), (5, "discord")]);
        let t = BackgroundThrottle::new(p.clone());
        t.set(true).unwrap();
        assert_eq!(*p.calls.lock().unwrap(), vec!["throttle 1".to_string(), "throttle 3".to_string()]);
        assert_eq!(t.held_count(), 2);
        t.set(true).unwrap(); // already on: no second pass
        assert_eq!(p.calls.lock().unwrap().len(), 2);
        t.set(false).unwrap();
        assert_eq!(t.held_count(), 0);
        assert!(!t.is_active());
        assert!(p.calls.lock().unwrap().contains(&"restore 1".to_string()) && p.calls.lock().unwrap().contains(&"restore 3".to_string()));
    }

    #[test]
    fn rescan_picks_up_new_processes_and_forgets_exited_ones() {
        let p = FakeProcs::new(&[(1, "chrome")]);
        let t = BackgroundThrottle::new(p.clone());
        t.set(true).unwrap();
        p.list.lock().unwrap().push(ProcInfo { pid: 9, name: "msedge".into() });
        t.scan().unwrap();
        assert_eq!(t.held_count(), 2);
        p.list.lock().unwrap().retain(|x| x.pid != 1); // chrome exits, pid 1 may be reused later
        p.list.lock().unwrap().push(ProcInfo { pid: 1, name: "firefox".into() });
        t.scan().unwrap();
        assert!(p.calls.lock().unwrap().iter().filter(|c| *c == "throttle 1").count() >= 1);
    }

    #[test]
    fn failed_restore_keeps_those_entries_for_a_retry() {
        let p = FakeProcs::new(&[(1, "chrome"), (2, "msedge")]);
        let t = BackgroundThrottle::new(p.clone());
        t.set(true).unwrap();
        p.fail_restore.lock().unwrap().push(2);
        let err = t.set(false).unwrap_err();
        assert!(matches!(err, Error::Revert(ref f) if f.len() == 1), "{err}");
        assert_eq!(t.held_count(), 1, "pid 2 stays recorded");
        p.fail_restore.lock().unwrap().clear();
        t.set(false).unwrap();
        assert_eq!(t.held_count(), 0);
    }

    #[test]
    fn off_without_ever_on_does_nothing() {
        let p = FakeProcs::new(&[(1, "chrome")]);
        BackgroundThrottle::new(p.clone()).set(false).unwrap();
        assert!(p.calls.lock().unwrap().is_empty());
    }
}
