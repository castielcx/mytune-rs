//! Speed test (C# `SpeedTest`): idle ping, 8 s download, 8 s upload against Cloudflare's public test
//! servers, ping sampled while the line is busy. Runs only when the user starts it.
//!
//! Differences from C#: blocking `ureq` on threads instead of async HttpClient; pings come from
//! `ping.exe` (one process per sample) instead of the .NET Ping class.

use crate::pccheck::parse_ping;
use crate::process::run;
use std::io::Read;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

const BASE: &str = "https://speed.cloudflare.com";
const PING_HOST: &str = "1.1.1.1";
const STREAMS: usize = 4;
const SECONDS: f64 = 8.0;
const WARM_UP: f64 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SpeedResult {
    pub down_mbps: f64,
    pub up_mbps: f64,
    pub ping_ms: f64,
    pub jitter_ms: f64,
    pub loaded_ms: f64,
}

/// The last finished test with its live readings, kept in `speedtest.json` next to `state.json`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LastSpeed {
    /// Local time, `yyyy-MM-dd HH:mm`.
    pub at: String,
    pub result: SpeedResult,
    pub down: Vec<f64>,
    pub up: Vec<f64>,
}

const LAST_FILE: &str = "speedtest.json";
/// More than a run produces (about 28 readings per phase); a longer list in the file is cut.
const MAX_SAMPLES: usize = 64;

/// The saved test. A missing, corrupt or nonsensical file is simply no saved test.
pub fn load_last(dir: &std::path::Path) -> Option<LastSpeed> {
    let mut l: LastSpeed = serde_json::from_slice(&std::fs::read(dir.join(LAST_FILE)).ok()?).ok()?;
    l.down.truncate(MAX_SAMPLES);
    l.up.truncate(MAX_SAMPLES);
    let r = l.result;
    let ok = |v: &f64| v.is_finite() && *v >= 0.0;
    let valid = l.at.chars().count() <= 32 && [r.down_mbps, r.up_mbps, r.ping_ms, r.jitter_ms, r.loaded_ms].iter().all(ok) && l.down.iter().chain(&l.up).all(ok);
    valid.then_some(l)
}

/// Writes through a temporary file, so a crash mid-write leaves the previous test intact.
pub fn save_last(dir: &std::path::Path, last: &LastSpeed) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join("speedtest.json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(last)?)?;
    std::fs::rename(tmp, dir.join(LAST_FILE))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Ping,
    Download,
    Upload,
}

pub fn mbps(bytes: u64, seconds: f64) -> f64 {
    if seconds <= 0.0 {
        0.0
    } else {
        bytes as f64 * 8.0 / 1e6 / seconds
    }
}

/// Middle sample of the sorted list (upper middle for even counts, like C# `ElementAt(Count / 2)`).
pub fn median(l: &[f64]) -> f64 {
    let mut v = l.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    v.get(v.len() / 2).copied().unwrap_or(0.0)
}

/// Mean absolute difference of consecutive samples.
pub fn jitter(l: &[f64]) -> f64 {
    let d: Vec<f64> = l.windows(2).map(|w| (w[0] - w[1]).abs()).collect();
    if d.is_empty() {
        0.0
    } else {
        d.iter().sum::<f64>() / d.len() as f64
    }
}

fn ping_once() -> Option<f64> {
    let r = run("ping.exe", &["-n", "1", "-w", "1000", PING_HOST]).ok()?;
    match parse_ping(&r.output) {
        (n, Some((_, _, avg))) if n > 0 => Some(avg),
        _ => None,
    }
}

/// Pings every `gap` until `count` samples were tried or `stop` is set.
fn pings(count: usize, gap: Duration, stop: &AtomicBool) -> Vec<f64> {
    let mut v = Vec::new();
    for _ in 0..count {
        if stop.load(Relaxed) {
            break;
        }
        v.extend(ping_once());
        thread::sleep(gap);
    }
    v
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout_read(Duration::from_secs(15)).build()
}

fn download(bytes: &AtomicU64, stop: &AtomicBool) {
    let agent = agent();
    let mut buf = vec![0u8; 81920];
    while !stop.load(Relaxed) {
        let Ok(res) = agent.get(&format!("{BASE}/__down?bytes=50000000")).call() else { return };
        let mut r = res.into_reader();
        while !stop.load(Relaxed) {
            match r.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    bytes.fetch_add(n as u64, Relaxed);
                }
                Err(_) => return,
            }
        }
    }
}

fn upload(bytes: &AtomicU64, stop: &AtomicBool) {
    let agent = agent();
    // pseudo-random so nothing on the way can compress it
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1);
    let mut x = 0x9E37_79B9_7F4A_7C15u64 ^ nanos | 1; // xorshift needs a non-zero seed
    let data: Vec<u8> = (0..2_000_000)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect();
    while !stop.load(Relaxed) {
        if agent.post(&format!("{BASE}/__up")).send_bytes(&data).is_err() {
            return;
        }
        bytes.fetch_add(data.len() as u64, Relaxed);
    }
}

