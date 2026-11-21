//! Taskbar look (clear, tinted or one colour), as TranslucentTB does it.
//!
//! On Windows 11 the taskbar paints its own XAML background, so the window-level call
//! (`SetWindowCompositionAttribute`) does nothing (measured on build 26200). Instead Windows' XAML
//! diagnostics load `taskbar/tap.cpp` (built by `build.rs`, embedded here) into explorer.exe, where it
//! swaps the background brush. This module tells that DLL which look to show through the shared memory
//! `Local\MyTuneTaskbar7` and counts a heartbeat; 3 s without one and the DLL shows the normal taskbar.
//! The same DLL hides Windows' own volume icon in the tray while `Config.hide_volume` (Sound page switch
//! "Use MyTune advanced sound settings"); the `HideSCAVolume` policy is ignored on build 26200.
//!
//! The maximised look shows only on the taskbars of monitors that have a maximised window; the Start and
//! Search looks only on the taskbar of the monitor the Start menu / Search flyout is on.
//!
//! The tweak `cust.clearstart` (`ClearStartTweak`) does the same for the Start menu: `taskbar/start.cpp`
//! goes into StartMenuExperienceHost.exe and SearchHost.exe (shared memory `Local\MyTuneStart1.<pid>` each)
//! and clears the panel's backgrounds. Its switch is `Settings.ClearStart` in state.json.

