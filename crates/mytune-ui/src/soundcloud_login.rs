//! Sign in to SoundCloud inside MyTune: a small WebView2 window on SoundCloud's own sign-in page. When the
//! `oauth_token` cookie appears (the user signed in, or was already), it is saved to the credential store and
//! the window closes. Nothing is typed for the user and no password is read; SoundCloud's own page handles it.
//!
//! The webview runs on its own thread with its own Win32 window and message loop (WebView2 needs a
//! single-threaded apartment with a running loop), apart from the egui window.

use eframe::egui;
use raw_window_handle::{HasWindowHandle, HandleError, RawWindowHandle, Win32WindowHandle, WindowHandle};
use std::num::NonZeroIsize;
use std::ptr;
use std::sync::Mutex;
use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use wry::{WebContext, WebViewBuilder};

/// The last sign-in result for the page to show: `None` while none finished, `Some(Ok)` once a token was
/// saved, `Some(Err)` with why it did not.
static RESULT: Mutex<Option<Result<(), String>>> = Mutex::new(None);
/// So only one window opens at a time.
static OPEN: Mutex<bool> = Mutex::new(false);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Takes the finished result, if any (clears it).
pub fn take_result() -> Option<Result<(), String>> {
    lock(&RESULT).take()
}

pub fn is_open() -> bool {
    *lock(&OPEN)
}

/// Opens the sign-in window on its own thread; returns at once. The page reads `take_result` later.
pub fn open(ctx: &egui::Context) {
    if std::mem::replace(&mut *lock(&OPEN), true) {
        return;
    }
    let ctx = ctx.clone();
    std::thread::Builder::new()
        .name("soundcloud-login".into())
        .spawn(move || {
            let r = run();
            *lock(&RESULT) = Some(r);
            *lock(&OPEN) = false;
            ctx.request_repaint();
        })
        .ok();
}

/// A bare HWND as a window handle for wry.
struct Win(isize);
impl HasWindowHandle for Win {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let handle = Win32WindowHandle::new(NonZeroIsize::new(self.0).ok_or(HandleError::Unavailable)?);
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(handle)) })
    }
}

const CLASS: &[u16] = &[0x4D, 0x79, 0x54, 0x75, 0x6E, 0x65, 0x53, 0x63, 0x4C, 0x6F, 0x67, 0x69, 0x6E, 0]; // "MyTuneScLogin"

unsafe extern "system" fn wndproc(hwnd: *mut core::ffi::c_void, msg: u32, w: usize, l: isize) -> isize {
    if msg == WM_DESTROY {
        PostQuitMessage(0);
        return 0;
    }
    DefWindowProcW(hwnd, msg, w, l)
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

fn run() -> Result<(), String> {
    unsafe {
        // WebView2 needs the thread in a single-threaded apartment
        let _ = CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let class = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: ptr::null_mut(),
            hIcon: ptr::null_mut(),
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            hbrBackground: ptr::null_mut(), // the webview fills the window
            lpszMenuName: ptr::null(),
            lpszClassName: CLASS.as_ptr(),
        };
        RegisterClassW(&class); // a second open reuses the class; the duplicate error is harmless
        let title = utf16("Sign in to SoundCloud");
        let hwnd = CreateWindowExW(
            0,
            CLASS.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            520,
            720,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null(),
        );
        if hwnd.is_null() {
            return Err("Could not open the sign-in window".into());
        }

        // WebView2 keeps its profile here, so it works after an install into Program Files
        let data = std::path::PathBuf::from(std::env::var("ProgramData").unwrap_or_else(|_| "C:\\ProgramData".into())).join("MyTune").join("webview");
        let mut web_context = WebContext::new(Some(data));
        let win = Win(hwnd as isize);
        let webview = WebViewBuilder::new_with_web_context(&mut web_context)
            .with_url("https://soundcloud.com/signin")
            .build(&win)
            .map_err(|e| format!("Could not start WebView2 (is it installed?): {e}"))?;

        ShowWindow(hwnd, SW_SHOW);
        SetTimer(hwnd, 1, 800, None); // poll the cookie every 0.8 s

        // a stored token only exists here when SoundCloud refused it, so the same cookie must not count
        let stale = mytune_core::soundcloud::load_token();
        let mut captured = None;
        let mut msg: MSG = std::mem::zeroed();
        loop {
            let r = GetMessageW(&mut msg, ptr::null_mut(), 0, 0);
            if r <= 0 {
                break; // WM_QUIT (0) or error (-1)
            }
            if msg.message == WM_TIMER {
                if let Ok(cookies) = webview.cookies_for_url("https://soundcloud.com") {
                    if let Some(token) = cookies.iter().find(|c| c.name() == "oauth_token").map(|c| c.value().to_string()).filter(|v| !v.is_empty() && Some(v) != stale.as_ref()) {
                        captured = Some(token);
                        DestroyWindow(hwnd);
                    }
                }
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        match captured {
            Some(token) => mytune_core::soundcloud::save_token(&token),
            None => Err("Sign-in window was closed before a login was captured".into()),
        }
    }
}
