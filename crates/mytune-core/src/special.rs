//! Remaining C# tweaks that are not plain registry values:
//! DirectX user prefs (3), scheduled tasks (COM), network adapter properties and protocol bindings
//! (PowerShell), MSI mode (device list from PowerShell), service instances from the catalog, and the
//! revert-only tweaks (DNS, TCP stack, memory compression) that exist so old backups can be undone.
//!
//! NOT ported: NVIDIA tweaks (`general.nvidia`, `general.gsync`, per-game NVAPI) need NVAPI, which is out of
//! scope; `TimerResolutionTweak` also writes an app setting (`Settings.GameTimer`) and is not here.
//!
//! All programs go through `Cmd`, scheduled tasks through `TaskApi`, so tests never touch real adapters.
//!
//! Differences from C# (never weaker):
//! - Scheduled-task revert skips tasks that no longer exist and collects other failures into one error,
//!   keeping the backup (C# would throw a null reference on a vanished task).
//! - TCP revert restores the fields the backup has and logs the others (C# throws KeyNotFound).
//! - Removed tweaks can only be reverted, `apply` returns an error.

use crate::backup::TweakBackup;
use crate::catalog::{DataSet, DataTweak, Hooks, After, TweakFlags};
use crate::engine::Tweak;
use crate::error::{Error, Result};
use crate::log;
use crate::process::system32;
use crate::reg::{self, Hive, RegValue};
use crate::tweaks::{Cmd, ServicesManualTweak, DEFAULT_SERVICES};
use crate::util::b64_encode;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::Arc;

// ---------------------------------------------------------------- PowerShell helper

pub fn ps_quote(s: &str) -> String {
    // PowerShell also ends a single-quoted string at the curly quotes, so each is doubled too.
    format!("'{}'", s.replace('\'', "''").replace('\u{2018}', "\u{2018}\u{2018}").replace('\u{2019}', "\u{2019}\u{2019}").replace('\u{201A}', "\u{201A}\u{201A}").replace('\u{201B}', "\u{201B}\u{201B}"))
}

fn ps_list(items: &[String]) -> String {
    format!("@({})", items.iter().map(|s| ps_quote(s)).collect::<Vec<_>>().join(","))
}

/// An adapter name as a literal `-Name` argument: the cmdlets read `[`, `*` and `?` in it as wildcards.
fn ps_name(s: &str) -> String {
    format!("([Management.Automation.WildcardPattern]::Escape({}))", ps_quote(s))
}

/// Runs a script like C# `Sys.PowerShell`: `-EncodedCommand`, errors are terminating, non-zero exit fails.
pub fn powershell(cmd: &dyn Cmd, script: &str) -> Result<String> {
    let full = format!("$ProgressPreference='SilentlyContinue';$ErrorActionPreference='Stop';{script}");
    let utf16: Vec<u8> = full.encode_utf16().flat_map(|c| c.to_le_bytes()).collect();
    let exe = system32("WindowsPowerShell\\v1.0\\powershell.exe").to_string_lossy().into_owned();
    let r = cmd.run(&exe, &["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &b64_encode(&utf16)])?;
    if r.exit_code == 0 {
        Ok(r.output)
    } else {
        Err(Error::ProcessFailed { cmd: format!("powershell: {}", script.chars().take(160).collect::<String>()), code: r.exit_code, output: r.output })
    }
}

pub(crate) fn json_rows<T: for<'de> Deserialize<'de>>(out: &str) -> Result<Vec<T>> {
    let t = out.trim();
    Ok(serde_json::from_str(if t.is_empty() { "[]" } else { t })?)
}

/// For a revert's read-back script, which always prints JSON: no output means the read failed, not "nothing left".
fn read_back_rows<T: for<'de> Deserialize<'de>>(out: &str) -> Result<Vec<T>> {
    if out.trim().is_empty() {
        return Err(Error::Plain("the read-back after the revert returned nothing".into()));
    }
    json_rows(out)
}

// ---------------------------------------------------------------- adapters

#[derive(Deserialize)]
struct AdapterRow {
    #[serde(rename = "N")]
    name: String,
    #[serde(rename = "D")]
    description: Option<String>,
    #[serde(rename = "T")]
    kind: i64,
    /// `InterfaceGuid`, from `adapter_ids` only.
    #[serde(rename = "G", default)]
    guid: Option<String>,
    /// `Status`, from `adapter_ids` only.
    #[serde(rename = "S", default)]
    status: Option<String>,
}

const TWEAK_ADAPTERS: &str = "$_.Status -ne 'Disabled' -and $_.Status -ne 'Not Present'";

/// Connected Ethernet / Wi-Fi adapters, no virtual ones (same filter as C# `Net.ActiveAdapters`).
pub fn active_adapters(cmd: &dyn Cmd) -> Result<Vec<String>> {
    adapters(cmd, false, "$_.Status -eq 'Up'")
}

/// Adapters the driver tweaks work on: every enabled Ethernet / Wi-Fi adapter, connected or not.
/// Not `active_adapters`: a tweak restarts the adapter, so "connected right now" changed between two
/// calls and the tweaks read and wrote whichever adapter happened to be up.
fn tweak_adapters(cmd: &dyn Cmd) -> Result<Vec<String>> {
    adapters(cmd, false, TWEAK_ADAPTERS)
}

/// `tweak_adapters` for a change without a backup: a failed list is an error, not "no adapters".
fn tweak_adapters_strict(cmd: &dyn Cmd) -> Result<Vec<String>> {
    adapters(cmd, true, TWEAK_ADAPTERS)
}

fn adapters(cmd: &dyn Cmd, strict: bool, filter: &str) -> Result<Vec<String>> {
    let on_error = if strict { "Stop" } else { "SilentlyContinue" };
    let out = powershell(
        cmd,
        &format!("ConvertTo-Json -InputObject @(Get-NetAdapter -ErrorAction {on_error} | Where-Object {{ {filter} }} | ForEach-Object {{ [pscustomobject]@{{N=$_.Name;D=$_.InterfaceDescription;T=[int]$_.InterfaceType}} }}) -Compress"),
    )?;
    let rows: Vec<AdapterRow> = if strict { read_back_rows(&out)? } else { json_rows(&out)? };
    Ok(rows.into_iter().filter(|a| is_physical(a.kind, a.description.as_deref().unwrap_or_default())).map(|a| a.name).collect())
}

/// Every adapter Windows lists now, with its id. A failed read is an error, never "it was removed".
fn adapter_ids(cmd: &dyn Cmd) -> Result<Vec<AdapterRow>> {
    read_back_rows(&powershell(
        cmd,
        "$ids=@(Get-NetAdapter -Name * -ErrorAction Stop); if(-not $ids){ throw 'no adapters read' }; ConvertTo-Json -InputObject @($ids | ForEach-Object { [pscustomobject]@{N=$_.Name;D=$_.InterfaceDescription;T=[int]$_.InterfaceType;G=[string]$_.InterfaceGuid;S=[string]$_.Status} }) -Compress",
    )?)
}

/// Backup key `guid:<adapter>` = its `InterfaceGuid`: a name can change, the id cannot.
const GUID_KEY: &str = "guid:";

fn is_id_key(k: &str) -> bool {
    k.starts_with(GUID_KEY) && !k.contains('|')
}

/// Without ids the backup still works by name, so a failed read does not stop the tweak.
fn remember_ids(cmd: &dyn Cmd, b: &mut TweakBackup) {
    match adapter_ids(cmd) {
        Ok(ids) => {
            for r in ids.into_iter().filter(|r| !r.name.contains('|')) {
                if let Some(g) = r.guid.filter(|g| !g.is_empty()) {
                    b.data.entry(format!("{GUID_KEY}{}", r.name)).or_insert(g);
                }
            }
        }
        Err(e) => log::error("adapter ids not read, this backup uses names only", Some(&e)),
    }
}

/// Where the adapters of a backup are now.
#[derive(Default)]
struct Located {
    /// Saved name to current name. A renamed adapter is found by its id.
    now: BTreeMap<String, String>,
    /// Name-only entries (older backups) whose name is gone: removed or renamed, a name cannot tell.
    unknown: Vec<String>,
    /// Enabled physical adapters this backup does not know: a renamed one would be among them.
    others: Vec<String>,
    /// Entries with an id that Windows does not list now: unplugged, or gone for good.
    missing: Vec<String>,
}

const ADAPTER_GONE: &str = "is not connected, so its original settings were kept. Plug it in and try again, or use Forget if it is gone for good.";

fn adapter_gone(a: &str) -> String {
    format!("{a} {ADAPTER_GONE}")
}

/// True when the only thing a revert could not do is reach adapters that are not connected.
pub fn only_missing_adapters(e: &Error) -> bool {
    matches!(e, Error::Revert(v) if !v.is_empty() && v.iter().all(|m| m.ends_with(ADAPTER_GONE)))
}

/// The Restore page's Forget for an adapter that is gone. Tries the revert again first, so a present adapter is
/// never left changed and a failure of any other kind keeps the backup. True when the backup was dropped,
/// false when the adapter was back and everything was restored.
pub fn forget_missing_adapter(engine: &crate::engine::Engine, t: &dyn Tweak) -> Result<bool> {
    match engine.revert(t) {
        Err(e) if only_missing_adapters(&e) => engine.with_store(|s| s.remove_backup(t.id())).map(|()| true),
        r => r.map(|_| false),
    }
}

/// The end of a revert: what it could not do, with the adapters that are not connected. Any of it keeps the backup.
fn finish_revert(mut failed: Vec<String>, l: &Located) -> Result<()> {
    failed.extend(l.missing.iter().map(|a| adapter_gone(a)));
    failed.sort();
    failed.dedup();
    if failed.is_empty() {
        Ok(())
    } else {
        Err(Error::Revert(failed))
    }
}

fn locate(b: &TweakBackup, saved: &[String], here: &[AdapterRow]) -> Located {
    let mut l = Located::default();
    for a in saved {
        let row = match b.data.get(&format!("{GUID_KEY}{a}")) {
            Some(g) => {
                let r = here.iter().find(|r| r.guid.as_deref().is_some_and(|h| h.eq_ignore_ascii_case(g)));
                if r.is_none() {
                    l.missing.push(a.clone());
                }
                r
            }
            None => {
                let r = here.iter().find(|r| r.name.eq_ignore_ascii_case(a));
                if r.is_none() {
                    l.unknown.push(a.clone());
                }
                r
            }
        };
        if let Some(r) = row {
            l.now.insert(a.clone(), r.name.clone());
        }
    }
    let known: Vec<&String> = b.data.iter().filter(|(k, _)| is_id_key(k)).map(|(_, v)| v).collect();
    l.others = here
        .iter()
        .filter(|r| is_physical(r.kind, r.description.as_deref().unwrap_or_default()))
        .filter(|r| !matches!(r.status.as_deref(), Some("Disabled" | "Not Present")))
        .filter(|r| !l.now.values().any(|n| n.eq_ignore_ascii_case(&r.name)))
        .filter(|r| !r.guid.as_deref().is_some_and(|g| known.iter().any(|k| k.eq_ignore_ascii_case(g))))
        .map(|r| r.name.clone())
        .collect();
    l
}

fn unique<'a>(items: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut v: Vec<String> = items.cloned().collect();
    v.sort();
    v.dedup();
    v
}

/// A real Ethernet or Wi-Fi card, by `InterfaceType` and `InterfaceDescription`.
pub(crate) fn is_physical(kind: i64, description: &str) -> bool {
    const BAD: [&str; 11] = ["virtual", "hyper-v", "vpn", "tap-", "wintun", "loopback", "vmware", "virtualbox", "bluetooth", "wireguard", "tailscale"];
    let d = description.to_lowercase();
    matches!(kind, 6 | 71 | 117) // Ethernet, Wireless80211, GigabitEthernet
        && !BAD.iter().any(|b| d.contains(b))
}

pub const NO_ADAPTER: &str = "No enabled Ethernet or Wi-Fi adapter found.";

#[derive(Deserialize)]
struct PropRow {
    #[serde(rename = "A")]
    adapter: String,
    #[serde(rename = "K")]
    keyword: String,
    #[serde(rename = "V")]
    value: Option<String>,
    /// Driver default, empty when the driver names none.
    #[serde(rename = "D", default)]
    default: Option<String>,
    /// Values the driver accepts, comma separated; empty when it lists none.
    #[serde(rename = "L", default)]
    valid: Option<String>,
    /// Largest value of a numeric range, empty when the driver gives none.
    #[serde(rename = "X", default)]
    max: Option<String>,
    /// Step of that range, empty when the driver gives none.
    #[serde(rename = "S", default)]
    step: Option<String>,
}

/// Twice the driver default, capped at what the driver accepts (range max on its step grid, else the
/// smallest listed value at or above it, else the largest listed). None when the default or the accepted
/// values are unknown, or the result would not raise the value.
fn doubled(r: &PropRow) -> Option<String> {
    let num = |s: &Option<String>| s.as_deref().and_then(|s| s.trim().parse::<u64>().ok());
    let d = num(&r.default)?;
    let want = d.checked_mul(2)?;
    let t = if let Some(max) = num(&r.max) {
        let step = num(&r.step).filter(|s| *s > 0).unwrap_or(1);
        let t = want.min(max);
        d + t.checked_sub(d)? / step * step
    } else {
        let valid: Vec<u64> = r.valid.as_deref().unwrap_or("").split(',').filter_map(|v| v.trim().parse().ok()).collect();
        valid.iter().copied().filter(|v| *v >= want).min().or_else(|| valid.iter().copied().max())?
    };
    (t > d).then(|| t.to_string())
}

/// Restarts the adapter; waits for the link only when it had one (a disconnected adapter never comes up).
/// A failure is noted in `$f`, so the adapters after it still restart.
fn restart_script(a: &str) -> String {
    let (n, q) = (ps_name(a), ps_quote(a));
    format!("try {{ $n={n}; $o=Get-NetAdapter -Name $n -ErrorAction Stop; $u=$o.Status -eq 'Up'; $o | Where-Object Status -ne 'Disabled' | Restart-NetAdapter -ErrorAction Stop; for($i=0;$u -and $i -lt 30 -and (Get-NetAdapter -Name $n -ErrorAction SilentlyContinue).Status -ne 'Up';$i++){{ Start-Sleep 1 }} }} catch {{ $f+={q} }}")
}

const NO_SETTING: &str = "your network driver has no setting for this, nothing was changed";

/// Driver keyword tweak, e.g. "Adapter power management off". Targets are (keyword, value);
/// value None = twice the driver default (`doubled`).
pub struct AdapterPropertyTweak {
    pub cmd: Arc<dyn Cmd>,
    pub id: String,
    pub title: String,
    pub targets: Vec<(String, Option<String>)>,
}

type Found = (String, String, String); // adapter, keyword, value

impl AdapterPropertyTweak {
    fn keywords(&self) -> Vec<String> {
        self.targets.iter().map(|t| t.0.clone()).collect()
    }

    /// Leaves out a keyword whose driver does not accept the target value: it could never be set, so the
    /// tweak would never read as on.
    fn settable(&self, rows: Vec<PropRow>) -> Vec<PropRow> {
        rows.into_iter()
            .filter(|r| {
                let valid = r.valid.as_deref().unwrap_or("");
                self.target(r).is_some_and(|t| valid.is_empty() || valid.split(',').any(|v| v == t))
            })
            .collect()
    }

    /// The keywords this tweak can set on the adapters.
    fn found(&self, adapters: &[String]) -> Result<Vec<PropRow>> {
        Ok(self.settable(self.rows(adapters, &self.keywords())?))
    }

    /// Every row the driver has for these keywords, unfiltered. The `*` of a standard keyword is a wildcard to
    /// the cmdlet, so only the exact keyword is kept.
    fn rows(&self, adapters: &[String], kws: &[String]) -> Result<Vec<PropRow>> {
        if adapters.is_empty() {
            return Ok(vec![]);
        }
        let script = format!(
            "$r=@(); foreach($a in {}){{ $e=[Management.Automation.WildcardPattern]::Escape($a); foreach($k in {}){{ $p=Get-NetAdapterAdvancedProperty -Name $e -AllProperties -RegistryKeyword $k -ErrorAction SilentlyContinue | Where-Object {{ $_.RegistryKeyword -eq $k }}; if($p){{ $r+=[pscustomobject]@{{A=$a;K=$k;V=(@($p.RegistryValue) -join ',');D=(@($p.DefaultRegistryValue) -join ',');L=(@($p.ValidRegistryValues) -join ',');X=(@($p.NumericParameterMaxValue) -join ',');S=(@($p.NumericParameterStepValue) -join ',')}} }} }} }}; ConvertTo-Json -InputObject @($r) -Compress",
            ps_list(adapters),
            ps_list(kws)
        );
        json_rows(&powershell(&*self.cmd, &script)?)
    }

    /// `rows` for a revert: a read that fails is an error, because the failure that stopped a write can also
    /// empty a lenient read. An adapter that is really gone, or a keyword its driver lacks, gives no row.
    fn strict_rows(&self, adapters: &[String], kws: &[String]) -> Result<Vec<PropRow>> {
        if adapters.is_empty() || kws.is_empty() {
            return Ok(vec![]);
        }
        let script = format!(
            "$all=@(Get-NetAdapter -Name * -ErrorAction Stop | ForEach-Object Name); if(-not $all){{ throw 'no adapters read' }}; $r=@(); foreach($a in {}){{ if($all -notcontains $a){{ continue }}; $p=@(Get-NetAdapterAdvancedProperty -Name ([Management.Automation.WildcardPattern]::Escape($a)) -AllProperties -ErrorAction Stop); if(-not $p){{ throw 'no properties read' }}; foreach($k in {}){{ $q=@($p | Where-Object {{ $_.RegistryKeyword -eq $k }}); if($q){{ $r+=[pscustomobject]@{{A=$a;K=$k;V=(@($q.RegistryValue) -join ',');D=(@($q.DefaultRegistryValue) -join ',');L=(@($q.ValidRegistryValues) -join ',');X=(@($q.NumericParameterMaxValue) -join ',');S=(@($q.NumericParameterStepValue) -join ',')}} }} }} }}; ConvertTo-Json -InputObject @($r) -Compress",
            ps_list(adapters),
            ps_list(kws)
        );
        read_back_rows(&powershell(&*self.cmd, &script)?)
    }

    /// `set` hides per-value failures, so a revert reads the values back. An empty original can't be compared.
    fn check_restored(&self, restore: &[Found]) -> Result<()> {
        let want: Vec<&Found> = restore.iter().filter(|f| !f.2.is_empty()).collect();
        if want.is_empty() {
            return Ok(());
        }
        let rows = self.strict_rows(&unique(want.iter().map(|f| &f.0)), &unique(want.iter().map(|f| &f.1)))?;
        let failed: Vec<String> = want
            .iter()
            .filter(|(a, k, v)| rows.iter().any(|r| r.adapter == *a && r.keyword == *k && r.value.as_deref() != Some(v.as_str())))
            .map(|(a, k, v)| format!("{a}: {k} was not set back to {v}"))
            .collect();
        if failed.is_empty() {
            Ok(())
        } else {
            Err(Error::Revert(failed))
        }
    }

    /// The value this tweak sets on the row's keyword; None when it cannot be worked out.
    fn target(&self, r: &PropRow) -> Option<String> {
        match &self.targets.iter().find(|t| t.0 == r.keyword)?.1 {
            Some(v) => Some(v.clone()),
            None => doubled(r),
        }
    }

    /// The fixed target of a keyword, None for a doubled one.
    fn fixed(&self, kw: &str) -> Option<&str> {
        self.targets.iter().find(|t| t.0 == kw).and_then(|t| t.1.as_deref())
    }

    /// The driver default to write back: only while the tweak's value is still set and the default differs.
    fn default_change(&self, r: &PropRow) -> Option<String> {
        let v = r.value.as_deref()?;
        let d = r.default.as_deref().filter(|d| !d.is_empty() && *d != v)?;
        (self.target(r).as_deref() == Some(v)).then(|| d.to_string())
    }

    /// Off without a backup: every keyword at the target goes back to the driver's default.
    /// False when there is none to write (default unknown or equal to the target), nothing touched.
    fn defaults(&self) -> Result<bool> {
        let rows = self.strict_rows(&tweak_adapters_strict(&*self.cmd)?, &self.keywords())?;
        let changes: Vec<Found> = self.settable(rows).iter().filter_map(|r| self.default_change(r).map(|d| (r.adapter.clone(), r.keyword.clone(), d))).collect();
        self.set(&changes)?;
        self.check_restored(&changes)?;
        Ok(!changes.is_empty())
    }

    /// `defaults` for a stored backup without originals. An error when the tweak would still read as on,
    /// so the switch never reports an Off that did not happen.
    fn off_without_originals(&self) -> Result<()> {
        if self.defaults()? {
            return Ok(());
        }
        let rows = self.settable(self.strict_rows(&tweak_adapters_strict(&*self.cmd)?, &self.keywords())?);
        if !rows.is_empty() && rows.iter().all(|r| self.target(r).as_deref() == r.value.as_deref()) {
            return Err(Error::Revert(vec![format!("{}: your driver has no other default for these settings, so they stay as they are", self.title)]));
        }
        Ok(())
    }

    /// A keyword saved with no value ran on the driver default (`-RegistryValue` refuses an empty string).
    fn empty_originals(&self, empty: &[Found]) -> Result<Vec<Found>> {
        let rows = self.strict_rows(&unique(empty.iter().map(|f| &f.0)), &unique(empty.iter().map(|f| &f.1)))?;
        let mut out = Vec::new();
        for (a, k, _) in empty {
            let Some(r) = rows.iter().find(|r| r.adapter == *a && r.keyword == *k) else { continue };
            match self.default_change(r) {
                Some(d) => out.push((a.clone(), k.clone(), d)),
                None => log::info(&format!("revert {}: {a} {k} has no stored value and no driver default to use, left as it is", self.id)),
            }
        }
        Ok(out)
    }

    /// Name-only entries whose adapter is gone. Success only if no unknown adapter still holds this tweak's value,
    /// because that adapter could be the same card under a new name.
    fn maybe_renamed(&self, saved: &[Found], l: &Located) -> Result<Vec<String>> {
        let lost: Vec<&Found> = saved.iter().filter(|f| l.unknown.contains(&f.0)).collect();
        let rows = self.strict_rows(&l.others, &unique(lost.iter().map(|f| &f.1)))?;
        let mut out = Vec::new();
        for (a, k, _) in lost {
            match rows.iter().find(|r| r.keyword == *k && self.target(r).as_deref() == r.value.as_deref()) {
                Some(r) => out.push(format!("{a}: adapter not found, and {} has {k} at this tweak's value. Rename it back to {a}, or turn this on and off again", r.adapter)),
                None => out.push(adapter_gone(a)),
            }
        }
        Ok(out)
    }

    fn set(&self, changes: &[Found]) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        let mut lines: Vec<String> = changes
            .iter()
            .map(|(a, k, v)| {
                let (n, k, v) = (ps_name(a), ps_quote(k), ps_quote(v));
                // -RegistryKeyword on Set is a wildcard (`*EEE` also matches `AdvancedEEE`), so only exact rows are piped to it.
                format!("Get-NetAdapterAdvancedProperty -Name {n} -AllProperties -RegistryKeyword {k} -ErrorAction SilentlyContinue | Where-Object {{ $_.RegistryKeyword -eq {k} }} | Set-NetAdapterAdvancedProperty -RegistryValue {v} -NoRestart -ErrorAction SilentlyContinue")
            })
            .collect();
        lines.push("$f=@()".into());
        let mut seen = Vec::new();
        for (a, _, _) in changes {
            if !seen.contains(a) {
                seen.push(a.clone());
                lines.push(restart_script(a));
            }
        }
        lines.push("if($f){ throw ('could not restart: ' + ($f -join ', ')) }".into());
        powershell(&*self.cmd, &lines.join(";")).map(|_| ())
    }

    /// Reason this PC can't use the tweak, or None.
    pub fn unavailable(&self) -> Result<Option<String>> {
        let adapters = tweak_adapters(&*self.cmd)?;
        if adapters.is_empty() {
            return Ok(Some(NO_ADAPTER.into()));
        }
        if !self.found(&adapters)?.is_empty() {
            return Ok(None);
        }
        Ok(Some("Your network driver doesn't expose these settings.".into()))
    }
}

