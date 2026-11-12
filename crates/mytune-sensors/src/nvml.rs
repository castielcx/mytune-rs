//! NVIDIA NVML, loaded at runtime from the driver (no link-time dependency, absent on non-NVIDIA PCs).
use std::ffi::c_void;
use windows::core::{w, PCSTR};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

type Dev = *mut c_void;
type FnGet32 = unsafe extern "C" fn(Dev, *mut u32) -> i32;
type FnGet32Sel = unsafe extern "C" fn(Dev, u32, *mut u32) -> i32;

pub struct Nvml {
    dev: Dev,
    util: unsafe extern "C" fn(Dev, *mut [u32; 2]) -> i32,
    temp: FnGet32Sel,
    clock: FnGet32Sel,
    power: FnGet32,
    mem: unsafe extern "C" fn(Dev, *mut [u64; 3]) -> i32,
    name: unsafe extern "C" fn(Dev, *mut u8, u32) -> i32,
}
// SAFETY: NVML calls are thread-safe; the handle is only used from the sampler thread anyway.
unsafe impl Send for Nvml {}

#[derive(Default)]
pub struct Reading {
    pub load: Option<f32>,
    pub temp: Option<f32>,
    pub clock_mhz: Option<f32>,
    pub power_w: Option<f32>,
    pub vram_used_mb: Option<f32>,
    pub vram_total_mb: Option<f32>,
}

impl Nvml {
    /// shortcut: device 0 only, pick by index if multi-NVIDIA PCs matter.
    pub fn open() -> Option<Nvml> {
        unsafe {
            let lib = LoadLibraryW(w!("nvml.dll"))
                .or_else(|_| LoadLibraryW(w!("C:\\Program Files\\NVIDIA Corporation\\NVSMI\\nvml.dll")))
                .ok()?;
            macro_rules! sym {
                ($n:expr) => {
                    std::mem::transmute(GetProcAddress(lib, PCSTR(concat!($n, "\0").as_ptr()))?)
                };
            }
            let init: unsafe extern "C" fn() -> i32 = sym!("nvmlInit_v2");
            let by_index: unsafe extern "C" fn(u32, *mut Dev) -> i32 = sym!("nvmlDeviceGetHandleByIndex_v2");
            if init() != 0 {
                return None;
            }
            let mut dev: Dev = std::ptr::null_mut();
            if by_index(0, &mut dev) != 0 {
                return None;
            }
            Some(Nvml {
                dev,
                util: sym!("nvmlDeviceGetUtilizationRates"),
                temp: sym!("nvmlDeviceGetTemperature"),
                clock: sym!("nvmlDeviceGetClockInfo"),
                power: sym!("nvmlDeviceGetPowerUsage"),
                mem: sym!("nvmlDeviceGetMemoryInfo"),
                name: sym!("nvmlDeviceGetName"),
            })
        }
    }

    pub fn name(&self) -> Option<String> {
        let mut b = [0u8; 96];
        (unsafe { (self.name)(self.dev, b.as_mut_ptr(), 96) } == 0)
            .then(|| String::from_utf8_lossy(&b[..b.iter().position(|&c| c == 0).unwrap_or(96)]).into_owned())
    }

    pub fn read(&self) -> Reading {
        let mut r = Reading::default();
        unsafe {
            let mut u = [0u32; 2];
            if (self.util)(self.dev, &mut u) == 0 {
                r.load = Some(u[0] as f32);
            }
            let mut v = 0u32;
            if (self.temp)(self.dev, 0, &mut v) == 0 {
                r.temp = Some(v as f32);
            }
            if (self.clock)(self.dev, 0, &mut v) == 0 {
                r.clock_mhz = Some(v as f32);
            }
            if (self.power)(self.dev, &mut v) == 0 {
                r.power_w = Some(v as f32 / 1000.0);
            }
            let mut m = [0u64; 3]; // total, free, used
            if (self.mem)(self.dev, &mut m) == 0 {
                r.vram_total_mb = Some((m[0] / (1 << 20)) as f32);
                r.vram_used_mb = Some((m[2] / (1 << 20)) as f32);
            }
        }
        r
    }
}
