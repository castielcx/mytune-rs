//! Custom resolutions through NVAPI: what NVIDIA Control Panel's "Create Custom Resolution" (Timing:
//! Automatic) does, plus its scaling setting "Full-screen, perform scaling on GPU, override the scaling mode
//! set by games" (`NV_SCALING_GPU_SCALING_TO_NATIVE`, per display).
//!
//! Flow: `try_mode` (driver trial, nothing saved) -> `save` or `revert`; `stretch` after a save.
//! The saved list per display is the top-level `CustomResolutions` key of state.json.
//!
//! Function ids are from NVIDIA's public `nvapi_interface.h`, struct layouts and version numbers from
//! `nvapi.h` (github.com/NVIDIA/nvapi, read 2026-11-19; `#pragma pack(8)` = natural alignment = `repr(C)`).
//! Every versioned struct carries its size, so the driver refuses a wrong layout; `layout_sizes` pins them.

use crate::display::DisplayMode;
use crate::error::{Error, Result};
use crate::log;
use crate::nvapi::{chk, func, ver, NO_IMPLEMENTATION, OK};
use crate::store::Store;
use crate::vibrance::display_id;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::ffi::c_void;

const END_ENUMERATION: i32 = -7;
const TIMING_AUTO: u32 = 1; // NV_TIMING_OVERRIDE_AUTO: "the timing the driver will use based the current policy"
const FORMAT_A8R8G8B8: u32 = 21;
/// NV_SCALING_GPU_SCALING_TO_NATIVE, "Force GPU - Full Screen".
pub const SCALING_GPU_FULLSCREEN: u32 = 2;
const SAVE_TO_PERSISTENCE: u32 = 2; // NV_DISPLAYCONFIG_SAVE_TO_PERSISTENCE

// ---------------------------------------------------------------- structs (nvapi.h)

/// NV_TIMINGEXT
#[repr(C)]
#[derive(Clone, Copy)]
struct TimingExt {
    flag: u32,
    rr: u16,
    rrx1k: u32,
    aspect: u32,
    rep: u16,
    status: u32,
    name: [u8; 40],
}

/// NV_TIMING
#[repr(C)]
#[derive(Clone, Copy)]
struct Timing {
    h_visible: u16,
    h_border: u16,
    h_front_porch: u16,
    h_sync_width: u16,
    h_total: u16,
    h_sync_pol: u8,
    v_visible: u16,
    v_border: u16,
    v_front_porch: u16,
    v_sync_width: u16,
    v_total: u16,
    v_sync_pol: u8,
    interlaced: u16,
    pclk: u32,
    etc: TimingExt,
}

/// NV_TIMING_INPUT. `flag` is NV_TIMING_FLAG: a bitfield word, an anonymous union (its own word) and a
/// second bitfield word; all zero here.
#[repr(C)]
struct TimingInput {
    version: u32,
    width: u32,
    height: u32,
    rr: f32,
    flag: [u32; 3],
    ty: u32,
}

/// NV_CUSTOM_DISPLAY
#[repr(C)]
#[derive(Clone, Copy)]
struct CustomDisplay {
    version: u32,
    width: u32,
    height: u32,
    depth: u32,
    color_format: u32,
    src_partition: [f32; 4], // x, y, w, h
    x_ratio: f32,
    y_ratio: f32,
    timing: Timing,
    flags: u32, // bit 0 hwModeSetOnly
}

/// NV_DISPLAYCONFIG_PATH_ADVANCED_TARGET_INFO_V1
#[repr(C)]
struct Advanced {
    version: u32,
    rotation: u32,
    scaling: u32,
    refresh_rate_1k: u32,
    flags: u32,
    connector: u32,
    tv_format: u32,
    timing_override: u32,
    timing: Timing,
}

/// NV_DISPLAYCONFIG_PATH_TARGET_INFO_V2
#[repr(C)]
struct Target {
    display_id: u32,
    details: *mut Advanced,
    target_id: u32,
}

/// NV_DISPLAYCONFIG_SOURCE_MODE_INFO_V1: resolution (3), colour format, position (2), spanning, flag bits.
/// Only handed back to the driver as read.
#[repr(C)]
struct SourceMode([u32; 8]);

/// NV_DISPLAYCONFIG_PATH_INFO_V2
#[repr(C)]
struct Path {
    version: u32,
    source_id: u32,
    target_count: u32,
    targets: *mut Target,
    source: *mut SourceMode,
    flags: u32,
    os_adapter: *mut c_void,
}

