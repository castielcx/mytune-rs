//! Per-game tweaks: GPU preference, Defender exclusion and QoS policy, and `revert_only` for their stored backups.
//!
//! Per-game ids are `game:<GameEntry.Id>:gpu|defender|qos|nvidia` (nvidia is in `nvidia.rs`); the caller builds
//! the tweak on demand, exactly like C# `GameService`. Programs go through `Cmd`, so tests use a fake.
//!
//! Differences from C# (never weaker):
//! - Defender: the excluded folder is stored in the backup (`folder`) and revert uses that one. C# recomputes it
//!   at revert, which can leave the exclusion behind if the install folder changed.
//! - QoS: unchanged from C# (value restored only when the last `MyTune *` policy is gone). Known limitation kept:
//!   with several games the value ends as `1`, because later games captured what the first had already set.

use crate::backup::{RegBackup, TweakBackup};
use crate::catalog::TweakFlags;
use crate::engine::{RegistryTweak, Tweak};
use crate::error::{Error, Result};
use crate::gameprofile::ConfigTweak;
use crate::procs::WinProcs;
use crate::reg::{self, Hive, RegValue};
use crate::special::{powershell, ps_quote};
use crate::store::Store;
use crate::tweaks::{system_cmd, Cmd};
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;

// ---------------------------------------------------------------- ids

/// `game:<id>:<kind>`.
pub fn game_tweak_id(game_id: &str, kind: &str) -> String {
    format!("game:{game_id}:{kind}")
}

// ---------------------------------------------------------------- high-performance GPU

const GPU_KEY: &str = "Software\\Microsoft\\DirectX\\UserGpuPreferences";

const GPU_TOKEN: &str = "GpuPreference";
const GPU_HIGH: &str = "2";

/// The value of the `GpuPreference` token in data like `AppStatus=0;GpuPreference=2;`.
fn gpu_pref(data: &str) -> Option<&str> {
    data.split(';').find_map(|t| {
        let (k, v) = t.split_once('=')?;
        k.trim().eq_ignore_ascii_case(GPU_TOKEN).then_some(v.trim())
    })
}

/// `data` with only the `GpuPreference` token changed (`None`: taken out); the other tokens stay in their order.
fn with_gpu_pref(data: &str, pref: Option<&str>) -> String {
    let (mut out, mut placed) = (String::new(), false);
    for t in data.split(';').filter(|t| !t.trim().is_empty()) {
        if !t.split_once('=').is_some_and(|(k, _)| k.trim().eq_ignore_ascii_case(GPU_TOKEN)) {
            out.push_str(t);
            out.push(';');
        } else if let (Some(p), false) = (pref, placed) {
            out.push_str(&format!("{GPU_TOKEN}={p};"));
            placed = true;
        }
    }
    if let (Some(p), false) = (pref, placed) {
        out.push_str(&format!("{GPU_TOKEN}={p};"));
    }
    out
}

/// An Unreal game's root exe is a stub that starts `<Name>\Binaries\Win64\<Name>-Win64-Shipping.exe`. Windows
/// reads the preference per exe, so the real binary needs it too.
fn unreal_binary(exe: &Path) -> Option<PathBuf> {
    let (dir, stem) = (exe.parent()?, exe.file_stem()?.to_str()?);
    let real = dir.join(stem).join("Binaries\\Win64").join(format!("{stem}-Win64-Shipping.exe"));
    real.is_file().then_some(real)
}

/// The registry calls of `GpuTweak`, so tests can run it on a fake.
trait GpuReg: Send + Sync {
    fn get(&self, path: &str, name: &str) -> Result<Option<RegValue>>;
    fn set(&self, path: &str, name: &str, data: String) -> Result<()>;
    fn delete(&self, path: &str, name: &str) -> Result<()>;
    fn capture(&self, b: &mut TweakBackup, path: &str, name: &str) -> Result<()>;
    fn restore(&self, r: &RegBackup) -> Result<()>;
}

struct HkcuGpu;

impl GpuReg for HkcuGpu {
    fn get(&self, path: &str, name: &str) -> Result<Option<RegValue>> {
        reg::get(Hive::Hkcu, path, name)
    }
    fn set(&self, path: &str, name: &str, data: String) -> Result<()> {
        reg::set(Hive::Hkcu, path, name, &RegValue::String(data))
    }
    fn delete(&self, path: &str, name: &str) -> Result<()> {
        reg::delete_value(Hive::Hkcu, path, name)
    }
    fn capture(&self, b: &mut TweakBackup, path: &str, name: &str) -> Result<()> {
        b.capture(Hive::Hkcu, path, name)
    }
    fn restore(&self, r: &RegBackup) -> Result<()> {
        r.restore()
    }
}

