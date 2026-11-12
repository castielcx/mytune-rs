//! Event Tracing for Windows, for the two Diagnostics features that need it (both need admin):
//! - `NetTrace`: bytes sent/received per process (Microsoft-Windows-Kernel-Network, events 10/11/42/43).
//! - `latency_scan`: time per driver in interrupt (ISR) and deferred (DPC) routines (NT Kernel Logger).
//!
//! Events are decoded from fixed offsets of the payload instead of through TDH:
//! Kernel-Network: `{u32 PID, u32 size, ...}`. PerfInfo DPC / TimerDPC / ISR: `{u64 InitialTime, u64 Routine, ...}`.

use crate::diagnostics::NetBytes;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use windows::core::{GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::WIN32_ERROR;
use windows::Win32::System::Diagnostics::Etw::{
    CloseTrace, ControlTraceW, EnableTraceEx2, OpenTraceW, ProcessTrace, StartTraceW, CONTROLTRACE_HANDLE, EVENT_CONTROL_CODE_ENABLE_PROVIDER,
    EVENT_RECORD, EVENT_TRACE_CONTROL_STOP, EVENT_TRACE_FLAG, EVENT_TRACE_LOGFILEW, EVENT_TRACE_PROPERTIES, EVENT_TRACE_REAL_TIME_MODE,
    PROCESSTRACE_HANDLE, PROCESS_TRACE_MODE_EVENT_RECORD, PROCESS_TRACE_MODE_RAW_TIMESTAMP, PROCESS_TRACE_MODE_REAL_TIME, WNODE_FLAG_TRACED_GUID,
};

const NET_GUID: GUID = GUID::from_u128(0x7dd42a49_5329_4832_8dfd_43d979153a88); // Microsoft-Windows-Kernel-Network
const PERFINFO_GUID: GUID = GUID::from_u128(0xce1dbfb4_137e_4da6_87b0_3f59aa102cbc);
const SYSTEM_TRACE_CONTROL: GUID = GUID::from_u128(0x9e814aad_3204_11d2_9a82_006008a86939);
const KERNEL_LOGGER: &str = "NT Kernel Logger";
const NET_SESSION: &str = "MyTune-ProcessNet";
const FLAG_DPC: u32 = 0x20;
const FLAG_INTERRUPT: u32 = 0x40;
const ERROR_ALREADY_EXISTS: u32 = 183;
const ERROR_ACCESS_DENIED: u32 = 5;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn describe(code: WIN32_ERROR) -> String {
    match code.0 {
        ERROR_ACCESS_DENIED => "Access denied: MyTune must run as administrator.".into(),
        ERROR_ALREADY_EXISTS => "Another program is already using this trace session.".into(),
        c => format!("Windows error {c}"),
    }
}

/// `EVENT_TRACE_PROPERTIES` followed by room for the session name, 8-byte aligned.
struct Props {
    buf: Vec<u64>,
}

impl Props {
    fn new(name: &str, kernel_flags: Option<u32>, buffer_kb: u32) -> Props {
        let head = std::mem::size_of::<EVENT_TRACE_PROPERTIES>();
        let total = head + (name.len() + 1) * 2 + 8;
        let mut p = Props { buf: vec![0u64; total.div_ceil(8)] };
        let t = p.ptr();
        unsafe {
            (*t).Wnode.BufferSize = (p.buf.len() * 8) as u32;
            (*t).Wnode.Flags = WNODE_FLAG_TRACED_GUID;
            (*t).Wnode.ClientContext = 1; // QueryPerformanceCounter timestamps
            (*t).LogFileMode = EVENT_TRACE_REAL_TIME_MODE;
            (*t).LoggerNameOffset = head as u32;
            (*t).BufferSize = buffer_kb; // KB
            (*t).FlushTimer = 1;
            if let Some(f) = kernel_flags {
                (*t).Wnode.Guid = SYSTEM_TRACE_CONTROL;
                (*t).EnableFlags = EVENT_TRACE_FLAG(f);
            }
        }
        p
    }

    fn ptr(&mut self) -> *mut EVENT_TRACE_PROPERTIES {
        self.buf.as_mut_ptr().cast()
    }
}

struct Session {
    handle: CONTROLTRACE_HANDLE,
    name: Vec<u16>,
    props: Props,
}

impl Session {
    fn start(name: &str, kernel_flags: Option<u32>, buffer_kb: u32) -> Result<Session, String> {
        let name_w = wide(name);
        for attempt in 0..2 {
            let mut props = Props::new(name, kernel_flags, buffer_kb);
            let mut handle = CONTROLTRACE_HANDLE::default();
            let code = unsafe { StartTraceW(&mut handle, PCWSTR(name_w.as_ptr()), props.ptr()) };
            if code.0 == 0 {
                return Ok(Session { handle, name: name_w, props });
            }
            if code.0 == ERROR_ALREADY_EXISTS && attempt == 0 {
                // A session of ours was left behind (crash): stop it and try again.
                let mut stale = Props::new(name, kernel_flags, buffer_kb);
                unsafe { ControlTraceW(CONTROLTRACE_HANDLE::default(), PCWSTR(name_w.as_ptr()), stale.ptr(), EVENT_TRACE_CONTROL_STOP) };
                continue;
            }
            return Err(describe(code));
        }
        Err("The trace session could not be started.".into())
    }

    fn stop(&mut self) {
        unsafe { ControlTraceW(self.handle, PCWSTR(self.name.as_ptr()), self.props.ptr(), EVENT_TRACE_CONTROL_STOP) };
    }

    /// Blocks on the calling thread until the session is stopped.
    fn consume(name_w: &[u16], callback: unsafe extern "system" fn(*mut EVENT_RECORD)) {
        let mut logfile: EVENT_TRACE_LOGFILEW = unsafe { std::mem::zeroed() };
        logfile.LoggerName = PWSTR(name_w.as_ptr() as *mut u16);
        logfile.Anonymous1.ProcessTraceMode = PROCESS_TRACE_MODE_REAL_TIME | PROCESS_TRACE_MODE_EVENT_RECORD | PROCESS_TRACE_MODE_RAW_TIMESTAMP; // raw = QPC ticks
        logfile.Anonymous2.EventRecordCallback = Some(callback);
        let h = unsafe { OpenTraceW(&mut logfile) };
        if h.Value == u64::MAX {
            return;
        }
        unsafe {
            ProcessTrace(&[h], None, None);
            let _ = CloseTrace(h);
        }
    }
}

// ---------------------------------------------------------------- network per process

fn net_target() -> &'static Mutex<Option<NetBytes>> {
    static T: OnceLock<Mutex<Option<NetBytes>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(None))
}

