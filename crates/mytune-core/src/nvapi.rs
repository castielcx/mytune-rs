//! Minimal NVAPI driver-profile (DRS) access: `nvapi64.dll` is loaded at run time and every function is
//! fetched with `nvapi_QueryInterface(id)`, so there is no link-time dependency on NVIDIA files.
//!
//! Function ids were checked against the `nvapi-sys` 0.1.3 id table (reference only, not a dependency).
//! The struct layouts below follow NVIDIA's public `nvapi.h` (DRS). Every struct carries its own size in
//! its version field, so a wrong layout makes the driver answer `INCOMPATIBLE_STRUCT_VERSION` instead of
//! writing past our buffers; `layout_sizes` in the tests pins the sizes.
//!
//! One session at a time (a global lock, like the C# `Nvidia.Gate`). Changes only reach the driver on `save()`.

use crate::error::{Error, Result};
use std::ffi::c_void;
use std::sync::{Mutex, MutexGuard, OnceLock};
use windows::core::{s, w};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

pub const OK: i32 = 0;
pub const NO_IMPLEMENTATION: i32 = -3;
pub const INCOMPATIBLE_STRUCT_VERSION: i32 = -9;
pub const NOT_SUPPORTED: i32 = -104;
pub const SETTING_NOT_FOUND: i32 = -160;
pub const PROFILE_NOT_FOUND: i32 = -163;
pub const EXECUTABLE_NOT_FOUND: i32 = -166;

const USTR: usize = 2048;
type UStr = [u16; USTR];

fn ustr(s: &str) -> UStr {
    let mut a = [0u16; USTR];
    for (i, c) in s.encode_utf16().take(USTR - 1).enumerate() {
        a[i] = c;
    }
    a
}

fn ustr_to_string(a: &[u16]) -> String {
    String::from_utf16_lossy(&a[..a.iter().position(|&c| c == 0).unwrap_or(a.len())])
}

// ---------------------------------------------------------------- structs (nvapi.h, DRS)

const SETTING_TYPE_DWORD: u32 = 0;

#[repr(C)]
struct NvSetting {
    version: u32,
    name: UStr,
    id: u32,
    ty: u32,
    location: u32,
    is_current_predefined: u32,
    is_predefined_valid: u32,
    predefined: [u8; 4100], // union { u32; binary { u32 len; u8[4096] }; wchar[2048] }
    current: [u8; 4100],
}

#[repr(C)]
struct NvProfile {
    version: u32,
    name: UStr,
    gpu_support: u32,
    is_predefined: u32,
    num_apps: u32,
    num_settings: u32,
}

/// NVDRS_APPLICATION_V4 including `commandLine` (the driver also accepts the version without it, see `APP_VERSIONS`).
#[repr(C)]
struct NvApplication {
    version: u32,
    is_predefined: u32,
    app_name: UStr,
    friendly_name: UStr,
    launcher: UStr,
    file_in_folder: UStr,
    flags: u32,
    command_line: UStr,
}

pub(crate) const fn ver(size: usize, v: u32) -> u32 {
    size as u32 | (v << 16)
}

const SETTING_VER: u32 = ver(std::mem::size_of::<NvSetting>(), 1);
const PROFILE_VER: u32 = ver(std::mem::size_of::<NvProfile>(), 1);
/// Sizes of V4 with and without `commandLine`; both are tried because older drivers want the short one.
const APP_VERSIONS: [u32; 2] = [ver(std::mem::size_of::<NvApplication>(), 4), ver(std::mem::size_of::<NvApplication>() - 4096, 4)];

// ---------------------------------------------------------------- function table

pub(crate) type H = *mut c_void;
type FnInit = unsafe extern "C" fn() -> i32;
type FnCreateSession = unsafe extern "C" fn(*mut H) -> i32;
type FnSession = unsafe extern "C" fn(H) -> i32;
type FnGetBase = unsafe extern "C" fn(H, *mut H) -> i32;
type FnFindProfile = unsafe extern "C" fn(H, *const u16, *mut H) -> i32;
type FnCreateProfile = unsafe extern "C" fn(H, *mut NvProfile, *mut H) -> i32;
type FnProfile = unsafe extern "C" fn(H, H) -> i32;
type FnGetProfileInfo = unsafe extern "C" fn(H, H, *mut NvProfile) -> i32;
type FnCreateApp = unsafe extern "C" fn(H, H, *mut NvApplication) -> i32;
type FnFindApp = unsafe extern "C" fn(H, *const u16, *mut H, *mut NvApplication) -> i32;
type FnGetSetting = unsafe extern "C" fn(H, H, u32, *mut NvSetting) -> i32;
type FnSetSetting = unsafe extern "C" fn(H, H, *mut NvSetting) -> i32;
type FnProfileSetting = unsafe extern "C" fn(H, H, u32) -> i32;

