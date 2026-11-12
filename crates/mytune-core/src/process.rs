//! Run powercfg / bcdedit / sc / schtasks and capture exit code + output (C# `Sys.Run` / `RunResult`).
//!
//! No console window. stdout then stderr, trimmed. A non-zero exit is NOT an error (see `run_checked`).
//! On timeout the whole process tree is killed and `Error::Timeout` returned (not logged, like C#).
//! Difference from C#: output is read as UTF-8 and, if that is invalid, as the OEM code page, so
//! non-ASCII text from native tools is not garbled.

use crate::error::{Error, Result};
use crate::log;
use std::io::Read;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use windows::Win32::Globalization::{MultiByteToWideChar, CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunResult {
    pub exit_code: i32,
    pub output: String,
}

fn decode(b: &[u8]) -> String {
    if let Ok(s) = std::str::from_utf8(b) {
        return s.to_string();
    }
    unsafe {
        let n = MultiByteToWideChar(CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), b, None);
        if n <= 0 {
            return String::from_utf8_lossy(b).into_owned();
        }
        let mut w = vec![0u16; n as usize];
        MultiByteToWideChar(CP_OEMCP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), b, Some(&mut w));
        String::from_utf16_lossy(&w)
    }
}

fn drain<R: Read + Send + 'static>(mut r: R) -> thread::JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut v = Vec::new();
        let _ = r.read_to_end(&mut v);
        v
    })
}

pub fn run(file: &str, args: &[&str]) -> Result<RunResult> {
    run_with(file, args, DEFAULT_TIMEOUT, None)
}

/// `log_as` replaces the logged command text (use it to keep secrets out of the log).
pub fn run_with(file: &str, args: &[&str], timeout: Duration, log_as: Option<&str>) -> Result<RunResult> {
    let shown = log_as.map(str::to_string).unwrap_or_else(|| format!("{file} {}", args.join(" ")).trim().to_string());
    let mut child = Command::new(file)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()?;
    let out = drain(child.stdout.take().unwrap());
    let err = drain(child.stderr.take().unwrap());
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if Instant::now() >= deadline {
            let _ = Command::new("taskkill")
                .args(["/T", "/F", "/PID", &child.id().to_string()])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .creation_flags(CREATE_NO_WINDOW)
                .status();
            let _ = child.kill();
            let _ = child.wait();
            let _ = (out.join(), err.join());
            return Err(Error::Timeout(shown));
        }
        thread::sleep(Duration::from_millis(15));
    };
    let mut text = decode(&out.join().unwrap_or_default());
    text.push_str(&decode(&err.join().unwrap_or_default()));
    let code = status.code().unwrap_or(-1);
    log::info(&format!("run {shown} -> {code}"));
    Ok(RunResult { exit_code: code, output: text.trim().to_string() })
}

/// Like `run`, but a non-zero exit becomes `Error::ProcessFailed`. Returns the output.
pub fn run_checked(file: &str, args: &[&str]) -> Result<String> {
    let r = run(file, args)?;
    if r.exit_code == 0 {
        Ok(r.output)
    } else {
        Err(Error::ProcessFailed { cmd: format!("{file} {}", args.join(" ")), code: r.exit_code, output: r.output })
    }
}

/// `%SystemRoot%\System32\<exe>`.
pub fn system32(exe: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    PathBuf::from(root).join("System32").join(exe)
}

/// bcdedit entry to edit: `{current}` if it exists, else `{default}` (this PC has no `{current}`).
/// Not cached, like C#: boot config can change.
pub fn bcd_entry() -> &'static str {
    match run("bcdedit", &["/enum", "{current}"]) {
        Ok(r) => bcd_entry_for(r.exit_code),
        Err(_) => "{default}",
    }
}

pub fn bcd_entry_for(enum_current_exit_code: i32) -> &'static str {
    if enum_current_exit_code == 0 {
        "{current}"
    } else {
        "{default}"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_output_and_exit_code() {
        let r = run("cmd", &["/c", "echo hi& echo oops 1>&2& exit 3"]).unwrap();
        assert_eq!(r.exit_code, 3);
        assert!(r.output.contains("hi") && r.output.contains("oops"), "{:?}", r.output);
    }

    #[test]
    fn checked_fails_on_nonzero() {
        assert_eq!(run_checked("cmd", &["/c", "echo ok"]).unwrap(), "ok");
        assert!(matches!(run_checked("cmd", &["/c", "exit 2"]), Err(Error::ProcessFailed { code: 2, .. })));
    }

    #[test]
    fn timeout_kills_tree() {
        let t = Instant::now();
        let r = run_with("cmd", &["/c", "ping -n 30 127.0.0.1 >nul"], Duration::from_millis(400), None);
        assert!(matches!(r, Err(Error::Timeout(_))));
        assert!(t.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn missing_program_is_error() {
        assert!(run("definitely-not-a-program-xyz", &[]).is_err());
    }

    #[test]
    fn oem_fallback_decodes() {
        assert_eq!(decode(b"plain"), "plain");
        assert_eq!(decode("é".as_bytes()), "é");
        assert!(!decode(&[0x82]).is_empty()); // invalid UTF-8 -> OEM, no panic
    }

    #[test]
    fn bcd_fallback() {
        assert_eq!(bcd_entry_for(0), "{current}");
        assert_eq!(bcd_entry_for(1), "{default}");
        assert!(system32("bcdedit.exe").ends_with("System32\\bcdedit.exe"));
    }
}

/// Hands this process's unused pages back to Windows (used when the window hides to the tray); pages come back on demand.
pub fn trim_working_set() {
    use windows::Win32::System::{ProcessStatus::K32EmptyWorkingSet, Threading::GetCurrentProcess};
    unsafe {
        let _ = K32EmptyWorkingSet(GetCurrentProcess());
    }
}
