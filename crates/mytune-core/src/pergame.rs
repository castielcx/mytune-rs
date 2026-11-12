//! The last tweaks: per-game GPU preference, Defender exclusion and QoS policy, and the timer-resolution tweak.
//!
//! Per-game ids are `game:<GameEntry.Id>:gpu|defender|qos|nvidia` (nvidia is in `nvidia.rs`); the caller builds
//! the tweak on demand, exactly like C# `GameService`. Programs go through `Cmd`, so tests use a fake.
//!
//! Differences from C# (never weaker):
//! - Defender: the excluded folder is stored in the backup (`folder`) and revert uses that one. C# recomputes it
//!   at revert, which can leave the exclusion behind if the install folder changed.
//! - QoS: unchanged from C# (value restored only when the last `MyTune *` policy is gone). Known limitation kept:
//!   with several games the value ends as `1`, because later games captured what the first had already set.
//! - Timer resolution: only the registry value and the `Settings.GameTimer` flag in `state.json` are handled.
//!   Making the running timer follow the flag (C# `GameBooster.Update`) belongs to the game-booster component.
//!   `is_applied` checks the registry only; use `TimerResolutionTweak::flag_set` for the flag.

use crate::backup::TweakBackup;
use crate::catalog::{After, DataSet, DataTweak, Hooks, TweakFlags};
use crate::engine::Tweak;
use crate::error::{Error, Result};
use crate::reg::{self, Hive, RegValue};
use crate::special::{powershell, ps_quote};
use crate::store::Store;
use crate::tweaks::Cmd;
use std::sync::Arc;

// ---------------------------------------------------------------- ids

/// `game:<id>:<kind>`.
pub fn game_tweak_id(game_id: &str, kind: &str) -> String {
    format!("game:{game_id}:{kind}")
}

// ---------------------------------------------------------------- high-performance GPU

const GPU_KEY: &str = "Software\\Microsoft\\DirectX\\UserGpuPreferences";

/// "Use the high-performance GPU" for one exe: HKCU `UserGpuPreferences`, value NAME = full exe path,
/// data `GpuPreference=2;`.
pub fn gpu_tweak(game_id: &str, game_name: &str, exe_path: &str, hooks: Arc<dyn Hooks>) -> Result<DataTweak> {
    if exe_path.trim().is_empty() {
        return Err(Error::Catalog("GPU preference needs the game's exe path".into()));
    }
    Ok(DataTweak {
        id: game_tweak_id(game_id, "gpu"),
        title: format!("{game_name}: high-performance GPU"),
        description: String::new(),
        details: String::new(),
        flags: TweakFlags(0),
        removed: false,
        sets: vec![DataSet {
            hive: Hive::Hkcu,
            path: GPU_KEY.into(),
            name: exe_path.into(),
            value: RegValue::String("GpuPreference=2;".into()),
            only_if_key_exists: false,
        }],
        after: After::None,
        hooks,
    })
}

// ---------------------------------------------------------------- Defender exclusion

/// Adds a Defender folder exclusion. Flagged `SecurityRisk` (not recommended; locked after a Windows update).
pub struct DefenderExclusionTweak {
    pub cmd: Arc<dyn Cmd>,
    pub id: String,
    pub title: String,
    pub folder: String,
}

impl DefenderExclusionTweak {
    pub const FLAGS: TweakFlags = TweakFlags(TweakFlags::SECURITY_RISK);

    pub fn new(cmd: Arc<dyn Cmd>, game_id: &str, game_name: &str, folder: &str) -> Result<DefenderExclusionTweak> {
        if folder.trim().is_empty() {
            return Err(Error::Catalog("Defender exclusion needs a folder".into()));
        }
        Ok(DefenderExclusionTweak { cmd, id: game_tweak_id(game_id, "defender"), title: format!("{game_name}: Defender exclusion"), folder: folder.into() })
    }

    fn excluded(&self, folder: &str) -> Result<bool> {
        let out = powershell(&*self.cmd, &format!("(Get-MpPreference).ExclusionPath -contains {}", ps_quote(folder)))?;
        Ok(out.trim().eq_ignore_ascii_case("True"))
    }
}

