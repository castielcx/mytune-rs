//! Internet page, VPN section (C# `VpnService`): finds installed VPNs, switches the ones with a known
//! command line, and keeps them in the state the user chose (watcher).
//!
//! - Switchable: Mullvad, Cloudflare WARP, Tailscale (their own CLI) and Windows VPN profiles (`rasdial`).
//! - Status only (service / adapter match, no CLI): Proton, Nord, Express, Surfshark, Windscribe, OpenVPN,
//!   WireGuard, and other VPN-looking adapters.
//! - The wanted on/off state per VPN id lives in `state.json` under `Vpns`, same as the C# app. Never run the
//!   watcher together with the C# app: both would switch VPNs.
//! - Watcher: waits 8 s after start, then checks on network-address changes and every 2 min; a VPN that
//!   fails 3 times (or failed less than 60 s ago) is left alone until the user toggles it again.
//!
//! Differences from C#: regexes are plain substring / prefix matches written out by hand (same patterns);
//! all programs go through `Env` so tests never touch a real VPN.

use crate::error::{Error, Result};
use crate::log;
use crate::process::{self, system32, RunResult};
use crate::special::powershell;
use crate::store::Store;
use crate::tweaks::system_cmd;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VpnEntry {
    /// `cli:<product>`, `ras:<profile>` or `adapter:<name>`.
    pub id: String,
    pub name: String,
    pub detail: String,
    pub connected: bool,
    pub controllable: bool,
    /// Connecting right now (the user can cancel).
    pub pending: bool,
}

/// Everything the module needs from the machine. Real: `SysEnv`.
pub trait Env: Send + Sync {
    fn exists(&self, p: &Path) -> bool;
    fn run(&self, exe: &str, args: &[&str], timeout: Duration) -> Result<RunResult>;
    fn sleep(&self, d: Duration);
    /// JSON from the PowerShell scan (`adapters`, `services`, `vpn`).
    fn scan_json(&self) -> Result<String>;
}

pub struct SysEnv;
impl Env for SysEnv {
    fn exists(&self, p: &Path) -> bool {
        p.exists()
    }
    fn run(&self, exe: &str, args: &[&str], timeout: Duration) -> Result<RunResult> {
        process::run_with(exe, args, timeout, None)
    }
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d)
    }
    fn scan_json(&self) -> Result<String> {
        powershell(
            &*system_cmd(),
            r"
$ad=@();$sv=@();$vc=@()
try{$ad=@(Get-NetAdapter|Select-Object Name,InterfaceDescription,@{n='Up';e={$_.Status -eq 'Up'}})}catch{}
try{$sv=@(Get-Service|Select-Object Name)}catch{}
try{$vc+=@(Get-VpnConnection)}catch{}
try{$vc+=@(Get-VpnConnection -AllUserConnection)}catch{}
$vc=@($vc|Select-Object Name,@{n='Up';e={$_.ConnectionStatus -eq 'Connected'}})
@{adapters=@($ad);services=@($sv);vpn=@($vc)}|ConvertTo-Json -Depth 4 -Compress",
        )
    }
}

// ---------------------------------------------------------------- products

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cli {
    Mullvad,
    Warp,
    Tailscale,
}