impl Tweak for AdapterPropertyTweak {
    fn id(&self) -> &str {
        &self.id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn is_applied(&self) -> Result<bool> {
        let found = self.found(&tweak_adapters(&*self.cmd)?)?;
        Ok(!found.is_empty() && found.iter().all(|r| self.target(r).as_deref() == r.value.as_deref()))
    }
    fn capture(&self, b: &mut TweakBackup) -> Result<()> {
        for r in self.found(&tweak_adapters(&*self.cmd)?)? {
            let v = r.value.clone().unwrap_or_default();
            if self.target(&r).is_some_and(|t| t != v) {
                b.data.entry(format!("{}|{}", r.adapter, r.keyword)).or_insert(v);
            }
        }
        // Also on a re-apply, so an adapter added since counts as known to this backup.
        if !b.data.is_empty() {
            remember_ids(&*self.cmd, b);
        }
        Ok(())
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        let mut changes = Vec::new();
        let found = self.found(&tweak_adapters(&*self.cmd)?)?;
        // Nothing to set is a failure, not a silent success: the switch would report on and read off.
        if found.is_empty() {
            return Err(Error::Plain(NO_SETTING.into()));
        }
        for r in found {
            let t = self.target(&r);
            let v = r.value.unwrap_or_default();
            // Only a value this changes is an original worth keeping.
            if let Some(t) = t.filter(|t| *t != v) {
                b.data.entry(format!("{}|{}", r.adapter, r.keyword)).or_insert(v);
                changes.push((r.adapter, r.keyword, t));
            }
        }
        self.set(&changes)
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        let saved: Vec<Found> = b
            .data
            .iter()
            .filter(|(k, _)| !is_id_key(k))
            .filter_map(|(key, v)| {
                let (a, k) = key.rsplit_once('|')?;
                // Older backups also stored values that were already at the target: not an original.
                (self.fixed(k) != Some(v.as_str())).then(|| (a.to_string(), k.to_string(), v.clone()))
            })
            .collect();
        // A backup without entries is the no-backup case, so one Off really turns it off.
        if b.data.keys().all(|k| is_id_key(k)) {
            return self.off_without_originals();
        }
        // Older backups whose values were all at the target: the user had this value, so it stays.
        if saved.is_empty() {
            log::info(&format!("revert {}: the backup holds no original value, nothing changed", self.id));
            return Ok(());
        }
        let l = locate(b, &unique(saved.iter().map(|f| &f.0)), &adapter_ids(&*self.cmd)?);
        let restore = saved.iter().filter_map(|(a, k, v)| l.now.get(a).map(|n| (n.clone(), k.clone(), v.clone())));
        let (mut settable, empty): (Vec<Found>, Vec<Found>) = restore.partition(|f| !f.2.is_empty());
        settable.extend(self.empty_originals(&empty)?);
        self.set(&settable)?;
        self.check_restored(&settable)?;
        finish_revert(self.maybe_renamed(&saved, &l)?, &l)
    }
    fn revert_default(&self) -> Result<bool> {
        self.defaults()
    }
}

pub const BINDING_COMPONENTS: [&str; 3] = ["ms_lltdio", "ms_rspndr", "ms_lldp"];

#[derive(Deserialize)]
struct BindRow {
    #[serde(rename = "A")]
    adapter: String,
    #[serde(rename = "C")]
    component: String,
    #[serde(rename = "E")]
    enabled: bool,
}

/// Unbinds LLDP / LLTD protocols from the adapters. Data `adapter|component` = "1" if it was enabled.
pub struct ProtocolBindingsTweak {
    pub cmd: Arc<dyn Cmd>,
}

impl ProtocolBindingsTweak {
    fn read(&self) -> Result<Vec<BindRow>> {
        let adapters = tweak_adapters(&*self.cmd)?;
        if adapters.is_empty() {
            return Ok(vec![]);
        }
        let comps: Vec<String> = BINDING_COMPONENTS.iter().map(|s| s.to_string()).collect();
        let script = format!(
            "$r=@(); foreach($a in {}){{ Get-NetAdapterBinding -Name ([Management.Automation.WildcardPattern]::Escape($a)) -ErrorAction SilentlyContinue | Where-Object {{ {} -contains $_.ComponentID }} | ForEach-Object {{ $r+=[pscustomobject]@{{A=$a;C=$_.ComponentID;E=[bool]$_.Enabled}} }} }}; ConvertTo-Json -InputObject @($r) -Compress",
            ps_list(&adapters),
            ps_list(&comps)
        );
        json_rows(&powershell(&*self.cmd, &script)?)
    }

    /// A failed query is an error, because the failure that stopped the enable can also empty a lenient read.
    /// Only an adapter that is really gone gives no row.
    fn strict_read(&self, names: &[String]) -> Result<Vec<BindRow>> {
        if names.is_empty() {
            return Ok(vec![]);
        }
        let comps: Vec<String> = BINDING_COMPONENTS.iter().map(|s| s.to_string()).collect();
        let script = format!(
            "$all=@(Get-NetAdapter -Name * -ErrorAction Stop | ForEach-Object Name); if(-not $all){{ throw 'no adapters read' }}; $r=@(); foreach($a in {}){{ if($all -notcontains $a){{ continue }}; $b=@(Get-NetAdapterBinding -Name ([Management.Automation.WildcardPattern]::Escape($a)) -ErrorAction Stop); if(-not $b){{ throw 'no bindings read' }}; $b | Where-Object {{ {} -contains $_.ComponentID }} | ForEach-Object {{ $r+=[pscustomobject]@{{A=$a;C=$_.ComponentID;E=[bool]$_.Enabled}} }} }}; ConvertTo-Json -InputObject @($r) -Compress",
            ps_list(names),
            ps_list(&comps)
        );
        read_back_rows(&powershell(&*self.cmd, &script)?)
    }

    fn enable(&self, pairs: &[(String, String)]) -> Result<()> {
        if pairs.is_empty() {
            return Ok(());
        }
        let lines: Vec<String> = pairs
            .iter()
            .map(|(a, c)| format!("Get-NetAdapterBinding -Name {} -ComponentID {} -ErrorAction SilentlyContinue | Enable-NetAdapterBinding -ErrorAction SilentlyContinue", ps_name(a), ps_quote(c)))
            .collect();
        powershell(&*self.cmd, &lines.join(";")).map(|_| ())
    }

    /// `enable` hides failures of all but the last pair, so the result is read back.
    fn check_enabled(&self, on: &[(String, String)]) -> Result<()> {
        let rows = self.strict_read(&unique(on.iter().map(|p| &p.0)))?;
        let failed: Vec<String> = on
            .iter()
            .filter(|(a, c)| rows.iter().any(|r| r.adapter == *a && r.component.eq_ignore_ascii_case(c) && !r.enabled))
            .map(|(a, c)| format!("{a}: {c} is still unbound"))
            .collect();
        if failed.is_empty() {
            Ok(())
        } else {
            Err(Error::Revert(failed))
        }
    }

