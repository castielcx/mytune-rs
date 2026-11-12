//! Game booster (port of C# `GameBooster`): while any Game Hub game runs it can hold a 0.5 ms timer, quiet
//! toast notifications, switch to the performance power plan and throttle background programs; when the last
//! game exits everything is put back. Settings and state live in the shared `state.json`.
//!
//! Settings read (all in `Settings`, default off): `GameTimer`, `QuietDuringGames`, `AutoPowerPlan`,
//! `ThrottleBackground`. State kept at the top level, same keys as C#: `QuietActive`, `QuietPrevious`
//! (-1 = value was absent), `PowerPrevious`. Each is saved BEFORE the change, so a crash can be undone.
//!
//! Crash recovery: `start()` runs one update with "no game running", which undoes quiet mode and the power plan
//! left behind by a killed process (the timer is released by Windows; throttled programs are not recovered, as in C#).
//!
//! NOT ported: the GPU overclock hook (`Settings.GpuOc*`, NVAPI performance states). Its `GpuOcActive` flag is
//! left untouched. Only ONE booster may run: the C# app and this one would both write `state.json`.
//!
//! Differences from C# (never weaker): the power-plan restore keeps `PowerPrevious` until a plan was really
//! activated (C# cleared it first and ignored a failure, which could strand you on the performance plan); if the
//! saved plan is gone it falls back to Balanced. `stop()` also releases the timer.

use crate::error::{Error, Result};
use crate::log;
use crate::procs::Procs;
use crate::reg::{self, Hive, RegValue};
use crate::store::Store;
use crate::throttle::{BackgroundThrottle, RESCAN_SECS};
use crate::tweaks::{Cmd, HIGH_PERFORMANCE, PLAN_GUID};
use serde_json::Value;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::{s, w};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessInformation, ProcessPowerThrottling, PROCESS_POWER_THROTTLING_STATE};

/// 0.5 ms in 100 ns units.
pub const TARGET_RESOLUTION: u32 = 5000;
const BALANCED: &str = "381b4222-f694-41f0-9685-ff5bb260df2e";
const TOAST_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Notifications\\Settings";
const TOAST_VALUE: &str = "NOC_GLOBAL_SETTING_TOASTS_ENABLED";

// ---------------------------------------------------------------- settings

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BoosterSettings {
    pub game_timer: bool,
    pub quiet_during_games: bool,
    pub auto_power_plan: bool,
    pub throttle_background: bool,
}

impl BoosterSettings {
    pub fn read(store: &Store) -> BoosterSettings {
        let f = |n: &str| store.settings_flag(n) == Some(true);
        BoosterSettings {
            game_timer: f("GameTimer"),
            quiet_during_games: f("QuietDuringGames"),
            auto_power_plan: f("AutoPowerPlan"),
            throttle_background: f("ThrottleBackground"),
        }
    }
}

// ---------------------------------------------------------------- timer

/// Timer resolution requests. Real: `NtTimer` (ntdll). Tests: a fake.
pub trait TimerApi: Send + Sync {
    /// Called once at start (the real one opts this process out of Windows 11 hidden-process timer throttling).
    fn prepare(&self) {}
    /// Finest resolution the hardware offers, 100 ns units.
    fn finest(&self) -> Result<u32>;
    /// Requests `desired`; returns the granted resolution in 100 ns units.
    fn request(&self, desired: u32) -> Result<u32>;
    fn release(&self, desired: u32) -> Result<()>;
}

pub struct NtTimer;

type FnSet = unsafe extern "system" fn(u32, u8, *mut u32) -> i32;
type FnQuery = unsafe extern "system" fn(*mut u32, *mut u32, *mut u32) -> i32;

