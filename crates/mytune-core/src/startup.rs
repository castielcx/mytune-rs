//! Tools page, startup programs (C# `StartupApps`): lists what starts with Windows and switches entries on
//! or off the way Task Manager does (the `StartupApproved` keys), so nothing is deleted and Task Manager
//! shows the same state.

use crate::error::{Error, Result};
use crate::log;
use crate::reg::{self, Hive, RegValue};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupEntry {
    /// `<source index>|<value or file name>`
    pub id: String,
    pub name: String,
    pub command: String,
    pub source: &'static str,
    pub enabled: bool,
}

const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN32: &str = r"Software\WOW6432Node\Microsoft\Windows\CurrentVersion\Run";
const APPROVED: &str = r"Software\Microsoft\Windows\CurrentVersion\Explorer\StartupApproved\";

struct Source {
    label: &'static str,
    hive: Hive,
    run_path: Option<&'static str>,
    approved: String,
    folder: Option<PathBuf>,
}

fn folder(base: impl Into<PathBuf>) -> PathBuf {
    base.into().join(r"Microsoft\Windows\Start Menu\Programs\StartUp")
}

fn sources() -> Vec<Source> {
    let a = |s: &str| format!("{APPROVED}{s}");
    vec![
        Source { label: "Current user", hive: Hive::Hkcu, run_path: Some(RUN), approved: a("Run"), folder: None },
        Source { label: "All users", hive: Hive::Hklm, run_path: Some(RUN), approved: a("Run"), folder: None },
        Source { label: "All users (32-bit)", hive: Hive::Hklm, run_path: Some(RUN32), approved: a("Run32"), folder: None },
        Source { label: "Startup folder", hive: Hive::Hkcu, run_path: None, approved: a("StartupFolder"), folder: Some(folder(std::env::var_os("APPDATA").unwrap_or_default())) },
        Source { label: "Startup folder (all users)", hive: Hive::Hklm, run_path: None, approved: a("StartupFolder"), folder: Some(folder(crate::util::program_data())) },
    ]
}

/// Task Manager's value: bit 0 of the first byte set (03) = disabled; missing or empty = enabled.
pub fn approved_enabled(data: Option<&[u8]>) -> bool {
    data.is_none_or(|d| d.is_empty() || d[0] & 1 == 0)
}

/// The 12-byte value Task Manager writes: 02 = enabled, 03 = disabled, then a FILETIME (disabled only).
pub fn approved_data(enabled: bool, filetime: u64) -> Vec<u8> {
    let mut d = vec![0u8; 12];
    d[0] = if enabled { 0x02 } else { 0x03 };
    if !enabled {
        d[4..12].copy_from_slice(&filetime.to_le_bytes());
    }
    d
}

fn filetime_now() -> u64 {
    let since_unix = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos() / 100).unwrap_or(0) as u64;
    since_unix + 116_444_736_000_000_000
}

fn is_enabled(s: &Source, name: &str) -> bool {
    match reg::get(s.hive, &s.approved, name) {
        Ok(Some(RegValue::Binary(d))) => approved_enabled(Some(&d)),
        _ => true,
    }
}

pub fn list() -> Vec<StartupEntry> {
    let mut list = Vec::new();
    for (i, s) in sources().iter().enumerate() {
        if let Some(run) = s.run_path {
            for name in reg::value_names(s.hive, run).unwrap_or_default() {
                if name.is_empty() || name.eq_ignore_ascii_case("MyTune") {
                    continue;
                }
                let command = match reg::get(s.hive, run, &name) {
                    Ok(Some(RegValue::String(c) | RegValue::ExpandString(c))) => c,
                    _ => String::new(),
                };
                list.push(StartupEntry { id: format!("{i}|{name}"), enabled: is_enabled(s, &name), name, command, source: s.label });
            }
        } else if let Some(dir) = &s.folder {
            let Ok(rd) = std::fs::read_dir(dir) else { continue };
            for e in rd.flatten().filter(|e| e.path().is_file()) {
                let file = e.file_name().to_string_lossy().into_owned();
                if file.eq_ignore_ascii_case("desktop.ini") {
                    continue;
                }
                let name = e.path().file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| file.clone());
                list.push(StartupEntry { id: format!("{i}|{file}"), enabled: is_enabled(s, &file), name, command: e.path().to_string_lossy().into_owned(), source: s.label });
            }
        }
    }
    list.sort_by_key(|e| e.name.to_lowercase());
    list
}

pub fn set(id: &str, enabled: bool) -> Result<()> {
    let (idx, name) = id.split_once('|').ok_or_else(|| Error::Os(format!("bad startup id {id}")))?;
    let src = sources();
    let s = idx.parse::<usize>().ok().and_then(|i| src.get(i)).ok_or_else(|| Error::Os(format!("bad startup id {id}")))?;
    reg::set(s.hive, &s.approved, name, &RegValue::Binary(approved_data(enabled, filetime_now())))?;
    log::info(&format!("startup {}: {} / {name}", if enabled { "enabled" } else { "disabled" }, s.label));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approved_value_like_task_manager() {
        assert!(approved_enabled(None));
        assert!(approved_enabled(Some(&[])));
        assert!(approved_enabled(Some(&[0x02, 0, 0, 0])));
        assert!(approved_enabled(Some(&[0x06, 0, 0, 0]))); // 06 = enabled too (bit 0 clear)
        assert!(!approved_enabled(Some(&[0x03, 0, 0, 0])));
        let off = approved_data(false, 0x0102030405060708);
        assert_eq!(off.len(), 12);
        assert_eq!(&off[..4], &[3, 0, 0, 0]);
        assert_eq!(&off[4..], &0x0102030405060708u64.to_le_bytes());
        assert_eq!(approved_data(true, 99), vec![2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert!(filetime_now() > 133_000_000_000_000_000);
    }

    #[test]
    fn bad_ids_are_errors() {
        assert!(set("nonsense", true).is_err());
        assert!(set("99|x", true).is_err());
    }

    /// Read only.
    #[test]
    fn real_list_reads() {
        let l = list();
        eprintln!("{} startup entries, first: {:?}", l.len(), l.first());
        assert!(l.windows(2).all(|w| w[0].name.to_lowercase() <= w[1].name.to_lowercase()));
    }

    /// Writes a StartupApproved value for a name that does not exist in any Run key, then removes it.
    #[test]
    #[ignore = "real system: writes real HKCU StartupApproved"]
    fn toggle_roundtrip_on_a_fake_entry() {
        let src = sources();
        let (idx, s) = src.iter().enumerate().find(|(_, s)| s.hive == Hive::Hkcu && s.run_path.is_some()).unwrap();
        let name = "MyTuneTestEntry-delete-me";
        let id = format!("{idx}|{name}");
        set(&id, false).unwrap();
        assert!(!is_enabled(s, name));
        set(&id, true).unwrap();
        assert!(is_enabled(s, name));
        reg::delete_value(s.hive, &s.approved, name).unwrap();
    }
}
