//! Gaming session history (port of C# `SessionTracker`): one summary per stretch of "a Game Hub game is running",
//! stored newest first in `state.json` under `Sessions` (max 50, sessions under 1 minute are dropped).
//! Sensor samples and heat warnings are pushed in by the caller; FPS fields stay empty (no frame capture in Rust yet).

use crate::error::Result;
use crate::store::Store;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

pub const KEEP_SESSIONS: usize = 50;
pub const MIN_LENGTH: Duration = Duration::from_secs(60);

/// Same JSON shape as C# `GameSession`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct Session {
    pub start: String,
    pub minutes: f64,
    pub games: String,
    pub cpu_avg_temp: Option<f64>,
    pub cpu_max_temp: Option<f64>,
    pub gpu_avg_temp: Option<f64>,
    pub gpu_max_temp: Option<f64>,
    pub cpu_avg_load: Option<f64>,
    pub gpu_avg_load: Option<f64>,
    pub heat_warnings: i32,
    pub avg_fps: Option<f64>,
    pub low1_fps: Option<f64>,
    pub low01_fps: Option<f64>,
    pub stutters: Option<i32>,
    pub fps_graph: Vec<i32>,
}

/// One sensor reading; `None` = sensor not available.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sample {
    pub cpu_temp: Option<f32>,
    pub gpu_temp: Option<f32>,
    pub cpu_load: Option<f32>,
    pub gpu_load: Option<f32>,
}

#[derive(Default)]
struct Avg {
    sum: f64,
    n: u32,
    max: f64,
}
impl Avg {
    fn add(&mut self, v: Option<f32>) {
        if let Some(v) = v {
            let v = v as f64;
            self.sum += v;
            self.n += 1;
            self.max = self.max.max(v);
        }
    }
    // round_ties_even = C# Math.Round default
    fn avg(&self) -> Option<f64> {
        (self.n > 0).then(|| (self.sum / self.n as f64).round_ties_even())
    }
    fn peak(&self) -> Option<f64> {
        (self.n > 0).then(|| self.max.round_ties_even())
    }
}

#[derive(Default)]
struct Open {
    start: Option<(Instant, String)>,
    games: Vec<String>,
    cpu_temp: Avg,
    gpu_temp: Avg,
    cpu_load: Avg,
    gpu_load: Avg,
    warnings: i32,
}

#[derive(Default)]
pub struct SessionTracker {
    open: Open,
}

impl SessionTracker {
    pub fn new() -> SessionTracker {
        SessionTracker::default()
    }

    pub fn is_open(&self) -> bool {
        self.open.start.is_some()
    }

    /// Call when the set of running game ids changed (and once at start-up). Returns the session that was just
    /// saved, if one ended and was long enough.
    pub fn on_running_changed(&mut self, store: &mut Store, running: &BTreeSet<String>) -> Result<Option<Session>> {
        self.running_changed_at(store, running, Instant::now())
    }

