//! Mouse page (port of `Tweaks/Customize.cs` MouseToggles and `Services/MouseRate.cs` + `MouseRateVM`).
//!
//! - `acceleration_on` / `set_acceleration`: the "Enhance pointer precision" switch, applied live with SPI_SETMOUSE.
//!   Nothing to back up: off is simply the other state.
//! - `RateTest`: real polling rate from WM_INPUT raw input events on a message-only window with its own thread.
//!   Read only: nothing is hooked or written.
//!
//! Differences from C#: the 0.1 s slice counting uses binary search instead of rescanning every event
//! (the C# version was quadratic at 8000 Hz).

use crate::catalog::{Hooks, SystemHooks};
use crate::error::{Error, Result};
use crate::reg::{self, Hive, RegValue};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::mpsc::Sender;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

// ---------------------------------------------------------------- pointer acceleration

const MOUSE_KEY: &str = r"Control Panel\Mouse";

/// "Enhance pointer precision" is on unless MouseSpeed is "0" (a missing value counts as on).
pub fn acceleration_on() -> bool {
    !matches!(reg::get(Hive::Hkcu, MOUSE_KEY, "MouseSpeed"), Ok(Some(RegValue::String(s))) if s == "0")
}

pub fn set_acceleration(on: bool) -> Result<()> {
    use windows::Win32::UI::WindowsAndMessaging::{SystemParametersInfoW, SPI_SETMOUSE, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS};
    let (speed, t1, t2): (i32, i32, i32) = if on { (1, 6, 10) } else { (0, 0, 0) };
    for (name, v) in [("MouseSpeed", speed), ("MouseThreshold1", t1), ("MouseThreshold2", t2)] {
        reg::set(Hive::Hkcu, MOUSE_KEY, name, &RegValue::String(v.to_string()))?;
    }
    // SPI_SETMOUSE takes {threshold1, threshold2, acceleration}; 0x03 = UPDATEINIFILE | SENDCHANGE.
    let mut params = [t1, t2, speed];
    unsafe {
        SystemParametersInfoW(SPI_SETMOUSE, 0, Some(params.as_mut_ptr().cast()), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0x03))
            .map_err(|e| Error::Os(format!("SPI_SETMOUSE: {e}")))?;
    }
    SystemHooks.broadcast_setting_change();
    Ok(())
}

// ---------------------------------------------------------------- analysis (pure, tested)