fn ntdll_fn<T: Copy>(name: windows::core::PCSTR) -> Result<T> {
    unsafe {
        let lib = LoadLibraryW(w!("ntdll.dll")).map_err(|e| Error::Os(format!("ntdll: {e}")))?;
        let p = GetProcAddress(lib, name).ok_or_else(|| Error::Os("ntdll function missing".into()))?;
        Ok(std::mem::transmute_copy::<_, T>(&p))
    }
}

impl TimerApi for NtTimer {
    fn prepare(&self) {
        // PROCESS_POWER_THROTTLING_IGNORE_TIMER_RESOLUTION = 4, state 0 = never throttle our timer requests
        let st = PROCESS_POWER_THROTTLING_STATE { Version: 1, ControlMask: 4, StateMask: 0 };
        unsafe {
            let _ = SetProcessInformation(
                GetCurrentProcess(),
                ProcessPowerThrottling,
                &st as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            );
        }
    }
    fn finest(&self) -> Result<u32> {
        let f = ntdll_fn::<FnQuery>(s!("NtQueryTimerResolution"))?;
        let (mut coarsest, mut finest, mut current) = (0u32, 0u32, 0u32);
        match unsafe { f(&mut coarsest, &mut finest, &mut current) } {
            0 => Ok(finest),
            st => Err(Error::Os(format!("NtQueryTimerResolution 0x{st:08X}"))),
        }
    }
    fn request(&self, desired: u32) -> Result<u32> {
        let f = ntdll_fn::<FnSet>(s!("NtSetTimerResolution"))?;
        let mut current = 0u32;
        match unsafe { f(desired, 1, &mut current) } {
            0 => Ok(current),
            st => Err(Error::Os(format!("NtSetTimerResolution failed (0x{st:08X})"))),
        }
    }
    fn release(&self, desired: u32) -> Result<()> {
        let f = ntdll_fn::<FnSet>(s!("NtSetTimerResolution"))?;
        let mut current = 0u32;
        unsafe { f(desired, 0, &mut current) };
        Ok(())
    }
}

// ---------------------------------------------------------------- booster

pub struct Booster {
    cmd: Arc<dyn Cmd>,
    timer: Arc<dyn TimerApi>,
    throttle: BackgroundThrottle,
    /// Registry location of the toast switch (tests point it at a scratch key).
    pub toast_hive: Hive,
    pub toast_key: String,
    /// MyTune's power plan; used when it exists, else `SCHEME_MIN` (= High performance).
    pub plan_guid: String,
    pub high_performance: String,
    timer_state: Mutex<(bool, f64)>,
    last_scan: Mutex<Option<Instant>>,
}

fn json_bool(store: &Store, key: &str) -> bool {
    store.top(key).and_then(Value::as_bool) == Some(true)
}

impl Booster {
    pub fn new(cmd: Arc<dyn Cmd>, timer: Arc<dyn TimerApi>, procs: Arc<dyn Procs>) -> Booster {
        Booster::with_throttle(cmd, timer, BackgroundThrottle::new(procs))
    }

    pub fn with_throttle(cmd: Arc<dyn Cmd>, timer: Arc<dyn TimerApi>, throttle: BackgroundThrottle) -> Booster {
        Booster {
            cmd,
            timer,
            throttle,
            toast_hive: Hive::Hkcu,
            toast_key: TOAST_KEY.into(),
            plan_guid: PLAN_GUID.into(),
            high_performance: HIGH_PERFORMANCE.into(),
            timer_state: Mutex::new((false, 0.0)),
            last_scan: Mutex::new(None),
        }
    }

    pub fn timer_active(&self) -> bool {
        self.timer_state.lock().unwrap().0
    }

    /// Granted timer resolution in ms while active.
    pub fn timer_ms(&self) -> f64 {
        self.timer_state.lock().unwrap().1
    }

    pub fn throttle(&self) -> &BackgroundThrottle {
        &self.throttle
    }

    /// Opts out of timer throttling and undoes anything a crashed run left behind.
    pub fn start(&self, store: &mut Store) -> Vec<String> {
        self.timer.prepare();
        self.update(store, false)
    }

