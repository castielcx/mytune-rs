//! Real-system tests of the game booster on DISPOSABLE targets. Needs an elevated shell, so #[ignore]d:
//! `cargo test --test real_booster -- --ignored --nocapture --test-threads=1`
//!
//! - Games and "background programs" are copies of ping.exe started by the test and killed by a guard.
//! - Quiet mode is aimed at a scratch HKCU key (never the real notification setting).
//! - The power plan is switched to a THROWAWAY duplicate of Balanced and then back to what was active before;
//!   a guard restores the original plan and deletes the duplicate even if an assertion fails.
//! - The timer request is real (and released again); it only lasts while this test process lives.

use mytune_core::booster::{Booster, NtTimer, TimerApi};
use mytune_core::gamewatch::{CpuTopology, GameService, GameWatcher};
use mytune_core::process::{run, system32};
use mytune_core::procs::{priority_of, Procs, WinProcs, BELOW_NORMAL, NORMAL};
use mytune_core::reg::{self, Hive, RegValue};
use mytune_core::store::Store;
use mytune_core::throttle::BackgroundThrottle;
use mytune_core::tweaks::{find_guid, system_cmd};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;

fn init() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| std::env::set_var("MYTUNE_DIR", std::env::temp_dir().join("mytune-real-booster-test")));
}

fn exe(n: &str) -> String {
    system32(n).to_string_lossy().into_owned()
}

