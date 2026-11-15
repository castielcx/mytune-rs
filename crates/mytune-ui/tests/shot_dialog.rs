//! Settings in a tall window, then a click at env CLICK ("x,y", default: the "What's new" button): writes
//! dialog.png with that dialog open. The dialog's own buttons are not clicked.
use eframe::egui::{self, pos2, vec2};
use egui_kittest::Harness;
use mytune_ui::{app::App, nav::Page};

#[test]
fn optimize_dialog() {
    let dir = std::env::temp_dir().join("mytune-ui-shot");
    let _ = std::fs::create_dir_all(&dir);
    let mut app = App::with_store(mytune_core::store::Store::load(&dir));
    app.goto(Page::Settings);
    let mut first = true;
    let mut h = Harness::builder().with_size(vec2(1180.0, 1700.0)).build_ui(move |ui| {
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
    let click = std::env::var("CLICK").unwrap_or_else(|_| "1085,1191".into());
    let (x, y) = click.split_once(',').unwrap();
    let at = pos2(x.parse().unwrap(), y.parse().unwrap());
    for e in [
        egui::Event::PointerMoved(at),
        egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
        egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
    ] {
        h.event(e);
        h.run_steps(2);
    }
    for _ in 0..30 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let out = std::env::var("SHOT_DIR").unwrap_or_else(|_| ".".into());
    h.render().expect("render").save(format!("{out}/dialog.png")).unwrap();
}
