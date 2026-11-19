//! Draws the lens (`mytune_ui::lens`) in a real window and saves what the window shows, then closes: three
//! pictures named after env `LENS_OUT` (default `lens`): `<name>.png` (clear glass shapes, a frosted floating
//! panel, the tab row), `<name>-switch.png` (the app's switch with its knob held) and `<name>-slider.png` (the
//! app's slider with its thumb held). The pointer presses are made up here, so nobody has to hold the mouse.
//! The headless screenshot tests cannot show the lens: they draw with wgpu, and the lens is an OpenGL pass.
//!
//! `.\build.ps1 run -p mytune-ui --example lens_demo --features 'local-stickers,preview' --target-dir target`
use eframe::egui::{self, pos2, vec2, Color32, Pos2, Rect};
use mytune_ui::{lens, theme as th, widgets};
use std::time::Instant;

const SWITCH: Rect = Rect::from_min_max(pos2(500.0, 140.0), pos2(540.0, 164.0));
const SLIDER: Rect = Rect::from_min_max(pos2(500.0, 180.0), pos2(700.0, 200.0));
/// When the pointer is held down where (seconds from the start), and when each picture is taken.
const HOLDS: [(f32, f32, Pos2); 2] = [(0.5, 1.1, pos2(510.0, 152.0)), (1.4, 2.0, pos2(600.0, 190.0))];
const SHOTS: [(f32, &str); 3] = [(0.3, ""), (0.95, "-switch"), (1.85, "-slider")];

struct Demo {
    start: Instant,
    tab: usize,
    on: bool,
    value: f64,
    pressed: Option<Pos2>,
    shots: usize,
}

impl eframe::App for Demo {
    fn raw_input_hook(&mut self, _: &egui::Context, raw: &mut egui::RawInput) {
        let t = self.start.elapsed().as_secs_f32();
        let want = HOLDS.into_iter().find(|(from, to, _)| (*from..*to).contains(&t)).map(|h| h.2);
        if want == self.pressed {
            return;
        }
        let button = |pos, pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() };
        if let Some(pos) = self.pressed {
            raw.events.push(button(pos, false));
        }
        if let Some(pos) = want {
            raw.events.push(egui::Event::PointerMoved(pos));
            raw.events.push(button(pos, true));
        }
        self.pressed = want;
    }

    fn ui(&mut self, ui: &mut egui::Ui, _: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        th::backdrop(&ctx, &ctx.layer_painter(egui::LayerId::background()), ctx.content_rect());
        let p = ui.painter().clone();
        // what the lenses bend: stripes and lines of text
        for i in 0..40 {
            let colour = if i % 2 == 0 { Color32::from_rgb(90, 160, 255) } else { Color32::from_rgb(255, 150, 80) };
            p.rect_filled(Rect::from_min_size(pos2(20.0 + i as f32 * 18.0, 20.0), vec2(8.0, 190.0)), 0.0, colour);
        }
        for i in 0..8 {
            p.text(pos2(20.0, 225.0 + i as f32 * 22.0), egui::Align2::LEFT_TOP, "The quick brown fox jumps over the lazy dog 0123456789", egui::FontId::proportional(15.0), Color32::WHITE);
        }
        // clear glass, as a held knob or thumb
        let knob = Rect::from_center_size(pos2(120.0, 95.0), vec2(72.0, 72.0));
        p.add(lens::shape(knob, lens::Lens { radius: 36.0, rim: 30.0, bend: 12.0, zoom: 1.25, blur: 0.0, split: 0.3 }));
        th::capsule(&p, knob, Color32::from_white_alpha(40), 0x60);
        // a wide clear plate
        let bar = Rect::from_min_size(pos2(230.0, 60.0), vec2(220.0, 70.0));
        p.add(lens::shape(bar, lens::Lens { radius: 16.0, rim: 14.0, bend: 8.0, zoom: 1.0, blur: 0.0, split: 0.3 }));
        th::plate(&p, bar, 16.0, Color32::from_white_alpha(24), 0x60);
        // frosted: a floating panel over the text
        egui::Area::new(egui::Id::new("panel")).fixed_pos(pos2(300.0, 235.0)).show(&ctx, |ui| {
            th::float(ui, th::R_MENU, 12, |ui| {
                ui.set_width(220.0);
                for row in ["Mark as read", "Mute", "Copy link"] {
                    ui.label(th::rt(row, 14.0));
                    ui.add_space(8.0);
                }
            });
        });
        // the app's own controls: the tab row (its thumb is glass over the names), a switch and a slider
        let mut at = |rect: Rect, add: &mut dyn FnMut(&mut egui::Ui)| {
            ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| add(ui));
        };
        at(Rect::from_min_size(pos2(500.0, 80.0), vec2(240.0, 40.0)), &mut |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (i, name) in ["General", "Restore"].into_iter().enumerate() {
                    if th::tab(ui, name, self.tab == i).clicked() {
                        self.tab = i;
                    }
                }
            });
        });
        at(SWITCH, &mut |ui| {
            if th::switch(ui, self.on).clicked() {
                self.on = !self.on;
            }
        });
        at(SLIDER, &mut |ui| {
            widgets::slider(ui, egui::Id::new("demo-slider"), &mut self.value, 0.0, 100.0, SLIDER.width(), true);
        });
        if let Some((when, suffix)) = SHOTS.get(self.shots).copied() {
            if self.start.elapsed().as_secs_f32() >= when {
                self.shots += 1;
                let (window, last) = (ctx.clone(), self.shots == SHOTS.len());
                ctx.request_screenshot(move |img| {
                    let out = format!("{}{suffix}.png", std::env::var("LENS_OUT").unwrap_or_else(|_| "lens".into()));
                    let [w, h] = img.size;
                    image::RgbaImage::from_raw(w as u32, h as u32, img.as_raw().to_vec()).expect("picture").save(&out).expect("save");
                    println!("saved {out}, lens works: {}", lens::works());
                    if last {
                        window.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            }
        }
        ctx.request_repaint();
    }
}

fn main() -> eframe::Result {
    const ZOOM: f32 = th::UI_ZOOM;
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([760.0 * ZOOM, 420.0 * ZOOM]).with_title("MyTune lens demo"),
        dithering: false,
        ..Default::default()
    };
    eframe::run_native(
        "lens-demo",
        opts,
        Box::new(|cc| {
            cc.egui_ctx.set_zoom_factor(ZOOM);
            th::install_fonts(&cc.egui_ctx);
            th::apply_visuals(&cc.egui_ctx);
            Ok(Box::new(Demo { start: Instant::now(), tab: 0, on: false, value: 50.0, pressed: None, shots: 0 }))
        }),
    )
}