unsafe extern "system" fn net_callback(rec: *mut EVENT_RECORD) {
    let r = unsafe { &*rec };
    if r.EventHeader.ProviderId != NET_GUID || r.UserDataLength < 8 {
        return;
    }
    let d = unsafe { std::slice::from_raw_parts(r.UserData as *const u8, 8) };
    if let Some((pid, size)) = net_payload(r.EventHeader.EventDescriptor.Id, d) {
        if let Ok(g) = net_target().lock() {
            if let Some(map) = g.as_ref() {
                if let Ok(mut m) = map.lock() {
                    *m.entry(pid).or_insert(0) += size;
                }
            }
        }
    }
}

/// (pid, bytes) of a Kernel-Network send/receive event: 10/11 = IPv4, 42/43 = IPv6.
pub fn net_payload(id: u16, data: &[u8]) -> Option<(u32, u64)> {
    if !matches!(id, 10 | 11 | 42 | 43) || data.len() < 8 {
        return None;
    }
    Some((u32::from_le_bytes(data[0..4].try_into().ok()?), u32::from_le_bytes(data[4..8].try_into().ok()?) as u64))
}

/// Counts network bytes per process while it lives. Dropping it stops the trace.
pub struct NetTrace {
    session: Session,
    thread: Option<std::thread::JoinHandle<()>>,
    bytes: NetBytes,
}

impl NetTrace {
    pub fn start() -> Result<NetTrace, String> {
        let mut session = Session::start(NET_SESSION, None, 4)?;
        let code = unsafe { EnableTraceEx2(session.handle, &NET_GUID, EVENT_CONTROL_CODE_ENABLE_PROVIDER.0, 5, u64::MAX, 0, 0, None) };
        if code.0 != 0 {
            session.stop();
            return Err(describe(code));
        }
        let bytes: NetBytes = Default::default();
        *net_target().lock().map_err(|e| e.to_string())? = Some(bytes.clone());
        let name = session.name.clone();
        let thread = std::thread::Builder::new().name("ProcessNet".into()).spawn(move || Session::consume(&name, net_callback)).map_err(|e| e.to_string())?;
        Ok(NetTrace { session, thread: Some(thread), bytes })
    }

    pub fn bytes(&self) -> NetBytes {
        self.bytes.clone()
    }
}

