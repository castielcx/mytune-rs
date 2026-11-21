use eframe::egui::vec2;
use egui_kittest::Harness;
use mytune_ui::{app::App, nav::Page};

#[test]
fn crosshair_page() {
    let dir = std::env::temp_dir().join("mytune-ui-shot-crosshair");
    let _ = std::fs::create_dir_all(&dir);
    let mut store = mytune_core::store::Store::load(&dir);
    let mut look = mytune_core::crosshair::Crosshair::default();
    let mut saves = vec![];
    for (i, name) in ["Classic", "Valorant red", "Long cyan", "T shape", "Dot"].iter().enumerate() {
        look.preset(i);
        if i == 2 {
            (look.arms, look.rotation, look.round, look.blur) = (6, 15, true, 1);
        }
        if i == 4 {
            (look.dot_circle, look.dot_size, look.dot_blur) = (true, 9, 2);
        }
        saves.push(mytune_core::crosshair::Saved { name: name.to_string(), look: look.clone() });
    }
    mytune_core::crosshair::save_saves(&mut store, &saves).unwrap();
    let mut app = App::with_store(store);
    app.goto(Page::Crosshair);
    // Lines tab shows a rotated, round-ended, blurred look so every control has an effect to look at
    let mut c = mytune_core::crosshair::Crosshair::default();
    c.dot = true;
    c.arms = 6;
    c.rotation = 20;
    c.round = true;
    c.blur = 2;
    c.outline_thickness = 2;
    mytune_core::crosshair::set(c);
    let mut first = true;
    let mut h = Harness::builder().with_size(vec2(1180.0, 1250.0)).build_ui(move |ui| {
        if first {
            mytune_ui::theme::install_fonts(ui.ctx());
            mytune_ui::theme::apply_visuals(ui.ctx());
            first = false;
        }
        app.ui(ui);
    });
    h.run_steps(40);
    let out = std::env::var("SHOT_DIR").unwrap_or_else(|_| ".".into());
    // env MYTUNE_CROSSHAIR_TAB (0 Lines, 1 Center Dot, 2 Outline) picks the editor tab
    let tab = std::env::var("MYTUNE_CROSSHAIR_TAB").unwrap_or_else(|_| "0".into());
    h.render().expect("render").save(format!("{out}/crosshair-tab{tab}.png")).unwrap();
}
