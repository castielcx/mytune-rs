//! Settings > Help, system report (C# `SystemReport`) and Settings > Backup, Windows restore point.
//! The report is plain text for asking for help: hardware, Windows build, main drivers, which tweaks are on.
//! No username, serial numbers, IP addresses or file paths (the support file adds network adapters and the
//! log tail, where program paths can still appear).
//!
//! Difference from C#: the tweak states come from the caller (`states`), because only the registry and
//! network tweaks have Rust objects so far; the rest print as `?`.

use crate::error::Result;
use crate::layout::{self, Section};
use crate::process::run;
use crate::reg::{self, Hive, RegValue};
use crate::special::{active_adapters, powershell, ps_quote};
use crate::stability;
use crate::store::Store;
use crate::tweaks::Cmd;
use crate::updater::APP_VERSION;
use serde_json::Value;
use std::path::Path;

fn reg_text(name: &str) -> Option<String> {
    match reg::get(Hive::Hklm, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion", name).ok()?? {
        RegValue::String(s) | RegValue::ExpandString(s) => Some(s),
        RegValue::Dword(d) => Some(d.to_string()),
        _ => None,
    }
}

pub fn windows_name() -> String {
    let build = reg_text("CurrentBuild").unwrap_or_default();
    let product = reg_text("ProductName").unwrap_or_else(|| "Windows".into());
    let edition = product.replace("Windows 10", if build.parse::<u32>().is_ok_and(|b| b >= 22000) { "Windows 11" } else { "Windows 10" });
    let ubr = reg_text("UBR").map(|u| format!(".{u}")).unwrap_or_default();
    format!("{edition} {} (build {build}{ubr})", reg_text("DisplayVersion").unwrap_or_default()).replace("  ", " ")
}

// ---------------------------------------------------------------- drivers

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Driver {
    pub device: String,
    pub kind: &'static str,
    pub maker: String,
    pub version: String,
    pub date: String,
}

/// Keeps the drivers that matter for games (C# `DriverInfo.Read`): no integrated GPU next to a dedicated one,
/// no HDMI / GPU audio, only real chipset drivers; ordered Graphics, Chipset, Network, Audio.
pub fn filter_drivers(rows: Vec<(String, String, String, String, String)>) -> Vec<Driver> {
    let drivers: Vec<Driver> = rows
        .into_iter()
        .map(|(device, class, maker, version, date)| Driver {
            kind: match class.as_str() {
                "DISPLAY" => "Graphics",
                "NET" => "Network",
                "MEDIA" => "Audio",
                _ => "Chipset",
            },
            device,
            maker,
            version,
            date,
        })
        .collect();
    let intel = |s: &str| s.to_lowercase().contains("intel");
    let dedicated = drivers.iter().any(|d| d.kind == "Graphics" && !intel(&d.maker));
    let noisy_audio = ["hdmi", "display audio", "nvidia", "smart sound", "radeon"];
    let mut kept: Vec<Driver> = drivers
        .into_iter()
        .filter(|d| match d.kind {
            "Graphics" => !(dedicated && intel(&d.maker)),
            "Chipset" => d.device.to_lowercase().contains("chipset"),
            "Audio" => {
                let l = d.device.to_lowercase();
                !(noisy_audio.iter().any(|w| l.contains(w)) || (l.contains("high definition audio") && (l.contains("nvidia") || l.contains("amd"))))
            }
            _ => true,
        })
        .collect();
    let order = ["Graphics", "Chipset", "Network", "Audio"];
    kept.sort_by(|a, b| (order.iter().position(|o| *o == a.kind), &a.device).cmp(&(order.iter().position(|o| *o == b.kind), &b.device)));
    kept.dedup_by(|a, b| a.device == b.device && a.version == b.version);
    kept
}

pub fn drivers(cmd: &dyn Cmd) -> Result<Vec<Driver>> {
    let out = powershell(
        cmd,
        "ConvertTo-Json -Compress -InputObject @(Get-CimInstance Win32_PnPSignedDriver | Where-Object { $_.DeviceName -and (\
         ($_.DeviceClass -in 'DISPLAY','NET','MEDIA' -and $_.DeviceID -like 'PCI*') -or \
         ($_.DeviceClass -eq 'SYSTEM' -and $_.Manufacturer -match 'Intel|Advanced Micro|AMD' -and $_.DeviceName -match 'Chipset|Management Engine|SMBus|Serial IO|GPIO|Platform') ) } | \
         ForEach-Object { [pscustomobject]@{ N=$_.DeviceName; C=$_.DeviceClass; M=$_.Manufacturer; V=$_.DriverVersion; D=if($_.DriverDate){$_.DriverDate.ToString('yyyy-MM-dd')}else{''} } })",
    )?;
    Ok(filter_drivers(rows(&out).iter().map(|r| (s(r, "N"), s(r, "C"), s(r, "M"), s(r, "V"), s(r, "D"))).collect()))
}

