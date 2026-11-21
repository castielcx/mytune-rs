//! Sensor snapshots (port of C# `Services/Sensors.cs`) without LibreHardwareMonitor.
//! CPU/RAM/GPU via Windows APIs and NVML; CPU temp and power via PawnIO (opt-in). No fans, like the C# app.
//! Cadence: 1 s while the window is watched or a game runs, else 5 s.

mod hw;
mod nvml;
mod pdh;
pub mod decode;
pub mod pawnio;
mod thermal;
pub mod session_feed;

pub use hw::HwSource;

use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Same fields as the C# `SensorSnapshot`. `None` = sensor missing (UI shows a dash).
/// Units: % load, deg C, MHz, W, MB (VRAM), GB (RAM).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Snapshot {
    pub cpu_load: Option<f32>,
    pub cpu_temp: Option<f32>,
    pub cpu_clock_mhz: Option<f32>,
    pub cpu_power_w: Option<f32>,
    pub gpu_load: Option<f32>,
    pub gpu_temp: Option<f32>,
    pub gpu_clock_mhz: Option<f32>,
    pub gpu_power_w: Option<f32>,
    pub vram_used_mb: Option<f32>,
    pub vram_total_mb: Option<f32>,
    pub ram_load: Option<f32>,
    pub ram_used_gb: Option<f32>,
    pub ram_total_gb: Option<f32>,
}

/// Where readings come from; tests plug a fake in.
pub trait Source: Send + 'static {
    fn sample(&mut self) -> Snapshot;
    /// (CPU name, GPU name)
    fn names(&self) -> (String, String);
}

#[derive(Debug, Clone, Copy)]
pub struct Cadence {
    pub fast: Duration,
    pub idle: Duration,
}

impl Default for Cadence {
    fn default() -> Self {
        Cadence { fast: Duration::from_secs(1), idle: Duration::from_secs(5) }
    }
}

impl Cadence {
    pub fn interval(&self, watched: bool, game_running: bool) -> Duration {
        if watched || game_running { self.fast } else { self.idle }
    }
}

#[derive(Default)]
struct State {
    watched: bool,
    game: bool,
    stop: bool,
    wake: bool,
    latest: Option<Snapshot>,
    /// Last `HISTORY` loads: CPU, GPU, RAM. Filled by the normal loop, so the graphs have a past at no extra cost.
    hist: [Vec<f32>; 3],
}

const HISTORY: usize = 60;

type Callback = Box<dyn Fn(&Snapshot) + Send>;

struct Shared {
    st: Mutex<State>,
    cv: Condvar,
    subs: Mutex<Vec<Callback>>,
}

pub struct Sensors {
    sh: Arc<Shared>,
    names: (String, String),
    th: Option<JoinHandle<()>>,
}

impl Sensors {
    pub fn start(mut src: impl Source, cadence: Cadence) -> Sensors {
        let names = src.names(); // shortcut: names are read once, a GPU that shows up later is not renamed
        let sh = Arc::new(Shared { st: Mutex::new(State::default()), cv: Condvar::new(), subs: Mutex::new(Vec::new()) });
        let s2 = sh.clone();
        let th = std::thread::Builder::new()
            .name("sensors".into())
            .spawn(move || loop {
                if s2.st.lock().unwrap().stop {
                    return;
                }
                let snap = src.sample();
                for cb in s2.subs.lock().unwrap().iter() {
                    cb(&snap);
                }
                let mut g = s2.st.lock().unwrap();
                for (h, v) in g.hist.iter_mut().zip([snap.cpu_load, snap.gpu_load, snap.ram_load]) {
                    if let Some(v) = v {
                        h.push(v);
                        if h.len() > HISTORY {
                            h.remove(0);
                        }
                    }
                }
                g.latest = Some(snap);
                g.wake = false;
                let iv = cadence.interval(g.watched, g.game);
                // sleeps until the interval ends, or earlier on a stop / watched=true / game change
                let _ = s2.cv.wait_timeout_while(g, iv, |s| !s.wake && !s.stop).unwrap();
            })
            .expect("spawn sensors thread");
        Sensors { sh, names, th: Some(th) }
    }

    /// Window shown and not minimized. Turning it on wakes the loop for an immediate sample.
    pub fn set_watched(&self, v: bool) {
        let mut g = self.sh.st.lock().unwrap();
        let on = v && !g.watched;
        g.watched = v;
        g.wake |= on;
        self.sh.cv.notify_all();
    }

