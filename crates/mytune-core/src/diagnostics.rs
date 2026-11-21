//! Diagnostics page, without the benchmark (port of `Services/DriverInfo.cs`, `ProcessMonitor.cs`).
//! Network use per process and the driver latency scan need ETW and live in `etw.rs`.

use crate::error::{Error, Result};
use crate::special::powershell;
use crate::tweaks::system_cmd;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

// ---------------------------------------------------------------- drivers

#[derive(Debug, Clone, PartialEq)]
pub struct DriverRow {
    pub device: String,
    pub kind: &'static str,
    pub maker: String,
    pub version: String,
    /// (year, month, day)
    pub date: Option<(i32, u32, u32)>,
    pub url: Option<&'static str>,
}

fn contains_ci(hay: &str, needle: &str) -> bool {
    hay.to_ascii_lowercase().contains(&needle.to_ascii_lowercase())
}

fn download_for(maker: &str, device: &str) -> Option<&'static str> {
    let s = format!("{maker} {device}");
    if contains_ci(&s, "NVIDIA") {
        Some("https://www.nvidia.com/Download/index.aspx")
    } else if contains_ci(&s, "Advanced Micro") || contains_ci(&s, "AMD") || contains_ci(&s, "Radeon") {
        Some("https://www.amd.com/en/support/download/drivers.html")
    } else if contains_ci(&s, "Intel") || contains_ci(&s, "Killer") {
        Some("https://www.intel.com/content/www/us/en/download-center/home.html")
    } else if contains_ci(&s, "Realtek") {
        Some("https://www.realtek.com/Download/List?cate_id=584")
    } else {
        None
    }
}

fn parse_date(s: &str) -> Option<(i32, u32, u32)> {
    let mut it = s.trim().split('-');
    let (y, m, d) = (it.next()?.parse().ok()?, it.next()?.parse().ok()?, it.next()?.parse().ok()?);
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then_some((y, m, d))
}

