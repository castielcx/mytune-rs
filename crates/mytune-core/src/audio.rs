//! Volume mixer behind the Sound page: every output device with its master volume, and every app playing on
//! it with its own volume, mute and level meter (Windows Core Audio sessions).
//!
//! One worker thread owns the COM objects. It sends a `Snapshot` about 20 times a second while the page
//! pokes it, and does nothing once the pokes stop (page left, window hidden).
//! "Set default" and "Move app" have no public Windows API: like EarTrumpet they call undocumented
//! interfaces by vtable slot (`set_default`, `move_app`). A Windows update can break them; they then
//! return an error and nothing else is affected.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::{IInspectable, IUnknown, Interface, GUID, HRESULT, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Media::Audio::Endpoints::{IAudioEndpointVolume, IAudioMeterInformation};
use windows::Win32::Media::Audio::{
    eMultimedia, eRender, AudioSessionStateExpired, IAudioSessionControl2, IAudioSessionManager2, IMMDevice,
    IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToBSTR};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ};
use windows::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION};
use windows::Win32::System::WinRT::RoGetActivationFactory;

/// One app on one device. Every audio session of the same exe is merged into one row, as EarTrumpet does.
#[derive(Clone, Debug, PartialEq)]
pub struct AppRow {
    pub pids: Vec<u32>,
    /// Windows' own sounds (no process).
    pub system: bool,
    /// Full exe path; empty for system sounds or when the process cannot be opened.
    pub path: String,
    pub volume: f32,
    pub muted: bool,
    pub peak: f32,
}

#[derive(Clone, Debug)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub volume: f32,
    pub muted: bool,
    pub peak: f32,
    pub apps: Vec<AppRow>,
}

pub enum Msg {
    Snapshot(Vec<Device>),
    Error(String),
}

pub enum Cmd {
    DeviceVolume { device: String, volume: f32 },
    DeviceMute { device: String, muted: bool },
    AppVolume { device: String, pids: Vec<u32>, volume: f32 },
    AppMute { device: String, pids: Vec<u32>, muted: bool },
    SetDefault { device: String },
    Move { pids: Vec<u32>, device: String },
}

/// One audio session as read from Windows, before merging.
#[derive(Clone, Debug)]
pub struct Raw {
    pub pid: u32,
    pub path: String,
    pub volume: f32,
    pub muted: bool,
    pub peak: f32,
}

/// Sessions of one exe become one row: loudest volume and peak, muted only when all are. System sounds
/// (pid 0) first, then by exe path so rows do not jump between scans.
pub fn merge(raw: &[Raw]) -> Vec<AppRow> {
    let mut rows: Vec<AppRow> = Vec::new();
    for r in raw {
        let same = |a: &AppRow| if r.pid == 0 { a.system } else if r.path.is_empty() { a.pids.contains(&r.pid) } else { a.path.eq_ignore_ascii_case(&r.path) };
        match rows.iter_mut().find(|a| same(a)) {
            Some(a) => {
                if !a.pids.contains(&r.pid) {
                    a.pids.push(r.pid);
                }
                a.volume = a.volume.max(r.volume);
                a.peak = a.peak.max(r.peak);
                a.muted &= r.muted;
            }
            None => rows.push(AppRow { pids: vec![r.pid], system: r.pid == 0, path: r.path.clone(), volume: r.volume, muted: r.muted, peak: r.peak }),
        }
    }
    rows.sort_by_key(|a| (!a.system, a.path.to_ascii_lowercase(), a.pids[0]));
    rows
}

struct Sess {
    pid: u32,
    path: String,
    vol: ISimpleAudioVolume,
    meter: Option<IAudioMeterInformation>,
}

struct Dev {
    id: String,
    name: String,
    is_default: bool,
    vol: IAudioEndpointVolume,
    meter: Option<IAudioMeterInformation>,
    sessions: Vec<Sess>,
}