impl Cli {
    fn exe(self) -> PathBuf {
        // these programs run as admin, so not from `%ProgramFiles%`, which a non-admin can set
        let pf = crate::util::known_folder(&windows::Win32::UI::Shell::FOLDERID_ProgramFiles).unwrap_or_else(|_| PathBuf::from("C:\\Program Files"));
        match self {
            Cli::Mullvad => pf.join("Mullvad VPN").join("resources").join("mullvad.exe"),
            Cli::Warp => pf.join("Cloudflare").join("Cloudflare WARP").join("warp-cli.exe"),
            Cli::Tailscale => pf.join("Tailscale").join("tailscale.exe"),
        }
    }
    fn connect(self) -> &'static str {
        if self == Cli::Tailscale { "up" } else { "connect" }
    }
    fn disconnect(self) -> &'static str {
        if self == Cli::Tailscale { "down" } else { "disconnect" }
    }
    fn is_connected(self, r: &RunResult) -> bool {
        let o = r.output.to_lowercase();
        match self {
            Cli::Mullvad => line_starts_with_word(&o, "connected"),
            Cli::Warp => after_marker(&o, "status update:", "connected"),
            Cli::Tailscale => r.exit_code == 0 && !["stopped", "logged out", "needs login"].iter().any(|w| o.contains(w)),
        }
    }
    fn is_connecting(self, r: &RunResult) -> bool {
        let o = r.output.to_lowercase();
        match self {
            Cli::Mullvad => line_starts_with_word(&o, "connecting"),
            Cli::Warp => after_marker(&o, "status update:", "connecting"),
            Cli::Tailscale => false,
        }
    }
}

/// `(?im)^\s*<word>\b` on already lower-cased text.
fn line_starts_with_word(lower: &str, word: &str) -> bool {
    lower.lines().any(|l| {
        l.trim_start().strip_prefix(word).is_some_and(|rest| !rest.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '_'))
    })
}

/// `<marker>\s*<word>` on already lower-cased text.
fn after_marker(lower: &str, marker: &str, word: &str) -> bool {
    lower.match_indices(marker).any(|(i, _)| lower[i + marker.len()..].trim_start().starts_with(word))
}

enum Svc {
    Exact(&'static str),
    Prefix(&'static str),
}

struct Product {
    name: &'static str,
    service: Svc,
    /// Any of these in "adapter name + description" (lower case).
    adapter: &'static [&'static str],
    cli: Option<Cli>,
}

const PRODUCTS: &[Product] = &[
    Product { name: "Mullvad VPN", service: Svc::Exact("MullvadVPN"), adapter: &["mullvad"], cli: Some(Cli::Mullvad) },
    Product { name: "Cloudflare WARP", service: Svc::Exact("CloudflareWARP"), adapter: &["warp"], cli: Some(Cli::Warp) },
    Product { name: "Tailscale", service: Svc::Exact("Tailscale"), adapter: &["tailscale"], cli: Some(Cli::Tailscale) },
    Product { name: "Proton VPN", service: Svc::Prefix("ProtonVPN"), adapter: &["proton"], cli: None },
    Product { name: "NordVPN", service: Svc::Prefix("nordvpn"), adapter: &["nord"], cli: None },
    Product { name: "ExpressVPN", service: Svc::Prefix("ExpressVpn"), adapter: &["express", "xvpn"], cli: None },
    Product { name: "Surfshark", service: Svc::Prefix("Surfshark"), adapter: &["surfshark"], cli: None },
    Product { name: "Windscribe", service: Svc::Prefix("WindscribeService"), adapter: &["windscribe"], cli: None },
    Product { name: "OpenVPN", service: Svc::Prefix("OpenVPNService"), adapter: &["tap-windows", "openvpn", "data channel offload"], cli: None },
    Product { name: "WireGuard", service: Svc::Prefix("WireGuardTunnel$"), adapter: &["wireguard tunnel"], cli: None },
];

const GENERIC_ADAPTER: &[&str] = &["vpn", "wireguard", "wintun", "tap-windows", "zerotier"];
const IGNORED_ADAPTER: &[&str] = &["teredo", "isatap", "6to4", "miniport", "hyper-v"];

fn has_any(text: &str, words: &[&str]) -> bool {
    let t = text.to_lowercase();
    words.iter().any(|w| t.contains(w))
}

fn svc_matches(s: &Svc, name: &str) -> bool {
    let n = name.to_lowercase();
    match s {
        Svc::Exact(x) => n == x.to_lowercase(),
        Svc::Prefix(x) => n.starts_with(&x.to_lowercase()),
    }
}

fn product_cli(name: &str) -> Option<Cli> {
    PRODUCTS.iter().find(|p| p.name == name).and_then(|p| p.cli)
}

/// First line of `status`, cut to 70 characters.
fn status(env: &dyn Env, cli: Cli) -> Result<(String, RunResult)> {
    let r = env.run(&cli.exe().to_string_lossy(), &["status"], Duration::from_secs(15))?;
    let first: String = r.output.lines().next().unwrap_or("").trim().chars().take(70).collect();
    Ok((first, r))
}

// ---------------------------------------------------------------- scan

fn items<'a>(root: &'a Value, prop: &str) -> Vec<&'a Value> {
    match root.get(prop) {
        Some(Value::Array(a)) => a.iter().collect(),
        Some(v @ Value::Object(_)) => vec![v],
        _ => vec![],
    }
}

fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

fn b(v: &Value, k: &str) -> bool {
    v.get(k).and_then(Value::as_bool).unwrap_or(false)
}

pub fn scan(env: &dyn Env) -> Result<Vec<VpnEntry>> {
    let out = env.scan_json()?;
    let info: Value = serde_json::from_str(out.trim())?;
    let adapters: Vec<(String, String, bool)> =
        items(&info, "adapters").iter().map(|a| (s(a, "Name").to_string(), s(a, "InterfaceDescription").to_string(), b(a, "Up"))).collect();
    let services: Vec<String> = items(&info, "services").iter().map(|v| s(v, "Name").to_string()).collect();
    let mut list: Vec<VpnEntry> = Vec::new();
    let mut claimed: HashSet<String> = HashSet::new();

    for p in PRODUCTS {
        let mine: Vec<_> = adapters.iter().filter(|a| has_any(&format!("{} {}", a.0, a.1), p.adapter)).collect();
        let has_exe = p.cli.is_some_and(|c| env.exists(&c.exe()));
        let has_service = services.iter().any(|n| svc_matches(&p.service, n));
        if !has_exe && !has_service {
            continue;
        }
        claimed.extend(mine.iter().map(|a| a.0.clone()));
        let id = |kind: &str| format!("{kind}:{}", p.name);
        if has_exe {
            let cli = p.cli.unwrap();
            match status(env, cli) {
                Ok((line, r)) => {
                    let on = cli.is_connected(&r);
                    let detail = if !line.is_empty() { line } else if on { "Connected".into() } else { "Disconnected".into() };
                    list.push(VpnEntry { id: id("cli"), name: p.name.into(), detail, connected: on, controllable: true, pending: cli.is_connecting(&r) });
                }
                Err(e) => {
                    log::error(&format!("vpn status {}", p.name), Some(&e));
                    list.push(VpnEntry { id: id("cli"), name: p.name.into(), detail: "Status unavailable".into(), connected: false, controllable: false, pending: false });
                }
            }
        } else {
            let on = mine.iter().any(|a| a.2);
            let detail = if on { "Connected (adapter up), status only" } else { "Not connected, status only" };
            list.push(VpnEntry { id: id("adapter"), name: p.name.into(), detail: detail.into(), connected: on, controllable: false, pending: false });
        }
    }

    for v in items(&info, "vpn") {
        let name = s(v, "Name");
        if name.is_empty() {
            continue;
        }
        let on = b(v, "Up");
        let detail = if on { "Windows VPN connection, connected" } else { "Windows VPN connection" };
        list.push(VpnEntry { id: format!("ras:{name}"), name: name.into(), detail: detail.into(), connected: on, controllable: true, pending: false });
    }

    for (name, desc, up) in &adapters {
        let text = format!("{name} {desc}");
        if claimed.contains(name) || list.iter().any(|l| &l.name == name) {
            continue;
        }
        if !has_any(&text, GENERIC_ADAPTER) || has_any(&text, IGNORED_ADAPTER) {
            continue;
        }
        let detail = format!("{}, status only", if *up { "Adapter up" } else { "Adapter down" });
        list.push(VpnEntry { id: format!("adapter:{name}"), name: name.clone(), detail, connected: *up, controllable: false, pending: false });
    }
    Ok(list)
}