fn query_interface() -> Result<unsafe extern "C" fn(u32) -> *const c_void> {
    static QI: OnceLock<Option<usize>> = OnceLock::new();
    let p = *QI.get_or_init(|| unsafe {
        let lib = LoadLibraryW(w!("nvapi64.dll")).ok()?;
        GetProcAddress(lib, s!("nvapi_QueryInterface")).map(|f| f as usize)
    });
    match p {
        Some(p) => Ok(unsafe { std::mem::transmute::<usize, unsafe extern "C" fn(u32) -> *const c_void>(p) }),
        None => Err(Error::Nvapi { func: "LoadLibrary nvapi64.dll", status: NO_IMPLEMENTATION }),
    }
}

pub(crate) fn func<T: Copy>(id: u32, name: &'static str) -> Result<T> {
    let p = unsafe { query_interface()?(id) };
    if p.is_null() {
        return Err(Error::Nvapi { func: name, status: NO_IMPLEMENTATION });
    }
    assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*const c_void>());
    Ok(unsafe { std::mem::transmute_copy::<*const c_void, T>(&p) })
}

pub(crate) fn chk(func: &'static str, status: i32) -> Result<()> {
    if status == OK {
        Ok(())
    } else {
        Err(Error::Nvapi { func, status })
    }
}

/// `NvAPI_Initialize`, once. False when the driver files are missing or init fails.
pub fn available() -> bool {
    static INIT: OnceLock<bool> = OnceLock::new();
    *INIT.get_or_init(|| {
        crate::process::system32("nvapi64.dll").exists()
            && func::<FnInit>(0x0150E828, "NvAPI_Initialize").and_then(|f| chk("NvAPI_Initialize", unsafe { f() })).is_ok()
    })
}

/// Every setting id this driver knows, with its name; for finding ids (`examples/nvapi_readonly`).
pub fn known_settings() -> Result<Vec<(u32, String)>> {
    type FnIds = unsafe extern "C" fn(*mut u32, *mut u32) -> i32;
    type FnName = unsafe extern "C" fn(u32, *mut UStr) -> i32;
    let mut ids = vec![0u32; 4096];
    let mut n = ids.len() as u32;
    chk("DRS_EnumAvailableSettingIds", unsafe { func::<FnIds>(0xF020614A, "DRS_EnumAvailableSettingIds")?(ids.as_mut_ptr(), &mut n) })?;
    let name = func::<FnName>(0xD61CBE6E, "DRS_GetSettingNameFromId")?;
    Ok(ids[..n as usize]
        .iter()
        .map(|&id| {
            let mut s: Box<UStr> = Box::new([0; USTR]);
            unsafe { name(id, &mut *s) };
            (id, ustr_to_string(&s[..]))
        })
        .collect())
}

// ---------------------------------------------------------------- session

static GATE: Mutex<()> = Mutex::new(());

/// A loaded DRS session. Dropping it destroys the session; unsaved changes are discarded.
pub struct Session {
    h: H,
    _gate: MutexGuard<'static, ()>,
}

#[derive(Clone, Copy)]
pub struct Profile(H);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingValue {
    pub value: u32,
    /// 0 = this profile, 1 = global, 2 = base, 3 = driver default
    pub location: u32,
    pub is_current_predefined: bool,
}

impl SettingValue {
    pub const LOCATION_CURRENT_PROFILE: u32 = 0;
}

impl Session {
    pub fn open() -> Result<Session> {
        if !available() {
            return Err(Error::Nvapi { func: "NvAPI_Initialize", status: NO_IMPLEMENTATION });
        }
        let gate = GATE.lock().unwrap_or_else(|p| p.into_inner());
        let mut h: H = std::ptr::null_mut();
        chk("DRS_CreateSession", unsafe { func::<FnCreateSession>(0x0694D52E, "DRS_CreateSession")?(&mut h) })?;
        let s = Session { h, _gate: gate }; // from here Drop destroys the session
        chk("DRS_LoadSettings", unsafe { func::<FnSession>(0x375DBD6B, "DRS_LoadSettings")?(s.h) })?;
        Ok(s)
    }

