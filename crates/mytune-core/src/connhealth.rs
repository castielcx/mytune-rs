//! Connection health: a one-shot, read-only check of the active physical adapter (router ping, adapter
//! errors, Ethernet link speed, Wi-Fi signal, band and channel). Changes no setting and stores nothing.
//! Uses only numeric sources (PowerShell object properties, ICMP and Native WiFi APIs): Windows may be
//! localized, so no command's text output is parsed.

use crate::special::{is_physical, powershell};
use crate::tweaks::Cmd;
use serde::Deserialize;
use std::net::Ipv4Addr;
use std::time::{Duration, Instant};

const ECHOES: usize = 50;
const ECHO_GAP: Duration = Duration::from_millis(100);
const ECHO_TIMEOUT_MS: u32 = 1000;
const GBPS: u64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grade {
    Good,
    Warning,
    Problem,
    Info,
    NotMeasured,
}

impl Grade {
    pub fn label(self) -> &'static str {
        match self {
            Grade::Good => "Good",
            Grade::Warning => "Warning",
            Grade::Problem => "Problem",
            Grade::Info => "Info",
            Grade::NotMeasured => "Not measured",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub name: &'static str,
    pub text: String,
    pub fix: Option<&'static str>,
    pub grade: Grade,
}

fn row(name: &'static str, grade: Grade, text: impl Into<String>, fix: Option<&'static str>) -> Row {
    Row { name, text: text.into(), fix, grade }
}

// ---------------------------------------------------------------- adapter pick

#[derive(Deserialize, Debug, Clone)]
struct Route {
    #[serde(rename = "I")]
    index: i64,
    #[serde(rename = "G")]
    gateway: Option<String>,
    #[serde(rename = "M")]
    metric: i64,
}

#[derive(Deserialize, Debug, Clone)]
struct Adapter {
    #[serde(rename = "I")]
    index: i64,
    #[serde(rename = "D", default)]
    description: Option<String>,
    #[serde(rename = "T")]
    kind: i64,
    #[serde(rename = "U")]
    up: bool,
    #[serde(rename = "Id", default)]
    guid: String,
    #[serde(rename = "RX", default)]
    rx_bps: Option<u64>,
    #[serde(rename = "TX", default)]
    tx_bps: Option<u64>,
    /// Errors plus discards, received and outbound.
    #[serde(rename = "B", default)]
    bad: Option<u64>,
    #[serde(rename = "P", default)]
    packets: Option<u64>,
    /// `*SpeedDuplex` values the driver offers.
    #[serde(rename = "V", default)]
    speed_values: Vec<String>,
    /// `*SpeedDuplex` value in use ("0" = auto).
    #[serde(rename = "C", default)]
    speed_set: String,
}

#[derive(Deserialize, Debug)]
struct Snapshot {
    #[serde(rename = "R", default)]
    routes: Vec<Route>,
    #[serde(rename = "A", default)]
    adapters: Vec<Adapter>,
}

const SNAPSHOT: &str = r#"
$r=@(Get-NetRoute -AddressFamily IPv4 -DestinationPrefix '0.0.0.0/0' -ErrorAction SilentlyContinue | ForEach-Object {
 $m=(Get-NetIPInterface -InterfaceIndex $_.ifIndex -AddressFamily IPv4 -ErrorAction SilentlyContinue).InterfaceMetric
 [pscustomobject]@{I=[int]$_.ifIndex;G=[string]$_.NextHop;M=[int64]$_.RouteMetric+[int64]$m} });
$st=@{};Get-NetAdapterStatistics -ErrorAction SilentlyContinue | ForEach-Object {$st[$_.Name]=$_};
$sd=@{};Get-NetAdapterAdvancedProperty -AllProperties -RegistryKeyword '*SpeedDuplex' -ErrorAction SilentlyContinue | ForEach-Object {$sd[$_.Name]=$_};
$a=@(Get-NetAdapter -ErrorAction SilentlyContinue | ForEach-Object {
 $s=$st[$_.Name];$d=$sd[$_.Name];$b=$null;$p=$null
 if($s){
  $b=[uint64]$s.ReceivedPacketErrors+[uint64]$s.ReceivedDiscardedPackets+[uint64]$s.OutboundPacketErrors+[uint64]$s.OutboundDiscardedPackets
  $p=[uint64]$s.ReceivedUnicastPackets+[uint64]$s.ReceivedMulticastPackets+[uint64]$s.ReceivedBroadcastPackets+[uint64]$s.SentUnicastPackets+[uint64]$s.SentMulticastPackets+[uint64]$s.SentBroadcastPackets
 }
 [pscustomobject]@{I=[int]$_.ifIndex;D=$_.InterfaceDescription;T=[int]$_.InterfaceType;U=([int]$_.InterfaceOperationalStatus -eq 1);Id=[string]$_.InterfaceGuid;RX=$_.ReceiveLinkSpeed;TX=$_.TransmitLinkSpeed;B=$b;P=$p;V=@($d.ValidRegistryValues | Where-Object {$_});C=[string]($d.RegistryValue -join '')} });
ConvertTo-Json -InputObject @{R=$r;A=$a} -Compress -Depth 4
"#;

fn gateway(r: &Route) -> Option<Ipv4Addr> {
    r.gateway.as_deref()?.parse().ok().filter(|ip: &Ipv4Addr| !ip.is_unspecified())
}

/// The physical adapter that is Up with the lowest-metric default route, or else any physical one
/// that is Up (it then has no gateway to ping). A VPN's default route is skipped this way.
fn pick<'a>(adapters: &'a [Adapter], routes: &[Route]) -> Option<(&'a Adapter, Option<Ipv4Addr>)> {
    let usable = |a: &&Adapter| a.up && is_physical(a.kind, a.description.as_deref().unwrap_or_default());
    let mut sorted: Vec<&Route> = routes.iter().filter(|r| gateway(r).is_some()).collect();
    sorted.sort_by_key(|r| r.metric);
    sorted
        .iter()
        .find_map(|r| adapters.iter().filter(usable).find(|a| a.index == r.index).map(|a| (a, gateway(r))))
        .or_else(|| adapters.iter().find(usable).map(|a| (a, None)))
}