impl Tweak for DefenderExclusionTweak {
    fn id(&self) -> &str {
        &self.id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn is_applied(&self) -> Result<bool> {
        self.excluded(&self.folder)
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        let had = self.excluded(&self.folder)?;
        b.data.entry("alreadyExcluded".into()).or_insert_with(|| if had { "1" } else { "0" }.into());
        b.data.entry("folder".into()).or_insert_with(|| self.folder.clone());
        if !had {
            powershell(&*self.cmd, &format!("Add-MpPreference -ExclusionPath {}", ps_quote(&self.folder)))?;
        }
        Ok(())
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        if b.data.get("alreadyExcluded").map(String::as_str) == Some("1") {
            return Ok(()); // the user's own exclusion stays
        }
        let folder = b.data.get("folder").unwrap_or(&self.folder);
        powershell(&*self.cmd, &format!("Remove-MpPreference -ExclusionPath {}", ps_quote(folder))).map(|_| ())
    }
}

// ---------------------------------------------------------------- QoS policy

const QOS_KEY: &str = "SYSTEM\\CurrentControlSet\\Services\\Tcpip\\QoS";
const QOS_PREFIX: &str = "MyTune - ";

/// DSCP 46 for one exe, plus the "Do not use NLA" switch that makes policies apply.
pub struct QosPolicyTweak {
    pub cmd: Arc<dyn Cmd>,
    pub id: String,
    pub title: String,
    pub exe_name: String,
    pub hive: Hive,
    pub qos_key: String,
}

impl QosPolicyTweak {
    pub fn new(cmd: Arc<dyn Cmd>, game_id: &str, game_name: &str, exe_name: &str) -> Result<QosPolicyTweak> {
        if exe_name.trim().is_empty() {
            return Err(Error::Catalog("QoS policy needs the exe name".into()));
        }
        Ok(QosPolicyTweak {
            cmd,
            id: game_tweak_id(game_id, "qos"),
            title: format!("{game_name}: network priority"),
            exe_name: exe_name.into(),
            hive: Hive::Hklm,
            qos_key: QOS_KEY.into(),
        })
    }

    fn policy_name(&self) -> String {
        format!("{QOS_PREFIX}{}", self.exe_name)
    }

