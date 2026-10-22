//! The map view: the network mesh, the camera and its input (ADR-0003).
//!
//! All world drawing lives here, so moving it to a wgpu pipeline later
//! (ADR-0003 Decision §3) stays a local change.

use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, FontId, Galley, Key, Mesh, Pos2, Rect, Sense, Shape, Ui, Vec2, epaint,
    pos2,
};
use rusthour_api::{Network, Point};

use crate::camera::{Camera, LANE_WIDTH_M};
use crate::labels::{self, Candidate};

/// Road and intersection fill: at least 3:1 against both egui themes' panel
/// backgrounds (ADR-0003 accessibility minimum).
const ROAD_COLOR: Color32 = Color32::from_gray(0x8a);
/// Keyboard pan speed, in screen points per second.
const KEY_PAN_SPEED: f32 = 600.0;
/// Zoom per `+`/`-` press.
const KEY_ZOOM_STEP: f32 = 1.5;
/// Zoom per screen point of scrolling: about ×1.4 per native wheel notch
/// (40 points), as in web maps.
const SCROLL_ZOOM_PER_POINT: f32 = 0.0084;
/// Room left around the map by "fit": keeps it clear of the overlays at the
/// top and bottom corners.
const FIT_PADDING: Vec2 = Vec2::new(8.0, 40.0);

/// Keys bound in the map view, for the `?` help: (keys, action).
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("Drag, arrow keys, WASD", "Pan"),
    ("Scroll, pinch, + / -", "Zoom"),
    ("Home, F, 0", "Fit the map"),
    ("? / Esc", "Show or close this list"),
];

/// The network mesh plus the camera looking at it.
pub struct World {
    /// Roads and intersections in world meters, `y` negated so that
    /// [`Camera::mesh_transform`] (scale + translation) maps it to the screen.
    mesh: Mesh,
    /// World bounds of the drawn network plus a margin, `y` north.
    bounds: Rect,
    /// `None` until the first frame, and after "fit": then fit to the viewport.
    camera: Option<Camera>,
    /// Street-name label candidates, found once per map.
    pub labels: Vec<Candidate>,
    /// Laid-out label text, per candidate, made when first needed; valid for
    /// `galleys_ppp` pixels per point.
    galleys: Vec<Option<Arc<Galley>>>,
    galleys_ppp: f32,
}

impl World {
    /// Tessellates `network` once.
    pub fn new(network: &Network) -> Self {
        let mesh = build_mesh(network);
        let b = mesh.calc_bounds();
        let bounds = if b.is_finite() {
            Rect::from_min_max(pos2(b.min.x, -b.max.y), pos2(b.max.x, -b.min.y))
                .expand(2.0 * LANE_WIDTH_M)
        } else {
            // Empty network: any non-degenerate view will do.
            Rect::from_center_size(Pos2::ZERO, Vec2::splat(100.0))
        };
        let labels = labels::candidates(network);
        Self {
            mesh,
            bounds,
            camera: None,
            galleys: vec![None; labels.len()],
            galleys_ppp: 0.0,
            labels,
        }
    }

    /// Resets the view to show the whole map.
    pub fn fit(&mut self) {
        self.camera = None;
    }

