use eframe::egui::{self, vec2};
use mytune_ui::{app::App, nav::Page};
use egui_kittest::Harness;

fn shot(page: Page, name: &str) {
    // SHOT_STORE: folder of a state.json to show (a copy of the real one, for the Restore list)
    let dir = std::env::var("SHOT_STORE").map_or_else(|_| std::env::temp_dir().join("mytune-ui-shot"), Into::into);
    let _ = std::fs::create_dir_all(&dir);
    let mut app = App::with_store(mytune_core::store::Store::load(&dir));
    app.goto(page);
    if let Ok(a) = std::env::var("SHOT_ACCENT") {
        mytune_ui::theme::set_accent_value(&a);
    }
    let mut first = true;
    let mut h = Harness::builder().with_size(vec2(std::env::var("SHOT_W").ok().and_then(|v| v.parse().ok()).unwrap_or(1180.0), std::env::var("SHOT_H").ok().and_then(|v| v.parse().ok()).unwrap_or(780.0))).build_ui(move |ui| {
        if first {
            mytune_ui::theme::install_fonts(ui.ctx());
            mytune_ui::theme::apply_visuals(ui.ctx());
            first = false;
        }
        app.ui(ui);
    });
    for _ in 0..40 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    if page == Page::Dashboard || page == Page::Diagnostics {
        std::thread::sleep(std::time::Duration::from_millis(if page == Page::Diagnostics { 7000 } else { 1500 }));
        for _ in 0..5 {
            h.step();
        }
    }
    let img = h.render().expect("render");
    let out = std::env::var("SHOT_DIR").unwrap_or_else(|_| ".".into());
    img.save(format!("{out}/{name}.png")).unwrap();
    let _ = egui::Id::NULL;
}

#[test]
fn shots() {
    if std::env::var("SHOT_ONLY").as_deref() == Ok("restore") {
        return shot(Page::Restore, "restore");
    }
    shot(Page::Customize, "customize");
    if std::env::var("SHOT_ONLY").as_deref() == Ok("customize") {
        return;
    }
    shot(Page::Dashboard, "home");
    shot(Page::Optimize, "optimize");
    shot(Page::Games, "games");
    shot(Page::Vpn, "vpn");
    shot(Page::Mouse, "mouse");
    shot(Page::Diagnostics, "diagnostics");
}