/// Reads the installed graphics, network, audio and chipset drivers (PowerShell `Win32_PnPSignedDriver`, slow).
pub fn read_drivers() -> Result<Vec<DriverRow>> {
    let script = "ConvertTo-Json -Compress -InputObject @(Get-CimInstance Win32_PnPSignedDriver | Where-Object { $_.DeviceName -and (\
($_.DeviceClass -in 'DISPLAY','NET','MEDIA' -and $_.DeviceID -like 'PCI*') -or \
($_.DeviceClass -eq 'SYSTEM' -and $_.Manufacturer -match 'Intel|Advanced Micro|AMD' -and $_.DeviceName -match 'Chipset|Management Engine|SMBus|Serial IO|GPIO|Platform') ) } | \
ForEach-Object { [pscustomobject]@{ N=$_.DeviceName; C=$_.DeviceClass; M=$_.Manufacturer; V=$_.DriverVersion; D=if($_.DriverDate){$_.DriverDate.ToString('yyyy-MM-dd')}else{''} } })";
    let out = powershell(&*system_cmd(), script)?;
    let t = out.trim();
    let v: serde_json::Value = serde_json::from_str(if t.is_empty() { "[]" } else { t })?;
    let get = |d: &serde_json::Value, k: &str| d.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let rows = v
        .as_array()
        .map(|a| {
            a.iter()
                .map(|d| {
                    let kind = match get(d, "C").as_str() {
                        "DISPLAY" => "Graphics",
                        "NET" => "Network",
                        "MEDIA" => "Audio",
                        _ => "Chipset",
                    };
                    let (maker, name) = (get(d, "M"), get(d, "N"));
                    DriverRow { url: download_for(&maker, &name), device: name, kind, maker, version: get(d, "V"), date: parse_date(&get(d, "D")) }
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(select_drivers(rows))
}

/// Only the drivers that matter for games: skip the integrated GPU when there is a dedicated one, HDMI/GPU audio,
/// and system plumbing unless it is really the chipset driver. One row per (device, version), in a fixed order.
pub fn select_drivers(rows: Vec<DriverRow>) -> Vec<DriverRow> {
    let dedicated = rows.iter().any(|r| r.kind == "Graphics" && !contains_ci(&r.maker, "Intel"));
    let audio_skip = ["HDMI", "Display Audio", "NVIDIA", "Smart Sound", "Radeon"];
    let mut rows: Vec<DriverRow> = rows
        .into_iter()
        .filter(|r| match r.kind {
            "Graphics" => !(dedicated && contains_ci(&r.maker, "Intel")),
            "Chipset" => contains_ci(&r.device, "Chipset"),
            "Audio" => {
                let hd_gpu = contains_ci(&r.device, "High Definition Audio") && (contains_ci(&r.device, "NVIDIA") || contains_ci(&r.device, "AMD"));
                !(hd_gpu || audio_skip.iter().any(|k| contains_ci(&r.device, k)))
            }
            _ => true,
        })
        .collect();
    let order = |k: &str| ["Graphics", "Chipset", "Network", "Audio"].iter().position(|o| *o == k).unwrap_or(9);
    rows.sort_by(|a, b| order(a.kind).cmp(&order(b.kind)).then_with(|| a.device.cmp(&b.device)));
    let mut seen = std::collections::HashSet::new();
    rows.retain(|r| seen.insert((r.device.clone(), r.version.clone())));
    rows
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChipLevel {
    Good,
    Neutral,
    Warn,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DriverView {
    pub title: String,
    pub detail: String,
    pub age: (ChipLevel, String),
    pub url: Option<&'static str>,
}

/// Days since 1970-01-01 of a civil date.
pub fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y } as i64;
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

pub fn today_days() -> i64 {
    (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) / 86400) as i64
}

/// Texts of one driver row (`DriverRowVM`).
pub fn driver_view(r: &DriverRow, today: i64) -> DriverView {
    // Windows reports placeholder dates (1968, 2006 and similar) for built-in INF packages, so old dates can't be trusted.
    let date = r.date.filter(|&(y, m, d)| y >= 2015 && days_from_civil(y, m, d) <= today);
    let detail = format!(
        "Version {}{}{}",
        r.version,
        match date {
            Some((y, m, d)) => format!(", dated {y:04}-{m:02}-{d:02}"),
            None => ", date not reported".into(),
        },
        if r.maker.is_empty() { String::new() } else { format!(", {}", r.maker) }
    );
    let years = date.map(|(y, m, d)| (today - days_from_civil(y, m, d)) as f64 / 365.25);
    // Graphics drivers change often; chipset, network and audio drivers are fine for several years.
    let (fresh, stale) = if r.kind == "Graphics" { (1.0, 2.0) } else { (2.0, 4.0) };
    let age = if contains_ci(&r.maker, "Microsoft") && r.kind == "Graphics" {
        (ChipLevel::Warn, "Generic Windows driver".to_string())
    } else {
        match years {
            None => (ChipLevel::Neutral, "Date not reported".to_string()),
            Some(y) if y < fresh => (ChipLevel::Good, "Recent".to_string()),
            Some(y) if y < stale => (ChipLevel::Neutral, format!("{y:.1} years old")),
            Some(y) => (ChipLevel::Warn, format!("{y:.1} years old")),
        }
    };
    DriverView { title: format!("{}: {}", r.kind, r.device), detail, age, url: r.url }
}

// ---------------------------------------------------------------- processes

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessSample {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f64,
    pub disk_mb_per_sec: f64,
    pub net_kb_per_sec: f64,
    pub memory_mb: f64,
    pub can_end: bool,
    pub path: Option<String>,
    /// Program name without the "(n)" suffix; `end` takes this.
    pub base: String,
}

/// Never offered for ending: Windows itself, security software, anti-cheat and MyTune.
const PROTECTED: &[&str] = &[
    "system", "idle", "registry", "smss", "csrss", "wininit", "winlogon", "services", "lsass", "svchost", "dwm", "explorer", "sihost", "ctfmon",
    "taskhostw", "runtimebroker", "audiodg", "fontdrvhost", "conhost", "startmenuexperiencehost", "shellexperiencehost", "searchhost",
    "textinputhost", "securityhealthsystray", "securityhealthservice", "msmpeng", "nissrv", "mytune", "mytune-ui", "vgc", "vgtray", "easyanticheat",
    "easyanticheat_eos", "beservice", "battleye", "eaanticheat", "faceitclient", "faceit", "ricochet", "gameguard",
];

/// Rules of `ProcessMonitor.CanEnd`, without the OS lookups.
pub fn can_end_rules(pid: u32, own_pid: u32, session: u32, name: &str, path: Option<&str>, windir: &str, game_paths: &[String]) -> bool {
    if pid <= 4 || pid == own_pid || session == 0 {
        return false;
    }
    if PROTECTED.iter().any(|p| p.eq_ignore_ascii_case(name)) {
        return false;
    }
    let Some(path) = path else { return false };
    if !windir.is_empty() && path.to_ascii_lowercase().starts_with(&windir.to_ascii_lowercase()) {
        return false;
    }
    // Never offer a game from Game Hub.
    !game_paths.iter().any(|g| !g.is_empty() && g.eq_ignore_ascii_case(path))
}

/// Per-process CPU/disk/network rates from two counter readings.
/// `prev`/`cur` hold (cpu in 100 ns, io bytes, net bytes).
pub fn rates(prev: (i64, u64, u64), cur: (i64, u64, u64), seconds: f64, cores: f64) -> (f64, f64, f64) {
    let cpu = ((cur.0 - prev.0) as f64 / 1e7 / seconds / cores * 100.0).max(0.0);
    let disk = (cur.1.saturating_sub(prev.1)) as f64 / 1_048_576.0 / seconds;
    let net = (cur.2.saturating_sub(prev.2)) as f64 / 1024.0 / seconds;
    (cpu, disk, net)
}

/// One row per program name (browsers and launchers run many processes).
pub fn group_by_name(all: Vec<ProcessSample>) -> Vec<ProcessSample> {
    let mut order: Vec<String> = vec![];
    let mut groups: HashMap<String, Vec<ProcessSample>> = HashMap::new();
    for s in all {
        let key = s.name.to_ascii_lowercase();
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(s);
    }
    order
        .into_iter()
        .map(|k| {
            let g = &groups[&k];
            let first = &g[0];
            ProcessSample {
                pid: first.pid,
                name: format!("{}{}", first.name, if g.len() > 1 { format!(" ({})", g.len()) } else { String::new() }),
                cpu_percent: g.iter().map(|x| x.cpu_percent).sum(),
                disk_mb_per_sec: g.iter().map(|x| x.disk_mb_per_sec).sum(),
                net_kb_per_sec: g.iter().map(|x| x.net_kb_per_sec).sum(),
                memory_mb: g.iter().map(|x| x.memory_mb).sum(),
                can_end: g.iter().all(|x| x.can_end),
                path: first.path.clone(),
                base: first.name.clone(),
            }
        })
        .collect()
}

/// Bytes sent and received per pid, filled by the network trace while it runs.
pub type NetBytes = Arc<Mutex<HashMap<u32, u64>>>;

struct Handle(windows::Win32::Foundation::HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.0);
        }
    }
}

fn open(pid: u32, access: u32) -> Option<Handle> {
    use windows::Win32::System::Threading::{OpenProcess, PROCESS_ACCESS_RIGHTS};
    unsafe { OpenProcess(PROCESS_ACCESS_RIGHTS(access), false, pid).ok().map(Handle) }
}

const QUERY_LIMITED: u32 = 0x1000;
const TERMINATE: u32 = 0x0001;

fn image_path(h: &Handle) -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::System::Threading::{QueryFullProcessImageNameW, PROCESS_NAME_FORMAT};
    let mut buf = [0u16; 520];
    let mut size = buf.len() as u32;
    unsafe { QueryFullProcessImageNameW(h.0, PROCESS_NAME_FORMAT(0), PWSTR(buf.as_mut_ptr()), &mut size).ok()? };
    Some(String::from_utf16_lossy(&buf[..size as usize]))
}

