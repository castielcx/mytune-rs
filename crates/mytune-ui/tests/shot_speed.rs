use eframe::egui::vec2;
use egui_kittest::Harness;
use mytune_core::speedtest::{save_last, LastSpeed, Phase, SpeedResult};
use mytune_ui::{app::App, nav::Page};

/// A plausible run: the running average climbs, then settles.
fn curve(top: f64, n: usize) -> Vec<f64> {
    (0..n).map(|i| top * (1.0 - 0.45 * (-(i as f64) / 4.0).exp()) * (1.0 + 0.012 * (i as f64 * 1.7).sin())).collect()
}

fn last() -> LastSpeed {
    let result = SpeedResult { down_mbps: 342.0, up_mbps: 48.6, ping_ms: 12.0, jitter_ms: 1.2, loaded_ms: 78.0 };
    LastSpeed { at: "2026-11-21 14:32".into(), result, down: curve(342.0, 28), up: curve(48.6, 28) }
}

/// The speed card in every state, at normal and narrow width. No network traffic.
#[test]
fn speed_card() {
    let out = std::env::var("SHOT_DIR").unwrap_or_else(|_| ".".into());
    let _ = std::fs::create_dir_all(&out);
    for (size, w, h) in [("normal", 1180.0, 900.0), ("narrow", 760.0, 1000.0)] {
        for state in ["idle-empty", "idle-saved", "mid-download", "mid-upload", "finished"] {
            let dir = std::env::temp_dir().join(format!("mytune-ui-shot-speed-{size}-{state}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            if state == "idle-saved" {
                save_last(&dir, &last()).unwrap(); // read back by the page, as after a restart
            }
            let mut app = App::with_store(mytune_core::store::Store::load(&dir));
            app.goto(Page::Vpn);
            match state {
                "mid-download" => app.fake_speed(Phase::Download, &curve(342.0, 14), &[]),
                "mid-upload" => app.fake_speed(Phase::Upload, &curve(342.0, 28), &curve(48.6, 12)),
                "finished" => app.fake_speed_done(last()),
                _ => {}
            }
            let mut first = true;
            let mut harness = Harness::builder().with_size(vec2(w, h)).build_ui(move |ui| {
                if first {
                    mytune_ui::theme::install_fonts(ui.ctx());
                    mytune_ui::theme::apply_visuals(ui.ctx());
                    first = false;
                }
                app.ui(ui);
            });
            harness.run_steps(90); // springs settle
            harness.render().expect("render").save(format!("{out}/speed-{size}-{state}.png")).unwrap();
        }
    }
}
