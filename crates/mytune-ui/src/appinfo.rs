//! The application behind a window: its name (the exe's file description) and its icon, for the Go Live picker.
use std::os::windows::ffi::OsStringExt;
use windows_sys::Win32::Foundation::{CloseHandle, HWND};
use windows_sys::Win32::Graphics::Gdi::{
    DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
};
use windows_sys::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW, VerQueryValueW};
use windows_sys::Win32::System::Threading::{OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION};
use windows_sys::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
use windows_sys::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, GetWindowThreadProcessId, ICONINFO};

pub struct App {
    pub name: String,
    pub icon: Option<eframe::egui::ColorImage>,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn exe_of(hwnd: isize) -> Option<String> {
    unsafe {
        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd as HWND, &mut pid);
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h.is_null() {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut n);
        CloseHandle(h);
        (ok != 0).then(|| std::ffi::OsString::from_wide(&buf[..n as usize]).to_string_lossy().into_owned())
    }
}

/// "FileDescription" of the exe, else None.
fn description(path: &str) -> Option<String> {
    unsafe {
        let p = wide(path);
        let size = GetFileVersionInfoSizeW(p.as_ptr(), std::ptr::null_mut());
        if size == 0 {
            return None;
        }
        let mut data = vec![0u8; size as usize];
        if GetFileVersionInfoW(p.as_ptr(), 0, size, data.as_mut_ptr().cast()) == 0 {
            return None;
        }
        let mut tp: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut tl = 0u32;
        let key = wide("\\VarFileInfo\\Translation");
        let (lang, page) = if VerQueryValueW(data.as_ptr().cast(), key.as_ptr(), &mut tp, &mut tl) != 0 && tl >= 4 {
            let t = std::slice::from_raw_parts(tp as *const u16, 2);
            (t[0], t[1])
        } else {
            (0x0409, 0x04B0)
        };
        let key = wide(&format!("\\StringFileInfo\\{lang:04x}{page:04x}\\FileDescription"));
        let mut sp: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut sl = 0u32;
        if VerQueryValueW(data.as_ptr().cast(), key.as_ptr(), &mut sp, &mut sl) == 0 || sl < 2 {
            return None;
        }
        let s = std::slice::from_raw_parts(sp as *const u16, sl as usize - 1);
        let s = String::from_utf16_lossy(s);
        let s = s.trim();
        (!s.is_empty()).then(|| s.to_owned())
    }
}

/// The exe's large icon as RGBA.
fn icon(path: &str) -> Option<eframe::egui::ColorImage> {
    unsafe {
        let p = wide(path);
        let mut fi: SHFILEINFOW = std::mem::zeroed();
        SHGetFileInfoW(p.as_ptr(), 0, &mut fi, std::mem::size_of::<SHFILEINFOW>() as u32, SHGFI_ICON | SHGFI_LARGEICON);
        if fi.hIcon.is_null() {
            return None;
        }
        let mut ii: ICONINFO = std::mem::zeroed();
        let mut out = None;
        if GetIconInfo(fi.hIcon, &mut ii) != 0 {
            let mut bm: BITMAP = std::mem::zeroed();
            if !ii.hbmColor.is_null() && GetObjectW(ii.hbmColor, std::mem::size_of::<BITMAP>() as i32, (&mut bm as *mut BITMAP).cast()) != 0 {
                let (w, h) = (bm.bmWidth, bm.bmHeight);
                let mut bi: BITMAPINFO = std::mem::zeroed();
                bi.bmiHeader = BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB,
                    ..std::mem::zeroed()
                };
                let mut px = vec![0u8; (w * h * 4) as usize];
                let dc = GetDC(std::ptr::null_mut());
                let got = GetDIBits(dc, ii.hbmColor, 0, h as u32, px.as_mut_ptr().cast(), &mut bi, DIB_RGB_COLORS);
                ReleaseDC(std::ptr::null_mut(), dc);
                if got != 0 {
                    // BGRA -> RGBA; icons without an alpha channel come back fully transparent
                    let opaque = px.chunks(4).all(|c| c[3] == 0);
                    for c in px.chunks_mut(4) {
                        c.swap(0, 2);
                        if opaque {
                            c[3] = 255;
                        }
                    }
                    out = Some(eframe::egui::ColorImage::from_rgba_unmultiplied([w as usize, h as usize], &px));
                }
            }
            DeleteObject(ii.hbmColor);
            DeleteObject(ii.hbmMask);
        }
        DestroyIcon(fi.hIcon);
        out
    }
}

/// Name (file description, else the file name) and icon of the program at `path`.
pub fn of_path(path: &str) -> App {
    let stem = std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    App { name: description(path).unwrap_or(stem), icon: icon(path) }
}

/// Name and icon of the program that owns window `hwnd`; None when it cannot be told (use the window title).
pub fn of_window(hwnd: u64) -> Option<App> {
    let path = exe_of(hwnd as isize)?;
    let stem = std::path::Path::new(&path).file_stem()?.to_string_lossy().into_owned();
    // UWP windows all belong to this host: its name would be wrong
    if stem.eq_ignore_ascii_case("ApplicationFrameHost") {
        return None;
    }
    Some(App { name: description(&path).unwrap_or(stem), icon: icon(&path) })
}