// ---------------------------------------------------------------- switching

#[derive(Default)]
struct Gate {
    in_flight: HashSet<String>,
    retry: HashMap<String, (u32, Instant)>,
}

static GATE: LazyLock<Mutex<Gate>> = LazyLock::new(|| Mutex::new(Gate::default()));

fn gate() -> std::sync::MutexGuard<'static, Gate> {
    GATE.lock().unwrap_or_else(|p| p.into_inner())
}

fn name_of(id: &str) -> &str {
    id.split_once(':').map(|x| x.1).unwrap_or(id)
}

fn fail(msg: impl Into<String>) -> Error {
    Error::Os(msg.into())
}

/// Connects or disconnects one scanned entry and waits until it reaches that state (CLI products: up to 15 s).
/// A connect that never completes is cancelled so no VPN is left half connected. `cancel` aborts the wait.
pub fn set(env: &dyn Env, id: &str, on: bool, cancel: &AtomicBool) -> Result<()> {
    gate().in_flight.insert(id.to_string());
    let r = set_core(env, id, on, cancel);
    gate().in_flight.remove(id);
    r
}

fn run_ok(env: &dyn Env, exe: &str, args: &[&str], secs: u64, name: &str) -> Result<()> {
    let r = env.run(exe, args, Duration::from_secs(secs))?;
    if r.exit_code != 0 {
        return Err(fail(if r.output.is_empty() { format!("{name} returned {}", r.exit_code) } else { r.output }));
    }
    Ok(())
}

fn set_core(env: &dyn Env, id: &str, on: bool, cancel: &AtomicBool) -> Result<()> {
    let name = name_of(id);
    if id.starts_with("ras:") {
        let rasdial = system32("rasdial.exe").to_string_lossy().into_owned();
        let args: Vec<&str> = if on { vec![name] } else { vec![name, "/disconnect"] };
        return run_ok(env, &rasdial, &args, 60, name);
    }
    let cli = id.starts_with("cli:").then(|| product_cli(name)).flatten().ok_or_else(|| fail("This VPN can't be switched from MyTune."))?;
    let exe = cli.exe().to_string_lossy().into_owned();
    run_ok(env, &exe, &[if on { cli.connect() } else { cli.disconnect() }], 30, name)?;
    // Connecting is asynchronous in these CLIs; poll until the state matches.
    for _ in 0..30 {
        if cancel.load(Relaxed) {
            return Err(fail("Cancelled."));
        }
        if status(env, cli).is_ok_and(|(_, r)| cli.is_connected(&r) == on) {
            return Ok(());
        }
        env.sleep(Duration::from_millis(500));
    }
    if on {
        if let Err(e) = cancel_connect(env, id) {
            log::error(&format!("vpn cancel {name}"), Some(&e));
        }
    }
    Err(fail(format!("{name} did not {} within 15 seconds{}", if on { "connect" } else { "disconnect" }, if on { " and was cancelled." } else { "." })))
}

/// Aborts a connection attempt (or disconnects) by sending the VPN's disconnect command.
pub fn cancel_connect(env: &dyn Env, id: &str) -> Result<()> {
    let name = name_of(id);
    if id.starts_with("ras:") {
        let rasdial = system32("rasdial.exe").to_string_lossy().into_owned();
        env.run(&rasdial, &[name, "/disconnect"], Duration::from_secs(30))?;
        return Ok(());
    }
    let cli = id.starts_with("cli:").then(|| product_cli(name)).flatten().ok_or_else(|| fail("This VPN can't be switched from MyTune."))?;
    run_ok(env, &cli.exe().to_string_lossy(), &[cli.disconnect()], 30, name)
}