/// Returns (Mbit/s over the measured window, loaded pings). Errors with a user-readable message.
fn phase(
    ph: Phase,
    worker: fn(&AtomicU64, &AtomicBool),
    progress: &(dyn Fn(Phase, f64) + Sync),
    cancel: &AtomicBool,
) -> Result<(f64, Vec<f64>), String> {
    let bytes = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let workers: Vec<_> = (0..STREAMS)
        .map(|_| {
            let (b, s) = (bytes.clone(), stop.clone());
            thread::spawn(move || worker(&b, &s))
        })
        .collect();
    let pinger = {
        let s = stop.clone();
        thread::spawn(move || {
            // start after the ramp-up second so the line is already busy
            thread::sleep(Duration::from_secs_f64(WARM_UP));
            pings(usize::MAX, Duration::from_millis(200), &s)
        })
    };
    let start = Instant::now();
    let (mut t0, mut t1, mut b0, mut b1) = (0.0, 0.0, 0u64, 0u64);
    while start.elapsed().as_secs_f64() < SECONDS && !cancel.load(Relaxed) {
        thread::sleep(Duration::from_millis(250));
        t1 = start.elapsed().as_secs_f64();
        b1 = bytes.load(Relaxed);
        if t0 == 0.0 && t1 >= WARM_UP {
            // the first second is ramp-up, not line speed
            (t0, b0) = (t1, b1);
        } else if t0 > 0.0 {
            progress(ph, mbps(b1 - b0, t1 - t0));
        }
    }
    stop.store(true, Relaxed);
    for w in workers {
        let _ = w.join();
    }
    let loaded = pinger.join().unwrap_or_default();
    if cancel.load(Relaxed) {
        return Err("Stopped.".into());
    }
    if b1 == 0 {
        return Err("Couldn't reach the speed test server.".into());
    }
    Ok((mbps(b1 - b0, t1 - t0), loaded))
}

/// Blocks for about 20 s. `progress` gets (phase, live Mbit/s); set `cancel` to stop early.
pub fn run_test(progress: &(dyn Fn(Phase, f64) + Sync), cancel: &AtomicBool) -> Result<SpeedResult, String> {
    progress(Phase::Ping, 0.0);
    let idle = pings(10, Duration::from_millis(100), cancel);
    if cancel.load(Relaxed) {
        return Err("Stopped.".into());
    }
    if idle.is_empty() {
        return Err("No internet connection.".into());
    }
    let (down, down_pings) = phase(Phase::Download, download, progress, cancel)?;
    let (up, up_pings) = phase(Phase::Upload, upload, progress, cancel)?;
    Ok(SpeedResult {
        down_mbps: down,
        up_mbps: up,
        ping_ms: median(&idle),
        jitter_ms: jitter(&idle),
        // the worse direction: home routers usually bloat on upload first
        loaded_ms: median(&down_pings).max(median(&up_pings)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_matches_csharp() {
        assert_eq!(mbps(12_500_000, 1.0), 100.0);
        assert_eq!(mbps(1, 0.0), 0.0);
        assert_eq!(median(&[]), 0.0);
        assert_eq!(median(&[30.0, 10.0, 20.0]), 20.0);
        assert_eq!(median(&[10.0, 20.0, 30.0, 40.0]), 30.0); // upper middle
        assert_eq!(jitter(&[10.0]), 0.0);
        assert_eq!(jitter(&[10.0, 14.0, 12.0]), 3.0);
    }

    #[test]
    fn last_result_round_trip_and_bad_files() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(load_last(d.path()), None, "missing file");
        let r = SpeedResult { down_mbps: 320.5, up_mbps: 41.0, ping_ms: 12.0, jitter_ms: 1.2, loaded_ms: 18.0 };
        let last = LastSpeed { at: "2026-11-21 14:32".into(), result: r, down: vec![100.0, 320.5], up: vec![41.0] };
        save_last(d.path(), &last).unwrap();
        assert_eq!(load_last(d.path()), Some(last.clone()));
        let file = d.path().join(LAST_FILE);
        for bad in ["{ not json", "", r#"{"at":"x"}"#, r#"{"at":"x","result":{"down_mbps":-1,"up_mbps":0,"ping_ms":0,"jitter_ms":0,"loaded_ms":0},"down":[],"up":[]}"#] {
            std::fs::write(&file, bad).unwrap();
            assert_eq!(load_last(d.path()), None, "{bad}");
        }
        let mut long = last;
        long.down = vec![1.0; 500];
        save_last(d.path(), &long).unwrap();
        assert_eq!(load_last(d.path()).unwrap().down.len(), MAX_SAMPLES);
    }

    #[test]
    fn stopped_before_start_returns_quickly() {
        let cancel = AtomicBool::new(true);
        assert_eq!(run_test(&|_, _| {}, &cancel), Err("Stopped.".into()));
    }
}
