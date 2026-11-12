//! Real-system tests: the actual sc.exe and bcdedit.exe, on disposable targets only.
//! Needs an elevated shell, so they are #[ignore]d: `cargo test --test real_system -- --ignored --test-threads=1`.
//!
//! - Services: a dummy service `MyTuneTestSvc` created for the test and deleted afterwards.
//! - Boot: a throwaway COPY of the current boot entry (`bcdedit /copy`), deleted afterwards.
//!   The real entry, the real DeviceGuard keys, the power plan and real services are never touched.

use mytune_core::engine::{Engine, RevertOutcome, Tweak};
use mytune_core::process::{run, system32};
use mytune_core::reg::{self, Hive, RegValue};
use mytune_core::store::Store;
use mytune_core::tweaks::{system_cmd, ServicesManualTweak, TimerTickTweak, VbsTweak};

fn exe(n: &str) -> String {
    // every test calls this first: keep the log and state out of the real %ProgramData%\MyTune
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| std::env::set_var("MYTUNE_DIR", std::env::temp_dir().join("mytune-real-system-test")));
    system32(n).to_string_lossy().into_owned()
}

fn engine() -> (tempfile::TempDir, Engine) {
    let d = tempfile::tempdir().unwrap();
    let e = Engine::new(Store::load(d.path()));
    (d, e)
}

// ------------------------------------------------------------------ service

struct DummyService(&'static str);
impl DummyService {
    fn create(name: &'static str) -> DummyService {
        let _ = run(&exe("sc.exe"), &["delete", name]);
        let r = run(&exe("sc.exe"), &["create", name, "binPath=", "C:\\Windows\\System32\\cmd.exe", "start=", "auto"]).unwrap();
        assert_eq!(r.exit_code, 0, "needs admin: {}", r.output);
        DummyService(name)
    }
    fn start(&self) -> Option<RegValue> {
        reg::get(Hive::Hklm, &format!("SYSTEM\\CurrentControlSet\\Services\\{}", self.0), "Start").unwrap()
    }
}
impl Drop for DummyService {
    fn drop(&mut self) {
        let _ = run(&exe("sc.exe"), &["delete", self.0]);
    }
}

#[test]
#[ignore]
fn real_sc_services_apply_and_revert() {
    let svc = DummyService::create("MyTuneTestSvc");
    assert_eq!(svc.start(), Some(RegValue::Dword(2)));
    let t = ServicesManualTweak::system(system_cmd(), "test.svc", "Dummy service", &["MyTuneTestSvc", "NoSuchSvcXyz"]);
    let (_d, e) = engine();
    e.apply(&t).unwrap();
    assert_eq!(svc.start(), Some(RegValue::Dword(3)), "sc config demand worked");
    assert!(t.is_applied().unwrap());
    assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted);
    assert_eq!(svc.start(), Some(RegValue::Dword(2)), "back to auto-start");
    // second revert: no backup -> nothing changes
    assert_eq!(e.revert(&t).unwrap(), RevertOutcome::NoBackup);
}

// ------------------------------------------------------------------ boot entry copy

struct BootCopy(String);
impl BootCopy {
    fn create() -> BootCopy {
        let desc = format!("MyTuneTest-{}", std::process::id());
        let r = run(&exe("bcdedit.exe"), &["/copy", "{current}", "/d", &desc]).unwrap();
        assert_eq!(r.exit_code, 0, "needs admin: {}", r.output);
        let start = r.output.rfind('{').expect("guid in output");
        let end = r.output[start..].find('}').unwrap() + start;
        BootCopy(r.output[start..=end].to_string())
    }
    fn value(&self, name: &str) -> String {
        let r = run(&exe("bcdedit.exe"), &["/enum", &self.0]).unwrap();
        assert_eq!(r.exit_code, 0, "{}", r.output);
        r.output
            .lines()
            .find_map(|l| {
                let mut it = l.split_whitespace();
                (it.next().is_some_and(|k| k.eq_ignore_ascii_case(name))).then(|| it.next().unwrap_or("").to_string())
            })
            .unwrap_or_default()
    }
    fn set(&self, name: &str, v: &str) {
        assert_eq!(run(&exe("bcdedit.exe"), &["/set", &self.0, name, v]).unwrap().exit_code, 0);
    }
    fn delete(&self, name: &str) {
        let _ = run(&exe("bcdedit.exe"), &["/deletevalue", &self.0, name]);
    }
}
impl Drop for BootCopy {
    fn drop(&mut self) {
        let _ = run(&exe("bcdedit.exe"), &["/delete", &self.0]);
    }
}

fn copy_is_gone(guid: &str) -> bool {
    let all = run(&exe("bcdedit.exe"), &["/enum", "all"]).unwrap();
    !all.output.to_ascii_lowercase().contains(&guid.to_ascii_lowercase())
}

fn vbs_on(entry: &BootCopy, scratch: &str) -> VbsTweak {
    // DeviceGuard keys point at a throwaway HKCU key; only the bcdedit part runs for real.
    VbsTweak {
        cmd: system_cmd(),
        entry: Some(entry.0.clone()),
        hive: Hive::Hkcu,
        device_guard: format!("{scratch}\\DG"),
        hvci: format!("{scratch}\\DG\\Hvci"),
    }
}

