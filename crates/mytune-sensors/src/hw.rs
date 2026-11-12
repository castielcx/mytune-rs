//! Real-machine source: GetSystemTimes + PDH (CPU), GlobalMemoryStatusEx (RAM), NVML or PDH+DXGI (GPU),
//! PawnIO (CPU temp/power, only when `use_pawnio` is set).
use crate::{nvml::Nvml, pdh, thermal, Snapshot, Source};
use mytune_core::gamewatch::CpuTopology;
use windows::Win32::Foundation::FILETIME;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1, DXGI_ADAPTER_FLAG_SOFTWARE};
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;

struct Adapter {
    name: String,
    nvidia: bool,
    vram_mb: f32,
    luid: String,
}

/// Same order as the C# scoring: NVIDIA > discrete > other > integrated; VRAM breaks ties.
fn pick_adapter() -> Option<Adapter> {
    let f: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;
    let mut best: Option<(i64, Adapter)> = None;
    for i in 0.. {
        let Ok(a) = (unsafe { f.EnumAdapters1(i) }) else { break };
        let Ok(d) = (unsafe { a.GetDesc1() }) else { continue };
        if d.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 || d.VendorId == 0x1414 {
            continue;
        }
        let vram = d.DedicatedVideoMemory as f32 / (1 << 20) as f32;
        let score = match d.VendorId {
            0x10DE => 3000,
            _ if vram >= 1024.0 => 2500,
            _ => 0,
        } + vram as i64 / 1024;
        let name_len = d.Description.iter().position(|&c| c == 0).unwrap_or(128);
        let ad = Adapter {
            name: String::from_utf16_lossy(&d.Description[..name_len]),
            nvidia: d.VendorId == 0x10DE,
            vram_mb: vram,
            luid: format!("luid_0x{:08x}_0x{:08x}", d.AdapterLuid.HighPart as u32, d.AdapterLuid.LowPart),
        };
        if best.as_ref().map_or(true, |(s, _)| score > *s) {
            best = Some((score, ad));
        }
    }
    best.map(|(_, a)| a)
}

