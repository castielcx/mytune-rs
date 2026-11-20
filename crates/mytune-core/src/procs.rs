//! Process access behind a trait, so the booster and the game watcher can be tested with a fake.
//! `WinProcs` is the real thing (Toolhelp snapshot, `SetPriorityClass`, EcoQoS via `SetProcessInformation`).

use crate::error::{Error, Result};
use std::ffi::c_void;
use windows::Win32::Foundation::{CloseHandle, FILETIME, HANDLE};
use windows::Win32::System::Diagnostics::ToolHelp::{CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS};
use windows::Win32::System::Threading::{
    GetPriorityClass, GetProcessTimes, OpenProcess, ProcessPowerThrottling, SetPriorityClass, SetProcessInformation,
    PROCESS_CREATION_FLAGS, PROCESS_POWER_THROTTLING_STATE, PROCESS_SET_INFORMATION, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_ACCESS_RIGHTS,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcInfo {
    pub pid: u32,
    /// Executable name without `.exe`, as .NET `Process.ProcessName`.
    pub name: String,
}

/// What `throttle` changed, so `restore` can undo exactly that.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held {
    /// Process creation time: tells a reused pid from the process we throttled.
    pub created: u64,
    /// Original priority class, if we lowered it.
    pub priority: Option<u32>,
    /// EcoQoS (efficiency mode) was switched on.
    pub eco: bool,
}

pub const NORMAL: u32 = 0x20;
pub const ABOVE_NORMAL: u32 = 0x8000;
pub const BELOW_NORMAL: u32 = 0x4000;

pub trait Procs: Send + Sync {
    fn list(&self) -> Result<Vec<ProcInfo>>;
    /// Lowers priority (Normal/AboveNormal only) and switches EcoQoS on. `Ok(None)`: protected, gone, or nothing changed.
    fn throttle(&self, pid: u32) -> Result<Option<Held>>;
    /// Undoes `throttle`. A pid that now belongs to another program (creation time differs) is left alone.
    fn restore(&self, pid: u32, held: &Held) -> Result<()>;
}

struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn open(pid: u32, access: PROCESS_ACCESS_RIGHTS) -> Option<Handle> {
    unsafe { OpenProcess(access, false, pid).ok().map(Handle) }
}

fn created(h: &Handle) -> Option<u64> {
    let (mut c, mut e, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    unsafe { GetProcessTimes(h.0, &mut c, &mut e, &mut k, &mut u).ok()? };
    Some((c.dwHighDateTime as u64) << 32 | c.dwLowDateTime as u64)
}

const EXECUTION_SPEED: u32 = 1;

fn set_eco(h: &Handle, on: bool) -> bool {
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: 1,
        ControlMask: if on { EXECUTION_SPEED } else { 0 }, // both masks 0 = Windows decides again
        StateMask: if on { EXECUTION_SPEED } else { 0 },
    };
    unsafe { SetProcessInformation(h.0, ProcessPowerThrottling, &state as *const _ as *const c_void, std::mem::size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32).is_ok() }
}

pub struct WinProcs;

impl Procs for WinProcs {
    fn list(&self) -> Result<Vec<ProcInfo>> {
        let mut out = Vec::new();
        unsafe {
            let snap = Handle(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0).map_err(|e| Error::Os(format!("process snapshot: {e}")))?);
            let mut e = PROCESSENTRY32W { dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32, ..Default::default() };
            let mut more = Process32FirstW(snap.0, &mut e).is_ok();
            while more {
                let end = e.szExeFile.iter().position(|&c| c == 0).unwrap_or(e.szExeFile.len());
                let file = String::from_utf16_lossy(&e.szExeFile[..end]);
                let name = file.strip_suffix(".exe").or_else(|| file.strip_suffix(".EXE")).unwrap_or(&file).to_string();
                out.push(ProcInfo { pid: e.th32ProcessID, name });
                more = Process32NextW(snap.0, &mut e).is_ok();
            }
        }
        Ok(out)
    }

    fn throttle(&self, pid: u32) -> Result<Option<Held>> {
        let Some(h) = open(pid, PROCESS_ACCESS_RIGHTS(PROCESS_SET_INFORMATION.0 | PROCESS_QUERY_LIMITED_INFORMATION.0)) else { return Ok(None) };
        let Some(created) = created(&h) else { return Ok(None) };
        let priority = unsafe { GetPriorityClass(h.0) };
        let lowered = (priority == NORMAL || priority == ABOVE_NORMAL) && unsafe { SetPriorityClass(h.0, PROCESS_CREATION_FLAGS(BELOW_NORMAL)).is_ok() };
        let eco = set_eco(&h, true);
        Ok((lowered || eco).then_some(Held { created, priority: lowered.then_some(priority), eco }))
    }

    fn restore(&self, pid: u32, held: &Held) -> Result<()> {
        let Some(h) = open(pid, PROCESS_ACCESS_RIGHTS(PROCESS_SET_INFORMATION.0 | PROCESS_QUERY_LIMITED_INFORMATION.0)) else { return Ok(()) }; // closed meanwhile
        if created(&h) != Some(held.created) {
            return Ok(()); // the pid now belongs to another program
        }
        if let Some(p) = held.priority {
            unsafe { SetPriorityClass(h.0, PROCESS_CREATION_FLAGS(p)).map_err(|e| Error::Os(format!("restore priority of {pid}: {e}")))? };
        }
        if held.eco && !set_eco(&h, false) {
            return Err(Error::Os(format!("restore efficiency mode of {pid}")));
        }
        Ok(())
    }
}

/// Current priority class of a process (tests and diagnostics).
pub fn priority_of(pid: u32) -> Option<u32> {
    let h = open(pid, PROCESS_QUERY_LIMITED_INFORMATION)?;
    match unsafe { GetPriorityClass(h.0) } {
        0 => None,
        p => Some(p),
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Fake process table: records throttle/restore calls.
    pub struct FakeProcs {
        pub list: Mutex<Vec<ProcInfo>>,
        pub calls: Mutex<Vec<String>>,
        pub fail_restore: Mutex<Vec<u32>>,
    }
    impl FakeProcs {
        pub fn new(procs: &[(u32, &str)]) -> Arc<FakeProcs> {
            Arc::new(FakeProcs {
                list: Mutex::new(procs.iter().map(|(pid, n)| ProcInfo { pid: *pid, name: n.to_string() }).collect()),
                calls: Mutex::new(vec![]),
                fail_restore: Mutex::new(vec![]),
            })
        }
    }
    impl Procs for FakeProcs {
        fn list(&self) -> Result<Vec<ProcInfo>> {
            Ok(self.list.lock().unwrap().clone())
        }
        fn throttle(&self, pid: u32) -> Result<Option<Held>> {
            self.calls.lock().unwrap().push(format!("throttle {pid}"));
            Ok(Some(Held { created: pid as u64, priority: Some(0x20), eco: true }))
        }
        fn restore(&self, pid: u32, _: &Held) -> Result<()> {
            self.calls.lock().unwrap().push(format!("restore {pid}"));
            if self.fail_restore.lock().unwrap().contains(&pid) {
                Err(Error::Os("nope".into()))
            } else {
                Ok(())
            }
        }
    }
}
