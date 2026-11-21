//! What fills the system drive, by category (Home's storage bar). One background-priority thread walks the
//! folders each category owns; the result is kept in `%ProgramData%\MyTune\diskuse.json`.
//!
//! The UI only calls `snapshot` (a short lock, no I/O) and `start` (spawns the thread, which also does the
//! first load of the saved file and decides whether an automatic scan is due).

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::Mutex;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// The categories in bar order; "Other" is what is left of the used space.
pub const NAMES: [&str; 8] = ["Games", "Apps", "Windows", "Documents", "Pictures", "Videos", "Music", "Downloads"];
const GAMES: usize = 0;
const APPS: usize = 1;
const WINDOWS: usize = 2;

/// A saved result is fresh for a day.
const FRESH_SECS: u64 = 24 * 3600;

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    /// Letter of the drive Windows is on.
    pub drive: char,
    pub total: u64,
    pub free: u64,
    /// Bytes per category, in the order of `NAMES`.
    pub cats: [u64; 8],
    /// Unix seconds of the scan; 0 = only total and free are known, nothing was measured yet.
    pub at: u64,
}

impl Usage {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.free)
    }

    /// Used space no category claims. 0 when the categories add up to more than is used (hard links in the
    /// Windows folder are counted once per name).
    pub fn other(&self) -> u64 {
        self.used().saturating_sub(self.cats.iter().sum())
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Snapshot {
    pub usage: Option<Usage>,
    /// A scan is walking the drive now.
    pub scanning: bool,
}

static LATEST: Mutex<Option<Usage>> = Mutex::new(None);
static LOADED: AtomicBool = AtomicBool::new(false);
static BUSY: AtomicBool = AtomicBool::new(false);
static SCANNING: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);

/// For the UI, every frame: no I/O, the lock is never held while scanning.
pub fn snapshot() -> Snapshot {
    Snapshot { usage: *LATEST.lock().unwrap_or_else(|e| e.into_inner()), scanning: SCANNING.load(Relaxed) }
}

/// Stops a running scan; its partial result is dropped.
pub fn cancel() {
    CANCEL.store(true, Relaxed);
}

/// Exe names (no `.exe`) of the Game Hub games in state.json, for `start`.
pub fn game_stems(store: &crate::store::Store) -> Vec<String> {
    let games = store.top("Games").and_then(|g| g.as_array());
    games.into_iter().flatten().filter_map(|g| Some(Path::new(g.get("ExePath")?.as_str()?).file_stem()?.to_str()?.to_string())).collect()
}

/// Measures on a background-priority thread; one scan at a time. `auto`: only when the saved result is
/// older than a day and none of `games` runs. `done` is called on that thread whenever `snapshot` changed.
pub fn start(auto: bool, games: Vec<String>, done: impl Fn() + Send + 'static) {
    if BUSY.swap(true, Relaxed) {
        return;
    }
    std::thread::spawn(move || {
        run(auto, &games, &done);
        SCANNING.store(false, Relaxed);
        BUSY.store(false, Relaxed);
        done();
    });
}

fn run(auto: bool, games: &[String], done: &dyn Fn()) {
    let set = |u: Usage| *LATEST.lock().unwrap_or_else(|e| e.into_inner()) = Some(u);
    let drive = system_drive();
    if !LOADED.swap(true, Relaxed) {
        let saved = fs::read(file()).ok().and_then(|b| serde_json::from_slice::<Usage>(&b).ok()).filter(|u| u.drive == drive);
        if let Some(u) = saved.or_else(|| space(drive).map(|(total, free)| Usage { drive, total, free, ..Default::default() })) {
            set(u);
            done();
        }
    }
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if auto {
        let fresh = snapshot().usage.is_some_and(|u| u.at != 0 && now.saturating_sub(u.at) < FRESH_SECS);
        if fresh || game_runs(games) {
            return;
        }
    }
    CANCEL.store(false, Relaxed);
    SCANNING.store(true, Relaxed);
    done();
    background(true);
    let began = Instant::now();
    let cats = scan(drive);
    background(false);
    let (Some(cats), Some((total, free))) = (cats, space(drive)) else { return };
    let u = Usage { drive, total, free, cats, at: now };
    set(u);
    if let Ok(json) = serde_json::to_vec(&u) {
        let _ = fs::create_dir_all(crate::util::default_dir());
        let _ = fs::write(file(), json);
    }
    crate::log::info(&format!("disk use: {drive}: measured in {:.0} s", began.elapsed().as_secs_f64()));
}

fn file() -> PathBuf {
    crate::util::default_dir().join("diskuse.json")
}

fn system_drive() -> char {
    std::env::var("SystemDrive").ok().and_then(|s| s.chars().next()).unwrap_or('C').to_ascii_uppercase()
}