    pub fn save(&self) -> Result<()> {
        chk("DRS_SaveSettings", unsafe { func::<FnSession>(0xFCBC7E14, "DRS_SaveSettings")?(self.h) })
    }

    pub fn base_profile(&self) -> Result<Profile> {
        let mut p: H = std::ptr::null_mut();
        chk("DRS_GetBaseProfile", unsafe { func::<FnGetBase>(0xDA8466A0, "DRS_GetBaseProfile")?(self.h, &mut p) })?;
        Ok(Profile(p))
    }

    pub fn find_profile(&self, name: &str) -> Result<Option<Profile>> {
        let n = ustr(name);
        let mut p: H = std::ptr::null_mut();
        match unsafe { func::<FnFindProfile>(0x7E4A9A0B, "DRS_FindProfileByName")?(self.h, n.as_ptr(), &mut p) } {
            OK => Ok(Some(Profile(p))),
            PROFILE_NOT_FOUND => Ok(None),
            s => Err(Error::Nvapi { func: "DRS_FindProfileByName", status: s }),
        }
    }

    pub fn profile_name(&self, p: Profile) -> Result<String> {
        let mut info: NvProfile = unsafe { std::mem::zeroed() };
        info.version = PROFILE_VER;
        chk("DRS_GetProfileInfo", unsafe { func::<FnGetProfileInfo>(0x61CD6FD6, "DRS_GetProfileInfo")?(self.h, p.0, &mut info) })?;
        Ok(ustr_to_string(&info.name))
    }

    pub fn create_profile(&self, name: &str) -> Result<Profile> {
        let mut info: NvProfile = unsafe { std::mem::zeroed() };
        info.version = PROFILE_VER;
        info.name = ustr(name);
        info.gpu_support = 0b111; // GeForce | Quadro | NVS
        let mut p: H = std::ptr::null_mut();
        chk("DRS_CreateProfile", unsafe { func::<FnCreateProfile>(0xCC176068, "DRS_CreateProfile")?(self.h, &mut info, &mut p) })?;
        Ok(Profile(p))
    }

    pub fn delete_profile(&self, p: Profile) -> Result<()> {
        chk("DRS_DeleteProfile", unsafe { func::<FnProfile>(0x17093206, "DRS_DeleteProfile")?(self.h, p.0) })
    }

    pub fn create_application(&self, p: Profile, exe: &str) -> Result<()> {
        let f = func::<FnCreateApp>(0x4347A9DE, "DRS_CreateApplication")?;
        let mut last = OK;
        for v in APP_VERSIONS {
            let mut app: Box<NvApplication> = Box::new(unsafe { std::mem::zeroed() });
            app.version = v;
            app.app_name = ustr(exe);
            last = unsafe { f(self.h, p.0, &mut *app) };
            if last != INCOMPATIBLE_STRUCT_VERSION {
                break;
            }
        }
        chk("DRS_CreateApplication", last)
    }

    /// Profile that holds an application entry for this exe name; `None` if there is none.
    pub fn find_application(&self, exe: &str) -> Result<Option<Profile>> {
        let f = func::<FnFindApp>(0xEEE566B2, "DRS_FindApplicationByName")?;
        let n = ustr(exe);
        let mut last = OK;
        for v in APP_VERSIONS {
            let mut app: Box<NvApplication> = Box::new(unsafe { std::mem::zeroed() });
            app.version = v;
            let mut p: H = std::ptr::null_mut();
            last = unsafe { f(self.h, n.as_ptr(), &mut p, &mut *app) };
            match last {
                OK => return Ok(Some(Profile(p))),
                EXECUTABLE_NOT_FOUND | PROFILE_NOT_FOUND => return Ok(None),
                INCOMPATIBLE_STRUCT_VERSION => continue,
                s => return Err(Error::Nvapi { func: "DRS_FindApplicationByName", status: s }),
            }
        }
        Err(Error::Nvapi { func: "DRS_FindApplicationByName", status: last })
    }

