//! `cust.searchstats`: a MyTune stats panel beside the Windows Start menu and Search panel. Nothing in Windows is
//! changed: the switch is `Settings.StatsBesideSearch`. With it off no thread or hook exists. With it on, one thread
//! sleeps in a message loop on a foreground WinEvent. On this Windows both Start and Search open with SearchHost's
//! `CoreWindow` in front, sized to the visible panel, while Start's own window covers the screen, so that rectangle
//! is the panel's bounds. Nothing is loaded into either process.

use crate::backup::TweakBackup;
use crate::engine::Tweak;
use crate::error::Result;
use crate::store::Store;
use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::Mutex;
use windows::core::{w, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::System::Threading::{GetCurrentProcessId, GetCurrentThreadId, OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::UI::Accessibility::{SetWinEventHook, UnhookWinEvent, HWINEVENTHOOK};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::*;

pub const ID: &str = "cust.searchstats";
const SETTING: &str = "StatsBesideSearch";
/// Title of the panel's window, which is how it is found (never just "MyTune": that is the main window).
pub const TITLE: &str = "MyTune stats";
/// Panel width and gap to Start or Search, in pixels at 100% display scale.
pub const WIDTH: i32 = 178;
const GAP: i32 = 8;
const HOSTS: [&str; 3] = ["searchhost.exe", "startmenuexperiencehost.exe", "searchapp.exe"];
const MEASURE_MS: u32 = 80;
const MEASURE_TRIES: u32 = 12;

/// A screen rectangle in physical pixels: left, top, right, bottom.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Px {
    pub l: i32,
    pub t: i32,
    pub r: i32,
    pub b: i32,
}

impl From<RECT> for Px {
    fn from(r: RECT) -> Px {
        Px { l: r.left, t: r.top, r: r.right, b: r.bottom }
    }
}

/// Right of `anchor` with the same top and height; left of it when the work area has no room; always inside it.
pub fn place(anchor: Px, work: Px, width: i32, gap: i32) -> Px {
    let (t, b) = (anchor.t.max(work.t), anchor.b.min(work.b));
    let right = anchor.r + gap;
    let l = if right + width <= work.r { right } else { anchor.l - gap - width };
    let l = l.min(work.r - width).max(work.l);
    Px { l, t, r: l + width, b }
}

/// Whether a window rectangle is the visible panel: on this monitor, not full-screen, and tall enough. Otherwise
/// nothing is shown rather than a panel in the wrong place.
pub fn is_panel(r: Px, monitor: Px) -> bool {
    r.l >= monitor.l && r.t >= monitor.t && r.r <= monitor.r && r.b <= monitor.b && r.r > r.l
        && (r.r - r.l) * 100 <= (monitor.r - monitor.l) * 85 && r.b - r.t >= 120
}

const DASH: &str = "--";

fn known<T: Into<f64> + Copy>(v: Option<T>) -> Option<f64> {
    v.map(Into::into).filter(|x: &f64| x.is_finite() && *x >= 0.0)
}

pub fn percent(v: Option<f32>) -> String {
    known(v).map_or_else(|| DASH.into(), |x| format!("{:.0}%", x.min(100.0)))
}

pub fn temp(v: Option<f32>) -> String {
    known(v).map_or_else(|| DASH.into(), |x| format!("{x:.0} \u{B0}C"))
}

pub fn gb(v: Option<f64>) -> String {
    known(v).map_or_else(|| DASH.into(), |x| format!("{x:.1} GB"))
}

/// Bytes a second as KB/s, MB/s or GB/s.
pub fn speed(v: Option<f64>) -> String {
    let Some(x) = known(v) else { return DASH.into() };
    const K: f64 = 1024.0;
    if x < K * K {
        format!("{:.0} KB/s", x / K)
    } else if x < K * K * K {
        format!("{:.1} MB/s", x / (K * K))
    } else {
        format!("{:.1} GB/s", x / (K * K * K))
    }
}

static ON: AtomicBool = AtomicBool::new(false);
static OPEN: AtomicBool = AtomicBool::new(false);
static THREAD: AtomicU32 = AtomicU32::new(0);
static PANEL: AtomicIsize = AtomicIsize::new(0);
/// Where the panel belongs while open, for a window created after Start opened.
static PLACED: Mutex<Option<Px>> = Mutex::new(None);
type Wake = Box<dyn Fn() + Send + Sync>;
static WAKE: Mutex<Option<Wake>> = Mutex::new(None);

fn wake() {
    if let Some(f) = &*WAKE.lock().unwrap_or_else(|p| p.into_inner()) {
        f();
    }
}

/// Asks the UI for a frame when the panel opens or closes.
pub fn set_wake(f: impl Fn() + Send + Sync + 'static) {
    *WAKE.lock().unwrap_or_else(|p| p.into_inner()) = Some(Box::new(f));
}

pub fn enabled() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Start or Search is open and the panel has a place beside it.
pub fn is_open() -> bool {
    OPEN.load(Ordering::Relaxed)
}

/// At start: what `Settings.StatsBesideSearch` in state.json says.
pub fn load(store: &Store) {
    set_on(store.settings_flag(SETTING) == Some(true));
}

fn set_on(on: bool) {
    if ON.swap(on, Ordering::SeqCst) == on {
        return;
    }
    if on {
        let _ = std::thread::Builder::new().name("search-stats".into()).spawn(run);
    } else {
        let t = THREAD.swap(0, Ordering::SeqCst);
        if t != 0 {
            let _ = unsafe { PostThreadMessageW(t, WM_QUIT, WPARAM(0), LPARAM(0)) };
        }
        hide();
        PANEL.store(0, Ordering::Relaxed); // egui drops the viewport now; switching on again makes a new window
    }
}

/// Registers the panel's window once it exists: never activated, not in Alt+Tab, click-through by its builder.
/// Returns false while the window is not there yet.
pub fn adopt_panel() -> bool {
    if PANEL.load(Ordering::Relaxed) != 0 {
        return true;
    }
    unsafe {
        let mut at = None;
        while let Ok(h) = FindWindowExW(None, at, w!("Window Class"), w!("MyTune stats")) {
            let mut pid = 0;
            GetWindowThreadProcessId(h, Some(&mut pid));
            if pid == GetCurrentProcessId() {
                let ex = GetWindowLongPtrW(h, GWL_EXSTYLE);
                SetWindowLongPtrW(h, GWL_EXSTYLE, ex | (WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_TRANSPARENT).0 as isize);
                PANEL.store(h.0 as isize, Ordering::Relaxed);
                if let Some(r) = *PLACED.lock().unwrap_or_else(|p| p.into_inner()) {
                    move_panel(r);
                }
                return true;
            }
            at = Some(h);
        }
    }
    false
}

fn panel() -> Option<HWND> {
    match PANEL.load(Ordering::Relaxed) {
        0 => None,
        h => Some(HWND(h as *mut _)),
    }
}

/// Async, so this thread never waits for a busy UI thread; `SWP_NOACTIVATE` keeps the focus on Start.
fn move_panel(r: Px) {
    if let Some(h) = panel() {
        let flags = SWP_NOACTIVATE | SWP_ASYNCWINDOWPOS | SWP_SHOWWINDOW;
        let _ = unsafe { SetWindowPos(h, Some(HWND_TOPMOST), r.l, r.t, r.r - r.l, r.b - r.t, flags) };
    }
}

fn hide() {
    *PLACED.lock().unwrap_or_else(|p| p.into_inner()) = None;
    let was = OPEN.swap(false, Ordering::SeqCst);
    if let Some(h) = panel() {
        let _ = unsafe { ShowWindowAsync(h, SW_HIDE) };
    }
    if was {
        wake();
    }
}

/// State of the watcher thread. Borrowed only briefly, because a callback can run during a Win32 call.
#[derive(Default)]
struct Watch {
    host: isize,
    pid: u32,
    scoped: Vec<HWINEVENTHOOK>,
    timer: usize,
    tries: u32,
    last: Option<Px>,
}

thread_local! {
    static WATCH: RefCell<Watch> = RefCell::new(Watch::default());
}

fn run() {
    unsafe {
        let me = GetCurrentThreadId();
        let mut msg = MSG::default();
        let _ = PeekMessageW(&mut msg, None, WM_USER, WM_USER, PM_NOREMOVE); // the queue exists before the id is public
        if THREAD.compare_exchange(0, me, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return;
        }
        if !ON.load(Ordering::SeqCst) {
            let _ = THREAD.compare_exchange(me, 0, Ordering::SeqCst, Ordering::SeqCst);
            return;
        }
        let hook = SetWinEventHook(EVENT_SYSTEM_FOREGROUND, EVENT_SYSTEM_FOREGROUND, None, Some(on_event), 0, 0, WINEVENT_OUTOFCONTEXT);
        refresh();
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            DispatchMessageW(&msg);
        }
        let _ = UnhookWinEvent(hook);
        forget_host();
        hide();
        let _ = THREAD.compare_exchange(me, 0, Ordering::SeqCst, Ordering::SeqCst);
    }
}