// ---------------------------------------------------------------- remembered state + watcher

/// The on/off state the user chose per VPN id (`Vpns` in state.json).
pub fn wanted(store: &Store) -> Vec<(String, bool)> {
    store.top("Vpns").and_then(Value::as_object).map(|m| m.iter().filter_map(|(k, v)| Some((k.clone(), v.as_bool()?))).collect()).unwrap_or_default()
}

/// Records the state the user chose; the watcher keeps the VPN there, also after a restart.
pub fn remember(store: &mut Store, id: &str, on: bool) -> Result<()> {
    let mut m = store.top("Vpns").and_then(Value::as_object).cloned().unwrap_or_default();
    m.insert(id.to_string(), Value::Bool(on));
    store.set_top("Vpns", Value::Object(m));
    gate().retry.remove(id);
    store.save()
}

/// `None` while unknown or still connecting.
fn is_on(env: &dyn Env, id: &str) -> Result<Option<bool>> {
    let name = name_of(id);
    if id.starts_with("cli:") {
        let Some(cli) = product_cli(name) else { return Ok(None) };
        if !env.exists(&cli.exe()) {
            return Ok(None);
        }
        let (_, r) = status(env, cli)?;
        if r.exit_code != 0 && cli != Cli::Tailscale {
            return Ok(None); // daemon not running yet
        }
        if cli.is_connecting(&r) {
            return Ok(None);
        }
        return Ok(Some(cli.is_connected(&r)));
    }
    if id.starts_with("ras:") {
        let r = env.run(&system32("rasdial.exe").to_string_lossy(), &[], Duration::from_secs(15))?;
        return Ok(Some(r.output.to_lowercase().contains(&name.to_lowercase())));
    }
    Ok(None)
}

/// One watcher pass over the wanted states.
pub fn enforce(env: &dyn Env, want: &[(String, bool)]) {
    for (id, on) in want {
        if gate().in_flight.contains(id) {
            continue;
        }
        let now = match is_on(env, id) {
            Ok(Some(n)) => n,
            Ok(None) => continue,
            Err(e) => {
                log::error(&format!("vpn state {id}"), Some(&e));
                continue;
            }
        };
        if now == *on {
            gate().retry.remove(id);
            continue;
        }
        let prior = gate().retry.get(id).copied();
        // Back off so a VPN that won't connect isn't hammered; the user can toggle it to retry.
        if prior.is_some_and(|(fails, last)| fails >= 3 || last.elapsed() < Duration::from_secs(60)) {
            continue;
        }
        log::info(&format!("vpn watcher: {id} should be {}, switching", if *on { "on" } else { "off" }));
        match set(env, id, *on, &AtomicBool::new(false)) {
            Ok(()) => {
                gate().retry.remove(id);
            }
            Err(e) => {
                log::error(&format!("vpn watcher {id}"), Some(&e));
                gate().retry.insert(id.clone(), (prior.map_or(0, |p| p.0) + 1, Instant::now()));
            }
        }
    }
}

static CHANGED: LazyLock<(Mutex<bool>, Condvar)> = LazyLock::new(|| (Mutex::new(false), Condvar::new()));

fn listen_for_address_changes() {
    use windows::Win32::NetworkManagement::IpHelper::NotifyAddrChange;
    std::thread::Builder::new()
        .name("VpnAddrChange".into())
        .spawn(|| loop {
            // both arguments null = block this thread until an address changes
            let rc = unsafe { NotifyAddrChange(std::ptr::null_mut(), std::ptr::null()) };
            if rc != 0 {
                std::thread::sleep(Duration::from_secs(5)); // API unavailable: fall back to the 2 min pass
                continue;
            }
            *CHANGED.0.lock().unwrap_or_else(|p| p.into_inner()) = true;
            CHANGED.1.notify_all();
        })
        .ok();
}

