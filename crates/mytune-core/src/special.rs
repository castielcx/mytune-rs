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
//! - TCP revert returns a clear error if the backup lacks a field (C# throws KeyNotFound).
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
    format!("'{}'", s.replace('\'', "''"))
}

fn ps_list(items: &[String]) -> String {
    format!("@({})", items.iter().map(|s| ps_quote(s)).collect::<Vec<_>>().join(","))
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

fn json_rows<T: for<'de> Deserialize<'de>>(out: &str) -> Result<Vec<T>> {
    let t = out.trim();
    Ok(serde_json::from_str(if t.is_empty() { "[]" } else { t })?)
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
}

/// Connected Ethernet / Wi-Fi adapters, no virtual ones (same filter as C# `Net.ActiveAdapters`).
pub fn active_adapters(cmd: &dyn Cmd) -> Result<Vec<String>> {
    adapters(cmd, "$_.Status -eq 'Up'")
}

/// Adapters the driver tweaks work on: every enabled Ethernet / Wi-Fi adapter, connected or not.
/// Not `active_adapters`: a tweak restarts the adapter, so "connected right now" changed between two
/// calls and the tweaks read and wrote whichever adapter happened to be up.
fn tweak_adapters(cmd: &dyn Cmd) -> Result<Vec<String>> {
    adapters(cmd, "$_.Status -ne 'Disabled' -and $_.Status -ne 'Not Present'")
}

fn adapters(cmd: &dyn Cmd, filter: &str) -> Result<Vec<String>> {
    let out = powershell(
        cmd,
        &format!("ConvertTo-Json -InputObject @(Get-NetAdapter -ErrorAction SilentlyContinue | Where-Object {{ {filter} }} | ForEach-Object {{ [pscustomobject]@{{N=$_.Name;D=$_.InterfaceDescription;T=[int]$_.InterfaceType}} }}) -Compress"),
    )?;
    const BAD: [&str; 11] = ["virtual", "hyper-v", "vpn", "tap-", "wintun", "loopback", "vmware", "virtualbox", "bluetooth", "wireguard", "tailscale"];
    Ok(json_rows::<AdapterRow>(&out)?
        .into_iter()
        .filter(|a| matches!(a.kind, 6 | 71 | 117)) // Ethernet, Wireless80211, GigabitEthernet
        .filter(|a| {
            let d = a.description.clone().unwrap_or_default().to_lowercase();
            !BAD.iter().any(|b| d.contains(b))
        })
        .map(|a| a.name)
        .collect())
}

pub const NO_ADAPTER: &str = "No connected Ethernet or Wi-Fi adapter found.";

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
}

/// Restarts the adapter; waits for the link only when it had one (a disconnected adapter never comes up).
fn restart_script(a: &str) -> String {
    let q = ps_quote(a);
    format!("$u=(Get-NetAdapter -Name {q} -ErrorAction SilentlyContinue).Status -eq 'Up'; Restart-NetAdapter -Name {q}; for($i=0;$u -and $i -lt 30 -and (Get-NetAdapter -Name {q} -ErrorAction SilentlyContinue).Status -ne 'Up';$i++){{ Start-Sleep 1 }}")
}

const NOTHING_TO_UNDO: &str = "these are already your network driver's default settings, there is nothing to turn off";

/// Driver keyword tweak, e.g. "Adapter power management off". Targets are (keyword, value).
pub struct AdapterPropertyTweak {
    pub cmd: Arc<dyn Cmd>,
    pub id: String,
    pub title: String,
    pub targets: Vec<(String, String)>,
}

type Found = (String, String, String); // adapter, keyword, value

