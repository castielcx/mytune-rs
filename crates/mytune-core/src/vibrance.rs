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

// ---------------------------------------------------------------- colour space conversion (extra saturation)
//
// Undocumented NvAPI_GPU_Get/SetColorSpaceConversion. Ids, layout and meaning of the fields are the facts
// published in novideo_srgb (github.com/ledoge/novideo_srgb, Novideo.cs); the code here is our own.
// Observed pipeline there: content degamma -> content to sRGB -> sRGB to monitor -> matrix2 -> matrix1 ->
// monitor gamma. With content = monitor = 2 (sRGB) only the matrix changes the picture, in linear light.

/// V1 struct, 0x7C bytes. Matrices are 3 rows of (r, g, b gain, offset).
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Csc {
    version: u32,
    /// 1..12, driver default 2 (sRGB).
    pub content: u32,
    /// 0 = conversion off (driver default), 2 = sRGB.
    pub monitor: u32,
    unknown1: u32,
    unknown2: u32,
    pub use_matrix1: u32,
    pub matrix1: [f32; 12],
    pub use_matrix2: u32,
    pub matrix2: [f32; 12],
}

const CSC_VER: u32 = ver(std::mem::size_of::<Csc>(), 1);

type FnDisplayId = unsafe extern "C" fn(*const u8, *mut u32) -> i32;
type FnCsc = unsafe extern "C" fn(u32, *mut Csc) -> i32;

impl Csc {
    /// The state that switches the conversion off (what novideo_srgb writes to disable).
    pub fn off() -> Csc {
        Csc { version: CSC_VER, content: 2, ..Default::default() }
    }

    /// sRGB in, sRGB out, with a saturation matrix in between.
    pub fn saturation(s: f64) -> Csc {
        Csc { version: CSC_VER, content: 2, monitor: 2, use_matrix1: 1, matrix1: saturation_matrix(s), ..Default::default() }
    }

    /// Off, or Unity's HDR output state (12/12 without matrices), which is not a user's conversion either.
    pub fn is_off(&self) -> bool {
        self.monitor == 0 || (self.monitor == 12 && self.content == 12 && self.use_matrix1 != 1 && self.use_matrix2 != 1)
    }

    /// The factor when this is exactly a `Csc::saturation` (what MyTune writes), else `None`.
    pub fn factor(&self) -> Option<f64> {
        let s = ((self.matrix1[0] - 0.2126) / (1.0 - 0.2126)) as f64;
        let m = saturation_matrix(s);
        let close = self.matrix1.iter().zip(m).all(|(a, b)| (a - b).abs() < 1e-4);
        (self.content == 2 && self.monitor == 2 && self.use_matrix1 == 1 && self.use_matrix2 != 1 && close).then_some(s)
    }

    /// Same picture as `other` (ignores the unknown fields and unused matrices).
    pub fn same(&self, other: &Csc) -> bool {
        let m = |c: &Csc| (c.content, c.monitor, (c.use_matrix1 == 1).then_some(c.matrix1), (c.use_matrix2 == 1).then_some(c.matrix2));
        (self.is_off() && other.is_off()) || m(self) == m(other)
    }
}

/// M = (1 - s) * L + s * I with Rec.709 luma weights: every row sums to 1, so greys stay grey.
/// s = 1 is the identity, above 1 pushes colours away from grey.
pub fn saturation_matrix(s: f64) -> [f32; 12] {
    const L: [f64; 3] = [0.2126, 0.7152, 0.0722];
    let mut m = [0f32; 12];
    for r in 0..3 {
        for c in 0..3 {
            m[r * 4 + c] = ((1.0 - s) * L[c] + if r == c { s } else { 0.0 }) as f32;
        }
    }
    m
}

/// NVAPI display id of `\\.\DISPLAY<n>`; `None` when no NVIDIA GPU drives it.
pub(crate) fn display_id(dev: &str) -> Result<Option<u32>> {
    if !available() {
        return Ok(None);
    }
    let name: Vec<u8> = dev.bytes().chain(Some(0)).collect();
    let mut id = 0u32;
    let s = unsafe { func::<FnDisplayId>(0xAE457190, "NvAPI_DISP_GetDisplayIdByDisplayName")?(name.as_ptr(), &mut id) };
    Ok((s == OK).then_some(id))
}

/// The display's colour space conversion, or `None` when it is not driven by an NVIDIA GPU.
pub fn csc_get(dev: &str) -> Result<Option<Csc>> {
    let Some(id) = display_id(dev)? else { return Ok(None) };
    let mut c = Csc { version: CSC_VER, ..Default::default() };
    chk("NvAPI_GPU_GetColorSpaceConversion", unsafe { func::<FnCsc>(0x8159E87A, "NvAPI_GPU_GetColorSpaceConversion")?(id, &mut c) })?;
    Ok(Some(c))
}

