//! A persistent, signed-in WebView2 that runs SoundCloud's write API (favourites, playlists) from the
//! browser context. SoundCloud gates writes behind DataDome bot protection: a plain HTTP client gets a 403
//! captcha, but a `fetch()` from a loaded soundcloud.com page carries the browser cookies and fingerprint
//! and passes. Reads stay direct (mytune-core). The webview is hidden, shares the sign-in profile (so it is
//! already logged in), and answers one request at a time over its IPC channel.

use raw_window_handle::{HasWindowHandle, HandleError, RawWindowHandle, Win32WindowHandle, WindowHandle};
use std::collections::HashMap;
use std::num::NonZeroIsize;
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use windows_sys::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows_sys::Win32::UI::WindowsAndMessaging::*;
use wry::{WebContext, WebViewBuilder};

struct Req {
    method: String,
    url: String,
    body: Option<String>,
    token: String,
    reply: Sender<Result<(u16, String), String>>,
}

type Reply = Sender<Result<(u16, String), String>>;

static TX: OnceLock<Mutex<Sender<Req>>> = OnceLock::new();
static NEXT_ID: AtomicU64 = AtomicU64::new(1);
/// Set once soundcloud.com has loaded, so the DataDome cookie is in place before the first write.
static READY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn pending() -> &'static Mutex<HashMap<u64, Reply>> {
    static P: OnceLock<Mutex<HashMap<u64, Reply>>> = OnceLock::new();
    P.get_or_init(Default::default)
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// Run one SoundCloud write through the signed-in webview. Blocks until it answers (or times out).
/// Returns the HTTP status and the response body.
pub fn fetch(method: &str, url: &str, body: Option<String>, token: &str) -> Result<(u16, String), String> {
    request(method, url, body, token)
}

/// Request kinds handled on the webview thread itself instead of by a page fetch().
const READ_COOKIE: &str = "MYTUNE_READ_COOKIE";
const FORGET_COOKIE: &str = "MYTUNE_FORGET_COOKIE";

/// The profile's current `oauth_token` cookie ("" when there is none). Blocks.
pub fn token_cookie() -> Result<String, String> {
    request(READ_COOKIE, "", None, "").map(|(_, v)| v)
}

/// Drops the profile's `oauth_token` cookie, so the next sign-in cannot capture the same one. Blocks.
pub fn forget_token() -> Result<(), String> {
    request(FORGET_COOKIE, "", None, "").map(|_| ())
}

fn request(method: &str, url: &str, body: Option<String>, token: &str) -> Result<(u16, String), String> {
    let (reply, rx) = channel();
    let tx = TX.get_or_init(|| {
        let (tx, rrx) = channel::<Req>();
        std::thread::Builder::new().name("soundcloud-web".into()).spawn(move || run(rrx)).ok();
        Mutex::new(tx)
    });
    lock(tx).send(Req { method: method.into(), url: url.into(), body, token: token.into(), reply }).map_err(|_| "the SoundCloud webview is not running".to_string())?;
    rx.recv_timeout(Duration::from_secs(25)).map_err(|_| "SoundCloud did not answer in time".to_string())?
}

struct Win(isize);
impl HasWindowHandle for Win {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let h = Win32WindowHandle::new(NonZeroIsize::new(self.0).ok_or(HandleError::Unavailable)?);
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(h)) })
    }
}

const CLASS: &[u16] = &[0x4D, 0x79, 0x54, 0x75, 0x6E, 0x65, 0x53, 0x63, 0x57, 0x65, 0x62, 0]; // "MyTuneScWeb"

unsafe extern "system" fn wndproc(hwnd: *mut core::ffi::c_void, msg: u32, w: usize, l: isize) -> isize {
    if msg == WM_DESTROY {
        PostQuitMessage(0);
        return 0;
    }
    DefWindowProcW(hwnd, msg, w, l)
}

/// A JS string literal for safe embedding (quotes, escapes).
fn js_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

