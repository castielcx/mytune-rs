//! Opens a Discord forum channel by name (env FORUM, default boosting-shop) with the saved login and writes forum.png.
use eframe::egui::vec2;
use egui_kittest::Harness;
use mytune_ui::{app::App, nav::Page};

#[test]
fn forum_page() {
    std::env::set_var("MYTUNE_CHAT_OPEN", std::env::var("FORUM").unwrap_or_else(|_| "boosting-shop".into()));
    let dir = std::env::temp_dir().join("mytune-ui-shot-forum");
    let _ = std::fs::create_dir_all(&dir);
    let mut app = App::with_store(mytune_core::store::Store::load(&dir));
    app.goto(Page::Chat);
    let mut first = true;
    let mut h = Harness::builder().with_size(vec2(1180.0, 900.0)).build_ui(move |ui| {
        if first {
            mytune_ui::theme::install_fonts(ui.ctx());
            mytune_ui::theme::apply_visuals(ui.ctx());
            ui.ctx().set_missing_glyph_policy(eframe::egui::MissingGlyphPolicy::Tofu); // Discord text holds emoji
            first = false;
        }
        app.ui(ui);
    });
    h.run_steps(20);
    std::thread::sleep(std::time::Duration::from_secs(8)); // let Discord connect and avatars arrive
    h.run_steps(20);
    std::thread::sleep(std::time::Duration::from_secs(12)); // the forum opens once listed, then posts arrive
    h.run_steps(20);
    let out = std::env::var("SHOT_DIR").unwrap_or_else(|_| ".".into());
    h.render().expect("render").save(format!("{out}/forum.png")).unwrap();
}