const INPUT_VER: u32 = ver(std::mem::size_of::<TimingInput>(), 1);
const CUSTOM_VER: u32 = ver(std::mem::size_of::<CustomDisplay>(), 1);
const ADVANCED_VER: u32 = ver(std::mem::size_of::<Advanced>(), 1);
const PATH_VER: u32 = ver(std::mem::size_of::<Path>(), 2);

type FnTiming = unsafe extern "C" fn(u32, *mut TimingInput, *mut Timing) -> i32;
type FnEnum = unsafe extern "C" fn(u32, u32, *mut CustomDisplay) -> i32;
type FnCustom = unsafe extern "C" fn(*mut u32, u32, *mut CustomDisplay) -> i32;
type FnSave = unsafe extern "C" fn(*mut u32, u32, u32, u32) -> i32;
type FnRevert = unsafe extern "C" fn(*mut u32, u32) -> i32;
type FnGetConfig = unsafe extern "C" fn(*mut u32, *mut Path) -> i32;
type FnSetConfig = unsafe extern "C" fn(u32, *mut Path, u32) -> i32;

fn id_of(dev: &str, func: &'static str) -> Result<u32> {
    display_id(dev)?.ok_or(Error::Nvapi { func, status: NO_IMPLEMENTATION })
}

// ---------------------------------------------------------------- custom display

/// The driver's own ("Automatic") timing for the mode, wrapped as a custom display: source = the mode,
/// whole surface, ratio 1, 32 bit.
fn custom(id: u32, m: DisplayMode) -> Result<CustomDisplay> {
    let mut input = TimingInput { version: INPUT_VER, width: m.width, height: m.height, rr: m.hz as f32, flag: [0; 3], ty: TIMING_AUTO };
    // plain integers and floats only: all zero is a valid value
    let mut c: CustomDisplay = unsafe { std::mem::zeroed() };
    chk("NvAPI_DISP_GetTiming", unsafe { func::<FnTiming>(0x175167E9, "NvAPI_DISP_GetTiming")?(id, &mut input, &mut c.timing) })?;
    c.version = CUSTOM_VER;
    c.width = m.width;
    c.height = m.height;
    c.depth = 32;
    c.color_format = FORMAT_A8R8G8B8;
    c.src_partition = [0.0, 0.0, 1.0, 1.0];
    c.x_ratio = 1.0;
    c.y_ratio = 1.0;
    Ok(c)
}

/// Shows the mode on the display without saving it. Follow with `save` or `revert`.
pub fn try_mode(dev: &str, m: DisplayMode) -> Result<()> {
    let mut id = id_of(dev, "NvAPI_DISP_TryCustomDisplay")?;
    let mut c = custom(id, m)?;
    chk("NvAPI_DISP_TryCustomDisplay", unsafe { func::<FnCustom>(0x1F7DB630, "NvAPI_DISP_TryCustomDisplay")?(&mut id, 1, &mut c) })?;
    log::info(&format!("custom resolution trial {dev}: {} x {} @ {} Hz", m.width, m.height, m.hz));
    Ok(())
}

/// Ends a trial: back to the mode before `try_mode`.
pub fn revert(dev: &str) -> Result<()> {
    let mut id = id_of(dev, "NvAPI_DISP_RevertCustomDisplayTrial")?;
    chk("NvAPI_DISP_RevertCustomDisplayTrial", unsafe { func::<FnRevert>(0xCBBD40F0, "NvAPI_DISP_RevertCustomDisplayTrial")?(&mut id, 1) })
}

/// Saves the mode on trial for this monitor on this output, so Windows lists it.
pub fn save(dev: &str) -> Result<()> {
    let mut id = id_of(dev, "NvAPI_DISP_SaveCustomDisplay")?;
    chk("NvAPI_DISP_SaveCustomDisplay", unsafe { func::<FnSave>(0x49882876, "NvAPI_DISP_SaveCustomDisplay")?(&mut id, 1, 1, 1) })?;
    log::info(&format!("custom resolution saved on {dev}"));
    Ok(())
}

