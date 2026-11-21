//! Disk and network rates for the stats panel beside Search. Exists only while that panel is open.
use crate::pdh::{Counter, Query};

/// `None` = counter missing or no second sample yet (the UI shows a dash).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct IoNow {
    pub disk_active: Option<f32>,
    pub disk_bytes: Option<f64>,
    pub down_bytes: Option<f64>,
    pub up_bytes: Option<f64>,
}

pub struct IoSampler {
    q: Query,
    idle: Option<Counter>,
    disk: Option<Counter>,
    down: Option<Counter>,
    up: Option<Counter>,
}

impl IoSampler {
    pub fn new() -> Option<IoSampler> {
        let q = Query::new()?;
        let (idle, disk) = (q.add(r"\PhysicalDisk(*)\% Idle Time"), q.add(r"\PhysicalDisk(*)\Disk Bytes/sec"));
        let (down, up) = (q.add(r"\Network Interface(*)\Bytes Received/sec"), q.add(r"\Network Interface(*)\Bytes Sent/sec"));
        q.collect(); // rates need two samples
        Some(IoSampler { q, idle, disk, down, up })
    }

    pub fn read(&self) -> IoNow {
        if !self.q.collect() {
            return IoNow::default();
        }
        let total = |c: &Option<Counter>| c.as_ref().and_then(|c| c.array().into_iter().find(|(n, _)| n == "_Total").map(|(_, v)| v));
        // shortcut: a VPN adapter counts its traffic twice with the real one; filter adapters if that misleads
        let sum = |c: &Option<Counter>| c.as_ref().map(Counter::array).filter(|a| !a.is_empty()).map(|a| a.iter().map(|(_, v)| v).sum());
        IoNow {
            disk_active: total(&self.idle).map(|i| (100.0 - i).clamp(0.0, 100.0) as f32),
            disk_bytes: total(&self.disk),
            down_bytes: sum(&self.down),
            up_bytes: sum(&self.up),
        }
    }
}
