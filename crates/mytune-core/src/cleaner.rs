//! Tools page: junk cleaner (C# `Cleaner`) and shader caches (C# `ShaderCaches`).
//! Files in use are skipped, read-only flags are cleared, empty sub-folders are removed. Reparse points
//! (junctions, symlinks) are never followed. Cleaning is irreversible, so nothing here runs by itself.

use crate::log;
use crate::process::{run, run_with, system32};
use crate::reg::{self, Hive, RegValue};
use std::fs;
use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;
use crate::util::as_standard_user;
use windows::Win32::Storage::FileSystem::{FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ};

const REPARSE_POINT: u32 = 0x400;

enum Kind {
    /// Folders whose contents are deleted, or `folder|pattern` for matching files of one folder.
    Paths(fn() -> Vec<String>),
    RecycleBin,
    EventLogs,
}

pub struct CleanTarget {
    pub name: &'static str,
    pub description: &'static str,
    pub default_on: bool,
    kind: Kind,
}

fn env(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_default())
}

fn local() -> PathBuf {
    env("LOCALAPPDATA")
}

fn s(p: PathBuf) -> String {
    p.to_string_lossy().into_owned()
}

fn chromium_caches(user_data: &str) -> Vec<String> {
    let root = local().join(user_data);
    let Ok(rd) = fs::read_dir(&root) else { return vec![] };
    let mut out = Vec::new();
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if e.path().is_dir() && (name == "Default" || name.starts_with("Profile ")) {
            for c in ["Cache", "Code Cache", "GPUCache", "Service Worker\\CacheStorage"] {
                out.push(s(e.path().join(c)));
            }
        }
    }
    out
}

fn firefox_caches() -> Vec<String> {
    let Ok(rd) = fs::read_dir(local().join(r"Mozilla\Firefox\Profiles")) else { return vec![] };
    rd.flatten().filter(|e| e.path().is_dir()).map(|e| s(e.path().join("cache2"))).collect()
}

fn t(name: &'static str, description: &'static str, default_on: bool, kind: Kind) -> CleanTarget {
    CleanTarget { name, description, default_on, kind }
}

pub fn targets() -> Vec<CleanTarget> {
    const WEB: &str = "Cached web files (you stay signed in)";
    vec![
        t("Temporary files", "Your temp folder", true, Kind::Paths(|| vec![s(std::env::temp_dir())])),
        t("Windows temporary files", r"C:\Windows\Temp", true, Kind::Paths(|| vec![s(crate::util::windows_dir().join("Temp"))])),
        t("Error reports", "Windows Error Reporting archive and queue", true, Kind::Paths(|| {
            let data = crate::util::program_data();
            vec![s(data.join(r"Microsoft\Windows\WER\ReportArchive")), s(data.join(r"Microsoft\Windows\WER\ReportQueue"))]
        })),
        t("Internet cache", "Windows INetCache", true, Kind::Paths(|| vec![s(local().join(r"Microsoft\Windows\INetCache"))])),
        t("Chrome cache", WEB, true, Kind::Paths(|| chromium_caches(r"Google\Chrome\User Data"))),
        t("Edge cache", WEB, true, Kind::Paths(|| chromium_caches(r"Microsoft\Edge\User Data"))),
        t("Brave cache", WEB, true, Kind::Paths(|| chromium_caches(r"BraveSoftware\Brave-Browser\User Data"))),
        t("Firefox cache", WEB, true, Kind::Paths(firefox_caches)),
        t("Thumbnail and icon cache", "Rebuilt automatically by Explorer", false, Kind::Paths(|| {
            let dir = s(local().join(r"Microsoft\Windows\Explorer"));
            vec![format!("{dir}|thumbcache_*.db"), format!("{dir}|iconcache_*.db")]
        })),
        t("Prefetch", "Windows rebuilds it, but apps may start slower at first", false, Kind::Paths(|| vec![s(crate::util::windows_dir().join("Prefetch"))])),
        t("DirectX shader cache", "Games recompile shaders and may stutter at first", false, Kind::Paths(|| vec![s(local().join("D3DSCache"))])),
        t("Recycle Bin", "Permanently deletes everything in the Recycle Bin", false, Kind::RecycleBin),
        t("Windows event logs", "Clears all event logs (useful history for troubleshooting)", false, Kind::EventLogs),
    ]
}