impl AdapterPropertyTweak {
    /// The keywords this tweak can set on the adapters. A keyword whose driver does not accept the target
    /// value is left out: it could never be set, so the tweak would never read as on.
    fn found(&self, adapters: &[String]) -> Result<Vec<PropRow>> {
        if adapters.is_empty() {
            return Ok(vec![]);
        }
        let kws: Vec<String> = self.targets.iter().map(|t| t.0.clone()).collect();
        let script = format!(
            "$r=@(); foreach($a in {}){{ foreach($k in {}){{ $p=Get-NetAdapterAdvancedProperty -Name $a -AllProperties -RegistryKeyword $k -ErrorAction SilentlyContinue; if($p){{ $r+=[pscustomobject]@{{A=$a;K=$k;V=(@($p.RegistryValue) -join ',');D=(@($p.DefaultRegistryValue) -join ',');L=(@($p.ValidRegistryValues) -join ',')}} }} }} }}; ConvertTo-Json -InputObject @($r) -Compress",
            ps_list(adapters),
            ps_list(&kws)
        );
        let rows = json_rows::<PropRow>(&powershell(&*self.cmd, &script)?)?;
        Ok(rows
            .into_iter()
            .filter(|r| {
                let valid = r.valid.as_deref().unwrap_or("");
                valid.is_empty() || self.target(&r.keyword).is_some_and(|t| valid.split(',').any(|v| v == t))
            })
            .collect())
    }

    fn target(&self, kw: &str) -> Option<&str> {
        self.targets.iter().find(|t| t.0 == kw).map(|t| t.1.as_str())
    }

    /// Off without stored originals: every keyword at the target goes back to the driver's default.
    /// False when there is none to write (default unknown or equal to the target), nothing touched.
    fn defaults(&self) -> Result<bool> {
        let changes: Vec<Found> = self
            .found(&tweak_adapters(&*self.cmd)?)?
            .into_iter()
            .filter_map(|r| {
                let (v, d) = (r.value.unwrap_or_default(), r.default.unwrap_or_default());
                (self.target(&r.keyword) == Some(v.as_str()) && !d.is_empty() && d != v).then_some((r.adapter, r.keyword, d))
            })
            .collect();
        self.set(&changes)?;
        Ok(!changes.is_empty())
    }

    fn set(&self, changes: &[Found]) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        let mut lines: Vec<String> = changes
            .iter()
            .map(|(a, k, v)| {
                format!("Set-NetAdapterAdvancedProperty -Name {} -AllProperties -RegistryKeyword {} -RegistryValue {} -NoRestart -ErrorAction SilentlyContinue", ps_quote(a), ps_quote(k), ps_quote(v))
            })
            .collect();
        let mut seen = Vec::new();
        for (a, _, _) in changes {
            if !seen.contains(a) {
                seen.push(a.clone());
                lines.push(restart_script(a));
            }
        }
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
        Ok(!found.is_empty() && found.iter().all(|r| self.target(&r.keyword) == r.value.as_deref()))
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        let mut changes = Vec::new();
        for r in self.found(&tweak_adapters(&*self.cmd)?)? {
            let v = r.value.unwrap_or_default();
            // Only a value this changes is an original worth keeping.
            if let Some(t) = self.target(&r.keyword).filter(|t| *t != v) {
                b.data.entry(format!("{}|{}", r.adapter, r.keyword)).or_insert(v);
                changes.push((r.adapter, r.keyword, t.to_string()));
            }
        }
        self.set(&changes)
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        let restore: Vec<Found> = b
            .data
            .iter()
            .filter_map(|(key, v)| {
                let mut p = key.splitn(3, '|');
                match (p.next(), p.next(), p.next()) {
                    // Older backups also stored values that were already at the target: not an original.
                    (Some(a), Some(k), None) if self.target(k) != Some(v.as_str()) => Some((a.to_string(), k.to_string(), v.clone())),
                    _ => None,
                }
            })
            .collect();
        if restore.is_empty() && !self.defaults()? {
            return Err(Error::Plain(NOTHING_TO_UNDO.into()));
        }
        self.set(&restore)
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
            "$r=@(); foreach($a in {}){{ Get-NetAdapterBinding -Name $a -ErrorAction SilentlyContinue | Where-Object {{ {} -contains $_.ComponentID }} | ForEach-Object {{ $r+=[pscustomobject]@{{A=$a;C=$_.ComponentID;E=[bool]$_.Enabled}} }} }}; ConvertTo-Json -InputObject @($r) -Compress",
            ps_list(&adapters),
            ps_list(&comps)
        );
        json_rows(&powershell(&*self.cmd, &script)?)
    }

    fn enable(&self, pairs: &[(String, String)]) -> Result<()> {
        if pairs.is_empty() {
            return Ok(());
        }
        let lines: Vec<String> = pairs.iter().map(|(a, c)| format!("Enable-NetAdapterBinding -Name {} -ComponentID {} -ErrorAction SilentlyContinue", ps_quote(a), ps_quote(c))).collect();
        powershell(&*self.cmd, &lines.join(";")).map(|_| ())
    }

    /// Off without stored originals: Windows binds all three, so every unbound one is bound again.
    fn defaults(&self) -> Result<bool> {
        let off: Vec<(String, String)> = self.read()?.into_iter().filter(|r| !r.enabled).map(|r| (r.adapter, r.component)).collect();
        self.enable(&off)?;
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
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        let mut lines = Vec::new();
        for r in self.read()? {
            if r.enabled {
                b.data.entry(format!("{}|{}", r.adapter, r.component)).or_insert_with(|| "1".into());
                lines.push(format!("Disable-NetAdapterBinding -Name {} -ComponentID {}", ps_quote(&r.adapter), ps_quote(&r.component)));
            }
        }
        if lines.is_empty() {
            return Ok(());
        }
        powershell(&*self.cmd, &lines.join(";")).map(|_| ())
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        let on: Vec<(String, String)> = b
            .data
            .iter()
            .filter(|(_, v)| v.as_str() == "1")
            .filter_map(|(k, _)| k.split_once('|'))
            .map(|(a, c)| (a.to_string(), c.to_string()))
            .collect();
        if on.is_empty() && !self.defaults()? {
            return Err(Error::Plain("nothing is unbound, there is nothing to turn off".into()));
        }
        self.enable(&on)
    }
    fn revert_default(&self) -> Result<bool> {
        self.defaults()
    }
}