    /// `Ok(None)` if the profile has no such setting at all.
    pub fn get(&self, p: Profile, id: u32) -> Result<Option<SettingValue>> {
        let mut s: Box<NvSetting> = Box::new(unsafe { std::mem::zeroed() });
        s.version = SETTING_VER;
        match unsafe { func::<FnGetSetting>(0x73BF8338, "DRS_GetSetting")?(self.h, p.0, id, &mut *s) } {
            OK => Ok(Some(SettingValue {
                value: u32::from_le_bytes(s.current[..4].try_into().unwrap()),
                location: s.location,
                is_current_predefined: s.is_current_predefined != 0,
            })),
            SETTING_NOT_FOUND => Ok(None),
            st => Err(Error::Nvapi { func: "DRS_GetSetting", status: st }),
        }
    }

    /// Every setting stored in the profile as (id, value); for finding ids (`examples/nvapi_readonly`).
    pub fn settings(&self, p: Profile) -> Result<Vec<(u32, u32)>> {
        type FnEnum = unsafe extern "C" fn(H, H, u32, *mut u32, *mut NvSetting) -> i32;
        let f = func::<FnEnum>(0xAE3039DA, "DRS_EnumSettings")?;
        let mut all: Vec<NvSetting> = (0..512).map(|_| unsafe { std::mem::zeroed() }).collect();
        all.iter_mut().for_each(|s| s.version = SETTING_VER);
        let mut n = all.len() as u32;
        chk("DRS_EnumSettings", unsafe { f(self.h, p.0, 0, &mut n, all.as_mut_ptr()) })?;
        Ok(all[..n as usize].iter().map(|s| (s.id, u32::from_le_bytes(s.current[..4].try_into().unwrap()))).collect())
    }

    pub fn set(&self, p: Profile, id: u32, value: u32) -> Result<()> {
        let mut s: Box<NvSetting> = Box::new(unsafe { std::mem::zeroed() });
        s.version = SETTING_VER;
        s.id = id;
        s.ty = SETTING_TYPE_DWORD;
        s.current[..4].copy_from_slice(&value.to_le_bytes());
        chk("DRS_SetSetting", unsafe { func::<FnSetSetting>(0x577DD202, "DRS_SetSetting")?(self.h, p.0, &mut *s) })
    }

    pub fn restore_default(&self, p: Profile, id: u32) -> Result<()> {
        chk("DRS_RestoreProfileDefaultSetting", unsafe { func::<FnProfileSetting>(0x53F0381E, "DRS_RestoreProfileDefaultSetting")?(self.h, p.0, id) })
    }

    pub fn delete_setting(&self, p: Profile, id: u32) -> Result<()> {
        chk("DRS_DeleteProfileSetting", unsafe { func::<FnProfileSetting>(0xE4A26362, "DRS_DeleteProfileSetting")?(self.h, p.0, id) })
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Ok(f) = func::<FnSession>(0xDAD9CFF8, "DRS_DestroySession") {
            unsafe {
                f(self.h);
            }
        }
    }
}

impl Error {
    pub fn nvapi_status(&self) -> Option<i32> {
        match self {
            Error::Nvapi { status, .. } => Some(*status),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    #[test]
    fn layout_sizes() {
        // nvapi.h: 4 + 4096 + 5*4 + 2*4100
        assert_eq!(size_of::<NvSetting>(), 12320);
        assert_eq!(offset_of!(NvSetting, id), 4100);
        assert_eq!(offset_of!(NvSetting, predefined), 4120);
        assert_eq!(offset_of!(NvSetting, current), 8220);
        assert_eq!(size_of::<NvProfile>(), 4 + 4096 + 16);
        assert_eq!(offset_of!(NvApplication, flags), 4 + 4 + 4 * 4096);
        assert_eq!(SETTING_VER, 12320 | (1 << 16));
        assert_eq!(PROFILE_VER, 4116 | (1 << 16));
        assert_eq!(APP_VERSIONS[1] & 0xFFFF, 16396);
        assert_eq!(APP_VERSIONS[0] & 0xFFFF, 20492);
        assert_eq!(APP_VERSIONS[0] >> 16, 4);
    }

    #[test]
    fn ustr_roundtrip_and_truncation() {
        assert_eq!(ustr_to_string(&ustr("MyTune - Game é")), "MyTune - Game é");
        let long = "x".repeat(5000);
        let a = ustr(&long);
        assert_eq!(a[USTR - 1], 0, "always null terminated");
        assert_eq!(ustr_to_string(&a).len(), USTR - 1);
    }
}