use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::{s, w, BOOL, GUID, HRESULT, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Memory::{MapViewOfFile, OpenFileMappingW, UnmapViewOfFile, FILE_MAP_READ, FILE_MAP_WRITE, MEMORY_MAPPED_VIEW_ADDRESS};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::WindowsAndMessaging::{EnumWindows, FindWindowW, GetForegroundWindow, GetWindowThreadProcessId, IsWindowVisible, IsZoomed};

#[derive(Clone, Copy, PartialEq, Debug, Default, Serialize, Deserialize)]
pub enum Look {
    Normal,
    #[default]
    Clear,
    Tinted,
    Solid,
}

impl Look {
    pub const ALL: [Look; 4] = [Look::Normal, Look::Clear, Look::Tinted, Look::Solid];
    pub fn name(self) -> &'static str {
        match self {
            Look::Normal => "Normal",
            Look::Clear => "Clear",
            Look::Tinted => "Tinted",
            Look::Solid => "One colour",
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Rule {
    pub look: Look,
    pub color: [u8; 3],
    /// Percent, used by `Look::Tinted`.
    pub opacity: u8,
}

impl Default for Rule {
    fn default() -> Rule {
        Rule { look: Look::Clear, color: [0, 0, 0], opacity: 50 }
    }
}

impl Rule {
    /// (look, ARGB) as the DLL reads them: look 0 is the normal taskbar, 1 a colour (alpha 0 = clear).
    pub fn wire(&self) -> (i32, i32) {
        let alpha = match self.look {
            Look::Normal => return (0, 0),
            Look::Clear => 0,
            Look::Tinted => self.opacity.min(100) as u32 * 255 / 100,
            Look::Solid => 255,
        };
        let [r, g, b] = self.color.map(u32::from);
        (1, (alpha << 24 | r << 16 | g << 8 | b) as i32)
    }
}

/// What the taskbar looks like and when. `None` = that situation uses the `desktop` look.
/// `on` has no switch of its own: the card sets it to `wanted()`.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub on: bool,
    pub desktop: Rule,
    pub maximized: Option<Rule>,
    pub start: Option<Rule>,
    pub search: Option<Rule>,
    /// Windows' own volume icon is hidden from the tray (the DLL is loaded for this too).
    pub hide_volume: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config { on: false, desktop: Rule { look: Look::Normal, ..Rule::default() }, maximized: None, start: None, search: None, hide_volume: false }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum State {
    Desktop,
    Maximized,
    Start,
    Search,
}

impl Config {
    /// Some situation asks for a look that is not the normal taskbar, so the DLL is needed.
    pub fn wanted(&self) -> bool {
        [Some(&self.desktop), self.maximized.as_ref(), self.start.as_ref(), self.search.as_ref()].into_iter().flatten().any(|r| r.look != Look::Normal)
    }

    pub fn rule(&self, state: State) -> &Rule {
        match state {
            State::Desktop => None,
            State::Maximized => self.maximized.as_ref(),
            State::Start => self.start.as_ref(),
            State::Search => self.search.as_ref(),
        }
        .unwrap_or(&self.desktop)
    }
}

/// `Settings.Taskbar` in state.json.
pub fn load(store: &Store) -> Config {
    store.top("Settings").and_then(|s| s.get("Taskbar")).and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

pub fn save(store: &mut Store, config: &Config) -> crate::Result<()> {
    store.set_settings_value("Taskbar", serde_json::to_value(config).unwrap_or_default());
    store.save()
}

/// Removes the per-user `HideSCAVolume` policy an older MyTune set for the same switch: build 26200
/// ignores it, and on a Windows that honours it the icon would stay hidden without MyTune.
pub fn clear_volume_icon_policy() {
    use crate::reg::{self, Hive};
    const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Policies\Explorer";
    if matches!(reg::get(Hive::Hkcu, KEY, "HideSCAVolume"), Ok(Some(_))) && reg::delete_value(Hive::Hkcu, KEY, "HideSCAVolume").is_ok() {
        use crate::catalog::{Hooks, SystemHooks};
        SystemHooks.broadcast_setting_change();
    }
}

static PROBLEM: Mutex<Option<String>> = Mutex::new(None);

/// Why the look is not showing, for the card.
pub fn problem() -> Option<String> {
    PROBLEM.lock().unwrap_or_else(|p| p.into_inner()).clone()
}

fn set_problem(text: Option<&str>) {
    *PROBLEM.lock().unwrap_or_else(|p| p.into_inner()) = text.map(String::from);
}

/// Must match `Shared` in tap.cpp. A changed layout needs a new mapping name in both files.
#[repr(C)]
struct Shared {
    beat: i32,
    look: i32,
    argb: i32,
    bars: i32,
    max_look: i32,
    max_argb: i32,
    max_count: i32,
    hide_volume: i32,
    max_mons: [i64; 8],
    start_mon: i64,
    start_look: i32,
    start_argb: i32,
    outline_select: i32,
    active: i32,
}

/// Must match `Shared` in start.cpp. A changed layout needs a new mapping name in both files.
#[repr(C)]
struct StartShared {
    beat: i32,
    on: i32,
    panels: i32,
}

const TASKBAR_MAP: PCWSTR = w!("Local\\MyTuneTaskbar7");
/// The processes start.cpp goes into, each with the prefix of the name MyTune opens its shared memory by
/// (the process id follows). SearchHost is an app container: its `Local\` names sit under the container's
/// SID, which Windows derives from the package name `MicrosoftWindows.Client.CBS_cw5n1h2txyewy` (the same on
/// every PC). A name missing there is looked up in the session's names, hence the process id in the name.
const START_HOSTS: [(&str, &str); 2] = [
    ("StartMenuExperienceHost", "Local\\MyTuneStart1"),
    ("SearchHost", "AppContainerNamedObjects\\S-1-15-2-283421221-3183566570-1718213290-751554359-3541592344-2312209569-3374928651\\MyTuneStart1"),
];

/// A DLL's shared memory (`Shared` or `StartShared`) in the process `pid`.
struct Link<T = Shared> {
    pid: u32,
    map: HANDLE,
    view: *mut T,
    since: Instant,
}

impl<T> Link<T> {
    fn open(pid: u32, name: PCWSTR) -> Option<Link<T>> {
        unsafe {
            let map = OpenFileMappingW((FILE_MAP_READ | FILE_MAP_WRITE).0, false, name).ok()?;
            let view = MapViewOfFile(map, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, size_of::<T>()).Value as *mut T;
            if view.is_null() {
                let _ = CloseHandle(map);
                return None;
            }
            Some(Link { pid, map, view, since: Instant::now() })
        }
    }
}

impl Link<StartShared> {
    fn write(&self, on: bool, beat: i32) {
        unsafe {
            (&raw mut (*self.view).on).write_volatile(on as i32);
            (&raw mut (*self.view).beat).write_volatile(beat);
        }
    }

    fn panels(&self) -> i32 {
        unsafe { (&raw const (*self.view).panels).read_volatile() }
    }
}

impl Link<Shared> {
    /// `max`: the look for taskbars on the monitors in `mons` (those with a maximised window).
    fn write(&self, w: &Wire, hide_volume: bool, outline: bool, beat: i32) {
        let (look, max, mons) = (w.base, w.max, &w.mons[..]);
        let n = mons.len().min(8);
        unsafe {
            let v = self.view;
            (&raw mut (*v).start_mon).write_volatile(w.start_mon);
            (&raw mut (*v).start_look).write_volatile(w.start.0);
            (&raw mut (*v).start_argb).write_volatile(w.start.1);
            (&raw mut (*v).hide_volume).write_volatile(hide_volume as i32);
            (&raw mut (*v).outline_select).write_volatile(outline as i32);
            (&raw mut (*v).active).write_volatile((w.active || hide_volume) as i32);
            for (i, m) in mons[..n].iter().enumerate() {
                (&raw mut (*v).max_mons[i]).write_volatile(*m);
            }
            (&raw mut (*v).max_count).write_volatile(n as i32);
            (&raw mut (*v).max_look).write_volatile(max.0);
            (&raw mut (*v).max_argb).write_volatile(max.1);
            (&raw mut (*v).look).write_volatile(look.0);
            (&raw mut (*v).argb).write_volatile(look.1);
            (&raw mut (*v).beat).write_volatile(beat);
        }
    }

    fn bars(&self) -> i32 {
        unsafe { (&raw const (*self.view).bars).read_volatile() }
    }
}

impl<T> Drop for Link<T> {
    fn drop(&mut self) {
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.view.cast() });
            let _ = CloseHandle(self.map);
        }
    }
}

fn tray() -> Option<HWND> {
    unsafe { FindWindowW(w!("Shell_TrayWnd"), PCWSTR::null()).ok().filter(|h| !h.is_invalid()) }
}

fn pid_of(window: HWND) -> u32 {
    let mut pid = 0;
    unsafe { GetWindowThreadProcessId(window, Some(&mut pid)) };
    pid
}

static DLL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/mytune-taskbar.dll"));
static START_DLL: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/mytune-start.dll"));
// Also in tap.cpp and start.cpp.
const CLSID: GUID = GUID::from_u128(0x7a3b5c1e_9d2f_4e6a_b8c4_1f0e2d3c4b5a);
const START_CLSID: GUID = GUID::from_u128(0x7a3b5c1e_9d2f_4e6a_b8c4_1f0e2d3c4b5c);

/// Loads of one DLL allowed within `LOOP_WINDOW` before its feature turns itself off.
const LOOP_LOADS: usize = 2;
const LOOP_WINDOW: u64 = 120;

/// Crash-loop guard for every DLL MyTune loads into another process. A target that keeps dying is loaded
/// again after each restart, so a third successful load of the same feature within 2 minutes means a loop:
/// `may_load` refuses it. The load times are kept on disk (`%ProgramData%\MyTune\loads-<feature>.txt`),
/// so the count survives MyTune restarts (a restart of MyTune once reset the old in-memory count and
/// Explorer crashed every 4 s, 2026-11-21). Only loads that went in are counted (`note_load`): a load
/// Windows refuses is retried every 5 s and is no crash.
fn loads_file(feature: &str) -> PathBuf {
    PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into())).join("MyTune").join(format!("loads-{feature}.txt"))
}

