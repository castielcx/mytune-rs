//! Renders every chat settings page with the saved login: settings-0.png .. settings-7.png in SHOT_DIR.
//! The Voice page opens the microphone for its level meter while it shows (nothing is sent).
use eframe::egui::{self, vec2};
use egui_kittest::Harness;
use mytune_ui::{app::App, nav::Page};

#[test]
fn settings_pages() {
    let dir = std::env::temp_dir().join("mytune-ui-shot-settings");
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
    std::thread::sleep(std::time::Duration::from_secs(8));
    h.run_steps(10);
    let out = std::env::var("SHOT_DIR").unwrap_or_else(|_| ".".into());
    for page in 0..8 {
        std::env::set_var("MYTUNE_CHAT_SETTINGS", page.to_string());
        h.run_steps(5);
        std::thread::sleep(std::time::Duration::from_millis(600));
        h.run_steps(5);
        h.render().expect("render").save(format!("{out}/settings-{page}.png")).unwrap();
    }
}
