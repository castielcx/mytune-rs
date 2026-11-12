//! Display page (C# `DisplayService`): monitors, resolution / refresh modes, colour curve, per-display colour
//! profiles in `state.json` (`Displays`), NVIDIA vibrance.
//!
//! Mode switch: `CDS_TEST` first, then the real change. A temporary change (no registry) lasts until the
//! next restart or mode change, which is what lets the UI try a mode and revert on a black screen.
//! Differences from C#:
//! - Every mode is listed, also those narrower than 800 px (C# hid them, so a low current mode could not be picked).
//! - No background watcher: Windows clears the gamma ramp after a restart, and nothing re-applies it here.
//!   (C# `DisplayService.Start` does; never run both.)
//! - Game Hub colour presets (`PresetService`) are not ported.

use crate::error::{Error, Result};
use crate::log;
use crate::store::Store;
use crate::vibrance::{self, VibranceRange};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use windows::core::PCWSTR;
use windows::Win32::Graphics::Gdi::{
    ChangeDisplaySettingsExW, CreateDCW, DeleteDC, EnumDisplayDevicesW, EnumDisplaySettingsW, CDS_TEST, CDS_TYPE, CDS_UPDATEREGISTRY, DEVMODEW,
    DEVMODE_FIELD_FLAGS, DISPLAY_DEVICEW, ENUM_CURRENT_SETTINGS, ENUM_DISPLAY_SETTINGS_MODE,
};
use windows::Win32::UI::ColorSystem::{GetDeviceGammaRamp, SetDeviceGammaRamp};

const DM_PELSWIDTH: u32 = 0x80000;
const DM_PELSHEIGHT: u32 = 0x100000;
const DM_DISPLAYFREQUENCY: u32 = 0x400000;
const DISPLAY_ATTACHED: u32 = 1;
const DM_INTERLACED: u32 = 2;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn text(a: &[u16]) -> String {
    String::from_utf16_lossy(&a[..a.iter().position(|&c| c == 0).unwrap_or(a.len())])
}

// ---------------------------------------------------------------- modes

/// Ordered like the C# page: width, height, refresh rate, all descending.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct DisplayMode {
    pub width: u32,
    pub height: u32,
    pub hz: u32,
}

