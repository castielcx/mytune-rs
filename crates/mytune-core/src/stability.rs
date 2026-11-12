//! Tools page, stability log (C# `StabilityLog`): a short history of what MyTune changed (`changes.log`
//! next to `state.json`) lined up with Windows crash and reboot events. Relevance, not proof.
//!
//! `Engine` records every successful apply ("on") and revert ("off"). App removal ("app") is for the Debloat
//! apps list, which is not ported yet. Times are local wall-clock, no time zone maths.

use crate::error::Result;
use crate::log;
use crate::special::powershell;
use crate::tweaks::Cmd;
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

const DAYS: i64 = 30;
const DAY: i64 = 86_400;

// ---------------------------------------------------------------- local time without a date crate

/// Seconds since 1970-01-01 of a civil date (no zone).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// "2026-11-10T14:35:54" (or with a space) -> local seconds.
pub fn parse_time(t: &str) -> Option<i64> {
    let t = t.trim();
    let (date, time) = t.split_once(['T', ' '])?;
    let mut d = date.split('-').map(|p| p.parse::<i64>());
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let mut c = time.split(':').map(|p| p.split('.').next().unwrap_or("").parse::<i64>());
    let (h, mi, s) = (c.next()?.ok()?, c.next()?.ok()?, c.next().and_then(|x| x.ok()).unwrap_or(0));
    ((1..=12).contains(&m) && (1..=31).contains(&day)).then(|| days_from_civil(y, m, day) * DAY + h * 3600 + mi * 60 + s)
}

pub fn iso(secs: i64) -> String {
    let (y, m, d) = civil_from_days(secs.div_euclid(DAY));
    let r = secs.rem_euclid(DAY);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}", r / 3600, r % 3600 / 60, r % 60)
}

/// "5 Nov 14:35" like the C# `d MMM HH:mm`.
pub fn short(secs: i64) -> String {
    const MON: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (_, m, d) = civil_from_days(secs.div_euclid(DAY));
    let r = secs.rem_euclid(DAY);
    format!("{d} {} {:02}:{:02}", MON[(m - 1) as usize], r / 3600, r % 3600 / 60)
}

pub fn now() -> i64 {
    use windows::Win32::System::SystemInformation::GetLocalTime;
    let t = unsafe { GetLocalTime() };
    days_from_civil(t.wYear as i64, t.wMonth as i64, t.wDay as i64) * DAY + t.wHour as i64 * 3600 + t.wMinute as i64 * 60 + t.wSecond as i64
}

// ---------------------------------------------------------------- change history

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangeEntry {
    pub time: i64,
    pub text: String,
    /// "on", "off", "app"; `None` for old `time|text` lines.
    pub kind: Option<String>,
    /// Tweak id or app package name.
    pub id: Option<String>,
    pub label: Option<String>,
}

/// Appends `time|kind|id|label`. Never fails the caller.
pub fn record(dir: &Path, kind: &str, id: &str, label: &str) {
    let write = || -> std::io::Result<()> {
        fs::create_dir_all(dir)?;
        let path = dir.join("changes.log");
        let new = !path.exists();
        let mut f = OpenOptions::new().create(true).append(true).open(path)?;
        if new {
            f.write_all(&[0xEF, 0xBB, 0xBF])?;
        }
        writeln!(f, "{}|{kind}|{id}|{}", iso(now()), label.replace('|', "/"))
    };
    let _ = write();
}

/// Entries of the last 30 days, newest first.
pub fn changes(dir: &Path, now: i64) -> Vec<ChangeEntry> {
    let Ok(raw) = fs::read(dir.join("changes.log")) else { return vec![] };
    let text = String::from_utf8_lossy(raw.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&raw)).into_owned();
    let from = now - DAYS * DAY;
    let mut list: Vec<ChangeEntry> = Vec::new();
    for line in text.lines() {
        let p: Vec<&str> = line.splitn(4, '|').collect();
        let Some(time) = p.first().and_then(|t| parse_time(t)).filter(|t| p.len() >= 2 && *t >= from) else { continue };
        if p.len() < 4 {
            list.push(ChangeEntry { time, text: p[1].to_string(), kind: None, id: None, label: None }); // old line, no action possible
            continue;
        }
        let prefix = match p[1] {
            "on" => "Switched on: ",
            "off" => "Switched off: ",
            "app" => "Removed app: ",
            _ => "",
        };
        list.push(ChangeEntry { time, text: format!("{prefix}{}", p[3]), kind: Some(p[1].into()), id: Some(p[2].into()), label: Some(p[3].into()) });
    }
    list.sort_by_key(|c| std::cmp::Reverse(c.time));
    list
}

// ---------------------------------------------------------------- events

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StabilityEvent {
    pub time: i64,
    pub what: String,
    pub detail: String,
    pub related: Vec<ChangeEntry>,
}