// ---------------------------------------------------------------- grading

/// Sends the echoes; stops after 5 if none of them was answered (the router may block ping).
fn sample(echo: &mut dyn FnMut() -> Option<u32>, gap: Duration) -> Vec<Option<u32>> {
    let mut out = Vec::with_capacity(ECHOES);
    for i in 0..ECHOES {
        let t = Instant::now();
        out.push(echo());
        if i == 4 && out.iter().all(Option::is_none) {
            break;
        }
        if i + 1 < ECHOES {
            std::thread::sleep(gap.saturating_sub(t.elapsed()));
        }
    }
    out
}

fn grade_ping(samples: &[Option<u32>], wifi: bool) -> Row {
    const NAME: &str = "Router ping";
    let mut ok: Vec<u32> = samples.iter().flatten().copied().collect();
    if samples.len() < ECHOES || ok.is_empty() {
        return row(NAME, Grade::NotMeasured, "The router did not answer ping. Many routers block it, so this says nothing about your connection.", None);
    }
    ok.sort_unstable();
    let lost = samples.len() - ok.len();
    let median = ok[ok.len() / 2];
    let p95 = ok[(ok.len() * 95).div_ceil(100) - 1];
    let ms = |v: u32| if v == 0 { "under 1 ms".to_string() } else { format!("{v} ms") };
    let text = format!("Lost {lost} of {}. Median {}, p95 {}.", samples.len(), ms(median), ms(p95));
    let link_fix = if wifi { "Move closer to the router or use a cable." } else { "Replace the cable or try another router port." };
    let limit = if wifi { 50 } else { 20 };
    match lost {
        3.. => row(NAME, Grade::Problem, text, Some(link_fix)),
        1..=2 => row(NAME, Grade::Warning, text, Some(link_fix)),
        0 if p95 > limit => row(NAME, Grade::Warning, format!("{text} Ping spikes above {limit} ms."), Some("Check for large downloads or uploads on your network, then run this again.")),
        _ => row(NAME, Grade::Good, text, None),
    }
}