const NS: f64 = 1e9;
const WINDOW: i64 = 2_000_000_000; // history kept
const PAUSE: i64 = 1_000_000_000 / 30; // a longer gap means the mouse stopped moving
const SLICE: i64 = 100_000_000; // 0.1 s slices for the "slowest" figure
const SLICE_10MS: i64 = 10_000_000;
const MIN_REPORTS: usize = 40;
const MIN_SECONDS: f64 = 0.25;
const COUNT_BINS: usize = 512;
const STANDARD: [f64; 7] = [125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0];

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Snapshot {
    pub rate: f64,
    pub slowest: f64,
    pub samples: usize,
    pub reports: u64,
    pub active_seconds: f64,
    pub peak: f64,
    pub load: f64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stability {
    pub slices: u64,
    pub expected: f64,
    pub dropout_percent: f64,
    pub worst_percent: f64,
}

/// Reports per 10 ms slice during continuous movement, kept as a histogram.
#[derive(Clone)]
struct SliceStats {
    cur: i64,
    count: usize,
    flowing: bool,
    hist: Vec<u32>,
    slices: u64,
}

impl SliceStats {
    fn new(cur: i64) -> SliceStats {
        SliceStats { cur, count: 0, flowing: false, hist: vec![0; COUNT_BINS], slices: 0 }
    }

    fn percentile(&self, p: f64) -> usize {
        let target = self.slices as f64 * p;
        let mut seen = 0u64;
        for (i, &h) in self.hist.iter().enumerate() {
            seen += h as u64;
            if seen as f64 >= target {
                return i;
            }
        }
        COUNT_BINS - 1
    }

    fn stability(&self) -> Option<Stability> {
        if self.slices < 150 {
            return None;
        }
        let expected = self.percentile(0.5) as f64;
        if expected < 8.0 {
            return None; // below about 800 Hz a 10 ms slice holds too few reports to judge
        }
        let limit = expected * 0.75;
        let dropouts: u64 = self.hist.iter().enumerate().filter(|(i, _)| (*i as f64) < limit).map(|(_, &h)| h as u64).sum();
        Some(Stability {
            slices: self.slices,
            expected,
            dropout_percent: 100.0 * dropouts as f64 / self.slices as f64,
            worst_percent: 100.0 * self.percentile(0.01) as f64 / expected,
        })
    }
}

#[derive(Default)]
struct Stats {
    /// timestamps (ns) of movement reports per device
    devices: HashMap<isize, Vec<i64>>,
    slices: HashMap<isize, SliceStats>,
    reports: u64,
}

impl Stats {
    fn record(&mut self, device: isize, dx: i32, dy: i32, now: i64) {
        if dx == 0 && dy == 0 {
            return; // clicks and wheel don't follow the movement report rate
        }
        self.reports += 1;
        let list = self.devices.entry(device).or_default();
        list.push(now);

        let index = now / SLICE_10MS;
        let s = self.slices.entry(device).or_insert_with(|| SliceStats::new(index));
        if index != s.cur {
            let jump = index - s.cur;
            if s.flowing && jump <= 3 {
                // up to two empty slices still count: a stall, not a pause of the hand
                s.hist[s.count.min(COUNT_BINS - 1)] += 1;
                s.slices += 1;
                for _ in 1..jump {
                    s.hist[0] += 1;
                    s.slices += 1;
                }
            }
            s.flowing = jump <= 3;
            s.cur = index;
            s.count = 0;
        }
        s.count += 1;
        // Drop old history in bulk now and then instead of on every report.
        if list.len() > 4096 && now - list[0] > WINDOW * 2 {
            let keep = list.iter().position(|&t| now - t <= WINDOW).unwrap_or(0);
            list.drain(..keep);
        }
    }

    /// Events of the mouse that reported most in the last two seconds.
    fn best_events(&self, now: i64) -> Vec<i64> {
        self.devices
            .values()
            .map(|v| v.iter().copied().filter(|&t| now - t <= WINDOW).collect::<Vec<_>>())
            .max_by_key(|v| v.len())
            .unwrap_or_default()
    }

    fn stability(&self) -> Option<Stability> {
        self.slices.values().max_by_key(|s| s.slices)?.stability()
    }
}

/// Rate figures from the recent events (sorted ascending, ns).
pub fn analyze(events: &[i64], reports: u64, load: f64) -> Snapshot {
    let none = Snapshot { samples: events.len(), reports, ..Default::default() };
    if events.len() < MIN_REPORTS {
        return none;
    }
    let (mut counted, mut time) = (0i64, 0i64);
    let mut slices: Vec<f64> = vec![];
    let mut start = 0usize;
    for i in 1..=events.len() {
        if i < events.len() && events[i] - events[i - 1] <= PAUSE {
            continue;
        }
        let span = events[i - 1] - events[start];
        if i - start >= MIN_REPORTS && span > 0 {
            counted += (i - start - 1) as i64;
            time += span;
            let stretch = &events[start..i];
            let mut t = events[start];
            while t + SLICE <= events[i - 1] {
                let lo = stretch.partition_point(|&e| e < t);
                let hi = stretch.partition_point(|&e| e < t + SLICE);
                slices.push((hi - lo) as f64 * 10.0);
                t += SLICE;
            }
        }
        start = i;
    }
    if (time as f64) < MIN_SECONDS * NS {
        return none;
    }
    let rate = counted as f64 * NS / time as f64;
    slices.sort_by(|a, b| a.total_cmp(b));
    let slowest = if slices.is_empty() { rate } else { slices[(slices.len() as f64 * 0.05) as usize] };
    // The best sustained 0.1 s slices show what the mouse can deliver; the median also drops when the hand is slow.
    let peak = if slices.is_empty() { rate } else { slices[(slices.len() - 1).min((slices.len() as f64 * 0.95) as usize)] };
    Snapshot { rate, slowest, samples: events.len(), reports, active_seconds: time as f64 / NS, peak, load }
}

/// The usual rate (125 ... 8000 Hz) closest to the measurement, or 0 if none is within 12%.
pub fn nearest(hz: f64) -> f64 {
    let n = STANDARD.iter().copied().min_by(|a, b| (a - hz).abs().total_cmp(&(b - hz).abs())).unwrap();
    if (n - hz).abs() / n <= 0.12 {
        n
    } else {
        0.0
    }
}

// ---------------------------------------------------------------- end of test: texts

pub const IDLE_HINT: &str = "Press Start, then move the mouse in fast, large circles for 10 seconds without stopping.";

/// Grouped like .NET `N0` (en-US).
pub fn n0(x: f64) -> String {
    let s = format!("{:.0}", x.abs());
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if x < 0.0 && x.round() != 0.0 {
        out.insert(0, '-');
    }
    out
}

/// What the page shows after the 10 seconds (`MouseRateVM.Finish`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Finished {
    pub enough: bool,
    pub rate_text: String,
    pub match_text: String,
    pub spread_text: String,
    pub slowest_text: String,
    pub load_text: String,
    pub dropouts_text: String,
    pub worst_text: String,
    pub stability_text: String,
    pub verdict: String,
    pub summary: String,
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    sorted[(sorted.len() - 1).min((sorted.len() as f64 * p) as usize)]
}

