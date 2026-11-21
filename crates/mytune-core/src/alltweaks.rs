//! One list of every catalog tweak as a ready object (C# `Catalog.All` + `Catalog.Removed`), so the UI can
//! show, apply and revert the non-registry ones (power plan, NVIDIA, hibernation, VBS, DirectX preferences,
//! scheduled tasks, services, MSI mode, network) the same way as the plain registry tweaks.
//!
//! The object's id and title are the catalog's, whatever the inner struct calls itself: the C# app stores its
//! backups under the catalog id (e.g. `general.powerplan`, not the old `adv.power`).
//! Differences from C#:
//! - `adv.timer` is "applied" when its registry value is set; C# also needs `Settings.GameTimer` (the tweak
//!   sets and clears that flag when applied or reverted, like C#).
//! - NVIDIA tweaks are left out on a PC without an NVIDIA driver.
//! - Building the list runs one PowerShell command (the MSI device list), so call it off the UI thread.

use crate::backup::TweakBackup;
use crate::catalog::{self, data_tweak, DataTweak, Entry, Hooks, SystemHooks, TweakFlags};
use crate::engine::Tweak;
use crate::error::{Error, Result};
use crate::nvidia::{self, NvidiaGlobalProfileTweak, NvidiaGsyncTweak};
use crate::pccheck::on_battery_capable;
use crate::special::{adapter_tweaks, msi_tweak, msi_without_devices, removed_apply, service_tweaks, AdBlockTweak, CleanStartTweak, ComTaskApi, DirectXTweak, DnsRevert, MemoryCompressionRevert, ProtocolBindingsTweak, ScheduledTasksTweak, TakeOwnTweak, TcpStackRevert};
use crate::store::Store;
use crate::tweaks::{Cmd, HibernationTweak, PowerPlanTweak, TimerTickTweak, VbsTweak};
use std::collections::HashMap;
use std::sync::Arc;

type Boxed = Box<dyn Tweak + Send + Sync>;

/// A catalog tweak: its text and flags from the catalog, its behaviour from the inner object.
pub struct TweakItem {
    pub id: String,
    pub title: String,
    pub description: String,
    pub details: String,
    pub flags: TweakFlags,
    /// Removed from MyTune: it can only be reverted (Restore page).
    pub removed: bool,
    inner: Boxed,
}