    /// Everything off, as on exit.
    pub fn stop(&self, store: &mut Store) -> Vec<String> {
        self.update_with(store, false, BoosterSettings::default())
    }

    /// Re-evaluates every feature for "a game is running or not". Returns the failures (each already logged);
    /// one feature failing never stops the others.
    pub fn update(&self, store: &mut Store, gaming: bool) -> Vec<String> {
        let settings = BoosterSettings::read(store);
        self.update_with(store, gaming, settings)
    }

    fn update_with(&self, store: &mut Store, gaming: bool, s: BoosterSettings) -> Vec<String> {
        let mut failures = Vec::new();
        let mut run = |what: &str, r: Result<()>| {
            if let Err(e) = r {
                log::error(&format!("booster {what}"), Some(&e));
                failures.push(format!("{what}: {e}"));
            }
        };
        run("quiet mode", self.set_quiet(store, gaming && s.quiet_during_games));
        run("power plan", self.set_plan(store, gaming && s.auto_power_plan));
        run("background throttle", self.throttle.set(gaming && s.throttle_background));
        run("timer", self.set_timer(gaming && s.game_timer));
        failures
    }

    /// Call every few seconds: looks for background programs that started mid-game.
    pub fn periodic(&self) {
        if !self.throttle.is_active() {
            return;
        }
        let mut last = self.last_scan.lock().unwrap();
        if last.is_some_and(|t| t.elapsed() < Duration::from_secs(RESCAN_SECS)) {
            return;
        }
        *last = Some(Instant::now());
        drop(last);
        if let Err(e) = self.throttle.scan() {
            log::error("booster background scan", Some(&e));
        }
    }

    // ---- quiet toasts

    fn set_quiet(&self, store: &mut Store, on: bool) -> Result<()> {
        if on == json_bool(store, "QuietActive") {
            return Ok(());
        }
        if on {
            let prev: i64 = match reg::get(self.toast_hive, &self.toast_key, TOAST_VALUE)? {
                Some(RegValue::Dword(d)) => d as i32 as i64,
                _ => -1,
            };
            store.set_top("QuietPrevious", Value::from(prev));
            store.set_top("QuietActive", Value::Bool(true));
            store.save()?; // before the change: a crash can still be undone
            reg::set(self.toast_hive, &self.toast_key, TOAST_VALUE, &RegValue::Dword(0))?;
            log::info("booster: quiet mode on");
        } else {
            let prev = store.top("QuietPrevious").and_then(Value::as_i64).unwrap_or(-1);
            if prev < 0 {
                reg::delete_value(self.toast_hive, &self.toast_key, TOAST_VALUE)?;
            } else {
                reg::set(self.toast_hive, &self.toast_key, TOAST_VALUE, &RegValue::Dword(prev as u32))?;
            }
            store.set_top("QuietActive", Value::Bool(false));
            store.save()?;
            log::info("booster: quiet mode off");
        }
        Ok(())
    }

    // ---- power plan

    fn powercfg(&self, args: &[&str]) -> Result<crate::process::RunResult> {
        self.cmd.run(&crate::process::system32("powercfg.exe").to_string_lossy(), args)
    }

    fn active_plan(&self) -> Result<Option<String>> {
        let r = self.powercfg(&["/getactivescheme"])?;
        if r.exit_code != 0 {
            return Err(Error::ProcessFailed { cmd: "powercfg /getactivescheme".into(), code: r.exit_code, output: r.output });
        }
        Ok(crate::tweaks::find_guid(&r.output))
    }

    fn activate(&self, plan: &str) -> Result<()> {
        let r = self.powercfg(&["/setactive", plan])?;
        if r.exit_code == 0 {
            Ok(())
        } else {
            Err(Error::ProcessFailed { cmd: format!("powercfg /setactive {plan}"), code: r.exit_code, output: r.output })
        }
    }

