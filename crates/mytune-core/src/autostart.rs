//! Settings > Start with Windows (C# `Autostart`): a Task Scheduler logon task with the highest run level
//! (the app needs admin, so a plain Run key would show a UAC prompt on every logon) plus an HKCU Run entry
//! that starts the task, so it also shows in Task Manager > Startup apps and can be disabled there.
//!
//! The task is called "MyTune", the same as in the C# app: enabling it here points that task at THIS exe.

use crate::log;
use crate::process::{run_with, system32};
use crate::reg::{self, Hive, RegValue};
use std::time::Duration;

const TASK: &str = "MyTune";
const HELPER: &str = "MyTuneStart.exe";
pub const MINIMIZED_ARG: &str = "--minimized";
const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\Run";

fn schtasks(args: &[&str]) -> i32 {
    run_with(&system32("schtasks.exe").to_string_lossy(), args, Duration::from_secs(15), None).map(|r| r.exit_code).unwrap_or(-1)
}

pub fn is_enabled() -> bool {
    matches!(reg::get(Hive::Hkcu, RUN, TASK), Ok(Some(_))) && schtasks(&["/Query", "/TN", TASK]) == 0
}

fn set_run_key(on: bool) -> crate::Result<()> {
    if on {
        // The helper has no console and is named MyTune in Startup apps; a missing helper must not break autostart.
        let cmd = match std::env::current_exe().map(|e| e.with_file_name(HELPER)) {
            Ok(h) if h.is_file() => format!("\"{}\"", h.display()),
            _ => format!("\"{}\" /run /tn {TASK}", system32("schtasks.exe").display()),
        };
        reg::set(Hive::Hkcu, RUN, TASK, &RegValue::String(cmd))?;
        // 02 00 00 00 ... = enabled (clears a previous "disabled in Task Manager" state)
        reg::set(Hive::Hkcu, APPROVED, TASK, &RegValue::Binary(vec![2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]))
    } else {
        reg::delete_value(Hive::Hkcu, RUN, TASK)?;
        reg::delete_value(Hive::Hkcu, APPROVED, TASK)
    }
}

/// The Run value older versions wrote: schtasks directly, which flashes a console at sign-in.
fn is_old_run_value(v: &str) -> bool {
    let v = v.replace('"', "").trim().to_ascii_lowercase();
    v.contains("schtasks") && v.ends_with("/run /tn mytune")
}

/// Moves an old Run value to the helper. Leaves StartupApproved alone, so autostart disabled in Task Manager stays disabled.
pub fn migrate_run_key() {
    let Ok(exe) = std::env::current_exe() else { return };
    let helper = exe.with_file_name(HELPER);
    let Ok(Some(RegValue::String(v))) = reg::get(Hive::Hkcu, RUN, TASK) else { return };
    if !is_old_run_value(&v) || !helper.is_file() {
        return;
    }
    // Only the copy the task starts: a test or preview build must not point the Run value at its own folder.
    let schtasks_exe = system32("schtasks.exe");
    let query = run_with(&schtasks_exe.to_string_lossy(), &["/Query", "/TN", TASK, "/XML"], Duration::from_secs(15), None);
    let wanted = xml_escape(&exe.to_string_lossy()).to_lowercase();
    if query.is_ok_and(|r| r.exit_code == 0 && r.output.to_lowercase().contains(&wanted)) {
        let ok = reg::set(Hive::Hkcu, RUN, TASK, &RegValue::String(format!("\"{}\"", helper.display()))).is_ok();
        log::info(&format!("Autostart Run value moved to the helper: {ok}"));
    }
}

pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&apos;")
}

/// The task definition (UTF-16 when written to disk).
pub fn task_xml(user: &str, exe: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Principals><Principal id="Author"><UserId>{}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <StartWhenAvailable>true</StartWhenAvailable>
  </Settings>
  <Actions Context="Author"><Exec><Command>{}</Command><Arguments>{MINIMIZED_ARG}</Arguments></Exec></Actions>
</Task>"#,
        xml_escape(user),
        xml_escape(exe)
    )
}

fn current_user() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(d) if !d.is_empty() => format!("{d}\\{user}"),
        _ => user,
    }
}

pub fn enable() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let xml = task_xml(&current_user(), &exe.to_string_lossy());
    // Beside the exe, not in TEMP: a non-admin can redirect TEMP or swap the file, and this XML becomes an admin task.
    let file = exe.with_file_name("mytune-task.xml");
    let utf16: Vec<u8> = std::iter::once(0xFEFFu16).chain(xml.encode_utf16()).flat_map(|c| c.to_le_bytes()).collect();
    let ok = std::fs::write(&file, utf16).is_ok() && schtasks(&["/Create", "/TN", TASK, "/XML", &file.to_string_lossy(), "/F"]) == 0 && set_run_key(true).is_ok();
    let _ = std::fs::remove_file(&file);
    log::info(&format!("Autostart enabled: {ok}"));
    ok
}

pub fn disable() -> bool {
    let key_ok = set_run_key(false).is_ok();
    let ok = key_ok && (schtasks(&["/Query", "/TN", TASK]) != 0 || schtasks(&["/Delete", "/TN", TASK, "/F"]) == 0);
    log::info(&format!("Autostart disabled: {ok}"));
    ok
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_xml_escapes_and_launches_minimized() {
        let x = task_xml("PC\\Dom&<Co>", r"C:\Program Files\My'Tune\mytune.exe");
        assert!(x.contains("<UserId>PC\\Dom&amp;&lt;Co&gt;</UserId>"));
        assert!(x.contains("<Command>C:\\Program Files\\My&apos;Tune\\mytune.exe</Command><Arguments>--minimized</Arguments>"));
        assert!(x.contains("<RunLevel>HighestAvailable</RunLevel>"));
        assert!(x.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
    }

    #[test]
    fn old_run_value_detected() {
        assert!(is_old_run_value(r#""C:\WINDOWS\System32\schtasks.exe" /run /tn MyTune"#));
        assert!(is_old_run_value("schtasks /run /tn MyTune"));
        assert!(!is_old_run_value(r#""C:\Apps\MyTune\MyTuneStart.exe""#));
        assert!(!is_old_run_value(r#""C:\WINDOWS\System32\schtasks.exe" /run /tn Other"#));
    }

    /// Read only. Never calls enable/disable: that would repoint the real "MyTune" task.
    #[test]
    fn real_state_reads() {
        eprintln!("autostart enabled: {}", is_enabled());
    }
}
