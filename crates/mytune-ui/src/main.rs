#![windows_subsystem = "windows"]
use eframe::egui;
use tray_icon::{menu::{Menu, MenuEvent, MenuItem}, Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

/// The accent saved by the C# app or this one (`Settings.AccentColor`).
fn accent() -> String {
    mytune_core::store::Store::load_default().settings_str("AccentColor").unwrap_or_else(|| "#FF5A0A".into())
}

/// The MyTune "M" in the accent colour (the same icon the C# app draws).
fn icon() -> Icon {
    Icon::from_rgba(mytune_ui::icon::render(&accent(), 32), 32, 32).expect("icon")
}

/// Title bar / taskbar icon of the window.
fn window_icon() -> egui::IconData {
    egui::IconData { rgba: mytune_ui::icon::render(&accent(), 64), width: 64, height: 64 }
}

fn show(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    ctx.request_repaint();
}

/// True when another MyTune already runs: its window is brought to the front and this start should end.
/// The named mutex is held until this process exits.
fn already_running() -> bool {
    use windows_sys::Win32::{Foundation::{GetLastError, ERROR_ALREADY_EXISTS}, System::Threading::CreateMutexW, UI::WindowsAndMessaging::*};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    unsafe {
        CreateMutexW(std::ptr::null(), 0, wide("Local\\MyTuneRs.SingleInstance").as_ptr());
        if GetLastError() != ERROR_ALREADY_EXISTS {
            return false;
        }
        raise();
        true
    }
}

/// Shows MyTune's window (hidden in the tray, minimised or behind others) and puts it in front.
fn raise() {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    unsafe {
        // "Window Class" is winit's class name: an Explorer window of a folder named MyTune must not match
        let w = FindWindowW(wide("Window Class").as_ptr(), wide("MyTune").as_ptr());
        if !w.is_null() {
            ShowWindow(w, if IsIconic(w) != 0 { SW_RESTORE } else { SW_SHOW });
            SetForegroundWindow(w);
        }
    }
}

/// Win+Y from anywhere brings the window up (out of the tray too). The hotkey lives on its own thread:
/// `WM_HOTKEY` goes to the thread that registered it, and winit's loop would drop it.
fn open_hotkey(ctx: egui::Context) {
    use windows_sys::Win32::UI::{Input::KeyboardAndMouse::{RegisterHotKey, MOD_NOREPEAT, MOD_WIN}, WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY}};
    std::thread::spawn(move || unsafe {
        if RegisterHotKey(std::ptr::null_mut(), 1, MOD_WIN | MOD_NOREPEAT, b'Y' as u32) == 0 {
            mytune_core::log::info("Win+Y is taken by another program: the open-MyTune hotkey is off");
            return;
        }
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            if msg.message == WM_HOTKEY {
                show(&ctx);
                raise(); // this thread got the key press, so Windows lets it take the foreground
            }
        }
    });
}

fn main() -> eframe::Result {
    // Go Live and the share picker's screen pictures run in a copy of this exe; it opens no window.
    mytune_chat::run_stream_worker();
    // Setup runs this after it removed the C# MyTune, whose uninstaller deletes the "Start with Windows" task.
    if std::env::args().any(|a| a == "--enable-autostart") {
        mytune_core::autostart::enable();
        return Ok(());
    }
    if already_running() {
        return Ok(());
    }
    let hidden = std::env::args().any(|a| a == "--minimized");
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 720.0])
            .with_min_inner_size([800.0, 520.0])
            .with_decorations(false)
            .with_visible(!hidden)
            .with_title("MyTune").with_icon(std::sync::Arc::new(window_icon())),
        // egui's dither uses the same noise for every shape, so it adds up through stacked see-through layers
        // (2-level grain on the chat page); the backdrop picture is dithered once by `theme::backdrop_image`
        dithering: false,
        ..Default::default()
    };
    eframe::run_native("MyTune", opts, Box::new(move |cc| {
        let menu = Menu::new();
        let open = MenuItem::new("Open MyTune", true, None);
        let exit = MenuItem::new("Exit", true, None);
        let _ = menu.append_items(&[&open, &exit]);
        let (open_id, exit_id) = (open.id().clone(), exit.id().clone());
        let ctx = cc.egui_ctx.clone();
        MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
            if e.id == exit_id {
                std::process::exit(0);
            } else if e.id == open_id {
                show(&ctx);
            }
        }));
        open_hotkey(cc.egui_ctx.clone());
        let ctx = cc.egui_ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
                show(&ctx);
            }
        }));
        let tray = TrayIconBuilder::new().with_tooltip("MyTune").with_icon(icon()).with_menu(Box::new(menu)).with_menu_on_left_click(false).build();
        let mut app = mytune_ui::app::App::new();
        mytune_ui::theme::install_fonts(&cc.egui_ctx);
        mytune_ui::theme::apply_visuals(&cc.egui_ctx);
        app.keep_in_tray = tray.is_ok();
        // The icon must live as long as the app.
        std::mem::forget(tray);
        Ok(Box::new(app))
    }))
}
