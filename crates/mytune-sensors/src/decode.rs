//! Pure register decoders (ported from LibreHardwareMonitor IntelCpu / Amd17Cpu). No I/O, unit-tested.

/// IA32_TEMPERATURE_TARGET (0x1A2) -> TjMax; 100 when the field is empty (LHM fallback).
pub fn intel_tjmax(msr: u64) -> f32 {
    let v = ((msr >> 16) & 0xFF) as f32;
    if v > 0.0 { v } else { 100.0 }
}

/// IA32_THERM_STATUS (0x19C) / IA32_PACKAGE_THERM_STATUS (0x1B1): bit 31 = reading valid, bits 22:16 = degrees below TjMax.
pub fn intel_temp(status: u64, tjmax: f32) -> Option<f32> {
    if status & (1 << 31) == 0 {
        return None;
    }
    Some(tjmax - ((status >> 16) & 0x7F) as f32)
}

/// MSR_RAPL_POWER_UNIT (0x606): joules per energy tick.
pub fn intel_energy_unit(msr: u64) -> f64 {
    1.0 / (1u64 << ((msr >> 8) & 0x1F)) as f64
}

/// AMD MSR_PWR_UNIT (0xC0010299): joules per energy tick.
pub fn amd_energy_unit(msr: u64) -> f64 {
    0.5f64.powi(((msr >> 8) & 0x1F) as i32)
}

/// Ticks between two reads of a 32-bit energy counter.
pub fn energy_delta(cur: u64, last: u64) -> u64 {
    let (c, l) = (cur & 0xFFFF_FFFF, last & 0xFFFF_FFFF);
    if c >= l { c - l } else { 0x1_0000_0000 - l + c }
}

/// Zen THM_TCON_CUR_TMP (SMN 0x59800): bits 31:21 in 0.125 C, bit 19 = -49 C range select (k10temp).
pub fn amd_tctl(raw: u32, offset: f32) -> f32 {
    let mut t = (raw >> 21) as f32 * 0.125;
    if raw & (1 << 19) != 0 {
        t -= 49.0;
    }
    t - offset
}

/// Tctl offsets by CPU name (LHM table, from k10temp.c).
pub fn amd_tctl_offset(name: &str) -> f32 {
    if name.contains("1600X") || name.contains("1700X") || name.contains("1800X") {
        20.0
    } else if name.contains("Threadripper 19") || name.contains("Threadripper 29") {
        27.0
    } else if name.contains("2700X") {
        10.0
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intel() {
        assert_eq!(intel_tjmax(0x0064_0000), 100.0);
        assert_eq!(intel_tjmax(0), 100.0);
        assert_eq!(intel_tjmax(0x0061_0000), 97.0);
        assert_eq!(intel_temp(0x8000_0000 | (30 << 16), 100.0), Some(70.0));
        assert_eq!(intel_temp(30 << 16, 100.0), None);
        assert_eq!(intel_energy_unit(14 << 8), 1.0 / 16384.0);
    }

    #[test]
    fn amd() {
        assert_eq!(amd_energy_unit(16 << 8), 0.5f64.powi(16));
        assert_eq!(amd_tctl(480 << 21, 0.0), 60.0);
        assert_eq!(amd_tctl((480 << 21) | (1 << 19), 0.0), 11.0);
        assert_eq!(amd_tctl(480 << 21, 20.0), 40.0);
        assert_eq!(amd_tctl_offset("AMD Ryzen 7 1700X"), 20.0);
        assert_eq!(amd_tctl_offset("AMD Ryzen 7 7800X3D"), 0.0);
    }

    #[test]
    fn energy_wrap() {
        assert_eq!(energy_delta(150, 100), 50);
        assert_eq!(energy_delta(10, 0xFFFF_FFF0), 26);
    }
}