#[test]
#[ignore]
fn real_bcdedit_vbs_and_tick_on_a_copy() {
    let scratch = format!("Software\\MyTuneTest\\real-{}", std::process::id());
    let guid;
    {
        let copy = BootCopy::create();
        guid = copy.0.clone();
        let (_d, e) = engine();

        // 1) hypervisorlaunchtype had a value (Auto): apply -> Off, revert -> Auto
        copy.set("hypervisorlaunchtype", "auto");
        let vbs = vbs_on(&copy, &scratch);
        e.apply(&vbs).unwrap();
        assert_eq!(copy.value("hypervisorlaunchtype").to_lowercase(), "off");
        assert!(vbs.is_applied().unwrap());
        assert_eq!(e.revert(&vbs).unwrap(), RevertOutcome::Reverted);
        assert_eq!(copy.value("hypervisorlaunchtype").to_lowercase(), "auto");

        // 2) value was ABSENT: revert must delete it again (the case where C# gets stuck)
        copy.delete("hypervisorlaunchtype");
        assert_eq!(copy.value("hypervisorlaunchtype"), "");
        e.apply(&vbs).unwrap();
        assert_eq!(copy.value("hypervisorlaunchtype").to_lowercase(), "off");
        assert_eq!(e.revert(&vbs).unwrap(), RevertOutcome::Reverted);
        assert_eq!(copy.value("hypervisorlaunchtype"), "", "removed again, backup consumed");

        // 3) dynamic tick + HPET: originals Yes/No are restored, absent ones are removed
        copy.set("useplatformclock", "yes");
        copy.delete("disabledynamictick");
        let tick = TimerTickTweak { cmd: system_cmd(), entry: Some(copy.0.clone()) };
        e.apply(&tick).unwrap();
        assert_eq!(copy.value("disabledynamictick").to_lowercase(), "yes");
        assert_eq!(copy.value("useplatformclock"), "", "HPET forced off");
        assert!(tick.is_applied().unwrap());
        assert_eq!(e.revert(&tick).unwrap(), RevertOutcome::Reverted);
        assert_eq!(copy.value("disabledynamictick"), "", "was absent -> removed");
        assert_eq!(copy.value("useplatformclock").to_lowercase(), "yes", "HPET restored");
    } // copy dropped here -> entry deleted
    assert!(copy_is_gone(&guid), "throwaway boot entry {guid} must be deleted");
    let _ = reg::delete_tree(Hive::Hkcu, &scratch);
    let _ = reg::delete_key_if_empty(Hive::Hkcu, "Software\\MyTuneTest");
    let _ = RegValue::Dword(0); // keep import used in all cfgs
}

// ------------------------------------------------------------------ scheduled tasks (COM) on a dummy task

struct DummyTask(&'static str);
impl Drop for DummyTask {
    fn drop(&mut self) {
        let _ = run(&exe("schtasks.exe"), &["/delete", "/tn", self.0, "/f"]);
    }
}

#[test]
#[ignore]
fn real_task_scheduler_com_on_a_dummy_task() {
    use mytune_core::special::{ComTaskApi, ScheduledTasksTweak, TaskApi};
    use std::sync::Arc;
    let name = "MyTuneTestDummy";
    let _ = run(&exe("schtasks.exe"), &["/delete", "/tn", name, "/f"]);
    let r = run(&exe("schtasks.exe"), &["/create", "/tn", name, "/tr", "cmd /c exit", "/sc", "once", "/st", "23:59", "/f"]).unwrap();
    assert_eq!(r.exit_code, 0, "{}", r.output);
    let _guard = DummyTask(name);
    let path = format!(r"\{name}");
    let api = ComTaskApi;
    assert_eq!(api.is_enabled(&path).unwrap(), Some(true));
    assert_eq!(api.is_enabled(r"\MyTuneNoSuchTask").unwrap(), None);
    assert_eq!(api.is_enabled(r"\No\Such\Folder\Task").unwrap(), None);

    let t = ScheduledTasksTweak { api: Arc::new(ComTaskApi), id: "test.tasks".into(), title: "dummy".into(), tasks: vec![path.clone(), r"\MyTuneNoSuchTask".into()] };
    let (_d, e) = engine();
    assert!(t.unavailable().unwrap().is_none());
    e.apply(&t).unwrap();
    assert_eq!(api.is_enabled(&path).unwrap(), Some(false));
    assert!(t.is_applied().unwrap());
    assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted);
    assert_eq!(api.is_enabled(&path).unwrap(), Some(true));
}

// ------------------------------------------------------------------ read-only checks of PowerShell output

#[test]
#[ignore]
fn real_powershell_reads_parse() {
    use mytune_core::special::{active_adapters, adapter_tweaks, msi_tweak, DirectXTweak, ProtocolBindingsTweak, ScheduledTasksTweak, ComTaskApi};
    use mytune_core::catalog::SystemHooks;
    use std::sync::Arc;
    let _ = exe("powershell.exe"); // sets MYTUNE_DIR before anything logs
    let cmd = system_cmd();
    let adapters = active_adapters(&*cmd).unwrap();
    println!("adapters: {adapters:?}");
    for t in adapter_tweaks(cmd.clone()) {
        println!("{}: applied={:?} unavailable={:?}", t.id, t.is_applied().unwrap(), t.unavailable().unwrap());
    }
    println!("bindings applied={:?}", ProtocolBindingsTweak { cmd: cmd.clone() }.is_applied().unwrap());
    let msi = msi_tweak(&*cmd, Arc::new(SystemHooks)).unwrap();
    println!("msi devices: {} applied={:?}", msi.sets.len(), msi.is_applied().unwrap());
    assert!(!msi.sets.is_empty(), "a PC has at least one PCI display or network device");
    for d in DirectXTweak::catalog() {
        println!("{} applied={:?}", d.id, d.is_applied().unwrap());
    }
    let tasks = ScheduledTasksTweak::telemetry(Arc::new(ComTaskApi));
    println!("telemetry tasks: unavailable={:?} applied={:?}", tasks.unavailable().unwrap(), tasks.is_applied().unwrap());
}