unsafe extern "system" fn on_event(_: HWINEVENTHOOK, event: u32, hwnd: HWND, object: i32, _: i32, _: u32, _: u32) {
    if event == EVENT_SYSTEM_FOREGROUND {
        refresh();
    } else if object == OBJID_WINDOW.0 && WATCH.with_borrow(|w| w.host) == hwnd.0 as isize {
        if event == EVENT_OBJECT_UNCLOAKED {
            if unsafe { GetForegroundWindow() } == hwnd {
                start_measuring();
            }
        } else {
            stop_timer();
            hide(); // the scoped hooks stay: Start can return without a foreground change
        }
    }
}

fn refresh() {
    let fg = unsafe { GetForegroundWindow() };
    let Some(pid) = host_pid(fg) else {
        forget_host();
        hide();
        return;
    };
    let same_pid = WATCH.with_borrow(|w| w.pid == pid && !w.scoped.is_empty());
    if !same_pid {
        forget_host();
        let scoped: Vec<_> = [(EVENT_OBJECT_HIDE, EVENT_OBJECT_HIDE), (EVENT_OBJECT_CLOAKED, EVENT_OBJECT_UNCLOAKED)]
            .into_iter()
            .map(|(a, b)| unsafe { SetWinEventHook(a, b, None, Some(on_event), pid, 0, WINEVENT_OUTOFCONTEXT) })
            .filter(|h| !h.is_invalid())
            .collect();
        WATCH.with_borrow_mut(|w| (w.pid, w.scoped) = (pid, scoped));
    }
    WATCH.with_borrow_mut(|w| w.host = fg.0 as isize);
    start_measuring();
}

