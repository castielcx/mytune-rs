//! With the saved login on the first DM: clicks the composer's emoji button (emoji.png), then its GIF button
//! (gifs.png). Nothing is picked, so nothing is sent.
use eframe::egui::{self, pos2, vec2};
use egui_kittest::Harness;
use mytune_ui::{app::App, nav::Page};

#[test]
fn pickers() {
    let dir = std::env::temp_dir().join("mytune-ui-shot-picker");
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
    let out = std::env::var("SHOT_DIR").unwrap_or_else(|_| ".".into());
    for (x, name) in [(452.0, "plus"), (890.0, "emoji"), (848.0, "gifs")] {
        let at = pos2(x, 852.0);
        for e in [
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
            egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
        ] {
            h.event(e);
            h.run_steps(2);
        }
        std::thread::sleep(std::time::Duration::from_secs(3));
        h.run_steps(10);
        h.render().expect("render").save(format!("{out}/{name}.png")).unwrap();
    }
}
