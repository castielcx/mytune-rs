//! CPU temperature and package power through PawnIO modules (Intel MSR, AMD Zen SMN/MSR).
use crate::{decode, pawnio::Pawnio};
use std::time::Instant;

const INTEL_BIN: &[u8] = include_bytes!("../modules/IntelMSR.bin");
const AMD17_BIN: &[u8] = include_bytes!("../modules/AMDFamily17.bin");

enum Kind {
    Intel { tjmax: f32 },
    Amd { offset: f32 },
}

pub struct Thermal {
    drv: Pawnio,
    kind: Kind,
    unit: f64,
    last: Option<(u64, Instant)>,
    power: Option<f32>,
}

fn cpuid_vendor_family() -> ([u8; 12], u32) {
    use std::arch::x86_64::__cpuid;
    let v = __cpuid(0);
    let mut s = [0u8; 12];
    s[..4].copy_from_slice(&v.ebx.to_le_bytes());
    s[4..8].copy_from_slice(&v.edx.to_le_bytes());
    s[8..].copy_from_slice(&v.ecx.to_le_bytes());
    let eax = __cpuid(1).eax;
    let (base, ext) = ((eax >> 8) & 0xF, (eax >> 20) & 0xFF);
    (s, if base == 0xF { base + ext } else { base })
}

pub fn cpu_name() -> String {
    use std::arch::x86_64::__cpuid;
    let mut b = Vec::new();
    for leaf in 0x8000_0002u32..=0x8000_0004 {
        let r = __cpuid(leaf);
        for x in [r.eax, r.ebx, r.ecx, r.edx] {
            b.extend_from_slice(&x.to_le_bytes());
        }
    }
    String::from_utf8_lossy(&b).trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_string()
}

impl Thermal {
    /// Opens the driver and loads the module for this CPU. None when PawnIO is missing or the CPU is unsupported.
    pub fn open() -> Option<Thermal> {
        let (vendor, family) = cpuid_vendor_family();
        let drv = Pawnio::open().ok()?;
        let (kind, unit) = match &vendor {
            b"GenuineIntel" => {
                drv.load(INTEL_BIN).ok()?;
                let tj = drv.read_msr(0x1A2).map(decode::intel_tjmax).unwrap_or(100.0);
                let unit = drv.read_msr(0x606).map(decode::intel_energy_unit).unwrap_or(0.0);
                (Kind::Intel { tjmax: tj }, unit)
            }
            b"AuthenticAMD" if family >= 0x17 => {
                drv.load(AMD17_BIN).ok()?;
                let unit = drv.read_msr(0xC001_0299).map(decode::amd_energy_unit).unwrap_or(0.0);
                (Kind::Amd { offset: decode::amd_tctl_offset(&cpu_name()) }, unit)
            }
            // shortcut: AMD K8/K10 not ported, add the AMDFamily0F/10 modules if such a CPU matters
            _ => return None,
        };
        Some(Thermal { drv, kind, unit, last: None, power: None })
    }

    /// (package temp C, package power W)
    pub fn read(&mut self) -> (Option<f32>, Option<f32>) {
        let (temp, energy) = match self.kind {
            Kind::Intel { tjmax } => (
                self.drv
                    .read_msr(0x1B1)
                    .and_then(|s| decode::intel_temp(s, tjmax))
                    .or_else(|| self.drv.read_msr(0x19C).and_then(|s| decode::intel_temp(s, tjmax))),
                self.drv.read_msr(0x611),
            ),
            // shortcut: no Access_PCI mutex around SMN, add if another monitor tool collides
            Kind::Amd { offset } => (
                self.drv.read_smn(0x0005_9800).map(|r| decode::amd_tctl(r, offset)),
                self.drv.read_msr(0xC001_029B),
            ),
        };
        if let (Some(e), true) = (energy, self.unit > 0.0) {
            let now = Instant::now();
            match self.last {
                Some((le, lt)) if now.duration_since(lt).as_secs_f64() >= 0.01 => {
                    let dt = now.duration_since(lt).as_secs_f64();
                    self.power = Some((self.unit * decode::energy_delta(e, le) as f64 / dt) as f32);
                    self.last = Some((e, now));
                }
                Some(_) => {}
                None => self.last = Some((e, now)),
            }
        }
        (temp, self.power)
    }
}
