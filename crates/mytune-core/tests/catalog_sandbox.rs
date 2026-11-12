//! Applies and reverts EVERY plain registry tweak of the C# catalog (66) against a sandbox registry.
//!
//! `RegOverridePredefKey` redirects HKLM, HKCU and HKU of THIS PROCESS to scratch keys under
//! `HKCU\Software\MyTuneTest\sandbox-<pid>`, so the real registry is never touched. The process-wide
//! redirect is why this is its own test binary with a single test.

use mytune_core::catalog::{registry_tweaks, Hooks};
use mytune_core::engine::{Engine, RevertOutcome, Tweak};
use mytune_core::reg::{self, RegValue};
use mytune_core::store::Store;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use windows::core::PCWSTR;
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::*;

#[derive(Default)]
struct CountHooks {
    explorer: AtomicU32,
    broadcast: AtomicU32,
    drag: AtomicU32,
}
impl Hooks for CountHooks {
    fn broadcast_setting_change(&self) {
        self.broadcast.fetch_add(1, Ordering::SeqCst);
    }
    fn apply_drag_full_windows(&self) {
        self.drag.fetch_add(1, Ordering::SeqCst);
    }
    fn restart_explorer(&self) {
        self.explorer.fetch_add(1, Ordering::SeqCst);
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

struct Sandbox {
    base: String,
    roots: [(HKEY, HKEY); 3], // (predefined, scratch handle)
}

impl Sandbox {
    fn new() -> Sandbox {
        let base = format!("Software\\MyTuneTest\\sandbox-{}", std::process::id());
        let _ = reg::delete_tree(reg::Hive::Hkcu, &base);
        let mk = |name: &str| unsafe {
            let p = wide(&format!("{base}\\{name}"));
            let mut h = HKEY::default();
            let r = RegCreateKeyExW(HKEY_CURRENT_USER, PCWSTR(p.as_ptr()), None, PCWSTR::null(), REG_OPTION_NON_VOLATILE, KEY_ALL_ACCESS | KEY_WOW64_64KEY, None, &mut h, None);
            assert_eq!(r, ERROR_SUCCESS);
            h
        };
        let roots = [(HKEY_LOCAL_MACHINE, mk("hklm")), (HKEY_CURRENT_USER, mk("hkcu")), (HKEY_USERS, mk("hku"))];
        // HKCU last: the base keys above are already created against the real HKCU
        for (pre, h) in [roots[0], roots[2], roots[1]] {
            assert_eq!(unsafe { RegOverridePredefKey(pre, Some(h)) }, ERROR_SUCCESS);
        }
        Sandbox { base, roots }
    }

    /// Removes everything inside the three sandbox roots.
    fn clear(&self) {
        for (_, h) in self.roots {
            unsafe {
                let _ = RegDeleteTreeW(h, PCWSTR::null());
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.roots.iter().all(|(_, h)| {
            let (mut sub, mut val) = (0u32, 0u32);
            let r = unsafe { RegQueryInfoKeyW(*h, None, None, None, Some(&mut sub), None, None, Some(&mut val), None, None, None, None) };
            assert_eq!(r, ERROR_SUCCESS);
            sub == 0 && val == 0
        })
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        unsafe {
            for (pre, h) in self.roots {
                let _ = RegOverridePredefKey(pre, None);
                let _ = RegCloseKey(h);
            }
        }
        let _ = reg::delete_tree(reg::Hive::Hkcu, &self.base);
        let _ = reg::delete_key_if_empty(reg::Hive::Hkcu, "Software\\MyTuneTest");
    }
}

fn different(v: &RegValue) -> RegValue {
    match v {
        RegValue::Dword(_) => RegValue::Dword(7777),
        RegValue::Qword(_) => RegValue::Qword(7777),
        RegValue::String(_) => RegValue::String("original".into()),
        RegValue::ExpandString(_) => RegValue::ExpandString("%orig%".into()),
        RegValue::MultiString(_) => RegValue::MultiString(vec!["o".into()]),
        RegValue::Binary(_) => RegValue::Binary(vec![1, 2, 3]),
    }
}

#[test]
fn every_catalog_registry_tweak_applies_and_reverts_in_a_sandbox() {
    let tmp = tempfile::tempdir().unwrap();
    // before anything logs: keep the log and state out of the real %ProgramData%\MyTune
    std::env::set_var("MYTUNE_DIR", tmp.path());
    let sb = Sandbox::new();
    let hooks = Arc::new(CountHooks::default());
    let tweaks = registry_tweaks(hooks.clone()).unwrap();
    assert_eq!(tweaks.len(), 66);
    let engine = Engine::new(Store::load(tmp.path()));

    // ---- pass 1: no original values anywhere -> revert must leave the sandbox completely empty
    for t in &tweaks {
        engine.apply(t).unwrap_or_else(|e| panic!("{}: apply {e}", t.id));
        if !t.sets.is_empty() && t.sets.iter().any(|s| !s.only_if_key_exists) {
            assert!(t.is_applied().unwrap(), "{} not applied after apply", t.id);
        }
        engine.revert(t).unwrap_or_else(|e| panic!("{}: revert {e}", t.id));
        assert!(sb.is_empty(), "{}: sandbox not empty after revert (leftover values or keys)", t.id);
    }

    // ---- pass 2: originals exist (different values, keys exist) -> revert must put them back
    let mut checked = 0;
    for t in &tweaks {
        sb.clear();
        let mut originals = Vec::new();
        for s in &t.sets {
            let orig = different(&s.value);
            reg::set(s.hive, &s.path, &s.name, &orig).unwrap();
            originals.push((s, orig));
        }
        engine.apply(t).unwrap_or_else(|e| panic!("{}: apply {e}", t.id));
        if !t.sets.is_empty() {
            assert!(t.is_applied().unwrap(), "{} not applied", t.id);
            assert_eq!(engine.revert(t).unwrap(), RevertOutcome::Reverted, "{}", t.id);
            for (s, orig) in &originals {
                assert_eq!(reg::get(s.hive, &s.path, &s.name).unwrap().as_ref(), Some(orig), "{}: {}\\{} not restored", t.id, s.path, s.name);
                checked += 1;
            }
        } else {
            // Removed stub: nothing to apply, so nothing to revert
            assert_eq!(engine.revert(t).unwrap(), RevertOutcome::NoBackup, "{}", t.id);
        }
    }
    assert_eq!(checked, 122, "registry values verified");

    // hooks ran (RestartsExplorer tweaks restart Explorer, Customize ones broadcast, drag outline calls SPI)
    assert!(hooks.explorer.load(Ordering::SeqCst) > 0);
    assert!(hooks.broadcast.load(Ordering::SeqCst) > 0);
    assert!(hooks.drag.load(Ordering::SeqCst) >= 4, "apply+revert, both passes");
    sb.clear();
}
