use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};
use windows::Win32::System::SystemInformation::GetLocalTime;

/// A Windows known folder, asked from the shell. Never from an environment variable: a non-admin can set
/// those through `HKCU\Environment`, and this app runs elevated.
pub(crate) fn known_folder(id: &windows::core::GUID) -> Result<PathBuf, String> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).map_err(|e| e.to_string())?;
        let text = p.to_string();
        CoTaskMemFree(Some(p.0 as *const _));
        text.map(PathBuf::from).map_err(|e| e.to_string())
    }
}

/// The real ProgramData folder, whatever the environment says. Not a known folder: HKLM stores it as
/// `%SystemDrive%\ProgramData` and the shell expands that from this process's environment.
pub fn program_data() -> PathBuf {
    profile_list_folder("ProgramData").unwrap_or_else(|| PathBuf::from(format!("{}\\ProgramData", windows_drive())))
}

/// A folder from HKLM's ProfileList, read raw, with `%SystemDrive%` taken from the real Windows folder.
pub(crate) fn profile_list_folder(value: &str) -> Option<PathBuf> {
    use crate::reg::{self, Hive, RegValue};
    match reg::get(Hive::Hklm, r"SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList", value) {
        Ok(Some(RegValue::String(v) | RegValue::ExpandString(v))) => expand_system_drive(&v, &windows_drive()).map(PathBuf::from),
        _ => None,
    }
}

/// A leading `%SystemDrive%` becomes `drive`; the rest keeps its case. `None` when empty or another variable remains.
fn expand_system_drive(raw: &str, drive: &str) -> Option<String> {
    const VAR: &str = "%SystemDrive%";
    let p = match raw.get(..VAR.len()) {
        Some(head) if head.eq_ignore_ascii_case(VAR) => format!("{drive}{}", &raw[VAR.len()..]),
        _ => raw.to_string(),
    };
    (!p.is_empty() && !p.contains('%')).then_some(p)
}

/// The drive of the real Windows folder, like `C:`.
pub(crate) fn windows_drive() -> String {
    windows_dir().to_string_lossy().get(..2).filter(|d| d.ends_with(':')).unwrap_or("C:").to_string()
}

/// The real Windows folder, whatever the environment says. Literal `C:\Windows` if the call fails.
pub fn windows_dir() -> PathBuf {
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;
    let mut buf = [0u16; 260];
    let n = unsafe { GetSystemWindowsDirectoryW(Some(&mut buf)) } as usize;
    if n == 0 || n >= buf.len() {
        return PathBuf::from("C:\\Windows");
    }
    PathBuf::from(std::ffi::OsString::from_wide(&buf[..n]))
}

/// This process's token with Administrators deny-only, no privileges and Medium integrity: what a standard user has.
fn standard_user_token() -> windows::core::Result<windows::core::Owned<windows::Win32::Foundation::HANDLE>> {
    use windows::core::Owned;
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::Security::*;
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    const SE_GROUP_INTEGRITY: u32 = 0x20;
    unsafe {
        let (mut process, mut restricted, mut token) = (HANDLE::default(), HANDLE::default(), HANDLE::default());
        OpenProcessToken(GetCurrentProcess(), TOKEN_DUPLICATE | TOKEN_QUERY, &mut process)?;
        let process = Owned::new(process);
        // u64: aligned for a SID, 72 bytes hold the largest one (68)
        let (mut admins, mut local_admin, mut medium) = ([0u64; 9], [0u64; 9], [0u64; 9]);
        let (admins, local_admin, medium) = (PSID(admins.as_mut_ptr().cast()), PSID(local_admin.as_mut_ptr().cast()), PSID(medium.as_mut_ptr().cast()));
        let mut n = SECURITY_MAX_SID_SIZE;
        CreateWellKnownSid(WinBuiltinAdministratorsSid, None, Some(admins), &mut n)?;
        n = SECURITY_MAX_SID_SIZE;
        // S-1-5-114 is enabled only in an elevated token
        CreateWellKnownSid(WinLocalAccountAndAdministratorSid, None, Some(local_admin), &mut n)?;
        let deny = [SID_AND_ATTRIBUTES { Sid: admins, Attributes: 0 }, SID_AND_ATTRIBUTES { Sid: local_admin, Attributes: 0 }];
        CreateRestrictedToken(*process, DISABLE_MAX_PRIVILEGE, Some(&deny), None, None, &mut restricted)?;
        let restricted = Owned::new(restricted);
        DuplicateTokenEx(*restricted, TOKEN_IMPERSONATE | TOKEN_QUERY | TOKEN_ADJUST_DEFAULT, None, SecurityImpersonation, TokenImpersonation, &mut token)?;
        let token = Owned::new(token);
        n = SECURITY_MAX_SID_SIZE;
        CreateWellKnownSid(WinMediumLabelSid, None, Some(medium), &mut n)?;
        let label = TOKEN_MANDATORY_LABEL { Label: SID_AND_ATTRIBUTES { Sid: medium, Attributes: SE_GROUP_INTEGRITY } };
        SetTokenInformation(*token, TokenIntegrityLevel, (&raw const label).cast(), std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32 + n)?;
        Ok(token)
    }
}

