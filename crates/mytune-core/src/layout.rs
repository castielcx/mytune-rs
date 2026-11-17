//! Which page and heading each catalog tweak is shown under (C# `Tweaks/Layout.cs`).
//! Optimize = performance, Debloat = privacy and clutter, Customize = how Windows looks and behaves.
//! The catalog dump has no page info, so the table lives here; a test checks that every active catalog
//! tweak is placed or retired, like C# `Layout.CheckAllPlaced`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    General,
    Advanced,
    Network,
    Debloat,
    Customize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub section: Section,
    pub group: &'static str,
    /// Position over all pages, for sorting.
    pub order: usize,
}

type Group = (&'static str, &'static [&'static str]);

const GENERAL: &[Group] = &[
    ("Games", &["general.gamebar", "adv.windowed", "game.vrr", "game.autohdr", "general.nvidia", "general.gsync"]),
    ("CPU and scheduling", &["general.powerplan", "adv.timer"]),
    ("Input devices", &["sys.usbsuspend"]),
];
const ADVANCED: &[Group] = &[
    ("Graphics and interrupts", &["adv.hags", "adv.mpo", "adv.msi"]),
    ("Storage and services", &["adv.storagesense", "adv.hibernation", "sys.longpaths"]),
    ("Windows Update", &["general.windowsupdate", "general.storeupdate", "sys.wudrivers", "sys.norestart", "sys.deliveryopt"]),
    ("Startup and shutdown", &["sys.startupdelay", "sys.shutdown"]),
    ("Security trade-offs", &["adv.vbs"]),
];
const NETWORK: &[Group] = &[("", &["net.adapterpower", "net.adapteradvanced", "net.bindings"])];
const DEBLOAT: &[Group] = &[
    ("Privacy and telemetry", &["general.telemetry", "deb.tasks", "deb.adid", "deb.activity", "deb.tailored", "deb.feedback", "deb.wer"]),
    ("Ads and suggestions", &["deb.ads", "adv.copilot"]),
    ("Background apps and services", &["adv.backgroundapps", "deb.xbox"]),
    ("Browsers", &["adv.edge", "adv.brave"]),
    ("Windows clutter", &["deb.chat"]),
    ("Aggressive, use with care", &["deb.spooler"]),
];
const CUSTOMIZE: &[Group] = &[
    ("Appearance", &["cust.dark", "general.display", "cust.dragoutline", "sys.menudelay", "qol.hover"]),
    ("Taskbar", &["cust.taskbarleft", "cust.hidesearch", "cust.hidetaskview", "adv.notifications", "qol.seconds"]),
    ("Start and search", &["cust.bing", "cust.startrecs", "qol.searchhighlights"]),
    ("File Explorer", &["cust.fileext", "cust.hiddenfiles", "qol.thispc", "qol.navpane"]),
    ("Right-click menu", &["adv.classicmenu", "qol.cleanmenu", "qol.takeown"]),
    ("Window snapping", &["cust.snapoff", "cust.snapflyout", "cust.snapassist"]),
    ("Keyboard", &["cust.numlock", "sys.keyboard", "qol.stickykeys"]),
    ("Windows behavior", &["cust.verbose", "cust.crashdetails", "cust.settingshome"]),
    ("Browsers", &["cust.adblock"]),
];

const PAGES: [(Section, &[Group]); 5] =
    [(Section::General, GENERAL), (Section::Advanced, ADVANCED), (Section::Network, NETWORK), (Section::Debloat, DEBLOAT), (Section::Customize, CUSTOMIZE)];

/// Tweaks no longer offered on any page; they stay in the catalog so an old setup can still revert them.
pub const RETIRED: [&str; 3] = ["deb.onedrive", "deb.location", "deb.store"];

pub fn is_retired(id: &str) -> bool {
    RETIRED.contains(&id)
}

pub fn placement(id: &str) -> Option<Placed> {
    let mut order = 0;
    for (section, groups) in PAGES {
        for (group, ids) in groups {
            for i in *ids {
                if *i == id {
                    return Some(Placed { section, group, order });
                }
                order += 1;
            }
        }
    }
    None
}

/// What the tweak is for. Debloat shows none, Customize defaults to "Cosmetic".
pub fn impact(id: &str) -> &'static str {
    let Some(p) = placement(id) else { return "" };
    if p.section == Section::Debloat {
        return "";
    }
    match id {
        "general.gamebar" | "general.powerplan" | "general.nvidia" | "general.display" | "adv.hags" | "adv.vbs" => "FPS",
        "general.gsync" | "adv.timer" | "adv.windowed" | "adv.msi" | "net.adapteradvanced" | "sys.usbsuspend" | "sys.keyboard" => "Latency",
        "general.telemetry" | "adv.copilot" | "adv.edge" | "adv.brave" => "Privacy",
        "general.windowsupdate" | "general.storeupdate" | "sys.wudrivers" | "sys.norestart" => "Updates",
        "adv.backgroundapps" | "adv.storagesense" | "adv.hibernation" => "Cleanup",
        "adv.mpo" => "Stability",
        "net.adapterpower" | "net.bindings" | "sys.deliveryopt" => "Network",
        "game.vrr" | "game.autohdr" => "Display",
        "sys.shutdown" | "sys.startupdelay" => "Startup",
        "sys.longpaths" => "Compatibility",
        "sys.menudelay" | "qol.hover" => "Cosmetic",
        _ if p.section == Section::Customize => "Cosmetic",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_active_catalog_tweak_is_placed_or_retired() {
        let mut missing = vec![];
        for e in crate::catalog::entries().unwrap().into_iter().filter(|e| !e.removed) {
            if placement(&e.id).is_none() && !is_retired(&e.id) {
                missing.push(e.id);
            }
        }
        assert!(missing.is_empty(), "not on any page: {missing:?}");
    }

    #[test]
    fn no_id_is_placed_twice_or_unknown() {
        let known: Vec<String> = crate::catalog::entries().unwrap().into_iter().map(|e| e.id).collect();
        let mut seen = std::collections::HashSet::new();
        for (_, groups) in PAGES {
            for (_, ids) in groups {
                for id in *ids {
                    assert!(seen.insert(*id), "{id} placed twice");
                    assert!(known.iter().any(|k| k == id), "{id} not in the catalog");
                }
            }
        }
        assert_eq!(seen.len(), 71);
    }

    #[test]
    fn spot_checks_match_csharp() {
        assert_eq!(placement("general.telemetry").unwrap().section, Section::Debloat);
        assert_eq!(placement("qol.seconds").unwrap().group, "Taskbar");
        assert_eq!(placement("game.vrr").unwrap().section, Section::General);
        assert_eq!(impact("general.telemetry"), "");
        assert_eq!(impact("adv.hags"), "FPS");
        assert_eq!(impact("cust.dark"), "Cosmetic");
        assert!(placement("deb.onedrive").is_none() && is_retired("deb.onedrive"));
    }
}

/// (group, id) of every tweak of a page in display order (for the system report).
pub fn ids(section: Section) -> Vec<(&'static str, &'static str)> {
    PAGES.iter().filter(|(s, _)| *s == section).flat_map(|(_, groups)| groups.iter().flat_map(|(g, ids)| ids.iter().map(move |i| (*g, *i)))).collect()
}