impl Drop for NetTrace {
    fn drop(&mut self) {
        self.session.stop();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Ok(mut g) = net_target().lock() {
            *g = None;
        }
    }
}

// ---------------------------------------------------------------- driver latency

#[derive(Debug, Clone, PartialEq)]
pub struct LatencyRow {
    pub driver: String,
    pub count: u32,
    pub total_ms: f64,
    pub max_us: f64,
}

/// routine address -> (calls, total ticks, longest ticks)
type Acc = HashMap<u64, (u32, u64, u64)>;

fn lat_acc() -> &'static Mutex<Acc> {
    static A: OnceLock<Mutex<Acc>> = OnceLock::new();
    A.get_or_init(|| Mutex::new(HashMap::new()))
}

/// (routine, ticks) of a PerfInfo DPC (66), ISR (67) or TimerDPC (68) event at `timestamp`.
pub fn perfinfo_payload(opcode: u8, data: &[u8], timestamp: i64) -> Option<(u64, u64)> {
    if !matches!(opcode, 66..=68) || data.len() < 16 {
        return None;
    }
    let initial = i64::from_le_bytes(data[0..8].try_into().ok()?);
    let routine = u64::from_le_bytes(data[8..16].try_into().ok()?);
    (timestamp > initial && initial > 0).then(|| (routine, (timestamp - initial) as u64))
}

unsafe extern "system" fn kernel_callback(rec: *mut EVENT_RECORD) {
    let r = unsafe { &*rec };
    if r.EventHeader.ProviderId != PERFINFO_GUID || r.UserDataLength < 16 {
        return;
    }
    let d = unsafe { std::slice::from_raw_parts(r.UserData as *const u8, 16) };
    if let Some((routine, ticks)) = perfinfo_payload(r.EventHeader.EventDescriptor.Opcode, d, r.EventHeader.TimeStamp) {
        if let Ok(mut m) = lat_acc().lock() {
            let e = m.entry(routine).or_insert((0, 0, 0));
            e.0 = e.0.saturating_add(1);
            e.1 = e.1.saturating_add(ticks);
            e.2 = e.2.max(ticks);
        }
    }
}

/// Loaded kernel drivers sorted by base address, so a routine address maps to the module containing it.
pub fn loaded_drivers() -> Vec<(u64, String)> {
    use windows::Win32::System::ProcessStatus::{EnumDeviceDrivers, GetDeviceDriverBaseNameW};
    let mut needed = 0u32;
    unsafe {
        let _ = EnumDeviceDrivers(std::ptr::null_mut(), 0, &mut needed);
    }
    let mut bases = vec![std::ptr::null_mut::<std::ffi::c_void>(); needed as usize / std::mem::size_of::<usize>() + 8];
    let cb = (bases.len() * std::mem::size_of::<usize>()) as u32;
    if unsafe { EnumDeviceDrivers(bases.as_mut_ptr(), cb, &mut needed) }.is_err() {
        return vec![];
    }
    let mut list: Vec<(u64, String)> = vec![];
    let mut buf = [0u16; 260];
    for b in bases.into_iter().take(needed as usize / std::mem::size_of::<usize>()) {
        if b.is_null() {
            continue;
        }
        let n = unsafe { GetDeviceDriverBaseNameW(b, &mut buf) };
        if n > 0 {
            list.push((b as u64, String::from_utf16_lossy(&buf[..n as usize])));
        }
    }
    list.sort_by_key(|d| d.0);
    list
}

/// Driver whose module contains `routine`; modules are rarely larger than 64 MB.
pub fn resolve(drivers: &[(u64, String)], routine: u64) -> &str {
    let i = drivers.partition_point(|d| d.0 <= routine);
    match i.checked_sub(1).map(|i| &drivers[i]) {
        Some((base, name)) if routine - base < 64 * 1024 * 1024 => name,
        _ => "unknown",
    }
}