fn forget_host() {
    stop_timer();
    let scoped = WATCH.with_borrow_mut(|w| {
        (w.host, w.pid) = (0, 0);
        std::mem::take(&mut w.scoped)
    });
    for h in scoped {
        let _ = unsafe { UnhookWinEvent(h) };
    }
}

fn start_measuring() {
    stop_timer();
    let id = unsafe { SetTimer(None, 0, MEASURE_MS, Some(on_timer)) };
    WATCH.with_borrow_mut(|w| (w.timer, w.tries, w.last) = (id, 0, None));
}

fn stop_timer() {
    let id = WATCH.with_borrow_mut(|w| std::mem::take(&mut w.timer));
    if id != 0 {
        let _ = unsafe { KillTimer(None, id) };
    }
}

unsafe extern "system" fn on_timer(_: HWND, _: u32, _: usize, _: u32) {
    let host = HWND(WATCH.with_borrow(|w| w.host) as *mut _);
    if host.is_invalid() {
        return stop_timer();
    }
    let found = measure(host);
    if WATCH.with_borrow(|w| w.host) != host.0 as isize {
        return; // closed or switched meanwhile
    }
    let (tries, last) = WATCH.with_borrow_mut(|w| {
        w.tries += 1;
        (w.tries, std::mem::replace(&mut w.last, found.map(|f| f.0)))
    });
    if let Some((anchor, work, dpi)) = found {
        let k = |v: i32| v * dpi as i32 / 96;
        let r = place(anchor, work, k(WIDTH), k(GAP));
        *PLACED.lock().unwrap_or_else(|p| p.into_inner()) = Some(r);
        move_panel(r);
        if !OPEN.swap(true, Ordering::SeqCst) {
            wake();
        }
        if last == Some(anchor) {
            stop_timer(); // two equal readings: the opening animation is over
        }
    }
    if tries >= MEASURE_TRIES {
        stop_timer();
        if found.is_none() {
            let mut r = RECT::default();
            let _ = unsafe { GetWindowRect(host, &mut r) };
            crate::log::info(&format!("search stats: no panel-sized window (foreground {:?}), panel not shown", Px::from(r)));
        }
    }
}

/// The panel's rectangle, the monitor's work area, and the display's DPI.
fn measure(host: HWND) -> Option<(Px, Px, u32)> {
    unsafe {
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        GetMonitorInfoW(MonitorFromWindow(host, MONITOR_DEFAULTTONEAREST), &mut info).ok().ok()?;
        let dpi = GetDpiForWindow(host).max(96);
        let monitor = Px::from(info.rcMonitor);
        let sized = |h: HWND| {
            let mut r = RECT::default();
            GetWindowRect(h, &mut r).ok().map(|_| Px::from(r)).filter(|r| is_panel(*r, monitor))
        };
        // Start's own window is full-screen: then the panel's size is on SearchHost's window.
        let anchor = sized(host).or_else(|| {
            let mut at = None;
            while let Ok(h) = FindWindowExW(None, at, w!("Windows.UI.Core.CoreWindow"), windows::core::PCWSTR::null()) {
                if is_search_host(h) {
                    return sized(h);
                }
                at = Some(h);
            }
            None
        })?;
        Some((anchor, info.rcWork.into(), dpi))
    }
}

