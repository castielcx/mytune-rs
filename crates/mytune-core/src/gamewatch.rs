//! Game watcher + service loop (port of C# `GameService.Tick`, `CpuTopology`, and the start/stop wiring).
//!
//! Every tick (first after 2 s, then every 3 s) it takes ONE process list, matches the Game Hub games in
//! `state.json` (`Games[].ExePath`, matched by exe name without extension, case-insensitive), applies the
//! P-core affinity to games with `PCoresOnly`, adds play time, and when the SET of running games changes saves
//! `state.json` and runs the booster.
//!
//! Affinity is tried once per pid (the pid is marked handled first), so anti-cheat games that refuse
//! ("Access is denied") are logged once and not retried until that process exits.
//!
//! Only one watcher/booster may run: the C# app would write `state.json` too. `run()` never starts it
//! if you also run the C# MyTune.

use crate::booster::Booster;
use crate::error::{Error, Result};
use crate::log;
use crate::procs::Procs;
use crate::store::Store;
use serde_json::Value;
use std::collections::{BTreeSet, HashSet};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::Win32::System::SystemInformation::{GetLogicalProcessorInformationEx, RelationCache, RelationProcessorCore, LOGICAL_PROCESSOR_RELATIONSHIP, SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX};

// ---------------------------------------------------------------- CPU topology

/// Which logical processors are "performance" ones. Only processor group 0 is used, like C#.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CpuTopology {
    pub all_mask: u64,
    pub performance_mask: u64,
    pub is_hybrid: bool,
    pub cores: u32,
    pub p_cores: u32,
    /// Mask of the V-Cache chiplet on dual-CCD X3D parts (largest L3 when L3 sizes differ), else 0.
    pub vcache_mask: u64,
}

fn walk(buf: &[u8], mut each: impl FnMut(&[u8])) {
    let mut off = 0usize;
    while off + 8 <= buf.len() {
        let size = i32::from_le_bytes(buf[off + 4..off + 8].try_into().unwrap()) as usize;
        if size < 8 || off + size > buf.len() {
            break;
        }
        each(&buf[off..off + size]);
        off += size;
    }
}

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes(b[o..o + 2].try_into().unwrap())
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

impl CpuTopology {
    /// Pure parser over the two `GetLogicalProcessorInformationEx` buffers (offsets as in the Windows SDK).
    pub fn parse(core_buf: &[u8], cache_buf: &[u8], logical_processors: u32) -> CpuTopology {
        let mut cores: Vec<(u8, u64)> = Vec::new(); // (efficiency class, mask)
        walk(core_buf, |e| {
            if e.len() >= 42 && u16_at(e, 30) >= 1 && u16_at(e, 40) == 0 {
                cores.push((e[9], u64_at(e, 32)));
            }
        });
        let mut l3: Vec<(u32, u64)> = Vec::new(); // (size, mask)
        walk(cache_buf, |e| {
            if e.len() >= 50 && e[8] == 3 && u16_at(e, 38) >= 1 && u16_at(e, 48) == 0 {
                l3.push((u32_at(e, 12), u64_at(e, 40)));
            }
        });
        let vcache_mask = if l3.len() >= 2 && l3.iter().any(|c| c.0 != l3[0].0) { l3.iter().max_by_key(|c| c.0).map(|c| c.1).unwrap_or(0) } else { 0 };

        let mut t = CpuTopology { vcache_mask, ..Default::default() };
        if cores.is_empty() {
            t.all_mask = if logical_processors >= 64 { u64::MAX } else { (1u64 << logical_processors) - 1 };
            t.performance_mask = t.all_mask;
        } else {
            t.all_mask = cores.iter().fold(0, |a, c| a | c.1);
            let max_eff = cores.iter().map(|c| c.0).max().unwrap();
            t.performance_mask = cores.iter().filter(|c| c.0 == max_eff).fold(0, |a, c| a | c.1);
            t.is_hybrid = cores.iter().any(|c| c.0 < max_eff);
            t.cores = cores.len() as u32;
            t.p_cores = cores.iter().filter(|c| c.0 == max_eff).count() as u32;
        }
        if vcache_mask != 0 {
            t.performance_mask = vcache_mask & t.all_mask;
            t.p_cores = cores.iter().filter(|c| c.1 & t.performance_mask != 0).count() as u32;
            t.is_hybrid = t.performance_mask != 0 && t.performance_mask != t.all_mask;
        }
        t
    }

