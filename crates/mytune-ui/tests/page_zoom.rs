use eframe::egui::{self, pos2, vec2, Event, RawInput, Rect, ViewportId};
use mytune_ui::{app::App, nav::Page, theme};

fn input(events: Vec<Event>) -> RawInput {
    let mut raw = RawInput { screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 800.0))), events, ..Default::default() };
    raw.viewports.entry(ViewportId::ROOT).or_default().native_pixels_per_point = Some(1.0);
    raw
}

/// One frame of a 1000 x 420 px window with the shell at scale `k`, as `page_input` sets it.
fn frame(app: &mut App, ctx: &egui::Context, t: &mut f64, k: f32, events: Vec<Event>) {
    *t += 1.0 / 60.0;
    theme::set_shell(k);
    let ppp = theme::UI_ZOOM / k;
    let mut raw = input(events);
    raw.screen_rect = Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 420.0) / ppp));
    raw.time = Some(*t);
    raw.predicted_dt = 1.0 / 60.0;
    raw.viewports.entry(ViewportId::ROOT).or_default().native_pixels_per_point = Some(ppp);
    let mut out = ctx.run_ui(raw, |ui| app.ui(ui));
    out.textures_delta.clear();
}

fn tooltip_px(ctx: &egui::Context, k: f32) -> egui::Vec2 {
    let rect = ctx.memory(|m| m.layer_ids().filter(|l| l.order == egui::Order::Tooltip).find_map(|l| m.area_rect(l.id)));
    rect.expect("a tooltip").size() * (theme::UI_ZOOM / k)
}

/// A page outside the chat, drawn at both shell scales: nothing talks to Discord.
#[test]
fn sidebar_keeps_its_pixels_across_a_scale_change() {
    std::env::set_var("MYTUNE_NO_CAP", "1");
    let dir = std::env::temp_dir().join("mytune-ui-page-zoom-side");
    let _ = std::fs::create_dir_all(&dir);
    let mut app = App::with_store(mytune_core::store::Store::load(&dir));
    app.goto(Page::Customize);
    let ctx = egui::Context::default();
    theme::install_fonts(&ctx);
    theme::apply_visuals(&ctx);
    let (z, big, mut t) = (theme::UI_ZOOM, theme::UI_ZOOM, 0.0);
    for _ in 0..60 {
        frame(&mut app, &ctx, &mut t, 1.0, vec![]); // the rail folds
    }
    let icon = |ctx: &egui::Context, k: f32| ctx.read_response(egui::Id::new("search-icon")).expect("search icon").rect.top() * (z / k);
    let at_top = icon(&ctx, 1.0);

    // the tooltip has the same pixels at both scales
    let over = ctx.read_response(egui::Id::new("search-icon")).unwrap().rect.center();
    frame(&mut app, &ctx, &mut t, 1.0, vec![Event::PointerMoved(over)]);
    for _ in 0..70 {
        frame(&mut app, &ctx, &mut t, 1.0, vec![]);
    }
    let small_tip = tooltip_px(&ctx, 1.0);
    frame(&mut app, &ctx, &mut t, big, vec![Event::PointerMoved(pos2(over.x * big, over.y * big))]);
    for _ in 0..70 {
        frame(&mut app, &ctx, &mut t, big, vec![]);
    }
    let big_tip = tooltip_px(&ctx, big);
    assert!((small_tip - big_tip).length() <= 2.0, "tooltip {small_tip:?} px against {big_tip:?} px");

    // scrolled to the end, the list stays where it is in pixels, there and back
    let wheel = Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta: vec2(0.0, -60.0), phase: egui::TouchPhase::Move, source: egui::MouseWheelSource::Unknown, modifiers: egui::Modifiers::NONE };
    for _ in 0..60 {
        frame(&mut app, &ctx, &mut t, 1.0, vec![Event::PointerMoved(over), wheel.clone()]);
    }
    // the wheel overshoots the end and springs back: wait until the list rests
    let mut scrolled = icon(&ctx, 1.0);
    for _ in 0..1200 {
        frame(&mut app, &ctx, &mut t, 1.0, vec![]);
        let now = icon(&ctx, 1.0);
        let rests = now == scrolled;
        scrolled = now;
        if rests {
            break;
        }
    }
    assert!(at_top - scrolled > 20.0, "the list did not scroll: {at_top} to {scrolled}");
    for _ in 0..3 {
        frame(&mut app, &ctx, &mut t, big, vec![]);
    }
    let on_chat = icon(&ctx, big);
    // each scale snaps the list to its own pixel grid: about 1 px is rounding, the bug was 43 px
    assert!((on_chat - scrolled).abs() <= 1.5, "entering the chat moved the list from {scrolled} to {on_chat} px");
    for _ in 0..3 {
        frame(&mut app, &ctx, &mut t, 1.0, vec![]);
    }
    let back = icon(&ctx, 1.0);
    assert!((back - scrolled).abs() <= 1.5, "leaving the chat moved the list from {scrolled} to {back} px");
    println!("sidebar offset px: top {at_top}, scrolled {scrolled}, on chat {on_chat}, back {back}; tooltip {small_tip:?} / {big_tip:?}");
}

/// No chat is drawn: `page_input` only, so nothing talks to Discord.
#[test]
fn chat_page_scales_the_main_windows_input_once() {
    let dir = std::env::temp_dir().join("mytune-ui-page-zoom");
    let _ = std::fs::create_dir_all(&dir);
    let mut app = App::with_store(mytune_core::store::Store::load(&dir));
    let ctx = egui::Context::default();
    let z = theme::UI_ZOOM;
    app.goto(Page::Chat);

    // off, as in the shot tests: nothing changes
    let mut raw = input(vec![Event::PointerMoved(pos2(100.0, 50.0))]);
    app.page_input(&ctx, &mut raw);
    assert_eq!(theme::shell(), 1.0);
    assert_eq!(raw.events[0], Event::PointerMoved(pos2(100.0, 50.0)));

    app.page_zoom = true;
    let mut raw = input(vec![Event::PointerMoved(pos2(100.0, 50.0))]);
    app.page_input(&ctx, &mut raw);
    assert_eq!(theme::shell(), z);
    assert_eq!(raw.events[0], Event::PointerMoved(pos2(100.0 * z, 50.0 * z)));
    assert_eq!(raw.screen_rect.unwrap().max, pos2(1000.0 * z, 800.0 * z));
    assert_eq!(raw.viewports[&ViewportId::ROOT].native_pixels_per_point, Some(1.0 / z));

    // a hidden window's input comes back with new events behind it: only the new ones are scaled
    raw.events.push(Event::PointerMoved(pos2(10.0, 10.0)));
    app.page_input(&ctx, &mut raw);
    assert_eq!(raw.events[0], Event::PointerMoved(pos2(100.0 * z, 50.0 * z)));
    assert_eq!(raw.events[1], Event::PointerMoved(pos2(10.0 * z, 10.0 * z)));

    // every other page keeps the app's scale
    app.goto(Page::Dashboard);
    let mut raw = input(vec![Event::PointerMoved(pos2(100.0, 50.0))]);
    app.page_input(&ctx, &mut raw);
    assert_eq!(theme::shell(), 1.0);
    assert_eq!(raw.events[0], Event::PointerMoved(pos2(100.0, 50.0)));
    assert_eq!(raw.viewports[&ViewportId::ROOT].native_pixels_per_point, Some(1.0));
}