fn rows(json: &str) -> Vec<Value> {
    match serde_json::from_str::<Value>(if json.trim().is_empty() { "[]" } else { json.trim() }) {
        Ok(Value::Array(a)) => a,
        Ok(v @ Value::Object(_)) => vec![v],
        _ => vec![],
    }
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or("").to_string()
}

fn strings(v: &Value, k: &str) -> Vec<String> {
    match v.get(k) {
        Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect(),
        Some(Value::String(x)) if !x.trim().is_empty() => vec![x.trim().to_string()],
        _ => vec![],
    }
}

fn plan_name() -> Option<String> {
    let o = run("powercfg.exe", &["/getactivescheme"]).ok()?.output;
    let (a, b) = (o.rfind('(')?, o.rfind(')')?);
    (b > a).then(|| o[a + 1..b].to_string())
}

// ---------------------------------------------------------------- the report

/// `states(id)` is `"ON "`, `"off"`, `"n/a"` or `"?  "`.
pub fn build(cmd: &dyn Cmd, states: &dyn Fn(&str) -> String) -> String {
    let mut out = String::new();
    let mut line = |t: &str| {
        out.push_str(t);
        out.push_str("\r\n");
    };
    let hw = powershell(
        cmd,
        "ConvertTo-Json -Compress -InputObject ([pscustomobject]@{ \
         cpu = @(Get-CimInstance Win32_Processor | ForEach-Object { \"$($_.Name.Trim()) ($($_.NumberOfCores) cores, $($_.NumberOfLogicalProcessors) threads)\" }); \
         gpus = @(Get-CimInstance Win32_VideoController | Where-Object { $_.Name -notmatch 'Basic|Remote|Virtual|Parsec|Mirage' } | ForEach-Object { $_.Name }); \
         board = @(Get-CimInstance Win32_BaseBoard | ForEach-Object { \"$($_.Manufacturer) $($_.Product)\" }); \
         ram = @(Get-CimInstance Win32_PhysicalMemory | ForEach-Object { \"$([math]::Round($_.Capacity/1GB)) GB at $($_.ConfiguredClockSpeed) MT/s\" }); \
         disks = @(Get-PhysicalDisk | ForEach-Object { \"$($_.FriendlyName) ($($_.MediaType), $([math]::Round($_.Size/1GB)) GB)\" }) })",
    )
    .ok()
    .and_then(|o| serde_json::from_str::<Value>(o.trim()).ok());
    let drv = drivers(cmd).unwrap_or_default();
    let plan = plan_name();
    let now = stability::iso(stability::now()).replace('T', " ");

    line(&format!("MyTune system report - {}", &now[..16]));
    line(&format!("MyTune {APP_VERSION}"));
    line("No username, serial numbers, IP addresses or file paths are included.");
    line("");
    line("== System ==");
    line(&format!("Windows: {}", windows_name()));
    match &hw {
        Some(h) if h.is_object() => {
            strings(h, "cpu").iter().for_each(|x| line(&format!("CPU: {x}")));
            strings(h, "gpus").iter().for_each(|x| line(&format!("Graphics: {x}")));
            strings(h, "board").iter().for_each(|x| line(&format!("Motherboard: {x}")));
            let ram = strings(h, "ram");
            if !ram.is_empty() {
                line(&format!("Memory: {} module(s): {}", ram.len(), ram.join(", ")));
            }
            strings(h, "disks").iter().for_each(|x| line(&format!("Drive: {x}")));
        }
        _ => line("(Hardware details could not be read.)"),
    }
    if let Some(p) = plan.filter(|p| !p.is_empty()) {
        line(&format!("Power plan: {p}"));
    }

    line("");
    line("== Main drivers ==");
    if drv.is_empty() {
        line("(none found)");
    }
    for d in &drv {
        let date = if d.date.len() >= 4 && d.date[..4].parse::<u32>().is_ok_and(|y| y >= 2015) { format!(" ({})", d.date) } else { String::new() };
        line(&format!("{}: {} - {}{date}", d.kind, d.device, d.version));
    }

    let titles: std::collections::HashMap<String, String> = crate::catalog::entries().unwrap_or_default().into_iter().map(|e| (e.id, e.title)).collect();
    let all: Vec<&str> = [Section::General, Section::Advanced, Section::Network, Section::Debloat, Section::Customize].iter().flat_map(|sec| layout::ids(*sec)).map(|(_, id)| id).collect();
    let st: Vec<String> = all.iter().map(|id| states(id)).collect();
    line("");
    line(&format!("Tweaks on: {} of {} available", st.iter().filter(|v| *v == "ON ").count(), st.iter().filter(|v| *v != "n/a").count()));
    for (title, sec) in [("Optimize > General", Section::General), ("Optimize > Advanced", Section::Advanced), ("Optimize > Network", Section::Network), ("Debloat", Section::Debloat), ("Customize", Section::Customize)] {
        line("");
        line(&format!("== {title} =="));
        let mut group = "";
        for (g, id) in layout::ids(sec) {
            if !g.is_empty() && g != group {
                group = g;
                line(&format!("[{g}]"));
            }
            line(&format!("  {}  {}", states(id), titles.get(id).map(String::as_str).unwrap_or(id)));
        }
    }
    out
}