impl DisplayMode {
    pub fn label(&self) -> String {
        format!("{} x {}", self.width, self.height)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Monitor {
    /// `\\.\DISPLAY1`
    pub device: String,
    pub label: String,
}

fn devmode() -> DEVMODEW {
    DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() }
}

fn enum_settings(dev: &[u16], i: ENUM_DISPLAY_SETTINGS_MODE, dm: &mut DEVMODEW) -> bool {
    unsafe { EnumDisplaySettingsW(PCWSTR(dev.as_ptr()), i, dm).as_bool() }
}

fn current(dev: &[u16]) -> Option<DEVMODEW> {
    let mut dm = devmode();
    enum_settings(dev, ENUM_CURRENT_SETTINGS, &mut dm).then_some(dm)
}

fn interlaced(dm: &DEVMODEW) -> bool {
    unsafe { dm.Anonymous2.dmDisplayFlags & DM_INTERLACED != 0 }
}

/// Non-interlaced modes at `bpp` with a real refresh rate, de-duplicated, largest first.
pub fn collect_modes(raw: impl IntoIterator<Item = (u32, u32, u32, u32, bool)>, bpp: u32) -> Vec<DisplayMode> {
    let set: BTreeSet<DisplayMode> = raw
        .into_iter()
        .filter(|&(_, _, hz, b, inter)| !inter && b == bpp && hz > 1)
        .map(|(width, height, hz, _, _)| DisplayMode { width, height, hz })
        .collect();
    set.into_iter().rev().collect()
}

/// The display's current mode and every mode it offers at the current colour depth.
pub fn get_modes(dev: &str) -> Option<(DisplayMode, Vec<DisplayMode>)> {
    let d = wide(dev);
    let cur = current(&d)?;
    let mut raw = Vec::new();
    let mut m = devmode();
    let mut i = 0;
    while enum_settings(&d, ENUM_DISPLAY_SETTINGS_MODE(i), &mut m) {
        raw.push((m.dmPelsWidth, m.dmPelsHeight, m.dmDisplayFrequency, m.dmBitsPerPel, interlaced(&m)));
        i += 1;
    }
    let now = DisplayMode { width: cur.dmPelsWidth, height: cur.dmPelsHeight, hz: cur.dmDisplayFrequency };
    Some((now, collect_modes(raw, cur.dmBitsPerPel)))
}

/// Current and highest refresh rate at the current resolution.
pub fn refresh(dev: &str) -> (u32, u32) {
    match get_modes(dev) {
        Some((now, all)) => {
            let max = all.iter().filter(|m| m.width == now.width && m.height == now.height).map(|m| m.hz).max().unwrap_or(0).max(now.hz);
            (now.hz, max)
        }
        None => (0, 0),
    }
}

/// Switches to a mode Windows accepts. `persist` false = until the next restart or mode change.
pub fn set_mode(dev: &str, mode: DisplayMode, persist: bool) -> Result<()> {
    let d = wide(dev);
    let mut dm = current(&d).ok_or_else(|| Error::Os(format!("Couldn't read {dev}.")))?;
    dm.dmPelsWidth = mode.width;
    dm.dmPelsHeight = mode.height;
    dm.dmDisplayFrequency = mode.hz;
    dm.dmFields = DEVMODE_FIELD_FLAGS(DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY);
    let change = |flags: CDS_TYPE| unsafe { ChangeDisplaySettingsExW(PCWSTR(d.as_ptr()), Some(&dm), None, flags, None) }.0;
    let mut r = change(CDS_TEST);
    if r == 0 {
        r = change(if persist { CDS_UPDATEREGISTRY } else { CDS_TYPE(0) });
    }
    if r != 0 {
        return Err(Error::Os(format!("Windows couldn't switch to {} x {} at {} Hz (code {r}).", mode.width, mode.height, mode.hz)));
    }
    log::info(&format!("display mode {dev} -> {} x {} @ {} Hz{}", mode.width, mode.height, mode.hz, if persist { "" } else { " (not saved yet)" }));
    Ok(())
}

pub fn monitors() -> Vec<Monitor> {
    let mut list = Vec::new();
    let mut i = 0;
    loop {
        let mut d = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        if !unsafe { EnumDisplayDevicesW(PCWSTR::null(), i, &mut d, 0) }.as_bool() {
            break;
        }
        i += 1;
        if d.StateFlags.0 & DISPLAY_ATTACHED == 0 {
            continue; // not attached to the desktop
        }
        let device = text(&d.DeviceName);
        let mut m = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
        let name = if unsafe { EnumDisplayDevicesW(PCWSTR(d.DeviceName.as_ptr()), 0, &mut m, 0) }.as_bool() { text(&m.DeviceString) } else { text(&d.DeviceString) };
        let num = vibrance::display_number(&device).map(|n| n.to_string()).unwrap_or_default();
        list.push(Monitor { label: format!("Display {num}: {name}"), device });
    }
    list
}

// ---------------------------------------------------------------- colour profile

/// Colour settings of one display, same JSON as C# `DisplayProfile`. Percent values: 100 = unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayProfile {
    #[serde(rename = "Gamma")]
    pub gamma: f64,
    /// -50..50, 0 = unchanged.
    #[serde(rename = "Brightness")]
    pub brightness: f64,
    #[serde(rename = "Contrast")]
    pub contrast: f64,
    #[serde(rename = "Red")]
    pub red: f64,
    #[serde(rename = "Green")]
    pub green: f64,
    #[serde(rename = "Blue")]
    pub blue: f64,
    /// NVIDIA digital vibrance as 0..100 percent; `None` = never touched.
    #[serde(rename = "Vibrance")]
    pub vibrance: Option<f64>,
    /// The driver's vibrance level before MyTune first changed it, restored on reset.
    #[serde(rename = "OriginalVibranceLevel")]
    pub original_vibrance_level: Option<i32>,
}

impl Default for DisplayProfile {
    fn default() -> Self {
        DisplayProfile { gamma: 1.0, brightness: 0.0, contrast: 100.0, red: 100.0, green: 100.0, blue: 100.0, vibrance: None, original_vibrance_level: None }
    }
}

impl DisplayProfile {
    pub fn ramp_is_default(&self) -> bool {
        self.gamma == 1.0 && self.brightness == 0.0 && self.contrast == 100.0 && self.red == 100.0 && self.green == 100.0 && self.blue == 100.0
    }
}

/// Saved profile of a display (`Displays` in state.json), or the neutral one.
pub fn get_profile(store: &Store, dev: &str) -> DisplayProfile {
    all_profiles(store).remove(dev).unwrap_or_default()
}

