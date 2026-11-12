//! The Friends screen (env SCREEN=requests for Pending) with the saved login: writes friends.png. Nothing is clicked.
use eframe::egui::{self, vec2};
use egui_kittest::Harness;
use mytune_ui::{app::App, nav::Page};

#[test]
fn friends_screen() {
    std::env::set_var("MYTUNE_CHAT_SCREEN", std::env::var("SCREEN").unwrap_or_else(|_| "friends".into()));
    let dir = std::env::temp_dir().join("mytune-ui-shot-friends");
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
    h.run_steps(30);
    let out = std::env::var("SHOT_DIR").unwrap_or_else(|_| ".".into());
    h.render().expect("render").save(format!("{out}/friends.png")).unwrap();
}
