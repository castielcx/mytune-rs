//! Display page (C# `DisplayService`): monitors, resolution / refresh modes, colour curve, per-display colour
//! profiles in `state.json` (`Displays`), NVIDIA vibrance.
//!
//! Mode switch: `CDS_TEST` first, then the real change. A temporary change (no registry) lasts until the
//! next restart or mode change, which is what lets the UI try a mode and revert on a black screen.
//! Differences from C#:
//! - Every mode is listed, also those narrower than 800 px (C# hid them, so a low current mode could not be picked).
//! - Colours are applied by one worker thread (`want`, `apply_saved`), never by the caller: it also puts the
//!   curve back every 5 s when a game or a mode change cleared it. (C# `DisplayService.Start` does the same;
//!   never run both.)
//! - `Shadows` (shadow lift) exists only here; the C# app does not know the field.
//! - Game Hub colour presets (`PresetService`) are not ported.

use crate::error::{Error, Result};
use crate::log;
use crate::store::Store;
use crate::vibrance::{self, VibranceRange};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::sync::{Condvar, Mutex, Once};
use std::time::Duration;
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
    /// Shadow lift 0..100: brightens dark areas only, 0 = unchanged.
    #[serde(rename = "Shadows")]
    pub shadows: f64,
    /// Extra saturation 0..100 through NVIDIA's colour conversion matrix (beyond Digital Vibrance), 0 = off.
    #[serde(rename = "Saturation")]
    pub saturation: f64,
    /// NVIDIA digital vibrance as 0..100 percent; `None` = never touched.
    #[serde(rename = "Vibrance")]
    pub vibrance: Option<f64>,
    /// The driver's vibrance level before MyTune first changed it, restored on reset.
    #[serde(rename = "OriginalVibranceLevel")]
    pub original_vibrance_level: Option<i32>,
}

impl Default for DisplayProfile {
    fn default() -> Self {
        DisplayProfile { gamma: 1.0, brightness: 0.0, contrast: 100.0, red: 100.0, green: 100.0, blue: 100.0, shadows: 0.0, saturation: 0.0, vibrance: None, original_vibrance_level: None }
    }
}

impl DisplayProfile {
    /// Nothing for the worker to do at program start.
    fn is_neutral(&self) -> bool {
        self.ramp_is_default() && self.vibrance.is_none() && self.saturation <= 0.0
    }