/// (total, free) bytes of a drive.
fn space(drive: char) -> Option<(u64, u64)> {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let (mut free, mut total) = (0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(&HSTRING::from(format!("{drive}:\\")), None, Some(&mut total), Some(&mut free)).ok()? };
    Some((total, free))
}

fn game_runs(stems: &[String]) -> bool {
    use crate::procs::Procs;
    let list = crate::procs::WinProcs.list().unwrap_or_default();
    list.iter().any(|p| stems.iter().any(|s| s.eq_ignore_ascii_case(&p.name)))
}

/// Background mode lowers the thread's CPU, disk and memory priority together.
fn background(on: bool) {
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN, THREAD_MODE_BACKGROUND_END};
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), if on { THREAD_MODE_BACKGROUND_BEGIN } else { THREAD_MODE_BACKGROUND_END });
    }
}

// ---------------------------------------------------------------- which folder is what

/// A path as the roots are compared: lower case, backslashes, no trailing one.
fn key(p: &Path) -> String {
    p.to_string_lossy().replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// Folders (keys) with their category, each folder once: the first claim wins, so list games before apps.
fn dedup(roots: Vec<(String, usize)>) -> Vec<(String, usize)> {
    let mut seen = HashSet::new();
    roots.into_iter().filter(|(k, _)| !k.is_empty() && seen.insert(k.clone())).collect()
}

fn known_folder(id: &windows::core::GUID) -> Option<PathBuf> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::{SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        let p = SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        s.map(PathBuf::from)
    }
}

/// Folders games are installed in.
// shortcut: Steam libraries, Epic's install list and the launchers' usual folders; a Battle.net, Ubisoft or
// EA game installed elsewhere in Program Files counts as an app. Read the uninstall list if that matters.
fn game_roots(drive: char) -> Vec<PathBuf> {
    use crate::reg::{self, Hive, RegValue};
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut out = Vec::new();
    if let Ok(Some(RegValue::String(steam))) = reg::get(Hive::Hkcu, "Software\\Valve\\Steam", "SteamPath") {
        let steam = PathBuf::from(steam);
        // each library is a line `"path"  "D:\\SteamLibrary"`
        let vdf = fs::read_to_string(steam.join("steamapps\\libraryfolders.vdf")).unwrap_or_default();
        let libs = vdf.lines().filter(|l| l.trim_start().starts_with("\"path\"")).filter_map(|l| l.rsplit('"').nth(1)).map(|p| PathBuf::from(p.replace("\\\\", "\\")));
        out.extend(libs.chain([steam]).map(|lib| lib.join("steamapps")));
    }
    let manifests = crate::util::program_data().join("Epic\\EpicGamesLauncher\\Data\\Manifests");
    for e in fs::read_dir(manifests).into_iter().flatten().flatten() {
        let item = fs::read(e.path()).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok());
        out.extend(item.as_ref().and_then(|v| v.get("InstallLocation")?.as_str()).map(PathBuf::from));
    }
    let root = PathBuf::from(format!("{drive}:\\"));
    out.extend(["Riot Games", "XboxGames", "Games", "GOG Games"].map(|d| root.join(d)));
    for pf in [env("ProgramFiles"), env("ProgramFiles(x86)")].into_iter().flatten() {
        out.extend(["Epic Games", "Riot Games", "EA Games", "Origin Games", "GOG Galaxy\\Games", "Ubisoft\\Ubisoft Game Launcher\\games"].map(|d| pf.join(d)));
    }
    out
}

/// Every counted folder on `drive` with its category index.
fn roots(drive: char) -> Vec<(String, usize)> {
    use windows::Win32::UI::Shell::{FOLDERID_Documents, FOLDERID_Downloads, FOLDERID_Music, FOLDERID_Pictures, FOLDERID_Videos};
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    let mut v: Vec<(PathBuf, usize)> = game_roots(drive).into_iter().map(|p| (p, GAMES)).collect();
    v.extend([env("ProgramFiles"), env("ProgramFiles(x86)"), Some(crate::util::program_data()), env("LOCALAPPDATA").map(|p| p.join("Programs"))].into_iter().flatten().map(|p| (p, APPS)));
    v.push((crate::util::windows_dir(), WINDOWS));
    for (i, id) in [FOLDERID_Documents, FOLDERID_Pictures, FOLDERID_Videos, FOLDERID_Music, FOLDERID_Downloads].iter().enumerate() {
        v.extend(known_folder(id).map(|p| (p, 3 + i)));
    }
    let on_drive = format!("{}:\\", drive.to_ascii_lowercase());
    dedup(v.iter().map(|(p, c)| (key(p), *c)).filter(|(k, _)| k.starts_with(&on_drive)).collect())
}

// ---------------------------------------------------------------- the walk

/// A file whose data is not on this disk (a OneDrive placeholder).
const RECALL_ON_DATA_ACCESS: u32 = 0x0040_0000;

