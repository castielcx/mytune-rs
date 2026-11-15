//! With the saved login: clicks at env PX,PY (default: first member row), optionally again at PX2,PY2 (Full profile),
//! writes profile.png. Env FORUM names the channel to open. Reads only; nothing is sent.
use eframe::egui::{self, pos2, vec2};
use egui_kittest::Harness;
use mytune_ui::{app::App, nav::Page};

fn num(k: &str) -> Option<f32> {
    std::env::var(k).ok().and_then(|v| v.parse().ok())
}

#[test]
fn profile() {
    if let Ok(f) = std::env::var("FORUM") {
        std::env::set_var("MYTUNE_CHAT_OPEN", f);
    }
    let dir = std::env::temp_dir().join("mytune-ui-shot-profile");
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
    h.render().expect("render").save(format!("{out}/profile0.png")).unwrap();
    let mut clicks = vec![];
    if let (Some(x), Some(y)) = (num("PX"), num("PY")) {
        clicks.push(pos2(x, y));
    }
    if let (Some(x), Some(y)) = (num("PX2"), num("PY2")) {
        clicks.push(pos2(x, y));
    }
    for at in clicks {
        for e in [
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: true, modifiers: Default::default() },
            egui::Event::PointerButton { pos: at, button: egui::PointerButton::Primary, pressed: false, modifiers: Default::default() },
        ] {
            h.event(e);
            h.run_steps(2);
        }
        std::thread::sleep(std::time::Duration::from_secs(3));
        h.run_steps(20);
    }
    h.render().expect("render").save(format!("{out}/profile.png")).unwrap();
}