fn exe_path(pid: u32) -> String {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return String::new() };
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut n).is_ok();
        let _ = CloseHandle(h);
        if ok { String::from_utf16_lossy(&buf[..n as usize]) } else { String::new() }
    }
}

/// How a Store app shows itself in the Start menu.
pub struct Look {
    pub name: String,
    /// Width, height, RGBA with premultiplied alpha.
    pub icon: Option<(usize, usize, Vec<u8>)>,
}

/// Name and logo of a packaged (Microsoft Store) app, from its Start menu entry. Their exe has no description
/// and no icon (SoundCloud's is `soundcouch.exe`). None for ordinary programs. Call on a thread with COM.
pub fn packaged(pid: u32) -> Option<Look> {
    use windows::Win32::Foundation::{ERROR_SUCCESS, SIZE};
    use windows::Win32::Graphics::Gdi::{DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS};
    use windows::Win32::Storage::Packaging::Appx::GetApplicationUserModelId;
    use windows::Win32::UI::Shell::{IShellItem, IShellItemImageFactory, SHCreateItemInKnownFolder, FOLDERID_AppsFolder, KF_FLAG_DEFAULT, SIGDN_NORMALDISPLAY, SIIGBF_RESIZETOFIT};
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut id = [0u16; 256];
        let mut n = id.len() as u32;
        let rc = GetApplicationUserModelId(h, &mut n, Some(PWSTR(id.as_mut_ptr())));
        let _ = CloseHandle(h);
        if rc != ERROR_SUCCESS {
            return None; // not a packaged app
        }
        let item: IShellItem = SHCreateItemInKnownFolder(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, PCWSTR(id.as_ptr())).ok()?;
        let p = item.GetDisplayName(SIGDN_NORMALDISPLAY).ok()?;
        let name = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const c_void));
        let icon = item.cast::<IShellItemImageFactory>().ok().and_then(|f| f.GetImage(SIZE { cx: 48, cy: 48 }, SIIGBF_RESIZETOFIT).ok()).and_then(|bmp| {
            let mut bm = BITMAP::default();
            let got = GetObjectW(bmp.into(), std::mem::size_of::<BITMAP>() as i32, Some((&mut bm as *mut BITMAP).cast()));
            let (w, hgt) = (bm.bmWidth, bm.bmHeight);
            let mut px = vec![0u8; (w.max(0) * hgt.max(0) * 4) as usize];
            let mut bi = BITMAPINFO::default();
            bi.bmiHeader = BITMAPINFOHEADER { biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32, biWidth: w, biHeight: -hgt, biPlanes: 1, biBitCount: 32, biCompression: BI_RGB.0, ..Default::default() };
            let dc = GetDC(None);
            let rows = if got != 0 && !px.is_empty() { GetDIBits(dc, bmp, 0, hgt as u32, Some(px.as_mut_ptr().cast()), &mut bi, DIB_RGB_COLORS) } else { 0 };
            ReleaseDC(None, dc);
            let _ = DeleteObject(bmp.into());
            if rows == 0 {
                return None;
            }
            let opaque = px.chunks(4).all(|c| c[3] == 0); // no alpha channel at all
            for c in px.chunks_mut(4) {
                c.swap(0, 2); // BGRA -> RGBA
                if opaque {
                    c[3] = 255;
                }
            }
            Some((w as usize, hgt as usize, px))
        });
        Some(Look { name: name?, icon })
    }
}

unsafe fn id_of(d: &IMMDevice) -> Option<String> {
    unsafe {
        let p = d.GetId().ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const c_void));
        s
    }
}

