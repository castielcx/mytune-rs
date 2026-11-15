use eframe::egui::vec2;
use egui_kittest::Harness;
use mytune_ui::{app::App, nav::Page};

#[test]
fn music_page() {
    let dir = std::env::temp_dir().join("mytune-ui-shot-music");
    let _ = std::fs::create_dir_all(&dir);
    let mut app = App::with_store(mytune_core::store::Store::load(&dir));
    app.goto(Page::Music);
    let mut first = true;
    let mut h = Harness::builder().with_size(vec2(1180.0, 900.0)).build_ui(move |ui| {
        if first {
            mytune_ui::theme::install_fonts(ui.ctx());
            mytune_ui::theme::apply_visuals(ui.ctx());
            first = false;
        }
        app.ui(ui);
    });
    h.run_steps(20);
    std::thread::sleep(std::time::Duration::from_millis(3500)); // let the media thread read the song and its picture
    h.run_steps(20);
    let out = std::env::var("SHOT_DIR").unwrap_or_else(|_| ".".into());
    h.render().expect("render").save(format!("{out}/music.png")).unwrap();
}