    fn running_changed_at(&mut self, store: &mut Store, running: &BTreeSet<String>, now: Instant) -> Result<Option<Session>> {
        if !running.is_empty() {
            if self.open.start.is_none() {
                self.open.start = Some((now, crate::util::local_iso_now()));
            }
            let games = store.top("Games").and_then(Value::as_array);
            for id in running {
                let name = games
                    .and_then(|g| g.iter().find(|g| g.get("Id").and_then(Value::as_str) == Some(id)))
                    .and_then(|g| g.get("Name").and_then(Value::as_str));
                if let Some(n) = name {
                    if !self.open.games.iter().any(|x| x == n) {
                        self.open.games.push(n.to_string());
                    }
                }
            }
            return Ok(None);
        }
        let Some((t0, start)) = self.open.start.take() else { return Ok(None) };
        let o = std::mem::take(&mut self.open);
        let length = now.saturating_duration_since(t0);
        if length < MIN_LENGTH {
            return Ok(None);
        }
        let session = Session {
            start,
            minutes: (length.as_secs_f64() / 60.0 * 10.0).round_ties_even() / 10.0,
            games: o.games.join(", "),
            cpu_avg_temp: o.cpu_temp.avg(),
            cpu_max_temp: o.cpu_temp.peak(),
            gpu_avg_temp: o.gpu_temp.avg(),
            gpu_max_temp: o.gpu_temp.peak(),
            cpu_avg_load: o.cpu_load.avg(),
            gpu_avg_load: o.gpu_load.avg(),
            heat_warnings: o.warnings,
            ..Default::default()
        };
        let mut list: Vec<Value> = store.top("Sessions").and_then(Value::as_array).cloned().unwrap_or_default();
        list.insert(0, serde_json::to_value(&session).expect("session serializes"));
        list.truncate(KEEP_SESSIONS);
        store.set_top("Sessions", Value::Array(list));
        store.save()?;
        Ok(Some(session))
    }

    /// A sensor reading; ignored while no session is open.
    pub fn on_sample(&mut self, s: Sample) {
        if self.open.start.is_none() {
            return;
        }
        self.open.cpu_temp.add(s.cpu_temp);
        self.open.gpu_temp.add(s.gpu_temp);
        self.open.cpu_load.add(s.cpu_load);
        self.open.gpu_load.add(s.gpu_load);
    }

    /// Counts a heat warning in the open session (ignored when none is open).
    pub fn note_heat_warning(&mut self) {
        if self.open.start.is_some() {
            self.open.warnings += 1;
        }
    }
}

/// Wipes the history (C# `Clear`, no confirmation there either).
pub fn clear(store: &mut Store) -> Result<()> {
    store.set_top("Sessions", Value::Array(vec![]));
    store.save()
}

pub fn duration(minutes: f64) -> String {
    if minutes >= 60.0 {
        format!("{} h {} min", (minutes / 60.0) as i64, (minutes % 60.0) as i64)
    } else {
        format!("{} min", minutes.round() as i64)
    }
}