/// The report plus network adapters, version history and the last 150 log lines. The user name is replaced.
pub fn build_support(cmd: &dyn Cmd, store: &Store, states: &dyn Fn(&str) -> String) -> String {
    let mut out = build(cmd, states);
    let mut line = |t: &str| {
        out.push_str(t);
        out.push_str("\r\n");
    };
    let top = |k: &str| store.top(k).and_then(Value::as_str).unwrap_or("").to_string();
    line("");
    line("== Support details ==");
    line("The log below can contain program paths; the Windows user name is replaced.");
    let prev = top("PreviousVersion");
    line(&format!("MyTune: {} (previous: {})", top("LastVersion"), if prev.is_empty() { "none" } else { &prev }));
    line(&format!("Windows build recorded by MyTune: {}", top("LastWinBuild")));
    if let Some(Value::Object(l)) = store.top("Locked") {
        if !l.is_empty() {
            line(&format!("Locked after a Windows update: {}", l.keys().cloned().collect::<Vec<_>>().join(", ")));
        }
    }
    match powershell(cmd, "ConvertTo-Json -Compress -InputObject @(Get-NetAdapter | ForEach-Object { [pscustomobject]@{N=$_.Name;D=$_.InterfaceDescription;T=\"$($_.MediaType)\";S=\"$($_.Status)\"} })") {
        Ok(o) => {
            line("Network adapters (name | description | type | status):");
            for r in rows(&o) {
                line(&format!("  {} | {} | {} | {}", s(&r, "N"), s(&r, "D"), s(&r, "T"), s(&r, "S")));
            }
        }
        Err(e) => line(&format!("(adapters could not be read: {e})")),
    }
    if let Ok(a) = active_adapters(cmd) {
        line(&format!("Counted by MyTune: {}", a.join(", ")));
    }
    line("");
    line("== Last 150 log lines ==");
    match log_tail(&store.dir().join("mytune.log"), 150) {
        Ok(lines) => lines.iter().for_each(|l| line(&replace_user(l))),
        Err(e) => line(&format!("(log could not be read: {e})")),
    }
    out
}

fn log_tail(path: &Path, n: usize) -> std::io::Result<Vec<String>> {
    let text = String::from_utf8_lossy(&std::fs::read(path)?).into_owned();
    let lines: Vec<String> = text.lines().map(String::from).collect();
    Ok(lines[lines.len().saturating_sub(n)..].to_vec())
}

/// Case-insensitive replace of the Windows user name with `<user>`.
pub fn replace_user(line: &str) -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    replace_ci(line, &user, "<user>")
}

fn replace_ci(text: &str, needle: &str, with: &str) -> String {
    if needle.is_empty() {
        return text.to_string();
    }
    let (lt, ln) = (text.to_lowercase(), needle.to_lowercase());
    if lt.len() != text.len() {
        return text.replace(needle, with); // lower-casing changed the byte length: plain replace is the safe fallback
    }
    let mut out = String::new();
    let mut i = 0;
    while let Some(p) = lt[i..].find(&ln) {
        out.push_str(&text[i..i + p]);
        out.push_str(with);
        i += p + ln.len();
    }
    out.push_str(&text[i..]);
    out
}