fn recent_loads(feature: &str) -> Vec<u64> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    std::fs::read_to_string(loads_file(feature)).unwrap_or_default().lines().filter_map(|l| l.trim().parse().ok()).filter(|t: &u64| now.saturating_sub(*t) < LOOP_WINDOW).collect()
}

fn may_load(feature: &str) -> bool {
    recent_loads(feature).len() < LOOP_LOADS
}

/// Records a load that went in. Returns false when it cannot be recorded (then the caller stops: no
/// record, no protection).
fn note_load(feature: &str) -> bool {
    let mut times = recent_loads(feature);
    times.push(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0));
    let path = loads_file(feature);
    let text: String = times.iter().map(|t| format!("{t}\n")).collect();
    path.parent().is_some_and(|d| std::fs::create_dir_all(d).is_ok()) && std::fs::write(&path, text).is_ok()
}

/// Forgets the loads of a feature, so turning it on again after a give-up starts a fresh count.
fn forget_loads(feature: &str) {
    let _ = std::fs::remove_file(loads_file(feature));
}

/// An embedded DLL as a file its target can load: `%ProgramData%\MyTune\<prefix>-<hash>.dll`
/// (`taskbar` for explorer.exe, `startmenu` for Start).
/// The hash in the name lets a new version sit beside one that the target still holds open.
fn dll_path(dll: &[u8], prefix: &str) -> Result<PathBuf, String> {
    let dir = PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into())).join("MyTune");
    let hash = dll.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3));
    let path = dir.join(format!("{prefix}-{hash:016x}.dll"));
    if std::fs::read(&path).ok().as_deref() != Some(dll) {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(&path, dll).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    for old in std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()) {
        let name = old.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if old != path && name.starts_with(&format!("{prefix}-")) && name.ends_with(".dll") {
            let _ = std::fs::remove_file(old); // fails while an Explorer still has it loaded: fine
        }
    }
    Ok(path)
}