    /// Any Game Hub game running. Any change wakes the loop so the new cadence applies at once.
    pub fn set_game_running(&self, v: bool) {
        let mut g = self.sh.st.lock().unwrap();
        if g.game != v {
            g.game = v;
            g.wake = true;
            self.sh.cv.notify_all();
        }
    }

    pub fn latest(&self) -> Option<Snapshot> {
        self.sh.st.lock().unwrap().latest
    }

    /// Recent loads, oldest first: [CPU, GPU, RAM]. One point per sample: 1 s while watched or gaming, else 5 s.
    pub fn history(&self) -> [Vec<f32>; 3] {
        self.sh.st.lock().unwrap().hist.clone()
    }

    /// Called on the sensor thread after every sample (like `Sensors.Updated`). Keep it short.
    pub fn on_update(&self, cb: impl Fn(&Snapshot) + Send + 'static) {
        self.sh.subs.lock().unwrap().push(Box::new(cb));
    }

    pub fn cpu_name(&self) -> &str {
        &self.names.0
    }

    pub fn gpu_name(&self) -> &str {
        &self.names.1
    }

    pub fn stop(&mut self) {
        self.sh.st.lock().unwrap().stop = true;
        self.sh.cv.notify_all();
        if let Some(t) = self.th.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Sensors {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Instant;

    struct Fake(Arc<AtomicU32>);
    impl Source for Fake {
        fn sample(&mut self) -> Snapshot {
            let n = self.0.fetch_add(1, Ordering::SeqCst);
            Snapshot { cpu_load: Some(n as f32), ..Default::default() }
        }
        fn names(&self) -> (String, String) {
            ("cpu".into(), "gpu".into())
        }
    }

    #[test]
    fn interval_rules() {
        let c = Cadence::default();
        assert_eq!(c.interval(false, false), Duration::from_secs(5));
        assert_eq!(c.interval(true, false), Duration::from_secs(1));
        assert_eq!(c.interval(false, true), Duration::from_secs(1));
    }

    #[test]
    fn idle_is_slow_watched_is_fast_and_wakes() {
        let n = Arc::new(AtomicU32::new(0));
        let c = Cadence { fast: Duration::from_millis(20), idle: Duration::from_secs(30) };
        let s = Sensors::start(Fake(n.clone()), c);
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(n.load(Ordering::SeqCst), 1, "idle: one sample, then waits");
        s.set_watched(true); // wakes the 30 s sleep at once, then 20 ms cadence
        std::thread::sleep(Duration::from_millis(300));
        assert!(n.load(Ordering::SeqCst) >= 8, "watched: fast cadence, got {}", n.load(Ordering::SeqCst));
        s.set_watched(false);
        std::thread::sleep(Duration::from_millis(60)); // let the in-flight fast wait end
        let after = n.load(Ordering::SeqCst);
        std::thread::sleep(Duration::from_millis(200));
        assert!(n.load(Ordering::SeqCst) <= after + 1, "back to idle");
        assert!(s.latest().is_some());
    }

    #[test]
    fn game_running_is_fast_and_stop_is_prompt() {
        let n = Arc::new(AtomicU32::new(0));
        let c = Cadence { fast: Duration::from_millis(20), idle: Duration::from_secs(30) };
        let mut s = Sensors::start(Fake(n.clone()), c);
        s.set_game_running(true);
        std::thread::sleep(Duration::from_millis(250));
        assert!(n.load(Ordering::SeqCst) >= 6);
        let t = Instant::now();
        s.stop();
        assert!(t.elapsed() < Duration::from_secs(1));
    }

    /// Loads the PawnIO module and reads MSRs. Never runs by default: `cargo test -p mytune-sensors -- --ignored pawnio_live --nocapture`
    #[test]
    #[ignore]
    fn pawnio_live() {
        let mut h = HwSource::new(true);
        let _ = h.sample();
        std::thread::sleep(Duration::from_millis(1100));
        let s = h.sample();
        println!("{:?}", s);
        assert!(s.cpu_temp.is_some(), "no CPU temp: driver missing, module refused, or unsupported CPU");
    }

    /// Read-only on the real machine; PawnIO stays off.
    #[test]
    fn live_without_driver() {
        let mut h = HwSource::new(false);
        let _ = h.sample();
        std::thread::sleep(Duration::from_millis(1100));
        let s = h.sample();
        println!("{:?}\nnames {:?}", s, h.names());
        assert!(s.ram_total_gb.unwrap() > 1.0);
        assert!(s.cpu_load.is_some());
        assert!(s.cpu_temp.is_none() && s.cpu_power_w.is_none());
    }
}