fn enumerate(id: u32) -> Result<Vec<CustomDisplay>> {
    let f = func::<FnEnum>(0xA2072D59, "NvAPI_DISP_EnumCustomDisplay")?;
    let mut list = Vec::new();
    for i in 0..64 {
        let mut c: CustomDisplay = unsafe { std::mem::zeroed() };
        c.version = CUSTOM_VER;
        match unsafe { f(id, i, &mut c) } {
            OK => list.push(c),
            END_ENUMERATION => break,
            s => return Err(Error::Nvapi { func: "NvAPI_DISP_EnumCustomDisplay", status: s }),
        }
    }
    Ok(list)
}

/// Custom resolutions the driver has saved for this display (also those made in NVIDIA Control Panel).
pub fn driver_list(dev: &str) -> Result<Vec<DisplayMode>> {
    let id = id_of(dev, "NvAPI_DISP_EnumCustomDisplay")?;
    Ok(enumerate(id)?.iter().map(|c| DisplayMode { width: c.width, height: c.height, hz: c.timing.etc.rr as u32 }).collect())
}

/// Removes a saved custom resolution from the driver. Ok when the driver no longer has it.
pub fn delete(dev: &str, m: DisplayMode) -> Result<()> {
    let mut id = id_of(dev, "NvAPI_DISP_DeleteCustomDisplay")?;
    let f = func::<FnCustom>(0x552E5B9B, "NvAPI_DISP_DeleteCustomDisplay")?;
    for mut c in enumerate(id)?.into_iter().filter(|c| (c.width, c.height, c.timing.etc.rr as u32) == (m.width, m.height, m.hz)) {
        chk("NvAPI_DISP_DeleteCustomDisplay", unsafe { f(&mut id, 1, &mut c) })?;
        log::info(&format!("custom resolution deleted on {dev}: {} x {} @ {} Hz", m.width, m.height, m.hz));
    }
    Ok(())
}

// ---------------------------------------------------------------- scaling

/// The whole display configuration as the driver reports it. The vectors own what the raw pointers in
/// `paths` point at and are never resized.
struct Config {
    paths: Vec<Path>,
    targets: Vec<Vec<Target>>,
    details: Vec<Box<Advanced>>,
    _sources: Vec<Box<SourceMode>>,
}

/// The three passes nvapi.h describes for NvAPI_DISP_GetDisplayConfig: count, target counts, details.
fn config() -> Result<Config> {
    let f = func::<FnGetConfig>(0x11ABCCF8, "NvAPI_DISP_GetDisplayConfig")?;
    let get = |n: &mut u32, p: *mut Path| chk("NvAPI_DISP_GetDisplayConfig", unsafe { f(n, p) });
    let mut n = 0u32;
    get(&mut n, std::ptr::null_mut())?;
    let mut c = Config { paths: Vec::new(), targets: Vec::new(), details: Vec::new(), _sources: Vec::new() };
    use std::ptr::null_mut;
    c.paths = (0..n).map(|_| Path { version: PATH_VER, source_id: 0, target_count: 0, targets: null_mut(), source: null_mut(), flags: 0, os_adapter: null_mut() }).collect();
    get(&mut n, c.paths.as_mut_ptr())?;
    for i in 0..c.paths.len() {
        let mut targets: Vec<Target> = Vec::new();
        for _ in 0..c.paths[i].target_count {
            // plain integers only: all zero is a valid value
            let mut d: Box<Advanced> = Box::new(unsafe { std::mem::zeroed() });
            d.version = ADVANCED_VER;
            targets.push(Target { display_id: 0, details: &mut *d, target_id: 0 });
            c.details.push(d);
        }
        let mut source = Box::new(SourceMode([0; 8]));
        c.paths[i].targets = targets.as_mut_ptr();
        c.paths[i].source = &mut *source;
        c.targets.push(targets);
        c._sources.push(source);
    }
    get(&mut n, c.paths.as_mut_ptr())?;
    Ok(c)
}

impl Config {
    /// The advanced settings of the display with this id.
    fn target(&mut self, id: u32) -> Option<&mut Advanced> {
        let all = self.targets.iter().flatten().position(|t| t.display_id == id)?;
        self.details.get_mut(all).map(|b| &mut **b)
    }
}

/// The display's scaling mode (NV_SCALING), `SCALING_GPU_FULLSCREEN` = stretched by the GPU.
pub fn scaling(dev: &str) -> Result<u32> {
    let id = id_of(dev, "NvAPI_DISP_GetDisplayConfig")?;
    config()?.target(id).map(|a| a.scaling).ok_or(Error::Nvapi { func: "NvAPI_DISP_GetDisplayConfig", status: NO_IMPLEMENTATION })
}