fn session_of(pid: u32) -> u32 {
    let mut s = 0u32;
    unsafe {
        let _ = windows::Win32::System::RemoteDesktop::ProcessIdToSessionId(pid, &mut s);
    }
    s
}

/// (pid, name without .exe) of every running process.
fn list_processes() -> Result<Vec<(u32, String)>> {
    use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
    let mut out = vec![];
    unsafe {
        let snap = Handle(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(|e| Error::Os(format!("process snapshot: {e}")))?);
        let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
        let mut more = Process32FirstW(snap.0, &mut e).is_ok();
        while more {
            let end = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
            let file = String::from_utf16_lossy(&e.szExeFile[..end]);
            let name = file.strip_suffix(".exe").or_else(|| file.strip_suffix(".EXE")).unwrap_or(&file).to_string();
            out.push((e.th32ProcessID, name));
            more = Process32NextW(snap.0, &mut e).is_ok();
        }
    }
    Ok(out)
}

fn windir() -> String {
    std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into())
}

/// Per-process CPU, disk, network and memory, sampled on demand. The first sample has no rates (all 0).
pub struct ProcessMonitor {
    prev: HashMap<u32, (i64, u64, u64)>,
    prev_at: Option<Instant>,
    net: Option<NetBytes>,
}

impl ProcessMonitor {
    pub fn new(net: Option<NetBytes>) -> ProcessMonitor {
        ProcessMonitor { prev: HashMap::new(), prev_at: None, net }
    }