// ---------------------------------------------------------------- files

/// `*` wildcard match, case-insensitive (enough for `thumbcache_*.db`).
fn wildcard(pattern: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pattern.to_lowercase().chars().collect(), name.to_lowercase().chars().collect());
    fn go(p: &[char], n: &[char]) -> bool {
        match p.split_first() {
            None => n.is_empty(),
            Some(('*', rest)) => (0..=n.len()).any(|i| go(rest, &n[i..])),
            Some((c, rest)) => n.first() == Some(c) && go(rest, &n[1..]),
        }
    }
    go(&p, &n)
}

fn is_reparse(md: &fs::Metadata) -> bool {
    md.file_attributes() & REPARSE_POINT != 0
}

fn walk(dir: &Path, recurse: bool, pattern: Option<&str>, files: &mut Vec<PathBuf>, dirs: &mut Vec<PathBuf>) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let Ok(md) = e.metadata() else { continue };
        if is_reparse(&md) {
            continue;
        }
        if md.is_dir() {
            if recurse {
                dirs.push(e.path());
                walk(&e.path(), true, pattern, files, dirs);
            }
        } else if pattern.is_none_or(|p| wildcard(p, &e.file_name().to_string_lossy())) {
            files.push(e.path());
        }
    }
}

/// (files, folders) a spec covers. `dir|pattern` is one folder, no recursion.
fn collect(spec: &str) -> (Vec<PathBuf>, Vec<PathBuf>) {
    let (dir, pattern) = match spec.split_once('|') {
        Some((d, p)) => (d, Some(p)),
        None => (spec, None),
    };
    let (mut files, mut dirs) = (vec![], vec![]);
    walk(Path::new(dir), pattern.is_none(), pattern, &mut files, &mut dirs);
    (files, dirs)
}

pub fn size_specs(specs: &[String]) -> u64 {
    specs.iter().flat_map(|sp| collect(sp).0).filter_map(|f| fs::metadata(f).ok()).map(|m| m.len()).sum()
}

/// The only roots cleaned with admin rights. An exact match: a child of one can be a folder a non-admin made
/// (Windows\Temp allows that), and a spec can come from HKCU (`SteamPath`). Not the WER folders: Authenticated
/// Users can write and delete on those roots, so they run as a standard user.
fn machine_roots() -> [PathBuf; 2] {
    let win = crate::util::windows_dir();
    [win.join("Temp"), win.join("Prefetch")]
}

/// Compared on path components, ignoring case, so `C:\Windows\TempX` and `C:\Windows\Temp\..\System32` do not match.
/// The temp folder is never a machine target: a non-admin can point `TEMP` at a machine root.
fn is_machine_spec(spec: &str, roots: &[PathBuf], temp: &Path) -> bool {
    let low = |p: &Path| p.components().map(|c| c.as_os_str().to_string_lossy().to_lowercase()).collect::<Vec<_>>();
    let dir = low(Path::new(spec.split_once('|').map_or(spec, |(d, _)| d)));
    dir != low(temp) && roots.iter().any(|r| low(r) == dir)
}

