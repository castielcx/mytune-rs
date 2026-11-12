//! Minimal PawnIO driver client (protocol from LibreHardwareMonitor PawnIo.cs).
//! `open` only opens the device handle. `load` / `exec` talk to the driver; callers must opt in.
use std::io;
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows::Win32::System::IO::DeviceIoControl;

const DEVICE_TYPE: u32 = 41394 << 16;
const IOCTL_LOAD_BINARY: u32 = DEVICE_TYPE | (0x821 << 2);
const IOCTL_EXECUTE_FN: u32 = DEVICE_TYPE | (0x841 << 2);
const FN_NAME_LEN: usize = 32;

fn err(e: windows::core::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, e.to_string())
}

pub struct Pawnio(HANDLE);
// SAFETY: a kernel handle may be used from any thread.
unsafe impl Send for Pawnio {}

impl Pawnio {
    pub fn open() -> io::Result<Pawnio> {
        let h = unsafe {
            CreateFileW(
                w!("\\\\?\\GLOBALROOT\\Device\\PawnIO"),
                (GENERIC_READ | GENERIC_WRITE).0,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
        .map_err(err)?;
        Ok(Pawnio(h))
    }

    /// Load a signed module .bin into the driver for this handle.
    pub fn load(&self, bin: &[u8]) -> io::Result<()> {
        unsafe { DeviceIoControl(self.0, IOCTL_LOAD_BINARY, Some(bin.as_ptr().cast()), bin.len() as u32, None, 0, None, None) }.map_err(err)
    }

    /// Call a module function with i64 args; returns up to `out_len` i64 values.
    pub fn exec(&self, name: &str, args: &[i64], out_len: usize) -> io::Result<Vec<i64>> {
        let mut inp = vec![0u8; FN_NAME_LEN + args.len() * 8];
        let n = name.len().min(FN_NAME_LEN - 1);
        inp[..n].copy_from_slice(&name.as_bytes()[..n]);
        for (i, a) in args.iter().enumerate() {
            inp[FN_NAME_LEN + i * 8..][..8].copy_from_slice(&a.to_le_bytes());
        }
        let mut out = vec![0i64; out_len];
        let mut ret = 0u32;
        unsafe {
            DeviceIoControl(
                self.0,
                IOCTL_EXECUTE_FN,
                Some(inp.as_ptr().cast()),
                inp.len() as u32,
                Some(out.as_mut_ptr().cast()),
                (out_len * 8) as u32,
                Some(&mut ret),
                None,
            )
        }
        .map_err(err)?;
        out.truncate(ret as usize / 8);
        Ok(out)
    }

    pub fn read_msr(&self, msr: u32) -> Option<u64> {
        self.exec("ioctl_read_msr", &[msr as i64], 1).ok()?.first().map(|v| *v as u64)
    }

    pub fn read_smn(&self, addr: u32) -> Option<u32> {
        self.exec("ioctl_read_smn", &[addr as i64], 1).ok()?.first().map(|v| *v as u32)
    }
}

impl Drop for Pawnio {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