/// Has Windows load the DLL at `path` into the XAML process `pid`.
fn inject(pid: u32, path: PathBuf, clsid: GUID) -> Result<(), String> {
    type Init = unsafe extern "system" fn(PCWSTR, u32, PCWSTR, PCWSTR, GUID, PCWSTR) -> HRESULT;
    let path = HSTRING::from(path.as_os_str());
    unsafe {
        let xaml = LoadLibraryW(w!("Windows.UI.Xaml.dll")).map_err(|e| e.to_string())?;
        let init = GetProcAddress(xaml, s!("InitializeXamlDiagnosticsEx")).ok_or("InitializeXamlDiagnosticsEx is missing")?;
        let init: Init = std::mem::transmute(init);
        for n in 1..=100 {
            let name = HSTRING::from(format!("VisualDiagConnection{n}"));
            let hr = init(PCWSTR(name.as_ptr()), pid, w!(""), PCWSTR(path.as_ptr()), clsid, PCWSTR::null());
            // ERROR_NOT_FOUND: this connection name is taken, try the next
            if hr != HRESULT(0x80070490u32 as i32) {
                return hr.ok().map_err(|e| e.to_string());
            }
        }
    }
    Err("no free XAML diagnostics connection".into())
}

fn exe_name(window: HWND) -> String {
    let mut buf = [0u16; 260];
    let mut len = buf.len() as u32;
    unsafe {
        let Ok(process) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid_of(window)) else { return String::new() };
        let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(process);
        if !ok {
            return String::new();
        }
    }
    let path = String::from_utf16_lossy(&buf[..len as usize]).to_lowercase();
    path.rsplit('\\').next().unwrap_or_default().to_string()
}

/// The monitors (HMONITOR values, the same in every process) that show a visible maximised window.
fn maximized_monitors() -> Vec<i64> {
    unsafe extern "system" fn each(window: HWND, found: LPARAM) -> BOOL {
        let found = &mut *(found.0 as *mut Vec<i64>);
        if IsWindowVisible(window).as_bool() && IsZoomed(window).as_bool() {
            // windows on other virtual desktops are "visible" but cloaked
            let mut cloaked = 0u32;
            let _ = DwmGetWindowAttribute(window, DWMWA_CLOAKED, (&raw mut cloaked).cast(), 4);
            let mon = MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST).0 as i64;
            if cloaked == 0 && !found.contains(&mon) {
                found.push(mon);
            }
        }
        true.into()
    }
    let mut found = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&raw mut found as isize));
    }
    found
}

/// What the DLL gets: the desktop look, the maximised look for `mons`, and the Start / Search look for
/// the taskbar of `start_mon` only (0 = none).
#[derive(Default, Debug, PartialEq)]
struct Wire {
    base: (i32, i32),
    max: (i32, i32),
    mons: Vec<i64>,
    start: (i32, i32),
    start_mon: i64,
    /// A taskbar look is on (even one that only shows with Start open): the DLL stays at 100 ms.
    active: bool,
}

/// `mon` is the monitor of the Start menu / Search window when `state` is Start or Search.
fn wires(config: &Config, state: State, mon: i64, maximised: Vec<i64>) -> Wire {
    let on_start = matches!(state, State::Start | State::Search) && mon != 0;
    Wire {
        base: config.rule(State::Desktop).wire(),
        max: config.rule(State::Maximized).wire(),
        mons: maximised,
        start: if on_start { config.rule(state).wire() } else { (0, 0) },
        start_mon: if on_start { mon } else { 0 },
        active: true,
    }
}

