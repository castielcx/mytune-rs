use eframe::egui::vec2;
use egui_kittest::Harness;
use mytune_ui::{app::App, nav::Page};

#[test]
fn chat_page() {
    let dir = std::env::temp_dir().join("mytune-ui-shot-chat");
    let _ = std::fs::create_dir_all(&dir);
    let mut app = App::with_store(mytune_core::store::Store::load(&dir));
    // env SHOT_OTHER: a page outside the chat, to compare the app shell at the same window size
    app.goto(if std::env::var_os("SHOT_OTHER").is_some() { Page::Customize } else { Page::Chat });
    let mut first = true;
    // env PPP: pixels per point, 1.15 is the real app's scale (`UI_ZOOM`)
    let ppp = std::env::var("PPP").ok().and_then(|v| v.parse().ok()).unwrap_or(1.0);
    // env PAGE_ZOOM: the real window's chat page (shell compensated, chat fonts); use with PPP=1.0
    let mut size = vec2(1180.0, 900.0);
    if std::env::var_os("PAGE_ZOOM").is_some() {
        mytune_ui::theme::set_shell(mytune_ui::theme::UI_ZOOM);
        size *= mytune_ui::theme::UI_ZOOM; // the same window in pixels as PPP=1.15
    }
    let mut h = Harness::builder().with_size(size).with_pixels_per_point(ppp).build_ui(move |ui| {
        if first {
            mytune_ui::theme::install_fonts(ui.ctx());
            mytune_ui::theme::apply_visuals(ui.ctx());
            ui.ctx().set_missing_glyph_policy(eframe::egui::MissingGlyphPolicy::Tofu); // Discord text holds emoji
            first = false;
        }
        // the harness puts 8 points around the ui, which the real window has not: same pixels as at PPP=1.15
        let edge = if mytune_ui::theme::shell() != 1.0 { 8.0 * (mytune_ui::theme::shell() - 1.0) } else { 0.0 };
        let r = ui.max_rect().shrink(edge);
        ui.scope_builder(eframe::egui::UiBuilder::new().max_rect(r), |ui| app.ui(ui));
    });
    h.run_steps(20);
    std::thread::sleep(std::time::Duration::from_secs(8)); // let Discord connect and avatars arrive
    h.run_steps(20);
    let out = std::env::var("SHOT_DIR").unwrap_or_else(|_| ".".into());
    h.render().expect("render").save(format!("{out}/chat.png")).unwrap();
}
