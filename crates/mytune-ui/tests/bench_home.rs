//! Home page: mean and worst frame time over 200 frames (the bar's light animation repaints every frame).
//! `.\build.ps1 test -p mytune-ui --test bench_home '--' --nocapture`
use eframe::egui::vec2;
use egui_kittest::Harness;
use mytune_ui::{app::App, nav::Page};

#[test]
fn home() {
    std::env::set_var("MYTUNE_NO_CAP", "1");
    let dir = std::env::temp_dir().join("mytune-ui-bench-home");
    let _ = std::fs::create_dir_all(&dir);
    let mut app = App::with_store(mytune_core::store::Store::load(&dir));
    app.goto(Page::Dashboard);
    let mut first = true;
    let mut h = Harness::builder().with_size(vec2(1180.0, 780.0)).build_ui(move |ui| {
        if first {
            mytune_ui::theme::install_fonts(ui.ctx());
            mytune_ui::theme::apply_visuals(ui.ctx());
            first = false;
        }
        app.ui(ui);
    });
    for _ in 0..40 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    let (mut worst, t) = (0.0f64, std::time::Instant::now());
    for _ in 0..200 {
        let f = std::time::Instant::now();
        h.step();
        worst = worst.max(f.elapsed().as_secs_f64() * 1000.0);
    }
    println!("HOME_MS_PER_FRAME {:.2}  HOME_WORST_MS {worst:.2}", t.elapsed().as_secs_f64() * 1000.0 / 200.0);
}