/// Which rule applies now on every taskbar (Desktop, Start or Search; maximised is per monitor).
/// `front` remembers the foreground window's answer, so the process name is looked up only when the
/// foreground window changes.
fn state_now(front: &mut (isize, State)) -> State {
    let window = unsafe { GetForegroundWindow() };
    if window.0 as isize != front.0 {
        let state = match exe_name(window).as_str() {
            "startmenuexperiencehost.exe" => State::Start,
            "searchhost.exe" | "searchapp.exe" => State::Search,
            _ => State::Desktop,
        };
        *front = (window.0 as isize, state);
    }
    front.1
}

/// Starts the thread that keeps the look applied while `config.on`. It re-injects after Explorer
/// restarts. If Explorer dies twice shortly after an injection, it resets `config` to all Normal and calls
/// `gave_up` (the caller saves the config), so a broken Windows update cannot loop-crash the desktop.
pub fn start(config: Arc<Mutex<Config>>, gave_up: impl Fn() + Send + 'static) {
    let run = move || {
        let mut link: Option<Link> = None;
        let mut injected: Option<Instant> = None;
        let mut strikes = 0;
        let mut beat = 0i32;
        let mut front = (0isize, State::Desktop);
        loop {
            let now = config.lock().unwrap_or_else(|p| p.into_inner()).clone();
            let outline = OUTLINE_SELECT.load(Ordering::Relaxed);
            if !now.on && !now.hide_volume && !outline {
                if let Some(l) = link.take() {
                    l.write(&Wire::default(), false, false, beat.wrapping_add(1)); // normal taskbar at once, not after the 3 s
                }
                strikes = 0;
                std::thread::sleep(Duration::from_millis(500));
                continue;
            }
            let Some(tray) = tray() else {
                link = None;
                std::thread::sleep(Duration::from_secs(1));
                continue;
            };
            let pid = pid_of(tray);
            // MyTune's own restart (ten tweaks do one) is no crash: neither a strike nor a counted load
            let planned = crate::process::explorer_restart_planned();
            if link.as_ref().is_some_and(|l| l.pid != pid) {
                link = None; // Explorer restarted
                if !planned && injected.is_some_and(|t| t.elapsed() < Duration::from_secs(30)) {
                    strikes += 1;
                }
            }
            if strikes >= 2 {
                strikes = 0;
                *config.lock().unwrap_or_else(|p| p.into_inner()) = Config::default(); // every look back to Normal
                set_problem(Some("Turned off: Windows Explorer restarted twice right after the taskbar look was loaded."));
                crate::log::error("Taskbar look turned off: Explorer restarted twice after loading it", None);
                gave_up();
                continue;
            }
            if link.is_none() {
                link = Link::open(pid, TASKBAR_MAP);
                if link.is_none() && !planned && !may_load("taskbar") {
                    forget_loads("taskbar");
                    *config.lock().unwrap_or_else(|p| p.into_inner()) = Config::default(); // every look back to Normal
                    set_problem(Some("Turned off: Windows Explorer kept restarting after the taskbar look was loaded."));
                    crate::log::error("Taskbar look turned off: loaded 3 times within 2 minutes (Explorer kept restarting)", None);
                    gave_up();
                    continue;
                }
                if link.is_none() {
                    injected = Some(Instant::now());
                    match dll_path(DLL, "taskbar").and_then(|p| inject(pid, p, CLSID)) {
                        Ok(()) => {
                            if !planned && !note_load("taskbar") {
                                crate::log::error("Taskbar look: could not record the load (crash-loop guard)", None);
                            }
                            for _ in 0..50 {
                                link = Link::open(pid, TASKBAR_MAP);
                                if link.is_some() {
                                    break;
                                }
                                std::thread::sleep(Duration::from_millis(100));
                            }
                        }
                        Err(e) => crate::log::error("Taskbar look: could not load into Explorer", Some(&e)),
                    }
                }
                if link.is_none() {
                    OUTLINE_FAILED.store(true, Ordering::Relaxed);
                    set_problem(Some("Could not load the taskbar look into Windows Explorer. Trying again."));
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                }
            }
            OUTLINE_FAILED.store(false, Ordering::Relaxed);
            let l = link.as_ref().unwrap();
            beat = beat.wrapping_add(1);
            if now.on {
                let state = state_now(&mut front);
                let mon = if state == State::Desktop { 0 } else { unsafe { MonitorFromWindow(GetForegroundWindow(), MONITOR_DEFAULTTONEAREST).0 as i64 } };
                l.write(&wires(&now, state, mon, maximized_monitors()), now.hide_volume, outline, beat);
            } else {
                l.write(&Wire::default(), now.hide_volume, outline, beat); // no taskbar look: only the volume icon and the selection box
            }
            if l.bars() > 0 {
                set_problem(None);
            } else if l.since.elapsed() > Duration::from_secs(5) {
                set_problem(Some("This Windows version's taskbar was not recognised, so the look cannot show."));
            }
            // Only the selection box on: a heartbeat a second is enough, also during games.
            std::thread::sleep(Duration::from_millis(if now.on || now.hide_volume { 100 } else { 1000 }));
        }
    };
    let _ = std::thread::Builder::new().name("Taskbar".into()).spawn(run);
}

