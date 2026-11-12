//! PC check (port of C# `Services/PcCheck.cs`, first batch). Checks only read; `run_fix` writes.
//! Not ported yet: monitor refresh rate, G-SYNC, graphics notes, memory channels/speed, game drive,
//! connection type, temperatures, startup programs, timer resolution, background load.

use crate::engine::Engine;
use crate::process::run;
use crate::reg::{self, Hive, RegValue};
use crate::tweaks::{system_cmd, PowerPlanTweak, PLAN_GUID};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Sorts first.
    Warn,
    Tip,
    Ok,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fix {
    PowerPlan,
    GameMode,
}

impl Fix {
    pub fn label(self) -> &'static str {
        match self {
            Fix::PowerPlan => "Use performance plan",
            Fix::GameMode => "Turn on",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Check {
    pub title: &'static str,
    pub level: Level,
    pub message: String,
    pub fix: Option<Fix>,
}

fn c(title: &'static str, level: Level, message: impl Into<String>, fix: Option<Fix>) -> Check {
    Check { title, level, message: message.into(), fix }
}

/// Runs every check (the internet one pings for about 8 s: call from a worker thread).
pub fn run_all() -> Vec<Check> {
    let mut v: Vec<Check> = [power_plan(), battery(), game_mode(), mouse_accel(), memory(), system_drive(), resizable_bar(), internet()]
        .into_iter()
        .flatten()
        .collect();
    v.sort_by_key(|x| x.level);
    v
}

pub fn summary(checks: &[Check]) -> String {
    let warn = checks.iter().filter(|x| x.level == Level::Warn).count();
    let tip = checks.iter().filter(|x| x.level == Level::Tip).count();
    match (warn, tip) {
        (0, 0) => "Everything looks fine.".into(),
        (w, 0) => format!("{w} {} to check", if w == 1 { "thing" } else { "things" }),
        (0, t) => format!("{t} {}", if t == 1 { "tip" } else { "tips" }),
        (w, t) => format!("{w} {} to check and {t} {}", if w == 1 { "thing" } else { "things" }, if t == 1 { "tip" } else { "tips" }),
    }
}

pub fn run_fix(fix: Fix, engine: &Engine) -> crate::error::Result<()> {
    match fix {
        Fix::PowerPlan => engine.apply(&PowerPlanTweak { cmd: system_cmd(), laptop: on_battery_capable() }),
        Fix::GameMode => reg::set(Hive::Hkcu, "Software\\Microsoft\\GameBar", "AutoGameModeEnabled", &RegValue::Dword(1)),
    }
}

// ---------------------------------------------------------------- checks

fn power_status() -> Option<windows::Win32::System::Power::SYSTEM_POWER_STATUS> {
    let mut s = Default::default();
    unsafe { windows::Win32::System::Power::GetSystemPowerStatus(&mut s).ok()?; }
    Some(s)
}

/// BatteryFlag 128 = no battery.
pub fn on_battery_capable() -> bool {
    power_status().is_some_and(|s| s.BatteryFlag != 128 && s.BatteryFlag != 255)
}

fn power_plan() -> Option<Check> {
    let out = run("powercfg.exe", &["/getactivescheme"]).ok()?.output.to_ascii_lowercase();
    if out.contains(PLAN_GUID) || out.contains("performance") {
        return Some(c("Power plan", Level::Ok, "A performance power plan is active.", None));
    }
    Some(c("Power plan", Level::Warn, "A power-saving plan is active, which can hold back games.", Some(Fix::PowerPlan)))
}

fn battery() -> Option<Check> {
    let s = power_status()?;
    // ACLineStatus 0 = running on battery.
    (s.ACLineStatus == 0 && on_battery_capable()).then(|| c("Battery", Level::Warn, "Your laptop runs on battery. Plug it in for full speed.", None))
}

fn game_mode() -> Option<Check> {
    let on = match reg::get(Hive::Hkcu, "Software\\Microsoft\\GameBar", "AutoGameModeEnabled").ok()? {
        Some(RegValue::Dword(0)) => false,
        _ => true,
    };
    Some(if on {
        c("Game Mode", Level::Ok, "Windows Game Mode is on.", None)
    } else {
        c("Game Mode", Level::Tip, "Windows Game Mode is off. It helps games get CPU time.", Some(Fix::GameMode))
    })
}

fn mouse_accel() -> Option<Check> {
    match reg::get(Hive::Hkcu, "Control Panel\\Mouse", "MouseSpeed").ok()? {
        Some(RegValue::String(s)) if s != "0" => {
            Some(c("Mouse acceleration", Level::Tip, "\"Enhance pointer precision\" is on. Turn it off on the Mouse page for steadier aim.", None))
        }
        _ => None,
    }
}

fn memory() -> Option<Check> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { GlobalMemoryStatusEx(&mut m).ok()?; }
    Some(if m.dwMemoryLoad > 85 {
        c("Memory", Level::Warn, format!("{}% of RAM is in use. Close programs you do not need.", m.dwMemoryLoad), None)
    } else {
        c("Memory", Level::Ok, format!("{}% of RAM is in use.", m.dwMemoryLoad), None)
    })
}

fn system_drive() -> Option<Check> {
    use windows::core::w;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let (mut free, mut total) = (0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(w!("C:\\"), Some(&mut free), Some(&mut total), None).ok()?; }
    let gb = free as f64 / 1e9;
    Some(if gb < 20.0 || (free as f64) < total as f64 * 0.10 {
        c("System drive", Level::Warn, format!("Only {gb:.0} GB free on C:. Clean up junk files on the Tools page."), None)
    } else {
        c("System drive", Level::Ok, format!("{gb:.0} GB free on C:."), None)
    })
}

/// MiB value of the first `Total` line after `section` in `nvidia-smi -q -d MEMORY`.
fn smi_total(text: &str, section: &str) -> Option<u64> {
    let rest = &text[text.find(section)?..];
    let line = rest.lines().skip(1).find(|l| l.trim_start().starts_with("Total"))?;
    line.split(':').nth(1)?.split_whitespace().next()?.parse().ok()
}

fn resizable_bar() -> Option<Check> {
    let mem = run("nvidia-smi", &["-q", "-d", "MEMORY"]).ok().filter(|r| r.exit_code == 0)?.output;
    let fb = smi_total(&mem, "FB Memory Usage")?;
    let bar = smi_total(&mem, "BAR1 Memory Usage")?;
    if bar * 10 >= fb * 9 {
        return Some(c("Resizable BAR", Level::Ok, "Resizable BAR is on.", None));
    }
    // Only RTX 30 series and newer support it.
    let name = run("nvidia-smi", &["--query-gpu=name", "--format=csv,noheader"]).ok()?.output;
    let model: u32 = name.split("RTX ").nth(1)?.chars().take_while(char::is_ascii_digit).collect::<String>().parse().ok()?;
    (model >= 3000).then(|| {
        c("Resizable BAR", Level::Tip, "Resizable BAR is off. Turn on \"Above 4G Decoding\" and \"Resizable BAR\" in your BIOS.", None)
    })
}

/// (min, max, avg) ms from the summary line of `ping`, and number of replies. Locale independent.
pub fn parse_ping(out: &str) -> (usize, Option<(f64, f64, f64)>) {
    let replies = out.lines().filter(|l| l.contains("TTL=") || l.contains("TTL =")).count();
    let nums: Vec<f64> = out
        .lines()
        .filter(|l| l.contains('=')).last()
        .map(|l| l.split('=').skip(1).filter_map(|p| p.trim().split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()).collect())
        .unwrap_or_default();
    (replies, (nums.len() == 3).then(|| (nums[0], nums[1], nums[2])))
}

fn internet() -> Option<Check> {
    let out = run("ping.exe", &["-n", "8", "1.1.1.1"]).ok()?.output;
    let (replies, stats) = parse_ping(&out);
    let t = "Internet";
    Some(match stats {
        _ if replies == 0 => c(t, Level::Warn, "No answer from the internet. Check your connection.", None),
        Some((min, max, avg)) if replies < 8 || avg > 60.0 || max - min > 30.0 => c(
            t,
            Level::Warn,
            format!("Connection is unsteady: {} of 8 answers, {avg:.0} ms average. Try the Internet page.", replies),
            None,
        ),
        Some((_, _, avg)) => c(t, Level::Ok, format!("{avg:.0} ms to the internet, no lost packets."), None),
        None => c(t, Level::Ok, "The internet answers.", None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ping_parse_english_and_german() {
        let en = "Reply from 1.1.1.1: bytes=32 time=9ms TTL=58\nReply from 1.1.1.1: bytes=32 time=9ms TTL=58\n\nApproximate round trip times in milli-seconds:\n    Minimum = 8ms, Maximum = 12ms, Average = 9ms\n";
        assert_eq!(parse_ping(en), (2, Some((8.0, 12.0, 9.0))));
        let de = "Antwort von 1.1.1.1: Bytes=32 Zeit=9ms TTL=58\n    Minimum = 8ms, Maximum = 12ms, Mittelwert = 9ms\n";
        assert_eq!(parse_ping(de), (1, Some((8.0, 12.0, 9.0))));
    }

    #[test]
    fn smi_parse() {
        let t = "    FB Memory Usage\n        Total                             : 12288 MiB\n        Used : 1 MiB\n    BAR1 Memory Usage\n        Total                             : 16384 MiB\n";
        assert_eq!(smi_total(t, "FB Memory Usage"), Some(12288));
        assert_eq!(smi_total(t, "BAR1 Memory Usage"), Some(16384));
    }

    #[test]
    fn summary_text() {
        assert_eq!(summary(&[]), "Everything looks fine.");
    }
}

// ---------------------------------------------------------------- live numbers for Home

/// (used GB, total GB, free GB, load %)
pub fn memory_now() -> Option<(f64, f64, f64, u32)> {
    use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
    unsafe { GlobalMemoryStatusEx(&mut m).ok()?; }
    let gb = |b: u64| b as f64 / 1_073_741_824.0;
    Some((gb(m.ullTotalPhys - m.ullAvailPhys), gb(m.ullTotalPhys), gb(m.ullAvailPhys), m.dwMemoryLoad))
}

/// (idle, total) CPU ticks since boot; load = 1 - didle/dtotal between two samples.
pub fn cpu_ticks() -> Option<(u64, u64)> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::GetSystemTimes;
    let (mut i, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetSystemTimes(Some(&mut i), Some(&mut k), Some(&mut u)).ok()?; }
    let q = |f: FILETIME| ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64;
    // kernel time already includes idle time
    Some((q(i), q(k) + q(u)))
}

#[derive(Debug, Clone, Default)]
pub struct ThisPc {
    pub cpu: String,
    pub cores: String,
    pub gpu: String,
    pub memory: String,
    pub windows: String,
    pub power_plan: String,
}

fn reg_str(hive: Hive, path: &str, name: &str) -> Option<String> {
    match reg::get(hive, path, name).ok()? {
        Some(RegValue::String(s)) | Some(RegValue::ExpandString(s)) => Some(s),
        _ => None,
    }
}

/// Slow (spawns powercfg and nvidia-smi): call from a worker thread.
pub fn this_pc() -> ThisPc {
    let cpu = reg_str(Hive::Hklm, r"HARDWARE\DESCRIPTION\System\CentralProcessor\0", "ProcessorNameString").unwrap_or_default().trim().to_string();
    let cores = std::thread::available_parallelism().map(|n| n.get().to_string()).unwrap_or_default();
    let gpu = run("nvidia-smi", &["--query-gpu=name", "--format=csv,noheader"])
        .ok()
        .filter(|r| r.exit_code == 0)
        .map(|r| r.output.lines().next().unwrap_or("").trim().to_string())
        .unwrap_or_default();
    let build = reg_str(Hive::Hklm, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", "CurrentBuildNumber").unwrap_or_default();
    let plan = run("powercfg.exe", &["/getactivescheme"])
        .ok()
        .and_then(|r| {
            let o = r.output;
            let a = o.rfind('(')?;
            let b = o.rfind(')')?;
            (b > a).then(|| o[a + 1..b].to_string())
        })
        .unwrap_or_default();
    ThisPc {
        cpu,
        cores,
        gpu,
        memory: memory_now().map(|m| format!("{:.0} GB", m.1)).unwrap_or_default(),
        windows: format!("10.0 build {build}"),
        power_plan: plan,
    }
}