    pub fn ramp_is_default(&self) -> bool {
        self.gamma == 1.0 && self.brightness == 0.0 && self.contrast == 100.0 && self.red == 100.0 && self.green == 100.0 && self.blue == 100.0 && self.shadows == 0.0
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

/// 3 x 256 ramp: contrast, brightness, gamma, shadow lift, then per-channel balance. Each entry stays within
/// 0.49 of linear because Windows rejects ramps that stray further.
pub fn build_ramp(p: &DisplayProfile) -> Vec<u16> {
    let mut ramp = vec![0u16; 3 * 256];
    let gain = [p.red / 100.0, p.green / 100.0, p.blue / 100.0];
    // Shadow lift: x + a * x * (1 - x)^6. Black stays black, the bump peaks at 14 % input (+0.11 at 100)
    // and is under 0.016 from 50 % up; the slope stays positive for a < 5.4.
    let lift = 2.0 * p.shadows.clamp(0.0, 100.0) / 100.0;
    for i in 0..256 {
        let linear = i as f64 / 255.0;
        let x = (linear - 0.5) * (p.contrast / 100.0) + 0.5 + p.brightness / 100.0;
        let x = x.clamp(0.0, 1.0).powf(1.0 / p.gamma.max(0.1));
        let x = x + lift * x * (1.0 - x).powi(6);
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

/// Sets the driver to `p.vibrance`. The level found before MyTune's first change is kept by the caller in
/// `original_vibrance_level` (from `get_vibrance(..).current`), not here.
fn apply_vibrance(dev: &str, p: &DisplayProfile) -> Result<()> {
    let Some(v) = p.vibrance else { return Ok(()) };
    let n = number(dev).ok_or_else(|| Error::Os("Digital vibrance needs an NVIDIA GPU on this display.".into()))?;
    let r = vibrance::get(n)?.ok_or_else(|| Error::Os("Digital vibrance needs an NVIDIA GPU on this display.".into()))?;
    let level = r.level_for(v);
    if level != r.current {
        vibrance::set(n, level)?;
    }
    Ok(())
}

/// Saturation factor of the colour matrix for `Saturation` 0..100: 1.0..2.0.
pub fn saturation_factor(extra: f64) -> f64 {
    1.0 + extra.clamp(0.0, 100.0) / 100.0
}

/// Puts the extra saturation of `p` on the display. A conversion that is not a plain saturation matrix
/// (novideo_srgb, an ICC clamp) belongs to another tool and is never overwritten. The message is for the user.
fn apply_saturation(dev: &str, p: &DisplayProfile) -> std::result::Result<(), String> {
    let on = p.saturation > 0.0;
    let now = match vibrance::csc_get(dev) {
        Ok(Some(c)) => c,
        Ok(None) if on => return Err("Extra saturation needs an NVIDIA graphics card on this display.".into()),
        Ok(None) => return Ok(()),
        Err(_) if !on => return Ok(()),
        Err(e) => return Err(e.to_string()),
    };
    let ours = now.factor().is_some();
    let want = if on { vibrance::Csc::saturation(saturation_factor(p.saturation)) } else { vibrance::Csc::off() };
    if now.same(&want) || (!on && !ours) {
        return Ok(());
    }
    if !now.is_off() && !ours {
        return Err("Another colour tool is already changing this display, so extra saturation was left off.".into());
    }
    vibrance::csc_set(dev, &want).map_err(|e| match e {
        Error::Nvapi { status: crate::nvapi::NOT_SUPPORTED, .. } => "Turn off Windows HDR to use extra saturation".into(),
        e => e.to_string(),
    })
}

/// Stores a copy of another display's colours for `dev` (in memory; save and `want` it afterwards).
/// `range` is `get_vibrance(dev)`: vibrance and extra saturation are copied only where NVIDIA drives the display.
pub fn copy_to(store: &mut Store, src: &DisplayProfile, dev: &str, range: Option<VibranceRange>) -> DisplayProfile {
    let existing = get_profile(store, dev);
    let mut copy = DisplayProfile { vibrance: existing.vibrance, original_vibrance_level: existing.original_vibrance_level, ..src.clone() };
    if range.is_none() {
        copy.saturation = existing.saturation;
    }
    if let (Some(v), Some(r)) = (src.vibrance, range) {
        copy.vibrance = Some(v);
        copy.original_vibrance_level.get_or_insert(r.current);
    }
    register(store, dev, Some(&copy));
    copy
}

/// Forgets the profile and saves. Returns the vibrance level MyTune found before its first change, for
/// `want_reset`.
pub fn reset(store: &mut Store, dev: &str) -> Result<Option<i32>> {
    let old = get_profile(store, dev);
    register(store, dev, None);
    store.save()?;
    log::info(&format!("display reset {dev}"));
    Ok(old.original_vibrance_level)
}

// ---------------------------------------------------------------- colour worker

/// What a display should look like: the profile, and for a reset the raw driver vibrance level to put back.
struct Wanted {
    profile: DisplayProfile,
    level: Option<i32>,
}

static QUEUE: Mutex<BTreeMap<String, Wanted>> = Mutex::new(BTreeMap::new());
static WAKE: Condvar = Condvar::new();
static PROBLEM: Mutex<Option<String>> = Mutex::new(None);

/// The last colour change that failed, once (for a notice).
pub fn take_problem() -> Option<String> {
    PROBLEM.lock().unwrap_or_else(|p| p.into_inner()).take()
}

/// Asks the worker to give `dev` this look. Returns at once; a newer call for the same display replaces
/// one the worker has not started yet, so a slider drag never queues up.
pub fn want(dev: &str, profile: &DisplayProfile) {
    queue(dev, Wanted { profile: profile.clone(), level: None });
}

/// Neutral curve, and the driver vibrance `level` when there is one (from `reset`).
pub fn want_reset(dev: &str, level: Option<i32>) {
    queue(dev, Wanted { profile: DisplayProfile::default(), level });
}

/// At program start: applies every saved profile of an attached display that changes something.
/// Starts no thread when there is none.
pub fn apply_saved(store: &Store) {
    let all = all_profiles(store);
    if all.values().all(|p| p.is_neutral()) {
        return;
    }
    // listing monitors is a GDI call: not under the caller's store lock
    std::thread::spawn(move || {
        for m in monitors() {
            if let Some(p) = all.iter().find(|(d, _)| d.eq_ignore_ascii_case(&m.device)).map(|(_, p)| p) {
                if !p.is_neutral() {
                    want(&m.device, p);
                }
            }
        }
    });
}

fn queue(dev: &str, w: Wanted) {
    static START: Once = Once::new();
    QUEUE.lock().unwrap_or_else(|p| p.into_inner()).insert(dev.to_string(), w);
    START.call_once(|| {
        let _ = std::thread::Builder::new().name("Colours".into()).spawn(worker);
    });
    WAKE.notify_one();
}

fn worker() {
    // display -> (profile, the curve Windows reported right after it was set); only displays whose curve is
    // not neutral or that have extra saturation
    let mut kept: BTreeMap<String, (DisplayProfile, Option<Vec<u16>>)> = BTreeMap::new();
    loop {
        let jobs = {
            let mut q = QUEUE.lock().unwrap_or_else(|p| p.into_inner());
            if q.is_empty() {
                q = if kept.is_empty() {
                    WAKE.wait(q).unwrap_or_else(|p| p.into_inner()) // every curve neutral: sleep until a change
                } else {
                    WAKE.wait_timeout(q, Duration::from_secs(5)).unwrap_or_else(|p| p.into_inner()).0
                };
            }
            std::mem::take(&mut *q)
        };
        if jobs.is_empty() {
            // a game in exclusive fullscreen or a mode change cleared the curve: put it back
            for (dev, (p, seen)) in kept.iter_mut() {
                let now = current_ramp(dev);
                if !p.ramp_is_default() && now.is_some() && now != *seen && apply_ramp(dev, p).is_ok() {
                    *seen = current_ramp(dev);
                }
                // same for the saturation matrix; one refusal (HDR switched on) ends the retries
                if p.saturation > 0.0 && matches!(vibrance::csc_get(dev), Ok(Some(c)) if c.is_off()) {
                    if let Err(e) = apply_saturation(dev, p) {
                        p.saturation = 0.0;
                        log::info(&format!("display saturation {dev}: {e}"));
                        *PROBLEM.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
                    }
                }
            }
            kept.retain(|_, (p, _)| !p.ramp_is_default() || p.saturation > 0.0);
            continue;
        }
        for (dev, w) in jobs {
            let mut r = apply_ramp(&dev, &w.profile);
            let vib = match (w.level, number(&dev)) {
                (Some(level), Some(n)) => vibrance::set(n, level),
                _ => apply_vibrance(&dev, &w.profile),
            };
            if r.is_ok() {
                r = vib;
            }
            if let Err(e) = r {
                log::error(&format!("display colours {dev}"), Some(&e));
                *PROBLEM.lock().unwrap_or_else(|p| p.into_inner()) = Some(e.to_string());
            }
            let mut w = w;
            if let Err(e) = apply_saturation(&dev, &w.profile) {
                w.profile.saturation = 0.0; // not on: nothing to put back later
                log::info(&format!("display saturation {dev}: {e}"));
                *PROBLEM.lock().unwrap_or_else(|p| p.into_inner()) = Some(e);
            }
            if w.profile.ramp_is_default() && w.profile.saturation <= 0.0 {
                kept.remove(&dev);
            } else {
                let seen = current_ramp(&dev);
                kept.insert(dev, (w.profile, seen));
            }
        }
    }
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

    #[test]
    fn default_ramp_is_exactly_identity() {
        let r = build_ramp(&DisplayProfile::default());
        for i in 0..256 {
            assert_eq!([r[i], r[256 + i], r[512 + i]], [(i * 257) as u16; 3], "i{i}");
        }
        assert!(DisplayProfile::default().ramp_is_default());
        assert!(!DisplayProfile { shadows: 1.0, ..Default::default() }.ramp_is_default());
    }

    #[test]
    fn ramp_never_falls_at_the_slider_ends() {
        for gamma in [0.30, 1.0, 2.80] {
            for brightness in [-50.0, 0.0, 50.0] {
                for contrast in [50.0, 100.0, 150.0] {
                    for shadows in [0.0, 100.0] {
                        let p = DisplayProfile { gamma, brightness, contrast, shadows, red: 50.0, ..Default::default() };
                        let r = build_ramp(&p);
                        for c in 0..3 {
                            assert!(r[c * 256..(c + 1) * 256].windows(2).all(|w| w[0] <= w[1]), "{p:?} c{c}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn shadow_lift_brightens_the_dark_end_only() {
        let r = build_ramp(&DisplayProfile { shadows: 100.0, ..Default::default() });
        assert_eq!((r[0], r[255]), (0, 65535));
        assert!(r[36] as f64 > 36.0 * 257.0 + 0.10 * 65535.0, "dark greys are lifted: {}", r[36]);
        for i in 128..256 {
            assert!((r[i] as f64 - i as f64 * 257.0).abs() < 0.02 * 65535.0, "i{i}");
        }
    }

    #[test]
    fn shadows_field_reads_old_files_and_roundtrips() {
        let old: DisplayProfile = serde_json::from_str(r#"{"Gamma":1.2,"Brightness":0,"Contrast":100,"Red":100,"Green":100,"Blue":100}"#).unwrap();
        assert_eq!(old.shadows, 0.0);
        let json = serde_json::to_string(&DisplayProfile { shadows: 40.0, ..old }).unwrap();
        assert!(json.contains("\"Shadows\":40.0"), "{json}");
        assert_eq!(serde_json::from_str::<DisplayProfile>(&json).unwrap().shadows, 40.0);
        // extra saturation: absent in old files = off; 0..100 maps to a matrix factor of 1..2
        assert_eq!(old.saturation, 0.0);
        let json = serde_json::to_string(&DisplayProfile { saturation: 30.0, ..Default::default() }).unwrap();
        assert!(json.contains("\"Saturation\":30.0"), "{json}");
        assert_eq!(serde_json::from_str::<DisplayProfile>(&json).unwrap().saturation, 30.0);
        assert_eq!((saturation_factor(0.0), saturation_factor(50.0), saturation_factor(500.0)), (1.0, 1.5, 2.0));
        assert!(DisplayProfile::default().is_neutral() && !DisplayProfile { saturation: 1.0, ..Default::default() }.is_neutral());
    }

    /// Changes the screen for a moment: sets the strongest curves and another vibrance level on every
    /// display through the real calls, reads them back, then restores what was there.
    /// `cargo test -p mytune-core --lib display::tests::real_colour_calls -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real_colour_calls() {
        for m in monitors() {
            let before = current_ramp(&m.device).expect("GetDeviceGammaRamp");
            let extremes = [
                DisplayProfile { gamma: 0.30, ..Default::default() },
                DisplayProfile { gamma: 2.80, ..Default::default() },
                DisplayProfile { brightness: -50.0, ..Default::default() },
                DisplayProfile { brightness: 50.0, ..Default::default() },
                DisplayProfile { contrast: 50.0, ..Default::default() },
                DisplayProfile { contrast: 150.0, ..Default::default() },
                DisplayProfile { shadows: 100.0, ..Default::default() },
                DisplayProfile { gamma: 2.80, brightness: 50.0, contrast: 150.0, shadows: 100.0, ..Default::default() },
                DisplayProfile { gamma: 0.30, brightness: -50.0, contrast: 150.0, red: 50.0, ..Default::default() },
            ];
            for p in &extremes {
                let set = apply_ramp(&m.device, p);
                let same = current_ramp(&m.device).as_deref() == Some(&build_ramp(p)[..]);
                eprintln!("{} g{} b{} c{} s{}: set {:?}, read back equal: {same}", m.device, p.gamma, p.brightness, p.contrast, p.shadows, set.is_ok());
                assert!(set.is_ok());
            }
            let d = wide(&m.device);
            let hdc = unsafe { CreateDCW(PCWSTR(d.as_ptr()), PCWSTR::null(), PCWSTR::null(), None) };
            let restored = unsafe { SetDeviceGammaRamp(hdc, before.as_ptr().cast()) }.as_bool();
            unsafe {
                let _ = DeleteDC(hdc);
            }
            eprintln!("{} curve restored: {restored}", m.device);
            assert!(restored && current_ramp(&m.device) == Some(before));

            match get_vibrance(&m.device) {
                None => eprintln!("{} no NVIDIA vibrance", m.device),
                Some(r) => {
                    let n = number(&m.device).unwrap();
                    let other = if r.current == r.max { r.max - 1 } else { r.current + 1 };
                    vibrance::set(n, other).unwrap();
                    let got = vibrance::get(n).unwrap().unwrap().current;
                    vibrance::set(n, r.current).unwrap();
                    let back = vibrance::get(n).unwrap().unwrap().current;
                    eprintln!("{} vibrance {r:?}: set {other}, read {got}, restored to {back}", m.device);
                    assert_eq!((got, back), (other, r.current));
                }
            }
        }
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