/// Deletes the files, then empty folders (deepest first). Returns the bytes freed.
fn delete_spec(spec: &str) -> u64 {
    let mut freed = 0;
    let (files, mut dirs) = collect(spec);
    for f in files {
        let Ok(md) = fs::metadata(&f) else { continue };
        let mut perm = md.permissions();
        if perm.readonly() {
            #[allow(clippy::permissions_set_readonly_false)]
            perm.set_readonly(false);
            let _ = fs::set_permissions(&f, perm);
        }
        if fs::remove_file(&f).is_ok() {
            freed += md.len();
        }
    }
    dirs.sort_by_key(|d| std::cmp::Reverse(d.as_os_str().len()));
    for d in dirs {
        let _ = fs::remove_dir(d); // fails while not empty, which is what we want
    }
    freed
}

/// One machine root, with admin rights but never through a link. Each top-level entry goes on its own, so a locked
/// file stops nothing else, and `remove_dir_all` does not follow junctions. Links themselves are left alone.
fn clean_machine(spec: &str) -> u64 {
    let (dir, pattern) = match spec.split_once('|') {
        Some((d, p)) => (d, Some(p)),
        None => (spec, None),
    };
    // Held for the whole loop: without write or delete sharing, nobody can turn the root into a link meanwhile.
    let held = fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ.0)
        .custom_flags((FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT).0)
        .open(dir);
    match held.as_ref().map(|f| f.metadata()) {
        Ok(Ok(m)) if m.is_dir() && !is_reparse(&m) => {}
        Ok(_) => {
            log::error(&format!("Cleaner skipped {dir}: it is a link or not a folder"), None);
            return 0;
        }
        Err(e) => {
            log::error(&format!("Cleaner skipped {dir}"), Some(e));
            return 0;
        }
    }
    let Ok(rd) = fs::read_dir(dir) else { return 0 };
    let mut freed = 0;
    for e in rd.flatten() {
        let Ok(md) = e.metadata() else { continue }; // of the entry itself, a link is not followed
        if is_reparse(&md) {
            continue;
        }
        let path = e.path();
        if md.is_dir() {
            if pattern.is_none() {
                let spec = [s(path)];
                let before = size_specs(&spec);
                let _ = fs::remove_dir_all(&spec[0]);
                freed += before.saturating_sub(size_specs(&spec)); // a locked file keeps part of it
            }
        } else if pattern.is_none_or(|p| wildcard(p, &e.file_name().to_string_lossy())) && fs::remove_file(&path).is_ok() {
            freed += md.len();
        }
    }
    freed
}

/// Machine roots keep admin rights without following links. Everything else is deleted as a standard user, so a
/// junction swapped in after `collect` can reach only what the user could delete anyway. Returns the bytes freed.
pub fn clean_specs(specs: &[String]) -> u64 {
    let roots = machine_roots();
    let temp = std::env::temp_dir();
    let (machine, user): (Vec<&String>, Vec<&String>) = specs.iter().partition(|sp| is_machine_spec(sp, &roots, &temp));
    let mut freed: u64 = machine.iter().map(|sp| clean_machine(sp)).sum();
    if !user.is_empty() {
        match as_standard_user(|| user.iter().map(|sp| delete_spec(sp)).sum::<u64>()) {
            Ok(n) => freed += n,
            Err(e) => log::error("Cleaner skipped user folders: no standard-user token", Some(&e)),
        }
    }
    freed
}

// ---------------------------------------------------------------- recycle bin and event logs

fn recycle_bin_size() -> u64 {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{SHQueryRecycleBinW, SHQUERYRBINFO};
    let mut info = SHQUERYRBINFO { cbSize: std::mem::size_of::<SHQUERYRBINFO>() as u32, ..Default::default() };
    match unsafe { SHQueryRecycleBinW(PCWSTR::null(), &mut info) } {
        Ok(()) => info.i64Size.max(0) as u64,
        Err(_) => 0,
    }
}

fn empty_recycle_bin() -> u64 {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::SHEmptyRecycleBinW;
    let size = recycle_bin_size();
    // no confirmation, progress or sound
    let _ = unsafe { SHEmptyRecycleBinW(None, PCWSTR::null(), 0x1 | 0x2 | 0x4) };
    size
}

