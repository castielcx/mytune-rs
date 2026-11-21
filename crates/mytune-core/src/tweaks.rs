//! Ports of the C# tweaks that keep backup data outside plain registry values:
//! `ServicesManualTweak`, `PowerPlanTweak`, `HibernationTweak`, `VbsTweak`, `TimerTickTweak`.
//!
//! External programs go through the `Cmd` trait so tests run against a fake and never touch real
//! services, power plans or boot settings. Registry locations are fields, so tests point them at
//! `HKCU\Software\MyTuneTest`.
//!
//! Deliberate differences from C# (all make revert stricter or more honest, none weaker):
//! - bcdedit `/deletevalue` of a value that was absent before: a non-zero exit counts as success only
//!   if re-reading the entry shows the value really is gone (C# VbsTweak throws and keeps the backup
//!   forever; C# TimerTickTweak ignores any failure and drops the backup).
//! - TimerTick restore of a previous `disabledynamictick` value is checked (C# ignores failure).
//! - `ServicesManualTweak::is_applied` = "no installed service is still on auto-start (2)" and at least
//!   one is installed (C# also returns true just because a backup exists).
//! - `PowerPlanTweak`: laptop vs desktop is a constructor argument (`Sys.IsLaptop` is not ported).
//!   Revert falls back to Balanced, then High performance, when the saved previous plan is gone.

use crate::backup::TweakBackup;
use crate::engine::Tweak;
use crate::error::{Error, Result};
use crate::log;
use crate::process::{self, RunResult};
use crate::reg::{self, Hive, RegValue};
use crate::store::Store;
use serde_json::Value;
use std::sync::Arc;

/// Runs an external program. Real: `SystemCmd`. Tests: a fake.
pub trait Cmd: Send + Sync {
    fn run(&self, file: &str, args: &[&str]) -> Result<RunResult>;
}

pub struct SystemCmd;
impl Cmd for SystemCmd {
    fn run(&self, file: &str, args: &[&str]) -> Result<RunResult> {
        process::run(file, args)
    }
}

pub fn system_cmd() -> Arc<dyn Cmd> {
    Arc::new(SystemCmd)
}

fn exe(name: &str) -> String {
    process::system32(name).to_string_lossy().into_owned()
}

fn checked(cmd: &dyn Cmd, file: &str, args: &[&str]) -> Result<String> {
    let r = cmd.run(file, args)?;
    if r.exit_code == 0 {
        Ok(r.output)
    } else {
        Err(Error::ProcessFailed { cmd: format!("{file} {}", args.join(" ")), code: r.exit_code, output: r.output })
    }
}

fn dword(hive: Hive, path: &str, name: &str) -> Result<Option<u32>> {
    Ok(match reg::get(hive, path, name)? {
        Some(RegValue::Dword(d)) => Some(d),
        _ => None,
    })
}

// ---------------------------------------------------------------- bcdedit

fn bcdedit() -> String {
    exe("bcdedit.exe")
}

/// Backup key: the BCD entry this backup edited.
const BCD_ENTRY: &str = "bcdEntry";

/// The entry to edit: `entry` if given (tests use a throwaway copy), else `{current}`, else `{default}`.
/// bcdedit's errors are localized, so a readable `{default}` is what proves `{current}` is merely missing;
/// when both fail the store itself failed and nothing may be edited.
fn bcd_entry(cmd: &dyn Cmd, entry: &Option<String>) -> Result<String> {
    if let Some(e) = entry {
        return Ok(e.clone());
    }
    if cmd.run(&bcdedit(), &["/enum", "{current}"])?.exit_code == 0 {
        return Ok("{current}".into());
    }
    checked(cmd, &bcdedit(), &["/enum", "{default}"])?;
    Ok("{default}".into())
}

/// Real `{guid}` of an entry; `/v` prints it in place of aliases such as `{current}`.
fn bcd_guid(cmd: &dyn Cmd, entry: &str) -> Option<String> {
    let r = cmd.run(&bcdedit(), &["/enum", entry, "/v"]).ok().filter(|r| r.exit_code == 0)?;
    find_guid(&r.output).map(|g| format!("{{{g}}}"))
}

/// The entry this backup edits, stored on first use as its GUID: `{default}` can later point at another OS.
fn bcd_pin(cmd: &dyn Cmd, entry: &Option<String>, b: &mut TweakBackup) -> Result<String> {
    if let Some(e) = b.data.get(BCD_ENTRY) {
        return Ok(e.clone());
    }
    let live = bcd_entry(cmd, entry)?;
    let e = bcd_guid(cmd, &live).unwrap_or(live);
    b.data.insert(BCD_ENTRY.into(), e.clone());
    Ok(e)
}

/// Where revert writes: the stored entry; the live one for an old backup or a stored entry that no longer exists
/// (a Windows upgrade replaces it). An unreadable store is an error, so the backup is kept.
fn bcd_revert_entry(cmd: &dyn Cmd, entry: &Option<String>, b: &TweakBackup) -> Result<String> {
    if let Some(e) = b.data.get(BCD_ENTRY) {
        if cmd.run(&bcdedit(), &["/enum", e])?.exit_code == 0 {
            return Ok(e.clone());
        }
        log::info(&format!("bcd entry {e} no longer exists, using the live entry"));
    }
    bcd_entry(cmd, entry)
}

/// `hypervisorlaunchtype` from the mounted BCD store (element 250000f0: 0 = Off, 1 = Auto). bcdedit may print
/// localized values, which neither compare nor can be passed back to `/set`. `None`: use the text.
fn hypervisor_raw(cmd: &dyn Cmd, entry: &str) -> Option<&'static str> {
    let key = format!("BCD00000000\\Objects\\{}\\Elements\\250000f0", bcd_guid(cmd, entry)?);
    match reg::get(Hive::Hklm, &key, "Element").ok()?? {
        RegValue::Binary(v) if v.len() == 8 && v[1..].iter().all(|x| *x == 0) => match v[0] {
            0 => Some("Off"),
            1 => Some("Auto"),
            _ => None,
        },
        _ => None,
    }
}

/// Value of a bcdedit element: second word of the line that starts with `name`. Empty if absent.
fn parse_bcd_value(output: &str, name: &str) -> String {
    for line in output.lines() {
        let mut it = line.split_whitespace();
        if it.next().is_some_and(|k| k.eq_ignore_ascii_case(name)) {
            return it.next().unwrap_or("").to_string();
        }
    }
    String::new()
}

/// Lenient read for capturing originals (empty on any failure, like C#).
fn bcd_value(cmd: &dyn Cmd, entry: &str, name: &str) -> String {
    match cmd.run(&bcdedit(), &["/enum", entry]) {
        Ok(r) if r.exit_code == 0 => parse_bcd_value(&r.output, name),
        _ => String::new(),
    }
}

/// Read for storing an original: a failed read is an error, because a stored "" means "was absent".
fn bcd_value_strict(cmd: &dyn Cmd, entry: &str, name: &str) -> Result<String> {
    Ok(parse_bcd_value(&checked(cmd, &bcdedit(), &["/enum", entry])?, name))
}