fn sorted(v: &[f64]) -> Vec<f64> {
    let mut v = v.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    v
}

pub fn finish(rates: &[f64], slowest: &[f64], peaks: &[f64], loads: &[f64], st: Option<Stability>) -> Finished {
    let dash = "\u{2013}".to_string();
    if rates.len() < 5 {
        return Finished {
            enough: false,
            rate_text: format!("{} Hz", dash),
            slowest_text: dash.clone(),
            load_text: dash.clone(),
            dropouts_text: dash.clone(),
            worst_text: dash,
            summary: "Not enough continuous movement. Try again with fast, large circles.".into(),
            verdict: "Try again: fast, large circles for the full 10 seconds, without stopping.".into(),
            ..Default::default()
        };
    }
    let rates = sorted(rates);
    let slow = sorted(slowest);
    let peaks = sorted(peaks);
    let median = percentile(&rates, 0.5);
    let peak = percentile(&peaks, 0.5);
    let near = nearest(peak);
    let slow_med = percentile(&slow, 0.5);
    let load = if loads.is_empty() { 0.0 } else { loads.iter().sum::<f64>() / loads.len() as f64 };

    let mut f = Finished {
        enough: true,
        rate_text: format!("{} Hz", n0(peak)),
        match_text: if near > 0.0 { format!("\u{2248} {} Hz setting", n0(near)) } else { String::new() },
        spread_text: format!("{} Hz", n0(median)),
        slowest_text: format!("{} Hz", n0(slow_med)),
        load_text: format!("{:.0}%", load * 100.0),
        dropouts_text: dash.clone(),
        worst_text: dash,
        ..Default::default()
    };

    let mut grade: Option<&str> = None;
    match st {
        None => f.stability_text = "Stability needs a mouse at about 1000 Hz or more and a few seconds of continuous movement.".into(),
        Some(s) => {
            f.dropouts_text = format!("{:.1} %", s.dropout_percent);
            f.worst_text = format!("{:.0} %", s.worst_percent);
            let g = if s.dropout_percent < 0.5 && s.worst_percent >= 80.0 {
                "Excellent"
            } else if s.dropout_percent < 2.0 && s.worst_percent >= 65.0 {
                "Good"
            } else if s.dropout_percent < 5.0 {
                "Fair"
            } else {
                "Poor"
            };
            grade = Some(g);
            let advice = if matches!(g, "Fair" | "Poor") {
                "Try another USB port on the back of the PC (no hub), turn off USB power saving, and close programs that use a lot of CPU, then run it again."
            } else {
                "Compare ports and settings: lower dropouts are better. Windows adds a little timing noise of its own."
            };
            f.stability_text = format!(
                "Stability: {g}. A dropout is a 10 ms stretch where the mouse sent at least a quarter fewer reports than usual; it feels like tiny stutters in aim. {advice}"
            );
        }
    }

    let busy = near == 0.0 && peak >= 2000.0 && load >= 0.7;
    let fast_unmatched = near == 0.0 && peak >= 2000.0;
    f.verdict = if busy {
        "This PC is too busy handling the mouse for the test to keep up, so the real rate may be higher than shown. Close other programs and try again.".into()
    } else if fast_unmatched {
        format!(
            "The mouse delivers about {} Hz here and the test itself isn't the limit (it used only {:.0}% of its time). Most often the hand is too slow for the DPI: try 1600 to 3200 DPI with fast swipes. If that doesn't help, check the polling rate in the mouse software and use a USB port on the back of the PC without a hub.",
            n0(peak),
            load * 100.0
        )
    } else if near == 0.0 {
        "The rate doesn't match a usual setting. Run the test again with faster, larger circles.".into()
    } else if near <= 250.0 && peak < 150.0 {
        "This is a low rate, typical of Bluetooth or a 125 Hz setting. For games use the cable or the wireless dongle and set 1000 Hz in the mouse maker's software.".into()
    } else if median < peak * 0.85 {
        format!(
            "The mouse reaches {n} Hz in its best moments but averaged {m} Hz. That is normal when the hand is slow: a mouse only reports while it moves, and {n} Hz needs about {cm} cm/s at 800 DPI (half of that at 1600 DPI). Raise the DPI for this test or swipe faster. If it stays low even then, use a USB port on the back of the PC (not a hub or front panel).",
            n = n0(near),
            m = n0(median),
            cm = n0(near / 800.0 * 2.5)
        )
    } else if slow_med < near * 0.7 {
        "Reports sometimes arrive late. Plug the mouse straight into the PC (not a hub), avoid the front ports, and check that USB power saving is off in Optimize.".into()
    } else {
        "Reports arrive steadily at your mouse's rate. If you expected a higher rate, set it in the mouse maker's software.".into()
    };

    f.summary = if busy {
        "This PC is too busy for the test to keep up. Close other programs and try again.".into()
    } else if fast_unmatched {
        format!("The mouse delivers about {} Hz here. Try a higher DPI with fast swipes.", n0(peak))
    } else if near == 0.0 {
        "The rate is uneven. Try again with faster, larger circles.".into()
    } else if near <= 250.0 && peak < 150.0 {
        "Low rate, typical of Bluetooth or a 125 Hz setting.".into()
    } else if median < peak * 0.85 {
        format!("Reaches {} Hz at best. Swipe faster or raise the DPI for the full rate.", n0(near))
    } else if let Some(g) = grade {
        format!("Stability: {g}.{}", if matches!(g, "Fair" | "Poor") { " Try another USB port." } else { "" })
    } else {
        "Reports arrive steadily.".into()
    };
    f
}