    pub fn detect() -> CpuTopology {
        fn read(rel: LOGICAL_PROCESSOR_RELATIONSHIP) -> Vec<u8> {
            let mut len = 0u32;
            unsafe {
                let _ = GetLogicalProcessorInformationEx(rel, None, &mut len); // fails on purpose: asks for the size
                if len == 0 {
                    return vec![];
                }
                let mut buf = vec![0u8; len as usize];
                if GetLogicalProcessorInformationEx(rel, Some(buf.as_mut_ptr() as *mut SYSTEM_LOGICAL_PROCESSOR_INFORMATION_EX), &mut len).is_err() {
                    return vec![];
                }
                buf.truncate(len as usize);
                buf
            }
        }
        let n = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1);
        CpuTopology::parse(&read(RelationProcessorCore), &read(RelationCache), n)
    }

    /// Affinity mask for a game: the performance cores, or everything.
    pub fn mask_for(&self, p_cores_only: bool) -> u64 {
        if p_cores_only && self.performance_mask != 0 {
            self.performance_mask
        } else {
            self.all_mask
        }
    }
}

// ---------------------------------------------------------------- watcher

pub struct GameWatcher {
    procs: Arc<dyn Procs>,
    topo: CpuTopology,
    handled: HashSet<u32>,
    running: BTreeSet<String>,
    last_tick: Option<Instant>,
}

impl GameWatcher {
    pub fn new(procs: Arc<dyn Procs>, topo: CpuTopology) -> GameWatcher {
        GameWatcher { procs, topo, handled: HashSet::new(), running: BTreeSet::new(), last_tick: None }
    }

    /// Ids of games that have a live process (as of the last tick).
    pub fn running(&self) -> &BTreeSet<String> {
        &self.running
    }

    /// Forget which pids already got an affinity (call when a game's `PCoresOnly` changes).
    pub fn reset_handled(&mut self) {
        self.handled.clear();
    }

    /// One watcher pass. Returns true when the set of running games changed (state already saved then).
    pub fn tick(&mut self, store: &mut Store) -> Result<bool> {
        let list = self.procs.list()?;
        let mut games: Vec<Value> = store.top("Games").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut running = BTreeSet::new();
        let mut alive = HashSet::new();
        let mut running_idx = Vec::new();

        for (i, g) in games.iter().enumerate() {
            let Some(exe_path) = g.get("ExePath").and_then(Value::as_str).filter(|p| !p.is_empty()) else { continue };
            let Some(stem) = Path::new(exe_path).file_stem().and_then(|s| s.to_str()) else { continue };
            let pids: Vec<u32> = list.iter().filter(|p| p.name.eq_ignore_ascii_case(stem)).map(|p| p.pid).collect();
            if pids.is_empty() {
                continue;
            }
            let Some(id) = g.get("Id").and_then(Value::as_str) else { continue };
            running.insert(id.to_string());
            running_idx.push(i);
            let p_cores = g.get("PCoresOnly").and_then(Value::as_bool) == Some(true);
            for pid in pids {
                alive.insert(pid);
                if p_cores && self.handled.insert(pid) {
                    let mask = self.topo.mask_for(true);
                    match self.procs.set_affinity(pid, mask) {
                        Ok(()) => log::info(&format!("affinity {stem} pid {pid} -> {mask:#x}")),
                        Err(e) => log::error(&format!("affinity {stem}"), Some(&e)), // anti-cheat: logged once per pid
                    }
                }
            }
        }
        self.handled.retain(|p| alive.contains(p));

        let now = Instant::now();
        let seconds = self.last_tick.map(|t| now.duration_since(t).as_secs_f64().min(10.0)).unwrap_or(0.0);
        self.last_tick = Some(now);
        if !running_idx.is_empty() {
            for i in &running_idx {
                if let Some(obj) = games[*i].as_object_mut() {
                    let played = obj.get("PlaySeconds").and_then(Value::as_i64).unwrap_or(0) + seconds.round() as i64;
                    obj.insert("PlaySeconds".into(), Value::from(played));
                    obj.insert("LastPlayed".into(), Value::String(crate::util::local_iso_now()));
                }
            }
            store.set_top("Games", Value::Array(games));
        }

        if running != self.running {
            self.running = running;
            store.save()?;
            return Ok(true);
        }
        Ok(false)
    }
}