/// Whether the Start menu is to be clear. `ClearStartTweak` switches it, the `start_menu` threads follow it.
static START_CLEAR: AtomicBool = AtomicBool::new(false);
/// The last load into the Start menu's process failed, so the switch must not read on.
static START_FAILED: AtomicBool = AtomicBool::new(false);

/// At start: what `Settings.ClearStart` in state.json says.
pub fn set_start_clear(on: bool) {
    START_CLEAR.store(on, Ordering::Relaxed);
    START_FAILED.store(false, Ordering::Relaxed);
}

/// `cust.clearstart`: the Start menu panel and the search panel are see-through, only the border shows.
/// Nothing in Windows is changed for good, so there is nothing to back up: the switch is the
/// `Settings.ClearStart` flag, and the look lasts only while MyTune runs.
pub struct ClearStartTweak;

impl crate::engine::Tweak for ClearStartTweak {
    fn id(&self) -> &str {
        "cust.clearstart"
    }
    fn title(&self) -> &str {
        "Transparent Start menu"
    }
    fn is_applied(&self) -> crate::Result<bool> {
        Ok(START_CLEAR.load(Ordering::Relaxed) && !START_FAILED.load(Ordering::Relaxed))
    }
    fn apply(&self, backup: &mut crate::backup::TweakBackup) -> crate::Result<()> {
        backup.data.insert("enabled".into(), "1".into());
        set_start_clear(true);
        Ok(())
    }
    fn revert(&self, _backup: &crate::backup::TweakBackup) -> crate::Result<()> {
        set_start_clear(false);
        Ok(())
    }
    fn revert_default(&self) -> crate::Result<bool> {
        set_start_clear(false);
        Ok(true)
    }
    fn on_applied(&self, store: &mut Store) -> crate::Result<()> {
        store.set_settings_flag("ClearStart", true)
    }
    fn on_reverted(&self, store: &mut Store) -> crate::Result<()> {
        store.set_settings_flag("ClearStart", false)
    }
}

/// Whether the drag-selection box is outline only. `OutlineSelectTweak` switches it, the `start` thread sends it.
static OUTLINE_SELECT: AtomicBool = AtomicBool::new(false);
/// The last load into Explorer failed, so the switch must not read on.
static OUTLINE_FAILED: AtomicBool = AtomicBool::new(false);

/// At start: what `Settings.OutlineSelect` in state.json says.
pub fn set_outline_select(on: bool) {
    OUTLINE_SELECT.store(on, Ordering::Relaxed);
    OUTLINE_FAILED.store(false, Ordering::Relaxed);
}

/// `cust.outlineselect`: tap.cpp skips comctl32's blue fill of the drag-selection box, Windows' own 1 px
/// outline stays. Nothing in Windows is changed for good: the switch is the `Settings.OutlineSelect` flag.
pub struct OutlineSelectTweak;

