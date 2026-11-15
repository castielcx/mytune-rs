//! Friends screen with the saved login: clicks env CLICK ("x,y", a friend's name or picture) and writes
//! friendclick-0.png (before), -1.png (2 frames after the click) and -2.png (3 s later): the profile card
//! must be up in -1 already.
use eframe::egui::{self, pos2, vec2};
use egui_kittest::Harness;
use mytune_ui::{app::App, nav::Page};

#[test]
fn friend_click_opens_profile() {
    std::env::set_var("MYTUNE_CHAT_SCREEN", "friends");
    let dir = std::env::temp_dir().join("mytune-ui-shot-friendclick");
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
    std::thread::sleep(std::time::Duration::from_secs(10));
    h.run_steps(30);
    let out = std::env::var("SHOT_DIR").unwrap_or_else(|_| ".".into());
    h.render().expect("render").save(format!("{out}/friendclick-0.png")).unwrap();
    let Ok(v) = std::env::var("CLICK") else { return };
    let (x, y) = v.split_once(',').expect("CLICK=x,y");
    let at = pos2(x.parse().unwrap(), y.parse().unwrap());
    for e in [
        egui::Event::PointerMoved(at),
        egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
        egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
    ] {
        h.event(e);
        h.run_steps(1);
    }
    h.run_steps(2);
    h.render().expect("render").save(format!("{out}/friendclick-1.png")).unwrap();
    std::thread::sleep(std::time::Duration::from_secs(3));
    h.run_steps(10);
    h.render().expect("render").save(format!("{out}/friendclick-2.png")).unwrap();
}