fn run(rx: std::sync::mpsc::Receiver<Req>) {
    unsafe {
        let _ = CoInitializeEx(ptr::null(), COINIT_APARTMENTTHREADED as u32);
        let class = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(wndproc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: ptr::null_mut(),
            hIcon: ptr::null_mut(),
            hCursor: ptr::null_mut(),
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: CLASS.as_ptr(),
        };
        RegisterClassW(&class);
        // a real window, but never shown (off screen, no taskbar); WebView2 needs one with a message loop
        let hwnd = CreateWindowExW(WS_EX_TOOLWINDOW, CLASS.as_ptr(), CLASS.as_ptr(), WS_OVERLAPPED, -10000, -10000, 400, 300, ptr::null_mut(), ptr::null_mut(), ptr::null_mut(), ptr::null());
        if hwnd.is_null() {
            return;
        }
        let data = mytune_core::util::program_data().join("MyTune").join("webview");
        let mut web_context = WebContext::new(Some(data));
        let win = Win(hwnd as isize);
        let webview = match WebViewBuilder::new_with_web_context(&mut web_context)
            .with_url("https://soundcloud.com/discover")
            .with_on_page_load_handler(|event, _| {
                if matches!(event, wry::PageLoadEvent::Finished) {
                    READY.store(true, Ordering::Relaxed);
                }
            })
            .with_ipc_handler(|req: wry::http::Request<String>| {
                // the page posts {"id":N,"status":S,"body":"..."}
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(req.body()) {
                    if let Some(id) = v["id"].as_u64() {
                        if let Some(reply) = lock(pending()).remove(&id) {
                            let status = v["status"].as_u64().unwrap_or(0) as u16;
                            let _ = reply.send(Ok((status, v["body"].as_str().unwrap_or("").to_string())));
                        }
                    }
                }
            })
            .build(&win)
        {
            Ok(w) => w,
            Err(_) => return, // every request will time out; the UI shows that
        };
        SetTimer(hwnd, 1, 60, None); // drain the request queue on the webview thread

        let mut msg: MSG = std::mem::zeroed();
        loop {
            let r = GetMessageW(&mut msg, ptr::null_mut(), 0, 0);
            if r <= 0 {
                break;
            }
            if msg.message == WM_TIMER && READY.load(Ordering::Relaxed) {
                while let Ok(req) = rx.try_recv() {
                    if req.method == READ_COOKIE {
                        let r = webview.cookies_for_url("https://soundcloud.com").map(|c| c.iter().find(|c| c.name() == "oauth_token").map(|c| c.value().to_string()).unwrap_or_default());
                        let _ = req.reply.send(r.map(|v| (200, v)).map_err(|e| e.to_string()));
                        continue;
                    }
                    if req.method == FORGET_COOKIE {
                        // wry 0.52 has no cookie delete; the site reads this cookie from JS, so JS can expire it
                        let js = "for(const d of['','; domain=.soundcloud.com','; domain=soundcloud.com'])document.cookie='oauth_token=; Max-Age=0; path=/'+d;";
                        let _ = req.reply.send(webview.evaluate_script(js).map(|_| (200, String::new())).map_err(|e| e.to_string()));
                        continue;
                    }
                    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                    lock(pending()).insert(id, req.reply);
                    // embed the body as a JS object literal (valid JSON is valid JS) and let the page
                    // stringify it, so there is no double-encoding
                    let (body_decl, body_part) = match &req.body {
                        Some(b) => (format!("const __b=JSON.stringify({b});"), "body:__b,"),
                        None => (String::new(), ""),
                    };
                    let js = format!(
                        "(async()=>{{try{{{decl}const res=await fetch({url},{{method:{method},credentials:'include',redirect:'follow',headers:{{'Authorization':'OAuth '+{token},'Content-Type':'application/json'}},{body}}});const txt=await res.text();window.ipc.postMessage(JSON.stringify({{id:{id},status:res.status,body:txt}}));}}catch(e){{window.ipc.postMessage(JSON.stringify({{id:{id},status:0,body:String(e)}}));}}}})();",
                        decl = body_decl,
                        url = js_str(&req.url),
                        method = js_str(&req.method),
                        token = js_str(&req.token),
                        body = body_part,
                        id = id,
                    );
                    if webview.evaluate_script(&js).is_err() {
                        if let Some(reply) = lock(pending()).remove(&id) {
                            let _ = reply.send(Err("the SoundCloud webview could not run the request".into()));
                        }
                    }
                }
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}