/// "Use the high-performance GPU" for a game: HKCU `UserGpuPreferences`, value NAME = full exe path, token
/// `GpuPreference=2;` in its data. Other tokens of the value (`AppStatus=0;`) are never touched.
pub struct GpuTweak {
    pub id: String,
    pub title: String,
    /// The game's exe, and the real binary when that exe is an Unreal stub. Empty for a revert-only tweak.
    pub exes: Vec<String>,
    pub key: String,
    reg: Arc<dyn GpuReg>,
}

pub fn gpu_tweak(game_id: &str, game_name: &str, exe_path: &str) -> Result<GpuTweak> {
    if exe_path.trim().is_empty() {
        return Err(Error::Catalog("GPU preference needs the game's exe path".into()));
    }
    let mut exes = vec![exe_path.to_string()];
    exes.extend(unreal_binary(Path::new(exe_path)).map(|p| p.to_string_lossy().to_string()));
    Ok(GpuTweak { id: game_tweak_id(game_id, "gpu"), title: format!("{game_name}: high-performance GPU"), exes, key: GPU_KEY.into(), reg: Arc::new(HkcuGpu) })
}

impl GpuTweak {
    /// Puts one value's GPU token back as the backup has it. Tokens added since stay.
    fn restore_token(&self, r: &RegBackup) -> Result<()> {
        if Hive::parse(&r.hive) != Some(Hive::Hkcu) {
            return Err(Error::BadBackup(format!("unexpected hive {}", r.hive)));
        }
        let original = if r.existed { r.value.as_deref().and_then(gpu_pref) } else { None };
        match self.reg.get(&r.path, &r.name)? {
            Some(RegValue::String(now)) => match with_gpu_pref(&now, original) {
                new if new.is_empty() => self.reg.restore(r),
                new => self.reg.set(&r.path, &r.name, new),
            },
            _ => self.reg.restore(r),
        }
    }

    /// Takes the high-performance token out of one value. True when it was there.
    fn clear_high(&self, exe: &str) -> Result<bool> {
        let Some(RegValue::String(now)) = self.reg.get(&self.key, exe)? else { return Ok(false) };
        if gpu_pref(&now) != Some(GPU_HIGH) {
            return Ok(false);
        }
        match with_gpu_pref(&now, None) {
            rest if rest.is_empty() => self.reg.delete(&self.key, exe)?,
            rest => self.reg.set(&self.key, exe, rest)?,
        }
        Ok(true)
    }
}

impl Tweak for GpuTweak {
    fn id(&self) -> &str {
        &self.id
    }
    fn title(&self) -> &str {
        &self.title
    }
    /// On when either path has it: a stub set by an older version must not read off after an update.
    fn is_applied(&self) -> Result<bool> {
        for exe in &self.exes {
            if matches!(self.reg.get(&self.key, exe)?, Some(RegValue::String(s)) if gpu_pref(&s) == Some(GPU_HIGH)) {
                return Ok(true);
            }
        }
        Ok(false)
    }
    fn capture(&self, b: &mut TweakBackup) -> Result<()> {
        self.exes.iter().try_for_each(|exe| self.reg.capture(b, &self.key, exe))
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        for exe in &self.exes {
            self.reg.capture(b, &self.key, exe)?;
            let now = match self.reg.get(&self.key, exe)? {
                Some(RegValue::String(s)) => s,
                None => String::new(),
                Some(_) => return Err(Error::Plain(format!("{exe}: the GPU preference is not text, so it was left alone."))),
            };
            if gpu_pref(&now) != Some(GPU_HIGH) {
                self.reg.set(&self.key, exe, with_gpu_pref(&now, Some(GPU_HIGH)))?;
            }
        }
        Ok(())
    }
    /// Only the paths the backup records: a token on a path MyTune never wrote is the user's.
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        let failed: Vec<String> = b.registry.iter().rev().filter_map(|r| self.restore_token(r).err().map(|e| format!("{}: {e}", r.name))).collect();
        if failed.is_empty() {
            Ok(())
        } else {
            Err(Error::Revert(failed))
        }
    }
    /// No backup (set before MyTune kept one): takes the high-performance token out of the game's listed exe.
    /// Versions without a backup never wrote the real binary, so a token there is the user's.
    fn revert_default(&self) -> Result<bool> {
        self.exes.first().map_or(Ok(false), |exe| self.clear_high(exe))
    }
}