    fn set_plan(&self, store: &mut Store, on: bool) -> Result<()> {
        let previous = store.top("PowerPrevious").and_then(Value::as_str).map(String::from);
        if on {
            if previous.is_some() {
                return Ok(()); // already switched by us
            }
            let current = self.active_plan()?;
            let listed = self.powercfg(&["/list"])?.output.to_ascii_lowercase().contains(&self.plan_guid.to_ascii_lowercase());
            let target = if listed { self.plan_guid.clone() } else { "SCHEME_MIN".to_string() };
            let Some(current) = current else { return Ok(()) };
            if current.eq_ignore_ascii_case(&target) || current.eq_ignore_ascii_case(&self.high_performance) {
                return Ok(()); // already on a performance plan: nothing to change, nothing to restore
            }
            store.set_top("PowerPrevious", Value::String(current.clone()));
            store.save()?;
            self.activate(&target)?;
            log::info(&format!("booster: power plan {target} (was {current})"));
        } else if let Some(previous) = previous {
            let restored = self.activate(&previous).or_else(|_| self.activate(BALANCED));
            restored?; // still recorded if both fail, so the next update retries
            store.set_top("PowerPrevious", Value::Null);
            store.save()?;
            log::info(&format!("booster: power plan restored to {previous}"));
        }
        Ok(())
    }

    // ---- timer