    /// Draws the map into the remaining space of `ui` and handles pan/zoom.
    pub fn show(&mut self, ui: &mut Ui) {
        let (viewport, response) = ui.allocate_exact_size(ui.available_size(), Sense::drag());
        if viewport.width() < 1.0 || viewport.height() < 1.0 {
            return; // e.g. a minimized window: a zero scale would poison the camera
        }
        let padded = viewport.shrink2(FIT_PADDING);
        let fit = Camera::fit(
            self.bounds,
            if padded.is_positive() {
                padded
            } else {
                viewport
            },
        );
        let camera = self.camera.get_or_insert(fit);
        // Zoom out to "whole map fits", in to "one lane fills the screen".
        let max_scale = viewport.size().min_elem() / LANE_WIDTH_M;

        let keyboard = !ui.ctx().egui_wants_keyboard_input();
        let (pan, zoom, anchor, keys_held, fit_key) = ui.input(|i| {
            // Ctrl/Cmd/Alt combinations belong to egui and the browser (UI zoom).
            let keyboard =
                keyboard && !(i.modifiers.command || i.modifiers.ctrl || i.modifiers.alt);
            let mut pan = Vec2::ZERO;
            let mut zoom = 1.0;
            let mut anchor = viewport.center();
            if let Some(touch) = i.multi_touch() {
                pan += touch.translation_delta;
                zoom *= touch.zoom_delta;
                anchor = touch.center_pos;
            } else {
                pan += response.drag_delta();
                if response.hovered() {
                    // Ctrl/Cmd+scroll and trackpad pinch arrive as zoom_delta,
                    // plain scrolling (mouse wheel or two fingers) as scroll.
                    zoom *=
                        i.zoom_delta() * (i.smooth_scroll_delta.y * SCROLL_ZOOM_PER_POINT).exp();
                    anchor = i.pointer.hover_pos().unwrap_or(anchor);
                }
            }
            let mut keys_held = false;
            let mut fit_key = false;
            if keyboard {
                fit_key = [Key::Home, Key::F, Key::Num0]
                    .iter()
                    .any(|&k| i.key_pressed(k));
                for (keys, dir) in [
                    ([Key::ArrowLeft, Key::A], egui::vec2(1.0, 0.0)),
                    ([Key::ArrowRight, Key::D], egui::vec2(-1.0, 0.0)),
                    ([Key::ArrowUp, Key::W], egui::vec2(0.0, 1.0)),
                    ([Key::ArrowDown, Key::S], egui::vec2(0.0, -1.0)),
                ] {
                    if keys.iter().any(|&k| i.key_down(k)) {
                        pan += dir * KEY_PAN_SPEED * i.stable_dt;
                        keys_held = true;
                    }
                }
                if i.key_pressed(Key::Plus) || i.key_pressed(Key::Equals) {
                    zoom *= KEY_ZOOM_STEP;
                    anchor = viewport.center();
                }
                if i.key_pressed(Key::Minus) {
                    zoom /= KEY_ZOOM_STEP;
                    anchor = viewport.center();
                }
            }
            (pan, zoom, anchor, keys_held, fit_key)
        });
        if fit_key {
            *camera = fit;
        }
        camera.pan(pan);
        // Also re-applies the clamp after a window resize.
        camera.zoom_around(viewport, anchor, zoom, fit.scale, max_scale);
        if keys_held {
            ui.ctx().request_repaint();
        }

        // egui copies every mesh it paints and re-uploads the vertex buffer each
        // frame anyway; only ADR-0003's custom-pipeline fallback (static vertex
        // buffer, camera uniform) avoids that. Frame times on bensheim-5km
        // are measured in M1.8.
        let mut mesh = self.mesh.clone();
        mesh.transform(camera.mesh_transform(viewport));
        let painter = ui.painter_at(viewport);
        painter.add(Shape::mesh(mesh));
        let camera = *camera;
        self.draw_labels(&painter, camera, viewport, ui.visuals());
    }

    /// Street names, with a halo for contrast over roads (ADR-0003).
    fn draw_labels(
        &mut self,
        painter: &egui::Painter,
        camera: Camera,
        viewport: Rect,
        visuals: &egui::Visuals,
    ) {
        let ppp = painter.pixels_per_point();
        #[expect(clippy::float_cmp, reason = "any change invalidates the layout")]
        if ppp != self.galleys_ppp {
            self.galleys.fill(None);
            self.galleys_ppp = ppp;
        }
        let font = FontId::proportional(labels::FONT_SIZE);
        let (labels, galleys) = (&self.labels, &mut self.galleys);
        let placed = labels::place(labels, &camera, viewport, |i| {
            galleys[i]
                .get_or_insert_with(|| {
                    painter.layout_no_wrap(
                        labels[i].text.clone(),
                        font.clone(),
                        Color32::PLACEHOLDER,
                    )
                })
                .size()
        });
        let (text, halo) = (visuals.strong_text_color(), visuals.extreme_bg_color);
        for p in placed {
            let (galley, angle) = (
                galleys[p.index].clone().expect("laid out by place"),
                labels[p.index].angle,
            );
            // `pos` is the unrotated text's top-left corner; it turns about its center.
            let shape = |center: Pos2, color| {
                epaint::TextShape::new(center - galley.size() / 2.0, galley.clone(), color)
                    .with_angle_and_anchor(angle, Align2::CENTER_CENTER)
            };
            for offset in HALO_OFFSETS {
                painter.add(shape(p.center + offset, halo));
            }
            painter.add(shape(p.center, text));
        }
    }
}

