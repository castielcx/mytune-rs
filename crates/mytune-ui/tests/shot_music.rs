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
    // CLICK=x,y: hover there, click, and let the result settle (e.g. the + on a SoundCloud row)
    if let Ok(c) = std::env::var("CLICK") {
        let mut it = c.split(',').filter_map(|v| v.trim().parse::<f32>().ok());
        let pos = eframe::egui::pos2(it.next().unwrap(), it.next().unwrap());
        h.input_mut().events.push(eframe::egui::Event::PointerMoved(pos));
        h.run_steps(3);
        let click: &[bool] = if std::env::var("NOCLICK").is_ok() { &[] } else { &[true, false] };
        for &pressed in click {
            h.input_mut().events.push(eframe::egui::Event::PointerButton { pos, button: eframe::egui::PointerButton::Primary, pressed, modifiers: Default::default() });
            h.run_steps(2);
        }
        std::thread::sleep(std::time::Duration::from_millis(3000));
        h.run_steps(10);
    }
    let out = std::env::var("SHOT_DIR").unwrap_or_else(|_| ".".into());
    h.render().expect("render").save(format!("{out}/music.png")).unwrap();
}
