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

fn main() -> eframe::Result {
    // Go Live and the share picker's screen pictures run in a copy of this exe; it opens no window.
    mytune_chat::run_stream_worker();
    let hidden = std::env::args().any(|a| a == "--minimized");
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1120.0, 720.0])
            .with_min_inner_size([800.0, 520.0])
            .with_decorations(false)
            .with_visible(!hidden)
            .with_title("MyTune").with_icon(std::sync::Arc::new(window_icon())),
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