fn clear_event_logs() -> u64 {
    let wevtutil = system32("wevtutil.exe").to_string_lossy().into_owned();
    let Ok(list) = run(&wevtutil, &["el"]) else { return 0 };
    for name in list.output.lines().map(str::trim).filter(|l| !l.is_empty()) {
        // One log that hangs or is locked must not stop the rest.
        if let Err(e) = run_with(&wevtutil, &["cl", name], Duration::from_secs(10), None) {
            log::error(&format!("clear event log {name}"), Some(&e));
        }
    }
    0
}

pub fn size(t: &CleanTarget) -> u64 {
    match &t.kind {
        Kind::Paths(f) => size_specs(&f()),
        Kind::RecycleBin => recycle_bin_size(),
        Kind::EventLogs => 0,
    }
}

pub fn clean(t: &CleanTarget) -> u64 {
    let freed = match &t.kind {
        Kind::Paths(f) => clean_specs(&f()),
        Kind::RecycleBin => empty_recycle_bin(),
        Kind::EventLogs => clear_event_logs(),
    };
    log::info(&format!("cleaned {}: {freed} bytes", t.name));
    freed
}

/// "1.5 GB", "320 MB", "12 KB" like the C# `Fmt`.
pub fn fmt_size(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} GB", b as f64 / (1u64 << 30) as f64)
    } else if b >= 1 << 20 {
        format!("{:.0} MB", (b as f64 / (1u64 << 20) as f64).round_ties_even())
    } else {
        format!("{:.0} KB", (b as f64 / 1024.0).round_ties_even())
    }
}

// ---------------------------------------------------------------- shader caches

pub struct ShaderCache {
    pub name: &'static str,
    pub detail: &'static str,
    pub paths: Vec<String>,
    pub bytes: u64,
}

fn shader_definitions() -> Vec<(&'static str, &'static str, Vec<PathBuf>)> {
    let l = local();
    let mut v = vec![
        ("NVIDIA shader cache", "Driver cache; games and apps rebuild it as you play", vec![l.join(r"NVIDIA\DXCache"), l.join(r"NVIDIA\GLCache")]),
        ("DirectX shader cache", "Windows cache used by DirectX games", vec![l.join("D3DSCache")]),
        ("AMD shader cache", "Driver cache for Radeon graphics", vec![l.join(r"AMD\DxCache"), l.join(r"AMD\GLCache")]),
    ];
    if let Ok(Some(RegValue::String(steam))) = reg::get(Hive::Hkcu, r"Software\Valve\Steam", "SteamPath") {
        if !steam.is_empty() {
            v.push(("Steam shader cache", "Pre-compiled shaders Steam downloads for your games", vec![PathBuf::from(steam.replace('/', "\\")).join(r"steamapps\shadercache")]));
        }
    }
    v
}

pub fn shader_caches() -> Vec<ShaderCache> {
    shader_definitions()
        .into_iter()
        .filter_map(|(name, detail, paths)| {
            let paths: Vec<String> = paths.into_iter().filter(|p| p.is_dir()).map(s).collect();
            (!paths.is_empty()).then(|| ShaderCache { name, detail, bytes: size_specs(&paths), paths })
        })
        .collect()
}