/// Sets the scaling mode of one display and saves it in the driver. The call hands the whole configuration
/// back to the driver with only this value changed. Nothing is sent when the display already has it.
pub fn set_scaling(dev: &str, value: u32) -> Result<()> {
    let id = id_of(dev, "NvAPI_DISP_SetDisplayConfig")?;
    let mut c = config()?;
    let a = c.target(id).ok_or(Error::Nvapi { func: "NvAPI_DISP_SetDisplayConfig", status: NO_IMPLEMENTATION })?;
    if a.scaling == value {
        return Ok(());
    }
    let old = a.scaling;
    a.scaling = value;
    let f = func::<FnSetConfig>(0x5D8CF8DE, "NvAPI_DISP_SetDisplayConfig")?;
    chk("NvAPI_DISP_SetDisplayConfig", unsafe { f(c.paths.len() as u32, c.paths.as_mut_ptr(), SAVE_TO_PERSISTENCE) })?;
    log::info(&format!("display scaling {dev}: {old} -> {value}"));
    Ok(())
}

/// Full-screen, scaled by the GPU, whatever the game asks for: every resolution of this display is stretched.
pub fn stretch(dev: &str) -> Result<()> {
    set_scaling(dev, SCALING_GPU_FULLSCREEN)
}

// ---------------------------------------------------------------- validation

/// Width and height of the monitor's preferred mode as Windows knows it. The largest listed mode is not it:
/// monitors and NVIDIA DSR list larger sizes (seen: 3840 x 2160 at 60 Hz on a 1920 x 1080 240 Hz monitor).
fn preferred(dev: &str) -> Option<(u32, u32)> {
    use windows::Win32::Devices::Display::*;
    let (mut np, mut nm) = (0u32, 0u32);
    unsafe {
        GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut np, &mut nm).ok().ok()?;
        let mut paths = vec![DISPLAYCONFIG_PATH_INFO::default(); np as usize];
        let mut modes = vec![DISPLAYCONFIG_MODE_INFO::default(); nm as usize];
        QueryDisplayConfig(QDC_ONLY_ACTIVE_PATHS, &mut np, paths.as_mut_ptr(), &mut nm, modes.as_mut_ptr(), None).ok().ok()?;
        for p in paths.iter().take(np as usize) {
            let mut src = DISPLAYCONFIG_SOURCE_DEVICE_NAME::default();
            src.header = DISPLAYCONFIG_DEVICE_INFO_HEADER { r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, size: std::mem::size_of_val(&src) as u32, adapterId: p.sourceInfo.adapterId, id: p.sourceInfo.id };
            if DisplayConfigGetDeviceInfo(&mut src.header) != 0 {
                continue;
            }
            let gdi = src.viewGdiDeviceName;
            if String::from_utf16_lossy(&gdi[..gdi.iter().position(|&c| c == 0).unwrap_or(gdi.len())]) != dev {
                continue;
            }
            let mut pref = DISPLAYCONFIG_TARGET_PREFERRED_MODE::default();
            pref.header = DISPLAYCONFIG_DEVICE_INFO_HEADER { r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_PREFERRED_MODE, size: std::mem::size_of_val(&pref) as u32, adapterId: p.targetInfo.adapterId, id: p.targetInfo.id };
            if DisplayConfigGetDeviceInfo(&mut pref.header) == 0 {
                return Some((pref.width, pref.height));
            }
        }
    }
    None
}

/// The display's own resolution with its highest listed refresh rate.
pub fn native(dev: &str, modes: &[DisplayMode]) -> Option<DisplayMode> {
    native_of(preferred(dev), modes)
}

fn native_of(preferred: Option<(u32, u32)>, modes: &[DisplayMode]) -> Option<DisplayMode> {
    let at = |r: (u32, u32)| modes.iter().filter(|m| (m.width, m.height) == r).max_by_key(|m| m.hz).copied();
    // Windows named none, or one that is not listed: the resolution with the highest refresh rate, the largest of those
    preferred.and_then(at).or_else(|| modes.iter().max_by_key(|m| (m.hz, m.width as u64 * m.height as u64)).copied())
}