// ---------------------------------------------------------------- raw input listener

fn epoch() -> Instant {
    static E: OnceLock<Instant> = OnceLock::new();
    *E.get_or_init(Instant::now)
}

fn now_ns() -> i64 {
    epoch().elapsed().as_nanos() as i64
}

fn stats() -> &'static Mutex<Stats> {
    static S: OnceLock<Mutex<Stats>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Stats::default()))
}

struct Listener {
    tx: Sender<Snapshot>,
    wake: Box<dyn Fn() + Send>,
    last_publish: i64,
    cpu_last: i64,
    load_since: i64,
}

thread_local! {
    static LISTENER: RefCell<Option<Listener>> = const { RefCell::new(None) };
}

/// CPU time used so far by the calling thread, in 100 ns units.
fn thread_cpu() -> i64 {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentThread, GetThreadTimes};
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe {
        let _ = GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u);
    }
    let q = |f: FILETIME| (((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64) as i64;
    q(k) + q(u)
}

unsafe extern "system" fn wndproc(
    hwnd: windows::Win32::Foundation::HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::UI::Input::{GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTHEADER, RID_INPUT};
    use windows::Win32::UI::WindowsAndMessaging::{DefWindowProcW, DestroyWindow, PostQuitMessage, WM_CLOSE, WM_DESTROY, WM_INPUT};
    match msg {
        WM_INPUT => {
            let now = now_ns();
            // One report per message, timestamped when it is handled.
            let mut raw: RAWINPUT = unsafe { std::mem::zeroed() };
            let mut size = std::mem::size_of::<RAWINPUT>() as u32;
            let got = unsafe {
                GetRawInputData(
                    HRAWINPUT(lparam.0 as *mut _),
                    RID_INPUT,
                    Some((&mut raw as *mut RAWINPUT).cast()),
                    &mut size,
                    std::mem::size_of::<RAWINPUTHEADER>() as u32,
                )
            };
            if got != u32::MAX && raw.header.dwType == 0 {
                let m = unsafe { raw.data.mouse };
                if let Ok(mut s) = stats().lock() {
                    s.record(raw.header.hDevice.0 as isize, m.lLastX, m.lLastY, now);
                }
            }
            LISTENER.with(|l| {
                if let Some(l) = l.borrow_mut().as_mut() {
                    publish_if_due(l, now);
                }
            });
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_CLOSE => {
            let _ = unsafe { DestroyWindow(hwnd) };
            windows::Win32::Foundation::LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            windows::Win32::Foundation::LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}

/// Publishes four times a second.
fn publish_if_due(l: &mut Listener, now: i64) {
    if (now - l.last_publish) * 4 < 1_000_000_000 {
        return;
    }
    l.last_publish = now;
    let elapsed = now - l.load_since;
    let cpu = thread_cpu();
    let cpu_ns = (cpu - l.cpu_last) * 100;
    let load = if elapsed > 0 { (cpu_ns as f64 / elapsed as f64).clamp(0.0, 1.0) } else { 0.0 };
    l.cpu_last = cpu;
    l.load_since = now;
    let (events, reports) = match stats().lock() {
        Ok(s) => (s.best_events(now), s.reports),
        Err(_) => return,
    };
    let _ = l.tx.send(analyze(&events, reports, load));
    (l.wake)();
}

/// A running measurement. Dropping it (or `stop`) unregisters the raw input device and ends the listener thread.
pub struct RateTest {
    hwnd: isize,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl RateTest {
    /// Starts listening. Snapshots arrive on `tx` four times a second; `wake` is called after each one
    /// (to repaint the UI).
    pub fn start(tx: Sender<Snapshot>, wake: impl Fn() + Send + 'static) -> std::result::Result<RateTest, String> {
        *stats().lock().map_err(|e| e.to_string())? = Stats::default();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<std::result::Result<isize, String>>();
        let thread = std::thread::Builder::new()
            .name("MouseRate".into())
            .spawn(move || listener_thread(tx, Box::new(wake), ready_tx))
            .map_err(|e| e.to_string())?;
        match ready_rx.recv_timeout(std::time::Duration::from_secs(3)) {
            Ok(Ok(hwnd)) => Ok(RateTest { hwnd, thread: Some(thread) }),
            Ok(Err(e)) => Err(e),
            Err(_) => Err("The mouse listener did not start.".into()),
        }
    }

    /// Evenness of the reports seen since start (None when there was too little continuous movement).
    pub fn stability(&self) -> Option<Stability> {
        stats().lock().ok()?.stability()
    }

    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
        use windows::Win32::UI::Input::{RegisterRawInputDevices, RAWINPUTDEVICE, RIDEV_REMOVE};
        use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
        let Some(t) = self.thread.take() else { return };
        unsafe {
            let rid = [RAWINPUTDEVICE { usUsagePage: 1, usUsage: 2, dwFlags: RIDEV_REMOVE, hwndTarget: HWND::default() }];
            let _ = RegisterRawInputDevices(&rid, std::mem::size_of::<RAWINPUTDEVICE>() as u32);
            let _ = PostMessageW(Some(HWND(self.hwnd as *mut _)), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
        let _ = t.join();
    }
}

impl Drop for RateTest {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn listener_thread(tx: Sender<Snapshot>, wake: Box<dyn Fn() + Send>, ready: Sender<std::result::Result<isize, String>>) {
    use windows::core::w;
    use windows::Win32::Foundation::HINSTANCE;
    use windows::Win32::Media::{timeBeginPeriod, timeEndPeriod};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Threading::{GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST};
    use windows::Win32::UI::Input::{RegisterRawInputDevices, RAWINPUTDEVICE, RIDEV_INPUTSINK};
    use windows::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DispatchMessageW, GetMessageW, RegisterClassW, TranslateMessage, HWND_MESSAGE, MSG, WINDOW_EX_STYLE,
        WINDOW_STYLE, WNDCLASSW,
    };
    unsafe {
        // High priority and a 1 ms system timer keep fast mice (4000+ Hz) from queuing up reports.
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        timeBeginPeriod(1);
        let setup = || -> std::result::Result<windows::Win32::Foundation::HWND, String> {
            let hinst = HINSTANCE(GetModuleHandleW(None).map_err(|e| e.to_string())?.0);
            let class = w!("MyTuneMouseRate");
            let wc = WNDCLASSW { lpfnWndProc: Some(wndproc), hInstance: hinst, lpszClassName: class, ..Default::default() };
            RegisterClassW(&wc); // 0 if it already exists from an earlier test: fine
            let hwnd = CreateWindowExW(WINDOW_EX_STYLE(0), class, w!("MyTuneMouseRate"), WINDOW_STYLE(0), 0, 0, 0, 0, Some(HWND_MESSAGE), None, Some(hinst), None)
                .map_err(|e| e.to_string())?;
            let rid = [RAWINPUTDEVICE { usUsagePage: 1, usUsage: 2, dwFlags: RIDEV_INPUTSINK, hwndTarget: hwnd }];
            RegisterRawInputDevices(&rid, std::mem::size_of::<RAWINPUTDEVICE>() as u32).map_err(|_| "Windows refused raw mouse input.".to_string())?;
            Ok(hwnd)
        };
        match setup() {
            Err(e) => {
                let _ = ready.send(Err(e));
            }
            Ok(hwnd) => {
                let now = now_ns();
                LISTENER.with(|l| {
                    *l.borrow_mut() = Some(Listener { tx, wake, last_publish: 0, cpu_last: thread_cpu(), load_since: now });
                });
                let _ = ready.send(Ok(hwnd.0 as isize));
                let mut msg = MSG::default();
                while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
                LISTENER.with(|l| *l.borrow_mut() = None);
            }
        }
        timeEndPeriod(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Events at `hz` for `secs` seconds with constant spacing.
    fn steady(hz: f64, secs: f64) -> Vec<i64> {
        (0..(hz * secs) as i64).map(|i| (i as f64 * NS / hz) as i64).collect()
    }

    #[test]
    fn analyze_steady_1000hz() {
        let s = analyze(&steady(1000.0, 2.0), 2000, 0.1);
        assert!((s.rate - 1000.0).abs() < 5.0, "{}", s.rate);
        assert!((s.peak - 1000.0).abs() < 20.0, "{}", s.peak);
        assert!(s.active_seconds > 1.9);
        assert_eq!(nearest(s.rate), 1000.0);
    }

    #[test]
    fn analyze_needs_movement() {
        assert_eq!(analyze(&steady(1000.0, 0.02), 20, 0.0).rate, 0.0); // fewer than 40 reports
        // two bursts split by a pause longer than 1/30 s never form a long stretch
        let mut v: Vec<i64> = (0..30).map(|i| i * 1_000_000).collect();
        v.extend((0..30).map(|i| 500_000_000 + i * 1_000_000));
        assert_eq!(analyze(&v, 60, 0.0).rate, 0.0);
    }

    #[test]
    fn nearest_snaps_within_12_percent() {
        assert_eq!(nearest(950.0), 1000.0);
        assert_eq!(nearest(1500.0), 0.0);
        assert_eq!(nearest(8100.0), 8000.0);
    }

    #[test]
    fn stability_steady_vs_dropouts() {
        let mut st = Stats::default();
        // 2 s at 1000 Hz: 10 reports per 10 ms slice, nothing missing
        for t in steady(1000.0, 2.0) {
            st.record(1, 1, 0, t);
        }
        let s = st.stability().expect("enough slices");
        assert_eq!(s.expected, 10.0);
        assert!(s.dropout_percent < 0.5, "{}", s.dropout_percent);
        // same with every 10th slice nearly empty
        let mut st = Stats::default();
        for t in steady(1000.0, 3.0) {
            if (t / SLICE_10MS) % 10 == 0 && t % SLICE_10MS > 2_000_000 {
                continue;
            }
            st.record(1, 1, 0, t);
        }
        let s = st.stability().expect("enough slices");
        assert!(s.dropout_percent > 5.0, "{}", s.dropout_percent);
    }

    #[test]
    fn stability_needs_fast_mouse() {
        let mut st = Stats::default();
        for t in steady(500.0, 3.0) {
            st.record(1, 1, 0, t);
        }
        assert!(st.stability().is_none()); // 5 reports per slice: too few to judge
        assert!(Stats::default().stability().is_none());
    }

    #[test]
    fn zero_movement_is_ignored() {
        let mut st = Stats::default();
        st.record(1, 0, 0, 5);
        assert_eq!(st.reports, 0);
    }

    #[test]
    fn finish_texts() {
        let rates = vec![990.0; 6];
        let f = finish(&rates, &rates, &vec![1000.0; 6], &[0.05; 6], None);
        assert!(f.enough);
        assert_eq!(f.rate_text, "1,000 Hz");
        assert_eq!(f.match_text, "\u{2248} 1,000 Hz setting");
        assert!(f.summary.starts_with("Reports arrive steadily"));
        assert!(!finish(&rates[..3], &rates[..3], &rates[..3], &[], None).enough);
        let low = finish(&[500.0; 6], &[500.0; 6], &[1000.0; 6], &[0.0; 6], None);
        assert!(low.summary.starts_with("Reaches 1,000 Hz at best")); // median 500 < 85% of peak
    }

    #[test]
    fn n0_groups_thousands() {
        assert_eq!(n0(0.0), "0");
        assert_eq!(n0(999.4), "999");
        assert_eq!(n0(8000.0), "8,000");
        assert_eq!(n0(1234567.0), "1,234,567");
    }

    #[test]
    fn listener_starts_and_stops() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let t = RateTest::start(tx, || {}).expect("raw input listener");
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(t.stability().is_none());
        t.stop(); // joins the thread: a hang here means the message loop did not end
    }

    /// Touches the real pointer setting (restores it): `cargo test -p mytune-core --lib mouse -- --ignored`.
    #[test]
    #[ignore]
    fn real_acceleration_roundtrip() {
        let before = acceleration_on();
        set_acceleration(!before).unwrap();
        assert_eq!(acceleration_on(), !before);
        set_acceleration(before).unwrap();
        assert_eq!(acceleration_on(), before);
    }
}
