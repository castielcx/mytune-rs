//! The push-to-talk key, from Yapfast's src/push_to_talk.rs (Windows part): a low-level keyboard hook on a
//! thread of its own sees the key in every app (MyTune runs as admin, so elevated games too) and tells the
//! engine (`push_to_talk::hold`). It also records a new key: the next key pressed while recording.

use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc;
use windows_sys::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetKeyNameTextW;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, PostThreadMessageW, SetWindowsHookExW, UnhookWindowsHookEx, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_EXTENDED, MSG,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
};

const NONE: u32 = u32::MAX;
/// The key held for talking, as a scan code with 0xE000 for extended keys.
static KEY: AtomicU32 = AtomicU32::new(NONE);
static RECORDING: AtomicBool = AtomicBool::new(false);
/// The key recorded last; NONE until one is pressed.
static RECORDED: AtomicU32 = AtomicU32::new(NONE);

/// The hook; removed when dropped.
pub struct Hook {
    thread: u32,
}

impl Hook {
    /// Starts watching keys; None when Windows refuses the hook.
    pub fn start() -> Option<Hook> {
        let (started, thread) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("mytune-talk-key".into())
            .spawn(move || {
                // SAFETY: the hook procedure only reads what Windows hands it; the message loop it needs runs on
                // this thread until WM_QUIT.
                unsafe {
                    let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(proc), GetModuleHandleW(ptr::null()), 0);
                    if hook.is_null() {
                        let _ = started.send(None);
                        return;
                    }
                    let _ = started.send(Some(GetCurrentThreadId()));
                    let mut message: MSG = std::mem::zeroed();
                    while GetMessageW(&mut message, ptr::null_mut(), 0, 0) > 0 {}
                    UnhookWindowsHookEx(hook);
                }
            })
            .ok()?;
        thread.recv().ok().flatten().map(|thread| Hook { thread })
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        // SAFETY: posting to a thread id is harmless once it has ended.
        unsafe { PostThreadMessageW(self.thread, WM_QUIT, 0, 0) };
    }
}

/// The key that talks, or none.
pub fn set_key(scancode: Option<u32>) {
    KEY.store(scancode.unwrap_or(NONE), Ordering::Relaxed);
    yapfast_core::push_to_talk::hold(false);
}

/// Records the next key pressed anywhere; read it with [`recorded`].
pub fn record(on: bool) {
    RECORDED.store(NONE, Ordering::Relaxed);
    RECORDING.store(on, Ordering::Relaxed);
}

pub fn recording() -> bool {
    RECORDING.load(Ordering::Relaxed)
}

/// The key recorded, once one was pressed: its scan code and name.
pub fn recorded() -> Option<(u32, String)> {
    let code = RECORDED.swap(NONE, Ordering::Relaxed);
    (code != NONE).then(|| (code, key_name(code)))
}

/// A key's name as Windows' keyboard layout spells it ("F", "Caps Lock", "Mouse" keys have none).
pub fn key_name(scancode: u32) -> String {
    let lparam = ((scancode & 0xFF) << 16) | if scancode & 0xE000 != 0 { 1 << 24 } else { 0 };
    let mut buf = [0u16; 64];
    // SAFETY: the buffer's length is passed with it.
    let n = unsafe { GetKeyNameTextW(lparam as i32, buf.as_mut_ptr(), buf.len() as i32) };
    if n > 0 {
        String::from_utf16_lossy(&buf[..n as usize])
    } else {
        format!("Key {scancode:#x}")
    }
}

/// Notes the talk key going down or up (or records a key), and passes every key on.
unsafe extern "system" fn proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code == HC_ACTION as i32 {
        // SAFETY: for HC_ACTION, lparam points at the key's details.
        let key = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
        let scancode = key.scanCode | if key.flags & LLKHF_EXTENDED != 0 { 0xE000 } else { 0 };
        let down = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        if down && RECORDING.swap(false, Ordering::Relaxed) {
            RECORDED.store(scancode, Ordering::Relaxed);
        } else if scancode == KEY.load(Ordering::Relaxed) {
            match wparam as u32 {
                WM_KEYDOWN | WM_SYSKEYDOWN => yapfast_core::push_to_talk::hold(true),
                WM_KEYUP | WM_SYSKEYUP => yapfast_core::push_to_talk::hold(false),
                _ => {}
            }
        }
    }
    // SAFETY: the next hook gets the event as it came.
    unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
}