const SCRIPT: &str = "
$since=(Get-Date).AddDays(-30)
$a=@(Get-WinEvent -FilterHashtable @{LogName='Application';ProviderName='Application Error';Id=1000;StartTime=$since} -ErrorAction SilentlyContinue)
$s=@(Get-WinEvent -FilterHashtable @{LogName='System';Id=41,1001,4101;StartTime=$since} -ErrorAction SilentlyContinue |
  Where-Object { $_.Id -ne 1001 -or $_.ProviderName -eq 'Microsoft-Windows-WER-SystemErrorReporting' })
ConvertTo-Json -Compress -InputObject @(
  $a | ForEach-Object { @{ T=$_.TimeCreated.ToString('s'); Id=1000; App=[string]$_.Properties[0].Value; Mod=[string]$_.Properties[3].Value; Code=[string]$_.Properties[6].Value; Path=[string]$_.Properties[10].Value } }
  $s | ForEach-Object { @{ T=$_.TimeCreated.ToString('s'); Id=[int]$_.Id } })";

/// App crashes (1000), unexpected reboots (Kernel-Power 41), blue screens (1001) and graphics driver resets
/// (4101). Without `all`, app crashes are limited to Windows components and Store apps.
pub fn events(cmd: &dyn Cmd, dir: &Path, all: bool) -> Result<Vec<StabilityEvent>> {
    let out = powershell(cmd, SCRIPT)?;
    let windows = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    Ok(parse_events(&out, &changes(dir, now()), all, &windows))
}

pub fn parse_events(json: &str, changes: &[ChangeEntry], all: bool, windows_dir: &str) -> Vec<StabilityEvent> {
    let v: Value = serde_json::from_str(if json.trim().is_empty() { "[]" } else { json.trim() }).unwrap_or(Value::Null);
    let rows: Vec<&Value> = match &v {
        Value::Array(a) => a.iter().collect(),
        Value::Object(_) => vec![&v],
        _ => vec![],
    };
    let text = |r: &Value, k: &str| r.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let mut result = Vec::new();
    for r in rows {
        let Some(time) = r.get("T").and_then(Value::as_str).and_then(parse_time) else { continue };
        let (what, detail) = match r.get("Id").and_then(Value::as_i64).unwrap_or(0) {
            41 => ("PC restarted unexpectedly".to_string(), String::new()),
            1001 => ("Blue screen".to_string(), String::new()),
            4101 => ("Graphics driver reset".to_string(), String::new()),
            _ => {
                let path = text(r, "Path");
                if !all && !path.to_lowercase().starts_with(&windows_dir.to_lowercase()) && !path.to_lowercase().contains(r"\windowsapps\") {
                    continue;
                }
                (format!("{} crashed", text(r, "App")), format!("{}, {}", text(r, "Mod"), text(r, "Code")))
            }
        };
        let related = changes.iter().filter(|c| c.time <= time && time - c.time <= 7 * DAY).take(3).cloned().collect();
        result.push(StabilityEvent { time, what, detail, related });
    }
    result.sort_by_key(|e| std::cmp::Reverse(e.time));
    result
}

/// "12 min", "3 h", "2 d".
pub fn ago(secs: i64) -> String {
    if secs < 3600 {
        format!("{} min", secs / 60)
    } else if secs < DAY {
        format!("{} h", secs / 3600)
    } else {
        format!("{} d", secs / DAY)
    }
}

/// Title and detail line of one event, like the C# row.
pub fn describe(e: &StabilityEvent) -> (String, String) {
    let rel = if e.related.is_empty() {
        "No MyTune change in the 7 days before.".to_string()
    } else {
        format!("After: {}", e.related.iter().map(|c| format!("{} ({} before)", c.text, ago(e.time - c.time))).collect::<Vec<_>>().join("; "))
    };
    (format!("{}  {}", short(e.time), e.what), if e.detail.is_empty() { rel } else { format!("{}. {rel}", e.detail) })
}

pub fn summary(events: &[StabilityEvent]) -> String {
    if events.is_empty() {
        "No related crashes or reboots in the last 30 days.".into()
    } else {
        format!("{} crash/reboot event(s) in the last 30 days. A change listed under an event may be a lead, not proof.", events.len())
    }
}

/// Only changes that can still be undone get a button: removed apps, and tweaks still switched on.
pub fn can_undo(c: &ChangeEntry, applied: &[String], known: impl Fn(&str) -> bool) -> bool {
    match (c.kind.as_deref(), c.id.as_deref()) {
        (Some("app"), _) => true,
        (Some("on"), Some(id)) => known(id) && applied.iter().any(|a| a == id),
        _ => false,
    }
}