/// Rows per driver from the per-address accumulator, longest stall first.
pub fn rows_from(acc: &Acc, drivers: &[(u64, String)], ticks_per_sec: f64) -> Vec<LatencyRow> {
    let mut by_name: HashMap<String, (u32, f64, f64)> = HashMap::new();
    for (&routine, &(n, total, max)) in acc {
        let e = by_name.entry(resolve(drivers, routine).to_ascii_lowercase()).or_insert((0, 0.0, 0.0));
        e.0 += n;
        e.1 += total as f64 / ticks_per_sec * 1000.0;
        e.2 = e.2.max(max as f64 / ticks_per_sec * 1000.0);
    }
    // keep the spelling of the first module with that name
    let spelled = |lower: &str| drivers.iter().find(|d| d.1.eq_ignore_ascii_case(lower)).map(|d| d.1.clone()).unwrap_or_else(|| lower.to_string());
    let mut rows: Vec<LatencyRow> = by_name
        .into_iter()
        .map(|(k, (n, total_ms, max_ms))| LatencyRow { driver: spelled(&k), count: n, total_ms: (total_ms * 100.0).round() / 100.0, max_us: (max_ms * 1000.0).round() })
        .collect();
    rows.sort_by(|a, b| b.max_us.total_cmp(&a.max_us));
    rows
}

fn qpc_frequency() -> f64 {
    let mut f = 0i64;
    unsafe {
        let _ = windows::Win32::System::Performance::QueryPerformanceFrequency(&mut f);
    }
    f.max(1) as f64
}

/// Kernel trace of interrupt and DPC routines for `seconds`; blocks (run it on a worker thread).
pub fn latency_scan(seconds: u32) -> Result<Vec<LatencyRow>, String> {
    let drivers = loaded_drivers();
    lat_acc().lock().map_err(|e| e.to_string())?.clear();
    let mut session = Session::start(KERNEL_LOGGER, Some(FLAG_DPC | FLAG_INTERRUPT), 64)?; // large buffers: fewer lost events under heavy DPC traffic;
    let name = session.name.clone();
    let worker = std::thread::spawn(move || Session::consume(&name, kernel_callback));
    std::thread::sleep(std::time::Duration::from_secs(seconds as u64));
    session.stop();
    let _ = worker.join();
    let acc = lat_acc().lock().map_err(|e| e.to_string())?.clone();
    crate::log::info(&format!("latency scan: {} routines", acc.len()));
    Ok(rows_from(&acc, &drivers, qpc_frequency()))
}

// ---------------------------------------------------------------- texts of the latency card (`LatencyVM`)

pub const LATENCY_INTRO: &str = "Finds drivers that hog the CPU in interrupt and DPC routines, a common cause of stutter, audio crackle and input lag. For best results play a game or a video while it scans.";

pub fn latency_detail(r: &LatencyRow) -> String {
    format!("{} calls, {:.1} ms in total, longest {} µs", crate::mouse::n0(r.count as f64), r.total_ms, crate::mouse::n0(r.max_us))
}

pub fn latency_verdict(max_us: f64) -> (crate::diagnostics::ChipLevel, &'static str) {
    use crate::diagnostics::ChipLevel::*;
    if max_us >= 500.0 {
        (Warn, "Can cause hitches")
    } else if max_us >= 200.0 {
        (Neutral, "Watch")
    } else {
        (Good, "Fine")
    }
}