/// Starts the background watcher. `wanted` is read on every pass (it takes the state lock briefly).
/// Call once. The threads are daemons; they end with the process.
pub fn start_watcher(wanted: Arc<dyn Fn() -> Vec<(String, bool)> + Send + Sync>) {
    listen_for_address_changes();
    let _ = std::thread::Builder::new().name("VpnWatcher".into()).spawn(move || {
        std::thread::sleep(Duration::from_secs(8)); // let VPN services come up after boot
        loop {
            enforce(&SysEnv, &wanted());
            let (m, cv) = &*CHANGED;
            let g = m.lock().unwrap_or_else(|p| p.into_inner());
            let (mut g, timeout) = cv.wait_timeout_while(g, Duration::from_secs(120), |c| !*c).unwrap_or_else(|p| p.into_inner());
            if !timeout.timed_out() {
                *g = false;
                drop(g);
                // After a change, give the VPN time to settle.
                std::thread::sleep(Duration::from_secs(20));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    struct Fake {
        json: String,
        files: Vec<PathBuf>,
        /// Output of `status` and of rasdial without args; connect flips `connected`.
        connected: Mutex<bool>,
        connect_works: bool,
        calls: Mutex<Vec<String>>,
        sleeps: AtomicUsize,
    }

    impl Fake {
        fn new(json: &str) -> Fake {
            Fake { json: json.into(), files: vec![], connected: Mutex::new(false), connect_works: true, calls: Mutex::new(vec![]), sleeps: AtomicUsize::new(0) }
        }
    }

    impl Env for Fake {
        fn exists(&self, p: &Path) -> bool {
            self.files.iter().any(|f| f == p)
        }
        fn run(&self, exe: &str, args: &[&str], _t: Duration) -> Result<RunResult> {
            let stem = Path::new(exe).file_stem().unwrap().to_string_lossy().to_lowercase();
            self.calls.lock().unwrap().push(format!("{stem} {}", args.join(" ")));
            let mut c = self.connected.lock().unwrap();
            let out = match (stem.as_str(), args) {
                ("mullvad", ["status"]) => (if *c { "Connected to se-got-wg-001" } else { "Disconnected" }).to_string(),
                ("mullvad", ["connect"]) => {
                    *c = self.connect_works;
                    String::new()
                }
                ("mullvad", ["disconnect"]) => {
                    *c = false;
                    String::new()
                }
                ("rasdial", []) => (if *c { "Connected to\nWork VPN\nCommand completed successfully." } else { "No connections" }).to_string(),
                _ => String::new(),
            };
            Ok(RunResult { exit_code: 0, output: out })
        }
        fn sleep(&self, _d: Duration) {
            self.sleeps.fetch_add(1, Relaxed);
        }
        fn scan_json(&self) -> Result<String> {
            Ok(self.json.clone())
        }
    }

    const JSON: &str = r#"{"adapters":[
        {"Name":"Ethernet","InterfaceDescription":"Realtek PCIe GbE","Up":true},
        {"Name":"Mullvad","InterfaceDescription":"Mullvad Tunnel","Up":false},
        {"Name":"vEthernet (WSL)","InterfaceDescription":"Hyper-V Virtual Ethernet Adapter","Up":true},
        {"Name":"ZeroTier One","InterfaceDescription":"ZeroTier Virtual Port","Up":true},
        {"Name":"WireGuard Tunnel: office","InterfaceDescription":"WireGuard Tunnel","Up":true}],
      "services":{"Name":"WireGuardTunnel$office"},
      "vpn":[{"Name":"Work VPN","Up":false}]}"#;

    #[test]
    fn matchers() {
        let r = |code: i32, o: &str| RunResult { exit_code: code, output: o.into() };
        assert!(Cli::Mullvad.is_connected(&r(0, "Connected to x")));
        assert!(!Cli::Mullvad.is_connected(&r(0, "Disconnected")));
        assert!(!Cli::Mullvad.is_connected(&r(0, "Connectedness")));
        assert!(Cli::Mullvad.is_connecting(&r(0, "  Connecting to x")));
        assert!(Cli::Warp.is_connected(&r(0, "Status update: Connected\nReason: x")));
        assert!(Cli::Warp.is_connecting(&r(0, "Status update:   Connecting")));
        assert!(Cli::Tailscale.is_connected(&r(0, "100.1.2.3 pc")));
        assert!(!Cli::Tailscale.is_connected(&r(0, "Tailscale is stopped.")));
        assert!(!Cli::Tailscale.is_connected(&r(1, "")));
    }

    #[test]
    fn scan_finds_each_kind_once() {
        let mut f = Fake::new(JSON);
        f.files.push(Cli::Mullvad.exe());
        let l = scan(&f).unwrap();
        let ids: Vec<&str> = l.iter().map(|e| e.id.as_str()).collect();
        // Mullvad via CLI, WireGuard by service (status only, adapter claimed), Windows profile, ZeroTier generic.
        assert_eq!(ids, ["cli:Mullvad VPN", "adapter:WireGuard", "ras:Work VPN", "adapter:ZeroTier One"]);
        assert!(l[0].controllable && !l[0].connected && l[0].detail == "Disconnected");
        assert!(!l[1].controllable && l[1].connected);
        assert!(l[2].controllable);
    }

    #[test]
    fn connect_polls_until_state_matches_and_cli_failure_cancels() {
        let mut f = Fake::new(JSON);
        f.files.push(Cli::Mullvad.exe());
        let no = AtomicBool::new(false);
        set(&f, "cli:Mullvad VPN", true, &no).unwrap();
        assert!(*f.connected.lock().unwrap());
        set(&f, "cli:Mullvad VPN", false, &no).unwrap();
        assert!(!*f.connected.lock().unwrap());

        let mut bad = Fake::new(JSON);
        bad.files.push(Cli::Mullvad.exe());
        bad.connect_works = false;
        let e = set(&bad, "cli:Mullvad VPN", true, &no).unwrap_err().to_string();
        assert!(e.contains("did not connect within 15 seconds and was cancelled"), "{e}");
        assert_eq!(bad.calls.lock().unwrap().last().unwrap(), "mullvad disconnect");
        assert!(gate().in_flight.is_empty() || !gate().in_flight.contains("cli:Mullvad VPN"));
        assert!(set(&bad, "adapter:WireGuard", true, &no).is_err());
    }

    #[test]
    fn rasdial_toggle_and_enforce() {
        let f = Fake::new(JSON);
        *f.connected.lock().unwrap() = true;
        // wanted off while on: watcher switches it off with rasdial /disconnect
        enforce(&f, &[("ras:Work VPN".into(), false)]);
        assert!(f.calls.lock().unwrap().contains(&"rasdial Work VPN /disconnect".to_string()));
        // unknown ids and status-only entries are left alone
        let before = f.calls.lock().unwrap().len();
        enforce(&f, &[("adapter:ZeroTier One".into(), true)]);
        assert_eq!(f.calls.lock().unwrap().len(), before);
    }

    #[test]
    fn remembered_state_roundtrips_and_keeps_other_keys() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("state.json"), r#"{"Games":[{"Name":"x"}],"Vpns":{"ras:a":true}}"#).unwrap();
        let mut st = Store::load(d.path());
        assert_eq!(wanted(&st), vec![("ras:a".to_string(), true)]);
        remember(&mut st, "cli:Mullvad VPN", false).unwrap();
        let st2 = Store::load(d.path());
        let mut w = wanted(&st2);
        w.sort();
        assert_eq!(w, vec![("cli:Mullvad VPN".to_string(), false), ("ras:a".to_string(), true)]);
        assert!(st2.top("Games").is_some());
    }
}
