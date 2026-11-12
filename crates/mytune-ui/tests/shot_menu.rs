//! Opens a channel by name (env FORUM, default rules) with the saved login, right-clicks its first message and
//! writes menu.png: the message menu must show. Nothing is clicked in the menu, so nothing is sent.
use eframe::egui::{self, pos2, vec2};
use egui_kittest::Harness;
use mytune_ui::{app::App, nav::Page};

#[test]
fn message_menu() {
    std::env::set_var("MYTUNE_CHAT_OPEN", std::env::var("FORUM").unwrap_or_else(|_| "rules".into()));
    let dir = std::env::temp_dir().join("mytune-ui-shot-menu");
    let _ = std::fs::create_dir_all(&dir);
    let mut app = App::with_store(mytune_core::store::Store::load(&dir));
    app.goto(Page::Chat);
    let mut first = true;
    let mut h = Harness::builder().with_size(vec2(1180.0, 900.0)).build_ui(move |ui| {
        if first {
            mytune_ui::theme::install_fonts(ui.ctx());
            mytune_ui::theme::apply_visuals(ui.ctx());
            ui.ctx().set_missing_glyph_policy(egui::MissingGlyphPolicy::Tofu);
            first = false;
        }
        app.ui(ui);
    });
    h.run_steps(20);
    std::thread::sleep(std::time::Duration::from_secs(10));
    h.run_steps(20);
    let at = pos2(640.0, 260.0);
    for e in [
        egui::Event::PointerMoved(at),
        egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() },
        egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() },
    ] {
        h.event(e);
        h.run_steps(2);
    }
    h.run_steps(5);
    let out = std::env::var("SHOT_DIR").unwrap_or_else(|_| ".".into());
    h.render().expect("render").save(format!("{out}/menu.png")).unwrap();
}
