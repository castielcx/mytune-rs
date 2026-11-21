//! Tools page, drives and system files (C# `ToolsVM.LoadDisksAsync`, `RunSfc`, `RunDism`).

use crate::error::Result;
use crate::special::powershell;
use crate::tweaks::Cmd;
use serde::Deserialize;
use std::os::windows::process::CommandExt;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Disk {
    pub name: String,
    /// "932 GB · SSD · NVMe"
    pub detail: String,
    /// Healthy, Warning, or what Windows reports.
    pub health: String,
}

#[derive(Deserialize)]
struct Row {
    #[serde(rename = "N")]
    name: Option<String>,
    #[serde(rename = "M")]
    media: Option<String>,
    #[serde(rename = "B")]
    bus: Option<String>,
    #[serde(rename = "S")]
    gb: Option<f64>,
    #[serde(rename = "H")]
    health: Option<String>,
}

pub fn parse_disks(json: &str) -> Result<Vec<Disk>> {
    let t = json.trim();
    let rows: Vec<Row> = match serde_json::from_str::<serde_json::Value>(if t.is_empty() { "[]" } else { t })? {
        v @ serde_json::Value::Array(_) => serde_json::from_value(v)?,
        v @ serde_json::Value::Object(_) => vec![serde_json::from_value(v)?],
        _ => vec![],
    };
    Ok(rows
        .into_iter()
        .map(|r| Disk {
            name: r.name.unwrap_or_default(),
            detail: format!("{:.0} GB · {} · {}", r.gb.unwrap_or(0.0), r.media.unwrap_or_default(), r.bus.unwrap_or_default()),
            health: r.health.unwrap_or_default(),
        })
        .collect())
}

pub fn disks(cmd: &dyn Cmd) -> Result<Vec<Disk>> {
    parse_disks(&powershell(
        cmd,
        "ConvertTo-Json -Compress -InputObject @(Get-PhysicalDisk | ForEach-Object { [pscustomobject]@{N=$_.FriendlyName;M=\"$($_.MediaType)\";B=\"$($_.BusType)\";S=[math]::Round($_.Size/1GB);H=\"$($_.HealthStatus)\"} })",
    )?)
}

/// Opens `cmd.exe /k <line>` in its own console window (can run 10 to 30 minutes).
fn open_console(line: &str) -> std::io::Result<()> {
    const CREATE_NEW_CONSOLE: u32 = 0x10;
    Command::new("cmd.exe").args(["/k", line]).creation_flags(CREATE_NEW_CONSOLE).spawn().map(|_| ())
}

pub fn run_sfc() -> std::io::Result<()> {
    open_console("sfc /scannow")
}

/// Restarts Explorer in this session (`process::restart_explorer`), then checks that a new shell is up, because
/// that call reports nothing. Blocks up to ~13 s: call it off the UI thread.
pub fn restart_explorer() -> std::io::Result<()> {
    if crate::process::shell_user_differs() {
        return Err(std::io::Error::other("it runs under another Windows account than MyTune, so it was left alone"));
    }
    let before = shell_pid();
    crate::process::restart_explorer();
    // the process can be back before its shell window
    let mut result = restart_result(before, shell_pid());
    for _ in 0..10 {
        if result.is_ok() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
        result = restart_result(before, shell_pid());
    }
    result.map_err(std::io::Error::other)
}

/// 0 = no shell window.
fn restart_result(before: u32, now: u32) -> std::result::Result<(), &'static str> {
    match now {
        0 => Err("Explorer did not start; press Ctrl+Shift+Esc > Run new task > explorer.exe"),
        n if n == before => Err("it was not closed, see the log"),
        _ => Ok(()),
    }
}

fn shell_pid() -> u32 {
    use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(GetShellWindow(), Some(&mut pid)) };
    pid
}

pub fn run_dism() -> std::io::Result<()> {
    open_console("DISM /Online /Cleanup-Image /RestoreHealth")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_rows_parse_array_object_and_empty() {
        let d = parse_disks(r#"[{"N":"Samsung SSD","M":"SSD","B":"NVMe","S":932,"H":"Healthy"},{"N":"Old HDD","M":"HDD","B":"SATA","S":1863,"H":"Warning"}]"#).unwrap();
        assert_eq!(d[0], Disk { name: "Samsung SSD".into(), detail: "932 GB · SSD · NVMe".into(), health: "Healthy".into() });
        assert_eq!(d[1].health, "Warning");
        assert_eq!(parse_disks(r#"{"N":"One","M":"SSD","B":"USB","S":64,"H":"Healthy"}"#).unwrap().len(), 1);
        assert!(parse_disks("").unwrap().is_empty());
    }

    #[test]
    fn restart_counts_only_a_new_shell() {
        assert!(restart_result(10, 20).is_ok());
        assert!(restart_result(0, 20).is_ok()); // Explorer was down and came back
        assert_eq!(restart_result(10, 10), Err("it was not closed, see the log"));
        assert!(restart_result(10, 0).unwrap_err().starts_with("Explorer did not start"));
    }

    /// Read only.
    #[test]
    fn real_disks_listed() {
        let d = disks(&*crate::tweaks::system_cmd()).unwrap();
        eprintln!("{d:?}");
        assert!(!d.is_empty());
    }
}