// ---------------------------------------------------------------- service

pub const FIRST_TICK: Duration = Duration::from_secs(2);
pub const TICK: Duration = Duration::from_secs(3);

/// Watcher + booster + store, run until `stop` is set. This is the "running part" of the game booster.
pub struct GameService {
    pub store: Store,
    pub watcher: GameWatcher,
    pub booster: Booster,
    /// Session history; sensor samples / heat warnings are pushed in by whoever owns the sensors.
    pub sessions: Arc<std::sync::Mutex<crate::session::SessionTracker>>,
    /// Heat warning state; fed by the sensor thread, `enabled` / `gaming` kept current by `tick`.
    pub heat: Arc<std::sync::Mutex<crate::heatwatch::HeatWatch>>,
    /// Called with "any game running" whenever the running set changes (the sensor cadence hangs off this).
    pub on_gaming: Option<Box<dyn FnMut(bool) + Send>>,
    seen_mtime: Option<std::time::SystemTime>,
}

impl GameService {
    pub fn new(store: Store, watcher: GameWatcher, booster: Booster) -> GameService {
        GameService { store, watcher, booster, sessions: Default::default(), heat: Default::default(), on_gaming: None, seen_mtime: None }
    }

    fn mtime(&self) -> Option<std::time::SystemTime> {
        std::fs::metadata(self.store.dir().join("state.json")).and_then(|m| m.modified()).ok()
    }

    /// Crash recovery: undo what a killed run left behind.
    pub fn start(&mut self) -> Vec<String> {
        let f = self.booster.start(&mut self.store);
        self.seen_mtime = self.mtime();
        f
    }

    /// One pass. Picks up settings edited on disk (from the C# settings UI or by hand), runs the watcher and
    /// updates the booster when the running set or the settings changed.
    pub fn tick(&mut self) -> Result<()> {
        let mut force = false;
        if self.mtime() != self.seen_mtime {
            self.store = Store::load(self.store.dir()); // settings changed behind our back
            force = true;
        }
        let changed = self.watcher.tick(&mut self.store)?;
        {
            let mut h = self.heat.lock().unwrap();
            h.enabled = self.store.settings_flag("HeatWarning").unwrap_or(true);
            h.gaming = !self.watcher.running().is_empty();
        }
        if changed {
            if let Some(cb) = self.on_gaming.as_mut() {
                cb(!self.watcher.running().is_empty());
            }
            let r = self.sessions.lock().unwrap().on_running_changed(&mut self.store, self.watcher.running());
            match r {
                Ok(Some(s)) => log::info(&format!("session: {}", crate::session::describe(&s))),
                Ok(None) => {}
                Err(e) => log::error("session save", Some(&e)),
            }
        }
        if changed || force {
            let gaming = !self.watcher.running().is_empty();
            for f in self.booster.update(&mut self.store, gaming) {
                log::error(&format!("booster update: {f}"), None);
            }
        }
        self.booster.periodic();
        self.seen_mtime = self.mtime();
        Ok(())
    }

