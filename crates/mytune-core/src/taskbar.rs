//! Taskbar look (clear, tinted or one colour), as TranslucentTB does it.
//!
//! On Windows 11 the taskbar paints its own XAML background, so the window-level call
//! (`SetWindowCompositionAttribute`) does nothing (measured on build 26200). Instead Windows' XAML
//! diagnostics load `taskbar/tap.cpp` (built by `build.rs`, embedded here) into explorer.exe, where it
//! swaps the background brush. This module tells that DLL which look to show through the shared memory
//! `Local\MyTuneTaskbar3` and counts a heartbeat; 3 s without one and the DLL shows the normal taskbar.
//!
//! The maximised look shows only on the taskbars of monitors that have a maximised window; Start and
//! Search looks show on every taskbar.

use crate::store::Store;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
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
}

impl Default for Config {
    fn default() -> Config {
        Config { on: false, desktop: Rule { look: Look::Normal, ..Rule::default() }, maximized: None, start: None, search: None }
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
    pad: i32,
    max_mons: [i64; 8],
}

/// The DLL's shared memory in one explorer.exe.
struct Link {
    pid: u32,
    map: HANDLE,
    view: *mut Shared,
    since: Instant,
}

impl Link {
    fn open(pid: u32) -> Option<Link> {
        unsafe {
            let map = OpenFileMappingW((FILE_MAP_READ | FILE_MAP_WRITE).0, false, w!("Local\\MyTuneTaskbar3")).ok()?;
            let view = MapViewOfFile(map, FILE_MAP_READ | FILE_MAP_WRITE, 0, 0, size_of::<Shared>()).Value as *mut Shared;
            if view.is_null() {
                let _ = CloseHandle(map);
                return None;
            }
            Some(Link { pid, map, view, since: Instant::now() })
        }
    }

    /// `max`: the look for taskbars on the monitors in `mons` (those with a maximised window).
    fn write(&self, look: (i32, i32), max: (i32, i32), mons: &[i64], beat: i32) {
        let n = mons.len().min(8);
        unsafe {
            let v = self.view;
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

impl Drop for Link {
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

/// The embedded DLL as a file explorer.exe can load: `%ProgramData%\MyTune\taskbar-<hash>.dll`.
/// The hash in the name lets a new version sit beside one that Explorer still holds open.
fn dll_path() -> Result<PathBuf, String> {
    let dir = PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into())).join("MyTune");
    let hash = DLL.iter().fold(0xcbf29ce484222325u64, |h, b| (h ^ *b as u64).wrapping_mul(0x100000001b3));
    let path = dir.join(format!("taskbar-{hash:016x}.dll"));
    if std::fs::read(&path).ok().as_deref() != Some(DLL) {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(&path, DLL).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    for old in std::fs::read_dir(&dir).into_iter().flatten().flatten().map(|e| e.path()) {
        let name = old.file_name().unwrap_or_default().to_string_lossy().into_owned();
        if old != path && name.starts_with("taskbar-") && name.ends_with(".dll") {
            let _ = std::fs::remove_file(old); // fails while an Explorer still has it loaded: fine
        }
    }
    Ok(path)
}

/// Has Windows load the DLL into explorer.exe `pid`.
fn inject(pid: u32) -> Result<(), String> {
    // Also in tap.cpp.
    const CLSID: GUID = GUID::from_u128(0x7a3b5c1e_9d2f_4e6a_b8c4_1f0e2d3c4b5a);
    type Init = unsafe extern "system" fn(PCWSTR, u32, PCWSTR, PCWSTR, GUID, PCWSTR) -> HRESULT;
    let path = HSTRING::from(dll_path()?.as_os_str());
    unsafe {
        let xaml = LoadLibraryW(w!("Windows.UI.Xaml.dll")).map_err(|e| e.to_string())?;
        let init = GetProcAddress(xaml, s!("InitializeXamlDiagnosticsEx")).ok_or("InitializeXamlDiagnosticsEx is missing")?;
        let init: Init = std::mem::transmute(init);
        for n in 1..=100 {
            let name = HSTRING::from(format!("VisualDiagConnection{n}"));
            let hr = init(PCWSTR(name.as_ptr()), pid, w!(""), PCWSTR(path.as_ptr()), CLSID, PCWSTR::null());
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
            if !now.on {
                if let Some(l) = link.take() {
                    l.write((0, 0), (0, 0), &[], beat.wrapping_add(1)); // normal taskbar at once, not after the 3 s
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
            if link.as_ref().is_some_and(|l| l.pid != pid) {
                link = None; // Explorer restarted
                if injected.is_some_and(|t| t.elapsed() < Duration::from_secs(30)) {
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
                link = Link::open(pid);
                if link.is_none() {
                    injected = Some(Instant::now());
                    match inject(pid) {
                        Ok(()) => {
                            for _ in 0..50 {
                                link = Link::open(pid);
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
                    set_problem(Some("Could not load the taskbar look into Windows Explorer. Trying again."));
                    std::thread::sleep(Duration::from_secs(5));
                    continue;
                }
            }
            let l = link.as_ref().unwrap();
            let state = state_now(&mut front);
            let mons = if state == State::Desktop { maximized_monitors() } else { Vec::new() };
            beat = beat.wrapping_add(1);
            l.write(now.rule(state).wire(), now.rule(State::Maximized).wire(), &mons, beat);
            if l.bars() > 0 {
                set_problem(None);
            } else if l.since.elapsed() > Duration::from_secs(5) {
                set_problem(Some("This Windows version's taskbar was not recognised, so the look cannot show."));
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    };
    let _ = std::thread::Builder::new().name("Taskbar".into()).spawn(run);
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
    fn config_reads_old_and_partial_json() {
        let c: Config = serde_json::from_str(r#"{"on":true,"desktop":{"look":"Tinted"}}"#).unwrap();
        assert!(c.on && c.desktop.look == Look::Tinted && c.desktop.opacity == 50 && c.start.is_none());
    }
}