// ---------------------------------------------------------------- reverting a stored backup

/// True for the ids `revert_only` can build: tweaks that are not in the catalog list.
pub fn reverts_alone(id: &str) -> bool {
    id == crate::pccheck::GAME_MODE_ID || id.strip_prefix("game:").and_then(|r| r.rsplit_once(':')).is_some_and(|(_, kind)| ["config", "gpu", "defender", "qos", "nvidia"].contains(&kind))
}

/// A tweak that reverts this id's stored backup without the game being in the list or installed.
pub fn revert_only(store: &Store, id: &str, title: &str) -> Option<Box<dyn Tweak>> {
    if !reverts_alone(id) {
        return None;
    }
    let (i, t) = (id.to_string(), title.to_string());
    if id == crate::pccheck::GAME_MODE_ID {
        return Some(Box::new(RegistryTweak { id: i, title: t, sets: vec![] }));
    }
    let (game, kind) = id.strip_prefix("game:")?.rsplit_once(':')?;
    // QoS policies and NVIDIA applications are named after the exe, which these old backups do not hold
    let exe = || {
        let games = store.top("Games").and_then(Value::as_array);
        let path = games.and_then(|g| g.iter().find(|g| g["Id"] == game)).and_then(|g| g["ExePath"].as_str());
        path.and_then(|p| Path::new(p).file_name()).map_or(String::new(), |f| f.to_string_lossy().to_string())
    };
    let tweak: Box<dyn Tweak> = match kind {
        "config" => Box::new(ConfigTweak::for_revert(Arc::new(WinProcs), id, title)),
        "gpu" => Box::new(GpuTweak { id: i, title: t, exes: vec![], key: GPU_KEY.into(), reg: Arc::new(HkcuGpu) }),
        "defender" => Box::new(DefenderExclusionTweak { cmd: system_cmd(), id: i, title: t, folder: String::new() }),
        "qos" => Box::new(QosPolicyTweak { cmd: system_cmd(), id: i, title: t, exe_name: exe(), hive: Hive::Hklm, qos_key: QOS_KEY.into() }),
        _ => Box::new(crate::nvidia::NvidiaGameTweak { id: i, exe: exe(), game_name: t, frame_cap: 0 }),
    };
    Some(tweak)
}