/// Runs `work` on its own short-lived thread under the standard-user token and waits for it. The impersonation ends
/// with that thread. An error when the token cannot be made: `work` then never runs, and never as admin.
/// `work` must not log or take the store lock: the data folder is admin-only.
pub(crate) fn as_standard_user<R: Send>(work: impl FnOnce() -> R + Send) -> Result<R, String> {
    use windows::Win32::Foundation::HANDLE;
    let token = standard_user_token().map_err(|e| e.to_string())?;
    let raw = token.0 as isize; // a HANDLE is not Send
    std::thread::scope(|scope| {
        scope
            .spawn(move || {
                unsafe { windows::Win32::System::Threading::SetThreadToken(None, Some(HANDLE(raw as *mut _))) }.map_err(|e| e.to_string())?;
                Ok(work())
            })
            .join()
            .unwrap_or_else(|_| Err("the standard-user thread panicked".to_string()))
    })
}

/// `as_standard_user` for file work: a missing token is one more I/O error.
pub(crate) fn as_user<T: Send>(work: impl FnOnce() -> std::io::Result<T> + Send) -> std::io::Result<T> {
    as_standard_user(work).map_err(std::io::Error::other)?
}

/// A junction, for tests of redirected paths.
#[cfg(test)]
pub(crate) fn test_junction(link: &std::path::Path, to: &std::path::Path) {
    let made = std::process::Command::new(crate::process::system32("cmd.exe")).args(["/c", "mklink", "/J"]).arg(link).arg(to).output().unwrap();
    assert!(made.status.success(), "{made:?}");
}

#[cfg(test)]
pub(crate) fn test_elevated() -> bool {
    unsafe { windows::Win32::UI::Shell::IsUserAnAdmin() }.as_bool()
}

/// A new folder only SYSTEM and Administrators can touch, holding `keep.txt` with the text `x`.
#[cfg(test)]
pub(crate) fn test_admin_only(dir: &std::path::Path) -> PathBuf {
    std::fs::create_dir(dir).unwrap();
    // a new folder also gets an explicit entry for the user, which `/inheritance:r` keeps
    let set = std::process::Command::new(crate::process::system32("icacls.exe"))
        .arg(dir)
        .args(["/inheritance:r", "/remove:g"])
        .arg(std::env::var("USERNAME").unwrap())
        .args(["/grant:r", "*S-1-5-18:(OI)(CI)F", "*S-1-5-32-544:(OI)(CI)F", "/Q"])
        .output()
        .unwrap();
    assert!(set.status.success(), "{set:?}");
    let file = dir.join("keep.txt");
    std::fs::write(&file, "x").unwrap();
    file
}

/// `MYTUNE_DIR` is for test binaries only: cargo puts those in a `deps` folder in every profile. The app
/// itself runs from `target\debug` or Program Files, cargo's own copy in `deps` keeps the name `MyTune`,
/// and an unknown exe does not qualify.
fn honors_test_dir(exe: Option<&std::path::Path>) -> bool {
    exe.is_some_and(|p| {
        p.parent().and_then(|d| d.file_name()).is_some_and(|d| d.eq_ignore_ascii_case("deps"))
            && p.file_stem().is_some_and(|s| !s.eq_ignore_ascii_case("MyTune"))
    })
}

/// `%ProgramData%\MyTune`. In test binaries the env var `MYTUNE_DIR` overrides it, so integration tests
/// never write to the real log or state (see `honors_test_dir`).
pub fn default_dir() -> PathBuf {
    if honors_test_dir(std::env::current_exe().ok().as_deref()) {
        if let Some(d) = std::env::var_os("MYTUNE_DIR") {
            return PathBuf::from(d);
        }
    }
    program_data().join("MyTune")
}