/// Bytes under `root`, not entering any folder in `stop` (the other roots) and never following a junction
/// or symbolic link. What cannot be read is left out.
fn walk(root: &Path, stop: &HashSet<&str>) -> u64 {
    let (mut sum, mut stack) = (0u64, vec![root.to_path_buf()]);
    while let Some(dir) = stack.pop() {
        if CANCEL.load(Relaxed) {
            break;
        }
        for e in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                let p = e.path();
                if !stop.contains(key(&p).as_str()) {
                    stack.push(p);
                }
            } else if let Ok(m) = e.metadata() {
                if m.file_attributes() & RECALL_ON_DATA_ACCESS == 0 {
                    sum += m.len();
                }
            }
        }
    }
    sum
}

/// Bytes per category under `roots`. Each root's walk stops at every other root, so a byte counts once,
/// for the deepest root it is in (a game inside Program Files is a game, not an app).
fn measure(roots: &[(String, usize)]) -> [u64; 8] {
    let stop: HashSet<&str> = roots.iter().map(|(k, _)| k.as_str()).collect();
    let mut cats = [0u64; 8];
    for (k, c) in roots {
        cats[*c] += walk(Path::new(k), &stop);
    }
    cats
}

/// Bytes per category on `drive`; `None` when cancelled.
fn scan(drive: char) -> Option<[u64; 8]> {
    let mut cats = measure(&roots(drive));
    // Windows' own big files in the drive's root (the folder listing gives their size; they cannot be opened)
    for e in fs::read_dir(format!("{drive}:\\")).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if ["pagefile.sys", "hiberfil.sys", "swapfile.sys"].contains(&name.as_str()) {
            cats[WINDOWS] += e.metadata().map_or(0, |m| m.len());
        }
    }
    (!CANCEL.load(Relaxed)).then_some(cats)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(list: &[(&str, usize)]) -> Vec<(String, usize)> {
        dedup(list.iter().map(|(p, c)| (key(Path::new(p)), *c)).collect())
    }

    /// A small tree in the temp folder: overlapping roots, each byte counted once for the deepest root.
    #[test]
    fn deepest_root_wins() {
        let t = std::env::temp_dir().join(format!("mytune-diskuse-{}", std::process::id()));
        let put = |rel: &str, bytes: usize| {
            let p = t.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, vec![0u8; bytes]).unwrap();
        };
        put(r"Program Files\Steam\steam.exe", 1);
        put(r"Program Files\Steam\steamapps\common\Game\game.exe", 10);
        put(r"Docs\a.txt", 100);
        put(r"Docs\Downloads\a.zip", 1000);
        put(r"Docs2\not-counted.txt", 10000); // a name that only starts like a root
        let at = |rel: &str| t.join(rel).to_string_lossy().to_string();
        let roots = r(&[
            (at(r"Program Files\Steam\STEAMAPPS").as_str(), GAMES),
            (at("Program Files").as_str(), APPS),
            (at("Docs").as_str(), 3),
            (at(r"Docs\Downloads\").as_str(), 7),
            (at("Missing").as_str(), 4),
        ]);
        let cats = measure(&roots);
        let _ = fs::remove_dir_all(&t);
        assert_eq!(cats, [10, 1, 0, 100, 0, 0, 0, 1000]);
    }

    #[test]
    fn one_folder_one_category() {
        // a game library that is also listed as an app folder stays a game folder
        let roots = r(&[(r"C:\Games", GAMES), (r"c:\games\", APPS), ("", APPS)]);
        assert_eq!(roots, vec![(r"c:\games".to_string(), GAMES)]);
    }

    #[test]
    fn other_is_the_rest_and_never_negative() {
        let mut u = Usage { drive: 'C', total: 1000, free: 400, ..Default::default() };
        assert_eq!(u.other(), 600);
        u.cats[GAMES] = 250;
        u.cats[WINDOWS] = 100;
        assert_eq!(u.other(), 250);
        u.cats[WINDOWS] = 900; // hard links counted twice: more than is used
        assert_eq!(u.other(), 0);
    }

    /// Walks the real system drive at background priority and prints what it found and how long it took.
    #[test]
    #[ignore]
    fn real_scan() {
        background(true);
        let began = Instant::now();
        let drive = system_drive();
        let cats = scan(drive).unwrap();
        let (total, free) = space(drive).unwrap();
        let u = Usage { drive, total, free, cats, at: 1 };
        for (n, b) in NAMES.iter().zip(cats) {
            println!("{n}: {:.1} GB", b as f64 / 1073741824.0);
        }
        println!("Other: {:.1} GB, used {:.1} GB, {:.0} s", u.other() as f64 / 1073741824.0, u.used() as f64 / 1073741824.0, began.elapsed().as_secs_f64());
        println!("{:?}", roots(drive));
    }
}