    /// Off without a backup: Windows binds all three, so every unbound one is bound again.
    fn defaults(&self) -> Result<bool> {
        let rows = self.strict_read(&tweak_adapters_strict(&*self.cmd)?)?;
        let off: Vec<(String, String)> = rows.into_iter().filter(|r| !r.enabled).map(|r| (r.adapter, r.component)).collect();
        self.enable(&off)?;
        self.check_enabled(&off)?;
        Ok(!off.is_empty())
    }
}

impl Tweak for ProtocolBindingsTweak {
    fn id(&self) -> &str {
        "net.bindings"
    }
    fn title(&self) -> &str {
        "Unbind unused protocols"
    }
    fn is_applied(&self) -> Result<bool> {
        let rows = self.read()?;
        Ok(!rows.is_empty() && rows.iter().all(|r| !r.enabled))
    }
    fn capture(&self, b: &mut TweakBackup) -> Result<()> {
        for r in self.read()?.into_iter().filter(|r| r.enabled) {
            b.data.entry(format!("{}|{}", r.adapter, r.component)).or_insert_with(|| "1".into());
        }
        if !b.data.is_empty() {
            remember_ids(&*self.cmd, b);
        }
        Ok(())
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        let mut lines = Vec::new();
        for r in self.read()? {
            if r.enabled {
                b.data.entry(format!("{}|{}", r.adapter, r.component)).or_insert_with(|| "1".into());
                lines.push(format!("Get-NetAdapterBinding -Name {} -ComponentID {} | Disable-NetAdapterBinding", ps_name(&r.adapter), ps_quote(&r.component)));
            }
        }
        if lines.is_empty() {
            return Ok(());
        }
        powershell(&*self.cmd, &lines.join(";")).map(|_| ())
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        let saved: Vec<(String, String)> = b
            .data
            .iter()
            .filter(|(k, v)| !is_id_key(k) && v.as_str() == "1")
            .filter_map(|(k, _)| k.rsplit_once('|'))
            .map(|(a, c)| (a.to_string(), c.to_string()))
            .collect();
        // A backup without entries is the no-backup case, so one Off really turns it off.
        if b.data.keys().all(|k| is_id_key(k)) {
            return self.defaults().map(|_| ());
        }
        if saved.is_empty() {
            log::info("revert net.bindings: the backup holds no bound protocol, nothing changed");
            return Ok(());
        }
        let l = locate(b, &unique(saved.iter().map(|p| &p.0)), &adapter_ids(&*self.cmd)?);
        let on: Vec<(String, String)> = saved.iter().filter_map(|(a, c)| l.now.get(a).map(|n| (n.clone(), c.clone()))).collect();
        self.enable(&on)?;
        self.check_enabled(&on)?;
        let lost: Vec<&(String, String)> = saved.iter().filter(|p| l.unknown.contains(&p.0)).collect();
        let rows = if lost.is_empty() { vec![] } else { self.strict_read(&l.others)? };
        let mut unsure = Vec::new();
        for (a, c) in lost {
            match rows.iter().find(|r| r.component.eq_ignore_ascii_case(c) && !r.enabled) {
                Some(r) => unsure.push(format!("{a}: adapter not found, and {} has {c} unbound. Rename it back to {a}, or turn this on and off again", r.adapter)),
                None => unsure.push(adapter_gone(a)),
            }
        }
        finish_revert(unsure, &l)
    }
    fn revert_default(&self) -> Result<bool> {
        self.defaults()
    }
}

/// The catalog instances of `AdapterPropertyTweak`.
pub fn adapter_tweaks(cmd: Arc<dyn Cmd>) -> Vec<AdapterPropertyTweak> {
    let to = |id: &str, title: &str, kws: &[&str], v: Option<&str>| AdapterPropertyTweak {
        cmd: cmd.clone(),
        id: id.into(),
        title: title.into(),
        targets: kws.iter().map(|k| (k.to_string(), v.map(str::to_string))).collect(),
    };
    let t = |id: &str, title: &str, kws: &[&str]| to(id, title, kws, Some("0"));
    vec![
        t("net.adapterpower", "Adapter power management off", &["*DeviceSleepOnDisconnect", "*SelectiveSuspend", "ReduceSpeedOnPowerDown", "ULPMode"]),
        t(
            "net.adapteradvanced",
            "Adapter latency tuning",
            &["*InterruptModeration", "*FlowControl", "*EEE", "EEELinkAdvertisement", "AdvancedEEE", "EnableGreenEthernet", "GreenEthernet"],
        ),
        // Intel values: band 4 = prefer 5 + 6 GHz, roaming 0 = lowest. shortcut: a driver without value 4 keeps its band setting, add a 5 GHz fallback if such drivers matter.
        AdapterPropertyTweak {
            cmd: cmd.clone(),
            id: "net.wifi".into(),
            title: "Wi-Fi latency tuning".into(),
            targets: vec![("RoamingPreferredBandType".into(), Some("4".into())), ("RoamAggressiveness".into(), Some("0".into()))],
        },
        // Removed from the app (many drivers publish no limits); kept so "Revert all" can restore it.
        to("net.buffers", "Larger adapter buffers", &["*ReceiveBuffers", "*TransmitBuffers"], None),
    ]
}

// ---------------------------------------------------------------- MSI mode

/// MSI mode for the present PCI display / network devices (`adv.msi`). The device list is read once here.
pub fn msi_tweak(cmd: &dyn Cmd, hooks: Arc<dyn Hooks>) -> Result<DataTweak> {
    let out = powershell(cmd, "Get-PnpDevice -PresentOnly -Class Display,Net -Status OK | Where-Object InstanceId -like 'PCI\\*' | ForEach-Object InstanceId")?;
    Ok(msi_from(out.lines().map(str::trim).filter(|l| !l.is_empty()), hooks, |path| reg::get(Hive::Hklm, path, "MSISupported")))
}

/// `adv.msi` when the device list could not be read: it changes nothing, but its backup can still be reverted.
pub fn msi_without_devices(hooks: Arc<dyn Hooks>) -> DataTweak {
    msi_from(std::iter::empty(), hooks, |_| Ok(None))
}

/// Only devices whose MSI key already exists (`only_if_key_exists`, it is never created), and never one whose
/// driver set `MSISupported=0`: such a device may not boot in MSI mode. An unreadable value counts as 0.
fn msi_from<'a>(devices: impl Iterator<Item = &'a str>, hooks: Arc<dyn Hooks>, read: impl Fn(&str) -> Result<Option<RegValue>>) -> DataTweak {
    DataTweak {
        id: "adv.msi".into(),
        title: "MSI mode for GPU and network adapter".into(),
        description: String::new(),
        details: String::new(),
        flags: TweakFlags(TweakFlags::RESTART | TweakFlags::EXPERIMENTAL),
        removed: false,
        sets: devices
            .map(|d| format!("SYSTEM\\CurrentControlSet\\Enum\\{d}\\Device Parameters\\Interrupt Management\\MessageSignaledInterruptProperties"))
            .filter(|path| !matches!(read(path), Ok(Some(RegValue::Dword(0))) | Err(_)))
            .map(|path| DataSet {
                hive: Hive::Hklm,
                path,
                name: "MSISupported".into(),
                value: RegValue::Dword(1),
                only_if_key_exists: true,
                default: None, // the driver's INF may set it: unknown
            })
            .collect(),
        after: After::None,
        hooks,
    }
}

// ---------------------------------------------------------------- DirectX user preferences

const DX_KEY: &str = "Software\\Microsoft\\DirectX\\UserGpuPreferences";
const DX_VALUE: &str = "DirectXUserGlobalSettings";

/// Edits one `Name=Value;` entry of the DirectXUserGlobalSettings string. Data `old:<Name>` = previous value.
pub struct DirectXTweak {
    pub id: String,
    pub title: String,
    pub tokens: Vec<(String, String)>,
    pub hive: Hive,
    pub key: String,
}

impl DirectXTweak {
    pub fn catalog() -> Vec<DirectXTweak> {
        let t = |id: &str, title: &str, n: &str, v: &str| DirectXTweak {
            id: id.into(),
            title: title.into(),
            tokens: vec![(n.into(), v.into())],
            hive: Hive::Hkcu,
            key: DX_KEY.into(),
        };
        vec![
            t("adv.windowed", "Optimizations for windowed games", "SwapEffectUpgradeEnable", "1"),
            t("game.vrr", "Variable refresh rate for windowed games", "VRROptimizeEnable", "1"),
            t("game.autohdr", "Auto HDR", "AutoHDREnable", "2097"),
        ]
    }

    /// The `;`-separated entries as they are, so an entry MyTune cannot parse is written back unchanged.
    fn read(&self) -> Result<Vec<String>> {
        match reg::get(self.hive, &self.key, DX_VALUE)? {
            Some(RegValue::String(s)) => Ok(dx_tokens(&s)),
            None => Ok(vec![]),
            Some(_) => Err(Error::Plain(format!("{DX_VALUE} is not a text value, nothing was changed"))),
        }
    }

    fn write(&self, list: &[String]) -> Result<()> {
        if list.is_empty() {
            reg::delete_value(self.hive, &self.key, DX_VALUE)
        } else {
            let s: String = list.iter().map(|t| format!("{t};")).collect();
            reg::set(self.hive, &self.key, DX_VALUE, &RegValue::String(s))
        }
    }

    /// `cur` with this tweak's entries back at their originals. An older backup saved the whole value: only this
    /// tweak's entries are taken from it, so the other two DirectX tweaks keep theirs.
    fn reverted(&self, mut cur: Vec<String>, b: &TweakBackup) -> Vec<String> {
        let whole = b.registry.iter().find(|r| r.name.eq_ignore_ascii_case(DX_VALUE)).map(|r| dx_tokens(r.value.as_deref().unwrap_or("")));
        for (n, _) in &self.tokens {
            let old = match &whole {
                Some(list) => dx_get(list, n).map(String::from),
                None => match b.data.get(&format!("old:{n}")) {
                    Some(v) => Some(v.clone()).filter(|v| !v.is_empty()),
                    None => continue, // original unknown: leave the entry alone
                },
            };
            dx_drop(&mut cur, n);
            if let Some(v) = old {
                put(&mut cur, n, &v);
            }
        }
        cur
    }
}

fn dx_tokens(raw: &str) -> Vec<String> {
    raw.split(';').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect()
}

fn dx_get<'a>(list: &'a [String], name: &str) -> Option<&'a str> {
    list.iter().find_map(|t| t.split_once('=').filter(|(n, _)| *n == name).map(|(_, v)| v))
}

fn dx_drop(list: &mut Vec<String>, name: &str) {
    list.retain(|t| t.split_once('=').is_none_or(|(n, _)| n != name));
}

fn put(list: &mut Vec<String>, name: &str, value: &str) {
    dx_drop(list, name);
    list.push(format!("{name}={value}"));
}

impl Tweak for DirectXTweak {
    fn id(&self) -> &str {
        &self.id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn is_applied(&self) -> Result<bool> {
        let cur = self.read()?;
        Ok(self.tokens.iter().all(|(n, v)| dx_get(&cur, n) == Some(v.as_str())))
    }
    fn capture(&self, b: &mut TweakBackup) -> Result<()> {
        let cur = self.read()?;
        for (n, _) in &self.tokens {
            b.data.entry(format!("old:{n}")).or_insert_with(|| dx_get(&cur, n).unwrap_or_default().to_string());
        }
        Ok(())
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        let mut cur = self.read()?;
        for (n, v) in &self.tokens {
            b.data.entry(format!("old:{n}")).or_insert_with(|| dx_get(&cur, n).unwrap_or_default().to_string());
            put(&mut cur, n, v);
        }
        self.write(&cur)
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        // An old whole-value backup of a non-text value holds no entries to merge: only the exact value can go back.
        if b.registry.iter().any(|r| r.name.eq_ignore_ascii_case(DX_VALUE) && r.existed && r.kind.as_deref() != Some("String")) {
            return b.restore_registry();
        }
        self.write(&self.reverted(self.read()?, b))
    }
}

// ---------------------------------------------------------------- services from the catalog

/// `deb.xbox`, `deb.spooler` (active) and `adv.search`, `adv.sysmain`, `adv.services` (Removed, revert only).
pub fn service_tweaks(cmd: Arc<dyn Cmd>) -> Vec<ServicesManualTweak> {
    let t = |id: &str, title: &str, names: &[&str]| ServicesManualTweak::system(cmd.clone(), id, title, names);
    vec![
        t("deb.xbox", "Xbox services to Manual", &["XblAuthManager", "XblGameSave", "XboxGipSvc", "XboxNetApiSvc"]),
        t("deb.spooler", "Print Spooler to Manual", &["Spooler"]),
        t("adv.search", "Search indexing to Manual", &["WSearch"]),
        t("adv.sysmain", "SysMain (Superfetch) to Manual", &["SysMain"]),
        t("adv.services", "Set unneeded services to Manual", &DEFAULT_SERVICES),
    ]
}

// ---------------------------------------------------------------- scheduled tasks

pub trait TaskApi: Send + Sync {
    /// `Some(enabled)` or `None` if the task does not exist.
    fn is_enabled(&self, path: &str) -> Result<Option<bool>>;
    fn set_enabled(&self, path: &str, enabled: bool) -> Result<()>;
}

/// Task Scheduler COM (`ITaskService`), so it works on any Windows language.
pub struct ComTaskApi;

impl ComTaskApi {
    fn find(path: &str) -> Result<Option<windows::Win32::System::TaskScheduler::IRegisteredTask>> {
        use windows::core::BSTR;
        use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
        use windows::Win32::System::TaskScheduler::{ITaskService, TaskScheduler};
        use windows::Win32::System::Variant::VARIANT;
        let com = |e: windows::core::Error| Error::Catalog(format!("task scheduler: {e}"));
        let i = path.rfind('\\').unwrap_or(0);
        let (folder, name) = if i == 0 { ("\\", path.trim_start_matches('\\')) } else { (&path[..i], &path[i + 1..]) };
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED); // S_FALSE if this thread already did
            let svc: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER).map_err(com)?;
            svc.Connect(&VARIANT::default(), &VARIANT::default(), &VARIANT::default(), &VARIANT::default()).map_err(com)?;
            let not_found = |e: &windows::core::Error| matches!(e.code().0 as u32, 0x8007_0002 | 0x8007_0003);
            let f = match svc.GetFolder(&BSTR::from(folder)) {
                Ok(f) => f,
                Err(e) if not_found(&e) => return Ok(None),
                Err(e) => return Err(com(e)),
            };
            match f.GetTask(&BSTR::from(name)) {
                Ok(t) => Ok(Some(t)),
                Err(e) if not_found(&e) => Ok(None),
                Err(e) => Err(com(e)),
            }
        }
    }
}

impl TaskApi for ComTaskApi {
    fn is_enabled(&self, path: &str) -> Result<Option<bool>> {
        match Self::find(path)? {
            None => Ok(None),
            Some(t) => unsafe { t.Enabled().map(|b| Some(b.as_bool())).map_err(|e| Error::Catalog(format!("task scheduler: {e}"))) },
        }
    }
    fn set_enabled(&self, path: &str, enabled: bool) -> Result<()> {
        use windows::Win32::Foundation::VARIANT_BOOL;
        let t = Self::find(path)?.ok_or_else(|| Error::Catalog(format!("task {path} not found")))?;
        unsafe { t.SetEnabled(VARIANT_BOOL(if enabled { -1 } else { 0 })).map_err(|e| Error::Catalog(format!("task scheduler: {e}"))) }
    }
}

pub const TELEMETRY_TASKS: [&str; 8] = [
    "\\Microsoft\\Windows\\Application Experience\\Microsoft Compatibility Appraiser",
    "\\Microsoft\\Windows\\Application Experience\\ProgramDataUpdater",
    "\\Microsoft\\Windows\\Customer Experience Improvement Program\\Consolidator",
    "\\Microsoft\\Windows\\Customer Experience Improvement Program\\UsbCeip",
    "\\Microsoft\\Windows\\Autochk\\Proxy",
    "\\Microsoft\\Windows\\Feedback\\Siuf\\DmClient",
    "\\Microsoft\\Windows\\Feedback\\Siuf\\DmClientOnScenarioDownload",
    "\\Microsoft\\Windows\\Windows Error Reporting\\QueueReporting",
];

/// Disables scheduled tasks. Data: task path = "1" for a task this tweak disabled, "0" for one noted by `capture`
/// just before (a crash in between leaves "0": revert enables both).
pub struct ScheduledTasksTweak {
    pub api: Arc<dyn TaskApi>,
    pub id: String,
    pub title: String,
    pub tasks: Vec<String>,
}

impl ScheduledTasksTweak {
    pub fn telemetry(api: Arc<dyn TaskApi>) -> ScheduledTasksTweak {
        ScheduledTasksTweak {
            api,
            id: "deb.tasks".into(),
            title: "Disable telemetry scheduled tasks".into(),
            tasks: TELEMETRY_TASKS.iter().map(|s| s.to_string()).collect(),
        }
    }

    pub fn unavailable(&self) -> Result<Option<String>> {
        for t in &self.tasks {
            if self.api.is_enabled(t)?.is_some() {
                return Ok(None);
            }
        }
        Ok(Some("These tasks don't exist on this PC.".into()))
    }
}

impl Tweak for ScheduledTasksTweak {
    fn id(&self) -> &str {
        &self.id
    }
    fn title(&self) -> &str {
        &self.title
    }
    /// True when at least one listed task exists and none is still enabled. (C# also returns true if a backup exists.)
    fn is_applied(&self) -> Result<bool> {
        let mut any = false;
        for t in &self.tasks {
            match self.api.is_enabled(t)? {
                Some(true) => return Ok(false),
                Some(false) => any = true,
                None => {}
            }
        }
        Ok(any)
    }
    fn capture(&self, b: &mut TweakBackup) -> Result<()> {
        for t in &self.tasks {
            if self.api.is_enabled(t)? == Some(true) {
                b.data.entry(t.clone()).or_insert_with(|| "0".into());
            }
        }
        Ok(())
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        let (mut done, mut failed) = (Vec::new(), Vec::new());
        for t in &self.tasks {
            if self.api.is_enabled(t)? != Some(true) {
                continue; // missing or already disabled: leave alone
            }
            match self.api.set_enabled(t, false) {
                Ok(()) => done.push(t),
                Err(e) => {
                    log::error(&format!("disable task {t}"), Some(&e));
                    failed.push((t, e.to_string()));
                }
            }
        }
        if failed.is_empty() {
            for t in done {
                b.data.insert(t.clone(), "1".into());
            }
            return Ok(());
        }
        // One switch, one state: undo this run's changes and name the task that refused.
        for t in done {
            match self.api.set_enabled(t, true) {
                Ok(()) if b.data.get(t).is_some_and(|v| v == "0") => drop(b.data.remove(t)),
                Ok(()) => {}
                Err(e) => {
                    log::error(&format!("enable task {t} again"), Some(&e));
                    b.data.insert(t.clone(), "1".into());
                }
            }
        }
        for (t, _) in &failed {
            if b.data.get(*t).is_some_and(|v| v == "0") {
                b.data.remove(*t);
            }
        }
        let list: Vec<String> = failed.iter().map(|(t, e)| format!("{t}: {e}")).collect();
        Err(Error::Plain(format!("could not disable {}", list.join("; "))))
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        let mut failed = Vec::new();
        for t in b.data.keys() {
            match self.api.is_enabled(t) {
                Ok(None) => continue, // task removed since
                Ok(Some(_)) => {}
                Err(e) => {
                    failed.push(format!("{t}: {e}"));
                    continue;
                }
            }
            if let Err(e) = self.api.set_enabled(t, true) {
                failed.push(format!("{t}: {e}"));
            }
        }
        if failed.is_empty() {
            Ok(())
        } else {
            Err(Error::Revert(failed))
        }
    }
}

// ---------------------------------------------------------------- revert-only (Removed) tweaks

pub(crate) fn removed_apply(id: &str) -> Error {
    Error::Catalog(format!("{id} was removed from MyTune; it can only be reverted"))
}

/// Old `net.dns`: put DNS servers back. Data `ns:<adapter>` = previous servers, comma separated.
pub struct DnsRevert {
    pub cmd: Arc<dyn Cmd>,
}