    /// Blocks until `stop`; everything is put back on the way out.
    pub fn run(&mut self, stop: &AtomicBool) -> Result<()> {
        for f in self.start() {
            log::error(&format!("booster start: {f}"), None);
        }
        let sleep = |d: Duration, stop: &AtomicBool| {
            let end = Instant::now() + d;
            while Instant::now() < end && !stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(100));
            }
        };
        sleep(FIRST_TICK, stop);
        while !stop.load(Ordering::SeqCst) {
            if let Err(e) = self.tick() {
                log::error("game watcher", Some(&e));
            }
            sleep(TICK, stop);
        }
        let failures = self.booster.stop(&mut self.store);
        if failures.is_empty() {
            Ok(())
        } else {
            Err(Error::Revert(failures))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procs::fake::FakeProcs;
    use crate::procs::ProcInfo;

    fn core_entry(eff: u8, mask: u64) -> Vec<u8> {
        let mut e = vec![0u8; 48];
        e[4..8].copy_from_slice(&48i32.to_le_bytes());
        e[9] = eff;
        e[30..32].copy_from_slice(&1u16.to_le_bytes());
        e[32..40].copy_from_slice(&mask.to_le_bytes());
        e
    }

    fn l3_entry(size: u32, mask: u64) -> Vec<u8> {
        let mut e = vec![0u8; 56];
        e[4..8].copy_from_slice(&56i32.to_le_bytes());
        e[8] = 3;
        e[12..16].copy_from_slice(&size.to_le_bytes());
        e[38..40].copy_from_slice(&1u16.to_le_bytes());
        e[40..48].copy_from_slice(&mask.to_le_bytes());
        e
    }

    fn cat(parts: Vec<Vec<u8>>) -> Vec<u8> {
        parts.concat()
    }

    #[test]
    fn topology_hybrid_pcores_are_the_highest_efficiency_class() {
        // 2 P-cores (class 1, 2 threads each) + 2 E-cores (class 0)
        let cores = cat(vec![core_entry(1, 0b0011), core_entry(1, 0b1100), core_entry(0, 0b0001_0000), core_entry(0, 0b0010_0000)]);
        let t = CpuTopology::parse(&cores, &[], 8);
        assert_eq!((t.all_mask, t.performance_mask), (0b11_1111, 0b1111));
        assert!(t.is_hybrid);
        assert_eq!((t.cores, t.p_cores), (4, 2));
        assert_eq!(t.mask_for(true), 0b1111);
        assert_eq!(t.mask_for(false), 0b11_1111);
    }

    #[test]
    fn topology_non_hybrid_uses_everything() {
        let cores = cat(vec![core_entry(0, 0b11), core_entry(0, 0b1100)]);
        let t = CpuTopology::parse(&cores, &[], 4);
        assert!(!t.is_hybrid);
        assert_eq!(t.performance_mask, t.all_mask);
    }

    #[test]
    fn topology_vcache_chiplet_wins_over_efficiency_classes() {
        let cores = cat(vec![core_entry(0, 0b0011), core_entry(0, 0b1100), core_entry(0, 0b0011_0000), core_entry(0, 0b1100_0000)]);
        let cache = cat(vec![l3_entry(32 << 20, 0b0000_1111), l3_entry(96 << 20, 0b1111_0000)]);
        let t = CpuTopology::parse(&cores, &cache, 8);
        assert_eq!(t.vcache_mask, 0b1111_0000);
        assert_eq!(t.performance_mask, 0b1111_0000);
        assert!(t.is_hybrid);
        assert_eq!(t.p_cores, 2);
        // equal L3 sizes: no V-Cache chiplet
        let same = cat(vec![l3_entry(32 << 20, 0b1111), l3_entry(32 << 20, 0b1111_0000)]);
        assert_eq!(CpuTopology::parse(&cores, &same, 8).vcache_mask, 0);
    }

    #[test]
    fn topology_without_data_falls_back_to_processor_count() {
        assert_eq!(CpuTopology::parse(&[], &[], 8).all_mask, 0xFF);
        assert_eq!(CpuTopology::parse(&[], &[], 64).all_mask, u64::MAX);
        assert_eq!(CpuTopology::parse(&[0xFF; 3], &[], 4).all_mask, 0xF, "garbage buffer is ignored");
    }

    const GAMES: &str = r#"{"Games":[
        {"Id":"g1","Name":"One","ExePath":"C:\\Games\\One\\One.exe","PCoresOnly":true,"PlaySeconds":100,"Favorite":true},
        {"Id":"g2","Name":"Two","ExePath":"D:\\Two.EXE","PCoresOnly":false},
        {"Id":"g3","Name":"Uri only","ExePath":null,"LaunchUri":"steam://run/1"}]}"#;

    fn store() -> (tempfile::TempDir, Store) {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("state.json"), GAMES).unwrap();
        let s = Store::load(d.path());
        (d, s)
    }

    fn topo() -> CpuTopology {
        CpuTopology { all_mask: 0xFF, performance_mask: 0x0F, is_hybrid: true, cores: 8, p_cores: 4, vcache_mask: 0 }
    }

    #[test]
    fn watcher_detects_games_sets_affinity_once_and_tracks_changes() {
        let (d, mut st) = store();
        let procs = FakeProcs::new(&[(10, "one"), (11, "ONE"), (20, "two"), (30, "notepad")]);
        let mut w = GameWatcher::new(procs.clone(), topo());
        assert!(w.tick(&mut st).unwrap(), "first sighting is a change");
        assert_eq!(w.running().iter().cloned().collect::<Vec<_>>(), ["g1", "g2"]);
        let calls = procs.calls.lock().unwrap().clone();
        assert_eq!(calls, ["affinity 10 0xf", "affinity 11 0xf"], "g1 has PCoresOnly, g2 not");
        assert!(!w.tick(&mut st).unwrap(), "same set: no change");
        assert_eq!(procs.calls.lock().unwrap().len(), 2, "affinity not retried");
        // saved on change, with the C# fields kept
        let disk = Store::load(d.path());
        let g1 = &disk.top("Games").unwrap()[0];
        assert_eq!(g1["Favorite"], true);
        assert!(g1["LastPlayed"].as_str().unwrap().contains('T'));
        // game 1 exits, a new pid appears for game 2
        procs.list.lock().unwrap().retain(|p| p.name.to_lowercase() != "one");
        assert!(w.tick(&mut st).unwrap());
        assert_eq!(w.running().iter().cloned().collect::<Vec<_>>(), ["g2"]);
    }

    #[test]
    fn watcher_play_time_adds_up_and_never_loses_other_games() {
        let (_d, mut st) = store();
        let procs = FakeProcs::new(&[(10, "One")]);
        let mut w = GameWatcher::new(procs, topo());
        w.tick(&mut st).unwrap(); // first tick adds 0 s
        assert_eq!(st.top("Games").unwrap()[0]["PlaySeconds"], 100);
        std::thread::sleep(Duration::from_millis(1200));
        w.tick(&mut st).unwrap();
        assert_eq!(st.top("Games").unwrap()[0]["PlaySeconds"], 101);
        assert!(st.top("Games").unwrap()[2]["LaunchUri"].is_string(), "URI-only game untouched");
        assert_eq!(st.top("Games").unwrap().as_array().unwrap().len(), 3);
    }

    #[test]
    fn affinity_failure_is_tried_once_per_pid() {
        struct Denied(Arc<FakeProcs>);
        impl Procs for Denied {
            fn list(&self) -> Result<Vec<ProcInfo>> {
                self.0.list()
            }
            fn throttle(&self, p: u32) -> Result<Option<crate::procs::Held>> {
                self.0.throttle(p)
            }
            fn restore(&self, p: u32, h: &crate::procs::Held) -> Result<()> {
                self.0.restore(p, h)
            }
            fn set_affinity(&self, pid: u32, mask: u64) -> Result<()> {
                self.0.calls.lock().unwrap().push(format!("affinity {pid} {mask:#x}"));
                Err(Error::Os("Access is denied".into()))
            }
        }
        let inner = FakeProcs::new(&[(10, "One")]);
        let (_d, mut st) = store();
        let mut w = GameWatcher::new(Arc::new(Denied(inner.clone())), topo());
        w.tick(&mut st).unwrap();
        w.tick(&mut st).unwrap();
        w.tick(&mut st).unwrap();
        assert_eq!(inner.calls.lock().unwrap().len(), 1, "anti-cheat refusal is not retried every tick");
        w.reset_handled();
        w.tick(&mut st).unwrap();
        assert_eq!(inner.calls.lock().unwrap().len(), 2);
    }
}