struct Dummy(Child);
impl Dummy {
    fn spawn(program: &std::path::Path) -> Dummy {
        Dummy(Command::new(program).args(["-n", "300", "127.0.0.1"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap())
    }
    fn pid(&self) -> u32 {
        self.0.id()
    }
}
impl Drop for Dummy {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for(pid: u32, procs: &WinProcs, name: &str) {
    for _ in 0..50 {
        if procs.list().unwrap().iter().any(|p| p.pid == pid && p.name.eq_ignore_ascii_case(name)) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("process {name} ({pid}) did not appear");
}

#[test]
#[ignore]
fn real_cpu_topology_and_process_list() {
    init();
    let t = CpuTopology::detect();
    println!("topology: {t:?}");
    assert!(t.all_mask != 0 && t.performance_mask != 0);
    assert_eq!(t.performance_mask & !t.all_mask, 0, "performance cores are a subset of all cores");
    let list = WinProcs.list().unwrap();
    assert!(list.len() > 20 && list.iter().any(|p| p.name.eq_ignore_ascii_case("explorer")), "{} processes", list.len());
}

#[test]
#[ignore]
fn real_timer_request_and_release() {
    init();
    let t = NtTimer;
    t.prepare();
    let finest = t.finest().unwrap();
    let granted = t.request(5000u32.max(finest)).unwrap();
    println!("finest {} ms, granted {} ms", finest as f64 / 1e4, granted as f64 / 1e4);
    assert!(granted > 0 && granted <= 160_000, "a sane resolution");
    t.release(5000).unwrap();
}

#[test]
#[ignore]
fn real_throttle_lowers_and_restores_priority_of_a_dummy() {
    init();
    let sep = '\\';
    let tmp = tempfile::tempdir().unwrap();
    let bg = tmp.path().join("mytunebg.exe");
    std::fs::copy(format!("{}", exe(&format!("PING.EXE"))), &bg).unwrap();
    let _ = sep;
    let d = Dummy::spawn(&bg);
    let procs = WinProcs;
    wait_for(d.pid(), &procs, "mytunebg");
    assert_eq!(priority_of(d.pid()), Some(NORMAL));
    let t = BackgroundThrottle::with_names(Arc::new(WinProcs), vec!["mytunebg".into()]);
    t.set(true).unwrap();
    assert_eq!(t.held_count(), 1);
    assert_eq!(priority_of(d.pid()), Some(BELOW_NORMAL));
    t.set(false).unwrap();
    assert_eq!(priority_of(d.pid()), Some(NORMAL), "priority restored");
    assert_eq!(t.held_count(), 0);
}

/// Restores the previously active power plan and deletes the throwaway one, always.
struct PlanGuard {
    original: String,
    temp: String,
}
impl Drop for PlanGuard {
    fn drop(&mut self) {
        let _ = run(&exe("powercfg.exe"), &["/setactive", &self.original]);
        let _ = run(&exe("powercfg.exe"), &["/delete", &self.temp]);
    }
}

fn active_plan() -> String {
    find_guid(&run(&exe("powercfg.exe"), &["/getactivescheme"]).unwrap().output).unwrap()
}

#[test]
#[ignore]
fn real_end_to_end_service_with_dummy_game() {
    init();
    let tmp = tempfile::tempdir().unwrap();
    let game_exe = tmp.path().join("mytunegame.exe");
    let bg_exe = tmp.path().join("mytunebg.exe");
    std::fs::copy(exe("PING.EXE"), &game_exe).unwrap();
    std::fs::copy(exe("PING.EXE"), &bg_exe).unwrap();

    // a throwaway power plan: a duplicate of Balanced
    let original = active_plan();
    let temp_plan = "5e7e0000-0000-4000-8000-00000000d00d".to_string();
    let r = run(&exe("powercfg.exe"), &["/duplicatescheme", "381b4222-f694-41f0-9685-ff5bb260df2e", &temp_plan]).unwrap();
    assert_eq!(r.exit_code, 0, "{}", r.output);
    let _plan_guard = PlanGuard { original: original.clone(), temp: temp_plan.clone() };
    println!("active plan before: {original}");

    // state.json: one game ("mytunegame"), all booster features on
    let state_dir = tmp.path().join("state");
    std::fs::create_dir_all(&state_dir).unwrap();
    let games = serde_json::json!({
        "Settings": {"GameTimer": true, "QuietDuringGames": true, "AutoPowerPlan": true, "ThrottleBackground": true},
        "Games": [{"Id": "dummy1", "Name": "Dummy", "ExePath": game_exe.to_string_lossy(), "PlaySeconds": 5}]
    });
    std::fs::write(state_dir.join("state.json"), games.to_string()).unwrap();
    let store = Store::load(&state_dir);

    let sep = '\\';
    let scratch = format!("Software{sep}MyTuneTest{sep}booster-{}", std::process::id());
    let _ = reg::delete_tree(Hive::Hkcu, &scratch);
    reg::set(Hive::Hkcu, &scratch, "NOC_GLOBAL_SETTING_TOASTS_ENABLED", &RegValue::Dword(1)).unwrap();

    let procs: Arc<dyn Procs> = Arc::new(WinProcs);
    let throttle = BackgroundThrottle::with_names(procs.clone(), vec!["mytunebg".into()]);
    let mut booster = Booster::with_throttle(system_cmd(), Arc::new(NtTimer), throttle);
    booster.toast_key = scratch.clone();
    booster.plan_guid = temp_plan.clone();
    let mut service = GameService::new(store, GameWatcher::new(procs.clone()), booster);
    assert!(service.start().is_empty());

    // nothing running yet
    service.tick().unwrap();
    assert!(service.watcher.running().is_empty() && !service.booster.timer_active());
    assert_eq!(active_plan(), original);

    // the "background program" and the "game" start
    let bg = Dummy::spawn(&bg_exe);
    let game = Dummy::spawn(&game_exe);
    wait_for(bg.pid(), &WinProcs, "mytunebg");
    wait_for(game.pid(), &WinProcs, "mytunegame");
    service.tick().unwrap();

    assert_eq!(service.watcher.running().iter().cloned().collect::<Vec<_>>(), ["dummy1"]);
    assert!(service.booster.timer_active(), "timer held");
    println!("timer granted: {} ms", service.booster.timer_ms());
    assert_eq!(reg::get(Hive::Hkcu, &scratch, "NOC_GLOBAL_SETTING_TOASTS_ENABLED").unwrap(), Some(RegValue::Dword(0)), "toasts quiet");
    assert_eq!(active_plan(), temp_plan, "power plan switched");
    assert_eq!(priority_of(bg.pid()), Some(BELOW_NORMAL), "background program throttled");
    assert_eq!(priority_of(game.pid()), Some(NORMAL), "the game itself is never throttled");
    let disk = Store::load(&state_dir);
    assert_eq!(disk.top("QuietActive").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(disk.top("PowerPrevious").and_then(|v| v.as_str()), Some(original.as_str()), "saved for crash recovery");

    // the game exits
    drop(game);
    std::thread::sleep(std::time::Duration::from_millis(500));
    service.tick().unwrap();
    assert!(service.watcher.running().is_empty());
    assert!(!service.booster.timer_active(), "timer released");
    assert_eq!(reg::get(Hive::Hkcu, &scratch, "NOC_GLOBAL_SETTING_TOASTS_ENABLED").unwrap(), Some(RegValue::Dword(1)), "toasts back");
    assert_eq!(active_plan(), original, "power plan back");
    assert_eq!(priority_of(bg.pid()), Some(NORMAL), "background program restored");
    let disk = Store::load(&state_dir);
    assert_eq!(disk.top("QuietActive").and_then(|v| v.as_bool()), Some(false));
    assert!(disk.top("PowerPrevious").map(|v| v.is_null()).unwrap_or(true));
    assert!(disk.top("Games").unwrap()[0]["LastPlayed"].is_string(), "play time stamped");

    let _ = reg::delete_tree(Hive::Hkcu, &scratch);
    let _ = reg::delete_key_if_empty(Hive::Hkcu, &format!("Software{sep}MyTuneTest"));
}
