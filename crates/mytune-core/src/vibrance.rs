//! NVIDIA digital vibrance through NVAPI (C# `DisplayService` uses NvAPIWrapper `DisplayApi.GetDVCInfoEx` /
//! `SetDVCLevelEx`). Same run-time loading as `nvapi.rs`, no link-time dependency.
//!
//! Display handles are matched to Windows monitors by the number in `\\.\DISPLAY<n>`, like C# `FindNv`.
//! `NV_DISPLAY_DVC_INFO_EX` carries its own size in `version`, so a wrong layout is refused by the driver.
//! Function ids are from the `nvapi-sys` id table; the call signatures (handle, outputId = 0, struct) follow
//! nvapi.h from memory and are checked by the real-GPU test below (read only).

use crate::error::{Error, Result};
use crate::nvapi::{available, chk, func, ver, H, NO_IMPLEMENTATION, OK};

/// NV_DISPLAY_DVC_INFO_EX: version, current, min, max, default level.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct DvcInfoEx {
    version: u32,
    current: i32,
    min: i32,
    max: i32,
    default: i32,
}

const DVC_VER: u32 = ver(std::mem::size_of::<DvcInfoEx>(), 1);
const END_ENUMERATION: i32 = -7;

type FnEnumDisplay = unsafe extern "C" fn(u32, *mut H) -> i32;
type FnDisplayName = unsafe extern "C" fn(H, *mut u8) -> i32;
type FnDvc = unsafe extern "C" fn(H, u32, *mut DvcInfoEx) -> i32;

/// Driver range of the digital vibrance level of one display.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VibranceRange {
    pub min: i32,
    pub max: i32,
    pub current: i32,
    pub default: i32,
}

impl VibranceRange {
    /// Linear map of 0..100 % onto min..max.
    pub fn level_for(&self, percent: f64) -> i32 {
        (self.min as f64 + percent.clamp(0.0, 100.0) / 100.0 * (self.max - self.min) as f64).round_ties_even() as i32
    }

    pub fn percent_for(&self) -> f64 {
        if self.max == self.min {
            0.0
        } else {
            ((self.current - self.min) as f64 * 100.0 / (self.max - self.min) as f64).round_ties_even()
        }
    }
}

/// Number in `\\.\DISPLAY<n>` (case-insensitive).
pub fn display_number(name: &str) -> Option<u32> {
    let l = name.to_ascii_lowercase();
    let rest = &l[l.find("display")? + 7..];
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// NVAPI display handle whose Windows name is DISPLAY<number>; `None` when no NVIDIA GPU drives it.
fn display_handle(number: u32) -> Result<Option<H>> {
    if !available() {
        return Ok(None);
    }
    let enumerate = func::<FnEnumDisplay>(0x9ABDD40D, "NvAPI_EnumNvidiaDisplayHandle")?;
    let name_of = func::<FnDisplayName>(0x22A78B05, "NvAPI_GetAssociatedNvidiaDisplayName")?;
    for i in 0..64 {
        let mut h: H = std::ptr::null_mut();
        match unsafe { enumerate(i, &mut h) } {
            OK => {}
            END_ENUMERATION => break,
            s => return Err(Error::Nvapi { func: "NvAPI_EnumNvidiaDisplayHandle", status: s }),
        }
        let mut buf = [0u8; 64];
        if unsafe { name_of(h, buf.as_mut_ptr()) } != OK {
            continue;
        }
        let text = String::from_utf8_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(64)]).into_owned();
        if display_number(&text) == Some(number) {
            return Ok(Some(h));
        }
    }
    Ok(None)
}

/// Vibrance range of `\\.\DISPLAY<number>`, or `None` when it is not driven by an NVIDIA GPU.
pub fn get(number: u32) -> Result<Option<VibranceRange>> {
    let Some(h) = display_handle(number)? else { return Ok(None) };
    let mut info = DvcInfoEx { version: DVC_VER, ..Default::default() };
    chk("NvAPI_GetDVCInfoEx", unsafe { func::<FnDvc>(0x0E45002D, "NvAPI_GetDVCInfoEx")?(h, 0, &mut info) })?;
    Ok(Some(VibranceRange { min: info.min, max: info.max, current: info.current, default: info.default }))
}

/// Sets the vibrance level; a display without an NVIDIA GPU is an error.
pub fn set(number: u32, level: i32) -> Result<()> {
    let h = display_handle(number)?.ok_or(Error::Nvapi { func: "NvAPI_SetDVCLevelEx", status: NO_IMPLEMENTATION })?;
    let mut info = DvcInfoEx { version: DVC_VER, current: level, ..Default::default() };
    chk("NvAPI_SetDVCLevelEx", unsafe { func::<FnDvc>(0x4A82C2B1, "NvAPI_SetDVCLevelEx")?(h, 0, &mut info) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_layout() {
        assert_eq!(display_number(r"\\.\DISPLAY2"), Some(2));
        assert_eq!(display_number(r"\\.\display12\Monitor0"), Some(12));
        assert_eq!(display_number("HDMI"), None);
        assert_eq!(std::mem::size_of::<DvcInfoEx>(), 20);
        assert_eq!(DVC_VER, 20 | (1 << 16));
    }

    #[test]
    fn percent_mapping_matches_csharp() {
        let r = VibranceRange { min: 0, max: 63, current: 0, default: 0 };
        assert_eq!(r.level_for(0.0), 0);
        assert_eq!(r.level_for(50.0), 32); // 31.5: ties go to even, like C# Math.Round
        assert_eq!(r.level_for(100.0), 63);
        assert_eq!(r.level_for(250.0), 63);
        let c = VibranceRange { current: 32, ..r };
        assert_eq!(c.percent_for(), 51.0);
        assert_eq!(VibranceRange { min: 5, max: 5, current: 5, default: 5 }.percent_for(), 0.0);
    }

    /// Writes the level the display already has (no visible change) and reads it back: proves the Set signature.
    #[test]
    fn real_gpu_set_same_level_is_accepted() {
        for n in 1..=4 {
            if let Some(r) = get(n).unwrap() {
                set(n, r.current).expect("SetDVCLevelEx refused the current level, signature may be wrong");
                assert_eq!(get(n).unwrap().unwrap().current, r.current);
            }
        }
    }

    /// Read only. On a PC with an NVIDIA GPU on DISPLAY1 the range must be sane; otherwise nothing is asserted.
    #[test]
    fn real_gpu_range_is_sane() {
        for n in 1..=4 {
            if let Some(r) = get(n).expect("NVAPI answered with an error, the call signature may be wrong") {
                eprintln!("DISPLAY{n}: {r:?}");
                assert!(r.min <= r.current && r.current <= r.max && r.min < r.max, "{r:?}");
            }
        }
    }
}