fn is_search_host(hwnd: HWND) -> bool {
    image_name(hwnd).is_some_and(|(_, name)| name == "searchhost.exe" || name == "searchapp.exe")
}

/// The process id when `hwnd` belongs to Start or Search.
fn host_pid(hwnd: HWND) -> Option<u32> {
    image_name(hwnd).filter(|(_, name)| HOSTS.contains(&name.as_str())).map(|(pid, _)| pid)
}

/// The process id and lower-case exe name of `hwnd`.
fn image_name(hwnd: HWND) -> Option<(u32, String)> {
    if hwnd.is_invalid() {
        return None;
    }
    unsafe {
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 512];
        let mut n = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return None;
        }
        let path = String::from_utf16_lossy(&buf[..n as usize]).to_ascii_lowercase();
        Some((pid, path.rsplit('\\').next().unwrap_or("").to_string()))
    }
}

/// The Customize switch. Nothing in Windows changes, so the backup only marks it as applied.
pub struct SearchStatsTweak;

impl Tweak for SearchStatsTweak {
    fn id(&self) -> &str {
        ID
    }
    fn title(&self) -> &str {
        "Stats beside Search"
    }
    fn is_applied(&self) -> Result<bool> {
        Ok(enabled())
    }
    fn apply(&self, backup: &mut TweakBackup) -> Result<()> {
        backup.data.insert("enabled".into(), "1".into());
        set_on(true);
        Ok(())
    }
    fn revert(&self, _backup: &TweakBackup) -> Result<()> {
        set_on(false);
        Ok(())
    }
    fn revert_default(&self) -> Result<bool> {
        set_on(false);
        Ok(true)
    }
    fn on_applied(&self, store: &mut Store) -> Result<()> {
        store.set_settings_flag(SETTING, true)
    }
    fn on_reverted(&self, store: &mut Store) -> Result<()> {
        store.set_settings_flag(SETTING, false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WORK: Px = Px { l: 0, t: 0, r: 1920, b: 1032 };

    #[test]
    fn placement_right_left_and_clamped() {
        let a = Px { l: 600, t: 200, r: 1300, b: 1000 };
        assert_eq!(place(a, WORK, 178, 8), Px { l: 1308, t: 200, r: 1486, b: 1000 });
        let wide = Px { l: 1000, r: 1800, ..a };
        assert_eq!(place(wide, WORK, 178, 8), Px { l: 814, t: 200, r: 992, b: 1000 });
        let full = Px { l: 40, t: -5, r: 1900, b: 1100 };
        assert_eq!(place(full, WORK, 178, 8), Px { l: 0, t: 0, r: 178, b: 1032 });
    }

    #[test]
    fn only_a_panel_sized_window_counts() {
        let m = Px { l: 0, t: 0, r: 1920, b: 1080 };
        assert!(is_panel(Px { l: 531, t: 142, r: 1389, b: 1032 }, m), "measured SearchHost window");
        assert!(!is_panel(Px { l: 0, t: 0, r: 1920, b: 1032 }, m), "Start's full-screen window");
        assert!(!is_panel(Px { l: 0, t: 0, r: 0, b: 0 }, m));
        assert!(!is_panel(Px { l: 600, t: 29, r: 1200, b: 61 }, m), "too short");
        assert!(!is_panel(Px { l: -1400, t: 142, r: -500, b: 1032 }, m), "on another monitor");
    }

    #[test]
    fn missing_values_show_dashes() {
        assert_eq!((percent(None), temp(None), gb(None), speed(None)), ("--".into(), "--".into(), "--".into(), "--".into()));
        assert_eq!((percent(Some(f32::NAN)), speed(Some(-1.0))), ("--".into(), "--".into()));
        assert_eq!((percent(Some(41.6)), percent(Some(120.0)), temp(Some(61.2)), gb(Some(12.44))), ("42%".into(), "100%".into(), "61 \u{B0}C".into(), "12.4 GB".into()));
        assert_eq!((speed(Some(2048.0)), speed(Some(5.5 * 1048576.0)), speed(Some(2.0 * 1073741824.0))), ("2 KB/s".into(), "5.5 MB/s".into(), "2.0 GB/s".into()));
    }

    #[test]
    fn switch_reverts_cleanly() {
        let mut b = TweakBackup::default();
        SearchStatsTweak.apply(&mut b).unwrap();
        assert!(SearchStatsTweak.is_applied().unwrap() && !b.data.is_empty());
        SearchStatsTweak.revert(&b).unwrap();
        assert!(!SearchStatsTweak.is_applied().unwrap() && !is_open());
    }
}