/// A one-point outline around label text, drawn under it.
const HALO_OFFSETS: [Vec2; 8] = [
    Vec2::new(-1.0, -1.0),
    Vec2::new(0.0, -1.0),
    Vec2::new(1.0, -1.0),
    Vec2::new(-1.0, 0.0),
    Vec2::new(1.0, 0.0),
    Vec2::new(-1.0, 1.0),
    Vec2::new(0.0, 1.0),
    Vec2::new(1.0, 1.0),
];

/// Mesh position of a world point: `y` negated (see [`World::mesh`]).
// f32 keeps millimeter precision across a few kilometers of local frame.
#[expect(clippy::cast_possible_truncation, reason = "local frame, meters")]
fn to_mesh(p: Point) -> Pos2 {
    pos2(p.x as f32, -p.y as f32)
}

/// Tessellates roads (as thick polylines, one lane width per lane) and
/// intersections (discs as wide as the widest road there) in world meters.
pub fn build_mesh(network: &Network) -> Mesh {
    let options = epaint::TessellationOptions {
        // Feathering and pixel rounding work in screen pixels, but this mesh
        // is in meters. shortcut: edges are not anti-aliased; enable MSAA or
        // tessellate per zoom level if that looks too rough.
        feathering: false,
        prerasterized_discs: false,
        round_line_segments_to_pixels: false,
        round_rects_to_pixels: false,
        ..Default::default()
    };
    // "Pixels per point" here means per meter; 4 makes discs round enough.
    let mut tessellator = epaint::Tessellator::new(4.0, options, [1, 1], vec![]);
    let mut mesh = Mesh::default();

    let mut node_radius = vec![0.0_f32; network.nodes.len()];
    for road in &network.roads {
        let width = f32::from(road.lanes) * LANE_WIDTH_M;
        for node in [road.from_node, road.to_node] {
            let r = &mut node_radius[node.0 as usize];
            *r = r.max(width);
        }
        let line = road.geometry.iter().copied().map(to_mesh).collect();
        tessellator.tessellate_shape(Shape::line(line, (width, ROAD_COLOR)), &mut mesh);
    }
    for (node, radius) in network.nodes.iter().zip(node_radius) {
        let disc =
            Shape::circle_filled(to_mesh(node.position), radius.max(LANE_WIDTH_M), ROAD_COLOR);
        tessellator.tessellate_shape(disc, &mut mesh);
    }
    mesh
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_mesh_is_plausible() {
        use rusthour_api::{Config, InProcess, Simulation};
        let sim = InProcess::with_network(Config::default(), 0, crate::demo::map().network)
            .expect("the demo network is valid");
        let network = sim.network();
        let mesh = build_mesh(network);
        assert!(mesh.is_valid(), "indices in range");
        let segments: usize = network.roads.iter().map(|r| r.geometry.len() - 1).sum();
        // At least a quad per road segment and a few triangles per disc.
        assert!(mesh.indices.len() / 3 >= 2 * segments + 3 * network.nodes.len());
        assert!(mesh.vertices.iter().all(|v| v.pos.is_finite()));

        // Every node lies inside the mesh's bounds (with y flipped back).
        let b = mesh.calc_bounds();
        for n in &network.nodes {
            assert!(b.contains(to_mesh(n.position)));
        }
    }
}
