use eframe::egui::vec2;
use egui_kittest::Harness;
use mytune_ui::{app::App, nav::Page};

#[test]
fn internet_page() {
    let dir = std::env::temp_dir().join("mytune-ui-shot-internet");
    let _ = std::fs::create_dir_all(&dir);
    let mut app = App::with_store(mytune_core::store::Store::load(&dir));
    app.goto(Page::Vpn);
    let mut first = true;
    let mut h = Harness::builder().with_size(vec2(1180.0, 1100.0)).build_ui(move |ui| {
        if first {
            mytune_ui::theme::install_fonts(ui.ctx());
            mytune_ui::theme::apply_visuals(ui.ctx());
            first = false;
        }
        app.ui(ui);
    });
    h.run();
    std::thread::sleep(std::time::Duration::from_secs(6)); // let the adapter and VPN scans finish
    h.run();
    let out = std::env::var("SHOT_DIR").unwrap_or_else(|_| ".".into());
    h.render().expect("render").save(format!("{out}/internet.png")).unwrap();
}