impl Tweak for TweakItem {
    fn id(&self) -> &str {
        &self.id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn is_applied(&self) -> Result<bool> {
        // A removed tweak is never "on": an original that matches its value must not make a finished revert read as failed.
        if self.removed {
            return Ok(false);
        }
        self.inner.is_applied()
    }
    fn capture(&self, b: &mut TweakBackup) -> Result<()> {
        if self.removed {
            return Err(removed_apply(&self.id));
        }
        self.inner.capture(b)
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        if self.removed {
            return Err(removed_apply(&self.id));
        }
        self.inner.apply(b)
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        self.inner.revert(b)
    }
    fn revert_default(&self) -> Result<bool> {
        self.inner.revert_default()
    }
    fn on_applied(&self, store: &mut Store) -> Result<()> {
        self.inner.on_applied(store)
    }
    fn on_reverted(&self, store: &mut Store) -> Result<()> {
        self.inner.on_reverted(store)
    }
}

/// `adv.timer`: registry value plus the `Settings.GameTimer` flag the game booster reads.
struct TimerResolution(DataTweak);

impl Tweak for TimerResolution {
    fn id(&self) -> &str {
        self.0.id()
    }
    fn title(&self) -> &str {
        self.0.title()
    }
    fn is_applied(&self) -> Result<bool> {
        self.0.is_applied()
    }
    fn capture(&self, b: &mut TweakBackup) -> Result<()> {
        self.0.capture(b)
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        self.0.apply(b)?;
        b.data.insert("enabled".into(), "1".into());
        Ok(())
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        self.0.revert(b)
    }
    fn revert_default(&self) -> Result<bool> {
        self.0.revert_default()
    }
    fn on_applied(&self, store: &mut Store) -> Result<()> {
        store.set_settings_flag("GameTimer", true)
    }
    fn on_reverted(&self, store: &mut Store) -> Result<()> {
        store.set_settings_flag("GameTimer", false)
    }
}

/// Every catalog entry that has an implementation, in catalog order.
pub fn build(cmd: Arc<dyn Cmd>) -> Result<Vec<TweakItem>> {
    let hooks: Arc<dyn Hooks> = Arc::new(SystemHooks);
    let mut by_id: HashMap<String, Boxed> = HashMap::new();
    let mut put = |id: &str, t: Boxed| {
        by_id.insert(id.to_string(), t);
    };

    let entries: Vec<Entry> = catalog::entries()?;
    for e in &entries {
        if e.cls == "RegistryTweak" && !e.provider {
            put(&e.id, Box::new(data_tweak(e.clone(), hooks.clone())?));
        } else if e.cls == "TimerResolutionTweak" {
            put(&e.id, Box::new(TimerResolution(data_tweak(e.clone(), hooks.clone())?)));
        } else if e.cls == "CleanStartTweak" {
            put(&e.id, Box::new(CleanStartTweak::new(data_tweak(e.clone(), hooks.clone())?, cmd.clone())));
        }
    }
    // Without the PCI device list MSI mode changes nothing, but a stored backup must still revert.
    let msi = msi_tweak(&*cmd, hooks.clone()).unwrap_or_else(|e| {
        crate::log::error("MSI device list not read", Some(&e));
        msi_without_devices(hooks.clone())
    });
    put("adv.msi", Box::new(msi));
    put("general.powerplan", Box::new(PowerPlanTweak { cmd: cmd.clone(), laptop: on_battery_capable() }));
    put("cust.adblock", Box::new(AdBlockTweak));
    put("qol.takeown", Box::new(TakeOwnTweak));
    // Owner's build only: without the feature the catalog entry has no object and no row.
    #[cfg(feature = "taskbar-look")]
    put("cust.clearstart", Box::new(crate::taskbar::ClearStartTweak));
    #[cfg(feature = "taskbar-look")]
    put("cust.outlineselect", Box::new(crate::taskbar::OutlineSelectTweak));
    put(crate::selcolor::ID, Box::new(crate::selcolor::SelectionColorTweak));
    put(crate::searchstats::ID, Box::new(crate::searchstats::SearchStatsTweak));
    put("adv.hibernation",Box::new(HibernationTweak::system(cmd.clone())));
    put("adv.vbs", Box::new(VbsTweak::system(cmd.clone())));
    for t in DirectXTweak::catalog() {
        let id = t.id.clone();
        put(&id, Box::new(t));
    }
    for t in adapter_tweaks(cmd.clone()) {
        let id = t.id.clone();
        put(&id, Box::new(t));
    }
    put("net.bindings", Box::new(ProtocolBindingsTweak { cmd: cmd.clone() }));
    put("deb.tasks", Box::new(ScheduledTasksTweak::telemetry(Arc::new(ComTaskApi))));
    for t in service_tweaks(cmd.clone()) {
        use crate::engine::Tweak as _;
        let id = t.id().to_string();
        put(&id, Box::new(t));
    }
    put("adv.tick", Box::new(TimerTickTweak { cmd: cmd.clone(), entry: None }));
    put("net.dns", Box::new(DnsRevert { cmd: cmd.clone() }));
    put("net.tcp", Box::new(TcpStackRevert { cmd: cmd.clone() }));
    put("mem.compression", Box::new(MemoryCompressionRevert { cmd: cmd.clone() }));
    if nvidia::available() {
        put("general.nvidia", Box::new(NvidiaGlobalProfileTweak::base()));
        put("general.gsync", Box::new(NvidiaGsyncTweak::base()));
    }

    let mut out = Vec::new();
    for e in entries {
        let Some(inner) = by_id.remove(&e.id) else { continue };
        out.push(item(e, inner)?);
    }
    Ok(out)
}

fn item(e: Entry, inner: Boxed) -> Result<TweakItem> {
    Ok(TweakItem {
        flags: TweakFlags::parse(&e.flags)?,
        id: e.id,
        title: e.title,
        description: e.description.unwrap_or_default(),
        details: e.details.unwrap_or_default(),
        removed: e.removed,
        inner,
    })
}

/// The registry-backed tweaks, for pages that build their list on the UI thread: unlike `build`, this runs no command.
pub fn registry_items() -> Result<Vec<TweakItem>> {
    let hooks: Arc<dyn Hooks> = Arc::new(SystemHooks);
    let mut out = Vec::new();
    for e in catalog::entries()? {
        let inner: Boxed = if e.cls == "RegistryTweak" && !e.provider {
            Box::new(data_tweak(e.clone(), hooks.clone())?)
        } else if e.cls == "TakeOwnTweak" {
            Box::new(TakeOwnTweak)
        } else {
            continue;
        };
        out.push(item(e, inner)?);
    }
    Ok(out)
}

/// Not found on this build: a clear error instead of a panic.
pub fn find<'a>(items: &'a [TweakItem], id: &str) -> Result<&'a TweakItem> {
    items.iter().find(|t| t.id == id).ok_or_else(|| Error::Catalog(format!("unknown tweak {id}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Result as R;
    use crate::process::RunResult;

    struct Quiet;
    impl Cmd for Quiet {
        fn run(&self, _f: &str, _a: &[&str]) -> R<RunResult> {
            Ok(RunResult { exit_code: 0, output: String::new() })
        }
    }

    #[test]
    fn every_catalog_entry_has_an_object_with_the_catalog_id() {
        let items = build(Arc::new(Quiet)).unwrap();
        let entries = catalog::entries().unwrap();
        let mut missing: Vec<&str> = entries.iter().map(|e| e.id.as_str()).filter(|id| !items.iter().any(|t| t.id == *id)).collect();
        if !nvidia::available() {
            missing.retain(|id| !["general.nvidia", "general.gsync"].contains(id));
        }
        if !cfg!(feature = "taskbar-look") {
            missing.retain(|id| !["cust.clearstart", "cust.outlineselect"].contains(id));
        }
        // adv.msi has no PCI devices with a silent PowerShell, but it is still built (an empty list of sets)
        assert!(missing.is_empty(), "no object for {missing:?}");
        let alias = |id: &str| items.iter().find(|t| t.id == id).map(|t| t.title.clone());
        assert_eq!(alias("general.powerplan").as_deref(), Some("High-performance power plan"));
        assert!(alias("adv.hibernation").is_some(), "hibernation uses the catalog id, not deb.hibernate");
        assert!(items.iter().filter(|t| t.removed).count() >= 16, "removed tweaks stay available for Restore");
    }

    #[test]
    fn registry_items_are_the_registry_tweaks_plus_take_ownership() {
        let items = registry_items().unwrap();
        let plain = catalog::registry_tweaks(Arc::new(SystemHooks)).unwrap();
        assert_eq!(items.len(), plain.len() + 1);
        assert!(plain.iter().all(|p| items.iter().any(|t| t.id == p.id)));
        assert!(items.iter().any(|t| t.id == "qol.takeown" && !t.removed));
    }

    #[test]
    fn timer_tweak_sets_and_clears_the_game_timer_flag() {
        let d = tempfile::tempdir().unwrap();
        let mut st = Store::load(d.path());
        let items = build(Arc::new(Quiet)).unwrap();
        let t = items.iter().find(|t| t.id == "adv.timer").unwrap();
        t.on_applied(&mut st).unwrap();
        assert_eq!(st.settings_flag("GameTimer"), Some(true));
        t.on_reverted(&mut st).unwrap();
        assert_eq!(st.settings_flag("GameTimer"), Some(false));
    }

    #[test]
    fn removed_tweaks_refuse_to_apply() {
        let items = build(Arc::new(Quiet)).unwrap();
        let dns = items.iter().find(|t| t.id == "net.dns").unwrap();
        assert!(dns.apply(&mut TweakBackup::new("x")).is_err());
        assert!(!dns.is_applied().unwrap());
        // every removed tweak, also those whose inner object still has working apply code
        for t in items.iter().filter(|t| t.removed) {
            let mut b = TweakBackup::new("x");
            assert!(t.capture(&mut b).is_err() && t.apply(&mut b).is_err(), "{}", t.id);
            assert!(b.is_empty(), "{}", t.id);
            assert!(!t.is_applied().unwrap(), "{}", t.id);
        }
    }

    #[test]
    fn msi_is_built_without_a_device_list_and_reverts_from_its_backup() {
        struct Fails;
        impl Cmd for Fails {
            fn run(&self, _f: &str, _a: &[&str]) -> R<RunResult> {
                Ok(RunResult { exit_code: 1, output: "failed".into() })
            }
        }
        let items = build(Arc::new(Fails)).unwrap();
        let msi = items.iter().find(|t| t.id == "adv.msi").expect("adv.msi stays in the list");
        assert!(!msi.is_applied().unwrap());
        msi.revert(&TweakBackup::new("x")).unwrap();
    }
}