impl crate::engine::Tweak for OutlineSelectTweak {
    fn id(&self) -> &str {
        "cust.outlineselect"
    }
    fn title(&self) -> &str {
        "Outline-only selection"
    }
    fn is_applied(&self) -> crate::Result<bool> {
        Ok(OUTLINE_SELECT.load(Ordering::Relaxed) && !OUTLINE_FAILED.load(Ordering::Relaxed))
    }
    fn apply(&self, backup: &mut crate::backup::TweakBackup) -> crate::Result<()> {
        backup.data.insert("enabled".into(), "1".into());
        set_outline_select(true);
        Ok(())
    }
    fn revert(&self, _backup: &crate::backup::TweakBackup) -> crate::Result<()> {
        set_outline_select(false);
        Ok(())
    }
    fn revert_default(&self) -> crate::Result<bool> {
        set_outline_select(false);
        Ok(true)
    }
    fn on_applied(&self, store: &mut Store) -> crate::Result<()> {
        store.set_settings_flag("OutlineSelect", true)
    }
    fn on_reverted(&self, store: &mut Store) -> crate::Result<()> {
        store.set_settings_flag("OutlineSelect", false)
    }
}

fn pid_named(name: &str) -> Option<u32> {
    use crate::procs::{Procs, WinProcs};
    WinProcs.list().ok()?.into_iter().find(|p| p.name.eq_ignore_ascii_case(name)).map(|p| p.pid)
}