/// Writes a conversion. With Windows HDR on the driver answers `NOT_SUPPORTED` (-104).
pub fn csc_set(dev: &str, c: &Csc) -> Result<()> {
    let id = display_id(dev)?.ok_or(Error::Nvapi { func: "NvAPI_GPU_SetColorSpaceConversion", status: NO_IMPLEMENTATION })?;
    let mut c = Csc { version: CSC_VER, unknown1: 0, ..*c };
    chk("NvAPI_GPU_SetColorSpaceConversion", unsafe { func::<FnCsc>(0xFCABD23A, "NvAPI_GPU_SetColorSpaceConversion")?(id, &mut c) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saturation_matrix_keeps_grey() {
        let one = saturation_matrix(1.0);
        assert_eq!(one, [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
        for s in [0.0, 0.5, 1.0, 1.25, 2.0, 4.0] {
            let m = saturation_matrix(s);
            for r in 0..3 {
                let row = &m[r * 4..r * 4 + 4];
                assert!((row[0] + row[1] + row[2] - 1.0).abs() < 1e-6 && row[3] == 0.0, "s{s} row{r}");
                let grey = 0.4 * (row[0] + row[1] + row[2]);
                assert!((grey - 0.4).abs() < 1e-6, "grey stays grey, s{s}");
            }
        }
        // above 1 a colour moves away from grey: pure red gets more red, negative green
        let m = saturation_matrix(2.0);
        assert!(m[0] > 1.0 && m[4] < 0.0);
        assert_eq!(std::mem::size_of::<Csc>(), 0x7C);
        assert_eq!(CSC_VER, 0x1007C);
        assert!(Csc::off().is_off() && !Csc::saturation(1.5).is_off());
        assert!(Csc::saturation(1.5).same(&Csc::saturation(1.5)) && !Csc::saturation(1.5).same(&Csc::saturation(1.6)));
        // MyTune knows its own matrix again, and not another tool's
        assert!((Csc::saturation(1.37).factor().unwrap() - 1.37).abs() < 1e-4);
        let mut clamp = Csc::saturation(1.0);
        clamp.matrix1[1] = 0.1;
        assert!(clamp.factor().is_none() && Csc::off().factor().is_none());
    }

    /// Changes the picture of the NON-primary display for a moment: reads its conversion, sets saturation
    /// matrices, reads them back, restores what was there (also on a panic). Never touches the primary display.
    /// `cargo test -p mytune-core --lib vibrance::tests::real_csc_on_second_display -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_csc_on_second_display() {
        use windows::core::PCWSTR;
        use windows::Win32::Graphics::Gdi::{EnumDisplayDevicesW, DISPLAY_DEVICEW};
        let mut dev = None;
        for i in 0.. {
            let mut d = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
            if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), i, &mut d, 0) }.as_bool() {
                break;
            }
            let name = String::from_utf16_lossy(&d.DeviceName[..d.DeviceName.iter().position(|&c| c == 0).unwrap_or(32)]);
            let (attached, primary) = (d.StateFlags.0 & 1 != 0, d.StateFlags.0 & 4 != 0);
            eprintln!("{name}: attached {attached}, primary {primary}");
            if attached && !primary && dev.is_none() {
                dev = Some(name);
            }
        }
        let Some(dev) = dev else { return eprintln!("no non-primary display: nothing tested") };
        let before = csc_get(&dev).expect("get").expect("not an NVIDIA display");
        eprintln!("{dev} before: {before:?}");
        struct Restore(String, Csc);
        impl Drop for Restore {
            fn drop(&mut self) {
                let to = if self.1.is_off() { Csc::off() } else { self.1 };
                eprintln!("{} restore: {:?}", self.0, csc_set(&self.0, &to));
                let now = csc_get(&self.0);
                eprintln!("{} after restore: {now:?}", self.0);
            }
        }
        let guard = Restore(dev.clone(), before);
        for s in [1.25, 2.0, 3.0] {
            let want = Csc::saturation(s);
            let set = csc_set(&dev, &want);
            let got = csc_get(&dev).expect("get").unwrap();
            eprintln!("{dev} s={s}: set {set:?}, read back same: {}, matrix1 {:?}", got.same(&want), got.matrix1);
            assert!(set.is_ok() && got.same(&want));
        }
        drop(guard);
        assert!(csc_get(&dev).unwrap().unwrap().same(&before));
    }

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