fn ft(f: FILETIME) -> u64 {
    ((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64
}

pub struct HwSource {
    q: Option<pdh::Query>,
    perf: Option<pdh::Counter>,
    freq: Option<pdh::Counter>,
    gpu_eng: Option<pdh::Counter>,
    gpu_mem: Option<pdh::Counter>,
    pmask: u64,
    nvml: Option<Nvml>,
    adapter: Option<Adapter>,
    cpu_name: String,
    last_times: Option<(u64, u64)>,
    use_pawnio: bool,
    thermal: Option<thermal::Thermal>,
    thermal_tried: bool,
}

impl HwSource {
    /// `use_pawnio` false never touches the PawnIO driver (no module load, no MSR/SMN read).
    pub fn new(use_pawnio: bool) -> HwSource {
        let adapter = pick_adapter();
        let nvml = adapter.as_ref().filter(|a| a.nvidia).and_then(|_| Nvml::open());
        let q = pdh::Query::new();
        let add = |p: &str| q.as_ref().and_then(|q| q.add(p));
        let (gpu_eng, gpu_mem) = if nvml.is_none() && adapter.is_some() {
            (add("\\GPU Engine(*)\\Utilization Percentage"), add("\\GPU Adapter Memory(*)\\Dedicated Usage"))
        } else {
            (None, None)
        };
        let perf = add("\\Processor Information(*)\\% Processor Performance");
        let freq = add("\\Processor Information(*)\\Processor Frequency");
        if let Some(q) = &q {
            q.collect(); // first sample primes the rate counters
        }
        HwSource {
            perf,
            freq,
            gpu_eng,
            gpu_mem,
            pmask: CpuTopology::detect().performance_mask,
            nvml,
            adapter,
            cpu_name: thermal::cpu_name(),
            q,
            last_times: None,
            use_pawnio,
            thermal: None,
            thermal_tried: false,
        }
    }

    fn cpu_load(&mut self) -> Option<f32> {
        let (mut i, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default());
        unsafe { GetSystemTimes(Some(&mut i), Some(&mut k), Some(&mut u)) }.ok()?;
        let (idle, total) = (ft(i), ft(k) + ft(u)); // kernel time includes idle
        let prev = self.last_times.replace((idle, total))?;
        let (di, dt) = (idle.wrapping_sub(prev.0), total.wrapping_sub(prev.1));
        (dt > 0).then(|| (100.0 * (1.0 - di as f64 / dt as f64)).clamp(0.0, 100.0) as f32)
    }

    /// Mean of freq x performance% over performance cores (all cores when not hybrid), like the C# P-Core average.
    fn cpu_clock(&self) -> Option<f32> {
        let perf = self.perf?.array();
        let freq = self.freq?.array();
        let (mut sum, mut n) = (0.0, 0);
        for (name, p) in &perf {
            let Some((g, c)) = name.split_once(',') else { continue };
            let (Ok(g), Ok(c)) = (g.parse::<u32>(), c.parse::<u32>()) else { continue };
            if g != 0 || c >= 64 || (self.pmask != 0 && self.pmask >> c & 1 == 0) {
                continue;
            }
            if let Some((_, f)) = freq.iter().find(|(n, _)| n == name) {
                sum += f * p / 100.0;
                n += 1;
            }
        }
        (n > 0).then(|| (sum / n as f64) as f32)
    }

    fn ram(s: &mut Snapshot) {
        let mut m = MEMORYSTATUSEX { dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32, ..Default::default() };
        if unsafe { GlobalMemoryStatusEx(&mut m) }.is_ok() && m.ullTotalPhys > 0 {
            let gb = (1u64 << 30) as f64;
            let used = m.ullTotalPhys - m.ullAvailPhys;
            s.ram_total_gb = Some((m.ullTotalPhys as f64 / gb) as f32);
            s.ram_used_gb = Some((used as f64 / gb) as f32);
            s.ram_load = Some((100.0 * used as f64 / m.ullTotalPhys as f64) as f32);
        }
    }
}

impl Source for HwSource {
    fn names(&self) -> (String, String) {
        let gpu = self.nvml.as_ref().and_then(|n| n.name()).or_else(|| self.adapter.as_ref().map(|a| a.name.clone()));
        (self.cpu_name.clone(), gpu.unwrap_or_else(|| "GPU".into()))
    }

    fn sample(&mut self) -> Snapshot {
        let mut s = Snapshot::default();
        if let Some(q) = &self.q {
            q.collect();
        }
        s.cpu_load = self.cpu_load();
        s.cpu_clock_mhz = self.cpu_clock();
        Self::ram(&mut s);

        if self.use_pawnio && !self.thermal_tried {
            self.thermal_tried = true;
            self.thermal = thermal::Thermal::open();
        }
        if let Some(t) = &mut self.thermal {
            (s.cpu_temp, s.cpu_power_w) = t.read();
        }

        if let Some(n) = &self.nvml {
            let r = n.read();
            (s.gpu_load, s.gpu_temp, s.gpu_clock_mhz, s.gpu_power_w) = (r.load, r.temp, r.clock_mhz, r.power_w);
            (s.vram_used_mb, s.vram_total_mb) = (r.vram_used_mb, r.vram_total_mb);
        } else if let Some(a) = &self.adapter {
            // AMD/Intel: only load and VRAM are available without vendor libraries.
            let eng = self.gpu_eng.map(|c| c.array()).unwrap_or_default();
            let eng: Vec<_> = eng.iter().filter(|(n, _)| n.contains(&a.luid) && n.contains("engtype_3D")).collect();
            if !eng.is_empty() {
                s.gpu_load = Some(eng.iter().map(|(_, v)| *v).sum::<f64>().min(100.0) as f32);
            }
            let used: f64 = self.gpu_mem.map(|c| c.array()).unwrap_or_default().iter().filter(|(n, _)| n.contains(&a.luid)).map(|(_, v)| *v).sum();
            if used > 0.0 {
                s.vram_used_mb = Some((used / (1 << 20) as f64) as f32);
            }
            s.vram_total_mb = (a.vram_mb > 0.0).then_some(a.vram_mb);
        }
        s
    }
}
