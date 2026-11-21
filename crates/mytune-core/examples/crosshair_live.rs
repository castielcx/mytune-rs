//! Shows the crosshair overlay on the second display for a few seconds and checks it.
//! `cargo run -p mytune-core --example crosshair_live`, then press F9 (or wait 15 s).
use mytune_core::crosshair::{self, Crosshair};
use std::time::{Duration, Instant};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

fn second_display() -> Option<String> {
    unsafe {
        for i in 0.. {
            let mut d = DISPLAY_DEVICEW { cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32, ..Default::default() };
            if !EnumDisplayDevicesW(PCWSTR::null(), i, &mut d, 0).as_bool() {
                return None;
            }
            let f = d.StateFlags.0;
            if f & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP.0 != 0 && f & DISPLAY_DEVICE_PRIMARY_DEVICE.0 == 0 {
                return Some(String::from_utf16_lossy(&d.DeviceName).trim_end_matches('\0').to_string());
            }
        }
        None
    }
}

fn own_overlay() -> Option<HWND> {
    unsafe {
        let me = std::process::id();
        let mut after = None;
        while let Ok(h) = FindWindowExW(None, after, w!("MyTuneCrosshair"), w!("MyTune Crosshair")) {
            let mut pid = 0;
            GetWindowThreadProcessId(h, Some(&mut pid));
            if pid == me {
                return Some(h);
            }
            after = Some(h);
        }
        None
    }
}

fn main() {
    let Some(mon) = second_display() else {
        println!("NO second display: stopped (never draw on the primary one)");
        return;
    };
    println!("display {mon}");
    let c = Crosshair { on: true, monitor: mon.clone(), hotkey: "F9".into(), ..Default::default() };
    crosshair::start(c.clone(), |c| println!("TOGGLED on={}", c.on));
    std::thread::sleep(Duration::from_millis(800));
    println!("hotkey problem: {:?}", crosshair::problem());
    let Some(h) = own_overlay() else {
        println!("FAIL no overlay window");
        return;
    };
    unsafe {
        let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
        let want = [(WS_EX_LAYERED.0, "layered"), (WS_EX_TRANSPARENT.0, "transparent"), (WS_EX_TOPMOST.0, "topmost"), (WS_EX_NOACTIVATE.0, "noactivate"), (WS_EX_TOOLWINDOW.0, "toolwindow")];
        for (bit, name) in want {
            println!("{name}: {}", ex & bit != 0);
        }
        println!("visible: {}", IsWindowVisible(h).as_bool());
        let mut r = RECT::default();
        let _ = GetWindowRect(h, &mut r);
        let mut dm = DEVMODEW { dmSize: std::mem::size_of::<DEVMODEW>() as u16, ..Default::default() };
        let wide: Vec<u16> = mon.encode_utf16().chain(Some(0)).collect();
        let _ = EnumDisplaySettingsW(PCWSTR(wide.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm);
        let p = dm.Anonymous1.Anonymous2.dmPosition;
        let (cx, cy) = (p.x + dm.dmPelsWidth as i32 / 2, p.y + dm.dmPelsHeight as i32 / 2);
        println!("window {:?} size {}x{}, monitor centre ({cx},{cy}), window centre ({},{})", (r.left, r.top), r.right - r.left, r.bottom - r.top, (r.left + r.right - 1) / 2, (r.top + r.bottom - 1) / 2);
        let under = WindowFromPoint(POINT { x: cx, y: cy });
        println!("click-through (window under centre is not the overlay): {}", under != h);
        let dc = GetDC(None);
        let at = |x: i32, y: i32| {
            let v = GetPixel(dc, x, y).0;
            format!("({},{},{})", v & 0xFF, (v >> 8) & 0xFF, (v >> 16) & 0xFF)
        };
        let gap = c.gap as i32 + 2;
        println!("pixels on: centre {} arm-right {} far {}", at(cx, cy), at(cx + gap + 2, cy), at(cx + 200, cy));
        let start = Instant::now();
        while crosshair::live().on && start.elapsed() < Duration::from_secs(15) {
            std::thread::sleep(Duration::from_millis(100));
        }
        std::thread::sleep(Duration::from_millis(300));
        println!("after F9 wait: on={} window exists={}", crosshair::live().on, own_overlay().is_some());
        println!("pixels off: centre {} arm-right {}", at(cx, cy), at(cx + gap + 2, cy));
        ReleaseDC(None, dc);
    }
    // the running app normally holds F8: switching to it should report the key as taken
    crosshair::set(Crosshair { on: false, hotkey: "F8".into(), ..c });
    std::thread::sleep(Duration::from_millis(500));
    println!("F8 problem: {:?}", crosshair::problem());
}