fn grade_errors(bad: Option<u64>, packets: Option<u64>) -> Row {
    const NAME: &str = "Adapter errors";
    let (Some(bad), Some(packets)) = (bad, packets) else {
        return row(NAME, Grade::NotMeasured, "Windows did not report counters for this adapter.", None);
    };
    if packets == 0 {
        return row(NAME, Grade::NotMeasured, "No packets counted since the adapter started.", None);
    }
    let (b, p) = (bad as u128, packets as u128);
    let text = format!("{bad} bad of {packets} packets ({:.3}%) since the adapter started.", b as f64 * 100.0 / p as f64);
    const FIX: Option<&str> = Some("Replace the cable or update the network driver.");
    if b * 1000 < p {
        row(NAME, Grade::Good, text, None)
    } else if b * 100 < p {
        row(NAME, Grade::Warning, text, FIX)
    } else {
        row(NAME, Grade::Problem, text, FIX)
    }
}

/// Highest speed in the standard `*SpeedDuplex` values. Drivers also use their own numbers: values of
/// 100 and up are Mbit/s (Intel: 2500), the rest are ignored.
/// shortcut: an unknown vendor code can hide a faster mode, add it here if a card reports one.
fn max_speed(values: &[String]) -> Option<u64> {
    values
        .iter()
        .filter_map(|v| v.trim().parse::<u64>().ok())
        .filter_map(|v| match v {
            1 | 2 => Some(10_000_000),
            3 | 4 => Some(100_000_000),
            5 | 6 => Some(GBPS),
            7 => Some(10 * GBPS),
            100.. => Some(v * 1_000_000),
            _ => None,
        })
        .max()
}

fn speed_text(bps: u64) -> String {
    if bps >= GBPS {
        format!("{} Gbps", bps as f64 / GBPS as f64)
    } else {
        format!("{} Mbps", bps / 1_000_000)
    }
}