/// Starts the threads (one per process in `START_HOSTS`) that keep the Start menu clear while
/// `cust.clearstart` is on. Each loads start.cpp again after its process restarts; if that process dies
/// twice shortly after a load, it calls `gave_up`, which is to revert the tweak.
pub fn start_menu(gave_up: impl Fn() + Send + Sync + 'static) {
    let gave_up = Arc::new(gave_up);
    for (host, map) in START_HOSTS {
        // SearchHost only draws the search panel: the Start menu is clear without it
        let is_start = host == START_HOSTS[0].0;
        let gave_up = gave_up.clone();
        let open = move |pid: u32| Link::<StartShared>::open(pid, PCWSTR(HSTRING::from(format!("{map}.{pid}")).as_ptr()));
        let run = move || {
            let mut link: Option<Link<StartShared>> = None;
            let mut injected: Option<Instant> = None;
            let (mut strikes, mut beat, mut tick, mut told) = (0, 0i32, 0u32, false);
            loop {
                if !START_CLEAR.load(Ordering::Relaxed) {
                    if let Some(l) = link.take() {
                        l.write(false, beat.wrapping_add(1)); // normal Start menu at once, not after the 3 s
                    }
                    strikes = 0;
                    std::thread::sleep(Duration::from_millis(500));
                    continue;
                }
                // The process list is read every 2 s, and each round while there is no link.
                if link.is_none() || tick % 20 == 0 {
                    let pid = pid_named(host);
                    if link.as_ref().is_some_and(|l| Some(l.pid) != pid) {
                        link = None; // the process restarted
                        if injected.is_some_and(|t| t.elapsed() < Duration::from_secs(30)) {
                            strikes += 1;
                        }
                    }
                    if strikes >= 2 {
                        strikes = 0;
                        crate::log::error("Transparent Start menu turned off: its process restarted twice after loading it", Some(&host));
                        set_start_clear(false); // also when the revert below fails
                        gave_up();
                        continue;
                    }
                    if link.is_none() {
                        let Some(pid) = pid else {
                            std::thread::sleep(Duration::from_secs(1));
                            continue;
                        };
                        link = open(pid);
                        if link.is_none() && !may_load(host) {
                            forget_loads(host);
                            crate::log::error("Transparent Start menu turned off: loaded 3 times within 2 minutes (its process kept restarting)", Some(&host));
                            set_start_clear(false); // also when the revert below fails
                            gave_up();
                            continue;
                        }
                        if link.is_none() {
                            injected = Some(Instant::now());
                            // SearchHost is an app container: it loads only files that "ALL APPLICATION PACKAGES"
                            // (S-1-15-2-1) may read. Without this the load fails with no error anywhere.
                            let granted = |p: PathBuf| {
                                let _ = crate::process::run("icacls", &[&p.to_string_lossy(), "/grant", "*S-1-15-2-1:(RX)"]);
                                p
                            };
                            match dll_path(START_DLL, "startmenu").map(granted).and_then(|p| inject(pid, p, START_CLSID)) {
                                Ok(()) => {
                                    if !note_load(host) {
                                        crate::log::error("Transparent Start menu: could not record the load (crash-loop guard)", Some(&host));
                                    }
                                    for _ in 0..50 {
                                        link = open(pid);
                                        if link.is_some() {
                                            break;
                                        }
                                        std::thread::sleep(Duration::from_millis(100));
                                    }
                                }
                                Err(e) => crate::log::error(&format!("Transparent Start menu: could not load into {host}"), Some(&e)),
                            }
                        }
                        if is_start {
                            START_FAILED.store(link.is_none(), Ordering::Relaxed);
                        }
                        if link.is_none() {
                            std::thread::sleep(Duration::from_secs(5));
                            continue;
                        }
                    }
                }
                let l = link.as_ref().unwrap();
                tick = tick.wrapping_add(1);
                beat = beat.wrapping_add(1);
                l.write(true, beat);
                if !told && l.panels() == 0 && l.since.elapsed() > Duration::from_secs(5) {
                    told = true;
                    crate::log::error("Transparent Start menu: this Windows version's panel was not recognised", Some(&host));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        };
        let _ = std::thread::Builder::new().name(format!("Start menu ({host})")).spawn(run);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_values() {
        let rule = |look, opacity| Rule { look, color: [0x12, 0x34, 0x56], opacity };
        assert_eq!(rule(Look::Normal, 50).wire(), (0, 0));
        assert_eq!(rule(Look::Clear, 50).wire(), (1, 0x00123456));
        assert_eq!(rule(Look::Tinted, 50).wire(), (1, 0x7F123456));
        assert_eq!(rule(Look::Tinted, 200).wire(), (1, 0xFF123456u32 as i32));
        assert_eq!(rule(Look::Solid, 0).wire(), (1, 0xFF123456u32 as i32));
    }

    #[test]
    fn start_look_only_on_its_monitor() {
        let mut c = Config::default();
        c.desktop = Rule { look: Look::Solid, ..Rule::default() };
        c.start = Some(Rule { look: Look::Clear, ..Rule::default() });
        let w = wires(&c, State::Start, 7, vec![9]);
        assert_eq!((w.start_mon, w.start, w.mons), (7, (1, 0x00_000000), vec![9]));
        assert_eq!(w.base, c.desktop.wire()); // other monitors keep the desktop / maximised look
        let w = wires(&c, State::Desktop, 0, vec![]);
        assert_eq!((w.start_mon, w.start), (0, (0, 0)));
    }

    #[test]
    fn unset_rules_use_the_desktop_look() {
        let mut c = Config::default();
        assert_eq!(c.rule(State::Start), &c.desktop);
        assert!(!c.wanted());
        c.maximized = Some(Rule { look: Look::Solid, ..Rule::default() });
        assert_eq!(c.rule(State::Maximized).look, Look::Solid);
        assert_eq!(c.rule(State::Search).look, Look::Normal);
        assert!(c.wanted());
    }

    #[test]
    fn clear_start_tweak_follows_its_switch() {
        use crate::engine::Tweak;
        let mut b = crate::backup::TweakBackup::default();
        ClearStartTweak.apply(&mut b).unwrap();
        assert!(ClearStartTweak.is_applied().unwrap() && !b.data.is_empty());
        START_FAILED.store(true, Ordering::Relaxed); // the helper did not load
        assert!(!ClearStartTweak.is_applied().unwrap());
        ClearStartTweak.apply(&mut b).unwrap(); // switching on again retries
        assert!(ClearStartTweak.is_applied().unwrap());
        ClearStartTweak.revert(&b).unwrap();
        assert!(!ClearStartTweak.is_applied().unwrap());
    }

    #[test]
    fn outline_select_tweak_follows_its_switch() {
        use crate::engine::Tweak;
        let mut b = crate::backup::TweakBackup::default();
        OutlineSelectTweak.apply(&mut b).unwrap();
        assert!(OutlineSelectTweak.is_applied().unwrap() && !b.data.is_empty());
        OUTLINE_FAILED.store(true, Ordering::Relaxed); // the helper did not load
        assert!(!OutlineSelectTweak.is_applied().unwrap());
        OutlineSelectTweak.apply(&mut b).unwrap(); // switching on again retries
        assert!(OutlineSelectTweak.is_applied().unwrap());
        OutlineSelectTweak.revert(&b).unwrap();
        assert!(!OutlineSelectTweak.is_applied().unwrap());
    }

    #[test]
    fn config_reads_old_and_partial_json() {
        let c: Config = serde_json::from_str(r#"{"on":true,"desktop":{"look":"Tinted"}}"#).unwrap();
        assert!(c.on && c.desktop.look == Look::Tinted && c.desktop.opacity == 50 && c.start.is_none());
    }
}
