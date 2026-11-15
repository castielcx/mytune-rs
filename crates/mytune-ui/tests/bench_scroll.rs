//! With the saved login: opens the channel named by env FORUM (default: the first DM), scrolls its timeline up and
//! down for 120 frames and prints the mean frame time as `SCROLL_MS_PER_FRAME`. Reads only; nothing is sent.
//! `cargo test -p mytune-ui --test bench_scroll -- --nocapture`
use eframe::egui::{self, pos2, vec2};
use egui_kittest::Harness;
use mytune_ui::{app::App, nav::Page};

#[test]
fn scroll() {
    if let Ok(f) = std::env::var("FORUM") {
        std::env::set_var("MYTUNE_CHAT_OPEN", f);
    }
    let dir = std::env::temp_dir().join("mytune-ui-bench-scroll");
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
    // the first frames with the room's messages: every row is laid out for the first time
    let (mut open_worst, open) = (0.0f64, std::time::Instant::now());
    for _ in 0..40 {
        let f = std::time::Instant::now();
        h.run_steps(1);
        open_worst = open_worst.max(f.elapsed().as_secs_f64() * 1000.0);
    }
    println!("OPEN_WORST_MS {open_worst:.2}  OPEN_40_FRAMES_MS {:.2}", open.elapsed().as_secs_f64() * 1000.0);
    h.event(egui::Event::PointerMoved(pos2(740.0, 500.0)));
    h.run_steps(2);
    let t = std::time::Instant::now();
    let mut worst = 0.0f64;
    for i in 0..120 {
        let f = std::time::Instant::now();
        h.event(egui::Event::MouseWheel {
            unit: egui::MouseWheelUnit::Point,
            delta: vec2(0.0, if i < 60 { 60.0 } else { -60.0 }),
            phase: egui::TouchPhase::Move,
            source: egui::MouseWheelSource::Wheel,
            modifiers: Default::default(),
        });
        h.run_steps(1);
        worst = worst.max(f.elapsed().as_secs_f64() * 1000.0);
    }
    println!("SCROLL_WORST_MS {worst:.2}");
    println!("SCROLL_MS_PER_FRAME {:.2}", t.elapsed().as_secs_f64() * 1000.0 / 120.0);
    if let Ok(out) = std::env::var("SHOT_DIR") {
        h.render().expect("render").save(format!("{out}/bench.png")).unwrap();
    }
}