    fn exists(&self) -> Result<bool> {
        let out = powershell(&*self.cmd, &format!("try {{ @(Get-NetQosPolicy -Name {}).Count }} catch {{ 0 }}", ps_quote(&self.policy_name())))?;
        Ok(out.trim() == "1")
    }
}

impl Tweak for QosPolicyTweak {
    fn id(&self) -> &str {
        &self.id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn is_applied(&self) -> Result<bool> {
        self.exists()
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        b.capture(self.hive, &self.qos_key, "Do not use NLA")?;
        reg::set(self.hive, &self.qos_key, "Do not use NLA", &RegValue::String("1".into()))?;
        let n = ps_quote(&self.policy_name());
        powershell(
            &*self.cmd,
            &format!(
                "if (Get-NetQosPolicy -Name {n} -ErrorAction SilentlyContinue) {{ Remove-NetQosPolicy -Name {n} -Confirm:$false }}; New-NetQosPolicy -Name {n} -AppPathNameMatchCondition {} -DSCPAction 46 -NetworkProfile All | Out-Null",
                ps_quote(&self.exe_name)
            ),
        )
        .map(|_| ())
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        powershell(&*self.cmd, &format!("Remove-NetQosPolicy -Name {} -Confirm:$false -ErrorAction SilentlyContinue", ps_quote(&self.policy_name())))?;
        let left = powershell(
            &*self.cmd,
            &format!("@(Get-NetQosPolicy -ErrorAction SilentlyContinue | Where-Object Name -like {}).Count", ps_quote(&format!("{QOS_PREFIX}*"))),
        )?;
        if left.trim() == "0" && !b.registry.is_empty() {
            b.restore_registry()?; // last MyTune policy gone: NLA goes back to what it was
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- timer resolution

/// `GlobalTimerResolutionRequests = 1` plus the `Settings.GameTimer` flag in `state.json`.
pub struct TimerResolutionTweak {
    pub inner: DataTweak,
}

impl TimerResolutionTweak {
    pub fn system(hooks: Arc<dyn Hooks>) -> TimerResolutionTweak {
        TimerResolutionTweak {
            inner: DataTweak {
                id: "adv.timer".into(),
                title: "0.5 ms timer resolution while gaming".into(),
                description: String::new(),
                details: String::new(),
                flags: TweakFlags(0),
                removed: false,
                sets: vec![DataSet {
                    hive: Hive::Hklm,
                    path: "SYSTEM\\CurrentControlSet\\Control\\Session Manager\\kernel".into(),
                    name: "GlobalTimerResolutionRequests".into(),
                    value: RegValue::Dword(1),
                    only_if_key_exists: false,
                }],
                after: After::None,
                hooks,
            },
        }
    }

    /// The `Settings.GameTimer` flag as the C# app reads it.
    pub fn flag_set(store: &Store) -> bool {
        store.settings_flag("GameTimer") == Some(true)
    }
}

impl Tweak for TimerResolutionTweak {
    fn id(&self) -> &str {
        self.inner.id()
    }
    fn title(&self) -> &str {
        self.inner.title()
    }
    fn is_applied(&self) -> Result<bool> {
        self.inner.is_applied()
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        self.inner.apply(b)?;
        b.data.insert("enabled".into(), "1".into());
        Ok(())
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        self.inner.revert(b)
    }
    fn on_applied(&self, store: &mut Store) -> Result<()> {
        store.set_settings_flag("GameTimer", true)
    }
    fn on_reverted(&self, store: &mut Store) -> Result<()> {
        store.set_settings_flag("GameTimer", false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Engine, RevertOutcome};
    use crate::process::RunResult;
    use crate::util::b64_decode;
    use std::collections::BTreeSet;
    use std::sync::Mutex;

    struct NoHooks;
    impl Hooks for NoHooks {
        fn broadcast_setting_change(&self) {}
        fn apply_drag_full_windows(&self) {}
        fn restart_explorer(&self) {}
    }

    type Handler = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;
    struct Fake {
        calls: Mutex<Vec<String>>,
        handler: Handler,
    }
    impl Fake {
        fn new(h: impl Fn(&str) -> Option<String> + Send + Sync + 'static) -> Arc<Fake> {
            Arc::new(Fake { calls: Mutex::new(vec![]), handler: Box::new(h) })
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }
    impl Cmd for Fake {
        fn run(&self, _file: &str, args: &[&str]) -> Result<RunResult> {
            let b = b64_decode(args.last().unwrap()).unwrap();
            let u: Vec<u16> = b.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let s = String::from_utf16(&u).unwrap();
            let s = s.trim_start_matches("$ProgressPreference='SilentlyContinue';$ErrorActionPreference='Stop';").to_string();
            self.calls.lock().unwrap().push(s.clone());
            Ok(RunResult { exit_code: 0, output: (self.handler)(&s).unwrap_or_default() })
        }
    }

    struct Scratch(String);
    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let p = format!("Software\\MyTuneTest\\pg-{tag}-{}", std::process::id());
            let _ = reg::delete_tree(Hive::Hkcu, &p);
            Scratch(p)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = reg::delete_tree(Hive::Hkcu, &self.0);
            let _ = reg::delete_key_if_empty(Hive::Hkcu, "Software\\MyTuneTest");
        }
    }

    fn engine() -> (tempfile::TempDir, Engine) {
        let d = tempfile::tempdir().unwrap();
        let e = Engine::new(Store::load(d.path()));
        (d, e)
    }

    #[test]
    fn gpu_tweak_shape_and_guard() {
        let t = gpu_tweak("abc", "Game", "C:\\Games\\g.exe", Arc::new(NoHooks)).unwrap();
        assert_eq!(t.id, "game:abc:gpu");
        assert_eq!(t.title, "Game: high-performance GPU");
        assert_eq!(t.sets[0].name, "C:\\Games\\g.exe");
        assert_eq!(t.sets[0].value, RegValue::String("GpuPreference=2;".into()));
        assert!(gpu_tweak("abc", "Game", " ", Arc::new(NoHooks)).is_err());
    }

    #[test]
    fn gpu_tweak_applies_and_reverts_in_scratch() {
        let s = Scratch::new("gpu");
        let mut t = gpu_tweak("abc", "Game", "C:\\Games\\g.exe", Arc::new(NoHooks)).unwrap();
        t.sets[0].path = s.0.clone();
        reg::set(Hive::Hkcu, &s.0, "C:\\Games\\g.exe", &RegValue::String("GpuPreference=1;".into())).unwrap();
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        assert!(t.is_applied().unwrap());
        e.revert(&t).unwrap();
        assert_eq!(reg::get(Hive::Hkcu, &s.0, "C:\\Games\\g.exe").unwrap(), Some(RegValue::String("GpuPreference=1;".into())));
    }

    // ------------------------------------------------------------ Defender

    fn defender_fake(excluded: Arc<Mutex<BTreeSet<String>>>) -> Arc<Fake> {
        Fake::new(move |s| {
            // the quoted folder: between the first and last quote, '' unescaped
            let folder = s[s.find('\'').unwrap() + 1..s.rfind('\'').unwrap()].replace("''", "'");
            let mut set = excluded.lock().unwrap();
            if s.starts_with("(Get-MpPreference)") {
                Some(if set.contains(&folder) { "True" } else { "False" }.into())
            } else if s.starts_with("Add-MpPreference") {
                set.insert(folder);
                None
            } else if s.starts_with("Remove-MpPreference") {
                set.remove(&folder);
                None
            } else {
                None
            }
        })
    }

    #[test]
    fn defender_adds_and_removes_only_its_own_exclusion() {
        let ex = Arc::new(Mutex::new(BTreeSet::new()));
        let fake = defender_fake(ex.clone());
        let t = DefenderExclusionTweak::new(fake.clone(), "g1", "Game", "C:\\Games\\G'ame").unwrap();
        assert!(DefenderExclusionTweak::FLAGS.has(TweakFlags::SECURITY_RISK));
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        assert!(ex.lock().unwrap().contains("C:\\Games\\G'ame"), "a folder with a quote survives the script escaping");
        assert!(t.is_applied().unwrap());
        assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted);
        assert!(ex.lock().unwrap().is_empty());
    }

    #[test]
    fn defender_keeps_a_users_own_exclusion() {
        let ex = Arc::new(Mutex::new(BTreeSet::from(["C:\\Games".to_string()])));
        let fake = defender_fake(ex.clone());
        let t = DefenderExclusionTweak::new(fake.clone(), "g1", "Game", "C:\\Games").unwrap();
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        assert!(!fake.calls().iter().any(|c| c.starts_with("Add-MpPreference")));
        e.revert(&t).unwrap();
        assert!(ex.lock().unwrap().contains("C:\\Games"), "user's exclusion stays");
        assert!(!fake.calls().iter().any(|c| c.starts_with("Remove-MpPreference")));
    }

    #[test]
    fn defender_revert_uses_the_folder_from_apply() {
        let ex = Arc::new(Mutex::new(BTreeSet::new()));
        let fake = defender_fake(ex.clone());
        let t = DefenderExclusionTweak::new(fake.clone(), "g1", "Game", "C:\\Old").unwrap();
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        let moved = DefenderExclusionTweak::new(fake, "g1", "Game", "C:\\New").unwrap(); // install folder changed
        e.revert(&moved).unwrap();
        assert!(ex.lock().unwrap().is_empty(), "the exclusion that was added is the one removed");
        assert!(DefenderExclusionTweak::new(Fake::new(|_| None), "g", "G", " ").is_err());
    }

    // ------------------------------------------------------------ QoS

    fn qos_fake(policies: Arc<Mutex<BTreeSet<String>>>) -> Arc<Fake> {
        Fake::new(move |s| {
            let mut p = policies.lock().unwrap();
            if s.starts_with("try { @(Get-NetQosPolicy -Name") {
                let n = s.split('\'').nth(1).unwrap();
                Some(if p.contains(n) { "1" } else { "0" }.into())
            } else if s.starts_with("if (Get-NetQosPolicy") {
                let n = s.split('\'').nth(1).unwrap().to_string();
                p.insert(n);
                None
            } else if s.starts_with("Remove-NetQosPolicy") {
                let n = s.split('\'').nth(1).unwrap();
                p.remove(n);
                None
            } else if s.starts_with("@(Get-NetQosPolicy") {
                Some(p.iter().filter(|n| n.starts_with("MyTune - ")).count().to_string())
            } else {
                None
            }
        })
    }

    fn qos(s: &Scratch, fake: Arc<Fake>, exe: &str) -> QosPolicyTweak {
        let mut t = QosPolicyTweak::new(fake, "g1", "Game", exe).unwrap();
        t.hive = Hive::Hkcu;
        t.qos_key = s.0.clone();
        t
    }

    #[test]
    fn qos_single_policy_restores_nla_to_absent() {
        let s = Scratch::new("qos1");
        let policies = Arc::new(Mutex::new(BTreeSet::new()));
        let t = qos(&s, qos_fake(policies.clone()), "a.exe");
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        assert!(t.is_applied().unwrap());
        e.revert(&t).unwrap();
        assert!(!t.is_applied().unwrap());
        assert_eq!(reg::get(Hive::Hkcu, &s.0, "Do not use NLA").unwrap(), None, "back to the original (absent)");
    }

    /// Known limitation, same as C#: game B's backup captured the value game A had already set ("1"), so with two
    /// games the value ends as "1" (harmless: it only lets QoS policies apply), not as the very first original.
    #[test]
    fn qos_nla_restored_only_after_the_last_policy() {
        let s = Scratch::new("qos");
        let policies = Arc::new(Mutex::new(BTreeSet::new()));
        let fake = qos_fake(policies.clone());
        let a = qos(&s, fake.clone(), "a.exe");
        let b = {
            let mut t = qos(&s, fake.clone(), "b.exe");
            t.id = "game:g2:qos".into();
            t
        };
        let (_d, e) = engine();
        e.apply(&a).unwrap();
        e.apply(&b).unwrap();
        assert_eq!(reg::get(Hive::Hkcu, &s.0, "Do not use NLA").unwrap(), Some(RegValue::String("1".into())));
        assert!(a.is_applied().unwrap() && b.is_applied().unwrap());
        e.revert(&a).unwrap();
        assert!(!a.is_applied().unwrap() && b.is_applied().unwrap());
        assert_eq!(reg::get(Hive::Hkcu, &s.0, "Do not use NLA").unwrap(), Some(RegValue::String("1".into())), "b still needs NLA off");
        e.revert(&b).unwrap();
        assert_eq!(reg::get(Hive::Hkcu, &s.0, "Do not use NLA").unwrap(), Some(RegValue::String("1".into())), "limitation: B captured \"1\"");
        assert!(fake.calls().iter().any(|c| c.contains("-DSCPAction 46 -NetworkProfile All") && c.contains("-AppPathNameMatchCondition 'a.exe'")));
    }

    // ------------------------------------------------------------ timer resolution

    #[test]
    fn timer_sets_and_clears_the_state_flag() {
        let s = Scratch::new("timer");
        let mut t = TimerResolutionTweak::system(Arc::new(NoHooks));
        t.inner.sets[0].hive = Hive::Hkcu;
        t.inner.sets[0].path = s.0.clone();
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("state.json"), r#"{"Settings":{"Theme":"dark"}}"#).unwrap();
        let e = Engine::new(Store::load(d.path()));
        e.apply(&t).unwrap();
        assert!(t.is_applied().unwrap());
        e.with_store(|st| {
            assert!(TimerResolutionTweak::flag_set(st));
            assert_eq!(st.get_backup("adv.timer").unwrap().data["enabled"], "1");
        });
        let reloaded = Store::load(d.path());
        assert!(TimerResolutionTweak::flag_set(&reloaded), "flag is on disk");
        assert_eq!(reloaded.other("Settings").unwrap()["Theme"], "dark");
        assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted);
        assert!(!TimerResolutionTweak::flag_set(&Store::load(d.path())));
        assert_eq!(reg::get(Hive::Hkcu, &s.0, "GlobalTimerResolutionRequests").unwrap(), None);
    }

    #[test]
    fn timer_failed_revert_keeps_flag_and_backup() {
        let s = Scratch::new("timerfail");
        let mut t = TimerResolutionTweak::system(Arc::new(NoHooks));
        t.inner.sets[0].hive = Hive::Hkcu;
        t.inner.sets[0].path = s.0.clone();
        let d = tempfile::tempdir().unwrap();
        let e = Engine::new(Store::load(d.path()));
        e.apply(&t).unwrap();
        e.with_store(|st| {
            let mut b = st.get_backup("adv.timer").unwrap().clone();
            b.registry[0].hive = "HKXX".into();
            st.set_backup("adv.timer", b).unwrap();
        });
        assert!(e.revert(&t).is_err());
        e.with_store(|st| {
            assert!(st.get_backup("adv.timer").is_some());
            assert!(TimerResolutionTweak::flag_set(st), "flag stays on while the revert failed");
        });
    }
}
