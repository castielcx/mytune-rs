use eframe::egui::vec2;
use egui_kittest::Harness;
use mytune_ui::{app::App, nav::Page};

#[test]
fn bios_page() {
    let dir = std::env::temp_dir().join("mytune-ui-shot-bios");
    let _ = std::fs::create_dir_all(&dir);
    let mut app = App::with_store(mytune_core::store::Store::load(&dir));
    app.goto(Page::Bios);
    let mut first = true;
    let h_px = std::env::var("SHOT_H").ok().and_then(|v| v.parse().ok()).unwrap_or(1000.0);
    let mut h = Harness::builder().with_size(vec2(1180.0, h_px)).build_ui(move |ui| {
        if first {
            mytune_ui::theme::install_fonts(ui.ctx());
            mytune_ui::theme::apply_visuals(ui.ctx());
            first = false;
        }
        app.ui(ui);
    });
    h.run_steps(5);
    // the read thread starts PowerShell and nvidia-smi
    std::thread::sleep(std::time::Duration::from_secs(4));
    h.run_steps(40);
    let out = std::env::var("SHOT_DIR").unwrap_or_else(|_| ".".into());
    h.render().expect("render").save(format!("{out}/bios.png")).unwrap();
}