    pub fn network_available(&self) -> bool {
        self.net.is_some()
    }

    /// `game_paths`: exe paths of the Game Hub games (never offered for ending).
    pub fn sample(&mut self, game_paths: &[String]) -> Result<Vec<ProcessSample>> {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS};
        use windows::Win32::System::Threading::{GetProcessIoCounters, GetProcessTimes, IO_COUNTERS};
        let now = Instant::now();
        let seconds = self.prev_at.map(|t| now.duration_since(t).as_secs_f64()).unwrap_or(0.0);
        let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1) as f64;
        let own = std::process::id();
        let windir = windir();
        let net_now: HashMap<u32, u64> = self.net.as_ref().and_then(|n| n.lock().ok().map(|m| m.clone())).unwrap_or_default();
        let mut current = HashMap::new();
        let mut result = vec![];
        for (pid, name) in list_processes()? {
            if pid == 0 {
                continue; // "System Idle Process" is just unused CPU
            }
            let (mut cpu, mut io, mut path, mut mem) = (0i64, 0u64, None, 0.0);
            if let Some(h) = open(pid, QUERY_LIMITED) {
                let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
                let q = |f: FILETIME| (((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64) as i64;
                if unsafe { GetProcessTimes(h.0, &mut c, &mut e, &mut k, &mut u) }.is_ok() {
                    cpu = q(k) + q(u);
                }
                let mut counters = IO_COUNTERS::default();
                if unsafe { GetProcessIoCounters(h.0, &mut counters) }.is_ok() {
                    io = counters.ReadTransferCount + counters.WriteTransferCount;
                }
                let mut m = PROCESS_MEMORY_COUNTERS { cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32, ..Default::default() };
                if unsafe { K32GetProcessMemoryInfo(h.0, &mut m, m.cb) }.as_bool() {
                    mem = m.WorkingSetSize as f64 / 1_048_576.0;
                }
                path = image_path(&h);
            }
            let net = net_now.get(&pid).copied().unwrap_or(0);
            current.insert(pid, (cpu, io, net));
            let (cpu_pct, disk, net_kb) = match self.prev.get(&pid) {
                Some(&old) if seconds > 0.0 => rates(old, (cpu, io, net), seconds, cores),
                _ => (0.0, 0.0, 0.0),
            };
            let can_end = can_end_rules(pid, own, session_of(pid), &name, path.as_deref(), &windir, game_paths);
            result.push(ProcessSample { pid, base: name.clone(), name, cpu_percent: cpu_pct, disk_mb_per_sec: disk, net_kb_per_sec: net_kb, memory_mb: mem, can_end, path });
        }
        self.prev = current;
        self.prev_at = Some(now);
        Ok(group_by_name(result))
    }

    /// Forget the readings (the network trace was restarted).
    pub fn reset(&mut self) {
        self.prev.clear();
        self.prev_at = None;
    }
}