/// One-line summary, same text as the C# balloon.
pub fn describe(s: &Session) -> String {
    let r = |v: f64| v.round() as i64;
    let mut parts = vec![duration(s.minutes)];
    if let (Some(a), Some(m)) = (s.cpu_avg_temp, s.cpu_max_temp) {
        parts.push(format!("CPU {}°C avg, {}°C peak", r(a), r(m)));
    }
    if let (Some(a), Some(m)) = (s.gpu_avg_temp, s.gpu_max_temp) {
        parts.push(format!("GPU {}°C avg, {}°C peak", r(a), r(m)));
    }
    if let Some(f) = s.avg_fps {
        parts.push(format!("{} FPS avg, 1% low {}", r(f), r(s.low1_fps.unwrap_or(0.0))));
    }
    parts.join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        let mut s = Store::load(d.path());
        s.set_top("Games", json!([{"Id":"a","Name":"Alpha"},{"Id":"b","Name":"Beta"}]));
        (d, s)
    }
    fn ids(v: &[&str]) -> BTreeSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }
    fn sessions(s: &Store) -> Vec<Value> {
        s.top("Sessions").and_then(Value::as_array).cloned().unwrap_or_default()
    }

    #[test]
    fn records_session_with_averages() {
        let (_d, mut st) = store();
        let mut t = SessionTracker::new();
        let t0 = Instant::now();
        t.running_changed_at(&mut st, &ids(&["a"]), t0).unwrap();
        t.running_changed_at(&mut st, &ids(&["a", "b"]), t0 + Duration::from_secs(30)).unwrap(); // second game joins
        t.on_sample(Sample { cpu_temp: Some(60.0), gpu_temp: None, cpu_load: Some(20.0), gpu_load: Some(90.0) });
        t.on_sample(Sample { cpu_temp: Some(71.0), gpu_temp: None, cpu_load: Some(30.0), gpu_load: Some(99.0) });
        t.note_heat_warning();
        let s = t.running_changed_at(&mut st, &ids(&[]), t0 + Duration::from_secs(5 * 60 + 4)).unwrap().unwrap();
        assert_eq!(s.games, "Alpha, Beta");
        assert_eq!(s.minutes, 5.1);
        assert_eq!(s.cpu_avg_temp, Some(66.0)); // 65.5 ties-to-even -> 66
        assert_eq!(s.cpu_max_temp, Some(71.0));
        assert_eq!(s.gpu_avg_temp, None);
        assert_eq!(s.cpu_avg_load, Some(25.0));
        assert_eq!(s.heat_warnings, 1);
        let v = &sessions(&st)[0];
        assert_eq!(v["Games"], "Alpha, Beta");
        assert!(v["GpuAvgTemp"].is_null() && v["AvgFps"].is_null() && v["FpsGraph"] == json!([]));
        assert!(!t.is_open());
        assert_eq!(describe(&s), "5 min · CPU 66°C avg, 71°C peak");
    }

    #[test]
    fn short_session_dropped_and_counters_reset() {
        let (_d, mut st) = store();
        let mut t = SessionTracker::new();
        let t0 = Instant::now();
        t.running_changed_at(&mut st, &ids(&["a"]), t0).unwrap();
        t.note_heat_warning();
        assert!(t.running_changed_at(&mut st, &ids(&[]), t0 + Duration::from_secs(59)).unwrap().is_none());
        assert!(sessions(&st).is_empty());
        t.running_changed_at(&mut st, &ids(&["a"]), t0 + Duration::from_secs(100)).unwrap();
        let s = t.running_changed_at(&mut st, &ids(&[]), t0 + Duration::from_secs(200)).unwrap().unwrap();
        assert_eq!(s.heat_warnings, 0);
    }

    #[test]
    fn samples_and_warnings_ignored_when_closed() {
        let (_d, mut st) = store();
        let mut t = SessionTracker::new();
        t.on_sample(Sample { cpu_temp: Some(99.0), ..Default::default() });
        t.note_heat_warning();
        assert!(t.running_changed_at(&mut st, &ids(&[]), Instant::now()).unwrap().is_none());
    }

    #[test]
    fn keeps_newest_50_and_unknown_fields_of_old_entries() {
        let (_d, mut st) = store();
        let old: Vec<Value> = (0..50).map(|i| json!({"Start":"x","Minutes":i,"Extra":"keep"})).collect();
        st.set_top("Sessions", Value::Array(old));
        let mut t = SessionTracker::new();
        let t0 = Instant::now();
        t.running_changed_at(&mut st, &ids(&["a"]), t0).unwrap();
        t.running_changed_at(&mut st, &ids(&[]), t0 + Duration::from_secs(120)).unwrap();
        let l = sessions(&st);
        assert_eq!(l.len(), 50);
        assert_eq!(l[0]["Minutes"], 2.0);
        assert_eq!(l[1]["Extra"], "keep");
        assert_eq!(l[49]["Minutes"], 48); // oldest (49) fell off
    }

    #[test]
    fn saved_to_disk_and_clear() {
        let (d, mut st) = store();
        let mut t = SessionTracker::new();
        let t0 = Instant::now();
        t.running_changed_at(&mut st, &ids(&["b"]), t0).unwrap();
        t.running_changed_at(&mut st, &ids(&[]), t0 + Duration::from_secs(90)).unwrap();
        assert_eq!(sessions(&Store::load(d.path())).len(), 1);
        clear(&mut st).unwrap();
        assert!(sessions(&Store::load(d.path())).is_empty());
    }

    #[test]
    fn duration_text() {
        assert_eq!(duration(5.4), "5 min");
        assert_eq!(duration(125.0), "2 h 5 min");
    }
}
