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
    // "stickers" clicks the third tab of the open emoji picker; it exists only with feature `local-stickers`
    // the sticker button sits between GIF and emoji, one button step (43 px) left of emoji's neighbour
    let (sticker_x, gif_x) = if cfg!(feature = "local-stickers") { (1078.0, 1035.0) } else { (0.0, 1078.0) };
    for (x, y, name) in [(440.0, 843.0, "plus"), (1121.0, 843.0, "emoji"), (907.0, 378.0, "stickers"), (sticker_x, 843.0, "sticker_button"), (gif_x, 843.0, "gifs")] {
        if name == "sticker_button" && !cfg!(feature = "local-stickers") {
            continue;
        }
        if name == "stickers" && !cfg!(feature = "local-stickers") {
            continue;
        }
        let at = pos2(x, y);
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
