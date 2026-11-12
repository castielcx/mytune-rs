//! Heat warning (port of C# `HeatWatch`): CPU >= 95 or GPU >= 88 deg C for 3 samples in a row while a Game Hub
//! game runs, at most one warning per 5 minutes. Pure logic; the caller shows/logs the returned text.

use std::time::{Duration, Instant};

pub const CPU_LIMIT: f32 = 95.0;
pub const GPU_LIMIT: f32 = 88.0;
pub const SAMPLES: u32 = 3;
pub const COOLDOWN: Duration = Duration::from_secs(5 * 60);

pub struct HeatWatch {
    /// `Settings.HeatWarning` (C# default: on).
    pub enabled: bool,
    pub gaming: bool,
    hot: u32,
    last: Option<Instant>,
}

impl Default for HeatWatch {
    fn default() -> Self {
        HeatWatch { enabled: true, gaming: false, hot: 0, last: None }
    }
}

impl HeatWatch {
    pub fn on_sample(&mut self, cpu: Option<f32>, gpu: Option<f32>) -> Option<String> {
        self.sample_at(cpu, gpu, Instant::now())
    }

    fn sample_at(&mut self, cpu_t: Option<f32>, gpu_t: Option<f32>, now: Instant) -> Option<String> {
        if !self.enabled || !self.gaming {
            self.hot = 0;
            return None;
        }
        let cpu = cpu_t.is_some_and(|t| t >= CPU_LIMIT); // a missing sensor is never hot
        let gpu = gpu_t.is_some_and(|t| t >= GPU_LIMIT);
        self.hot = if cpu || gpu { self.hot + 1 } else { 0 };
        if self.hot < SAMPLES || self.last.is_some_and(|l| now.duration_since(l) < COOLDOWN) {
            return None;
        }
        self.last = Some(now);
        let (c, g) = (cpu_t.unwrap_or(0.0).round() as i64, gpu_t.unwrap_or(0.0).round() as i64);
        Some(match (cpu, gpu) {
            (true, true) => format!("CPU {c}°C and GPU {g}°C"),
            (true, false) => format!("CPU at {c}°C"),
            _ => format!("GPU at {g}°C"),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watch() -> HeatWatch {
        HeatWatch { gaming: true, ..Default::default() }
    }

    #[test]
    fn needs_three_hot_samples_then_cools_down() {
        let (mut w, t0) = (watch(), Instant::now());
        assert!(w.sample_at(Some(96.0), None, t0).is_none());
        assert!(w.sample_at(Some(96.0), None, t0).is_none());
        assert_eq!(w.sample_at(Some(96.4), Some(70.0), t0).as_deref(), Some("CPU at 96°C"));
        for _ in 0..5 {
            assert!(w.sample_at(Some(99.0), None, t0 + Duration::from_secs(60)).is_none()); // cooldown
        }
        assert!(w.sample_at(Some(99.0), None, t0 + COOLDOWN).is_some());
    }

    #[test]
    fn cool_sample_resets_and_wording() {
        let (mut w, t0) = (watch(), Instant::now());
        w.sample_at(Some(99.0), None, t0);
        w.sample_at(Some(99.0), None, t0);
        w.sample_at(Some(50.0), Some(50.0), t0); // reset
        w.sample_at(None, Some(90.0), t0);
        w.sample_at(None, Some(90.0), t0);
        assert_eq!(w.sample_at(None, Some(89.0), t0).as_deref(), Some("GPU at 89°C"));
        let mut w = watch();
        for _ in 0..2 {
            w.sample_at(Some(95.0), Some(88.0), t0);
        }
        assert_eq!(w.sample_at(Some(95.0), Some(88.0), t0).as_deref(), Some("CPU 95°C and GPU 88°C"));
    }

    #[test]
    fn off_or_not_gaming_resets() {
        let (mut w, t0) = (watch(), Instant::now());
        w.sample_at(Some(99.0), None, t0);
        w.sample_at(Some(99.0), None, t0);
        w.gaming = false;
        assert!(w.sample_at(Some(99.0), None, t0).is_none());
        w.gaming = true;
        assert!(w.sample_at(Some(99.0), None, t0).is_none()); // counter restarted
        w.enabled = false;
        assert!(w.sample_at(Some(99.0), None, t0).is_none());
    }
}