/// Ends every process with this program name, re-checking `can_end` (the list may be a few seconds old).
/// Returns how many were ended.
pub fn end_processes(base: &str, game_paths: &[String]) -> usize {
    use windows::Win32::System::Threading::TerminateProcess;
    let windir = windir();
    let own = std::process::id();
    let mut n = 0;
    for (pid, name) in list_processes().unwrap_or_default() {
        if !name.eq_ignore_ascii_case(base) {
            continue;
        }
        let path = open(pid, QUERY_LIMITED).and_then(|h| image_path(&h));
        if !can_end_rules(pid, own, session_of(pid), &name, path.as_deref(), &windir, game_paths) {
            continue;
        }
        match open(pid, TERMINATE) {
            Some(h) if unsafe { TerminateProcess(h.0, 1) }.is_ok() => n += 1,
            _ => crate::log::info(&format!("could not end {base} ({pid})")),
        }
    }
    crate::log::info(&format!("ended {n} process(es) named {base}"));
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(kind: &'static str, device: &str, maker: &str, date: Option<(i32, u32, u32)>) -> DriverRow {
        DriverRow { device: device.into(), kind, maker: maker.into(), version: "1.0".into(), date, url: download_for(maker, device) }
    }

    #[test]
    fn civil_days() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        assert_eq!(days_from_civil(2026, 11, 10), 20767);
    }

    #[test]
    fn drivers_filtered_and_ordered() {
        let rows = vec![
            row("Audio", "Realtek High Definition Audio", "Realtek", None),
            row("Audio", "NVIDIA High Definition Audio", "NVIDIA", None),
            row("Graphics", "Intel UHD Graphics", "Intel Corporation", None),
            row("Graphics", "NVIDIA GeForce RTX 3070 Ti", "NVIDIA", None),
            row("Chipset", "Intel SMBus", "Intel", None),
            row("Chipset", "Intel Chipset SATA", "Intel", None),
            row("Network", "Intel Ethernet", "Intel", None),
            row("Network", "Intel Ethernet", "Intel", None),
        ];
        let kinds: Vec<_> = select_drivers(rows).iter().map(|r| (r.kind, r.device.clone())).collect();
        assert_eq!(
            kinds,
            vec![
                ("Graphics", "NVIDIA GeForce RTX 3070 Ti".to_string()),
                ("Chipset", "Intel Chipset SATA".to_string()),
                ("Network", "Intel Ethernet".to_string()),
                ("Audio", "Realtek High Definition Audio".to_string()),
            ]
        );
    }

    #[test]
    fn driver_age_chips() {
        let today = days_from_civil(2026, 11, 10);
        let v = driver_view(&row("Graphics", "NVIDIA GeForce", "NVIDIA", Some((2026, 5, 1))), today);
        assert_eq!(v.age, (ChipLevel::Good, "Recent".into()));
        assert_eq!(v.detail, "Version 1.0, dated 2026-05-01, NVIDIA");
        let v = driver_view(&row("Graphics", "NVIDIA GeForce", "NVIDIA", Some((2024, 5, 1))), today);
        assert_eq!(v.age.0, ChipLevel::Warn); // 2.5 years, graphics: stale after 2
        let v = driver_view(&row("Network", "Intel Ethernet", "Intel", Some((2024, 5, 1))), today);
        assert_eq!(v.age.0, ChipLevel::Neutral); // same age is fine for network
        let v = driver_view(&row("Audio", "x", "", Some((2006, 6, 21))), today);
        assert_eq!(v.age, (ChipLevel::Neutral, "Date not reported".into())); // placeholder date
        assert_eq!(v.detail, "Version 1.0, date not reported");
        let v = driver_view(&row("Graphics", "Microsoft Basic Display", "Microsoft", None), today);
        assert_eq!(v.age, (ChipLevel::Warn, "Generic Windows driver".into()));
        assert!(driver_view(&row("Graphics", "NVIDIA", "NVIDIA", None), today).url.is_some());
    }

    #[test]
    fn end_rules() {
        let games = vec![r"C:\Games\game.exe".to_string()];
        let ok = |pid, name, path: Option<&str>| can_end_rules(pid, 100, 1, name, path, r"C:\Windows", &games);
        assert!(ok(500, "notepad", Some(r"C:\Program Files\x\notepad.exe")));
        assert!(!ok(4, "system", None));
        assert!(!ok(100, "mytune", Some(r"C:\a.exe"))); // own pid
        assert!(!ok(500, "Explorer", Some(r"D:\explorer.exe"))); // protected name
        assert!(!ok(500, "foo", Some(r"c:\windows\system32\foo.exe"))); // under Windows
        assert!(!ok(500, "foo", None)); // path unknown
        assert!(!ok(500, "game", Some(r"c:\games\GAME.exe"))); // Game Hub game
        assert!(!can_end_rules(500, 100, 0, "foo", Some(r"D:\f.exe"), r"C:\Windows", &[])); // session 0
    }

    #[test]
    fn rate_math_and_grouping() {
        let (cpu, disk, net) = rates((0, 0, 0), (10_000_000, 2_097_152, 2048), 2.0, 4.0);
        assert!((cpu - 12.5).abs() < 1e-9); // 1 s of CPU in 2 s on 4 cores
        assert!((disk - 1.0).abs() < 1e-9);
        assert!((net - 1.0).abs() < 1e-9);
        let s = |pid, name: &str, cpu, can| ProcessSample {
            pid,
            name: name.into(),
            cpu_percent: cpu,
            disk_mb_per_sec: 0.0,
            net_kb_per_sec: 0.0,
            memory_mb: 10.0,
            can_end: can,
            path: None,
            base: name.into(),
        };
        let g = group_by_name(vec![s(1, "chrome", 1.0, true), s(2, "Chrome", 2.0, false), s(3, "calc", 0.5, true)]);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].name, "chrome (2)");
        assert_eq!(g[0].cpu_percent, 3.0);
        assert_eq!(g[0].memory_mb, 20.0);
        assert!(!g[0].can_end); // one of them is protected: the group is too
        assert_eq!(g[0].base, "chrome");
    }

    #[test]
    fn real_sample_sees_this_process() {
        let mut m = ProcessMonitor::new(None);
        m.sample(&[]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        let list = m.sample(&[]).unwrap();
        assert!(list.len() > 10);
        // a row is a name group and carries only its first pid: another copy of this test binary may hold it
        let exe = std::env::current_exe().unwrap();
        let own = exe.file_stem().unwrap().to_string_lossy();
        let me = list.iter().find(|s| s.base.eq_ignore_ascii_case(&own)).expect("own process listed");
        assert!(!me.can_end);
        assert!(me.memory_mb > 1.0);
    }
}