unsafe fn open(d: &IMMDevice, default: &Option<String>) -> windows::core::Result<Dev> {
    unsafe {
        let id = id_of(d).unwrap_or_default();
        let mut pv = d.OpenPropertyStore(STGM_READ)?.GetValue(&PKEY_Device_FriendlyName)?;
        let name = PropVariantToBSTR(&pv).map(|b| b.to_string()).unwrap_or_default();
        let _ = PropVariantClear(&mut pv);
        let mut sessions = Vec::new();
        let list = d.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None)?.GetSessionEnumerator()?;
        for i in 0..list.GetCount()? {
            let Ok(c) = list.GetSession(i) else { continue };
            if c.GetState() == Ok(AudioSessionStateExpired) {
                continue;
            }
            let (Ok(c2), Ok(vol)) = (c.cast::<IAudioSessionControl2>(), c.cast::<ISimpleAudioVolume>()) else { continue };
            // S_OK = the system sounds session, S_FALSE = an app's
            let pid = if c2.IsSystemSoundsSession() == HRESULT(0) { 0 } else { c2.GetProcessId().unwrap_or(0) };
            sessions.push(Sess { pid, path: if pid == 0 { String::new() } else { exe_path(pid) }, vol, meter: c.cast().ok() });
        }
        Ok(Dev {
            is_default: default.as_deref() == Some(id.as_str()),
            id,
            name,
            vol: d.Activate(CLSCTX_ALL, None)?,
            meter: d.Activate(CLSCTX_ALL, None).ok(),
            sessions,
        })
    }
}

fn scan() -> windows::core::Result<Vec<Dev>> {
    unsafe {
        let en: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let default = en.GetDefaultAudioEndpoint(eRender, eMultimedia).ok().and_then(|d| id_of(&d));
        let all = en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        let mut out = Vec::new();
        for i in 0..all.GetCount()? {
            // a device that vanishes mid-scan is left out, the next scan settles it
            if let Ok(d) = all.Item(i).and_then(|d| open(&d, &default)) {
                out.push(d);
            }
        }
        Ok(out)
    }
}

fn read(devs: &[Dev]) -> Vec<Device> {
    let peak = |m: &Option<IAudioMeterInformation>| m.as_ref().and_then(|m| unsafe { m.GetPeakValue().ok() }).unwrap_or(0.0);
    devs.iter()
        .map(|d| unsafe {
            let raw: Vec<Raw> = d
                .sessions
                .iter()
                .map(|s| Raw {
                    pid: s.pid,
                    path: s.path.clone(),
                    volume: s.vol.GetMasterVolume().unwrap_or(0.0),
                    muted: s.vol.GetMute().map(|b| b.as_bool()).unwrap_or(false),
                    peak: peak(&s.meter),
                })
                .collect();
            Device {
                id: d.id.clone(),
                name: d.name.clone(),
                is_default: d.is_default,
                volume: d.vol.GetMasterVolumeLevelScalar().unwrap_or(0.0),
                muted: d.vol.GetMute().map(|b| b.as_bool()).unwrap_or(false),
                peak: peak(&d.meter),
                apps: merge(&raw),
            }
        })
        .collect()
}

/// Calls slot `index` of a COM object's vtable. Safety: `F` must be that slot's real signature.
unsafe fn slot<F: Copy>(obj: *mut c_void, index: usize) -> F {
    unsafe { std::mem::transmute_copy(&*(*(obj as *const *const *const c_void)).add(index)) }
}

/// Makes `id` the default output for every role. Undocumented `IPolicyConfig` (the interface the Windows
/// sound settings use, unchanged since Windows 7): `SetDefaultEndpoint` is method 10 after IUnknown's 3.
fn set_default(id: &str) -> windows::core::Result<()> {
    const CLSID: GUID = GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);
    const IID: GUID = GUID::from_u128(0xf8679f50_850a_41cf_9c72_430f290290c8);
    type SetDefaultEndpoint = unsafe extern "system" fn(*mut c_void, PCWSTR, i32) -> HRESULT;
    unsafe {
        let unk: IUnknown = CoCreateInstance(&CLSID, None, CLSCTX_ALL)?;
        let mut raw = std::ptr::null_mut();
        unk.query(&IID, &mut raw).ok()?;
        let owner = IUnknown::from_raw(raw); // releases it on drop
        let wide = HSTRING::from(id);
        let f: SetDefaultEndpoint = slot(owner.as_raw(), 13);
        let mut result = Ok(());
        for role in 0..3 {
            // eConsole, eMultimedia, eCommunications
            if let Err(e) = f(owner.as_raw(), PCWSTR(wide.as_ptr()), role).ok() {
                result = Err(e);
            }
        }
        result
    }
}