/// Deletes a bcdedit element that did not exist before. A failing `/deletevalue` is fine only when a
/// successful re-read shows the element is gone.
fn bcd_delete(cmd: &dyn Cmd, entry: &str, name: &str) -> Result<()> {
    let r = cmd.run(&bcdedit(), &["/deletevalue", entry, name])?;
    if r.exit_code == 0 {
        return Ok(());
    }
    let e = cmd.run(&bcdedit(), &["/enum", entry])?;
    if e.exit_code == 0 && parse_bcd_value(&e.output, name).is_empty() {
        Ok(())
    } else {
        Err(Error::ProcessFailed { cmd: format!("bcdedit /deletevalue {entry} {name}"), code: r.exit_code, output: r.output })
    }
}

fn bcd_set(cmd: &dyn Cmd, entry: &str, name: &str, value: &str) -> Result<()> {
    checked(cmd, &bcdedit(), &["/set", entry, name, value]).map(|_| ())
}

// ---------------------------------------------------------------- TimerTick

pub struct TimerTickTweak {
    pub cmd: Arc<dyn Cmd>,
    /// bcdedit entry to edit; `None` = `{current}` / `{default}`.
    pub entry: Option<String>,
}

impl Tweak for TimerTickTweak {
    fn id(&self) -> &str {
        "adv.tick"
    }
    fn title(&self) -> &str {
        "Disable dynamic tick"
    }
    fn is_applied(&self) -> Result<bool> {
        let Ok(e) = bcd_entry(&*self.cmd, &self.entry) else { return Ok(false) };
        Ok(bcd_value(&*self.cmd, &e, "disabledynamictick").eq_ignore_ascii_case("yes")
            && !bcd_value(&*self.cmd, &e, "useplatformclock").eq_ignore_ascii_case("yes"))
    }
    fn capture(&self, b: &mut TweakBackup) -> Result<()> {
        let e = bcd_pin(&*self.cmd, &self.entry, b)?;
        for k in ["disabledynamictick", "useplatformclock"] {
            if !b.data.contains_key(k) {
                b.data.insert(k.into(), bcd_value_strict(&*self.cmd, &e, k)?);
            }
        }
        Ok(())
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        self.capture(b)?;
        let e = &*b.data[BCD_ENTRY].clone();
        bcd_set(&*self.cmd, e, "disabledynamictick", "yes")?;
        if !b.data["useplatformclock"].is_empty() {
            bcd_delete(&*self.cmd, e, "useplatformclock")?;
        }
        Ok(())
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        let e = &*bcd_revert_entry(&*self.cmd, &self.entry, b)?;
        let tick = b.data.get("disabledynamictick").map(String::as_str).unwrap_or("");
        let hpet = b.data.get("useplatformclock").map(String::as_str).unwrap_or("");
        if tick.is_empty() {
            bcd_delete(&*self.cmd, e, "disabledynamictick")?;
        } else {
            bcd_set(&*self.cmd, e, "disabledynamictick", tick)?;
        }
        if !hpet.is_empty() {
            bcd_set(&*self.cmd, e, "useplatformclock", hpet)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- VBS

pub struct VbsTweak {
    pub cmd: Arc<dyn Cmd>,
    /// bcdedit entry to edit; `None` = `{current}` / `{default}`.
    pub entry: Option<String>,
    pub hive: Hive,
    pub device_guard: String,
    pub hvci: String,
}

impl VbsTweak {
    pub fn system(cmd: Arc<dyn Cmd>) -> VbsTweak {
        VbsTweak {
            cmd,
            entry: None,
            hive: Hive::Hklm,
            device_guard: "SYSTEM\\CurrentControlSet\\Control\\DeviceGuard".into(),
            hvci: "SYSTEM\\CurrentControlSet\\Control\\DeviceGuard\\Scenarios\\HypervisorEnforcedCodeIntegrity".into(),
        }
    }
}

impl Tweak for VbsTweak {
    fn id(&self) -> &str {
        "adv.vbs"
    }
    fn title(&self) -> &str {
        "Turn off virtualization-based security"
    }
    fn is_applied(&self) -> Result<bool> {
        let Ok(e) = bcd_entry(&*self.cmd, &self.entry) else { return Ok(false) };
        let hypervisor = match hypervisor_raw(&*self.cmd, &e) {
            Some(v) => v.to_string(),
            None => bcd_value(&*self.cmd, &e, "hypervisorlaunchtype"),
        };
        Ok(dword(self.hive, &self.device_guard, "EnableVirtualizationBasedSecurity")? == Some(0)
            && dword(self.hive, &self.hvci, "Enabled")? != Some(1)
            && hypervisor.eq_ignore_ascii_case("off"))
    }
    fn capture(&self, b: &mut TweakBackup) -> Result<()> {
        let e = bcd_pin(&*self.cmd, &self.entry, b)?;
        if !b.data.contains_key("hypervisorlaunchtype") {
            let v = match hypervisor_raw(&*self.cmd, &e) {
                Some(v) => v.to_string(),
                None => bcd_value_strict(&*self.cmd, &e, "hypervisorlaunchtype")?,
            };
            b.data.insert("hypervisorlaunchtype".into(), v);
        }
        b.capture(self.hive, &self.device_guard, "EnableVirtualizationBasedSecurity")?;
        b.capture(self.hive, &self.hvci, "Enabled")
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        self.capture(b)?;
        let e = &*b.data[BCD_ENTRY].clone();
        reg::set(self.hive, &self.device_guard, "EnableVirtualizationBasedSecurity", &RegValue::Dword(0))?;
        reg::set(self.hive, &self.hvci, "Enabled", &RegValue::Dword(0))?;
        bcd_set(&*self.cmd, e, "hypervisorlaunchtype", "off")
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        // Registry first; if it fails the error is returned and bcdedit is not touched (same as C#).
        b.restore_registry()?;
        let e = &*bcd_revert_entry(&*self.cmd, &self.entry, b)?;
        match b.data.get("hypervisorlaunchtype").map(String::as_str).unwrap_or("") {
            "" => bcd_delete(&*self.cmd, e, "hypervisorlaunchtype"),
            prev => bcd_set(&*self.cmd, e, "hypervisorlaunchtype", prev),
        }
    }
}

// ---------------------------------------------------------------- Hibernation

pub struct HibernationTweak {
    pub cmd: Arc<dyn Cmd>,
    pub hive: Hive,
    pub path: String,
    /// Whether the firmware can hibernate at all (S4).
    pub supported: fn() -> bool,
}

impl HibernationTweak {
    pub fn system(cmd: Arc<dyn Cmd>) -> HibernationTweak {
        HibernationTweak { cmd, hive: Hive::Hklm, path: "SYSTEM\\CurrentControlSet\\Control\\Power".into(), supported: s4_supported }
    }
}

/// Stays true while hibernation is merely switched off. If the query fails, assume support so the backup is kept.
fn s4_supported() -> bool {
    use windows::Win32::System::Power::{GetPwrCapabilities, SYSTEM_POWER_CAPABILITIES};
    let mut c = SYSTEM_POWER_CAPABILITIES::default();
    !unsafe { GetPwrCapabilities(&mut c) } || c.SystemS4
}

impl Tweak for HibernationTweak {
    fn id(&self) -> &str {
        "adv.hibernation"
    }
    fn title(&self) -> &str {
        "Turn off hibernation"
    }
    fn is_applied(&self) -> Result<bool> {
        Ok(dword(self.hive, &self.path, "HibernateEnabled")? == Some(0))
    }
    fn capture(&self, b: &mut TweakBackup) -> Result<()> {
        if !b.data.contains_key("wasEnabled") {
            // An absent value means the Windows default applies; with no default it was never on.
            let on = match dword(self.hive, &self.path, "HibernateEnabled")? {
                Some(v) => v != 0,
                None => dword(self.hive, &self.path, "HibernateEnabledDefault")? == Some(1),
            };
            // "0" means hibernation was already off before MyTune
            b.data.insert("wasEnabled".into(), if on { "1" } else { "0" }.into());
            if let Some(t @ (1 | 2)) = dword(self.hive, &self.path, "HiberFileType")? {
                b.data.insert("hiberType".into(), t.to_string());
            }
        }
        Ok(())
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        self.capture(b)?;
        checked(&*self.cmd, &exe("powercfg.exe"), &["/hibernate", "off"]).map(|_| ())
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        if b.data.get("wasEnabled").map(String::as_str).unwrap_or("1") == "0" {
            return Ok(());
        }
        let powercfg = exe("powercfg.exe");
        if let Err(e) = checked(&*self.cmd, &powercfg, &["/hibernate", "on"]) {
            if (self.supported)() {
                return Err(e);
            }
            log::info("hibernation is not supported on this PC, nothing to turn back on");
            return Ok(());
        }
        let kind = match b.data.get("hiberType").map(String::as_str) {
            Some("1") => "reduced",
            Some("2") => "full",
            _ => return Ok(()),
        };
        // Hibernation is back; a type that will not take must not hold the backup forever.
        if let Err(e) = checked(&*self.cmd, &powercfg, &["/hibernate", "/type", kind]) {
            log::error("restoring the hiberfile type failed", Some(&e));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- Services to manual

pub const DEFAULT_SERVICES: [&str; 12] = [
    "MapsBroker", "TrkWks", "lfsvc", "SharedAccess", "WpcMonSvc", "RetailDemo", "Fax", "PhoneSvc", "WMPNetworkSvc", "wisvc",
    "edgeupdate", "edgeupdatem",
];

pub struct ServicesManualTweak {
    pub cmd: Arc<dyn Cmd>,
    pub id: String,
    pub title: String,
    pub names: Vec<String>,
    pub hive: Hive,
    pub root: String,
}

impl ServicesManualTweak {
    pub fn system(cmd: Arc<dyn Cmd>, id: &str, title: &str, names: &[&str]) -> ServicesManualTweak {
        ServicesManualTweak {
            cmd,
            id: id.into(),
            title: title.into(),
            names: names.iter().map(|s| s.to_string()).collect(),
            hive: Hive::Hklm,
            root: "SYSTEM\\CurrentControlSet\\Services".into(),
        }
    }

    fn key(&self, svc: &str) -> String {
        format!("{}\\{svc}", self.root)
    }

    fn set_start(&self, svc: &str, mode: &str) -> bool {
        match self.cmd.run(&exe("sc.exe"), &["config", svc, "start=", mode]) {
            Ok(r) if r.exit_code == 0 => true,
            Ok(r) => {
                log::error(&format!("sc config {svc} start= {mode} failed ({}): {}", r.exit_code, r.output), None);
                false
            }
            Err(e) => {
                log::error(&format!("sc config {svc} failed"), Some(&e));
                false
            }
        }
    }
}

/// `sc config start=` word for a saved `Start` value. Backups from before `DelayedAutostart` was saved give `false`.
fn sc_start_mode(start: Option<&str>, delayed: bool) -> Option<&'static str> {
    match start {
        Some("2") if delayed => Some("delayed-auto"),
        Some("2") => Some("auto"),
        Some("3") => Some("demand"),
        Some("4") => Some("disabled"),
        _ => None,
    }
}

impl Tweak for ServicesManualTweak {
    fn id(&self) -> &str {
        &self.id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn is_applied(&self) -> Result<bool> {
        let mut any = false;
        for s in &self.names {
            if !reg::key_exists(self.hive, &self.key(s))? {
                continue;
            }
            any = true;
            if dword(self.hive, &self.key(s), "Start")? == Some(2) {
                return Ok(false);
            }
        }
        Ok(any)
    }
    fn capture(&self, b: &mut TweakBackup) -> Result<()> {
        for s in &self.names {
            let key = self.key(s);
            if reg::key_exists(self.hive, &key)? && dword(self.hive, &key, "Start")? == Some(2) {
                b.capture(self.hive, &key, "Start")?;
                b.capture(self.hive, &key, "DelayedAutostart")?;
            }
        }
        Ok(())
    }
    fn revert_default(&self) -> Result<bool> {
        // Windows ships Spooler on Automatic. The Xbox services ship on Manual, so they have nothing to write.
        if self.id != "deb.spooler" || dword(self.hive, &self.key("Spooler"), "Start")? != Some(3) {
            return Ok(false);
        }
        if self.set_start("Spooler", "auto") {
            Ok(true)
        } else {
            Err(Error::Revert(vec!["Spooler: sc config start= auto failed".into()]))
        }
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        for s in &self.names {
            let key = self.key(s);
            // only services that auto-start (Start=2) are touched
            if !reg::key_exists(self.hive, &key)? || dword(self.hive, &key, "Start")? != Some(2) {
                continue;
            }
            b.capture(self.hive, &key, "Start")?;
            b.capture(self.hive, &key, "DelayedAutostart")?;
            if !self.set_start(s, "demand") {
                // not changed, so nothing to restore for it
                b.registry.retain(|r| !r.path.eq_ignore_ascii_case(&key));
            }
        }
        Ok(())
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        let mut failed = Vec::new();
        for r in b.registry.iter().filter(|r| r.name == "Start" && r.existed) {
            let delayed = b.registry.iter().any(|d| d.name == "DelayedAutostart" && d.path.eq_ignore_ascii_case(&r.path) && d.value.as_deref() == Some("1"));
            let Some(mode) = sc_start_mode(r.value.as_deref(), delayed) else { continue };
            let svc = r.path.rsplit('\\').next().unwrap_or("");
            if !reg::key_exists(self.hive, &self.key(svc))? {
                continue; // uninstalled since: nothing to put back
            }
            if !self.set_start(svc, mode) {
                failed.push(format!("{svc}: sc config start= {mode} failed"));
            }
        }
        if failed.is_empty() {
            Ok(())
        } else {
            Err(Error::Revert(failed))
        }
    }
}

// ---------------------------------------------------------------- Power plan

pub const PLAN_GUID: &str = "8e3f2a71-5c1b-4d7e-9a2f-6d795475e001";
pub const HIGH_PERFORMANCE: &str = "8c5e7fda-e8bf-4a96-9a85-a6e23a8c635c";
const BALANCED: &str = "381b4222-f694-41f0-9685-ff5bb260df2e";
const PROCESSOR: &str = "54533251-82be-4824-96c1-47b60b740d00";
const NONE: &str = "fea3413e-7e05-4911-9a71-700331f1c294";

/// AC value that leaves the base plan's setting alone.
const KEEP: u32 = u32::MAX;

/// (subgroup, setting, AC, DC). DC `None` leaves the battery value of the base plan.
type Row = (&'static str, &'static str, u32, Option<u32>);

const DESKTOP: [Row; 25] = [
    ("0012ee47-9041-4b5d-9b77-535fba8b1442", "6738e2c4-e8a5-4a42-b16a-e040e769756e", 0, Some(1200)),
    ("19cbb8fa-5279-450e-9fac-8a3d5fedd0c1", "12bbebe6-58d6-4636-95bb-3217ef867c1a", 0, Some(0)),
    ("238c9fa8-0aad-41ed-83f4-97be242c8f20", "29f6c1db-86da-48c5-9fdb-f2b67b1f44da", 0, Some(0)),
    ("238c9fa8-0aad-41ed-83f4-97be242c8f20", "9d7815a6-7ee4-497e-8888-515a05f02364", 0, Some(600)),
    ("2a737441-1930-4402-8d77-b2bebba308a3", "0853a681-27c8-4100-a2fd-82013e970683", 0, None),
    ("2a737441-1930-4402-8d77-b2bebba308a3", "48e6b7a6-50f5-4782-a5d4-53bb8f07e226", 0, None),
    ("2a737441-1930-4402-8d77-b2bebba308a3", "d4e98f31-5ffe-4ce1-be31-1b38b384c009", 0, None),
    (NONE, "4faab71a-92e5-4726-b531-224559672d19", 0, None),
    ("501a4d13-42af-4429-9fd1-a8218c268e20", "ee12f906-d277-404b-b6da-e5fa1a576df5", 0, Some(0)),
    (PROCESSOR, "06cadf0e-64ed-448a-8927-ce7bf90eb35d", 10, None),
    (PROCESSOR, "0cc5b647-c1df-4637-891a-dec35c318583", 100, None),
    (PROCESSOR, "36687f9e-e3a5-4dbf-b1dc-15eb381c6863", 0, Some(0)),
    (PROCESSOR, "3b04d4fd-1cc7-4f23-ab1c-d1337819c4bb", 0, None),
    (PROCESSOR, "4b92d758-5a24-4851-a470-815d78aee119", 10, None),
    (PROCESSOR, "4d2b0152-7d5c-498b-88e2-34345392a2c5", 5000, None),
    (PROCESSOR, "5d76a2ca-e8c0-402f-a133-2158492d58ad", 0, Some(0)),
    (PROCESSOR, "619b7505-003b-4e82-b7a6-4dd29c300971", 1, None),
    (PROCESSOR, "6c2993b0-8f48-481f-bcc6-00dd2742aa06", 0, Some(0)),
    (PROCESSOR, "7b224883-b3cc-4d79-819f-8374152cbe7c", 20, None),
    (PROCESSOR, "893dee8e-2bef-41e0-89c6-b55d0929964c", 0, None),
    (PROCESSOR, "8baa4a8a-14c6-4451-8e8b-14bdbd197537", 0, Some(0)),
    // system cooling policy: 1 = Active (fans first), 0 = Passive would slow the CPU before raising the fans
    (PROCESSOR, "94d3a615-a899-4ac5-ae2b-e4d8f634367f", 1, Some(0)),
    (PROCESSOR, "bc5038f7-23e0-4960-96da-33abaf5935ec", 100, None),
    (PROCESSOR, "ea062031-0e34-4ff1-9b6d-eb1059334028", 100, Some(100)),
    // display timeout: AC keeps the base plan's, a fixed 0 would never turn an OLED off
    ("7516b95f-f776-4464-8c53-06167f40cc99", "3c0bc021-c8a8-4e07-a973-6b14cbcb2b7e", KEEP, Some(600)),
];

// NOTE: the C# table has both ...6863 and ...6864 (near-identical GUIDs); copied as is.
const LAPTOP: [Row; 12] = [
    ("0012ee47-9041-4b5d-9b77-535fba8b1442", "6738e2c4-e8a5-4a42-b16a-e040e769756e", 0, Some(600)),
    ("238c9fa8-0aad-41ed-83f4-97be242c8f20", "29f6c1db-86da-48c5-9fdb-f2b67b1f44da", 0, Some(900)),
    ("238c9fa8-0aad-41ed-83f4-97be242c8f20", "9d7815a6-7ee4-497e-8888-515a05f02364", 0, Some(5400)),
    ("2a737441-1930-4402-8d77-b2bebba308a3", "48e6b7a6-50f5-4782-a5d4-53bb8f07e226", 0, Some(0)),
    // core parking: none on mains, allowed on battery
    (PROCESSOR, "0cc5b647-c1df-4637-891a-dec35c318583", 100, Some(4)),
    (PROCESSOR, "36687f9e-e3a5-4dbf-b1dc-15eb381c6863", 40, Some(60)),
    (PROCESSOR, "36687f9e-e3a5-4dbf-b1dc-15eb381c6864", 40, Some(60)),
    // minimum processor state: 5% on battery is the Windows Balanced value
    (PROCESSOR, "893dee8e-2bef-41e0-89c6-b55d0929964c", 100, Some(5)),
    (PROCESSOR, "97cfac41-2217-47eb-992d-618b1977c907", 1000, None),
    (PROCESSOR, "bc5038f7-23e0-4960-96da-33abaf5935ec", 100, Some(100)),
    (PROCESSOR, "ea062031-0e34-4ff1-9b6d-eb1059334028", 100, Some(100)),
    ("7516b95f-f776-4464-8c53-06167f40cc99", "3c0bc021-c8a8-4e07-a973-6b14cbcb2b7e", KEEP, Some(300)),
];

/// First `8-4-4-4-12` hex GUID in `s`, lowercased.
pub fn find_guid(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let dashes = [8usize, 13, 18, 23];
    (0..b.len().saturating_sub(35)).find_map(|i| {
        let w = &b[i..i + 36];
        let ok = w.iter().enumerate().all(|(j, c)| if dashes.contains(&j) { *c == b'-' } else { c.is_ascii_hexdigit() });
        ok.then(|| s[i..i + 36].to_ascii_lowercase())
    })
}

pub struct PowerPlanTweak {
    pub cmd: Arc<dyn Cmd>,
    pub laptop: bool,
}

impl PowerPlanTweak {
    fn pc(&self, args: &[&str]) -> Result<RunResult> {
        self.cmd.run(&exe("powercfg.exe"), args)
    }
    fn pc_checked(&self, args: &[&str]) -> Result<String> {
        checked(&*self.cmd, &exe("powercfg.exe"), args)
    }
    fn active_scheme(&self) -> Result<Option<String>> {
        Ok(find_guid(&self.pc_checked(&["/getactivescheme"])?))
    }
    fn scheme_exists(&self, guid: &str) -> bool {
        self.pc(&["/list"]).map(|r| r.output.to_ascii_lowercase().contains(guid)).unwrap_or(false)
    }
    /// powercfg also exits 0 for a setting this PC lacks; only a real one is printed with its GUID.
    fn setting_exists(&self, sub: &str, setting: &str) -> bool {
        self.pc(&["/query", PLAN_GUID, sub, setting]).is_ok_and(|r| r.exit_code == 0 && r.output.to_ascii_lowercase().contains(setting))
    }
}

impl Tweak for PowerPlanTweak {
    fn id(&self) -> &str {
        "general.powerplan"
    }
    fn title(&self) -> &str {
        "MyTune power plan"
    }
    fn is_applied(&self) -> Result<bool> {
        Ok(self.active_scheme()?.as_deref() == Some(PLAN_GUID))
    }
    fn capture(&self, b: &mut TweakBackup) -> Result<()> {
        if !b.data.contains_key("previous") {
            if let Some(p) = self.active_scheme()?.filter(|p| p != PLAN_GUID) {
                b.data.insert("previous".into(), p);
            }
        }
        Ok(())
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        self.capture(b)?;
        let base = if !self.laptop && self.scheme_exists(HIGH_PERFORMANCE) { HIGH_PERFORMANCE } else { BALANCED };
        if self.scheme_exists(PLAN_GUID) {
            // powercfg refuses to delete the active plan
            let was_active = self.is_applied()?;
            if was_active {
                let away = b.data.get("previous").filter(|p| self.scheme_exists(&p.to_ascii_lowercase())).map_or(base, String::as_str);
                self.pc_checked(&["/setactive", away])?;
            }
            if let Err(e) = self.pc_checked(&["/delete", PLAN_GUID]) {
                if was_active {
                    let _ = self.pc(&["/setactive", PLAN_GUID]);
                }
                return Err(e);
            }
        }
        self.pc_checked(&["/duplicatescheme", base, PLAN_GUID])?;
        self.pc_checked(&["/changename", PLAN_GUID, "MyTune Performance", "Low latency and stable FPS (made by MyTune)"])?;
        let rows: &[Row] = if self.laptop { &LAPTOP } else { &DESKTOP };
        let mut failed = Vec::new();
        for (sub, setting, ac, dc) in rows {
            let ac = (*ac != KEEP).then_some(("/setacvalueindex", *ac));
            for (flag, value) in ac.into_iter().chain(dc.map(|d| ("/setdcvalueindex", d))) {
                let written = self.pc(&[flag, PLAN_GUID, sub, setting, &value.to_string()]).is_ok_and(|r| r.exit_code == 0);
                // Some settings do not exist on every PC; skip those instead of failing the whole plan.
                if !written && self.setting_exists(sub, setting) {
                    failed.push(format!("{setting} {flag}"));
                }
            }
        }
        self.pc_checked(&["/setactive", PLAN_GUID])?;
        if failed.is_empty() {
            Ok(())
        } else {
            Err(Error::Plain(format!("{} power plan setting(s) could not be written: {}", failed.len(), failed.join(", "))))
        }
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        let saved = b.data.get("previous").map(|p| p.to_ascii_lowercase());
        // A failed list is an error: read as "plan gone", it would select Balanced and lose the saved plan.
        let list = self.pc_checked(&["/list"])?.to_ascii_lowercase();
        // Some PCs have no Balanced plan. If none is listed, trying Balanced fails and keeps the backup.
        let target = saved.iter().map(String::as_str).chain([BALANCED, HIGH_PERFORMANCE])
            .find(|p| !p.is_empty() && *p != PLAN_GUID && list.contains(p))
            .unwrap_or(BALANCED);
        self.pc_checked(&["/setactive", target])?;
        if list.contains(PLAN_GUID) {
            let _ = self.pc(&["/delete", PLAN_GUID]);
        }
        Ok(())
    }
    /// No record of the user's plan: Balanced is the Windows default. Without it, nothing is touched.
    fn revert_default(&self) -> Result<bool> {
        if !self.is_applied()? || !self.scheme_exists(BALANCED) {
            return Ok(false);
        }
        self.pc_checked(&["/setactive", BALANCED])?;
        let _ = self.pc(&["/delete", PLAN_GUID]);
        Ok(true)
    }
    /// A game is running on the booster's plan: its record is the user's real plan. The tweak takes it over,
    /// so there is one record and the game's exit does not undo this apply.
    fn on_applied(&self, store: &mut Store) -> Result<()> {
        let Some(user) = store.top("PowerPrevious").and_then(Value::as_str).map(String::from) else { return Ok(()) };
        let Some(mut b) = store.get_backup(self.id()).cloned() else { return Ok(()) };
        // High performance here is the booster's own fallback plan, not a choice of the user
        if b.data.get("previous").is_none_or(|p| p.eq_ignore_ascii_case(PLAN_GUID) || p.eq_ignore_ascii_case(HIGH_PERFORMANCE)) {
            b.data.insert("previous".into(), user);
        }
        store.set_top("PowerPrevious", Value::Null);
        store.set_backup(self.id(), b)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Engine, RevertOutcome};
    use crate::store::Store;
    use std::sync::Mutex;

    type Handler = Box<dyn Fn(&str) -> Option<(i32, String)> + Send + Sync>;

    /// Fake program runner: handler gets "<program-stem> <args>" and returns (exit, output);
    /// `None` means exit 0 with no output. Every call is recorded.
    struct Fake {
        calls: Mutex<Vec<String>>,
        handler: Handler,
    }
    impl Fake {
        fn new(h: impl Fn(&str) -> Option<(i32, String)> + Send + Sync + 'static) -> Arc<Fake> {
            Arc::new(Fake { calls: Mutex::new(vec![]), handler: Box::new(h) })
        }
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }
    impl Cmd for Fake {
        fn run(&self, file: &str, args: &[&str]) -> Result<RunResult> {
            let stem = std::path::Path::new(file).file_stem().unwrap().to_string_lossy().to_lowercase();
            let line = format!("{stem} {}", args.join(" "));
            self.calls.lock().unwrap().push(line.clone());
            let (exit_code, output) = (self.handler)(&line).unwrap_or((0, String::new()));
            Ok(RunResult { exit_code, output })
        }
    }

    struct Scratch(String);
    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let p = format!("Software\\MyTuneTest\\tw-{tag}-{}", std::process::id());
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
    fn guid_and_bcd_parsing() {
        let o = "Power Scheme GUID: 381B4222-F694-41F0-9685-FF5BB260DF2E  (Balanced)";
        assert_eq!(find_guid(o).as_deref(), Some("381b4222-f694-41f0-9685-ff5bb260df2e"));
        assert_eq!(find_guid("no guid here"), None);
        let bcd = "identifier {current}\r\nhypervisorlaunchtype    Auto\r\ndisabledynamictick  Yes";
        assert_eq!(parse_bcd_value(bcd, "hypervisorlaunchtype"), "Auto");
        assert_eq!(parse_bcd_value(bcd, "disabledynamictick"), "Yes");
        assert_eq!(parse_bcd_value(bcd, "useplatformclock"), "");
    }

    // ------------------------------------------------------------ services

    fn svc_setup(s: &Scratch) -> ServicesManualTweak {
        for (n, start) in [("A", 2), ("B", 3), ("C", 2), ("D", 4)] {
            reg::set(Hive::Hkcu, &format!("{}\\{n}", s.0), "Start", &RegValue::Dword(start)).unwrap();
        }
        ServicesManualTweak {
            cmd: system_cmd(), // replaced per test
            id: "deb.svc".into(),
            title: "Services".into(),
            names: ["A", "B", "C", "D", "Missing"].map(String::from).to_vec(),
            hive: Hive::Hkcu,
            root: s.0.clone(),
        }
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
    fn services_only_touches_auto_start_and_reverts_to_auto() {
        let s = Scratch::new("svc");
        let fake = Fake::new(|l| (l == "sc config C start= demand").then(|| (5, "denied".into())));
        let mut t = svc_setup(&s);
        t.cmd = fake.clone();
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        assert_eq!(fake.calls(), vec!["sc config A start= demand", "sc config C start= demand"]);
        // C failed -> not in the backup; only A is
        e.with_store(|st| {
            let b = st.get_backup("deb.svc").unwrap();
            assert_eq!(b.registry.iter().filter(|r| r.name == "Start").count(), 1);
            assert!(b.registry.iter().all(|r| r.path.ends_with("\\A")));
        });
        assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted);
        assert_eq!(fake.calls().last().unwrap(), "sc config A start= auto");
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
    fn services_all_failed_leaves_no_backup() {
        let s = Scratch::new("svcphantom");
        let mut t = svc_setup(&s);
        t.cmd = Fake::new(|l| l.starts_with("sc config").then(|| (5, "denied".into())));
        let (d, e) = engine();
        e.apply(&t).unwrap();
        assert!(e.with_store(|st| st.get_backup("deb.svc").is_none()));
        assert!(Store::load(d.path()).get_backup("deb.svc").is_none(), "not on disk either");
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
    fn hibernation_original_is_on_disk_before_the_change() {
        let (d, e) = engine();
        let state = d.path().join("state.json");
        let seen = Arc::new(Mutex::new(false));
        let s2 = seen.clone();
        let fake = Fake::new(move |l| {
            if l.starts_with("powercfg") {
                *s2.lock().unwrap() = std::fs::read_to_string(&state).is_ok_and(|j| j.contains("wasEnabled"));
            }
            None
        });
        let scratch = Scratch::new("hibcap");
        let t = HibernationTweak { cmd: fake, hive: Hive::Hkcu, path: scratch.0.clone(), supported: || true };
        e.apply(&t).unwrap();
        assert!(*seen.lock().unwrap());
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
    fn failed_bcd_read_is_not_stored_as_absent() {
        let s = Scratch::new("bcdfail");
        let fake = Fake::new(|l| l.starts_with("bcdedit /enum").then(|| (1, "failed".into())));
        let t = VbsTweak { cmd: fake.clone(), entry: Some("{t}".into()), hive: Hive::Hkcu, device_guard: format!("{}\\dg", s.0), hvci: format!("{}\\hvci", s.0) };
        let (_d, e) = engine();
        assert!(e.apply(&t).is_err());
        assert!(e.with_store(|st| st.get_backup("adv.vbs").is_none()));
        assert!(!fake.calls().iter().any(|c| c.contains("/set")));
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
    fn services_failed_revert_keeps_backup() {
        let s = Scratch::new("svcfail");
        let ok = Fake::new(|_| None);
        let mut t = svc_setup(&s);
        t.cmd = ok;
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        t.cmd = Fake::new(|l| l.starts_with("sc config").then(|| (1, "nope".into())));
        let err = e.revert(&t).unwrap_err();
        assert!(matches!(err, Error::Revert(ref f) if f.len() == 2), "{err}");
        assert!(e.with_store(|st| st.get_backup("deb.svc").is_some()));
        t.cmd = Fake::new(|_| None);
        assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted);
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
    fn services_is_applied_rules() {
        let s = Scratch::new("svcapplied");
        let mut t = svc_setup(&s);
        assert!(!t.is_applied().unwrap(), "A and C still auto");
        reg::set(Hive::Hkcu, &format!("{}\\A", s.0), "Start", &RegValue::Dword(3)).unwrap();
        reg::set(Hive::Hkcu, &format!("{}\\C", s.0), "Start", &RegValue::Dword(3)).unwrap();
        assert!(t.is_applied().unwrap());
        t.names = vec!["Missing".into()];
        assert!(!t.is_applied().unwrap(), "no service installed");
    }

    // ------------------------------------------------------------ hibernation

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
    fn hibernation_remembers_if_it_was_off() {
        for (was, expect_on) in [(1u32, true), (0u32, false)] {
            let s = Scratch::new(&format!("hib{was}"));
            reg::set(Hive::Hkcu, &s.0, "HibernateEnabled", &RegValue::Dword(was)).unwrap();
            let fake = Fake::new(|_| None);
            let t = HibernationTweak { cmd: fake.clone(), hive: Hive::Hkcu, path: s.0.clone(), supported: || true };
            let (_d, e) = engine();
            e.apply(&t).unwrap();
            e.revert(&t).unwrap();
            let calls = fake.calls();
            assert_eq!(calls[0], "powercfg /hibernate off");
            assert_eq!(calls.contains(&"powercfg /hibernate on".to_string()), expect_on, "{calls:?}");
        }
    }

    fn hib_backup(data: &[(&str, &str)]) -> TweakBackup {
        let mut b = TweakBackup::new("t");
        b.data = data.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        b
    }

    #[test]
    fn hibernation_absent_value_is_not_counted_as_enabled() {
        // only reads a key that does not exist
        let t = HibernationTweak { cmd: Fake::new(|_| None), hive: Hive::Hkcu, path: "Software\\MyTuneTest\\no-such-key".into(), supported: || true };
        let mut b = TweakBackup::new("t");
        t.capture(&mut b).unwrap();
        assert_eq!(b.data["wasEnabled"], "0");
    }

    #[test]
    fn hibernation_revert_without_support_ends_and_with_support_keeps_failing() {
        let fails = || Fake::new(|l| (l == "powercfg /hibernate on").then(|| (1, "not supported".into())));
        let b = hib_backup(&[("wasEnabled", "1")]);
        let no = HibernationTweak { cmd: fails(), hive: Hive::Hkcu, path: "x".into(), supported: || false };
        assert!(no.revert(&b).is_ok());
        let yes = HibernationTweak { cmd: fails(), hive: Hive::Hkcu, path: "x".into(), supported: || true };
        assert!(yes.revert(&b).is_err());
    }

    #[test]
    fn hibernation_revert_restores_the_file_type() {
        let fake = Fake::new(|_| None);
        let t = HibernationTweak { cmd: fake.clone(), hive: Hive::Hkcu, path: "x".into(), supported: || true };
        t.revert(&hib_backup(&[("wasEnabled", "1"), ("hiberType", "1")])).unwrap();
        assert_eq!(fake.calls(), ["powercfg /hibernate on", "powercfg /hibernate /type reduced"]);
    }

    #[test]
    fn delayed_autostart_comes_back_and_old_backups_stay_plain() {
        assert_eq!(sc_start_mode(Some("2"), true), Some("delayed-auto"));
        assert_eq!(sc_start_mode(Some("2"), false), Some("auto"));
        assert_eq!(sc_start_mode(Some("3"), true), Some("demand"));
        assert_eq!(sc_start_mode(None, false), None);
    }

    // ------------------------------------------------------------ VBS / timer tick (bcdedit)

    /// Fake bcdedit with a tiny state: no {current} entry (uses {default}), elements kept in a map.
    fn fake_bcd(initial: &[(&str, &str)], delete_fails: bool) -> Arc<Fake> {
        let st: Arc<Mutex<Vec<(String, String)>>> =
            Arc::new(Mutex::new(initial.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()));
        Fake::new(move |l| {
            let mut m = st.lock().unwrap();
            let parts: Vec<&str> = l.split_whitespace().collect();
            match parts.as_slice() {
                ["bcdedit", "/enum", "{current}"] => Some((1, "not found".into())),
                ["bcdedit", "/enum", _] => Some((0, m.iter().map(|(k, v)| format!("{k}    {v}\r\n")).collect())),
                ["bcdedit", "/set", _, k, v] => {
                    m.retain(|(x, _)| x != k);
                    m.push((k.to_string(), v.to_string()));
                    None
                }
                ["bcdedit", "/deletevalue", _, k] => {
                    if delete_fails {
                        return Some((1, "element not found".into()));
                    }
                    m.retain(|(x, _)| x != k);
                    None
                }
                _ => None,
            }
        })
    }

    fn vbs(s: &Scratch, cmd: Arc<Fake>) -> VbsTweak {
        VbsTweak { cmd, entry: None, hive: Hive::Hkcu, device_guard: format!("{}\\DG", s.0), hvci: format!("{}\\DG\\Hvci", s.0) }
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
    fn vbs_apply_and_revert_restores_registry_and_bcd() {
        let s = Scratch::new("vbs");
        reg::set(Hive::Hkcu, &format!("{}\\DG", s.0), "EnableVirtualizationBasedSecurity", &RegValue::Dword(1)).unwrap();
        let fake = fake_bcd(&[("hypervisorlaunchtype", "Auto")], false);
        let t = vbs(&s, fake.clone());
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        assert!(t.is_applied().unwrap());
        e.revert(&t).unwrap();
        assert_eq!(dword(Hive::Hkcu, &format!("{}\\DG", s.0), "EnableVirtualizationBasedSecurity").unwrap(), Some(1));
        assert!(!reg::key_exists(Hive::Hkcu, &format!("{}\\DG\\Hvci", s.0)).unwrap(), "created Hvci key removed");
        assert_eq!(bcd_value(&*fake, "{default}", "hypervisorlaunchtype"), "Auto");
        assert!(fake.calls().iter().any(|c| c == "bcdedit /set {default} hypervisorlaunchtype off"));
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
    fn vbs_stuck_bcd_delete_is_reported_and_backup_kept() {
        let s = Scratch::new("vbsabsent");
        let fake = fake_bcd(&[], true); // hypervisorlaunchtype absent; /deletevalue always fails
        let t = vbs(&s, fake.clone());
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        // /set added it; deletevalue fails and the value is still there -> revert must FAIL, backup kept
        let err = e.revert(&t).unwrap_err();
        assert!(matches!(err, Error::ProcessFailed { .. }), "{err}");
        assert!(e.with_store(|st| st.get_backup("adv.vbs").is_some()));
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
    fn vbs_failed_registry_restore_skips_bcdedit() {
        let s = Scratch::new("vbsskip");
        let fake = fake_bcd(&[("hypervisorlaunchtype", "Auto")], false);
        let t = vbs(&s, fake.clone());
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        e.with_store(|st| {
            let mut b = st.get_backup("adv.vbs").unwrap().clone();
            b.registry[0].hive = "HKXX".into();
            st.set_backup("adv.vbs", b).unwrap();
        });
        let n = fake.calls().len();
        assert!(e.revert(&t).is_err());
        assert_eq!(fake.calls().len(), n, "no bcdedit call after failed registry restore");
        assert!(e.with_store(|st| st.get_backup("adv.vbs").is_some()));
    }

    #[test]
    fn timer_tick_roundtrip_with_hpet() {
        let fake = fake_bcd(&[("disabledynamictick", "No"), ("useplatformclock", "Yes")], false);
        let t = TimerTickTweak { cmd: fake.clone(), entry: None };
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        assert!(t.is_applied().unwrap());
        assert_eq!(bcd_value(&*fake, "{default}", "useplatformclock"), "");
        e.revert(&t).unwrap();
        assert_eq!(bcd_value(&*fake, "{default}", "disabledynamictick"), "No");
        assert_eq!(bcd_value(&*fake, "{default}", "useplatformclock"), "Yes");
    }

    #[test]
    fn timer_tick_originally_absent_is_removed_on_revert() {
        let fake = fake_bcd(&[], false);
        let t = TimerTickTweak { cmd: fake.clone(), entry: None };
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        e.revert(&t).unwrap();
        assert_eq!(bcd_value(&*fake, "{default}", "disabledynamictick"), "");
    }

    #[test]
    fn bcd_falls_back_only_when_current_is_missing_and_pins_the_entry() {
        // {current} missing, store fine
        let fake = fake_bcd(&[], false);
        assert_eq!(bcd_entry(&*fake, &None).unwrap(), "{default}");
        // whole store unreadable: no entry, so nothing can be edited
        let broken = Fake::new(|l| l.starts_with("bcdedit /enum").then(|| (1, "access denied".into())));
        assert!(bcd_entry(&*broken, &None).is_err());
        let t = TimerTickTweak { cmd: broken.clone(), entry: None };
        let mut b = TweakBackup::new("t");
        assert!(t.apply(&mut b).is_err());
        assert!(!broken.calls().iter().any(|c| c.contains("/set")));
        // the entry's GUID is stored and revert uses it
        const G: &str = "11111111-2222-3333-4444-555555555555";
        let pinned = Fake::new(|l| l.ends_with("{current} /v").then(|| (0, format!("identifier {{{G}}}"))));
        let t = TimerTickTweak { cmd: pinned.clone(), entry: None };
        let mut b = TweakBackup::new("t");
        t.apply(&mut b).unwrap();
        assert_eq!(b.data[BCD_ENTRY], format!("{{{G}}}"));
        t.revert(&b).unwrap();
        assert!(pinned.calls().contains(&format!("bcdedit /deletevalue {{{G}}} disabledynamictick")));
    }

    // ------------------------------------------------------------ power plan

    #[test]
    fn power_plan_tables_keep_the_promises() {
        const COOLING: &str = "94d3a615-a899-4ac5-ae2b-e4d8f634367f";
        const DISPLAY: &str = "3c0bc021-c8a8-4e07-a973-6b14cbcb2b7e";
        const MIN_CORES: &str = "0cc5b647-c1df-4637-891a-dec35c318583";
        let row = |t: &[Row], s: &str| *t.iter().find(|r| r.1 == s).unwrap();
        assert_eq!((row(&DESKTOP, COOLING).2, row(&DESKTOP, COOLING).3), (1, Some(0)), "Active on AC, DC untouched");
        assert_eq!(row(&DESKTOP, DISPLAY).2, KEEP);
        assert_eq!(row(&LAPTOP, DISPLAY).2, KEEP);
        assert_eq!(row(&LAPTOP, MIN_CORES).2, 100, "no core parking on mains");
        assert_eq!(row(&LAPTOP, "893dee8e-2bef-41e0-89c6-b55d0929964c").3, Some(5), "battery-friendly");
    }

    /// Fake powercfg whose active plan is MyTune's, with `extra` handling first.
    fn fake_on_plan(extra: impl Fn(&str) -> Option<(i32, String)> + Send + Sync + 'static) -> Arc<Fake> {
        Fake::new(move |l| {
            extra(l).or_else(|| {
                if l.starts_with("powercfg /getactivescheme") {
                    Some((0, format!("Power Scheme GUID: {PLAN_GUID}  (MyTune)")))
                } else if l.starts_with("powercfg /list") {
                    Some((0, format!("{HIGH_PERFORMANCE}\n{PLAN_GUID}\naaaaaaaa-1111-2222-3333-bbbbbbbbbbbb\n")))
                } else {
                    None
                }
            })
        })
    }

    #[test]
    fn power_plan_reapply_leaves_the_active_plan_before_deleting_it() {
        let fake = fake_on_plan(|_| None);
        let t = PowerPlanTweak { cmd: fake.clone(), laptop: false };
        let mut b = hib_backup(&[("previous", "aaaaaaaa-1111-2222-3333-bbbbbbbbbbbb")]);
        t.apply(&mut b).unwrap();
        let c = fake.calls();
        let at = |x: String| c.iter().position(|l| *l == x).unwrap();
        assert!(at("powercfg /setactive aaaaaaaa-1111-2222-3333-bbbbbbbbbbbb".into()) < at(format!("powercfg /delete {PLAN_GUID}")));
        assert_eq!(c.last().unwrap(), &format!("powercfg /setactive {PLAN_GUID}"));
    }

    #[test]
    fn power_plan_failed_delete_stops_and_goes_back() {
        let fake = fake_on_plan(|l| l.starts_with("powercfg /delete").then(|| (1, "denied".into())));
        let t = PowerPlanTweak { cmd: fake.clone(), laptop: false };
        assert!(t.apply(&mut TweakBackup::new("t")).is_err());
        let c = fake.calls();
        assert!(!c.iter().any(|l| l.contains("/duplicatescheme")));
        assert_eq!(c.last().unwrap(), &format!("powercfg /setactive {PLAN_GUID}"));
    }

    #[test]
    fn power_plan_write_failure_counts_only_for_a_setting_that_exists() {
        const REAL: &str = "893dee8e-2bef-41e0-89c6-b55d0929964c";
        const MISSING: &str = "619b7505-003b-4e82-b7a6-4dd29c300971";
        let fake = fake_on_plan(|l| {
            if l.starts_with("powercfg /setacvalueindex") && (l.contains(REAL) || l.contains(MISSING)) {
                Some((1, "failed".into()))
            } else if l.starts_with("powercfg /query") {
                Some((0, if l.contains(REAL) { format!("Power Setting GUID: {REAL}") } else { "Power Scheme GUID: x".into() }))
            } else {
                None
            }
        });
        let t = PowerPlanTweak { cmd: fake.clone(), laptop: false };
        let err = t.apply(&mut TweakBackup::new("t")).unwrap_err().to_string();
        assert!(err.contains(REAL) && !err.contains(MISSING), "{err}");
        assert_eq!(fake.calls().last().unwrap(), &format!("powercfg /setactive {PLAN_GUID}"), "the plan still goes active");
    }

    #[test]
    fn power_plan_takes_over_the_boosters_record() {
        const USER: &str = "cccccccc-1111-2222-3333-dddddddddddd";
        let t = PowerPlanTweak { cmd: fake_on_plan(|_| None), laptop: false };
        let (_d, e) = engine();
        e.with_store(|s| s.set_top("PowerPrevious", Value::String(USER.into())));
        e.apply(&t).unwrap();
        e.with_store(|s| {
            assert_eq!(s.get_backup("general.powerplan").unwrap().data["previous"], USER);
            assert_eq!(s.top("PowerPrevious"), Some(&Value::Null));
        });
    }

    #[test]
    fn power_plan_without_a_backup_goes_to_balanced_or_stays() {
        let on_balanced_pc = fake_on_plan(|l| l.starts_with("powercfg /list").then(|| (0, format!("{BALANCED}\n{PLAN_GUID}\n"))));
        let t = PowerPlanTweak { cmd: on_balanced_pc.clone(), laptop: false };
        assert!(t.revert_default().unwrap());
        assert!(on_balanced_pc.calls().contains(&format!("powercfg /setactive {BALANCED}")));
        let no_balanced = fake_on_plan(|_| None);
        let t = PowerPlanTweak { cmd: no_balanced.clone(), laptop: false };
        assert!(!t.revert_default().unwrap());
        assert!(!no_balanced.calls().iter().any(|l| l.contains("/setactive") || l.contains("/delete")));
    }

    fn fake_power(prev_exists: bool) -> Arc<Fake> {
        Fake::new(move |l| {
            if l.starts_with("powercfg /getactivescheme") {
                Some((0, "Power Scheme GUID: aaaaaaaa-1111-2222-3333-bbbbbbbbbbbb  (Mine)".into()))
            } else if l.starts_with("powercfg /list") {
                let mut o = format!("{HIGH_PERFORMANCE}\n{PLAN_GUID}\n{BALANCED}\n");
                if prev_exists {
                    o.push_str("AAAAAAAA-1111-2222-3333-BBBBBBBBBBBB\n");
                }
                Some((0, o))
            } else {
                None
            }
        })
    }

    #[test]
    fn power_plan_apply_sequence_and_revert_to_previous() {
        let fake = fake_power(true);
        let t = PowerPlanTweak { cmd: fake.clone(), laptop: false };
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        let c = fake.calls();
        assert_eq!(c[0], "powercfg /getactivescheme");
        assert!(c.contains(&format!("powercfg /delete {PLAN_GUID}")), "old MyTune plan removed first");
        assert!(c.contains(&format!("powercfg /duplicatescheme {HIGH_PERFORMANCE} {PLAN_GUID}")));
        assert_eq!(c.iter().filter(|x| x.starts_with("powercfg /setacvalueindex")).count(), 24, "display timeout on AC stays the base plan's");
        assert_eq!(c.iter().filter(|x| x.starts_with("powercfg /setdcvalueindex")).count(), 12);
        assert_eq!(c.last().unwrap(), &format!("powercfg /setactive {PLAN_GUID}"));
        e.revert(&t).unwrap();
        let c = fake.calls();
        assert!(c.contains(&"powercfg /setactive aaaaaaaa-1111-2222-3333-bbbbbbbbbbbb".to_string()));
        assert_eq!(c.last().unwrap(), &format!("powercfg /delete {PLAN_GUID}"));
    }

    #[test]
    fn power_plan_falls_back_to_balanced_and_laptop_uses_balanced_base() {
        let fake = fake_power(false); // previous plan no longer listed
        let t = PowerPlanTweak { cmd: fake.clone(), laptop: true };
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        let c = fake.calls();
        assert!(c.contains(&format!("powercfg /duplicatescheme {BALANCED} {PLAN_GUID}")));
        assert_eq!(c.iter().filter(|x| x.starts_with("powercfg /setacvalueindex")).count(), 11);
        e.revert(&t).unwrap();
        assert!(fake.calls().contains(&format!("powercfg /setactive {BALANCED}")));
    }

    #[test]
    fn power_plan_failed_list_keeps_the_saved_plan() {
        let fake = fake_on_plan(|l| l.starts_with("powercfg /list").then(|| (1, "failed".into())));
        let t = PowerPlanTweak { cmd: fake.clone(), laptop: false };
        let (_d, e) = engine();
        e.with_store(|s| s.set_backup("general.powerplan", hib_backup(&[("previous", "aaaaaaaa-1111-2222-3333-bbbbbbbbbbbb")]))).unwrap();
        assert!(e.revert(&t).is_err());
        assert!(!fake.calls().iter().any(|l| l.contains("/setactive") || l.contains("/delete")));
        assert!(e.with_store(|s| s.get_backup("general.powerplan").is_some()));
    }
}