/// Revert-only tweaks for the entries in the engine's journal (`Pending` in state.json), so `Engine::recover`
/// can put back a change that was cut off.
pub fn pending_reverts(store: &Store) -> Vec<Box<dyn Tweak>> {
    let pending = store.top("Pending").and_then(Value::as_object).cloned().unwrap_or_default();
    pending.iter().filter_map(|(id, e)| revert_only(store, id, e["Title"].as_str().unwrap_or(id))).collect()
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

    fn note(&self, b: &mut TweakBackup, had: bool) {
        b.data.entry("alreadyExcluded".into()).or_insert_with(|| if had { "1" } else { "0" }.into());
        b.data.entry("folder".into()).or_insert_with(|| self.folder.clone());
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
    fn capture(&self, b: &mut TweakBackup) -> Result<()> {
        let had = self.excluded(&self.folder)?;
        self.note(b, had);
        Ok(())
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        let had = self.excluded(&self.folder)?;
        self.note(b, had);
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
        if folder.trim().is_empty() {
            return Err(Error::BadBackup(format!("{}: the backup does not say which folder was excluded", self.id)));
        }
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
    fn capture(&self, b: &mut TweakBackup) -> Result<()> {
        b.capture(self.hive, &self.qos_key, "Do not use NLA")
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
        if self.exe_name.trim().is_empty() {
            return Err(Error::Plain(format!("{}: the game is no longer in the list, so MyTune does not know which network policy to remove. Nothing changed.", self.title)));
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Engine, RevertOutcome};
    use crate::process::RunResult;
    use crate::util::b64_decode;
    use std::collections::BTreeSet;
    use std::sync::Mutex;

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
        let t = gpu_tweak("abc", "Game", "C:\\NoSuchFolder\\g.exe").unwrap();
        assert_eq!(t.id, "game:abc:gpu");
        assert_eq!(t.title, "Game: high-performance GPU");
        assert_eq!(t.exes, ["C:\\NoSuchFolder\\g.exe"]);
        assert!(gpu_tweak("abc", "Game", " ").is_err());
    }

    #[test]
    fn gpu_tokens_change_one_by_one() {
        assert_eq!(with_gpu_pref("", Some("2")), "GpuPreference=2;");
        assert_eq!(with_gpu_pref("AppStatus=0;", Some("2")), "AppStatus=0;GpuPreference=2;");
        assert_eq!(with_gpu_pref("AppStatus=0;GpuPreference=1;Other=x;", Some("2")), "AppStatus=0;GpuPreference=2;Other=x;");
        assert_eq!(with_gpu_pref("GpuPreference=2;AppStatus=0;", None), "AppStatus=0;");
        assert_eq!(with_gpu_pref("GpuPreference=2;", None), "");
        assert_eq!((gpu_pref("AppStatus=0;GpuPreference=2;"), gpu_pref("AppStatus=0;")), (Some("2"), None));
    }

    /// The registry values under one key; `fail` names a value whose writes fail.
    #[derive(Default)]
    struct FakeGpu {
        values: Mutex<std::collections::BTreeMap<String, String>>,
        fail: Mutex<Option<String>>,
    }
    impl FakeGpu {
        fn check(&self, name: &str) -> Result<()> {
            match self.fail.lock().unwrap().as_deref() == Some(name) {
                true => Err(Error::Plain("write failed".into())),
                false => Ok(()),
            }
        }
        fn value(&self, name: &str) -> Option<String> {
            self.values.lock().unwrap().get(name).cloned()
        }
    }
    impl GpuReg for FakeGpu {
        fn get(&self, _: &str, name: &str) -> Result<Option<RegValue>> {
            Ok(self.value(name).map(RegValue::String))
        }
        fn set(&self, _: &str, name: &str, data: String) -> Result<()> {
            self.check(name)?;
            self.values.lock().unwrap().insert(name.into(), data);
            Ok(())
        }
        fn delete(&self, _: &str, name: &str) -> Result<()> {
            self.check(name)?;
            self.values.lock().unwrap().remove(name);
            Ok(())
        }
        fn capture(&self, b: &mut TweakBackup, path: &str, name: &str) -> Result<()> {
            if !b.registry.iter().any(|r| r.name == name) {
                b.registry.push(gpu_entry(path, name, self.value(name).as_deref()));
            }
            Ok(())
        }
        fn restore(&self, r: &RegBackup) -> Result<()> {
            match &r.value {
                Some(v) => self.set(&r.path, &r.name, v.clone()),
                None => self.delete(&r.path, &r.name),
            }
        }
    }

    fn gpu_entry(path: &str, name: &str, value: Option<&str>) -> RegBackup {
        RegBackup { hive: "HKCU".into(), path: path.into(), name: name.into(), existed: value.is_some(), kind: value.map(|_| "String".into()), value: value.map(String::from), multi: None, created_keys: vec![] }
    }

    #[test]
    fn a_stub_set_by_an_older_version_stays_on_and_reverts_only_the_paths_in_its_backup() {
        let (stub, real) = ("C:\\G\\Pioneer.exe", "C:\\G\\Pioneer\\Binaries\\Win64\\Pioneer-Win64-Shipping.exe");
        let users = "AppStatus=1;GpuPreference=2;";
        let fake = Arc::new(FakeGpu::default());
        let t = GpuTweak { id: "game:g:gpu".into(), title: "G".into(), exes: vec![stub.into(), real.into()], key: "k".into(), reg: fake.clone() };
        fake.values.lock().unwrap().insert(stub.into(), "AppStatus=0;GpuPreference=2;".into());
        assert!(t.is_applied().unwrap(), "the old state reads on");

        // an old backup holds the stub alone: the token on the real exe is the user's and survives
        fake.values.lock().unwrap().insert(real.into(), users.into());
        let mut old = TweakBackup::new("G");
        old.registry.push(gpu_entry("k", stub, Some("AppStatus=0;")));
        let (_d, e) = engine();
        e.with_store(|s| s.set_backup(&t.id, old)).unwrap();
        *fake.fail.lock().unwrap() = Some(stub.into());
        assert!(matches!(e.revert(&t), Err(Error::Revert(_))));
        assert!(e.with_store(|s| s.get_backup(&t.id).is_some()), "backup kept");
        *fake.fail.lock().unwrap() = None;
        assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted);
        assert_eq!((fake.value(stub).as_deref(), fake.value(real).as_deref()), (Some("AppStatus=0;"), Some(users)));
        assert!(t.is_applied().unwrap(), "either path counts, and the real exe still has the user's token");

        // no backup at all: the real exe's token is still left alone
        assert_eq!(e.revert(&t).unwrap(), RevertOutcome::NoBackup);
        assert_eq!(fake.value(real).as_deref(), Some(users));

        // re-applying sets both, the new backup lists both, and revert clears both
        fake.values.lock().unwrap().remove(real);
        let mut b = TweakBackup::new("G");
        t.apply(&mut b).unwrap();
        assert_eq!(fake.value(real).as_deref(), Some("GpuPreference=2;"));
        assert_eq!(b.registry.len(), 2);
        e.with_store(|s| s.set_backup(&t.id, b)).unwrap();
        *fake.fail.lock().unwrap() = Some(real.into());
        assert!(matches!(e.revert(&t), Err(Error::Revert(_))));
        assert!(e.with_store(|s| s.get_backup(&t.id).is_some()), "backup kept");
        assert_eq!(fake.value(stub).as_deref(), Some("AppStatus=0;"), "the path that could be written is back");
        *fake.fail.lock().unwrap() = None;
        assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted);
        assert_eq!((fake.value(stub).as_deref(), fake.value(real)), (Some("AppStatus=0;"), None));
        assert!(!t.is_applied().unwrap() && e.with_store(|s| s.get_backup(&t.id).is_none()));

        // only the new path has it
        fake.values.lock().unwrap().insert(real.into(), "GpuPreference=2;".into());
        assert!(t.is_applied().unwrap());
    }

    #[test]
    fn an_unreal_stub_also_targets_the_real_binary() {
        let d = tempfile::tempdir().unwrap();
        let (stub, real) = (d.path().join("PioneerGame.exe"), d.path().join("PioneerGame\\Binaries\\Win64\\PioneerGame-Win64-Shipping.exe"));
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&stub, b"x").unwrap();
        assert_eq!(gpu_tweak("a", "A", stub.to_str().unwrap()).unwrap().exes.len(), 1, "no real binary beside it");
        std::fs::write(&real, b"x").unwrap();
        assert_eq!(gpu_tweak("a", "A", stub.to_str().unwrap()).unwrap().exes, [stub.to_str().unwrap(), real.to_str().unwrap()]);
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
    fn gpu_tweak_applies_and_reverts_in_scratch() {
        let s = Scratch::new("gpu");
        let mut t = gpu_tweak("abc", "Game", "C:\\Games\\g.exe").unwrap();
        t.key = s.0.clone();
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
    #[ignore = "real system: writes HKCU scratch key"]
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
    #[ignore = "real system: writes HKCU scratch key"]
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

    // ------------------------------------------------------------ revert-only

    #[test]
    fn stored_backups_get_a_revert_only_tweak() {
        let d = tempfile::tempdir().unwrap();
        let mut st = Store::load(d.path());
        st.set_top("Games", serde_json::json!([{ "Id": "g1", "ExePath": "C:\\Games\\g.exe" }]));
        for id in ["game:g1:config", "game:g1:gpu", "game:g1:defender", "game:g1:qos", "game:g1:nvidia", "pccheck.gamemode"] {
            assert_eq!(revert_only(&st, id, "T").map(|t| t.id().to_string()).as_deref(), Some(id));
        }
        assert!(revert_only(&st, "game:g1:priority", "T").is_none() && revert_only(&st, "net.x", "T").is_none());
    }

    #[test]
    fn old_defender_and_qos_backups_revert_or_refuse_without_guessing() {
        let ex = Arc::new(Mutex::new(BTreeSet::from(["C:\\Old".to_string()])));
        let fake = defender_fake(ex.clone());
        let t = DefenderExclusionTweak { cmd: fake.clone(), id: "game:g:defender".into(), title: "G".into(), folder: String::new() };
        let mut b = TweakBackup::new("G");
        assert!(t.revert(&b).is_err() && fake.calls().is_empty(), "no folder known: no command runs");
        b.data.insert("folder".into(), "C:\\Old".into());
        t.revert(&b).unwrap();
        assert!(ex.lock().unwrap().is_empty());

        let fake = Fake::new(|_| None);
        let q = QosPolicyTweak { cmd: fake.clone(), id: "game:g:qos".into(), title: "G".into(), exe_name: String::new(), hive: Hive::Hkcu, qos_key: "x".into() };
        assert!(q.revert(&TweakBackup::new("G")).is_err() && fake.calls().is_empty(), "no exe known: no command runs");
    }
}