/// Sends one process's sound to device `id` (empty = back to the default device). Undocumented WinRT
/// factory `Windows.Media.Internal.AudioPolicyConfig`, as used by EarTrumpet: after IInspectable's 6 slots
/// come 19 unrelated methods, then `SetPersistedDefaultAudioEndpoint`. Two interface IDs exist: Windows 10
/// 21H2 and later (Windows 11), and older Windows 10.
fn move_app(pid: u32, id: &str) -> windows::core::Result<()> {
    const NEW: GUID = GUID::from_u128(0xab3d4648_e242_459f_b02f_541c70306324);
    const OLD: GUID = GUID::from_u128(0x2a59116d_6c4f_45e0_a74f_707e3fef9258);
    type SetPersisted = unsafe extern "system" fn(*mut c_void, u32, i32, i32, *mut c_void) -> HRESULT;
    unsafe {
        let factory: IInspectable = RoGetActivationFactory(&HSTRING::from("Windows.Media.Internal.AudioPolicyConfig"))?;
        let mut raw = std::ptr::null_mut();
        if factory.query(&NEW, &mut raw).is_err() {
            factory.query(&OLD, &mut raw).ok()?;
        }
        let owner = IUnknown::from_raw(raw);
        let target = if id.is_empty() { HSTRING::new() } else { HSTRING::from(format!(r"\\?\SWD#MMDEVAPI#{id}#{{e6327cad-dcec-4949-ae8a-991e976a79d2}}")) };
        let f: SetPersisted = slot(owner.as_raw(), 6 + 19);
        let mut result = Ok(());
        for role in 0..2 {
            // eConsole, eMultimedia; flow 0 = eRender
            if let Err(e) = f(owner.as_raw(), pid, 0, role, std::mem::transmute_copy(&target)).ok() {
                result = Err(e);
            }
        }
        result
    }
}

fn apply(cmd: &Cmd, devs: &[Dev]) -> Result<(), String> {
    let dev = |id: &str| devs.iter().find(|d| d.id == id).ok_or_else(|| "That device is gone.".to_string());
    let text = |what: &str, r: windows::core::Result<()>| r.map_err(|e| format!("{what}: {}", e.message()));
    let null = std::ptr::null();
    unsafe {
        match cmd {
            Cmd::DeviceVolume { device, volume } => text("Volume", dev(device)?.vol.SetMasterVolumeLevelScalar(volume.clamp(0.0, 1.0), null)),
            Cmd::DeviceMute { device, muted } => text("Mute", dev(device)?.vol.SetMute(*muted, null)),
            Cmd::AppVolume { device, pids, volume } => {
                for s in dev(device)?.sessions.iter().filter(|s| pids.contains(&s.pid)) {
                    text("Volume", s.vol.SetMasterVolume(volume.clamp(0.0, 1.0), null))?;
                }
                Ok(())
            }
            Cmd::AppMute { device, pids, muted } => {
                for s in dev(device)?.sessions.iter().filter(|s| pids.contains(&s.pid)) {
                    text("Mute", s.vol.SetMute(*muted, null))?;
                }
                Ok(())
            }
            Cmd::SetDefault { device } => text("Windows refused to change the default device", set_default(device)),
            Cmd::Move { pids, device } => pids.iter().try_for_each(|p| text("Windows refused to move the app", move_app(*p, device))),
        }
    }
}

/// The running mixer. Dropping it ends the worker.
pub struct Mixer {
    tx: Sender<Cmd>,
    stop: Arc<AtomicBool>,
    seen: Arc<Mutex<Instant>>,
}

