use eframe::egui::vec2;
use egui_kittest::Harness;
use mytune_sensors::IoNow;
use mytune_ui::searchstats::{panel, Stats};

/// The panel at its real pixel size (178 x 720 at the app's 1.15 zoom), with one sensor missing.
#[test]
fn search_stats_panel() {
    let out = std::env::var("SHOT_FILE").unwrap_or_else(|_| "search-stats.png".into());
    let stats = Stats {
        cpu: Some(23.0),
        cpu_temp: Some(54.0),
        gpu: Some(100.0),
        gpu_temp: None,
        ram: Some(61.0),
        ram_gb: Some(19.6),
        io: IoNow { disk_active: Some(8.0), disk_bytes: Some(148.5 * 1048576.0), down_bytes: Some(12.4 * 1048576.0), up_bytes: Some(860.0 * 1024.0) },
    };
    let zoom = mytune_ui::theme::UI_ZOOM;
    let black = out.replace(".png", "-black.png");
    for (accent, file) in [("#FF5A0A", out), ("#000000", black)] {
        mytune_ui::theme::set_accent_value(accent);
        let mut first = true;
        let mut harness = Harness::builder().with_size(vec2(178.0 / zoom, 720.0 / zoom)).with_pixels_per_point(zoom).build_ui(move |ui| {
            if first {
                mytune_ui::theme::install_fonts(ui.ctx());
                mytune_ui::theme::apply_visuals(ui.ctx());
                first = false;
            }
            panel(ui, &stats);
        });
        harness.run_steps(4);
        harness.render().expect("render").save(file).unwrap();
    }
}