/// The two catalog instances of `AdapterPropertyTweak`.
pub fn adapter_tweaks(cmd: Arc<dyn Cmd>) -> Vec<AdapterPropertyTweak> {
    let t = |id: &str, title: &str, kws: &[&str]| AdapterPropertyTweak {
        cmd: cmd.clone(),
        id: id.into(),
        title: title.into(),
        targets: kws.iter().map(|k| (k.to_string(), "0".to_string())).collect(),
    };
    vec![
        t("net.adapterpower", "Adapter power management off", &["*DeviceSleepOnDisconnect", "*SelectiveSuspend", "ReduceSpeedOnPowerDown", "ULPMode"]),
        t(
            "net.adapteradvanced",
            "Adapter latency tuning",
            &["*InterruptModeration", "*FlowControl", "*EEE", "EEELinkAdvertisement", "AdvancedEEE", "EnableGreenEthernet", "GreenEthernet"],
        ),
    ]
}

// ---------------------------------------------------------------- MSI mode

/// MSI mode for every present PCI display / network device (`adv.msi`). The device list is read once here.
pub fn msi_tweak(cmd: &dyn Cmd, hooks: Arc<dyn Hooks>) -> Result<DataTweak> {
    let out = powershell(cmd, "Get-PnpDevice -PresentOnly -Class Display,Net -Status OK | Where-Object InstanceId -like 'PCI\\*' | ForEach-Object InstanceId")?;
    Ok(DataTweak {
        id: "adv.msi".into(),
        title: "MSI mode for GPU and network adapter".into(),
        description: String::new(),
        details: String::new(),
        flags: TweakFlags(TweakFlags::RESTART | TweakFlags::EXPERIMENTAL),
        removed: false,
        sets: out
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(|d| DataSet {
                hive: Hive::Hklm,
                path: format!("SYSTEM\\CurrentControlSet\\Enum\\{d}\\Device Parameters\\Interrupt Management\\MessageSignaledInterruptProperties"),
                name: "MSISupported".into(),
                value: RegValue::Dword(1),
                only_if_key_exists: false,
                default: None, // the driver's INF may set it: unknown
            })
            .collect(),
        after: After::None,
        hooks,
    })
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

    fn read(&self) -> Result<Vec<(String, String)>> {
        let raw = match reg::get(self.hive, &self.key, DX_VALUE)? {
            Some(RegValue::String(s)) => s,
            _ => String::new(),
        };
        Ok(raw
            .split(';')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(|e| e.split_once('=').map(|(n, v)| (n.to_string(), v.to_string())))
            .collect())
    }

    fn write(&self, list: &[(String, String)]) -> Result<()> {
        if list.is_empty() {
            reg::delete_value(self.hive, &self.key, DX_VALUE)
        } else {
            let s: String = list.iter().map(|(n, v)| format!("{n}={v};")).collect();
            reg::set(self.hive, &self.key, DX_VALUE, &RegValue::String(s))
        }
    }
}