/// Sentence under the title after a scan; `previous` is the longest stall of the scan before.
pub fn latency_summary(rows: &[LatencyRow], previous: Option<f64>) -> String {
    let n0 = crate::mouse::n0;
    let Some(worst) = rows.first() else { return "No interrupt activity was recorded.".into() };
    let mut s = if worst.max_us >= 500.0 {
        format!("{} had the longest stall ({} µs). Update or reinstall that driver, or check the device it belongs to.", worst.driver, n0(worst.max_us))
    } else {
        "No driver stalled for long enough to matter. Scan again while a game is running for a fuller picture.".into()
    };
    // Lets you see whether MSI mode, interrupt spreading or a driver update helped.
    if let Some(prev) = previous {
        let word = if worst.max_us < prev { "better" } else if worst.max_us > prev { "worse" } else { "the same" };
        s += &format!(" Previous scan: longest stall {} µs, now {} µs ({word}).", n0(prev), n0(worst.max_us));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latency_texts() {
        let r = |d: &str, us| LatencyRow { driver: d.into(), count: 1234, total_ms: 5.25, max_us: us };
        assert_eq!(latency_detail(&r("a", 80.0)), "1,234 calls, 5.2 ms in total, longest 80 µs");
        assert_eq!(latency_verdict(499.0).1, "Watch");
        assert_eq!(latency_verdict(500.0).1, "Can cause hitches");
        assert_eq!(latency_verdict(10.0).1, "Fine");
        assert_eq!(latency_summary(&[], None), "No interrupt activity was recorded.");
        let s = latency_summary(&[r("nvlddmkm.sys", 600.0)], Some(900.0));
        assert!(s.starts_with("nvlddmkm.sys had the longest stall (600"), "{s}");
        assert!(s.ends_with("Previous scan: longest stall 900 µs, now 600 µs (better)."), "{s}");
        assert!(latency_summary(&[r("x", 100.0)], None).starts_with("No driver stalled"));
    }

    #[test]
    fn net_payload_layout() {
        let mut d = 1234u32.to_le_bytes().to_vec();
        d.extend(1500u32.to_le_bytes());
        d.extend([0u8; 20]);
        assert_eq!(net_payload(10, &d), Some((1234, 1500)));
        assert_eq!(net_payload(43, &d), Some((1234, 1500)));
        assert_eq!(net_payload(12, &d), None); // connect, not data
        assert_eq!(net_payload(10, &d[..4]), None);
    }

    #[test]
    fn perfinfo_payload_layout() {
        let mut d = 1000i64.to_le_bytes().to_vec();
        d.extend(0xFFFFF800_12345678u64.to_le_bytes());
        assert_eq!(perfinfo_payload(66, &d, 1250), Some((0xFFFFF800_12345678, 250)));
        assert_eq!(perfinfo_payload(67, &d, 1250).map(|x| x.1), Some(250));
        assert_eq!(perfinfo_payload(65, &d, 1250), None); // not a DPC/ISR opcode
        assert_eq!(perfinfo_payload(66, &d, 900), None); // ends before it started
    }

    #[test]
    fn resolve_finds_module() {
        let drivers = vec![(0x1000u64, "a.sys".to_string()), (0x9000, "b.sys".to_string())];
        assert_eq!(resolve(&drivers, 0x0800), "unknown");
        assert_eq!(resolve(&drivers, 0x1000), "a.sys");
        assert_eq!(resolve(&drivers, 0x8FFF), "a.sys");
        assert_eq!(resolve(&drivers, 0x9100), "b.sys");
        assert_eq!(resolve(&drivers, 0x9000 + 65 * 1024 * 1024), "unknown"); // too far into nowhere
    }

    #[test]
    fn rows_group_and_sort() {
        let drivers = vec![(0x1000u64, "ndis.sys".to_string()), (0x9000, "nvlddmkm.sys".to_string())];
        let mut acc: Acc = HashMap::new();
        acc.insert(0x1100, (10, 1_000, 400)); // two routines in one driver
        acc.insert(0x1200, (5, 500, 900));
        acc.insert(0x9100, (2, 100, 50));
        let rows = rows_from(&acc, &drivers, 1_000_000.0); // 1 tick = 1 us
        assert_eq!(rows[0].driver, "ndis.sys");
        assert_eq!(rows[0].count, 15);
        assert_eq!(rows[0].max_us, 900.0);
        assert!((rows[0].total_ms - 1.5).abs() < 1e-9);
        assert_eq!(rows[1].driver, "nvlddmkm.sys");
    }

    /// Needs admin: a short real kernel trace (1 s).
    #[test]
    #[ignore]
    fn real_latency_scan() {
        let rows = latency_scan(1).expect("kernel trace");
        assert!(!rows.is_empty(), "no interrupt activity recorded in 1 s");
        println!("{:?}", &rows[..rows.len().min(3)]);
    }

    /// Needs admin: the network trace starts, sees this process's own traffic and stops.
    #[test]
    #[ignore]
    fn real_net_trace() {
        let t = NetTrace::start().expect("net trace");
        let bytes = t.bytes();
        // loopback traffic from this process
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        let h = std::thread::spawn(move || {
            use std::io::Read;
            let (mut s, _) = l.accept().unwrap();
            let mut buf = vec![0u8; 65536];
            let mut n = 0;
            while let Ok(k) = s.read(&mut buf) {
                if k == 0 {
                    break;
                }
                n += k;
            }
            n
        });
        {
            use std::io::Write;
            let mut c = std::net::TcpStream::connect(addr).unwrap();
            c.write_all(&vec![7u8; 200_000]).unwrap();
        }
        let _ = h.join();
        std::thread::sleep(std::time::Duration::from_millis(3000));
        let mine = bytes.lock().unwrap().get(&std::process::id()).copied().unwrap_or(0);
        println!("bytes counted for this pid: {mine}; own pid {}; top: {:?}", std::process::id(), { let m = bytes.lock().unwrap(); let mut v: Vec<_> = m.iter().map(|(k, v)| (*k, *v)).collect(); v.sort_by_key(|x| std::cmp::Reverse(x.1)); v.truncate(5); v });
        drop(t);
        assert!(mine > 0, "no network bytes seen for this process");
    }
}