impl Mixer {
    /// `wake` is called after every snapshot (ask the UI to repaint).
    pub fn start(out: Sender<Msg>, wake: impl Fn() + Send + 'static) -> Mixer {
        let (tx, rx) = mpsc::channel::<Cmd>();
        let stop = Arc::new(AtomicBool::new(false));
        let seen = Arc::new(Mutex::new(Instant::now()));
        let (stop2, seen2) = (stop.clone(), seen.clone());
        std::thread::spawn(move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            let mut devs: Vec<Dev> = Vec::new();
            let mut scanned: Option<Instant> = None;
            while !stop2.load(Ordering::Relaxed) {
                let cmds: Vec<Cmd> = rx.try_iter().collect();
                // nobody is looking: hold no audio objects and make no calls
                if cmds.is_empty() && seen2.lock().map(|t| t.elapsed() > Duration::from_millis(500)).unwrap_or(true) {
                    devs.clear();
                    scanned = None;
                    std::thread::sleep(Duration::from_millis(250));
                    continue;
                }
                for c in &cmds {
                    if let Err(e) = apply(c, &devs) {
                        let _ = out.send(Msg::Error(e));
                    }
                    if matches!(c, Cmd::SetDefault { .. } | Cmd::Move { .. }) {
                        scanned = None;
                    }
                }
                if scanned.map_or(true, |t| t.elapsed() > Duration::from_secs(1)) {
                    match scan() {
                        Ok(d) => devs = d,
                        Err(e) => {
                            let _ = out.send(Msg::Error(format!("Could not read the sound devices: {}", e.message())));
                        }
                    }
                    scanned = Some(Instant::now());
                }
                if out.send(Msg::Snapshot(read(&devs))).is_err() {
                    break;
                }
                wake();
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        Mixer { tx, stop, seen }
    }

    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }

    /// Call every frame the mixer is drawn; without it the worker goes idle after half a second.
    pub fn poke(&self) {
        if let Ok(mut t) = self.seen.lock() {
            *t = Instant::now();
        }
    }
}

impl Drop for Mixer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(pid: u32, path: &str, volume: f32, muted: bool, peak: f32) -> Raw {
        Raw { pid, path: path.into(), volume, muted, peak }
    }

    #[test]
    fn sessions_of_one_exe_merge() {
        let rows = merge(&[
            raw(10, r"C:\b\chrome.exe", 0.4, true, 0.1),
            raw(0, "", 0.2, false, 0.0),
            raw(11, r"C:\B\Chrome.exe", 0.9, false, 0.5),
            raw(10, r"C:\b\chrome.exe", 0.3, true, 0.7),
            raw(5, r"C:\a\game.exe", 1.0, true, 0.0),
            raw(7, "", 0.5, false, 0.0),
        ]);
        assert_eq!(rows.len(), 4);
        assert!(rows[0].system, "system sounds first");
        assert_eq!(rows[1].pids, vec![7], "unknown path stays its own row");
        assert_eq!(rows[2].path, r"C:\a\game.exe");
        assert!(rows[2].muted);
        let chrome = &rows[3];
        assert_eq!(chrome.pids, vec![10, 11]);
        assert_eq!((chrome.volume, chrome.peak), (0.9, 0.7));
        assert!(!chrome.muted, "muted only when every session is");
    }

    /// Reads this PC's real devices; changes nothing. `cargo test -p mytune-core --lib audio::tests::real -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn real() {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let devs = read(&scan().expect("scan"));
        assert!(!devs.is_empty());
        for a in devs.iter().flat_map(|d| &d.apps).filter(|a| !a.system) {
            println!("packaged {:?}: {:?}", a.path, packaged(a.pids[0]).map(|l| (l.name, l.icon.map(|i| (i.0, i.1)))));
        }
        for d in devs {
            println!("{} default={} vol={:.2} muted={} apps={:?}", d.name, d.is_default, d.volume, d.muted, d.apps.iter().map(|a| (&a.path, a.volume)).collect::<Vec<_>>());
        }
    }
}
