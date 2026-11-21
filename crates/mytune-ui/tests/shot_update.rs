#![cfg(feature = "license")]
//! The update screen and banner in each phase, and the gate inside `App`. Writes update-*.png to SHOT_DIR.
//! `updater::start` never runs here, so nothing contacts the update feed or the key server.
use eframe::egui::{vec2, LayerId, Ui, Vec2};
use egui_kittest::Harness;
use mytune_core::updater::{self, Pending, Phase, UpdateInfo};
use mytune_ui::{app::App, theme as th, updategate};
use std::sync::Mutex;

/// Two harnesses rendering at once crash the process (access violation).
static ONE_AT_A_TIME: Mutex<()> = Mutex::new(());

fn pending(phase: Phase, blocks: bool) -> Pending {
    let info = UpdateInfo {
        version: "0.3.0".into(),
        url: "https://github.com/castielcx/mytune-rs/releases/download/v0.3.0/MyTune-rs-Setup-0.3.0.exe".into(),
        sha256: "0".repeat(64),
    };
    Pending { info, phase, blocks }
}

fn save(h: &mut Harness<'_>, name: &str) {
    let dir = std::env::var("SHOT_DIR").map(Into::into).unwrap_or_else(|_| std::env::temp_dir().join("mytune-update-shots"));
    std::fs::create_dir_all(&dir).unwrap();
    let path: std::path::PathBuf = dir.join(name);
    h.render().expect("render").save(&path).unwrap();
    assert!(std::fs::metadata(&path).unwrap().len() > 1024, "{name} is empty");
}

fn shot(size: Vec2, name: &str, mut draw: impl FnMut(&mut Ui)) {
    let mut first = true;
    let mut h = Harness::builder().with_size(size).build_ui(|ui| {
        if first {
            th::install_fonts(ui.ctx());
            th::apply_visuals(ui.ctx());
            first = false;
        }
        th::backdrop(ui.ctx(), &ui.ctx().layer_painter(LayerId::background()), ui.ctx().content_rect());
        draw(ui);
    });
    for _ in 0..10 {
        h.step();
    }
    save(&mut h, name);
}

#[test]
fn update_screens() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let failed = || Phase::Failed("Setup stopped with code 5. Antivirus may have blocked it.".into());
    let downloading = || Phase::Downloading { got: 42, total: Some(100) };
    for (name, phase) in [("ready", Phase::Ready), ("downloading", downloading()), ("installing", Phase::Installing), ("failed", failed())] {
        let p = pending(phase, true);
        shot(vec2(1120.0, 720.0), &format!("update-{name}.png"), |ui| {
            updategate::ui(ui, &p, true);
        });
    }
    for (name, phase) in [("ready", Phase::Ready), ("downloading", downloading()), ("failed", failed())] {
        let p = pending(phase, false);
        shot(vec2(1120.0, 44.0), &format!("update-banner-{name}.png"), |ui| {
            let bar = ui.ctx().content_rect(); // the root ui has a margin, the real title bar has none
            updategate::banner(ui, bar, &p);
        });
    }
}

#[test]
fn app_shows_the_gate() {
    let _one = ONE_AT_A_TIME.lock().unwrap_or_else(|e| e.into_inner());
    let dir = std::env::temp_dir().join("mytune-update-shot-app");
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = App::with_store(mytune_core::store::Store::load(&dir));
    updater::fake_pending(Some(pending(Phase::Ready, true)));
    let mut first = true;
    let mut h = Harness::builder().with_size(vec2(1120.0, 720.0)).build_ui(move |ui| {
        if first {
            th::install_fonts(ui.ctx());
            th::apply_visuals(ui.ctx());
            first = false;
        }
        app.ui(ui);
    });
    for _ in 0..40 {
        h.step();
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    save(&mut h, "update-app.png");
    updater::fake_pending(None);
}