impl Tweak for DnsRevert {
    fn id(&self) -> &str {
        "net.dns"
    }
    fn title(&self) -> &str {
        "Fastest DNS (removed)"
    }
    fn is_applied(&self) -> Result<bool> {
        Ok(false)
    }
    fn apply(&self, _: &mut TweakBackup) -> Result<()> {
        Err(removed_apply("net.dns"))
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        let saved: Vec<(&str, &str)> = b.data.iter().filter_map(|(k, v)| k.strip_prefix("ns:").map(|a| (a, v.as_str()))).collect();
        let here = if saved.is_empty() { vec![] } else { adapter_ids(&*self.cmd)? };
        let mut lines = vec!["$f=@()".to_string()];
        let mut l = Located::default();
        for (a, prev) in saved {
            if !here.iter().any(|r| r.name.eq_ignore_ascii_case(a)) {
                l.missing.push(a.to_string());
                continue;
            }
            let set = if prev.trim().is_empty() {
                format!("Set-DnsClientServerAddress -InterfaceAlias {} -ResetServerAddresses", ps_name(a))
            } else {
                let servers: Vec<String> = prev.split(',').filter(|s| !s.is_empty()).map(String::from).collect();
                format!("Set-DnsClientServerAddress -InterfaceAlias {} -ServerAddresses {}", ps_name(a), ps_list(&servers))
            };
            lines.push(format!("try {{ {set} }} catch {{ $f+={} }}", ps_quote(a)));
        }
        lines.push("Clear-DnsClientCache".into());
        lines.push("if($f){ throw ('DNS not restored on: ' + ($f -join ', ')) }".into());
        powershell(&*self.cmd, &lines.join(";"))?;
        finish_revert(Vec::new(), &l)
    }
}

/// Old `net.tcp`: restore autotuning / ECN / RSS / RSC from Data `at`, `ecn`, `rss`, `rsc`.
pub struct TcpStackRevert {
    pub cmd: Arc<dyn Cmd>,
}

impl Tweak for TcpStackRevert {
    fn id(&self) -> &str {
        "net.tcp"
    }
    fn title(&self) -> &str {
        "TCP stack (removed)"
    }
    fn is_applied(&self) -> Result<bool> {
        Ok(false)
    }
    fn apply(&self, _: &mut TweakBackup) -> Result<()> {
        Err(removed_apply("net.tcp"))
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        // A field an old backup lacks has no original to restore: it must not block the others forever.
        let get = |k: &str| {
            let v = b.data.get(k).map(|v| v.trim().to_lowercase()).filter(|v| !v.is_empty());
            if v.is_none() {
                log::info(&format!("revert net.tcp: the backup has no `{k}`, left as it is"));
            }
            v
        };
        let ecn = get("ecn").map(|v| if v == "enabled" || v == "disabled" { v } else { "default".into() });
        let netsh = system32("netsh.exe").to_string_lossy().into_owned();
        let mut failed = Vec::new();
        for (name, v) in [("autotuninglevel", get("at")), ("ecncapability", ecn), ("rss", get("rss")), ("rsc", get("rsc"))] {
            let Some(v) = v else { continue };
            let arg = format!("{name}={v}");
            match self.cmd.run(&netsh, &["int", "tcp", "set", "global", &arg]) {
                Ok(r) if r.exit_code == 0 => {}
                Ok(r) => failed.push(format!("netsh {arg} exited with {}: {}", r.exit_code, r.output.trim())),
                Err(e) => failed.push(format!("netsh {arg}: {e}")),
            }
        }
        if failed.is_empty() {
            Ok(())
        } else {
            Err(Error::Revert(failed))
        }
    }
}

/// Old `mem.compression`: turn memory compression back on (and start SysMain) unless the backup says it was off.
pub struct MemoryCompressionRevert {
    pub cmd: Arc<dyn Cmd>,
}

impl Tweak for MemoryCompressionRevert {
    fn id(&self) -> &str {
        "mem.compression"
    }
    fn title(&self) -> &str {
        "Memory compression (removed)"
    }
    fn is_applied(&self) -> Result<bool> {
        Ok(false)
    }
    fn apply(&self, _: &mut TweakBackup) -> Result<()> {
        Err(removed_apply("mem.compression"))
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        // shortcut: the C# key name is lost, so any key naming compression with an "off" value counts; use the real key if an old state.json shows it.
        let was_off = b.data.iter().any(|(k, v)| k.to_lowercase().contains("compress") && matches!(v.trim().to_lowercase().as_str(), "false" | "0" | "off" | "disabled"));
        if was_off {
            log::info("revert mem.compression: it was off before, left off");
            return Ok(());
        }
        powershell(&*self.cmd, "Start-Service SysMain -ErrorAction SilentlyContinue;Enable-MMAgent -MemoryCompression").map(|_| ())
    }
}

/// uBlock Origin Lite force-installed in Chrome and Edge through their ExtensionInstallForcelist policy.
/// Brave is left out on purpose (Shields already blocks ads); Opera ignores these policies.
pub struct AdBlockTweak;

const ADBLOCK: [(&str, &str); 2] = [
    (r"SOFTWARE\Policies\Google\Chrome\ExtensionInstallForcelist", "ddkjiahejlhfcafbddmgiahcphecmpfh;https://clients2.google.com/service/update2/crx"),
    (r"SOFTWARE\Policies\Microsoft\Edge\ExtensionInstallForcelist", "cimighlppcgcoapaliogpjjdehbnofhn;https://edge.microsoft.com/extensionwebstorebase/v1/crx"),
];

/// Value name holding `entry`, or the first free one. Browsers read the list as 1, 2, 3... and stop at the
/// first missing number, so a new entry must fill the first gap, never overwrite the user's own entries.
fn adblock_slot(path: &str, entry: &str) -> Result<(String, bool)> {
    for i in 1u32.. {
        let name = i.to_string();
        match reg::get(Hive::Hklm, path, &name)? {
            None => return Ok((name, false)),
            Some(RegValue::String(s)) if s.eq_ignore_ascii_case(entry) => return Ok((name, true)),
            Some(_) => {}
        }
    }
    unreachable!()
}

/// Deletes slot `n` and moves every later entry down one: browsers stop reading at the first missing number.
fn close_slot(n: u32, get: impl Fn(&str) -> Result<Option<RegValue>>, set: impl Fn(&str, &RegValue) -> Result<()>, del: impl Fn(&str) -> Result<()>) -> Result<()> {
    del(&n.to_string())?;
    for i in n + 1.. {
        let Some(v) = get(&i.to_string())? else { break };
        set(&(i - 1).to_string(), &v)?;
        del(&i.to_string())?;
    }
    Ok(())
}

/// Removes this tweak's entry from its slot. A slot now holding something else is the user's and stays;
/// an empty one is a retry after a move that failed, so the gap is still closed.
fn adblock_remove(path: &str, entry: &str, n: u32) -> Result<()> {
    match reg::get(Hive::Hklm, path, &n.to_string())? {
        Some(RegValue::String(s)) if s.eq_ignore_ascii_case(entry) => {}
        None => {}
        Some(_) => return Ok(()),
    }
    close_slot(n, |i| reg::get(Hive::Hklm, path, i), |i, v| reg::set(Hive::Hklm, path, i, v), |i| reg::delete_value(Hive::Hklm, path, i))
}

