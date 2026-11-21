//! MyTuneStart.exe, the target of the HKCU Run entry: starts the elevated "MyTune" task without a console window.
#![windows_subsystem = "windows"]

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn main() {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    let _ = Command::new(PathBuf::from(root).join("System32").join("schtasks.exe"))
        .args(["/run", "/tn", "MyTune"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn();
}
