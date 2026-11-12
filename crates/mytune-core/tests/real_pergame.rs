//! Real-system checks for the per-game tweaks. Needs an elevated shell, so #[ignore]d:
//! `cargo test --test real_pergame -- --ignored --nocapture --test-threads=1`
//!
//! - QoS: creates a real policy `MyTune - MyTuneTestGame.exe` and removes it again. The "Do not use NLA" value is
//!   aimed at a throwaway HKCU key, never at the real HKLM Tcpip\QoS key.
//! - Defender: READ ONLY. Adding even a temporary exclusion would change a security setting, so apply/revert is
//!   covered by the fake-runner unit tests only.

use mytune_core::engine::{Engine, RevertOutcome, Tweak};
use mytune_core::process::{run, system32};
use mytune_core::reg::{self, Hive};
use mytune_core::store::Store;
use mytune_core::pergame::{DefenderExclusionTweak, QosPolicyTweak};
use mytune_core::special::powershell;
use mytune_core::tweaks::system_cmd;

fn init() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| std::env::set_var("MYTUNE_DIR", std::env::temp_dir().join("mytune-real-pergame-test")));
}

const POLICY: &str = "MyTune - MyTuneTestGame.exe";

struct PolicyGuard;
impl Drop for PolicyGuard {
    fn drop(&mut self) {
        let _ = powershell(&*system_cmd(), &format!("Remove-NetQosPolicy -Name '{POLICY}' -Confirm:$false -ErrorAction SilentlyContinue"));
    }
}

fn policy_count(cmd: &dyn mytune_core::tweaks::Cmd, filter: &str) -> usize {
    powershell(cmd, &format!("@(Get-NetQosPolicy -ErrorAction SilentlyContinue | Where-Object Name -like '{filter}').Count")).unwrap().trim().parse().unwrap()
}

#[test]
#[ignore]
fn real_qos_policy_create_and_remove() {
    init();
    let cmd = system_cmd();
    let _guard = PolicyGuard;
    let sep = '\\';
    let scratch = format!("Software{sep}MyTuneTest{sep}qos-{}", std::process::id());
    let _ = reg::delete_tree(Hive::Hkcu, &scratch);
    let others_before = policy_count(&*cmd, "MyTune - *");

    let mut t = QosPolicyTweak::new(cmd.clone(), "test", "MyTuneTestGame", "MyTuneTestGame.exe").unwrap();
    t.hive = Hive::Hkcu;
    t.qos_key = scratch.clone();
    let d = tempfile::tempdir().unwrap();
    let e = Engine::new(Store::load(d.path()));

    assert!(!t.is_applied().unwrap());
    e.apply(&t).unwrap();
    assert!(t.is_applied().unwrap(), "policy exists");
    assert_eq!(policy_count(&*cmd, POLICY), 1);
    let dscp = powershell(&*cmd, &format!("(Get-NetQosPolicy -Name '{POLICY}').DSCPAction")).unwrap();
    assert_eq!(dscp.trim(), "46");
    let app = powershell(&*cmd, &format!("(Get-NetQosPolicy -Name '{POLICY}').AppPathNameMatchCondition")).unwrap();
    assert_eq!(app.trim(), "MyTuneTestGame.exe");

    assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted);
    assert!(!t.is_applied().unwrap());
    assert_eq!(policy_count(&*cmd, POLICY), 0, "policy removed");
    if others_before == 0 {
        assert_eq!(reg::get(Hive::Hkcu, &scratch, "Do not use NLA").unwrap(), None, "NLA value back to absent");
    } else {
        println!("{others_before} other MyTune QoS policies exist: NLA value correctly left alone");
    }
    assert_eq!(policy_count(&*cmd, "MyTune - *"), others_before, "your own policies untouched");
    let _ = reg::delete_tree(Hive::Hkcu, &scratch);
    let _ = reg::delete_key_if_empty(Hive::Hkcu, &format!("Software{sep}MyTuneTest"));
    let _ = (run, system32); // keep imports used
}

#[test]
#[ignore]
fn real_defender_read_only() {
    init();
    let cmd = system_cmd();
    let t = DefenderExclusionTweak::new(cmd.clone(), "test", "T", "C:\\MyTuneTestNoSuchFolder").unwrap();
    assert!(!t.is_applied().unwrap(), "a made-up folder is not excluded");
    let listed = powershell(&*cmd, "@((Get-MpPreference).ExclusionPath).Count").unwrap();
    println!("Defender exclusion paths on this PC: {}", listed.trim());
}
