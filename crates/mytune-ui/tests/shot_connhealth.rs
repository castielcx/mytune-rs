use eframe::egui::vec2;
use egui_kittest::Harness;
use mytune_core::connhealth::{Grade, Row};
use mytune_ui::{app::App, nav::Page};

/// The Connection health card with one row per chip. No network traffic.
#[test]
fn health_card() {
    let out = std::env::var("SHOT_DIR").unwrap_or_else(|_| ".".into());
    let _ = std::fs::create_dir_all(&out);
    let dir = std::env::temp_dir().join("mytune-ui-shot-health");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::with_store(mytune_core::store::Store::load(&dir));
    app.goto(Page::Vpn);
    let r = |name, grade, text: &str, fix| Row { name, grade, text: text.into(), fix };
    app.fake_health(vec![
        r("Router ping", Grade::Warning, "Lost 2 of 50. Median 1 ms, p95 3 ms.", Some("Replace the cable or try another router port.")),
        r("Adapter errors", Grade::Good, "0 bad of 6825639 packets (0.000%) since the adapter started.", None),
        r("Ethernet link speed", Grade::Info, "Link runs at 1 Gbps, the card supports 2.5 Gbps. Not a fault: the router may top out at 1 Gbps.", None),
        r("Wi-Fi signal", Grade::Problem, "Signal 34%. Link rate 144 Mbps down, 144 Mbps up.", Some("Move closer to the router or use a cable.")),
        r("Wi-Fi channel", Grade::NotMeasured, "Windows denied the Wi-Fi scan. Allow location access for desktop apps in Windows Settings, Privacy, Location.", None),
    ]);
    let mut first = true;
    let mut harness = Harness::builder().with_size(vec2(1180.0, 1320.0)).build_ui(move |ui| {
        if first {
            mytune_ui::theme::install_fonts(ui.ctx());
            mytune_ui::theme::apply_visuals(ui.ctx());
            first = false;
        }
        app.ui(ui);
    });
    harness.run_steps(90);
    harness.render().expect("render").save(format!("{out}/connection-health.png")).unwrap();
}
