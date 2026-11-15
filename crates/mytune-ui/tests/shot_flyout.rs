//! Right-clicks the second server icon on the rail with the saved login, rests the pointer on env HOVER
//! ("x,y": an arrow row such as Mute Server) and writes flyout.png: the submenu must stand beside the menu.
use eframe::egui::{self, pos2, vec2};
use egui_kittest::Harness;
use mytune_ui::{app::App, nav::Page};

#[test]
fn submenu_opens_beside_the_menu() {
    let dir = std::env::temp_dir().join("mytune-ui-shot-flyout");
    let _ = std::fs::create_dir_all(&dir);
    let mut app = App::with_store(mytune_core::store::Store::load(&dir));
    app.goto(Page::Chat);
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
    std::thread::sleep(std::time::Duration::from_secs(8));
    h.run_steps(20);
    let at = pos2(108.0, 196.0); // second icon on the rail
    for e in [
        egui::Event::PointerMoved(at),
        egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: true, modifiers: Default::default() },
        egui::Event::PointerButton { pos: at, button: egui::PointerButton::Secondary, pressed: false, modifiers: Default::default() },
    ] {
        h.event(e);
        h.run_steps(2);
    }
    if let Ok(v) = std::env::var("HOVER") {
        let (x, y) = v.split_once(',').expect("HOVER=x,y");
        h.event(egui::Event::PointerMoved(pos2(x.parse().unwrap(), y.parse().unwrap())));
    }
    h.run_steps(20);
    let out = std::env::var("SHOT_DIR").unwrap_or_else(|_| ".".into());
    h.render().expect("render").save(format!("{out}/flyout.png")).unwrap();
}