fn grade_link(link: Option<u64>, max: Option<u64>, forced: bool) -> Row {
    const NAME: &str = "Ethernet link speed";
    let Some(link) = link.filter(|v| *v > 0) else {
        return row(NAME, Grade::NotMeasured, "Windows did not report a link speed.", None);
    };
    let now = speed_text(link);
    if forced {
        return row(NAME, Grade::Info, format!("Link runs at {now}. The speed is set by hand in the adapter's driver settings."), None);
    }
    let Some(max) = max else {
        return row(NAME, Grade::Info, format!("Link runs at {now}. The card's maximum could not be read."), None);
    };
    if link < GBPS && max >= GBPS {
        row(NAME, Grade::Problem, format!("Link runs at {now}. The card supports {}.", speed_text(max)), Some("Replace the cable or try another router port."))
    } else if link < max {
        row(NAME, Grade::Info, format!("Link runs at {now}, the card supports {}. Not a fault: the router may top out at {now}.", speed_text(max)), None)
    } else {
        row(NAME, Grade::Good, format!("Link runs at {now}, the card's maximum."), None)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WifiLink {
    pub quality: u32,
    pub rx_mbps: u32,
    pub tx_mbps: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WifiScan {
    pub freq_khz: u32,
    pub others: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WifiReading {
    pub link: Result<WifiLink, String>,
    pub supports_high_band: bool,
    pub scan: Result<WifiScan, String>,
}

fn band_name(freq_khz: u32) -> &'static str {
    match freq_khz {
        ..=3_000_000 => "2.4 GHz",
        3_000_001..=5_924_999 => "5 GHz",
        _ => "6 GHz",
    }
}

fn grade_signal(link: &Result<WifiLink, String>) -> Row {
    const NAME: &str = "Wi-Fi signal";
    let l = match link {
        Ok(l) => l,
        Err(e) => return row(NAME, Grade::NotMeasured, e.as_str(), None),
    };
    let text = format!("Signal {}%. Link rate {} Mbps down, {} Mbps up.", l.quality, l.rx_mbps, l.tx_mbps);
    const FIX: Option<&str> = Some("Move closer to the router or use a cable.");
    match l.quality {
        60.. => row(NAME, Grade::Good, text, None),
        40..=59 => row(NAME, Grade::Warning, text, FIX),
        _ => row(NAME, Grade::Problem, text, FIX),
    }
}

fn grade_band(scan: &Result<WifiScan, String>, supports_high: bool) -> Row {
    const NAME: &str = "Wi-Fi band";
    let s = match scan {
        Ok(s) => s,
        Err(e) => return row(NAME, Grade::NotMeasured, e.as_str(), None),
    };
    let band = band_name(s.freq_khz);
    if band == "2.4 GHz" && supports_high {
        row(NAME, Grade::Warning, "Connected on 2.4 GHz, but this card also supports faster bands.", Some("Connect to your router's 5 GHz network."))
    } else {
        row(NAME, Grade::Good, format!("Connected on {band}."), None)
    }
}

fn grade_crowd(scan: &Result<WifiScan, String>) -> Row {
    const NAME: &str = "Wi-Fi channel";
    let s = match scan {
        Ok(s) => s,
        Err(e) => return row(NAME, Grade::NotMeasured, e.as_str(), None),
    };
    // 5 and 6 GHz signals reach less far, so the same count disturbs less: one step more lenient.
    let extra = u32::from(band_name(s.freq_khz) != "2.4 GHz");
    let text = format!("{} other networks on your channel.", s.others);
    const FIX: Option<&str> = Some("Pick a less used channel in your router's settings.");
    if s.others <= 2 + extra {
        row(NAME, Grade::Good, text, None)
    } else if s.others <= 5 + extra {
        row(NAME, Grade::Warning, text, FIX)
    } else {
        row(NAME, Grade::Problem, text, FIX)
    }
}

// ---------------------------------------------------------------- the check

/// Runs every check once (about 5 to 10 s). Call it on its own thread.
pub fn run(cmd: &dyn Cmd) -> Vec<Row> {
    run_with(cmd, &mut icmp_echo, &wifi_reading, ECHO_GAP)
}

fn run_with(cmd: &dyn Cmd, echo: &mut dyn FnMut(Ipv4Addr) -> Option<u32>, wifi: &dyn Fn(&str) -> WifiReading, gap: Duration) -> Vec<Row> {
    const NAME: &str = "Connection";
    let snap = match powershell(cmd, SNAPSHOT).and_then(|o| Ok(serde_json::from_str::<Snapshot>(o.trim())?)) {
        Ok(s) => s,
        Err(e) => {
            crate::log::error("connection health", Some(&e));
            return vec![row(NAME, Grade::NotMeasured, "Could not read the network adapters.", None)];
        }
    };
    let Some((a, gw)) = pick(&snap.adapters, &snap.routes) else {
        return vec![row(NAME, Grade::NotMeasured, "No connected Ethernet or Wi-Fi adapter found.", None)];
    };
    let is_wifi = a.kind == 71;
    let mut rows = vec![match gw {
        Some(ip) => grade_ping(&sample(&mut || echo(ip), gap), is_wifi),
        None => row("Router ping", Grade::NotMeasured, "This adapter has no IPv4 default gateway to ping.", None),
    }];
    rows.push(grade_errors(a.bad, a.packets));
    if is_wifi {
        let w = wifi(&a.guid);
        rows.push(grade_signal(&w.link));
        rows.push(grade_band(&w.scan, w.supports_high_band));
        rows.push(grade_crowd(&w.scan));
    } else {
        let link = a.rx_bps.into_iter().chain(a.tx_bps).min();
        let forced = !matches!(a.speed_set.trim(), "" | "0");
        rows.push(grade_link(link, max_speed(&a.speed_values), forced));
    }
    rows
}

// ---------------------------------------------------------------- ICMP

fn icmp_echo(ip: Ipv4Addr) -> Option<u32> {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::NetworkManagement::IpHelper::{IcmpCloseHandle, IcmpCreateFile, IcmpSendEcho, ICMP_ECHO_REPLY};
    struct Icmp(HANDLE);
    impl Drop for Icmp {
        fn drop(&mut self) {
            let _ = unsafe { IcmpCloseHandle(self.0) };
        }
    }
    let h = Icmp(unsafe { IcmpCreateFile() }.ok()?);
    let payload = [0x4Du8; 32];
    // u64 storage: the reply struct holds a pointer and must be aligned.
    let mut buf = [0u64; (size_of::<ICMP_ECHO_REPLY>() + 32 + 8 + 7) / 8 + 1];
    let n = unsafe {
        IcmpSendEcho(h.0, u32::from_ne_bytes(ip.octets()), payload.as_ptr().cast(), payload.len() as u16, None, buf.as_mut_ptr().cast(), size_of_val(&buf) as u32, ECHO_TIMEOUT_MS)
    };
    if n == 0 {
        return None;
    }
    let reply = unsafe { &*buf.as_ptr().cast::<ICMP_ECHO_REPLY>() };
    (reply.Status == 0).then_some(reply.RoundTripTime)
}

// ---------------------------------------------------------------- Native WiFi

mod wlan {
    use super::{WifiLink, WifiReading, WifiScan};
    use std::collections::HashSet;
    use std::ffi::c_void;
    use std::mem::{offset_of, size_of};
    use std::ptr::{addr_of, null_mut};
    use windows::core::GUID;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::NetworkManagement::WiFi::*;

    const ACCESS_DENIED: u32 = 5;
    /// Far above any real PC; the interface list carries no byte size to check against.
    const MAX_INTERFACES: usize = 64;
    /// `dot11_phy_type_ofdm` (802.11a) and `dot11_phy_type_vht` (802.11ac) exist only on 5 GHz.
    const HIGH_BAND_PHYS: [DOT11_PHY_TYPE; 2] = [dot11_phy_type_ofdm, dot11_phy_type_vht];

    struct Client(HANDLE);
    impl Drop for Client {
        fn drop(&mut self) {
            unsafe { WlanCloseHandle(self.0, None) };
        }
    }

    /// Memory handed out by wlanapi.
    struct Mem<T>(*mut T);
    impl<T> Drop for Mem<T> {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { WlanFreeMemory(self.0 as *const c_void) };
            }
        }
    }

    fn denied_or(code: u32, what: &str) -> String {
        if code == ACCESS_DENIED {
            "Windows denied the Wi-Fi scan. Allow location access for desktop apps in Windows Settings, Privacy, Location.".into()
        } else {
            format!("Windows could not read {what} (code {code}).")
        }
    }

    fn same_guid(g: &GUID, text: &str) -> bool {
        format!("{g:?}").eq_ignore_ascii_case(text.trim_matches(['{', '}']))
    }

    fn find_interface(c: &Client, guid: &str) -> Result<GUID, String> {
        let mut list = Mem::<WLAN_INTERFACE_INFO_LIST>(null_mut());
        let code = unsafe { WlanEnumInterfaces(c.0, None, &mut list.0) };
        if code != 0 || list.0.is_null() {
            return Err(denied_or(code, "the Wi-Fi interfaces"));
        }
        let n = unsafe { (*list.0).dwNumberOfItems } as usize;
        if n > MAX_INTERFACES {
            return Err("Windows returned an invalid Wi-Fi interface list.".into());
        }
        let first = unsafe { addr_of!((*list.0).InterfaceInfo) }.cast::<WLAN_INTERFACE_INFO>();
        (0..n)
            .map(|i| unsafe { addr_of!((*first.add(i)).InterfaceGuid).read_unaligned() })
            .find(|g| same_guid(g, guid))
            .ok_or_else(|| "Windows did not list this Wi-Fi adapter.".to_string())
    }

    /// Link attributes plus the connected network's SSID and BSSID.
    fn connection(c: &Client, g: &GUID) -> Result<(WifiLink, DOT11_SSID, [u8; 6]), String> {
        let (mut size, mut data) = (0u32, Mem::<c_void>(null_mut()));
        let code = unsafe { WlanQueryInterface(c.0, g, wlan_intf_opcode_current_connection, None, &mut size, &mut data.0, None) };
        if code != 0 || data.0.is_null() {
            return Err(denied_or(code, "the Wi-Fi connection"));
        }
        if (size as usize) < size_of::<WLAN_CONNECTION_ATTRIBUTES>() {
            return Err("Windows returned an invalid Wi-Fi connection.".into());
        }
        let a = unsafe { addr_of!((*data.0.cast::<WLAN_CONNECTION_ATTRIBUTES>()).wlanAssociationAttributes).read_unaligned() };
        let link = WifiLink { quality: a.wlanSignalQuality.min(100), rx_mbps: a.ulRxRate / 1000, tx_mbps: a.ulTxRate / 1000 };
        Ok((link, a.dot11Ssid, a.dot11Bssid))
    }

    fn supports_high_band(c: &Client, g: &GUID) -> bool {
        let mut cap = Mem::<WLAN_INTERFACE_CAPABILITY>(null_mut());
        if unsafe { WlanGetInterfaceCapability(c.0, g, None, &mut cap.0) } != 0 || cap.0.is_null() {
            return false;
        }
        let v = unsafe { cap.0.read_unaligned() };
        v.dot11PhyTypes.iter().take((v.dwNumberOfSupportedPhys as usize).min(64)).any(|p| HIGH_BAND_PHYS.contains(p))
    }

    fn ssid_bytes(s: &DOT11_SSID) -> &[u8] {
        &s.ucSSID[..(s.uSSIDLength as usize).min(32)]
    }

    /// Reads Windows' last scan (it does not start a new one).
    fn scan(c: &Client, g: &GUID, own_ssid: &DOT11_SSID, own_bssid: [u8; 6]) -> Result<WifiScan, String> {
        let mut list = Mem::<WLAN_BSS_LIST>(null_mut());
        let code = unsafe { WlanGetNetworkBssList(c.0, g, None, dot11_BSS_type_any, false, None, &mut list.0) };
        if code != 0 || list.0.is_null() {
            return Err(denied_or(code, "the Wi-Fi networks"));
        }
        let (total, n) = unsafe { ((*list.0).dwTotalSize as usize, (*list.0).dwNumberOfItems as usize) };
        let start = offset_of!(WLAN_BSS_LIST, wlanBssEntries);
        if n.checked_mul(size_of::<WLAN_BSS_ENTRY>()).and_then(|b| b.checked_add(start)).is_none_or(|end| end > total) {
            return Err("Windows returned an invalid Wi-Fi network list.".into());
        }
        let first = unsafe { addr_of!((*list.0).wlanBssEntries) }.cast::<WLAN_BSS_ENTRY>();
        // Fields one by one: the entry holds a `bool` the OS writes, never read it as a Rust value.
        let entries: Vec<(DOT11_SSID, [u8; 6], u32)> = (0..n)
            .map(|i| unsafe {
                let e = first.add(i);
                (addr_of!((*e).dot11Ssid).read_unaligned(), addr_of!((*e).dot11Bssid).read_unaligned(), addr_of!((*e).ulChCenterFrequency).read_unaligned())
            })
            .collect();
        let Some(freq_khz) = entries.iter().find(|e| e.1 == own_bssid).map(|e| e.2) else {
            return Err("Your router was not in Windows' last Wi-Fi scan.".into());
        };
        let own = ssid_bytes(own_ssid);
        // One router can send several BSSIDs per SSID: count names. A hidden network counts per BSSID.
        let others: HashSet<Vec<u8>> = entries
            .iter()
            .filter(|e| e.2 == freq_khz && ssid_bytes(&e.0) != own)
            .map(|e| if e.0.uSSIDLength == 0 { e.1.to_vec() } else { ssid_bytes(&e.0).to_vec() })
            .collect();
        Ok(WifiScan { freq_khz, others: others.len() as u32 })
    }

    pub fn reading(guid: &str) -> WifiReading {
        let fail = |e: String| WifiReading { link: Err(e.clone()), supports_high_band: false, scan: Err(e) };
        let (mut version, mut handle) = (0u32, HANDLE::default());
        let code = unsafe { WlanOpenHandle(2, None, &mut version, &mut handle) };
        if code != 0 {
            return fail(denied_or(code, "Wi-Fi"));
        }
        let c = Client(handle);
        let g = match find_interface(&c, guid) {
            Ok(g) => g,
            Err(e) => return fail(e),
        };
        let supports_high_band = supports_high_band(&c, &g);
        match connection(&c, &g) {
            Ok((link, ssid, bssid)) => WifiReading { link: Ok(link), supports_high_band, scan: scan(&c, &g, &ssid, bssid) },
            Err(e) => WifiReading { link: Err(e.clone()), supports_high_band, scan: Err(e) },
        }
    }
}

fn wifi_reading(guid: &str) -> WifiReading {
    wlan::reading(guid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::RunResult;

    struct Fake(&'static str);
    impl Cmd for Fake {
        fn run(&self, _: &str, _: &[&str]) -> crate::Result<RunResult> {
            Ok(RunResult { exit_code: 0, output: self.0.to_string() })
        }
    }

    fn adapter(index: i64, kind: i64, desc: &str, up: bool) -> Adapter {
        Adapter { index, description: Some(desc.into()), kind, up, guid: String::new(), rx_bps: None, tx_bps: None, bad: None, packets: None, speed_values: vec![], speed_set: String::new() }
    }

    fn route(index: i64, gw: &str, metric: i64) -> Route {
        Route { index, gateway: Some(gw.into()), metric }
    }

    fn pings(lost: usize, rtt: u32) -> Vec<Option<u32>> {
        (0..ECHOES).map(|i| if i >= ECHOES - lost { None } else { Some(rtt) }).collect()
    }

    #[test]
    fn pick_skips_vpn_and_uses_lowest_metric() {
        let a = [adapter(1, 6, "Intel I225-V", true), adapter(2, 71, "Intel AX211", true), adapter(3, 53, "TAP-Windows", true), adapter(4, 6, "WireGuard Tunnel", true)];
        let r = [route(4, "10.0.0.1", 1), route(3, "10.8.0.1", 2), route(2, "192.168.1.1", 40), route(1, "192.168.0.1", 25)];
        let (p, gw) = pick(&a, &r).unwrap();
        assert_eq!((p.index, gw), (1, Some(Ipv4Addr::new(192, 168, 0, 1))));
    }

    #[test]
    fn pick_up_adapter_without_route_and_none() {
        let a = [adapter(1, 6, "Intel", false), adapter(2, 71, "Intel Wi-Fi", true)];
        let (p, gw) = pick(&a, &[route(1, "192.168.0.1", 1), route(2, "0.0.0.0", 1)]).unwrap();
        assert_eq!((p.index, gw), (2, None));
        assert!(pick(&[adapter(5, 6, "Hyper-V Virtual Ethernet", true)], &[]).is_none());
    }

    #[test]
    fn ping_thresholds() {
        assert_eq!(grade_ping(&pings(0, 1), false).grade, Grade::Good);
        assert_eq!(grade_ping(&pings(1, 1), false).grade, Grade::Warning);
        assert_eq!(grade_ping(&pings(2, 1), true).grade, Grade::Warning);
        assert_eq!(grade_ping(&pings(3, 1), false).grade, Grade::Problem);
        assert_eq!(grade_ping(&pings(0, 21), false).grade, Grade::Warning);
        assert_eq!(grade_ping(&pings(0, 20), false).grade, Grade::Good);
        assert_eq!(grade_ping(&pings(0, 21), true).grade, Grade::Good);
        assert_eq!(grade_ping(&pings(0, 51), true).grade, Grade::Warning);
    }

    #[test]
    fn five_timeouts_stop_as_not_measured() {
        let mut calls = 0;
        let s = sample(&mut || { calls += 1; None }, Duration::ZERO);
        assert_eq!(calls, 5);
        assert_eq!(grade_ping(&s, false).grade, Grade::NotMeasured);
        let mut n = 0;
        let s = sample(&mut || { n += 1; (n == 5).then_some(1) }, Duration::ZERO);
        assert_eq!(s.len(), ECHOES); // one answer among the first five: keep going
        assert_eq!(grade_ping(&s, false).grade, Grade::Problem);
    }

    #[test]
    fn error_thresholds() {
        assert_eq!(grade_errors(Some(0), Some(1000)).grade, Grade::Good);
        assert_eq!(grade_errors(Some(9), Some(10_000)).grade, Grade::Good);
        assert_eq!(grade_errors(Some(10), Some(10_000)).grade, Grade::Warning);
        assert_eq!(grade_errors(Some(99), Some(10_000)).grade, Grade::Warning);
        assert_eq!(grade_errors(Some(100), Some(10_000)).grade, Grade::Problem);
        assert_eq!(grade_errors(Some(1), Some(0)).grade, Grade::NotMeasured);
        assert_eq!(grade_errors(None, None).grade, Grade::NotMeasured);
    }

    #[test]
    fn link_speed() {
        let i225: Vec<String> = ["0", "1", "2", "3", "4", "6", "2500"].map(String::from).to_vec();
        assert_eq!(max_speed(&i225), Some(2_500_000_000));
        assert_eq!(max_speed(&["0".to_string()]), None);
        let m = max_speed(&i225);
        assert_eq!(grade_link(Some(100_000_000), m, false).grade, Grade::Problem);
        assert_eq!(grade_link(Some(GBPS), m, false).grade, Grade::Info);
        assert_eq!(grade_link(Some(2_500_000_000), m, false).grade, Grade::Good);
        assert_eq!(grade_link(Some(100_000_000), Some(100_000_000), false).grade, Grade::Good);
        assert_eq!(grade_link(Some(100_000_000), None, false).grade, Grade::Info);
        assert_eq!(grade_link(Some(100_000_000), m, true).grade, Grade::Info);
        assert_eq!(grade_link(None, m, false).grade, Grade::NotMeasured);
    }

    #[test]
    fn wifi_grades() {
        let link = |quality| Ok(WifiLink { quality, rx_mbps: 866, tx_mbps: 866 });
        assert_eq!(grade_signal(&link(60)).grade, Grade::Good);
        assert_eq!(grade_signal(&link(59)).grade, Grade::Warning);
        assert_eq!(grade_signal(&link(40)).grade, Grade::Warning);
        assert_eq!(grade_signal(&link(39)).grade, Grade::Problem);
        let scan = |freq_khz, others| Ok(WifiScan { freq_khz, others });
        assert_eq!(grade_band(&scan(2_437_000, 0), true).grade, Grade::Warning);
        assert_eq!(grade_band(&scan(2_437_000, 0), false).grade, Grade::Good);
        assert_eq!(grade_band(&scan(5_180_000, 0), true).grade, Grade::Good);
        for (others, g) in [(2, Grade::Good), (3, Grade::Warning), (5, Grade::Warning), (6, Grade::Problem)] {
            assert_eq!(grade_crowd(&scan(2_437_000, others)).grade, g);
        }
        for (others, g) in [(3, Grade::Good), (4, Grade::Warning), (6, Grade::Warning), (7, Grade::Problem)] {
            assert_eq!(grade_crowd(&scan(5_180_000, others)).grade, g);
        }
        let denied = Err("denied".to_string());
        assert_eq!(grade_crowd(&denied).grade, Grade::NotMeasured);
        assert_eq!(grade_band(&denied, true).grade, Grade::NotMeasured);
    }

    #[test]
    fn run_with_fakes() {
        let none = Fake(r#"{"R":[],"A":[]}"#);
        let r = run_with(&none, &mut |_| None, &|_| unreachable!(), Duration::ZERO);
        assert_eq!((r.len(), r[0].grade), (1, Grade::NotMeasured));

        let eth = Fake(r#"{"R":[{"I":19,"G":"192.168.178.1","M":25}],"A":[{"I":19,"N":"Ethernet","D":"Intel I225-V","T":6,"U":true,"Id":"{X}","RX":1000000000,"TX":1000000000,"B":0,"P":5000,"V":["0","4","6","2500"],"C":"0"}]}"#);
        let r = run_with(&eth, &mut |_| Some(1), &|_| unreachable!(), Duration::ZERO);
        let grades: Vec<Grade> = r.iter().map(|x| x.grade).collect();
        assert_eq!(grades, [Grade::Good, Grade::Good, Grade::Info]);
    }

    /// Real read-only run: `cargo test -p mytune-core --lib connhealth::tests::live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live() {
        for r in run(&crate::tweaks::SystemCmd) {
            println!("[{}] {}: {}{}", r.grade.label(), r.name, r.text, r.fix.map(|f| format!(" Fix: {f}")).unwrap_or_default());
        }
    }
}