impl Tweak for AdBlockTweak {
    fn id(&self) -> &str {
        "cust.adblock"
    }
    fn title(&self) -> &str {
        "Block ads in Chrome and Edge"
    }
    fn is_applied(&self) -> Result<bool> {
        for (path, entry) in ADBLOCK {
            if !adblock_slot(path, entry)?.1 {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn capture(&self, backup: &mut TweakBackup) -> Result<()> {
        for (path, entry) in ADBLOCK {
            let (name, there) = adblock_slot(path, entry)?;
            if !there {
                backup.capture(Hive::Hklm, path, &name)?;
            }
        }
        Ok(())
    }
    fn apply(&self, backup: &mut TweakBackup) -> Result<()> {
        for (path, entry) in ADBLOCK {
            let (name, there) = adblock_slot(path, entry)?;
            if !there {
                backup.capture(Hive::Hklm, path, &name)?;
                reg::set(Hive::Hklm, path, &name, &RegValue::String(entry.into()))?;
            }
        }
        Ok(())
    }
    // No `revert_default`: without a backup the entry is the user's own, so it is never removed.
    fn revert(&self, backup: &TweakBackup) -> Result<()> {
        let mut slots: Vec<_> = backup.registry.iter().collect();
        // Highest first: closing a lower slot would move a higher one this backup also holds.
        slots.sort_by_key(|r| std::cmp::Reverse(r.name.parse::<u32>().unwrap_or(0)));
        let mut failed = Vec::new();
        for r in slots {
            let ours = ADBLOCK.iter().find(|(p, _)| p.eq_ignore_ascii_case(&r.path)).zip(r.name.parse::<u32>().ok()).filter(|_| !r.existed);
            let done = match ours {
                Some((&(path, entry), n)) => adblock_remove(path, entry, n).and_then(|()| r.created_keys.iter().rev().try_for_each(|k| reg::delete_key_if_empty(Hive::Hklm, k).map(|_| ()))),
                None => r.restore(),
            };
            if let Err(e) = done {
                failed.push(format!("{}\\{}: {e}", r.path, r.name));
            }
        }
        if failed.is_empty() {
            Ok(())
        } else {
            Err(Error::Revert(failed))
        }
    }
}

/// `cust.cleanstart`: the catalog's Start policies plus the pin file `start2.bin`, which has no registry form.
pub struct CleanStartTweak {
    reg: DataTweak,
    cmd: Arc<dyn Cmd>,
}

/// Saves the pin file. `fresh` = the backup has no `start2` key yet, so a `start2.bin.bak` left on disk belongs to
/// no backup and moves to `start2.bin.old` (its old pins must not come back, but it may be the only copy). With the
/// key, a second apply must never replace the user's real pins with an emptied file. Copied under another name first,
/// so a copy that broke half way is not taken for a backup later.
fn save_pins(pins: &std::path::Path, bak: &std::path::Path, fresh: bool) -> Result<()> {
    if fresh || !bak.exists() {
        let tmp = bak.with_extension("tmp");
        std::fs::copy(pins, &tmp)?;
        if bak.exists() {
            std::fs::rename(bak, bak.with_extension("old"))?;
        }
        std::fs::rename(&tmp, bak)?;
    }
    Ok(())
}

/// The Start menu process comes back by itself and can hold the file for a moment: tries for 2 s.
fn retry_locked<T>(f: impl Fn() -> std::io::Result<T>) -> Result<T> {
    for _ in 0..10 {
        if let Ok(v) = f() {
            return Ok(v);
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    Ok(f()?)
}

/// Puts the saved pins back. The backup goes only after the copy worked, so a failed revert can be retried.
fn restore_pins(pins: &std::path::Path, bak: &std::path::Path) -> Result<()> {
    retry_locked(|| std::fs::copy(bak, pins))?;
    Ok(std::fs::remove_file(bak)?)
}

impl CleanStartTweak {
    pub fn new(mut reg: DataTweak, cmd: Arc<dyn Cmd>) -> CleanStartTweak {
        // Explorer restarts once, after the pin file, not right after the registry values.
        reg.flags = TweakFlags::default();
        CleanStartTweak { reg, cmd }
    }

    fn paths() -> (std::path::PathBuf, std::path::PathBuf) {
        let local = std::path::PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap_or_default());
        (local.join(r"Packages\Microsoft.Windows.StartMenuExperienceHost_cw5n1h2txyewy\LocalState\start2.bin"), crate::util::default_dir().join("start2.bin.bak"))
    }

    /// Fails when it was not running: fine.
    fn stop_start_menu(&self) {
        let _ = self.cmd.run(&system32("taskkill.exe").to_string_lossy(), &["/F", "/IM", "StartMenuExperienceHost.exe"]);
    }

    /// `saved`: the backup has the `start2` key, so the file on disk is this backup's copy.
    fn put_pins_back(&self, saved: bool) -> Result<()> {
        let (pins, bak) = Self::paths();
        if saved && bak.exists() {
            self.stop_start_menu();
            restore_pins(&pins, &bak)?;
        }
        self.reg.restart_explorer();
        Ok(())
    }

    /// Before the first change, so an apply cut off half way still has the pins and the key that ties them.
    fn keep_pins(b: &mut TweakBackup) -> Result<bool> {
        let (pins, bak) = Self::paths();
        if !pins.exists() {
            return Ok(false);
        }
        save_pins(&pins, &bak, !b.data.contains_key("start2"))?;
        b.data.insert("start2".into(), "saved".into());
        Ok(true)
    }
}

impl Tweak for CleanStartTweak {
    fn id(&self) -> &str {
        &self.reg.id
    }
    fn title(&self) -> &str {
        &self.reg.title
    }
    fn is_applied(&self) -> Result<bool> {
        self.reg.is_applied()
    }
    fn capture(&self, b: &mut TweakBackup) -> Result<()> {
        self.reg.capture(b)?;
        Self::keep_pins(b).map(|_| ())
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        self.reg.apply(b)?;
        if Self::keep_pins(b)? {
            let (pins, _) = Self::paths();
            self.stop_start_menu();
            retry_locked(|| std::fs::remove_file(&pins))?;
        }
        self.reg.restart_explorer();
        Ok(())
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        b.restore_registry()?;
        self.put_pins_back(b.data.contains_key("start2"))
    }
    fn revert_default(&self) -> Result<bool> {
        let known = self.reg.revert_default()?;
        self.put_pins_back(false)?;
        Ok(known)
    }
    fn on_applied(&self, s: &mut crate::store::Store) -> Result<()> {
        self.reg.on_applied(s)
    }
    fn on_reverted(&self, s: &mut crate::store::Store) -> Result<()> {
        self.reg.on_reverted(s)
    }
}

// ---------------------------------------------------------------- Take ownership (right-click entry)

const TAKE_OWN_VERB: &str = "MyTuneTakeOwn";
/// The cmd.exe entry of older versions: it never elevated under UAC, and old installs still have it.
const TAKE_OWN_OLD_VERB: &str = "MyTuneTakeOwnership";
const ADMINISTRATORS: &str = "*S-1-5-32-544";

fn take_own_keys(verb: &str) -> [String; 2] {
    ["SOFTWARE\\Classes\\*\\shell", "SOFTWARE\\Classes\\Directory\\shell"].map(|p| format!("{p}\\{verb}"))
}

/// (key, value name, data) of the entry. MyTune's manifest requires admin, so the shell shows the UAC prompt.
/// No cmd or PowerShell in between: a file name is never parsed by a shell.
fn take_own_values(exe: &str) -> Vec<(String, &'static str, String)> {
    take_own_keys(TAKE_OWN_VERB)
        .into_iter()
        .flat_map(|k| {
            let command = format!("{k}\\command");
            [(k.clone(), "MUIVerb", "Take ownership".to_string()), (k, "HasLUAShield", String::new()), (command, "", format!("\"{exe}\" --take-own \"%1\""))]
        })
        .collect()
}

/// The programs `--take-own` runs on one item. SIDs and no `/d` answer, so nothing depends on the Windows language.
/// `/l` makes icacls change a link itself; a link is never given to takeown, which would follow it.
fn take_own_args(path: &str, is_link: bool) -> Vec<(&'static str, Vec<String>)> {
    let grant = format!("{ADMINISTRATORS}:F");
    let icacls = |rest: &[&str]| ("icacls.exe", std::iter::once(path).chain(rest.iter().copied()).map(String::from).collect::<Vec<_>>());
    if is_link {
        vec![icacls(&["/setowner", ADMINISTRATORS, "/l", "/q"]), icacls(&["/grant", &grant, "/l", "/q"])]
    } else {
        vec![("takeown.exe", vec!["/f".to_string(), path.to_string(), "/a".to_string()]), icacls(&["/grant", &grant, "/l", "/q"])]
    }
}

/// Any reparse point: symlink, junction, mount point or cloud placeholder.
fn is_reparse(m: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    m.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT
}

/// Runs the steps on `root` and everything under it. Not icacls `/t`: it follows junctions out of the folder.
/// A folder is changed before it is read, because missing access is often why ownership is needed.
/// Returns what failed; one locked item never stops the rest.
fn take_own_walk(root: &std::path::Path, run: &mut dyn FnMut(&'static str, &[String]) -> std::result::Result<(), String>) -> Vec<String> {
    let mut failed = Vec::new();
    let mut todo = vec![root.to_path_buf()];
    while let Some(p) = todo.pop() {
        let shown = p.display().to_string();
        // a lossy name could be a different file
        let Some(path) = p.to_str() else {
            failed.push(format!("{shown}: the name has characters MyTune cannot pass on"));
            continue;
        };
        let meta = match std::fs::symlink_metadata(&p) {
            Ok(m) => m,
            Err(e) => {
                failed.push(format!("{shown}: {e}"));
                continue;
            }
        };
        let link = is_reparse(&meta);
        for (exe, args) in take_own_args(path, link) {
            if let Err(e) = run(exe, &args) {
                failed.push(format!("{shown}: {exe} {e}"));
            }
        }
        if meta.is_dir() && !link {
            match std::fs::read_dir(&p) {
                Ok(items) => {
                    for item in items {
                        match item {
                            Ok(i) => todo.push(i.path()),
                            Err(e) => failed.push(format!("{shown}: {e}")),
                        }
                    }
                }
                Err(e) => failed.push(format!("{shown}: {e}")),
            }
        }
    }
    failed
}

/// Full path in lower case with no `\\?\` and no trailing separator, so one folder compares equal however it is written.
fn take_own_norm(p: &str) -> String {
    let p = p.replace('/', "\\");
    let low = p.to_ascii_lowercase();
    let p = if low.starts_with(r"\\?\unc\") {
        format!(r"\\{}", &p[8..])
    } else if (low.starts_with(r"\\?\") || low.starts_with(r"\\.\")) && p.as_bytes().get(5) == Some(&b':') {
        p[4..].to_string()
    } else {
        p
    };
    let p = if p.ends_with(':') { format!("{p}\\") } else { p };
    let full = std::path::absolute(&p).map_or(p, |f| f.to_string_lossy().into_owned());
    full.trim_end_matches('\\').to_lowercase()
}

/// A drive or share root, a Windows or program folder, the Users folder or one user's profile.
fn take_own_protected(path: &str, env: impl Fn(&str) -> Option<String>) -> bool {
    let p = take_own_norm(path);
    if (p.len() == 2 && p.ends_with(':')) || p.strip_prefix(r"\\").is_some_and(|r| r.split('\\').count() <= 2) {
        return true;
    }
    let vars = ["SystemRoot", "windir", "ProgramFiles", "ProgramFiles(x86)", "ProgramW6432", "ProgramData", "USERPROFILE"];
    let fixed = ["C:\\Windows", "C:\\Program Files", "C:\\Program Files (x86)", "C:\\ProgramData", "C:\\Users"];
    if vars.iter().filter_map(|k| env(k)).filter(|v| !v.is_empty()).any(|v| take_own_norm(&v) == p) || fixed.iter().any(|v| take_own_norm(v) == p) {
        return true;
    }
    let mut users = vec![take_own_norm("C:\\Users")];
    users.extend(env("SystemDrive").filter(|d| !d.is_empty()).map(|d| take_own_norm(&format!("{d}\\Users"))));
    users.extend(env("USERPROFILE").and_then(|u| take_own_norm(&u).rsplit_once('\\').map(|(parent, _)| parent.to_string())));
    users.contains(&p) || p.rsplit_once('\\').is_some_and(|(parent, _)| users.iter().any(|u| u == parent))
}

/// The Windows folder or anything under it, at any depth.
fn take_own_in_windows(path: &str, env: impl Fn(&str) -> Option<String>) -> bool {
    let p = take_own_norm(path);
    ["SystemRoot", "windir"].iter().filter_map(|k| env(k)).filter(|v| !v.is_empty()).chain(["C:\\Windows".to_string()]).any(|w| {
        let w = take_own_norm(&w);
        p == w || p.strip_prefix(w.as_str()).is_some_and(|rest| rest.starts_with('\\'))
    })
}

/// Where the item really is. A link is changed itself, so its own location counts and a broken target does not matter.
fn take_own_real(path: &std::path::Path, is_link: bool) -> std::io::Result<std::path::PathBuf> {
    if !is_link {
        return std::fs::canonicalize(path);
    }
    let (Some(parent), Some(name)) = (path.parent(), path.file_name()) else {
        return Err(std::io::Error::other("it has no parent folder"));
    };
    Ok(std::fs::canonicalize(parent)?.join(name))
}

/// Refuses before anything changes. Both the given and the real path count (`C:\PROGRA~1`, a junction in the path).
/// A single file under Windows stays allowed; a location that cannot be confirmed is refused.
fn take_own_guard(path: &str, real: std::io::Result<std::path::PathBuf>, is_folder: bool, env: impl Fn(&str) -> Option<String>) -> Result<()> {
    let real = real.map_err(|e| Error::Plain(format!("MyTune could not confirm where {path} really is, so nothing was changed: {e}")))?;
    let real = real.to_string_lossy();
    if is_folder && [path, &*real].iter().any(|p| take_own_protected(p, &env) || take_own_in_windows(p, &env)) {
        return Err(Error::Plain(format!("{path} is a protected system folder, so MyTune does not change it. Inside Windows, choose a single file instead.")));
    }
    Ok(())
}

/// `"C:\"` on a command line arrives as `C:"`, because the backslash escapes the closing quote.
fn take_own_path(raw: &str) -> String {
    let p = raw.trim_end_matches('"');
    if p.ends_with(':') { format!("{p}\\") } else { p.to_string() }
}

fn take_own(raw: &str) -> Result<()> {
    let path = take_own_path(raw);
    if !std::path::Path::new(&path).is_absolute() {
        return Err(Error::Plain(format!("{path} is not a full path")));
    }
    let meta = std::fs::symlink_metadata(&path)?;
    // by attribute: `is_dir` is false for a junction, which is a folder here too
    let is_folder = std::os::windows::fs::MetadataExt::file_attributes(&meta) & 0x10 != 0; // FILE_ATTRIBUTE_DIRECTORY
    take_own_guard(&path, take_own_real(std::path::Path::new(&path), is_reparse(&meta)), is_folder, |k| std::env::var(k).ok())?;
    let failed = take_own_walk(std::path::Path::new(&path), &mut |exe, args| {
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        match crate::process::run_with(&system32(exe).to_string_lossy(), &args, std::time::Duration::from_secs(120), None) {
            Ok(r) if r.exit_code == 0 => Ok(()),
            Ok(r) => Err(format!("failed ({}): {}", r.exit_code, r.output.trim().chars().take(200).collect::<String>())),
            Err(e) => Err(e.to_string()),
        }
    });
    if failed.is_empty() {
        return Ok(());
    }
    for f in &failed {
        log::error(f, None);
    }
    let mut text = format!("{} item(s) could not be changed:\n{}", failed.len(), failed[..failed.len().min(10)].join("\n"));
    if failed.len() > 10 {
        text.push_str(&format!("\nand {} more, see the log", failed.len() - 10));
    }
    Err(Error::Plain(text))
}

/// True when this start is the right-click entry: it did its work, and MyTune must not open.
/// Silent on success, a message box with the real error on failure.
pub fn take_own_from_args() -> bool {
    use windows::core::PCWSTR;
    use windows::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONERROR, MB_OK};
    let mut args = std::env::args_os().skip(1);
    if args.next().is_none_or(|a| a != "--take-own") {
        return false;
    }
    let result = match (args.next(), args.next()) {
        (Some(p), None) => p.to_str().ok_or_else(|| Error::Plain("this path has characters MyTune cannot pass on".into())).and_then(take_own),
        _ => Err(Error::Plain("expected one file or folder".into())),
    };
    if let Err(e) = result {
        log::error("take ownership failed", Some(&e));
        let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
        let (text, title) = (wide(&format!("Could not take ownership.\n\n{e}")), wide("MyTune: Take ownership"));
        unsafe { MessageBoxW(None, PCWSTR(text.as_ptr()), PCWSTR(title.as_ptr()), MB_OK | MB_ICONERROR) };
    }
    true
}

/// `qol.takeown`. Its originals are "the entry was not there", so off deletes the old and new keys.
pub struct TakeOwnTweak;

impl TakeOwnTweak {
    fn present() -> Result<Vec<String>> {
        let mut found = Vec::new();
        for k in take_own_keys(TAKE_OWN_VERB).into_iter().chain(take_own_keys(TAKE_OWN_OLD_VERB)) {
            if reg::key_exists(Hive::Hklm, &k)? {
                found.push(k);
            }
        }
        Ok(found)
    }

    fn remove() -> Result<()> {
        let failed: Vec<String> = take_own_keys(TAKE_OWN_VERB)
            .into_iter()
            .chain(take_own_keys(TAKE_OWN_OLD_VERB))
            .filter_map(|k| reg::delete_tree(Hive::Hklm, &k).err().map(|e| format!("{k}: {e}")))
            .collect();
        if failed.is_empty() {
            Ok(())
        } else {
            Err(Error::Revert(failed))
        }
    }
}

impl Tweak for TakeOwnTweak {
    fn id(&self) -> &str {
        "qol.takeown"
    }
    fn title(&self) -> &str {
        "Add \"Take ownership\" to the right-click menu"
    }
    fn is_applied(&self) -> Result<bool> {
        Ok(!Self::present()?.is_empty())
    }
    /// Which keys were there, so the engine can tell whether a failed apply changed anything.
    fn capture(&self, b: &mut TweakBackup) -> Result<()> {
        if !b.data.contains_key("verbs") {
            b.data.insert("verbs".into(), Self::present()?.join("|"));
        }
        Ok(())
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        self.capture(b)?;
        let exe = std::env::current_exe()?;
        let exe = exe.to_str().ok_or_else(|| Error::Plain("MyTune's folder has characters a right-click command cannot hold".into()))?;
        for (key, name, value) in take_own_values(exe) {
            reg::set(Hive::Hklm, &key, name, &RegValue::String(value))?;
        }
        // Last, so a failed write leaves the old entry in place.
        for k in take_own_keys(TAKE_OWN_OLD_VERB) {
            reg::delete_tree(Hive::Hklm, &k)?;
        }
        Ok(())
    }
    fn revert(&self, _b: &TweakBackup) -> Result<()> {
        Self::remove()
    }
    fn revert_default(&self) -> Result<bool> {
        if Self::present()?.is_empty() {
            return Ok(false);
        }
        Self::remove()?;
        Ok(true)
    }
}

#[allow(dead_code)]
fn _unused(_: BTreeMap<String, String>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{Engine, RevertOutcome};
    use crate::process::RunResult;
    use crate::store::Store;
    use crate::util::b64_decode;
    use std::sync::Mutex;

    type Handler = Box<dyn Fn(&str) -> Option<String> + Send + Sync>;

    /// Fake runner. PowerShell calls are decoded, so the handler sees the script text.
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
        fn run(&self, file: &str, args: &[&str]) -> Result<RunResult> {
            let stem = std::path::Path::new(file).file_stem().unwrap().to_string_lossy().to_lowercase();
            let line = if stem == "powershell" {
                let b = b64_decode(args.last().unwrap()).unwrap();
                let u: Vec<u16> = b.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
                let s = String::from_utf16(&u).unwrap();
                format!("ps {}", s.trim_start_matches("$ProgressPreference='SilentlyContinue';$ErrorActionPreference='Stop';"))
            } else {
                format!("{stem} {}", args.join(" "))
            };
            self.calls.lock().unwrap().push(line.clone());
            let output = (self.handler)(&line).unwrap_or_else(|| if line.contains("InterfaceGuid") { IDS_JSON.to_string() } else { String::new() });
            Ok(RunResult { exit_code: 0, output })
        }
    }

    fn engine() -> (tempfile::TempDir, Engine) {
        let d = tempfile::tempdir().unwrap();
        let e = Engine::new(Store::load(d.path()));
        (d, e)
    }

    const ADAPTERS_JSON: &str = r#"[{"N":"Ethernet","D":"Realtek PCIe GbE","T":6},{"N":"vEthernet","D":"Hyper-V Virtual","T":6},{"N":"Wi-Fi","D":"Intel AX200","T":71},{"N":"Loop","D":"Loopback","T":24},{"N":"Single","D":null,"T":117}]"#;

    /// What `adapter_ids` reads unless a test's handler answers it.
    const IDS_JSON: &str = r#"[{"N":"Ethernet","D":"Realtek","T":6,"G":"{E}","S":"Up"},{"N":"Wi-Fi","D":"Intel","T":71,"G":"{W}","S":"Up"}]"#;

    struct Scratch(String);
    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let p = format!("Software\\MyTuneTest\\sp-{tag}-{}", std::process::id());
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

    #[test]
    fn adapter_filter_and_quoting() {
        let f = Fake::new(|l| l.contains("Get-NetAdapter -ErrorAction").then(|| ADAPTERS_JSON.to_string()));
        assert_eq!(active_adapters(&*f).unwrap(), vec!["Ethernet", "Wi-Fi", "Single"]);
        assert_eq!(ps_quote("it's"), "'it''s'");
        let f2 = Fake::new(|_| Some(String::new()));
        assert!(active_adapters(&*f2).unwrap().is_empty(), "blank output = no adapters");
    }

    // ------------------------------------------------------------ adapter properties

    /// `extra(keyword)` adds JSON fields to that keyword's rows, e.g. `,"D":"512"`.
    fn adapter_fake(state: Arc<Mutex<Vec<(String, String, String)>>>, extra: fn(&str) -> &'static str) -> Arc<Fake> {
        Fake::new(move |l| {
            if l.contains("Set-NetAdapterAdvancedProperty") {
                for part in l.trim_start_matches("ps ").split(';').filter(|p| p.contains("Set-NetAdapterAdvancedProperty")) {
                    let q: Vec<&str> = part.split('\'').collect(); // [.., adapter, .., kw, .., kw, .., value, ..]
                    let (a, k, v) = (q[1], q[3], q[7]);
                    for row in state.lock().unwrap().iter_mut().filter(|r| r.0 == a && r.1 == k) {
                        row.2 = v.to_string();
                    }
                }
                None
            } else if l.contains("Get-NetAdapter -ErrorAction") {
                Some(r#"[{"N":"Ethernet","D":"Realtek","T":6}]"#.into())
            } else if l.contains("Get-NetAdapterAdvancedProperty") {
                let rows: Vec<String> = state.lock().unwrap().iter().map(|(a, k, v)| format!(r#"{{"A":"{a}","K":"{k}","V":"{v}"{}}}"#, extra(k))).collect();
                Some(format!("[{}]", rows.join(",")))
            } else {
                None
            }
        })
    }

    #[test]
    fn adapter_off_without_backup_writes_driver_default_and_skips_unsettable() {
        // flow control at the target with default 3; *EEE cannot be 0 on this driver
        let state = Arc::new(Mutex::new(vec![
            ("Ethernet".to_string(), "*FlowControl".to_string(), "0".to_string()),
            ("Ethernet".to_string(), "*EEE".to_string(), "1".to_string()),
        ]));
        let fake = adapter_fake(state.clone(), |k| if k == "*FlowControl" { r#","D":"3","L":"0,1,2,3""# } else { r#","D":"1","L":"1,2""# });
        let t = adapter_tweaks(fake.clone()).remove(1);
        assert!(t.is_applied().unwrap(), "a keyword that cannot be 0 does not count");
        let (_d, e) = engine();
        assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted);
        let calls = fake.calls();
        let set = calls.iter().find(|c| c.contains("Set-NetAdapterAdvancedProperty")).unwrap();
        assert!(set.contains("-RegistryKeyword '*FlowControl'") && set.contains("-RegistryValue '3'") && !set.contains("*EEE"), "{set}");
        assert!(set.contains("Where-Object { $_.RegistryKeyword -eq '*FlowControl' } | Set-NetAdapterAdvancedProperty"), "the write takes exact rows only: {set}");
        assert_eq!(state.lock().unwrap()[0].2, "3");
    }

    /// The owner's case: a stored backup with no values.
    fn empty_backup_engine(id: &str) -> (tempfile::TempDir, Engine) {
        let (d, e) = engine();
        e.with_store(|s| s.set_backup(id, TweakBackup::new("a"))).unwrap();
        (d, e)
    }

    fn at_target_flow_control() -> Arc<Fake> {
        Fake::new(|l| {
            if l.contains("Get-NetAdapter -ErrorAction") {
                Some(r#"[{"N":"Ethernet","D":"Intel","T":6}]"#.into())
            } else if l.contains("Get-NetAdapterAdvancedProperty") {
                Some(r#"[{"A":"Ethernet","K":"*FlowControl","V":"0","D":"3","L":"0,1,2,3"}]"#.into())
            } else {
                None
            }
        })
    }

    #[test]
    fn adapter_empty_backup_goes_to_the_driver_default_in_one_off() {
        let state = Arc::new(Mutex::new(vec![("Ethernet".to_string(), "*FlowControl".to_string(), "0".to_string())]));
        let t = adapter_tweaks(adapter_fake(state.clone(), |_| r#","D":"3","L":"0,1,2,3""#)).remove(1);
        let (_d, e) = empty_backup_engine("net.adapteradvanced");
        assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted);
        assert_eq!(state.lock().unwrap()[0].2, "3");
        assert!(e.with_store(|s| s.get_backup("net.adapteradvanced").is_none()));
    }

    #[test]
    fn adapter_empty_backup_is_kept_when_the_off_cannot_happen() {
        // the read fails
        let failed_read = Fake::new(|l| {
            if l.contains("$all=") {
                Some("read failed".into())
            } else if l.contains("Get-NetAdapter -ErrorAction") {
                Some(r#"[{"N":"Ethernet","D":"Intel","T":6}]"#.into())
            } else {
                None
            }
        });
        let t = adapter_tweaks(failed_read.clone()).remove(1);
        let (_d, e) = empty_backup_engine("net.adapteradvanced");
        assert!(e.revert(&t).is_err());
        assert!(e.with_store(|s| s.get_backup("net.adapteradvanced").is_some()));
        assert!(!failed_read.calls().iter().any(|c| c.contains("Set-NetAdapterAdvancedProperty")));
        // the driver's default is the tweak's value: it would still read as on
        let state = Arc::new(Mutex::new(vec![("Ethernet".to_string(), "*FlowControl".to_string(), "0".to_string())]));
        let t = adapter_tweaks(adapter_fake(state, |_| r#","D":"0","L":"0,1""#)).remove(1);
        let (_d, e) = empty_backup_engine("net.adapteradvanced");
        assert!(matches!(e.revert(&t), Err(Error::Revert(_))));
        assert!(e.with_store(|s| s.get_backup("net.adapteradvanced").is_some()));
    }

    #[test]
    fn adapter_old_backup_all_at_target_writes_no_defaults() {
        let fake = at_target_flow_control();
        let t = adapter_tweaks(fake.clone()).remove(1);
        let (_d, e) = engine();
        let mut b = TweakBackup::new("a");
        b.data.insert("Ethernet|*FlowControl".into(), "0".into());
        e.with_store(|s| s.set_backup("net.adapteradvanced", b)).unwrap();
        e.revert(&t).unwrap();
        assert!(!fake.calls().iter().any(|c| c.contains("Set-NetAdapterAdvancedProperty")));
    }

    #[test]
    fn adapter_default_refused_is_an_error() {
        let t = adapter_tweaks(at_target_flow_control()).remove(1);
        let (_d, e) = engine();
        assert!(matches!(e.revert(&t), Err(Error::Revert(_))), "no backup: the driver default did not stick");
    }

    #[test]
    fn adapter_default_needs_a_strict_read() {
        let fake = Fake::new(|l| {
            if l.contains("$all=") {
                Some("read failed".into())
            } else if l.contains("Get-NetAdapter -ErrorAction") {
                Some(r#"[{"N":"Ethernet","D":"Intel","T":6}]"#.into())
            } else {
                None
            }
        });
        let t = adapter_tweaks(fake.clone()).remove(1);
        assert!(t.revert_default().is_err());
        assert!(!fake.calls().iter().any(|c| c.contains("Set-NetAdapterAdvancedProperty")));
    }

    #[test]
    fn adapter_failed_read_back_keeps_backup() {
        let fake = Fake::new(|l| {
            if l.contains("$all=") {
                Some("read failed".into())
            } else {
                None
            }
        });
        let t = adapter_tweaks(fake).remove(1);
        let (_d, e) = engine();
        let mut b = TweakBackup::new("a");
        b.data.insert("Ethernet|*FlowControl".into(), "3".into());
        e.with_store(|s| s.set_backup("net.adapteradvanced", b)).unwrap();
        assert!(e.revert(&t).is_err());
        assert!(e.with_store(|s| s.get_backup("net.adapteradvanced").is_some()));
    }

    /// Rows for whatever adapter the strict read asks about, with `value` on *FlowControl.
    fn flow_control_on_all(value: &'static str) -> Arc<Fake> {
        Fake::new(move |l| {
            (l.contains("Get-NetAdapterAdvancedProperty") && !l.contains("Set-NetAdapterAdvancedProperty")).then(|| {
                let rows: Vec<String> = ["Ethernet", "Wi-Fi"].iter().filter(|a| l.contains(&format!("'{a}'"))).map(|a| format!(r#"{{"A":"{a}","K":"*FlowControl","V":"{value}","D":"3"}}"#)).collect();
                format!("[{}]", rows.join(","))
            })
        })
    }

    #[test]
    fn adapter_renamed_is_found_by_its_id() {
        let fake = flow_control_on_all("3");
        let t = adapter_tweaks(fake.clone()).remove(1);
        let (_d, e) = engine();
        let mut b = TweakBackup::new("a");
        b.data.insert("Old name|*FlowControl".into(), "3".into());
        b.data.insert("guid:Old name".into(), "{e}".into()); // the case of a GUID does not matter
        e.with_store(|s| s.set_backup("net.adapteradvanced", b)).unwrap();
        e.revert(&t).unwrap();
        let calls = fake.calls();
        let set = calls.iter().find(|c| c.contains("Set-NetAdapterAdvancedProperty")).unwrap();
        assert!(set.contains("Escape('Ethernet')") && !set.contains("Old name"), "{set}");
    }

    #[test]
    fn adapter_unplugged_restores_the_others_and_keeps_the_backup() {
        let fake = flow_control_on_all("3");
        let t = adapter_tweaks(fake.clone()).remove(1);
        let (_d, e) = engine();
        let mut b = TweakBackup::new("a");
        b.data.insert("Gone|*FlowControl".into(), "1".into());
        b.data.insert("guid:Gone".into(), "{GONE}".into());
        b.data.insert("Wi-Fi|*FlowControl".into(), "3".into());
        e.with_store(|s| s.set_backup("net.adapteradvanced", b)).unwrap();
        assert!(matches!(e.revert(&t), Err(ref x @ Error::Revert(ref f)) if only_missing_adapters(x) && f.len() == 1 && f[0].starts_with("Gone ")));
        assert!(e.with_store(|s| s.get_backup("net.adapteradvanced").is_some_and(|b| b.data.contains_key("Gone|*FlowControl"))));
        let calls = fake.calls();
        let set = calls.iter().find(|c| c.contains("Set-NetAdapterAdvancedProperty")).unwrap();
        assert!(set.contains("Escape('Wi-Fi')") && !set.contains("Gone"), "{set}");
    }

    #[test]
    fn binding_unplugged_keeps_the_backup() {
        let fake = Fake::new(|l| l.contains("Get-NetAdapterBinding").then(|| r#"[{"A":"Ethernet","C":"ms_lldp","E":true}]"#.to_string()));
        let t = ProtocolBindingsTweak { cmd: fake.clone() };
        let (_d, e) = engine();
        let mut b = TweakBackup::new("b");
        b.data.insert("Dock|ms_lldp".into(), "1".into());
        b.data.insert("guid:Dock".into(), "{GONE}".into());
        b.data.insert("Ethernet|ms_lldp".into(), "1".into());
        e.with_store(|s| s.set_backup("net.bindings", b)).unwrap();
        assert!(matches!(e.revert(&t), Err(ref x) if only_missing_adapters(x)));
        assert!(e.with_store(|s| s.get_backup("net.bindings").is_some()));
        assert!(fake.calls().iter().any(|c| c.contains("Enable-NetAdapterBinding") && c.contains("Escape('Ethernet')")));
    }

    #[test]
    fn adapter_old_name_only_backup_never_reports_a_false_success() {
        let backup = || {
            let mut b = TweakBackup::new("a");
            b.data.insert("Old name|*FlowControl".into(), "3".into());
            b
        };
        // another adapter still holds the tweak's value: it may be the same card, renamed
        let t = adapter_tweaks(flow_control_on_all("0")).remove(1);
        let (_d, e) = engine();
        e.with_store(|s| s.set_backup("net.adapteradvanced", backup())).unwrap();
        assert!(matches!(e.revert(&t), Err(Error::Revert(ref f)) if f[0].contains("Old name")));
        assert!(e.with_store(|s| s.get_backup("net.adapteradvanced").is_some()));
        // nothing holds it: not connected now, so the original is kept until the user says it is gone
        let t = adapter_tweaks(flow_control_on_all("3")).remove(1);
        let (_d, e) = engine();
        e.with_store(|s| s.set_backup("net.adapteradvanced", backup())).unwrap();
        assert!(matches!(e.revert(&t), Err(ref x) if only_missing_adapters(x)));
        assert!(e.with_store(|s| s.get_backup("net.adapteradvanced").is_some()));
    }

    #[test]
    fn adapter_backup_stores_ids_additively() {
        let state = Arc::new(Mutex::new(vec![("Ethernet".to_string(), "*SelectiveSuspend".to_string(), "1".to_string())]));
        let t = adapter_tweaks(adapter_fake(state, |_| "")).remove(0);
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        let b = e.with_store(|s| s.get_backup("net.adapterpower").cloned()).unwrap();
        assert_eq!(b.data.get("Ethernet|*SelectiveSuspend").map(String::as_str), Some("1"));
        assert_eq!(b.data.get("guid:Ethernet").map(String::as_str), Some("{E}"));
    }

    #[test]
    fn restart_failure_is_reported_and_later_adapters_still_restart() {
        let fake = Fake::new(|_| None);
        let t = adapter_tweaks(fake.clone()).remove(1);
        t.set(&[("A[1]".into(), "*EEE".into(), "0".into()), ("B".into(), "*EEE".into(), "0".into())]).unwrap();
        let script = fake.calls().remove(0);
        assert_eq!(script.matches("try {").count(), 2, "{script}");
        assert!(script.contains("Escape('A[1]')") && script.contains("catch { $f+='A[1]' }"), "{script}");
        assert!(script.trim_end().ends_with("if($f){ throw ('could not restart: ' + ($f -join ', ')) }"), "{script}");
    }

    #[test]
    fn adapter_empty_original_goes_back_to_the_driver_default() {
        let fake = at_target_flow_control();
        let t = adapter_tweaks(fake.clone()).remove(1);
        let (_d, e) = engine();
        let mut b = TweakBackup::new("a");
        b.data.insert("Ethernet|*FlowControl".into(), String::new());
        e.with_store(|s| s.set_backup("net.adapteradvanced", b)).unwrap();
        let _ = e.revert(&t); // this fake never changes the value, so the read-back fails
        let calls = fake.calls();
        let set = calls.iter().find(|c| c.contains("Set-NetAdapterAdvancedProperty")).unwrap();
        assert!(set.contains("-RegistryValue '3'"), "{set}");
    }

    #[test]
    fn adapter_property_apply_and_revert() {
        let state = Arc::new(Mutex::new(vec![
            ("Ethernet".to_string(), "*SelectiveSuspend".to_string(), "1".to_string()),
            ("Ethernet".to_string(), "ULPMode".to_string(), "0".to_string()), // already at target
        ]));
        let fake = adapter_fake(state.clone(), |_| "");
        let t = adapter_tweaks(fake.clone()).remove(0);
        assert_eq!(t.id, "net.adapterpower");
        assert!(!t.is_applied().unwrap());
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        assert!(t.is_applied().unwrap());
        assert!(fake.calls().iter().any(|c| c.contains("$n=([Management.Automation.WildcardPattern]::Escape('Ethernet'))") && c.contains("$o | Where-Object Status -ne 'Disabled' | Restart-NetAdapter")));
        assert_eq!(state.lock().unwrap()[0].2, "0");
        assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted);
        assert_eq!(state.lock().unwrap()[0].2, "1", "original value back");
        assert_eq!(state.lock().unwrap()[1].2, "0");
    }

    #[test]
    fn adapter_unavailable_reasons() {
        let none = Fake::new(|l| l.contains("Get-NetAdapter -ErrorAction").then(|| "[]".to_string()));
        assert_eq!(adapter_tweaks(none).remove(0).unavailable().unwrap().as_deref(), Some(NO_ADAPTER));
        let state = Arc::new(Mutex::new(vec![]));
        let no_kw = adapter_fake(state, |_| "");
        assert!(adapter_tweaks(no_kw.clone()).remove(0).unavailable().unwrap().unwrap().contains("driver"));
        let (_d, e) = engine();
        assert!(e.apply(&adapter_tweaks(no_kw).remove(2)).is_err(), "nothing to set must not report success");
    }

    fn prop(d: &str, l: &str, x: &str, s: &str) -> PropRow {
        let o = |v: &str| (!v.is_empty()).then(|| v.to_string());
        PropRow { adapter: "Ethernet".into(), keyword: "*ReceiveBuffers".into(), value: None, default: o(d), valid: o(l), max: o(x), step: o(s) }
    }

    #[test]
    fn doubled_target_respects_driver_limits() {
        assert_eq!(doubled(&prop("512", "", "4096", "8")).as_deref(), Some("1024"));
        assert_eq!(doubled(&prop("512", "", "800", "")).as_deref(), Some("800"), "range max caps");
        assert_eq!(doubled(&prop("80", "", "150", "64")).as_deref(), Some("144"), "rounded down to the step grid");
        assert_eq!(doubled(&prop("256", "128,256,512,1024", "", "")).as_deref(), Some("512"));
        assert_eq!(doubled(&prop("256", "128,256,384,768", "", "")).as_deref(), Some("768"), "smallest listed at or above 2x");
        assert_eq!(doubled(&prop("256", "128,256,384", "", "")).as_deref(), Some("384"), "else the largest listed");
        assert_eq!(doubled(&prop("4096", "", "4096", "8")), None, "already at the maximum");
        assert_eq!(doubled(&prop("512", "256,512", "", "")), None, "largest listed is the default");
        assert_eq!(doubled(&prop("", "", "4096", "")), None, "unknown default");
        assert_eq!(doubled(&prop("Auto", "", "4096", "")), None, "non-numeric default");
        assert_eq!(doubled(&prop("512", "", "", "")), None, "no range and no list");
    }

    #[test]
    fn buffers_apply_doubles_and_reverts() {
        let state = Arc::new(Mutex::new(vec![
            ("Ethernet".to_string(), "*ReceiveBuffers".to_string(), "512".to_string()),
            ("Ethernet".to_string(), "*TransmitBuffers".to_string(), "128".to_string()),
        ]));
        let fake = adapter_fake(state.clone(), |k| if k == "*ReceiveBuffers" { r#","D":"512","X":"4096","S":"8""# } else { r#","D":"128","L":"128""# });
        let t = adapter_tweaks(fake.clone()).remove(3);
        assert_eq!(t.id, "net.buffers");
        assert!(!t.is_applied().unwrap());
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        assert_eq!(state.lock().unwrap()[0].2, "1024");
        assert_eq!(state.lock().unwrap()[1].2, "128", "a driver that allows no more is left alone");
        assert!(t.is_applied().unwrap());
        let b = e.with_store(|s| s.get_backup("net.buffers").cloned()).unwrap();
        assert_eq!(b.data.get("Ethernet|*ReceiveBuffers").map(String::as_str), Some("512"));
        assert!(!b.data.contains_key("Ethernet|*TransmitBuffers"));
        assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted);
        assert_eq!(state.lock().unwrap()[0].2, "512", "original value back");
    }

    // ------------------------------------------------------------ bindings

    #[test]
    fn bindings_only_reenable_what_was_on() {
        let fake = Fake::new(|l| {
            if l.contains("Get-NetAdapter -ErrorAction") {
                Some(r#"[{"N":"Ethernet","D":"Realtek","T":6}]"#.into())
            } else if l.contains("Get-NetAdapterBinding") {
                Some(r#"[{"A":"Ethernet","C":"ms_lltdio","E":true},{"A":"Ethernet","C":"ms_rspndr","E":false},{"A":"Ethernet","C":"ms_lldp","E":true}]"#.into())
            } else {
                None
            }
        });
        let t = ProtocolBindingsTweak { cmd: fake.clone() };
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        let c = fake.calls();
        assert!(c.iter().any(|x| x.contains("Disable-NetAdapterBinding") && x.contains("Escape('Ethernet')) -ComponentID 'ms_lltdio'") && x.contains("'ms_lldp'") && !x.contains("-ComponentID 'ms_rspndr'")));
        e.revert(&t).unwrap();
        let calls = fake.calls();
        let en = calls.iter().find(|c| c.contains("Enable-NetAdapterBinding")).unwrap();
        assert!(en.contains("ms_lltdio") && en.contains("ms_lldp") && !en.contains("-ComponentID 'ms_rspndr'"), "{en}");
    }

    #[test]
    fn binding_empty_backup_binds_again_in_one_off() {
        let on = Arc::new(Mutex::new(false));
        let seen = on.clone();
        let fake = Fake::new(move |l| {
            if l.contains("Enable-NetAdapterBinding") {
                *seen.lock().unwrap() = true;
                None
            } else if l.contains("Get-NetAdapter -ErrorAction") {
                Some(r#"[{"N":"Ethernet","D":"Realtek","T":6}]"#.into())
            } else if l.contains("Get-NetAdapterBinding") {
                Some(format!(r#"[{{"A":"Ethernet","C":"ms_lldp","E":{}}}]"#, seen.lock().unwrap()))
            } else {
                None
            }
        });
        let t = ProtocolBindingsTweak { cmd: fake };
        let (_d, e) = empty_backup_engine("net.bindings");
        assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted);
        assert!(*on.lock().unwrap());
        assert!(e.with_store(|s| s.get_backup("net.bindings").is_none()));
    }

    #[test]
    fn binding_empty_backup_is_kept_when_the_read_fails() {
        let fake = Fake::new(|l| {
            if l.contains("$all=") {
                Some("read failed".into())
            } else if l.contains("Get-NetAdapter -ErrorAction") {
                Some(r#"[{"N":"Ethernet","D":"Realtek","T":6}]"#.into())
            } else {
                None
            }
        });
        let t = ProtocolBindingsTweak { cmd: fake.clone() };
        let (_d, e) = empty_backup_engine("net.bindings");
        assert!(e.revert(&t).is_err());
        assert!(e.with_store(|s| s.get_backup("net.bindings").is_some()));
        assert!(!fake.calls().iter().any(|c| c.contains("Enable-NetAdapterBinding")));
    }

    #[test]
    fn take_own_entry_runs_mytune_without_a_shell() {
        let v = take_own_values("C:\\Program Files\\MyTune\\MyTune.exe");
        assert_eq!(v.len(), 6);
        let cmds: Vec<&(String, &str, String)> = v.iter().filter(|(k, _, _)| k.ends_with("\\command")).collect();
        assert_eq!(cmds.len(), 2);
        assert!(cmds.iter().all(|(_, n, c)| n.is_empty() && c == "\"C:\\Program Files\\MyTune\\MyTune.exe\" --take-own \"%1\""));
        assert!(v.iter().all(|(k, _, _)| k.contains("\\shell\\MyTuneTakeOwn") && !k.contains("MyTuneTakeOwnership")));
    }

    #[test]
    fn take_own_args_use_sids_and_never_follow_links() {
        let p = "C:\\x %windir% & y";
        let run = |l| take_own_args(p, l).into_iter().map(|(e, a)| format!("{e} {}", a.join(" "))).collect::<Vec<_>>();
        assert_eq!(run(false), [format!("takeown.exe /f {p} /a"), format!("icacls.exe {p} /grant *S-1-5-32-544:F /l /q")]);
        let link = run(true);
        assert!(link.iter().all(|c| c.starts_with("icacls.exe") && c.contains(" /l ") && !c.contains(" /t ")), "{link:?}");
        assert!(take_own_args(p, false).iter().all(|(_, a)| a.iter().filter(|x| x.as_str() == p).count() == 1), "the path stays one argument");
        assert_eq!(take_own_path("C:\""), "C:\\");
        assert_eq!(take_own_path("C:\\a b"), "C:\\a b");
    }

    /// A real junction in a scratch folder; the executor is fake, so takeown and icacls never run.
    #[test]
    fn take_own_walk_stops_at_a_junction_and_continues_after_a_failure() {
        let d = tempfile::tempdir().unwrap();
        let (root, outside) = (d.path().join("root"), d.path().join("outside"));
        std::fs::create_dir_all(root.join("sub")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(root.join("a.txt"), "a").unwrap();
        std::fs::write(root.join("sub").join("b.txt"), "b").unwrap();
        std::fs::write(outside.join("secret.txt"), "s").unwrap();
        let link = root.join("link");
        let made = std::process::Command::new(system32("cmd.exe")).args(["/c", "mklink", "/J"]).arg(&link).arg(&outside).output().unwrap();
        assert!(made.status.success(), "{made:?}");

        let mut calls: Vec<(String, Vec<String>)> = Vec::new();
        let failed = take_own_walk(&root, &mut |exe, args| {
            calls.push((exe.to_string(), args.to_vec()));
            if args.iter().any(|a| a.ends_with("a.txt")) && exe == "takeown.exe" { Err("locked".into()) } else { Ok(()) }
        });
        assert_eq!(failed.len(), 1, "{failed:?}");
        assert!(failed[0].contains("a.txt") && failed[0].contains("locked"));
        let on = |p: &std::path::Path, exe: &str| calls.iter().filter(|(e, a)| e == exe && a.iter().any(|x| x == p.to_str().unwrap())).count();
        for item in [root.clone(), root.join("a.txt"), root.join("sub"), root.join("sub").join("b.txt")] {
            assert_eq!((on(&item, "takeown.exe"), on(&item, "icacls.exe")), (1, 1), "{item:?}");
        }
        assert_eq!((on(&link, "takeown.exe"), on(&link, "icacls.exe")), (0, 2), "the link itself, by icacls only");
        assert!(calls.iter().filter(|(e, a)| e == "icacls.exe" && a[0] == link.to_str().unwrap()).all(|(_, a)| a.iter().any(|x| x == "/l")));
        assert!(!calls.iter().any(|(_, a)| a.iter().any(|x| x.contains("secret") || x.contains("outside") || x == "/t")), "{calls:?}");
    }

    #[test]
    fn take_own_refuses_protected_roots_however_they_are_written() {
        let env = |k: &str| match k {
            "SystemRoot" | "windir" => Some("D:\\Win".to_string()),
            "ProgramFiles" | "ProgramW6432" => Some("D:\\PF".to_string()),
            "ProgramFiles(x86)" => Some("D:\\PF86".to_string()),
            "ProgramData" => Some("D:\\Data".to_string()),
            "SystemDrive" => Some("D:".to_string()),
            "USERPROFILE" => Some("E:\\Home\\me".to_string()),
            _ => None,
        };
        for p in [
            "C:\\", "c:/", "\\\\?\\C:\\", "D:\\", "\\\\server\\share\\", "\\\\?\\UNC\\server\\share",
            "C:\\Windows", "c:\\WINDOWS\\", "\\\\?\\C:\\Windows\\", "C:/Windows//", "C:\\Windows\\.", "C:\\x\\..\\Windows",
            "D:\\win", "D:\\PF", "d:\\pf86\\", "D:\\Data", "C:\\Program Files", "C:\\PROGRAM FILES (X86)\\", "C:\\ProgramData",
            "C:\\Users", "D:\\Users\\", "C:\\Users\\someone", "D:\\users\\OTHER\\", "E:\\Home", "e:\\home\\ME",
        ] {
            assert!(take_own_protected(p, env), "{p}");
        }
        for p in ["C:\\Games", "C:\\Users\\me\\Downloads", "C:\\Windows\\Temp\\x", "D:\\Win2", "E:\\Home\\me\\a", "\\\\server\\share\\folder"] {
            assert!(!take_own_protected(p, env), "{p}");
        }
        for p in ["D:\\Win\\System32", "d:\\WIN\\system32\\drivers\\etc\\", "\\\\?\\D:\\Win\\Temp", "C:\\Windows\\System32", "C:/Windows/a/b/c"] {
            assert!(take_own_in_windows(p, env), "{p}");
            assert!(take_own_guard(p, Ok(p.into()), true, env).is_err(), "a folder: {p}");
            assert!(take_own_guard(p, Ok(p.into()), false, env).is_ok(), "a single file: {p}");
        }
        for p in ["D:\\Win2\\x", "D:\\Windows.old", "C:\\Games\\Windows"] {
            assert!(!take_own_in_windows(p, env), "{p}");
            assert!(take_own_guard(p, Ok(p.into()), true, env).is_ok(), "{p}");
        }
        let unconfirmed = || Err(std::io::Error::other("denied"));
        assert!(take_own_guard("C:\\Games", unconfirmed(), true, env).unwrap_err().to_string().contains("could not confirm"));
        assert!(take_own_guard("C:\\Games\\a.txt", unconfirmed(), false, env).is_err(), "fails closed for a file too");
    }

    /// Real junctions in a scratch folder. Only paths are read; nothing is run on them.
    #[test]
    fn take_own_guard_sees_windows_through_a_junction() {
        let d = tempfile::tempdir().unwrap();
        let win = d.path().join("win");
        std::fs::create_dir_all(win.join("sub")).unwrap();
        std::fs::write(win.join("sub").join("f.dll"), "x").unwrap();
        let junction = |link: &std::path::Path, to: &std::path::Path| {
            let made = std::process::Command::new(system32("cmd.exe")).args(["/c", "mklink", "/J"]).arg(link).arg(to).output().unwrap();
            assert!(made.status.success(), "{made:?}");
        };
        let j = d.path().join("j");
        junction(&j, &win);
        let real_win = std::fs::canonicalize(&win).unwrap().to_string_lossy().into_owned();
        let env = move |k: &str| (k == "SystemRoot").then(|| real_win.clone());
        let via = j.join("sub");
        assert!(take_own_guard(via.to_str().unwrap(), take_own_real(&via, false), true, &env).is_err(), "a folder of Windows, reached through a junction");
        let file = via.join("f.dll");
        assert!(take_own_guard(file.to_str().unwrap(), take_own_real(&file, false), false, &env).is_ok(), "one file stays allowed");

        // a broken link is still located by its own parent; the same path as a normal item cannot be confirmed
        let (gone, broken) = (d.path().join("gone"), d.path().join("broken"));
        std::fs::create_dir(&gone).unwrap();
        junction(&broken, &gone);
        std::fs::remove_dir(&gone).unwrap();
        assert!(take_own_real(&broken, true).is_ok());
        assert!(take_own_guard(broken.to_str().unwrap(), take_own_real(&broken, false), true, &env).is_err());
    }

    #[test]
    fn ps_quote_doubles_every_quote_powershell_reads() {
        for q in ['\'', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}'] {
            let s = format!("Ethernet{q};Write-Output INJECTED;{q}");
            let want = format!("'Ethernet{q}{q};Write-Output INJECTED;{q}{q}'");
            assert_eq!(ps_quote(&s), want);
        }
        assert_eq!(ps_name("a\u{2019}b"), "([Management.Automation.WildcardPattern]::Escape('a\u{2019}\u{2019}b'))");
    }

    #[test]
    fn forget_only_drops_a_backup_whose_adapter_is_missing() {
        let backup = |guid: &str| {
            let mut b = TweakBackup::new("a");
            b.data.insert("Dock|*FlowControl".into(), "1".into());
            b.data.insert("guid:Dock".into(), guid.into());
            b
        };
        // still unplugged: forgotten
        let t = adapter_tweaks(flow_control_on_all("3")).remove(1);
        let (_d, e) = engine();
        e.with_store(|s| s.set_backup("net.adapteradvanced", backup("{GONE}"))).unwrap();
        assert!(matches!(e.revert(&t), Err(ref x) if only_missing_adapters(x)));
        assert!(forget_missing_adapter(&e, &t).unwrap());
        assert!(e.with_store(|s| s.get_backup("net.adapteradvanced").is_none()));
        // a read that fails is not "missing": the backup stays
        let t = adapter_tweaks(Fake::new(|l| l.contains("InterfaceGuid").then(String::new))).remove(1);
        let (_d, e) = engine();
        e.with_store(|s| s.set_backup("net.adapteradvanced", backup("{GONE}"))).unwrap();
        assert!(matches!(e.revert(&t), Err(ref x) if !only_missing_adapters(x)));
        assert!(forget_missing_adapter(&e, &t).is_err());
        assert!(e.with_store(|s| s.get_backup("net.adapteradvanced").is_some()));
        // plugged back in: restored, not forgotten
        let fake = Fake::new(|l| {
            if l.contains("Get-NetAdapterAdvancedProperty") && !l.contains("Set-NetAdapterAdvancedProperty") {
                Some(r#"[{"A":"Ethernet","K":"*FlowControl","V":"1","D":"3"}]"#.to_string())
            } else {
                None
            }
        });
        let t = adapter_tweaks(fake.clone()).remove(1);
        let (_d, e) = engine();
        e.with_store(|s| s.set_backup("net.adapteradvanced", backup("{E}"))).unwrap();
        assert!(!forget_missing_adapter(&e, &t).unwrap());
        assert!(fake.calls().iter().any(|c| c.contains("Set-NetAdapterAdvancedProperty")));
    }

    #[test]
    fn binding_renamed_is_found_and_old_name_is_not_a_false_success() {
        let fake = Fake::new(|l| {
            l.contains("Get-NetAdapterBinding").then(|| if l.contains("Enable-NetAdapterBinding") { String::new() } else { r#"[{"A":"Ethernet","C":"ms_lldp","E":true}]"#.to_string() })
        });
        let t = ProtocolBindingsTweak { cmd: fake.clone() };
        let (_d, e) = engine();
        let mut b = TweakBackup::new("b");
        b.data.insert("Old|ms_lldp".into(), "1".into());
        b.data.insert("guid:Old".into(), "{E}".into());
        e.with_store(|s| s.set_backup("net.bindings", b)).unwrap();
        e.revert(&t).unwrap();
        assert!(fake.calls().iter().any(|c| c.contains("Enable-NetAdapterBinding") && c.contains("Escape('Ethernet')")));

        let unbound = Fake::new(|l| l.contains("Get-NetAdapterBinding").then(|| r#"[{"A":"Ethernet","C":"ms_lldp","E":false}]"#.to_string()));
        let t = ProtocolBindingsTweak { cmd: unbound };
        let (_d, e) = engine();
        let mut b = TweakBackup::new("b");
        b.data.insert("Old|ms_lldp".into(), "1".into());
        e.with_store(|s| s.set_backup("net.bindings", b)).unwrap();
        assert!(matches!(e.revert(&t), Err(Error::Revert(_))), "Ethernet may be Old, renamed");
        assert!(e.with_store(|s| s.get_backup("net.bindings").is_some()));
    }

    #[test]
    fn binding_still_off_after_revert_keeps_backup() {
        let fake = Fake::new(|l| {
            if l.contains("Get-NetAdapter -ErrorAction") {
                Some(r#"[{"N":"Ethernet","D":"Realtek","T":6}]"#.into())
            } else if l.contains("Get-NetAdapterBinding") {
                Some(r#"[{"A":"Ethernet","C":"ms_lltdio","E":false},{"A":"Ethernet","C":"ms_lldp","E":true}]"#.into())
            } else {
                None
            }
        });
        let t = ProtocolBindingsTweak { cmd: fake };
        let (_d, e) = engine();
        let mut b = TweakBackup::new("b");
        b.data.insert("Ethernet|ms_lltdio".into(), "1".into());
        b.data.insert("Ethernet|ms_lldp".into(), "1".into());
        e.with_store(|s| s.set_backup("net.bindings", b)).unwrap();
        assert!(matches!(e.revert(&t), Err(Error::Revert(ref f)) if f.len() == 1));
        assert!(e.with_store(|s| s.get_backup("net.bindings").is_some()));
    }

    #[test]
    fn binding_failed_read_back_keeps_backup() {
        let fake = Fake::new(|l| {
            if l.contains("$all=") {
                Some("read failed".into())
            } else {
                None
            }
        });
        let t = ProtocolBindingsTweak { cmd: fake };
        let (_d, e) = engine();
        let mut b = TweakBackup::new("b");
        b.data.insert("Wi-Fi|ms_lltdio".into(), "1".into());
        e.with_store(|s| s.set_backup("net.bindings", b)).unwrap();
        assert!(e.revert(&t).is_err());
        assert!(e.with_store(|s| s.get_backup("net.bindings").is_some()));
    }

    #[test]
    fn binding_empty_read_back_keeps_backup() {
        let fake = Fake::new(|l| l.contains("$all=").then(String::new));
        let t = ProtocolBindingsTweak { cmd: fake };
        let (_d, e) = engine();
        let mut b = TweakBackup::new("b");
        b.data.insert("Wi-Fi|ms_lltdio".into(), "1".into());
        e.with_store(|s| s.set_backup("net.bindings", b)).unwrap();
        assert!(e.revert(&t).is_err());
        assert!(e.with_store(|s| s.get_backup("net.bindings").is_some()));
    }

    /// One adapter whose `*SelectiveSuspend` reads 0 whatever is written.
    fn stuck_adapter() -> AdapterPropertyTweak {
        let fake = Fake::new(|l| {
            if l.contains("Get-NetAdapter -ErrorAction") {
                Some(r#"[{"N":"Ethernet","D":"Realtek","T":6}]"#.into())
            } else if l.contains("Get-NetAdapterAdvancedProperty") {
                // valid list without the target: `found` would hide this row
                Some(r#"[{"A":"Ethernet","K":"*SelectiveSuspend","V":"0","L":"1,2"}]"#.into())
            } else {
                None
            }
        });
        adapter_tweaks(fake).remove(0)
    }

    #[test]
    fn adapter_value_not_restored_keeps_backup() {
        let t = stuck_adapter();
        let (_d, e) = engine();
        let mut b = TweakBackup::new("a");
        b.data.insert("Ethernet|*SelectiveSuspend".into(), "1".into());
        e.with_store(|s| s.set_backup("net.adapterpower", b)).unwrap();
        assert!(matches!(e.revert(&t), Err(Error::Revert(_))));
        assert!(e.with_store(|s| s.get_backup("net.adapterpower").is_some()));
    }

    #[test]
    fn adapter_empty_original_does_not_block_revert() {
        let t = stuck_adapter();
        let (_d, e) = engine();
        let mut b = TweakBackup::new("a");
        b.data.insert("Ethernet|*SelectiveSuspend".into(), String::new());
        b.data.insert("Ethernet|ULPMode".into(), "1".into());
        e.with_store(|s| s.set_backup("net.adapterpower", b)).unwrap();
        assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted, "the empty one is skipped, the other still restored");
    }

    #[test]
    fn adapter_empty_original_is_not_sent_to_set() {
        let fake = Fake::new(|l| (!l.contains("InterfaceGuid")).then(|| "[]".to_string()));
        let t = adapter_tweaks(fake.clone()).remove(0);
        let (_d, e) = engine();
        let mut b = TweakBackup::new("a");
        b.data.insert("Ethernet|*SelectiveSuspend".into(), String::new());
        b.data.insert("Ethernet|ULPMode".into(), "1".into());
        e.with_store(|s| s.set_backup("net.adapterpower", b)).unwrap();
        e.revert(&t).unwrap();
        let calls = fake.calls();
        let set = calls.iter().find(|c| c.contains("Set-NetAdapterAdvancedProperty")).unwrap();
        assert!(set.contains("'ULPMode'") && !set.contains("*SelectiveSuspend"), "{set}");
    }

    #[test]
    fn adapter_read_keeps_only_the_exact_keyword() {
        let fake = Fake::new(|_| Some("[]".into()));
        let t = adapter_tweaks(fake.clone()).remove(1);
        t.rows(&["Ethernet".to_string()], &["*EEE".to_string()]).unwrap();
        assert!(fake.calls()[0].contains("Where-Object { $_.RegistryKeyword -eq $k }"));
    }

    // ------------------------------------------------------------ MSI

    #[test]
    fn msi_builds_one_value_per_pci_device() {
        let fake = Fake::new(|_| Some("PCI\\VEN_10DE&DEV_2684\\4&1\r\n\r\nPCI\\VEN_8086&DEV_15F3\\5&2\r\n".into()));
        let t = msi_tweak(&*fake, Arc::new(crate::catalog::SystemHooks)).unwrap();
        assert_eq!(t.sets.len(), 2);
        assert!(t.sets[0].path.ends_with("Device Parameters\\Interrupt Management\\MessageSignaledInterruptProperties"));
        assert_eq!(t.sets[0].value, RegValue::Dword(1));
        assert!(t.flags.has(TweakFlags::EXPERIMENTAL));
        assert!(t.sets.iter().all(|s| s.only_if_key_exists), "the MSI key is never created");
    }

    #[test]
    fn msi_skips_a_device_the_driver_set_to_zero() {
        let read = |p: &str| match p {
            p if p.contains("ZERO") => Ok(Some(RegValue::Dword(0))),
            p if p.contains("BAD") => Err(Error::Plain("no access".into())),
            p if p.contains("ONE") => Ok(Some(RegValue::Dword(1))),
            _ => Ok(None),
        };
        let t = msi_from(["PCI\\ZERO", "PCI\\BAD", "PCI\\ONE", "PCI\\NONE"].into_iter(), Arc::new(crate::catalog::SystemHooks), read);
        let kept: Vec<bool> = ["ONE", "NONE"].iter().map(|d| t.sets.iter().any(|s| s.path.contains(d))).collect();
        assert_eq!((t.sets.len(), kept), (2, vec![true, true]));
    }

    // ------------------------------------------------------------ DirectX

    fn dx(s: &Scratch, tokens: &[(&str, &str)]) -> DirectXTweak {
        DirectXTweak {
            id: "t.dx".into(),
            title: "dx".into(),
            tokens: tokens.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect(),
            hive: Hive::Hkcu,
            key: s.0.clone(),
        }
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
    fn directx_edits_only_its_entry() {
        let s = Scratch::new("dx");
        reg::set(Hive::Hkcu, &s.0, DX_VALUE, &RegValue::String("Other=5;VRROptimizeEnable=0;Z=9;".into())).unwrap();
        let t = dx(&s, &[("VRROptimizeEnable", "1")]);
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        assert_eq!(reg::get(Hive::Hkcu, &s.0, DX_VALUE).unwrap(), Some(RegValue::String("Other=5;Z=9;VRROptimizeEnable=1;".into())));
        assert!(t.is_applied().unwrap());
        e.revert(&t).unwrap();
        assert_eq!(reg::get(Hive::Hkcu, &s.0, DX_VALUE).unwrap(), Some(RegValue::String("Other=5;Z=9;VRROptimizeEnable=0;".into())));
    }

    #[test]
    #[ignore = "real system: writes HKCU scratch key"]
    fn directx_absent_value_is_removed_again() {
        let s = Scratch::new("dxabs");
        let t = dx(&s, &[("AutoHDREnable", "2097")]);
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        e.apply(&t).unwrap(); // re-apply keeps the true original ("")
        assert!(t.is_applied().unwrap());
        e.revert(&t).unwrap();
        assert_eq!(reg::get(Hive::Hkcu, &s.0, DX_VALUE).unwrap(), None, "empty list deletes the value");
    }

    #[test]
    fn directx_tokens_keep_what_they_cannot_parse() {
        let mut l = dx_tokens("Other=5;garbage;A=b=c;;");
        put(&mut l, "VRROptimizeEnable", "1");
        assert_eq!(l, ["Other=5", "garbage", "A=b=c", "VRROptimizeEnable=1"]);
        assert_eq!(dx_get(&l, "A"), Some("b=c"));
        dx_drop(&mut l, "garbage"); // not a Name=Value entry, so no name can remove it
        assert!(l.iter().any(|t| t == "garbage"));
    }

    fn dx_token_tweak(name: &str) -> DirectXTweak {
        DirectXTweak { id: "t.dx".into(), title: "dx".into(), tokens: vec![(name.into(), "1".into())], hive: Hive::Hkcu, key: "unused".into() }
    }

    fn whole_value(value: Option<&str>) -> TweakBackup {
        let mut b = TweakBackup::new("dx");
        b.registry.push(crate::backup::RegBackup {
            hive: "HKCU".into(),
            path: DX_KEY.into(),
            name: DX_VALUE.into(),
            existed: value.is_some(),
            kind: value.map(|_| "String".into()),
            value: value.map(String::from),
            multi: None,
            created_keys: vec![],
        });
        b
    }

    #[test]
    fn directx_old_whole_value_backup_restores_only_its_own_token() {
        let t = dx_token_tweak("VRROptimizeEnable");
        let cur = dx_tokens("AutoHDREnable=2097;VRROptimizeEnable=1;SwapEffectUpgradeEnable=1;");
        assert_eq!(t.reverted(cur.clone(), &whole_value(Some("VRROptimizeEnable=0;"))), ["AutoHDREnable=2097", "SwapEffectUpgradeEnable=1", "VRROptimizeEnable=0"]);
        assert_eq!(t.reverted(cur, &whole_value(None)), ["AutoHDREnable=2097", "SwapEffectUpgradeEnable=1"], "it did not exist before");
    }

    #[test]
    fn directx_token_without_a_saved_original_is_left_alone() {
        let t = dx_token_tweak("VRROptimizeEnable");
        assert_eq!(t.reverted(dx_tokens("VRROptimizeEnable=1;"), &TweakBackup::new("dx")), ["VRROptimizeEnable=1"]);
    }

    // ------------------------------------------------------------ services list

    #[test]
    fn service_catalog_names() {
        let v = service_tweaks(Arc::new(Fake { calls: Mutex::new(vec![]), handler: Box::new(|_| None) }));
        let ids: Vec<_> = v.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["deb.xbox", "deb.spooler", "adv.search", "adv.sysmain", "adv.services"]);
        assert_eq!(v[0].names.len(), 4);
        assert_eq!(v[4].names.len(), 12);
    }

    // ------------------------------------------------------------ tasks

    #[derive(Default)]
    struct FakeTasks(Mutex<BTreeMap<String, bool>>);
    impl TaskApi for FakeTasks {
        fn is_enabled(&self, p: &str) -> Result<Option<bool>> {
            Ok(self.0.lock().unwrap().get(p).copied())
        }
        fn set_enabled(&self, p: &str, e: bool) -> Result<()> {
            if p.contains("Locked") {
                return Err(Error::Catalog("access denied".into()));
            }
            self.0.lock().unwrap().insert(p.into(), e);
            Ok(())
        }
    }

    #[test]
    fn tasks_disable_only_enabled_and_revert_only_those() {
        let api = Arc::new(FakeTasks::default());
        for (p, e) in [("\\A", true), ("\\B", false), ("\\D", true)] {
            api.0.lock().unwrap().insert(p.into(), e);
        }
        let t = ScheduledTasksTweak { api: api.clone(), id: "t.tasks".into(), title: "t".into(), tasks: ["\\A", "\\B", "\\D", "\\Missing"].map(String::from).to_vec() };
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        e.with_store(|s| {
            let keys: Vec<_> = s.get_backup("t.tasks").unwrap().data.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            assert_eq!(keys, [("\\A".to_string(), "1".to_string()), ("\\D".to_string(), "1".to_string())], "B was already off, Missing absent");
        });
        assert_eq!(api.0.lock().unwrap()["\\A"], false);
        e.revert(&t).unwrap();
        let m = api.0.lock().unwrap();
        assert_eq!((m["\\A"], m["\\B"], m["\\D"]), (true, false, true), "B stays disabled: it was never ours");
    }

    #[test]
    fn task_that_cannot_be_disabled_is_named_and_the_rest_undone() {
        let api = Arc::new(FakeTasks::default());
        for p in ["\\A", "\\Locked", "\\D"] {
            api.0.lock().unwrap().insert(p.into(), true);
        }
        let t = ScheduledTasksTweak { api: api.clone(), id: "t.tasks".into(), title: "t".into(), tasks: ["\\A", "\\Locked", "\\D"].map(String::from).to_vec() };
        let (_d, e) = engine();
        let err = e.apply(&t).unwrap_err().to_string();
        assert!(err.contains("\\Locked"), "{err}");
        let m = api.0.lock().unwrap();
        assert!(m["\\A"] && m["\\D"], "the others are enabled again");
        assert!(e.with_store(|s| s.get_backup("t.tasks").is_none()), "nothing is left to undo");
    }

    #[test]
    fn tasks_read_as_off_when_none_exist() {
        let api = Arc::new(FakeTasks::default());
        let t = ScheduledTasksTweak { api: api.clone(), id: "t.tasks".into(), title: "t".into(), tasks: vec!["\\Missing".into()] };
        assert!(!t.is_applied().unwrap());
        api.0.lock().unwrap().insert("\\Missing".into(), false);
        assert!(t.is_applied().unwrap());
    }

    #[test]
    fn tasks_revert_failure_keeps_backup_and_skips_vanished() {
        let api = Arc::new(FakeTasks::default());
        api.0.lock().unwrap().insert("\\A".into(), true);
        api.0.lock().unwrap().insert("\\Gone".into(), true);
        let t = ScheduledTasksTweak { api: api.clone(), id: "t.tasks".into(), title: "t".into(), tasks: vec!["\\A".into(), "\\Gone".into()] };
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        api.0.lock().unwrap().remove("\\Gone"); // vanished since
        api.0.lock().unwrap().insert("\\Locked2".into(), false);
        e.with_store(|s| {
            let mut b = s.get_backup("t.tasks").unwrap().clone();
            b.data.insert("\\Locked2".into(), "1".into());
            s.set_backup("t.tasks", b).unwrap();
        });
        let err = e.revert(&t).unwrap_err();
        assert!(matches!(err, Error::Revert(ref f) if f.len() == 1), "{err}");
        assert!(e.with_store(|s| s.get_backup("t.tasks").is_some()));
        assert_eq!(api.0.lock().unwrap()["\\A"], true, "the others were still restored");
    }

    // ------------------------------------------------------------ removed reverts

    #[test]
    fn dns_revert_uses_the_saved_adapters() {
        let fake = Fake::new(|_| None);
        let mut b = TweakBackup::new("dns");
        b.data.insert("ns:Ethernet".into(), "9.9.9.9,149.112.112.112".into());
        b.data.insert("ns:Wi-Fi".into(), String::new()); // down or not, it is restored
        b.data.insert("ns:Gone".into(), "1.1.1.1".into());
        b.data.insert("choice".into(), "Cloudflare".into());
        let r = DnsRevert { cmd: fake.clone() }.revert(&b);
        assert!(matches!(r, Err(ref x @ Error::Revert(ref f)) if only_missing_adapters(x) && f.len() == 1 && f[0].starts_with("Gone ")), "{r:?}");
        let last = fake.calls().last().unwrap().clone();
        assert!(last.contains("Escape('Ethernet')) -ServerAddresses @('9.9.9.9','149.112.112.112')"), "{last}");
        assert!(last.contains("Escape('Wi-Fi')) -ResetServerAddresses") && last.contains("Clear-DnsClientCache"), "{last}");
        assert!(!last.contains("Gone"), "a missing adapter must not stop the script: {last}");
        assert!(DnsRevert { cmd: fake.clone() }.apply(&mut TweakBackup::new("x")).is_err());

        let t = DnsRevert { cmd: fake };
        let (_d, e) = engine();
        e.with_store(|s| s.set_backup("net.dns", b)).unwrap();
        assert!(matches!(e.revert(&t), Err(ref x) if only_missing_adapters(x)));
        assert!(e.with_store(|s| s.get_backup("net.dns").is_some_and(|b| b.data.contains_key("ns:Gone"))), "the backup stays");
        assert!(forget_missing_adapter(&e, &t).unwrap(), "the same Forget works");
        assert!(e.with_store(|s| s.get_backup("net.dns").is_none()));
    }

    #[test]
    fn dns_failure_with_a_missing_adapter_is_not_forgettable() {
        let fake = Arc::new(FailDns);
        let t = DnsRevert { cmd: fake };
        let (_d, e) = engine();
        let mut b = TweakBackup::new("dns");
        b.data.insert("ns:Ethernet".into(), "9.9.9.9".into());
        b.data.insert("ns:Gone".into(), "1.1.1.1".into());
        e.with_store(|s| s.set_backup("net.dns", b)).unwrap();
        assert!(matches!(e.revert(&t), Err(ref x) if !only_missing_adapters(x)));
        assert!(forget_missing_adapter(&e, &t).is_err());
        assert!(e.with_store(|s| s.get_backup("net.dns").is_some()));
    }

    /// Lists the adapters, then fails the DNS script.
    struct FailDns;
    impl Cmd for FailDns {
        fn run(&self, _: &str, args: &[&str]) -> Result<RunResult> {
            let b = b64_decode(args.last().unwrap()).unwrap();
            let u: Vec<u16> = b.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
            let script = String::from_utf16(&u).unwrap();
            Ok(if script.contains("InterfaceGuid") { RunResult { exit_code: 0, output: IDS_JSON.into() } } else { RunResult { exit_code: 1, output: "denied".into() } })
        }
    }

    #[test]
    fn tcp_revert_commands_and_missing_field() {
        let fake = Fake::new(|_| None);
        let mut b = TweakBackup::new("tcp");
        for (k, v) in [("at", "Normal"), ("ecn", "Enabled"), ("rss", "Enabled"), ("rsc", "Disabled")] {
            b.data.insert(k.into(), v.into());
        }
        TcpStackRevert { cmd: fake.clone() }.revert(&b).unwrap();
        assert_eq!(
            fake.calls(),
            ["netsh int tcp set global autotuninglevel=normal", "netsh int tcp set global ecncapability=enabled", "netsh int tcp set global rss=enabled", "netsh int tcp set global rsc=disabled"]
        );
        b.data.remove("rsc");
        let fake = Fake::new(|_| None);
        TcpStackRevert { cmd: fake.clone() }.revert(&b).unwrap();
        assert_eq!(fake.calls().len(), 3, "an incomplete backup restores what it has and does not error forever");
    }

    #[test]
    fn memory_compression_revert_honours_the_backup() {
        let fake = Fake::new(|_| None);
        MemoryCompressionRevert { cmd: fake.clone() }.revert(&TweakBackup::new("m")).unwrap();
        assert_eq!(fake.calls(), ["ps Start-Service SysMain -ErrorAction SilentlyContinue;Enable-MMAgent -MemoryCompression"]);
        let fake = Fake::new(|_| None);
        let mut b = TweakBackup::new("m");
        b.data.insert("MemoryCompression".into(), "False".into());
        MemoryCompressionRevert { cmd: fake.clone() }.revert(&b).unwrap();
        assert!(fake.calls().is_empty(), "it was off before");
    }
}

#[cfg(test)]
mod adblock_tests {
    use super::*;

    #[test]
    fn removing_a_slot_leaves_no_gap() {
        let list = std::cell::RefCell::new(BTreeMap::from([("1", "mine"), ("2", "ours"), ("3", "x"), ("4", "y")].map(|(k, v)| (k.to_string(), RegValue::String(v.into())))));
        close_slot(
            2,
            |i| Ok(list.borrow().get(i).cloned()),
            |i, v| {
                list.borrow_mut().insert(i.to_string(), v.clone());
                Ok(())
            },
            |i| {
                list.borrow_mut().remove(i);
                Ok(())
            },
        )
        .unwrap();
        let left: Vec<(String, RegValue)> = list.into_inner().into_iter().collect();
        let want = [("1", "mine"), ("2", "x"), ("3", "y")].map(|(k, v)| (k.to_string(), RegValue::String(v.into())));
        assert_eq!(left, want);
    }

    /// Writes the real Chrome/Edge policy keys (admin). Slot 1 holds a dummy the browsers ignore.
    #[test]
    #[ignore]
    fn real_adblock_keeps_user_entries() {
        let t = AdBlockTweak;
        let (chrome, _) = ADBLOCK[0];
        let dummy = RegValue::String("not-an-extension".into());
        reg::set(Hive::Hklm, chrome, "1", &dummy).unwrap();
        let mut b = TweakBackup::new("adblock");
        assert!(!t.is_applied().unwrap());
        t.apply(&mut b).unwrap();
        assert!(t.is_applied().unwrap());
        assert_eq!(reg::get(Hive::Hklm, chrome, "1").unwrap(), Some(dummy.clone()));
        assert!(reg::get(Hive::Hklm, chrome, "2").unwrap().is_some());
        t.revert(&b).unwrap();
        assert!(!t.is_applied().unwrap());
        assert_eq!(reg::get(Hive::Hklm, chrome, "1").unwrap(), Some(dummy));
        assert_eq!(reg::get(Hive::Hklm, chrome, "2").unwrap(), None);
        reg::delete_value(Hive::Hklm, chrome, "1").unwrap();
        reg::delete_key_if_empty(Hive::Hklm, chrome).unwrap();
    }
}

#[cfg(test)]
mod cleanstart_tests {
    use super::*;

    #[test]
    fn pin_file_backup_and_restore() {
        let d = tempfile::tempdir().unwrap();
        let (pins, bak) = (d.path().join("start2.bin"), d.path().join("start2.bin.bak"));

        // A copy that fails leaves the pins alone and no backup behind.
        std::fs::write(&pins, "real pins").unwrap();
        assert!(save_pins(&pins, &d.path().join("missing").join("start2.bin.bak"), true).is_err());
        assert_eq!(std::fs::read_to_string(&pins).unwrap(), "real pins");

        // A second apply sees the emptied file and must keep the first backup.
        save_pins(&pins, &bak, true).unwrap();
        std::fs::write(&pins, "emptied").unwrap();
        save_pins(&pins, &bak, false).unwrap();
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "real pins");

        // A leftover file that no backup key owns is replaced, so old pins cannot come back.
        std::fs::write(&pins, "new pins").unwrap();
        save_pins(&pins, &bak, true).unwrap();
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "new pins");
        assert_eq!(std::fs::read_to_string(bak.with_extension("old")).unwrap(), "real pins", "the leftover is kept aside");
        std::fs::write(&bak, "real pins").unwrap();
        std::fs::write(&pins, "emptied").unwrap();

        // A restore that fails keeps the backup (the target folder is missing, tried for 2 s).
        assert!(restore_pins(&d.path().join("missing").join("start2.bin"), &bak).is_err());
        assert!(bak.exists());

        restore_pins(&pins, &bak).unwrap();
        assert_eq!(std::fs::read_to_string(&pins).unwrap(), "real pins");
        assert!(!bak.exists());
    }
}