// ---------------------------------------------------------------- restore point

/// `None` on success, otherwise the reason.
pub fn restore_result(code: i64) -> Option<String> {
    match code {
        0 => None,
        1058 => Some("System Protection is turned off for drive C:".into()),
        c => Some(format!("error {c}")),
    }
}

pub fn create_restore_point(cmd: &dyn Cmd, description: &str) -> Option<String> {
    let script = format!(
        "$r=Invoke-CimMethod -Namespace root/default -ClassName SystemRestore -MethodName CreateRestorePoint -Arguments @{{Description={};RestorePointType=[uint32]12;EventType=[uint32]100}}; [int]$r.ReturnValue",
        ps_quote(description)
    );
    match powershell(cmd, &script) {
        Ok(o) => {
            let code = o.trim().parse::<i64>().unwrap_or(-1);
            crate::log::info(&format!("CreateRestorePoint -> {code}"));
            restore_result(code)
        }
        Err(e) => {
            crate::log::error("CreateRestorePoint", Some(&e));
            Some(e.to_string())
        }
    }
}

/// The automatic restore point, at most once per app run. Blocks up to 2 minutes, so call it on a worker thread
/// before the first apply, never inside `Engine::apply`. A failure (System Protection off) is logged and the
/// tweaks go on: each has its own backup.
pub fn restore_point_once(cmd: &dyn Cmd) {
    RESTORE_POINT.call_once(|| {
        if let Some(e) = create_restore_point(cmd, "MyTune automatic restore point") {
            crate::log::error(&format!("no restore point before the first change: {e}"), None);
        }
    });
}

static RESTORE_POINT: std::sync::Once = std::sync::Once::new();

/// Whether this run has made (or tried) its automatic restore point.
pub fn restore_point_done() -> bool {
    RESTORE_POINT.is_completed()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(n: &str, c: &str, m: &str) -> (String, String, String, String, String) {
        (n.into(), c.into(), m.into(), "1.0".into(), "2024-01-01".into())
    }

    #[test]
    fn driver_filter_like_csharp() {
        let d = filter_drivers(vec![
            row("Intel UHD Graphics", "DISPLAY", "Intel"),
            row("NVIDIA GeForce RTX 4070", "DISPLAY", "NVIDIA"),
            row("Realtek PCIe GbE", "NET", "Realtek"),
            row("NVIDIA High Definition Audio", "MEDIA", "NVIDIA"),
            row("Realtek Audio", "MEDIA", "Realtek"),
            row("AMD SMBus", "SYSTEM", "AMD"),
            row("AMD Chipset Software", "SYSTEM", "AMD"),
        ]);
        let names: Vec<&str> = d.iter().map(|x| x.device.as_str()).collect();
        assert_eq!(names, ["NVIDIA GeForce RTX 4070", "AMD Chipset Software", "Realtek PCIe GbE", "Realtek Audio"]);
        assert_eq!(filter_drivers(vec![row("Intel UHD Graphics", "DISPLAY", "Intel")]).len(), 1, "iGPU stays when it is the only one");
    }

    #[test]
    fn user_name_is_replaced_case_insensitively() {
        assert_eq!(replace_ci(r"C:\Users\Alice\x and alice", "alice", "<user>"), r"C:\Users\<user>\x and <user>");
        assert_eq!(replace_ci("nothing", "", "<user>"), "nothing");
    }

    #[test]
    fn restore_codes() {
        assert_eq!(restore_result(0), None);
        assert_eq!(restore_result(1058).unwrap(), "System Protection is turned off for drive C:");
        assert_eq!(restore_result(5).unwrap(), "error 5");
    }

    /// Read only (no restore point is created): the report builds and has its sections.
    #[test]
    fn real_report_builds() {
        let r = build(&*crate::tweaks::system_cmd(), &|_| "?  ".to_string());
        eprintln!("{}", &r[..r.len().min(1500)]);
        for h in ["== System ==", "== Main drivers ==", "Tweaks on:", "== Optimize > General ==", "== Customize =="] {
            assert!(r.contains(h), "missing {h}");
        }
        assert!(r.contains("[Games]") && !r.to_lowercase().contains(&std::env::var("USERNAME").unwrap().to_lowercase()), "no user name in the report");
    }
}
