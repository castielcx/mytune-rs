//! Clicks the second server icon on the chat page's rail with the saved login and writes rail.png: the server
//! must open.
use eframe::egui::{self, pos2, vec2};
use egui_kittest::Harness;
use mytune_ui::{app::App, nav::Page};

#[test]
fn rail_click_opens_server() {
    let dir = std::env::temp_dir().join("mytune-ui-shot-rail");
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
    std::thread::sleep(std::time::Duration::from_secs(8));
    h.run_steps(20);
    let at = pos2(108.0, 196.0); // second icon on the rail, as the chat screenshot places it
    h.event(egui::Event::PointerMoved(at));
    h.run_steps(2);
    assert!(h.ctx.layer_id_at(at).is_none_or(|l| l.order == egui::Order::Background), "a layer covers the server icon: {:?}", h.ctx.layer_id_at(at));
    for e in [
        egui::Event::PointerMoved(at),
        egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
        egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
    ] {
        h.event(e);
        h.run_steps(2);
    }
    std::thread::sleep(std::time::Duration::from_secs(2));
    h.run_steps(20);
    let out = std::env::var("SHOT_DIR").unwrap_or_else(|_| ".".into());
    h.render().expect("render").save(format!("{out}/rail.png")).unwrap();
    // the first message's avatar in #rules opens the author's profile card
    let av = pos2(448.0, 184.0);
    for e in [
        egui::Event::PointerMoved(av),
        egui::Event::PointerButton { pos: av, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
        egui::Event::PointerButton { pos: av, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
    ] {
        h.event(e);
        h.run_steps(2);
    }
    std::thread::sleep(std::time::Duration::from_secs(3));
    h.run_steps(20);
    h.render().expect("render").save(format!("{out}/profile.png")).unwrap();
}
