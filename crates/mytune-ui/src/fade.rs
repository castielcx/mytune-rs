//! Menus, popovers and modals fade out when they close, matching egui's fade-in when they open. egui drops a
//! closed area at once, so each frame the shapes of every open foreground area are kept, and an area that
//! stops drawing has its last shapes painted again, fading, for `DUR`. Paint only: a fading menu takes no
//! clicks. Costs nothing while no foreground area is open or fading.

use eframe::egui::{self, epaint::ClippedShape, Id, LayerId, Order};
use std::collections::HashMap;

const DUR: f64 = 0.16;

#[derive(Clone, Default)]
struct Ghosts {
    /// Shapes of the foreground areas drawn last frame.
    last: HashMap<LayerId, std::sync::Arc<Vec<ClippedShape>>>,
    /// Closed areas still fading: their shapes and when they closed.
    fading: Vec<(LayerId, std::sync::Arc<Vec<ClippedShape>>, f64)>,
}

/// Call once per frame after everything is drawn.
pub fn end_frame(ctx: &egui::Context) {
    // Windows (Middle) fade out too; the sidebar is the only other Middle area and never closes.
    let sidebar = LayerId::new(Order::Middle, Id::new("sidebar"));
    let layers: Vec<LayerId> = ctx.memory(|m| m.layer_ids().filter(|l| l.order == Order::Foreground || (l.order == Order::Middle && *l != sidebar)).collect());
    let now: HashMap<LayerId, std::sync::Arc<Vec<ClippedShape>>> = ctx.graphics(|g| {
        layers.iter().filter_map(|l| g.get(*l).filter(|p| !p.is_empty()).map(|p| (*l, std::sync::Arc::new(p.all_entries().cloned().collect())))).collect()
    });
    let id = Id::new("fade_ghosts");
    let mut gh = ctx.data_mut(|d| d.get_temp::<Ghosts>(id)).unwrap_or_default();
    if now.is_empty() && gh.last.is_empty() && gh.fading.is_empty() {
        return;
    }
    let t = ctx.input(|i| i.time);
    for (layer, shapes) in gh.last.drain() {
        if !now.contains_key(&layer) {
            gh.fading.push((layer, shapes, t));
        }
    }
    gh.fading.retain(|(layer, _, _)| !now.contains_key(layer)); // reopened: the live one shows
    gh.fading.retain(|(_, _, t0)| t - t0 < DUR);
    for (layer, shapes, t0) in &gh.fading {
        let a = 1.0 - ((t - t0) / DUR) as f32;
        let mut p = ctx.layer_painter(LayerId::new(Order::Foreground, layer.id.with("fade_ghost")));        p.multiply_opacity(a * a);
        for s in shapes.iter() {
            p.with_clip_rect(s.clip_rect).add(s.shape.clone());
        }
    }
    if !gh.fading.is_empty() {
        ctx.request_repaint();
    }
    gh.last = now;
    ctx.data_mut(|d| d.insert_temp(id, gh));
}