    fn set_timer(&self, want: bool) -> Result<()> {
        let mut st = self.timer_state.lock().unwrap();
        if want == st.0 {
            return Ok(());
        }
        if want {
            let finest = self.timer.finest()?;
            let granted = self.timer.request(TARGET_RESOLUTION.max(finest))?;
            *st = (true, granted as f64 / 10000.0);
            log::info(&format!("booster: timer resolution {} ms", st.1));
        } else {
            self.timer.release(TARGET_RESOLUTION)?;
            *st = (false, 0.0);
            log::info("booster: timer resolution released");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procs::fake::FakeProcs;
    use crate::process::RunResult;
    use std::sync::atomic::{AtomicU32, Ordering};

    struct FakeTimer {
        finest: u32,
        requests: Mutex<Vec<u32>>,
        releases: AtomicU32,
    }
    impl FakeTimer {
        fn new(finest: u32) -> Arc<FakeTimer> {
            Arc::new(FakeTimer { finest, requests: Mutex::new(vec![]), releases: AtomicU32::new(0) })
        }
    }
    impl TimerApi for FakeTimer {
        fn finest(&self) -> Result<u32> {
            Ok(self.finest)
        }
        fn request(&self, d: u32) -> Result<u32> {
            self.requests.lock().unwrap().push(d);
            Ok(d)
        }
        fn release(&self, _: u32) -> Result<()> {
            self.releases.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    /// powercfg fake with a mutable active plan and a plan list.
    struct FakePower {
        active: Mutex<String>,
        list: Vec<String>,
        calls: Mutex<Vec<String>>,
        fail_setactive: Mutex<Vec<String>>,
    }
    impl FakePower {
        fn new(active: &str, list: &[&str]) -> Arc<FakePower> {
            Arc::new(FakePower { active: Mutex::new(active.into()), list: list.iter().map(|s| s.to_string()).collect(), calls: Mutex::new(vec![]), fail_setactive: Mutex::new(vec![]) })
        }
        fn active(&self) -> String {
            self.active.lock().unwrap().clone()
        }
    }
    impl Cmd for FakePower {
        fn run(&self, _file: &str, args: &[&str]) -> Result<RunResult> {
            self.calls.lock().unwrap().push(args.join(" "));
            let ok = |o: String| Ok(RunResult { exit_code: 0, output: o });
            match args {
                ["/getactivescheme"] => ok(format!("Power Scheme GUID: {}  (x)", self.active())),
                ["/list"] => ok(self.list.join("\n")),
                ["/setactive", p] => {
                    if self.fail_setactive.lock().unwrap().iter().any(|f| f == p) {
                        return Ok(RunResult { exit_code: 1, output: "no such plan".into() });
                    }
                    *self.active.lock().unwrap() = if *p == "SCHEME_MIN" { HIGH_PERFORMANCE.into() } else { p.to_string() };
                    ok(String::new())
                }
                _ => ok(String::new()),
            }
        }
    }

    struct Scratch(String);
    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let p = format!("Software\\MyTuneTest\\bo-{tag}-{}", std::process::id());
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

    const MY_PLAN: &str = "aaaaaaaa-1111-2222-3333-bbbbbbbbbbbb";
    const USER_PLAN: &str = "cccccccc-1111-2222-3333-dddddddddddd";

    fn store_with(settings: &str) -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("state.json"), format!(r#"{{"Settings":{settings},"Games":[]}}"#)).unwrap();
        let s = Store::load(d.path());
        (d, s)
    }

    fn booster(power: Arc<FakePower>, timer: Arc<FakeTimer>, procs: Arc<FakeProcs>, scratch: &Scratch) -> Booster {
        let mut b = Booster::new(power, timer, procs);
        b.toast_key = scratch.0.clone();
        b.plan_guid = MY_PLAN.into();
        b
    }

    #[test]
    fn nothing_changes_when_all_settings_are_off() {
        let sc = Scratch::new("off");
        let (_d, mut st) = store_with("{}");
        let power = FakePower::new(USER_PLAN, &[MY_PLAN]);
        let timer = FakeTimer::new(5000);
        let b = booster(power.clone(), timer.clone(), FakeProcs::new(&[(1, "chrome")]), &sc);
        assert!(b.update(&mut st, true).is_empty());
        assert!(power.calls.lock().unwrap().is_empty() && timer.requests.lock().unwrap().is_empty());
        assert_eq!(reg::get(Hive::Hkcu, &sc.0, TOAST_VALUE).unwrap(), None);
        assert!(!b.timer_active() && !b.throttle().is_active());
    }

    #[test]
    fn quiet_mode_on_off_keeps_the_previous_value() {
        for (prev, label) in [(None, "absent"), (Some(1u32), "one")] {
            let sc = Scratch::new(&format!("quiet-{label}"));
            if let Some(v) = prev {
                reg::set(Hive::Hkcu, &sc.0, TOAST_VALUE, &RegValue::Dword(v)).unwrap();
            }
            let (d, mut st) = store_with(r#"{"QuietDuringGames":true}"#);
            let b = booster(FakePower::new(USER_PLAN, &[]), FakeTimer::new(5000), FakeProcs::new(&[]), &sc);
            assert!(b.update(&mut st, true).is_empty());
            assert_eq!(reg::get(Hive::Hkcu, &sc.0, TOAST_VALUE).unwrap(), Some(RegValue::Dword(0)));
            let on_disk = Store::load(d.path());
            assert_eq!(on_disk.top("QuietActive"), Some(&Value::Bool(true)), "saved before/with the change");
            assert!(b.update(&mut st, true).is_empty(), "second update is a no-op");
            assert!(b.update(&mut st, false).is_empty());
            assert_eq!(reg::get(Hive::Hkcu, &sc.0, TOAST_VALUE).unwrap().map(|v| match v { RegValue::Dword(d) => Some(d), _ => None }).unwrap_or(None), prev);
            assert_eq!(Store::load(d.path()).top("QuietActive"), Some(&Value::Bool(false)));
        }
    }

    #[test]
    fn crash_recovery_undoes_quiet_mode_and_power_plan_on_start() {
        let sc = Scratch::new("crash");
        reg::set(Hive::Hkcu, &sc.0, TOAST_VALUE, &RegValue::Dword(0)).unwrap(); // left behind by the dead process
        let (d, mut st) = store_with(r#"{"QuietDuringGames":true,"AutoPowerPlan":true}"#);
        st.set_top("QuietActive", Value::Bool(true));
        st.set_top("QuietPrevious", Value::from(1));
        st.set_top("PowerPrevious", Value::String(USER_PLAN.into()));
        st.save().unwrap();
        let power = FakePower::new(MY_PLAN, &[MY_PLAN, USER_PLAN]);
        let timer = FakeTimer::new(5000);
        let b = booster(power.clone(), timer.clone(), FakeProcs::new(&[]), &sc);
        assert!(b.start(&mut st).is_empty());
        assert_eq!(reg::get(Hive::Hkcu, &sc.0, TOAST_VALUE).unwrap(), Some(RegValue::Dword(1)), "toasts back");
        assert_eq!(power.active(), USER_PLAN, "power plan back");
        let disk = Store::load(d.path());
        assert_eq!(disk.top("QuietActive"), Some(&Value::Bool(false)));
        assert_eq!(disk.top("PowerPrevious"), Some(&Value::Null));
    }

    #[test]
    fn power_plan_switch_and_restore() {
        let sc = Scratch::new("plan");
        let (d, mut st) = store_with(r#"{"AutoPowerPlan":true}"#);
        let power = FakePower::new(USER_PLAN, &[MY_PLAN, USER_PLAN]);
        let b = booster(power.clone(), FakeTimer::new(5000), FakeProcs::new(&[]), &sc);
        assert!(b.update(&mut st, true).is_empty());
        assert_eq!(power.active(), MY_PLAN);
        assert_eq!(Store::load(d.path()).top("PowerPrevious").and_then(Value::as_str), Some(USER_PLAN));
        let n = power.calls.lock().unwrap().len();
        assert!(b.update(&mut st, true).is_empty());
        assert_eq!(power.calls.lock().unwrap().len(), n, "no second switch while recorded");
        assert!(b.update(&mut st, false).is_empty());
        assert_eq!(power.active(), USER_PLAN);
        assert_eq!(Store::load(d.path()).top("PowerPrevious"), Some(&Value::Null));
    }

    #[test]
    fn power_plan_falls_back_to_scheme_min_and_skips_when_already_fast() {
        let sc = Scratch::new("plan2");
        let (_d, mut st) = store_with(r#"{"AutoPowerPlan":true}"#);
        let power = FakePower::new(USER_PLAN, &[USER_PLAN]); // MyTune's plan does not exist
        let b = booster(power.clone(), FakeTimer::new(5000), FakeProcs::new(&[]), &sc);
        b.update(&mut st, true);
        assert!(power.calls.lock().unwrap().contains(&"/setactive SCHEME_MIN".to_string()));
        assert_eq!(power.active(), HIGH_PERFORMANCE);
        // already on high performance: nothing recorded
        let (_d2, mut st2) = store_with(r#"{"AutoPowerPlan":true}"#);
        let fast = FakePower::new(HIGH_PERFORMANCE, &[MY_PLAN]);
        let b2 = booster(fast.clone(), FakeTimer::new(5000), FakeProcs::new(&[]), &sc);
        b2.update(&mut st2, true);
        assert_eq!(st2.top("PowerPrevious"), None);
        assert!(!fast.calls.lock().unwrap().iter().any(|c| c.starts_with("/setactive")));
    }

    #[test]
    fn power_restore_failure_keeps_the_record_and_falls_back_to_balanced() {
        let sc = Scratch::new("plan3");
        let (_d, mut st) = store_with(r#"{"AutoPowerPlan":true}"#);
        let power = FakePower::new(USER_PLAN, &[MY_PLAN]);
        let b = booster(power.clone(), FakeTimer::new(5000), FakeProcs::new(&[]), &sc);
        b.update(&mut st, true);
        power.fail_setactive.lock().unwrap().push(USER_PLAN.into()); // the user deleted their plan meanwhile
        assert!(b.update(&mut st, false).is_empty());
        assert_eq!(power.active(), BALANCED, "fell back to Balanced");
        assert_eq!(st.top("PowerPrevious"), Some(&Value::Null));
        // both fail: record stays so the next update retries
        let (_d2, mut st2) = store_with(r#"{"AutoPowerPlan":true}"#);
        let p2 = FakePower::new(USER_PLAN, &[MY_PLAN]);
        let b2 = booster(p2.clone(), FakeTimer::new(5000), FakeProcs::new(&[]), &sc);
        b2.update(&mut st2, true);
        p2.fail_setactive.lock().unwrap().extend([USER_PLAN.to_string(), BALANCED.to_string()]);
        assert_eq!(b2.update(&mut st2, false).len(), 1);
        assert_eq!(st2.top("PowerPrevious").and_then(Value::as_str), Some(USER_PLAN), "still recorded");
    }

    #[test]
    fn timer_uses_the_coarser_of_target_and_hardware_finest() {
        for (finest, expect) in [(5000u32, 5000u32), (4000, 5000), (10000, 10000)] {
            let sc = Scratch::new("timer");
            let (_d, mut st) = store_with(r#"{"GameTimer":true}"#);
            let timer = FakeTimer::new(finest);
            let b = booster(FakePower::new(USER_PLAN, &[]), timer.clone(), FakeProcs::new(&[]), &sc);
            assert!(b.update(&mut st, true).is_empty());
            assert_eq!(*timer.requests.lock().unwrap(), vec![expect]);
            assert!(b.timer_active());
            assert!((b.timer_ms() - expect as f64 / 10000.0).abs() < 1e-9);
            b.update(&mut st, true);
            assert_eq!(timer.requests.lock().unwrap().len(), 1, "no second request");
            b.update(&mut st, false);
            assert_eq!(timer.releases.load(Ordering::SeqCst), 1);
            assert!(!b.timer_active());
        }
    }

    #[test]
    fn throttle_follows_the_game_and_stop_undoes_everything() {
        let sc = Scratch::new("thr");
        let (_d, mut st) = store_with(r#"{"ThrottleBackground":true,"GameTimer":true,"QuietDuringGames":true}"#);
        let procs = FakeProcs::new(&[(1, "chrome"), (2, "steam")]);
        let timer = FakeTimer::new(5000);
        let b = booster(FakePower::new(USER_PLAN, &[]), timer.clone(), procs.clone(), &sc);
        b.update(&mut st, true);
        assert_eq!(b.throttle().held_count(), 1);
        assert!(b.timer_active() && json_bool(&st, "QuietActive"));
        assert!(b.stop(&mut st).is_empty());
        assert_eq!(b.throttle().held_count(), 0);
        assert!(!b.timer_active() && !json_bool(&st, "QuietActive"));
        assert_eq!(reg::get(Hive::Hkcu, &sc.0, TOAST_VALUE).unwrap(), None);
    }

    #[test]
    fn one_failing_feature_does_not_stop_the_others() {
        let sc = Scratch::new("iso");
        let (_d, mut st) = store_with(r#"{"AutoPowerPlan":true,"GameTimer":true}"#);
        let power = FakePower::new(USER_PLAN, &[MY_PLAN]);
        power.fail_setactive.lock().unwrap().push(MY_PLAN.into());
        let timer = FakeTimer::new(5000);
        let b = booster(power, timer.clone(), FakeProcs::new(&[]), &sc);
        let failures = b.update(&mut st, true);
        assert_eq!(failures.len(), 1, "{failures:?}");
        assert!(failures[0].starts_with("power plan"));
        assert!(b.timer_active(), "timer still applied");
    }

    #[test]
    fn settings_reader() {
        let (_d, st) = store_with(r#"{"GameTimer":true,"ThrottleBackground":true,"Other":1}"#);
        assert_eq!(BoosterSettings::read(&st), BoosterSettings { game_timer: true, quiet_during_games: false, auto_power_plan: false, throttle_background: true });
    }
}