fn put(list: &mut Vec<(String, String)>, name: &str, value: &str) {
    list.retain(|(n, _)| n != name);
    list.push((name.into(), value.into()));
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
        Ok(self.tokens.iter().all(|(n, v)| cur.iter().any(|(cn, cv)| cn == n && cv == v)))
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        let mut cur = self.read()?;
        for (n, v) in &self.tokens {
            let old = cur.iter().find(|(cn, _)| cn == n).map(|(_, cv)| cv.clone()).unwrap_or_default();
            b.data.entry(format!("old:{n}")).or_insert(old);
            put(&mut cur, n, v);
        }
        self.write(&cur)
    }
    fn revert(&self, b: &TweakBackup) -> Result<()> {
        if !b.registry.is_empty() {
            return b.restore_registry(); // older whole-value backup
        }
        let mut cur = self.read()?;
        for (n, _) in &self.tokens {
            let old = b.data.get(&format!("old:{n}")).cloned().unwrap_or_default();
            cur.retain(|(cn, _)| cn != n);
            if !old.is_empty() {
                cur.push((n.clone(), old));
            }
        }
        self.write(&cur)
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

/// Disables scheduled tasks. Data: task path = "1" for every task this tweak disabled.
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
    /// True when no listed task that exists is still enabled. (C# also returns true if a backup exists.)
    fn is_applied(&self) -> Result<bool> {
        for t in &self.tasks {
            if self.api.is_enabled(t)? == Some(true) {
                return Ok(false);
            }
        }
        Ok(true)
    }
    fn apply(&self, b: &mut TweakBackup) -> Result<()> {
        for t in &self.tasks {
            if self.api.is_enabled(t)? != Some(true) {
                continue; // missing or already disabled: leave alone
            }
            match self.api.set_enabled(t, false) {
                Ok(()) => {
                    b.data.insert(t.clone(), "1".into());
                }
                Err(e) => log::error(&format!("disable task {t}"), Some(&e)),
            }
        }
        Ok(())
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

fn removed_apply(id: &str) -> Error {
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
        let mut lines = Vec::new();
        for a in active_adapters(&*self.cmd)? {
            let prev = b.data.get(&format!("ns:{a}")).map(String::as_str).unwrap_or("");
            lines.push(if prev.trim().is_empty() {
                format!("Set-DnsClientServerAddress -InterfaceAlias {} -ResetServerAddresses", ps_quote(&a))
            } else {
                let servers: Vec<String> = prev.split(',').filter(|s| !s.is_empty()).map(String::from).collect();
                format!("Set-DnsClientServerAddress -InterfaceAlias {} -ServerAddresses {}", ps_quote(&a), ps_list(&servers))
            });
        }
        lines.push("Clear-DnsClientCache".into());
        powershell(&*self.cmd, &lines.join(";")).map(|_| ())
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
        let get = |k: &str| b.data.get(k).cloned().ok_or_else(|| Error::BadBackup(format!("net.tcp backup lacks `{k}`")));
        let (at, ecn, rss, rsc) = (get("at")?, get("ecn")?, get("rss")?, get("rsc")?);
        let ecn = match ecn.as_str() {
            "Enabled" => "enabled",
            "Disabled" => "disabled",
            _ => "default",
        };
        let netsh = system32("netsh.exe").to_string_lossy().into_owned();
        let (at, rss, rsc) = (at.to_lowercase(), rss.to_lowercase(), rsc.to_lowercase());
        for arg in [format!("autotuninglevel={at}"), format!("ecncapability={ecn}"), format!("rss={rss}"), format!("rsc={rsc}")] {
            let r = self.cmd.run(&netsh, &["int", "tcp", "set", "global", &arg])?;
            if r.exit_code != 0 {
                return Err(Error::ProcessFailed { cmd: format!("netsh int tcp set global {arg}"), code: r.exit_code, output: r.output });
            }
        }
        Ok(())
    }
}

/// Old `mem.compression`: turn memory compression back on (and start SysMain), whatever the backup says.
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
    fn revert(&self, _: &TweakBackup) -> Result<()> {
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
    fn revert(&self, backup: &TweakBackup) -> Result<()> {
        backup.restore_registry()
    }
    fn revert_default(&self) -> Result<bool> {
        for (path, entry) in ADBLOCK {
            if let (name, true) = adblock_slot(path, entry)? {
                reg::delete_value(Hive::Hklm, path, &name)?;
                reg::delete_key_if_empty(Hive::Hklm, path)?;
            }
        }
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
            Ok(RunResult { exit_code: 0, output: (self.handler)(&line).unwrap_or_default() })
        }
    }

    fn engine() -> (tempfile::TempDir, Engine) {
        let d = tempfile::tempdir().unwrap();
        let e = Engine::new(Store::load(d.path()));
        (d, e)
    }

    const ADAPTERS_JSON: &str = r#"[{"N":"Ethernet","D":"Realtek PCIe GbE","T":6},{"N":"vEthernet","D":"Hyper-V Virtual","T":6},{"N":"Wi-Fi","D":"Intel AX200","T":71},{"N":"Loop","D":"Loopback","T":24},{"N":"Single","D":null,"T":117}]"#;

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

    fn adapter_fake(state: Arc<Mutex<Vec<(String, String, String)>>>) -> Arc<Fake> {
        Fake::new(move |l| {
            if l.contains("Get-NetAdapter -ErrorAction") {
                Some(r#"[{"N":"Ethernet","D":"Realtek","T":6}]"#.into())
            } else if l.contains("Get-NetAdapterAdvancedProperty") {
                let rows: Vec<String> = state.lock().unwrap().iter().map(|(a, k, v)| format!(r#"{{"A":"{a}","K":"{k}","V":"{v}"}}"#)).collect();
                Some(format!("[{}]", rows.join(",")))
            } else if l.contains("Set-NetAdapterAdvancedProperty") {
                for part in l.trim_start_matches("ps ").split(';').filter(|p| p.starts_with("Set-NetAdapterAdvancedProperty")) {
                    let q: Vec<&str> = part.split('\'').collect(); // [.., adapter, .., kw, .., value, ..]
                    let (a, k, v) = (q[1], q[3], q[5]);
                    for row in state.lock().unwrap().iter_mut().filter(|r| r.0 == a && r.1 == k) {
                        row.2 = v.to_string();
                    }
                }
                None
            } else {
                None
            }
        })
    }

    #[test]
    fn adapter_off_without_backup_writes_driver_default_and_skips_unsettable() {
        let fake = Fake::new(|l| {
            if l.contains("Get-NetAdapter -ErrorAction") {
                Some(r#"[{"N":"Ethernet","D":"Intel","T":6}]"#.into())
            } else if l.contains("Get-NetAdapterAdvancedProperty") {
                // flow control at the target with default 3; *EEE cannot be 0 on this driver
                Some(r#"[{"A":"Ethernet","K":"*FlowControl","V":"0","D":"3","L":"0,1,2,3"},{"A":"Ethernet","K":"*EEE","V":"1","D":"1","L":"1,2"}]"#.into())
            } else {
                None
            }
        });
        let t = adapter_tweaks(fake.clone()).remove(1);
        assert!(t.is_applied().unwrap(), "a keyword that cannot be 0 does not count");
        let (_d, e) = engine();
        assert_eq!(e.revert(&t).unwrap(), RevertOutcome::Reverted);
        let last = fake.calls().last().unwrap().clone();
        assert!(last.contains("-RegistryKeyword '*FlowControl' -RegistryValue '3'") && !last.contains("*EEE"), "{last}");
    }

    #[test]
    fn adapter_property_apply_and_revert() {
        let state = Arc::new(Mutex::new(vec![
            ("Ethernet".to_string(), "*SelectiveSuspend".to_string(), "1".to_string()),
            ("Ethernet".to_string(), "ULPMode".to_string(), "0".to_string()), // already at target
        ]));
        let fake = adapter_fake(state.clone());
        let t = adapter_tweaks(fake.clone()).remove(0);
        assert_eq!(t.id, "net.adapterpower");
        assert!(!t.is_applied().unwrap());
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        assert!(t.is_applied().unwrap());
        assert!(fake.calls().iter().any(|c| c.contains("Restart-NetAdapter -Name 'Ethernet'")));
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
        let no_kw = adapter_fake(state);
        assert!(adapter_tweaks(no_kw).remove(0).unavailable().unwrap().unwrap().contains("driver"));
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
        assert!(c.iter().any(|x| x.contains("Disable-NetAdapterBinding -Name 'Ethernet' -ComponentID 'ms_lltdio'") && x.contains("'ms_lldp'") && !x.contains("'ms_rspndr'")));
        e.revert(&t).unwrap();
        let last = fake.calls().last().unwrap().clone();
        assert!(last.contains("Enable-NetAdapterBinding") && last.contains("ms_lltdio") && last.contains("ms_lldp") && !last.contains("ms_rspndr"), "{last}");
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
        for (p, e) in [("\\A", true), ("\\B", false), ("\\Locked", true), ("\\D", true)] {
            api.0.lock().unwrap().insert(p.into(), e);
        }
        let t = ScheduledTasksTweak { api: api.clone(), id: "t.tasks".into(), title: "t".into(), tasks: ["\\A", "\\B", "\\Locked", "\\D", "\\Missing"].map(String::from).to_vec() };
        let (_d, e) = engine();
        e.apply(&t).unwrap();
        e.with_store(|s| {
            let keys: Vec<_> = s.get_backup("t.tasks").unwrap().data.keys().cloned().collect();
            assert_eq!(keys, ["\\A", "\\D"], "B was already off, Locked failed, Missing absent");
        });
        assert_eq!(api.0.lock().unwrap()["\\A"], false);
        e.revert(&t).unwrap();
        let m = api.0.lock().unwrap();
        assert_eq!((m["\\A"], m["\\B"], m["\\D"]), (true, false, true), "B stays disabled: it was never ours");
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
    fn dns_revert_resets_or_restores() {
        let fake = Fake::new(|l| {
            if l.contains("Get-NetAdapter -ErrorAction") {
                Some(r#"[{"N":"Ethernet","D":"Realtek","T":6},{"N":"Wi-Fi","D":"Intel","T":71}]"#.into())
            } else {
                None
            }
        });
        let mut b = TweakBackup::new("dns");
        b.data.insert("ns:Ethernet".into(), "9.9.9.9,149.112.112.112".into());
        b.data.insert("choice".into(), "Cloudflare".into());
        DnsRevert { cmd: fake.clone() }.revert(&b).unwrap();
        let last = fake.calls().last().unwrap().clone();
        assert!(last.contains("-InterfaceAlias 'Ethernet' -ServerAddresses @('9.9.9.9','149.112.112.112')"), "{last}");
        assert!(last.contains("-InterfaceAlias 'Wi-Fi' -ResetServerAddresses") && last.ends_with("Clear-DnsClientCache"));
        assert!(DnsRevert { cmd: fake }.apply(&mut TweakBackup::new("x")).is_err());
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
        assert!(matches!(TcpStackRevert { cmd: fake }.revert(&b), Err(Error::BadBackup(_))));
    }

    #[test]
    fn memory_compression_revert_enables() {
        let fake = Fake::new(|_| None);
        MemoryCompressionRevert { cmd: fake.clone() }.revert(&TweakBackup::new("m")).unwrap();
        assert_eq!(fake.calls(), ["ps Start-Service SysMain -ErrorAction SilentlyContinue;Enable-MMAgent -MemoryCompression"]);
    }
}

#[cfg(test)]
mod adblock_tests {
    use super::*;

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