fn all_profiles(store: &Store) -> BTreeMap<String, DisplayProfile> {
    store.top("Displays").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

/// In memory only; call `store.save()` afterwards. `None` forgets the profile.
pub fn register(store: &mut Store, dev: &str, p: Option<&DisplayProfile>) {
    let mut all = all_profiles(store);
    match p {
        Some(p) => all.insert(dev.to_string(), p.clone()),
        None => all.remove(dev),
    };
    store.set_top("Displays", serde_json::to_value(all).unwrap_or_default());
}

/// 3 x 256 ramp: contrast, brightness, gamma, then per-channel balance. Each entry stays within 0.49 of
/// linear because Windows rejects ramps that stray further.
pub fn build_ramp(p: &DisplayProfile) -> Vec<u16> {
    let mut ramp = vec![0u16; 3 * 256];
    let gain = [p.red / 100.0, p.green / 100.0, p.blue / 100.0];
    for i in 0..256 {
        let linear = i as f64 / 255.0;
        let x = (linear - 0.5) * (p.contrast / 100.0) + 0.5 + p.brightness / 100.0;
        let x = x.clamp(0.0, 1.0).powf(1.0 / p.gamma.max(0.1));
        for (c, g) in gain.iter().enumerate() {
            let y = (x * g).clamp(linear - 0.49, linear + 0.49);
            ramp[c * 256 + i] = (y.clamp(0.0, 1.0) * 65535.0).round_ties_even() as u16;
        }
    }
    ramp
}

pub fn apply_ramp(dev: &str, p: &DisplayProfile) -> Result<()> {
    let ramp = build_ramp(p);
    let d = wide(dev);
    let hdc = unsafe { CreateDCW(PCWSTR(d.as_ptr()), PCWSTR::null(), PCWSTR::null(), None) };
    if hdc.is_invalid() {
        return Err(Error::Os(format!("Couldn't open {dev}.")));
    }
    let ok = unsafe { SetDeviceGammaRamp(hdc, ramp.as_ptr().cast()) }.as_bool();
    unsafe {
        let _ = DeleteDC(hdc);
    }
    if ok {
        Ok(())
    } else {
        Err(Error::Os("Windows rejected the color curve for this display.".into()))
    }
}

/// The ramp the display has right now (tests and tools).
pub fn current_ramp(dev: &str) -> Option<Vec<u16>> {
    let d = wide(dev);
    let hdc = unsafe { CreateDCW(PCWSTR(d.as_ptr()), PCWSTR::null(), PCWSTR::null(), None) };
    if hdc.is_invalid() {
        return None;
    }
    let mut have = vec![0u16; 3 * 256];
    let ok = unsafe { GetDeviceGammaRamp(hdc, have.as_mut_ptr().cast()) }.as_bool();
    unsafe {
        let _ = DeleteDC(hdc);
    }
    ok.then_some(have)
}

// ---------------------------------------------------------------- vibrance

fn number(dev: &str) -> Option<u32> {
    vibrance::display_number(dev)
}

/// Vibrance range of a display, or `None` when it is not driven by an NVIDIA GPU (errors are logged).
pub fn get_vibrance(dev: &str) -> Option<VibranceRange> {
    match vibrance::get(number(dev)?) {
        Ok(r) => r,
        Err(e) => {
            log::error("vibrance range", Some(&e));
            None
        }
    }
}

/// Applies `p.vibrance` and remembers the level that was there before (first time only).
pub fn apply_vibrance(dev: &str, p: &mut DisplayProfile) -> Result<()> {
    let Some(v) = p.vibrance else { return Ok(()) };
    let n = number(dev).ok_or_else(|| Error::Os("Digital vibrance needs an NVIDIA GPU on this display.".into()))?;
    let r = vibrance::get(n)?.ok_or_else(|| Error::Os("Digital vibrance needs an NVIDIA GPU on this display.".into()))?;
    p.original_vibrance_level.get_or_insert(r.current);
    let level = r.level_for(v);
    if level != r.current {
        vibrance::set(n, level)?;
    }
    Ok(())
}

/// Copies another display's colours to `dev`; vibrance only where NVIDIA drives it.
pub fn copy_to(store: &mut Store, src: &DisplayProfile, dev: &str) -> Result<()> {
    let existing = get_profile(store, dev);
    let mut copy = DisplayProfile {
        gamma: src.gamma,
        brightness: src.brightness,
        contrast: src.contrast,
        red: src.red,
        green: src.green,
        blue: src.blue,
        vibrance: existing.vibrance,
        original_vibrance_level: existing.original_vibrance_level,
    };
    if src.vibrance.is_some() && get_vibrance(dev).is_some() {
        copy.vibrance = src.vibrance;
    }
    register(store, dev, Some(&copy));
    apply_ramp(dev, &copy)?;
    apply_vibrance(dev, &mut copy)?;
    register(store, dev, Some(&copy)); // original level may have been captured
    store.save()
}

/// Neutral curve, the vibrance MyTune found, and forget the profile.
pub fn reset(store: &mut Store, dev: &str) -> Result<()> {
    let old = get_profile(store, dev);
    register(store, dev, None);
    apply_ramp(dev, &DisplayProfile::default())?;
    if let (Some(level), Some(n)) = (old.original_vibrance_level, number(dev)) {
        if let Err(e) = vibrance::set(n, level) {
            log::error("vibrance reset", Some(&e));
        }
    }
    store.save()?;
    log::info(&format!("display reset {dev}"));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_filter_dedupe_and_sort() {
        // (w, h, hz, bpp, interlaced)
        let raw = vec![
            (1920, 1080, 60, 32, false),
            (1920, 1080, 144, 32, false),
            (1920, 1080, 144, 32, false),
            (1920, 1080, 120, 16, false), // other colour depth
            (1920, 1080, 50, 32, true),   // interlaced
            (2560, 1440, 1, 32, false),   // not a real rate
            (2560, 1440, 165, 32, false),
            (640, 480, 60, 32, false), // narrow modes stay listed
        ];
        let l = collect_modes(raw, 32);
        let got: Vec<(u32, u32, u32)> = l.iter().map(|m| (m.width, m.height, m.hz)).collect();
        assert_eq!(got, [(2560, 1440, 165), (1920, 1080, 144), (1920, 1080, 60), (640, 480, 60)]);
        assert_eq!(l[0].label(), "2560 x 1440");
    }

    #[test]
    fn default_ramp_is_identity() {
        let r = build_ramp(&DisplayProfile::default());
        for c in 0..3 {
            for i in 0..256 {
                let want = (i as f64 / 255.0 * 65535.0).round() as i32;
                assert!((r[c * 256 + i] as i32 - want).abs() <= 1, "c{c} i{i}");
            }
        }
    }

    #[test]
    fn ramp_follows_settings_and_stays_near_linear() {
        let p = DisplayProfile { gamma: 2.0, red: 50.0, ..Default::default() };
        let r = build_ramp(&p);
        let mid = 128;
        assert!(r[mid] < r[256 + mid], "red gain 50 % darkens red only");
        for c in 0..3 {
            for i in 0..256 {
                let lin = i as f64 / 255.0 * 65535.0;
                assert!((r[c * 256 + i] as f64 - lin).abs() <= 0.49 * 65535.0 + 1.0);
            }
        }
        // extreme brightness is clamped to the 0..1 range
        let hi = build_ramp(&DisplayProfile { brightness: 50.0, ..Default::default() });
        assert_eq!(hi[255], 65535);
    }

    #[test]
    fn profile_json_is_the_csharp_shape_and_store_roundtrips() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("state.json"),
            r#"{"Games":[1],"Displays":{"\\\\.\\DISPLAY1":{"Gamma":1.2,"Brightness":0,"Contrast":100,"Red":100,"Green":100,"Blue":100,"Vibrance":60.0,"OriginalVibranceLevel":null}}}"#,
        )
        .unwrap();
        let mut st = Store::load(d.path());
        let mut p = get_profile(&st, r"\\.\DISPLAY1");
        assert_eq!((p.gamma, p.vibrance, p.original_vibrance_level), (1.2, Some(60.0), None));
        assert_eq!(get_profile(&st, r"\\.\DISPLAY9"), DisplayProfile::default());
        p.gamma = 0.8;
        p.original_vibrance_level = Some(0);
        register(&mut st, r"\\.\DISPLAY1", Some(&p));
        register(&mut st, r"\\.\DISPLAY2", Some(&DisplayProfile::default()));
        st.save().unwrap();
        let st2 = Store::load(d.path());
        assert_eq!(get_profile(&st2, r"\\.\DISPLAY1").gamma, 0.8);
        assert!(st2.top("Games").is_some());
        let mut st3 = st2;
        register(&mut st3, r"\\.\DISPLAY2", None);
        assert_eq!(all_profiles(&st3).len(), 1);
        let raw = st3.top("Displays").unwrap().to_string();
        assert!(raw.contains("\"OriginalVibranceLevel\":0") && raw.contains("\"Vibrance\":60.0"), "{raw}");
    }

    /// Read only: every attached monitor has a current mode and a rate list that contains it.
    #[test]
    fn real_monitors_report_modes() {
        let ms = monitors();
        eprintln!("{ms:?}");
        for m in &ms {
            let (now, all) = get_modes(&m.device).unwrap();
            assert!(!all.is_empty());
            assert!(all.contains(&now), "{now:?} not in list");
            let (cur, max) = refresh(&m.device);
            assert!(cur >= 1 && max >= cur);
        }
    }
}
