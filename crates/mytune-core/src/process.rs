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

/// The child's output once it has exited. A grandchild that inherited the pipe (a service a CLI started) keeps
/// it open, and `join` would then wait for ever: after 2 s the reader thread is left behind.
// shortcut: output is dropped in that case, share the buffer with the reader if a caller needs the partial text
fn collect(h: thread::JoinHandle<Vec<u8>>) -> Vec<u8> {
    let until = Instant::now() + Duration::from_secs(2);
    while !h.is_finished() && Instant::now() < until {
        thread::sleep(Duration::from_millis(5));
    }
    if h.is_finished() { h.join().unwrap_or_default() } else { Vec::new() }
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
            return Err(Error::Timeout(shown));
        }
        thread::sleep(Duration::from_millis(15));
    };
    let mut text = decode(&collect(out));
    text.push_str(&decode(&collect(err)));
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

/// True when this process runs under another account than the desktop shell (elevated with a different admin):
/// HKCU and `%LOCALAPPDATA%` are then that admin's, not the signed-in user's. False when there is no shell to
/// compare with (Explorer restarting, a service).
pub fn shell_user_differs() -> bool {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Security::{EqualSid, GetTokenInformation, TokenUser, TOKEN_QUERY, TOKEN_USER};
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION};
    use windows::Win32::UI::WindowsAndMessaging::{GetShellWindow, GetWindowThreadProcessId};
    // u64: aligned for TOKEN_USER. 128 bytes hold it and the largest SID.
    fn user(process: HANDLE, buf: &mut [u64; 16]) -> bool {
        let mut token = HANDLE::default();
        unsafe {
            if OpenProcessToken(process, TOKEN_QUERY, &mut token).is_err() {
                return false;
            }
            let mut n = 0;
            let ok = GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), 128, &mut n).is_ok();
            let _ = CloseHandle(token);
            ok
        }
    }
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(GetShellWindow(), Some(&mut pid));
        if pid == 0 {
            return false;
        }
        let Ok(shell) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let (mut a, mut b) = ([0u64; 16], [0u64; 16]);
        let known = user(shell, &mut a) && user(GetCurrentProcess(), &mut b);
        let _ = CloseHandle(shell);
        known && EqualSid((*a.as_ptr().cast::<TOKEN_USER>()).User.Sid, (*b.as_ptr().cast::<TOKEN_USER>()).User.Sid).is_err()
    }
}

/// Restarts Explorer in this session only, and waits for Windows to bring it back. Only if Windows does not is it
/// started here, with the basic-user token: an elevated Explorer would be a shell nobody restarts.
/// Blocks up to ~8 s: call it off the UI thread. False when Explorer was not restarted or did not come back.
pub fn restart_explorer() -> bool {
    use windows::Win32::System::{RemoteDesktop::ProcessIdToSessionId, Threading::GetCurrentProcessId};
    // it would come back under MyTune's account instead of the signed-in user's
    if shell_user_differs() {
        log::info("Explorer not restarted: it belongs to another account than MyTune");
        return false;
    }
    let mut session = 0;
    if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) }.is_err() {
        log::error("Explorer not restarted: session unknown", None);
        return false;
    }
    let filter = format!("SESSION eq {session}");
    let tasklist = system32("tasklist.exe");
    let running = || run(&tasklist.to_string_lossy(), &["/FI", "IMAGENAME eq explorer.exe", "/FI", &filter, "/NH"]).is_ok_and(|r| r.output.to_ascii_lowercase().contains("explorer.exe"));
    let wait = |tries| (0..tries).any(|_| {
        thread::sleep(Duration::from_millis(500));
        running()
    });
    mark_planned_restart();
    let killed = run(&system32("taskkill.exe").to_string_lossy(), &["/F", "/IM", "explorer.exe", "/FI", &filter]).is_ok_and(|r| r.exit_code == 0);
    let mut back = wait(6);
    if back && !killed {
        log::error("Explorer not restarted: taskkill failed", None);
        return false; // the old shell is still the one running
    }
    if !back {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let exe = PathBuf::from(root).join("explorer.exe");
        let _ = run(&system32("runas.exe").to_string_lossy(), &["/trustlevel:0x20000", &exe.to_string_lossy()]);
        back = wait(10);
    }
    mark_planned_restart(); // the shell can come up late: the window counts from here
    if !back {
        log::error("Explorer did not come back after the restart", None);
    }
    back
}

/// When MyTune last restarted Explorer on purpose. The taskbar's crash-loop guard must not count those.
static PLANNED_RESTART: std::sync::Mutex<Option<Instant>> = std::sync::Mutex::new(None);

fn mark_planned_restart() {
    *PLANNED_RESTART.lock().unwrap_or_else(|p| p.into_inner()) = Some(Instant::now());
}

/// True for about 20 s after MyTune restarted Explorer itself.
pub fn explorer_restart_planned() -> bool {
    PLANNED_RESTART.lock().unwrap_or_else(|p| p.into_inner()).is_some_and(|t| t.elapsed() < Duration::from_secs(20))
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

    /// Read only. Tests run under the signed-in account, so its shell is the same user.
    #[test]
    fn shell_user_is_this_user() {
        assert!(!shell_user_differs());
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