/// Local time as (year, month, day, hour, minute, second).
pub fn local_now() -> (u16, u16, u16, u16, u16, u16) {
    let t = unsafe { GetLocalTime() };
    (t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond)
}

/// `yyyy-MM-dd HH:mm:ss` in local time (log line prefix).
pub fn local_stamp() -> String {
    let (y, mo, d, h, mi, s) = local_now();
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}")
}

/// `yyyyMMddHHmmss` in local time (.corrupt file suffix).
pub fn local_compact() -> String {
    let (y, mo, d, h, mi, s) = local_now();
    format!("{y:04}{mo:02}{d:02}{h:02}{mi:02}{s:02}")
}

/// Current time as ISO 8601 with 7 fractional digits and `+00:00`, readable by C# DateTimeOffset.
pub fn iso_now() -> String {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    // civil-from-days (Howard Hinnant)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:07}+00:00",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
        d.subsec_nanos() / 100
    )
}

/// Days since 1970-01-01 for a civil date (inverse of the algorithm in `iso_now`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Local time like C# `DateTime.Now` serialised by System.Text.Json: `2026-11-10T13:45:12.1234567+01:00`.
pub fn local_iso_now() -> String {
    let (y, mo, d, h, mi, s) = local_now();
    let utc = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let local_secs = days_from_civil(y as i64, mo as i64, d as i64) * 86400 + h as i64 * 3600 + mi as i64 * 60 + s as i64;
    let offset_min = ((local_secs - utc.as_secs() as i64) as f64 / 60.0).round() as i64;
    let sign = if offset_min < 0 { '-' } else { '+' };
    format!(
        "{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{:07}{sign}{:02}:{:02}",
        utc.subsec_nanos() / 100,
        offset_min.abs() / 60,
        offset_min.abs() % 60
    )
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() / 4 * 3);
    let (mut acc, mut bits) = (0u32, 0u32);
    for ch in s.trim_end_matches('=').bytes() {
        let v = B64.iter().position(|&b| b == ch)? as u32;
        acc = acc << 6 | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits & 0xFF) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn mytune_dir_is_only_for_test_exes() {
        for exe in [
            r"C:\repo\target\debug\MyTune.exe",
            r"C:\Program Files\MyTune rs\MyTune.exe",
            r"C:\repo\target\release\deps\MyTune.exe",
            r"C:\repo\target\debug\deps\mytune.exe",
            r"C:\repo\target\debug\deps\MYTUNE.EXE",
            r"C:\x\other.exe",
        ] {
            assert!(!honors_test_dir(Some(Path::new(exe))), "{exe}");
        }
        for exe in [
            r"C:\repo\target\debug\deps\real_system-abc.exe",
            r"C:\repo\target\release\deps\catalog_sandbox-abc.exe",
            r"C:\repo\target\debug\DEPS\real_system-abc.exe",
        ] {
            assert!(honors_test_dir(Some(Path::new(exe))), "{exe}");
        }
        assert!(!honors_test_dir(None), "unknown exe");
    }

    // Setting the variables here would race other tests, so this test runs itself again as a child with them faked.
    #[test]
    fn program_data_ignores_the_environment() {
        const REAL: &str = "MYTUNE_TEST_REAL_PROGRAM_DATA";
        const NAME: &str = "util::tests::program_data_ignores_the_environment";
        if let Some(real) = std::env::var_os(REAL) {
            assert_eq!(program_data(), PathBuf::from(&real));
            assert_eq!(default_dir(), PathBuf::from(&real).join("MyTune"));
            return;
        }
        let real = program_data();
        eprintln!("program_data = {}, default_dir = {}", real.display(), default_dir().display());
        // an existing folder: with a missing one the shell falls back and the test would prove nothing
        let drive = tempfile::tempdir().unwrap();
        let fake = drive.path().join("ProgramData");
        std::fs::create_dir(&fake).unwrap();
        assert_ne!(real, fake);
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--test-threads=1"])
            .env(REAL, &real)
            .env("ProgramData", &fake)
            .env("ALLUSERSPROFILE", &fake)
            .env("SystemDrive", drive.path())
            .env_remove("MYTUNE_DIR")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success() && text.contains("1 passed"), "{text}");
    }

    fn on_thread_token() -> bool {
        use windows::Win32::Foundation::{CloseHandle, HANDLE};
        use windows::Win32::Security::TOKEN_QUERY;
        use windows::Win32::System::Threading::{GetCurrentThread, OpenThreadToken};
        let mut t = HANDLE::default();
        let open = unsafe { OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, true, &mut t) }.is_ok();
        if open {
            let _ = unsafe { CloseHandle(t) };
        }
        open
    }

    #[test]
    fn standard_user_work_runs_impersonated_and_does_not_leak() {
        assert!(!on_thread_token());
        assert_eq!(as_standard_user(|| (on_thread_token(), 7)).unwrap(), (true, 7));
        assert!(!on_thread_token(), "the calling thread stays itself");
        assert!(as_user(|| -> std::io::Result<()> { Err(std::io::Error::other("no")) }).is_err());
    }

    #[test]
    fn standard_user_cannot_write_an_admin_only_file() {
        if !test_elevated() {
            eprintln!("skipped: not elevated");
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let keep = test_admin_only(&d.path().join("admin"));
        let normal = d.path().join("normal.txt");
        as_user(|| std::fs::write(&normal, "ok")).unwrap();
        assert!(as_user(|| std::fs::write(&keep, "changed")).is_err());
        assert_eq!(std::fs::read(&keep).unwrap(), b"x");
        assert_eq!(std::fs::read(&normal).unwrap(), b"ok");
    }

    #[test]
    fn system_drive_is_expanded_only_at_the_start() {
        assert_eq!(expand_system_drive(r"%SystemDrive%\ProgramData", "C:").as_deref(), Some(r"C:\ProgramData"));
        assert_eq!(expand_system_drive(r"%SYSTEMDRIVE%\ProgramData", "D:").as_deref(), Some(r"D:\ProgramData"));
        assert_eq!(expand_system_drive(r"E:\Data", "C:").as_deref(), Some(r"E:\Data"));
        for bad in ["", r"%Other%\ProgramData", r"C:\%SystemDrive%\x", r"%SystemDrive%\%USERNAME%"] {
            assert_eq!(expand_system_drive(bad, "C:"), None, "{bad}");
        }
    }

    #[test]
    fn windows_dir_and_system32_ignore_the_environment() {
        const REAL: &str = "MYTUNE_TEST_REAL_WINDOWS";
        const NAME: &str = "util::tests::windows_dir_and_system32_ignore_the_environment";
        if let Some(real) = std::env::var_os(REAL) {
            assert_eq!(windows_dir(), PathBuf::from(&real));
            assert_eq!(crate::process::system32("cmd.exe"), PathBuf::from(&real).join("System32").join("cmd.exe"));
            return;
        }
        let real = windows_dir();
        assert!(real.join("System32").join("cmd.exe").is_file(), "{real:?}");
        let fake = std::env::temp_dir().join("mytune-fake-windows");
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", NAME, "--test-threads=1"])
            .env(REAL, &real)
            .env("SystemRoot", &fake)
            .env("windir", &fake)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(out.status.success() && text.contains("1 passed"), "{text}");
    }

    #[test]
    fn known_folder_reports_a_failure() {
        assert!(known_folder(&windows::core::GUID::zeroed()).is_err());
    }

    #[test]
    fn base64_roundtrip_and_known() {
        assert_eq!(b64_encode(b"Man"), "TWFu");
        assert_eq!(b64_encode(b"Ma"), "TWE=");
        assert_eq!(b64_encode(b"M"), "TQ==");
        assert_eq!(b64_encode(b""), "");
        for n in 0..40u8 {
            let v: Vec<u8> = (0..n).map(|i| i.wrapping_mul(37)).collect();
            assert_eq!(b64_decode(&b64_encode(&v)).unwrap(), v);
        }
        assert!(b64_decode("!!").is_none());
    }

    #[test]
    fn days_from_civil_roundtrip_and_local_iso() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11017);
        assert_eq!(days_from_civil(2026, 11, 10), 20767);
        let s = local_iso_now();
        assert_eq!(s.len(), "2026-11-10T13:45:12.1234567+01:00".len(), "{s}");
        assert!(s.as_bytes()[10] == b'T' && (s.contains('+') || s[19..].contains('-')));
    }

    #[test]
    fn iso_shape() {
        let s = iso_now();
        assert_eq!(s.len(), "2026-11-09T23:28:34.9960836+01:00".len());
        assert!(s.ends_with("+00:00") && s.as_bytes()[10] == b'T');
    }
}