/// Deletes the contents of the cache folders (files in use are skipped). Returns the bytes freed.
pub fn clear_shader_cache(paths: &[String]) -> u64 {
    let freed = clean_specs(paths);
    log::info(&format!("cleared shader cache {}: {} MB", paths.join(", "), freed / 1_048_576));
    freed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wildcard_matches() {
        assert!(wildcard("thumbcache_*.db", "thumbcache_256.db"));
        assert!(wildcard("THUMBCACHE_*.DB", "thumbcache_idx.db"));
        assert!(!wildcard("thumbcache_*.db", "iconcache_32.db"));
        assert!(wildcard("*", "anything"));
    }

    #[test]
    fn cleans_files_readonly_and_empty_dirs_but_not_patterns_outside() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        fs::create_dir_all(root.join("a/b/c")).unwrap();
        fs::create_dir_all(root.join("keep")).unwrap();
        fs::write(root.join("one.tmp"), vec![0u8; 1000]).unwrap();
        fs::write(root.join("a/b/two.tmp"), vec![0u8; 500]).unwrap();
        let ro = root.join("a/ro.tmp");
        fs::write(&ro, vec![0u8; 250]).unwrap();
        let mut p = fs::metadata(&ro).unwrap().permissions();
        p.set_readonly(true);
        fs::set_permissions(&ro, p).unwrap();
        let spec = root.to_string_lossy().into_owned();
        assert_eq!(size_specs(&[spec.clone()]), 1750);
        assert_eq!(clean_specs(&[spec.clone()]), 1750);
        assert!(!root.join("a").exists(), "empty folders are removed deepest first");
        assert!(!root.join("keep").exists());
        assert!(root.exists(), "the root itself stays");
        assert_eq!(size_specs(&[spec]), 0);
    }

    #[test]
    fn pattern_spec_touches_only_matching_files_in_one_folder() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("thumbcache_32.db"), vec![0u8; 100]).unwrap();
        fs::write(root.join("other.db"), vec![0u8; 100]).unwrap();
        fs::write(root.join("sub/thumbcache_1.db"), vec![0u8; 100]).unwrap();
        let spec = format!("{}|thumbcache_*.db", root.display());
        assert_eq!(clean_specs(&[spec]), 100);
        assert!(root.join("other.db").exists() && root.join("sub/thumbcache_1.db").exists());
        assert!(root.join("sub").exists());
    }

    use crate::util::{test_admin_only as admin_only, test_elevated as elevated, test_junction as junction};

    #[test]
    fn only_exact_machine_roots_are_machine_specs() {
        let roots = [PathBuf::from(r"C:\Windows\Temp"), PathBuf::from(r"C:\Windows\Prefetch")];
        let temp = Path::new(r"C:\Users\me\AppData\Local\Temp\");
        for sp in [r"C:\Windows\Temp", r"c:\WINDOWS\tEMP\", "C:/Windows/Temp", r"C:\Windows\Temp|*.tmp", r"C:\windows\PREFETCH"] {
            assert!(is_machine_spec(sp, &roots, temp), "{sp}");
        }
        for sp in [r"C:\Windows\TempX", r"C:\Windows\Temp\sub", r"C:\Windows\Temp\..\System32", r"C:\Windows", r"D:\Windows\Temp", r"\\?\C:\Windows\Temp", "", r"C:\Users\me\AppData\Local\Temp", r"C:\ProgramData\Microsoft\Windows\WER\ReportQueue"] {
            assert!(!is_machine_spec(sp, &roots, temp), "{sp}");
        }
        // TEMP pointed at a machine root: "Temporary files" must not clean it as admin
        let faked = Path::new(r"c:\windows\prefetch\");
        assert!(!is_machine_spec(r"C:\Windows\Prefetch", &roots, faked));
        assert!(is_machine_spec(r"C:\Windows\Temp", &roots, faked));
        assert!(machine_roots().iter().all(|r| !r.to_string_lossy().to_lowercase().contains("wer")), "WER runs as a standard user");
    }

    #[test]
    fn machine_delete_skips_a_root_that_is_a_junction() {
        let d = tempfile::tempdir().unwrap();
        let (root, outside) = (d.path().join("root"), d.path().join("outside"));
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.txt"), "keep").unwrap();
        junction(&root, &outside);
        assert_eq!(clean_machine(&s(root)), 0);
        assert!(outside.join("keep.txt").exists());
    }

    #[test]
    fn standard_user_token_cannot_delete_an_admin_only_file() {
        if !elevated() {
            eprintln!("skipped: not elevated");
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let locked = admin_only(&d.path().join("admin"));
        let normal = d.path().join("normal.txt");
        fs::write(&normal, "x").unwrap();
        let (a, b) = as_standard_user(|| (fs::remove_file(&locked).is_ok(), fs::remove_file(&normal).is_ok())).unwrap();
        assert!(!a && locked.exists(), "the admin-only file must survive");
        assert!(b && !normal.exists(), "a normal file is deleted");
    }

    #[test]
    fn user_spec_through_a_junction_deletes_nothing_in_an_admin_folder() {
        if !elevated() {
            eprintln!("skipped: not elevated");
            return;
        }
        let d = tempfile::tempdir().unwrap();
        let admin = d.path().join("admin");
        let keep = admin_only(&admin);
        let link = d.path().join("link");
        junction(&link, &admin);
        assert_eq!(clean_specs(&[s(link)]), 0);
        assert!(keep.exists());
    }

    #[test]
    fn machine_delete_does_not_follow_a_junction() {
        let d = tempfile::tempdir().unwrap();
        let (root, outside) = (d.path().join("root"), d.path().join("outside"));
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(outside.join("keep.txt"), "keep").unwrap();
        fs::write(root.join("a.txt"), vec![0u8; 10]).unwrap();
        fs::write(root.join("sub").join("b.txt"), vec![0u8; 5]).unwrap();
        junction(&root.join("top"), &outside);
        junction(&root.join("sub").join("deep"), &outside);
        assert_eq!(clean_machine(&s(root.clone())), 15);
        assert!(outside.join("keep.txt").exists(), "the junction's target survives");
        assert!(!root.join("a.txt").exists() && !root.join("sub").exists());
        assert!(fs::symlink_metadata(root.join("top")).is_ok(), "a top-level link is left alone");
    }

    #[test]
    fn locked_file_does_not_stop_other_machine_entries() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        fs::create_dir(root.join("z_sub")).unwrap();
        fs::write(root.join("a_locked.txt"), vec![0u8; 7]).unwrap();
        fs::write(root.join("m_other.txt"), vec![0u8; 20]).unwrap();
        fs::write(root.join("z_sub").join("x.txt"), vec![0u8; 3]).unwrap();
        let ro = root.join("r_readonly.txt");
        fs::write(&ro, vec![0u8; 4]).unwrap();
        let mut p = fs::metadata(&ro).unwrap().permissions();
        p.set_readonly(true);
        fs::set_permissions(&ro, p).unwrap();
        let held = fs::OpenOptions::new().read(true).share_mode(0).open(root.join("a_locked.txt")).unwrap();
        assert_eq!(clean_machine(&s(root.to_path_buf())), 27);
        drop(held);
        assert!(root.join("a_locked.txt").exists());
        assert!(!root.join("m_other.txt").exists() && !ro.exists() && !root.join("z_sub").exists());
    }

    #[test]
    fn size_format_matches_csharp() {
        assert_eq!(fmt_size(500), "0 KB");
        assert_eq!(fmt_size(2048), "2 KB");
        assert_eq!(fmt_size(5 << 20), "5 MB");
        assert_eq!(fmt_size(3 << 29), "1.5 GB");
    }

    #[test]
    fn target_list_matches_csharp() {
        let t = targets();
        assert_eq!(t.len(), 13);
        assert_eq!(t.iter().filter(|x| x.default_on).count(), 8);
        assert!(!t.iter().find(|x| x.name == "Recycle Bin").unwrap().default_on);
    }

    /// Read only: sizes of every folder target and the recycle bin work on this PC.
    #[test]
    fn real_sizes_do_not_panic() {
        for t in targets().iter().filter(|t| t.name != "Windows event logs") {
            eprintln!("{}: {}", t.name, fmt_size(size(t)));
        }
        eprintln!("{:?}", shader_caches().iter().map(|c| (c.name, c.bytes)).collect::<Vec<_>>());
    }
}