pub fn log_error(what: &str, e: &dyn std::fmt::Display) {
    log::error(what, Some(e));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_roundtrip_and_format() {
        let t = parse_time("2026-11-10T14:35:54").unwrap();
        assert_eq!(iso(t), "2026-11-10T14:35:54");
        assert_eq!(short(t), "10 Nov 14:35");
        assert_eq!(parse_time("2026-11-10 14:35"), Some(t - 54));
        assert_eq!(iso(parse_time("2024-02-29T23:59:59").unwrap() + 1), "2024-03-01T00:00:00");
        assert_eq!(parse_time("garbage"), None);
        assert_eq!(ago(5 * 60), "5 min");
        assert_eq!(ago(3 * 3600 + 5), "3 h");
        assert_eq!(ago(2 * DAY + 5), "2 d");
    }

    #[test]
    fn record_and_read_with_bom_old_lines_and_expiry() {
        let d = tempfile::tempdir().unwrap();
        record(d.path(), "on", "adv.hags", "Hardware GPU scheduling");
        record(d.path(), "off", "adv.mpo", "Pipe | in label");
        let mut f = OpenOptions::new().append(true).open(d.path().join("changes.log")).unwrap();
        writeln!(f, "2020-01-01T00:00:00|on|old.id|Too old").unwrap();
        writeln!(f, "{}|just some old text", iso(now() - 60)).unwrap();
        let raw = fs::read(d.path().join("changes.log")).unwrap();
        assert_eq!(&raw[..3], &[0xEF, 0xBB, 0xBF], "BOM only once, at the start");
        assert_eq!(raw.windows(3).filter(|w| *w == [0xEF, 0xBB, 0xBF]).count(), 1);
        let c = changes(d.path(), now() + 5);
        assert_eq!(c.len(), 3, "the 2020 line is older than 30 days");
        assert!(c.iter().any(|x| x.text == "Switched on: Hardware GPU scheduling" && x.id.as_deref() == Some("adv.hags")));
        assert!(c.iter().any(|x| x.text == "Switched off: Pipe / in label"));
        assert!(c.iter().any(|x| x.kind.is_none() && x.text == "just some old text"));
    }

    #[test]
    fn events_filter_relate_and_describe() {
        let t = parse_time("2026-11-10T12:00:00").unwrap();
        let ch = vec![
            ChangeEntry { time: t - 2 * 3600, text: "Switched on: X".into(), kind: Some("on".into()), id: Some("a.x".into()), label: Some("X".into()) },
            ChangeEntry { time: t - 9 * DAY, text: "Switched on: Old".into(), kind: Some("on".into()), id: Some("a.old".into()), label: Some("Old".into()) },
            ChangeEntry { time: t + 60, text: "Switched on: Later".into(), kind: Some("on".into()), id: Some("a.l".into()), label: Some("Later".into()) },
        ];
        let json = r#"[
          {"T":"2026-11-10T12:00:00","Id":41},
          {"T":"2026-11-09T08:00:00","Id":1000,"App":"explorer.exe","Mod":"ntdll.dll","Code":"c0000005","Path":"C:\\Windows\\explorer.exe"},
          {"T":"2026-11-08T08:00:00","Id":1000,"App":"game.exe","Mod":"x.dll","Code":"c1","Path":"D:\\Games\\game.exe"},
          {"T":"2026-11-07T08:00:00","Id":1000,"App":"Calc.exe","Mod":"m","Code":"c2","Path":"C:\\Program Files\\WindowsApps\\Calc\\Calc.exe"}]"#;
        let e = parse_events(json, &ch, false, r"C:\Windows");
        assert_eq!(e.iter().map(|x| x.what.as_str()).collect::<Vec<_>>(), ["PC restarted unexpectedly", "explorer.exe crashed", "Calc.exe crashed"]);
        assert_eq!(parse_events(json, &ch, true, r"C:\Windows").len(), 4);
        assert_eq!(e[0].related.len(), 1, "only the change 2 h before, not 9 days before or after");
        let (title, detail) = describe(&e[0]);
        assert_eq!(title, "10 Nov 12:00  PC restarted unexpectedly");
        assert_eq!(detail, "After: Switched on: X (2 h before)");
        assert_eq!(describe(&e[1]).1, "ntdll.dll, c0000005. No MyTune change in the 7 days before.");
        assert_eq!(parse_events("", &ch, true, "C:\\Windows").len(), 0);
        // a single object, not an array
        assert_eq!(parse_events(r#"{"T":"2026-11-10T12:00:00","Id":4101}"#, &[], true, "C:").len(), 1);
        assert!(summary(&[]).starts_with("No related"));
        assert!(summary(&e).starts_with("3 crash/reboot"));
    }

    #[test]
    fn undo_only_when_still_possible() {
        let on = ChangeEntry { time: 0, text: String::new(), kind: Some("on".into()), id: Some("a.x".into()), label: None };
        let off = ChangeEntry { kind: Some("off".into()), ..on.clone() };
        let app = ChangeEntry { kind: Some("app".into()), ..on.clone() };
        let applied = vec!["a.x".to_string()];
        assert!(can_undo(&on, &applied, |_| true));
        assert!(!can_undo(&on, &[], |_| true));
        assert!(!can_undo(&on, &applied, |_| false));
        assert!(!can_undo(&off, &applied, |_| true));
        assert!(can_undo(&app, &[], |_| false));
    }

    /// Read only: the real event log answers (may be empty).
    #[test]
    fn real_events_query_runs() {
        let d = tempfile::tempdir().unwrap();
        let e = events(&*crate::tweaks::system_cmd(), d.path(), true).unwrap();
        eprintln!("{} events in 30 days, first: {:?}", e.len(), e.first().map(|x| &x.what));
    }
}