/// The mode a typed size stands for (highest refresh rate of the native resolution), or why it can't be added.
pub fn validate(width: u32, height: u32, native: Option<DisplayMode>, modes: &[DisplayMode]) -> std::result::Result<DisplayMode, String> {
    let n = native.ok_or("This display's modes couldn't be read.")?;
    if width < 640 || height < 480 {
        return Err("The smallest size is 640 x 480.".into());
    }
    if width > n.width || height > n.height {
        return Err(format!("The largest size is this display's own {} x {}.", n.width, n.height));
    }
    if modes.iter().any(|m| (m.width, m.height) == (width, height)) {
        return Err(format!("Windows already lists {width} x {height}. Pick it in the list above."));
    }
    Ok(DisplayMode { width, height, hz: n.hz })
}

// ---------------------------------------------------------------- state.json

#[derive(Serialize, Deserialize)]
struct Saved {
    #[serde(rename = "Width")]
    width: u32,
    #[serde(rename = "Height")]
    height: u32,
    #[serde(rename = "Hz")]
    hz: u32,
}

fn all(store: &Store) -> BTreeMap<String, Vec<Saved>> {
    store.top("CustomResolutions").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

/// Custom resolutions MyTune saved for a display (`CustomResolutions` in state.json).
pub fn saved(store: &Store, dev: &str) -> Vec<DisplayMode> {
    all(store).remove(dev).unwrap_or_default().iter().map(|s| DisplayMode { width: s.width, height: s.height, hz: s.hz }).collect()
}

/// Adds (`keep`) or drops a resolution of a display. In memory only; call `store.save()` afterwards.
pub fn register(store: &mut Store, dev: &str, m: DisplayMode, keep: bool) {
    let mut map = all(store);
    let list = map.entry(dev.to_string()).or_default();
    list.retain(|s| (s.width, s.height, s.hz) != (m.width, m.height, m.hz));
    if keep {
        list.push(Saved { width: m.width, height: m.height, hz: m.hz });
    }
    if list.is_empty() {
        map.remove(dev);
    }
    store.set_top("CustomResolutions", serde_json::to_value(map).unwrap_or_default());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display;

    fn m(width: u32, height: u32, hz: u32) -> DisplayMode {
        DisplayMode { width, height, hz }
    }

    #[test]
    fn layout_sizes() {
        use std::mem::size_of;
        assert_eq!(size_of::<TimingExt>(), 64);
        assert_eq!(size_of::<Timing>(), 96);
        assert_eq!(INPUT_VER, 0x10020);
        assert_eq!(CUSTOM_VER, 0x10090);
        assert_eq!(ADVANCED_VER, 0x10080);
        assert_eq!(size_of::<Target>(), 24);
        assert_eq!(size_of::<SourceMode>(), 32);
        assert_eq!(PATH_VER, 0x20030);
        assert_eq!(std::mem::offset_of!(CustomDisplay, timing), 44);
        assert_eq!(std::mem::offset_of!(Advanced, timing), 32);
    }

    #[test]
    fn validation() {
        // this PC's shape: a larger 60 Hz mode is listed above the native one
        let modes = [m(3840, 2160, 60), m(1920, 1080, 240), m(1920, 1080, 144), m(1920, 1080, 60), m(1280, 720, 240), m(1280, 720, 60)];
        let n = native_of(Some((1920, 1080)), &modes);
        assert_eq!(n, Some(m(1920, 1080, 240)));
        assert_eq!(native_of(None, &modes), n, "no preferred mode: highest rate, largest size");
        assert_eq!(native_of(Some((2560, 1440)), &modes), n, "preferred mode not listed");
        assert_eq!(native_of(Some((3840, 2160)), &modes), Some(m(3840, 2160, 60)));
        let v = |w, h| validate(w, h, n, &modes);
        assert_eq!(v(1720, 1080), Ok(m(1720, 1080, 240)), "refresh rate is the native maximum");
        assert_eq!(v(640, 480), Ok(m(640, 480, 240)));
        assert!(v(639, 480).is_err() && v(640, 479).is_err());
        assert!(v(1921, 1080).is_err() && v(1920, 1081).is_err() && v(3840, 2160).is_err());
        assert!(v(1280, 720).unwrap_err().contains("already lists"), "any listed rate counts");
        assert!(v(1920, 1080).is_err());
        assert!(validate(1720, 1080, None, &[]).is_err());
    }

    #[test]
    fn list_roundtrips_and_leaves_displays_alone() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("state.json"), r#"{"Displays":{"\\\\.\\DISPLAY1":{"Gamma":1.2}}}"#).unwrap();
        let mut st = Store::load(d.path());
        let (one, two) = (r"\\.\DISPLAY1", r"\\.\DISPLAY2");
        assert!(saved(&st, one).is_empty());
        register(&mut st, one, m(1720, 1080, 240), true);
        register(&mut st, one, m(1720, 1080, 240), true); // no duplicate
        register(&mut st, one, m(1440, 1080, 240), true);
        register(&mut st, two, m(1280, 960, 60), true);
        st.save().unwrap();
        let mut st = Store::load(d.path());
        assert_eq!(saved(&st, one), vec![m(1720, 1080, 240), m(1440, 1080, 240)]);
        assert_eq!(saved(&st, two), vec![m(1280, 960, 60)]);
        assert_eq!(st.top("CustomResolutions").unwrap()[one][0], serde_json::json!({"Width":1720,"Height":1080,"Hz":240}));
        assert_eq!(display::get_profile(&st, one).gamma, 1.2);
        register(&mut st, one, m(1720, 1080, 240), false);
        register(&mut st, two, m(1280, 960, 60), false);
        st.save().unwrap();
        let st = Store::load(d.path());
        assert_eq!(saved(&st, one), vec![m(1440, 1080, 240)]);
        assert!(saved(&st, two).is_empty() && st.top("CustomResolutions").unwrap().get(two).is_none());
    }

    fn describe(c: &CustomDisplay) -> String {
        let t = &c.timing;
        format!(
            "{}x{} depth {} fmt {} src {:?} ratio {}x{} flags {} | timing {}x{} total {}x{} rr {} rrx1k {} pclk {} status {:#x}",
            c.width, c.height, c.depth, c.color_format, c.src_partition, c.x_ratio, c.y_ratio, c.flags, t.h_visible, t.v_visible, t.h_total, t.v_total, t.etc.rr, t.etc.rrx1k, t.pclk, t.etc.status
        )
    }

    /// (device, is primary) of every attached display.
    fn displays() -> Vec<(String, bool)> {
        use windows::core::PCWSTR;
        use windows::Win32::Graphics::Gdi::{EnumDisplayDevicesW, DISPLAY_DEVICEW};
        let mut list = Vec::new();
        for i in 0.. {
            let mut d = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
            if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), i, &mut d, 0) }.as_bool() {
                break;
            }
            let name = String::from_utf16_lossy(&d.DeviceName[..d.DeviceName.iter().position(|&c| c == 0).unwrap_or(32)]);
            if d.StateFlags.0 & 1 != 0 {
                list.push((name, d.StateFlags.0 & 4 != 0));
            }
        }
        list
    }

    fn second() -> Option<String> {
        displays().into_iter().find(|d| !d.1).map(|d| d.0)
    }

    /// A size the display does not list: the native one, 200 px narrower.
    fn unlisted(dev: &str) -> (DisplayMode, DisplayMode) {
        let (now, modes) = display::get_modes(dev).expect("modes");
        let n = native(dev, &modes);
        eprintln!("  {dev} preferred {:?}, native {n:?}", preferred(dev));
        (now, validate(n.unwrap().width - 200, n.unwrap().height, n, &modes).expect("size is free"))
    }

    /// Reads only, changes nothing, every display: saved custom resolutions, scaling mode, automatic timing.
    /// `.\build.ps1 test -p mytune-core --lib customres::tests::real_read '--' --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_read() {
        assert!(crate::nvapi::available(), "no NVIDIA driver");
        for (dev, primary) in displays() {
            let id = display_id(&dev).unwrap();
            eprintln!("{dev} primary {primary} id {id:?} scaling {:?}", scaling(&dev));
            let Some(id) = id else { continue };
            for c in enumerate(id).expect("enum") {
                eprintln!("  saved: {}", describe(&c));
            }
            let (now, want) = unlisted(&dev);
            eprintln!("  now {now:?}; automatic timing for {want:?}: {:?}", custom(id, want).map(|c| describe(&c)));
            eprintln!("  automatic timing for now: {:?}", custom(id, now).map(|c| describe(&c)));
        }
    }

    /// Asks the driver whether it would accept GPU full-screen scaling on every display. Applies nothing:
    /// NV_DISPLAYCONFIG_VALIDATE_ONLY (1) only checks the configuration.
    /// `.\build.ps1 test -p mytune-core --lib customres::tests::real_validate_stretch '--' --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_validate_stretch() {
        assert!(crate::nvapi::available(), "no NVIDIA driver");
        for (dev, _) in displays() {
            let Some(id) = display_id(&dev).unwrap() else { continue };
            let mut c = config().expect("config");
            let a = c.target(id).expect("target");
            let old = a.scaling;
            a.scaling = SCALING_GPU_FULLSCREEN;
            let f = func::<FnSetConfig>(0x5D8CF8DE, "NvAPI_DISP_SetDisplayConfig").unwrap();
            let r = chk("NvAPI_DISP_SetDisplayConfig", unsafe { f(c.paths.len() as u32, c.paths.as_mut_ptr(), 1) });
            eprintln!("{dev}: scaling {old} -> {SCALING_GPU_FULLSCREEN} validate only: {r:?}; now {:?}", scaling(&dev));
            assert!(r.is_ok());
        }
    }

    /// Changes the NON-primary display for a few seconds: tries a custom resolution and reverts the trial,
    /// then sets another scaling mode, reads it back and restores the old one (also on a panic).
    /// Skips when there is one display. The scaling call hands the whole configuration to the driver.
    /// `.\build.ps1 test -p mytune-core --lib customres::tests::real_trial_and_scaling '--' --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_trial_and_scaling() {
        let Some(dev) = second() else { return eprintln!("no non-primary display: nothing tested") };
        let (before, want) = unlisted(&dev);
        eprintln!("{dev}: now {before:?}, trying {want:?}");
        try_mode(&dev, want).expect("try");
        std::thread::sleep(std::time::Duration::from_secs(2));
        let during = display::get_modes(&dev).map(|m| m.0);
        eprintln!("{dev} during the trial: {during:?}");
        revert(&dev).expect("revert");
        std::thread::sleep(std::time::Duration::from_secs(2));
        let after = display::get_modes(&dev).map(|m| m.0);
        eprintln!("{dev} after the revert: {after:?}");
        assert_eq!(after, Some(before));

        let old = scaling(&dev).expect("scaling");
        struct Restore(String, u32);
        impl Drop for Restore {
            fn drop(&mut self) {
                eprintln!("{} restore scaling {}: {:?}, now {:?}", self.0, self.1, set_scaling(&self.0, self.1), scaling(&self.0));
            }
        }
        let guard = Restore(dev.clone(), old);
        let other = if old == SCALING_GPU_FULLSCREEN { 5 } else { SCALING_GPU_FULLSCREEN };
        let set = set_scaling(&dev, other);
        let got = scaling(&dev);
        eprintln!("{dev} scaling {old} -> {other}: {set:?}, read back {got:?}");
        assert!(set.is_ok() && got.as_ref().ok() == Some(&other));
        drop(guard);
        assert_eq!(scaling(&dev).ok(), Some(old));
    }

    /// The Keep path on the NON-primary display: try, save, Windows lists the mode, switch back, delete,
    /// the driver no longer has it. Leaves nothing behind unless a step fails (then it says what).
    /// `.\build.ps1 test -p mytune-core --lib customres::tests::real_save_and_delete '--' --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_save_and_delete() {
        let Some(dev) = second() else { return eprintln!("no non-primary display: nothing tested") };
        let (before, want) = unlisted(&dev);
        try_mode(&dev, want).expect("try");
        std::thread::sleep(std::time::Duration::from_secs(2));
        let s = save(&dev);
        let (now, modes) = display::get_modes(&dev).expect("modes");
        eprintln!("{dev} save {s:?}; now {now:?}; Windows lists {want:?}: {}; driver list {:?}", modes.contains(&want), driver_list(&dev));
        if s.is_err() {
            eprintln!("revert {:?}", revert(&dev));
        }
        eprintln!("back to {before:?}: {:?}", display::set_mode(&dev, before, false));
        std::thread::sleep(std::time::Duration::from_secs(2));
        let del = delete(&dev, want);
        let left = driver_list(&dev);
        eprintln!("{dev} delete {del:?}; driver list {left:?}; now {:?}", display::get_modes(&dev).map(|m| m.0));
        assert!(s.is_ok() && modes.contains(&want), "saved and listed");
        assert!(del.is_ok() && !left.unwrap().contains(&want), "deleted");
        assert_eq!(display::get_modes(&dev).map(|m| m.0), Some(before));
    }
}
