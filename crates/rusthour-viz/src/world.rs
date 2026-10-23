//! The map view: the network mesh, the vehicles, the camera and its input
//! (ADR-0003).
//!
//! All world drawing lives here, so moving it to a wgpu pipeline later
//! (ADR-0003 Decision §3) stays a local change.

use std::sync::Arc;

use eframe::egui::{
    self, Align2, Color32, FontId, Galley, Key, Mesh, Pos2, Rect, Sense, Shape, Ui, Vec2, epaint,
    pos2,
};
use rusthour_api::{Network, Placement, Point, RoadId, VehicleSnapshot};

use crate::camera::{Camera, LANE_WIDTH_M};
use crate::labels::{self, Candidate};

/// Road and intersection fill: at least 3:1 against both egui themes' panel
/// backgrounds (ADR-0003 accessibility minimum).
const ROAD_COLOR: Color32 = Color32::from_gray(0x8a);
/// Drawn vehicle width in meters.
const VEHICLE_WIDTH_M: f32 = 1.8;
/// Smallest drawn vehicle, in screen points, so traffic stays visible when
/// zoomed out.
const MIN_VEHICLE_PT: Vec2 = Vec2::new(4.0, 3.0);
/// Vehicle fill in the dark theme: amber, apart from the gray roads in hue.
const VEHICLE_DARK: Color32 = Color32::from_rgb(0xff, 0xc8, 0x3c);
/// Vehicle fill in the light theme: a burnt orange that reads on the light
/// background where zoomed-out vehicles overhang their roads.
const VEHICLE_LIGHT: Color32 = Color32::from_rgb(0x8f, 0x35, 0x00);
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
    (
        "Drag (left, Shift + left or middle button), arrow keys, WASD",
        "Pan",
    ),
    ("Scroll, pinch, + / -", "Zoom"),
    ("Home, F, 0", "Fit the map"),
    ("Space", "Play or pause"),
    (".", "Step once (pauses)"),
    ("F3", "Show or hide frame times"),
    ("? / Esc", "Show or close this list"),
];

/// The network mesh and vehicles, plus the camera looking at them.
pub struct World {
    /// Places snapshot vehicles on the drawn roads.
    placement: Placement,
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
        let placement = Placement::new(network);
        let mesh = build_mesh(network, &placement);
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
            placement,
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

    /// Draws the map and `vehicles`, extrapolated by `ahead_s` sim seconds,
    /// into the remaining space of `ui` and handles pan/zoom.
    pub fn show(&mut self, ui: &mut Ui, vehicles: &VehicleSnapshot, ahead_s: f32) {
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
        // Left drag (also with Shift) and middle drag pan; right drag is kept
        // free for later tools. Read outside `ui.input`: `drag_delta` locks
        // the context too, and nesting the locks deadlocks.
        let drag = if response.dragged_by(egui::PointerButton::Primary)
            || response.dragged_by(egui::PointerButton::Middle)
        {
            response.drag_delta()
        } else {
            Vec2::ZERO
        };
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
                pan += drag;
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
        let color = if ui.visuals().dark_mode {
            VEHICLE_DARK
        } else {
            VEHICLE_LIGHT
        };
        painter.add(Shape::mesh(
            self.vehicle_mesh(vehicles, ahead_s, camera, viewport, color),
        ));
        self.draw_labels(&painter, camera, viewport, ui.visuals());
    }

    /// One mesh for all vehicles on screen, in screen points.
    fn vehicle_mesh(
        &self,
        vehicles: &VehicleSnapshot,
        ahead_s: f32,
        camera: Camera,
        viewport: Rect,
        color: Color32,
    ) -> Mesh {
        let to_screen = camera.mesh_transform(viewport);
        let width = (VEHICLE_WIDTH_M * camera.scale).max(MIN_VEHICLE_PT.y);
        let mut mesh = Mesh::default();
        mesh.reserve_vertices(4 * vehicles.len());
        mesh.reserve_triangles(2 * vehicles.len());
        for i in 0..vehicles.len() {
            let (front, dir) = vehicle_pose(&self.placement, vehicles, i, ahead_s);
            let front = to_screen * front;
            let length = (vehicles.length_m[i] * camera.scale).max(MIN_VEHICLE_PT.x);
            if !viewport.expand(length).contains(front) {
                continue;
            }
            #[expect(clippy::cast_possible_truncation, reason = "far below u32::MAX")]
            let base = mesh.vertices.len() as u32;
            for corner in vehicle_quad(front, dir, length, width) {
                mesh.colored_vertex(corner, color);
            }
            mesh.add_triangle(base, base + 1, base + 2);
            mesh.add_triangle(base, base + 2, base + 3);
        }
        mesh
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

/// Front bumper position and unit heading of vehicle `i` in mesh space (`y`
/// down), moved `ahead_s` seconds along its road and clamped to it.
fn vehicle_pose(
    placement: &Placement,
    vehicles: &VehicleSnapshot,
    i: usize,
    ahead_s: f32,
) -> (Pos2, Vec2) {
    let offset = vehicles.offset_m[i] + vehicles.speed_mps[i] * ahead_s;
    let (front, [hx, hy]) =
        placement.place(vehicles.road[i], vehicles.lane[i], offset, LANE_WIDTH_M);
    (to_mesh(front), Vec2::new(hx, -hy))
}

/// Corners of a vehicle `length` long behind `front` along the unit `dir`
/// and `width` wide, in drawing order, in any space where `dir` is unit.
// shortcut: the body is straight, so on a sharp bend its rear leaves the lane
// by a few decimeters; place the rear bumper too if that shows.
fn vehicle_quad(front: Pos2, dir: Vec2, length: f32, width: f32) -> [Pos2; 4] {
    let side = dir.rot90() * (width / 2.0);
    let rear = front - dir * length;
    [front + side, front - side, rear - side, rear + side]
}

/// `points` moved `d` meters to the right of their direction, keeping that
/// distance from each segment at corners (`y` north).
fn shifted(points: &[Point], dist: f64) -> Vec<Point> {
    let right = |p0: Point, p1: Point| {
        let len = (p1.x - p0.x).hypot(p1.y - p0.y);
        ((p1.y - p0.y) / len, -(p1.x - p0.x) / len)
    };
    let segments: Vec<_> = points.windows(2).map(|w| right(w[0], w[1])).collect();
    points
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let before = segments[i.saturating_sub(1)];
            let after = segments[i.min(segments.len() - 1)];
            let (mx, my) = (before.0 + after.0, before.1 + after.1);
            let sum = mx.hypot(my);
            if sum < 1e-6 {
                // A full reversal: no corner direction; keep the first side.
                return Point {
                    x: p.x + before.0 * dist,
                    y: p.y + before.1 * dist,
                };
            }
            // Miter length dist / cos(half the turn), limited at hairpins.
            let k = dist / sum * (2.0 / sum.max(1.0));
            Point {
                x: p.x + mx * k,
                y: p.y + my * k,
            }
        })
        .collect()
}

/// Mesh position of a world point: `y` negated (see [`World::mesh`]).
// f32 keeps millimeter precision across a few kilometers of local frame.
#[expect(clippy::cast_possible_truncation, reason = "local frame, meters")]
fn to_mesh(p: Point) -> Pos2 {
    pos2(p.x as f32, -p.y as f32)
}

/// Tessellates roads (as thick polylines, one lane width per lane) and
/// intersections (discs as wide as the widest road there) in world meters.
/// Roads with a twin are drawn right of their shared line, as `placement`
/// puts their lanes (ADR-0004 §3).
pub fn build_mesh(network: &Network, placement: &Placement) -> Mesh {
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
    for (i, road) in network.roads.iter().enumerate() {
        let width = f32::from(road.lanes) * LANE_WIDTH_M;
        for node in [road.from_node, road.to_node] {
            let r = &mut node_radius[node.0 as usize];
            *r = r.max(width);
        }
        let id = RoadId(u32::try_from(i).expect("road ids are u32"));
        let line = if placement.has_twin(id) {
            shifted(&road.geometry, f64::from(width) / 2.0)
        } else {
            road.geometry.clone()
        };
        let line = line.into_iter().map(to_mesh).collect();
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
    use egui::{Event, Modifiers, PointerButton};
    use rusthour_api::RoadId;

    /// Runs one frame of `world` with `events`.
    fn frame(ctx: &egui::Context, world: &mut World, events: Vec<Event>) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| world.show(ui, &VehicleSnapshot::default(), 0.0));
        out.textures_delta.clear();
    }

    /// World meters the view center moves when dragging 80 points right.
    fn drag(button: PointerButton, modifiers: Modifiers) -> f32 {
        let ctx = egui::Context::default();
        let mut world = World::new(&crate::demo::map().network);
        let p = pos2(400.0, 300.0);
        frame(&ctx, &mut world, vec![Event::PointerMoved(p)]);
        let before = world.camera.expect("set on the first frame").center;
        let press = Event::PointerButton {
            pos: p,
            button,
            pressed: true,
            modifiers,
        };
        frame(&ctx, &mut world, vec![press]);
        for dx in [40.0, 80.0] {
            let moved = Event::PointerMoved(p + egui::vec2(dx, 0.0));
            frame(&ctx, &mut world, vec![moved]);
        }
        world.camera.expect("still set").center.x - before.x
    }

    /// Dragging used to deadlock: `drag_delta` was read inside `ui.input`.
    #[test]
    fn regression_drag_deadlock() {
        // Dragging right moves the view west.
        assert!(drag(PointerButton::Primary, Modifiers::NONE) < -1.0);
    }

    #[test]
    fn shift_left_and_middle_drag_pan_but_right_does_not() {
        for (button, modifiers) in [
            (PointerButton::Primary, Modifiers::SHIFT),
            (PointerButton::Middle, Modifiers::NONE),
        ] {
            // Dragging right moves the view west.
            assert!(drag(button, modifiers) < -1.0, "{button:?} {modifiers:?}");
        }
        assert!(drag(PointerButton::Secondary, Modifiers::NONE).abs() < f32::EPSILON);
    }

    fn east_west(lanes: u8, twin: bool) -> Network {
        let p = |x| Point { x, y: 0.0 };
        let node = |x| rusthour_api::Node {
            position: p(x),
            osm_node: None,
        };
        let road = |from, to, geometry| rusthour_api::Road {
            from_node: rusthour_api::NodeId(from),
            to_node: rusthour_api::NodeId(to),
            geometry,
            speed_limit_mps: 13.9,
            lanes,
            name: None,
            osm: None,
            roundabout: false,
        };
        let mut roads = vec![road(0, 1, vec![p(0.0), p(100.0)])];
        if twin {
            roads.push(road(1, 0, vec![p(100.0), p(0.0)]));
        }
        Network {
            nodes: vec![node(0.0), node(100.0)],
            roads,
            connections: vec![],
        }
    }

    /// The vehicle body (mesh space, `y` down) for one vehicle on road 0.
    fn body(net: &Network, lane: u8, offset_m: f32, ahead_s: f32) -> [Pos2; 4] {
        let snap = VehicleSnapshot {
            id: vec![rusthour_api::VehicleId(0)],
            road: vec![RoadId(0)],
            lane: vec![lane],
            offset_m: vec![offset_m],
            speed_mps: vec![10.0],
            length_m: vec![4.5],
            ..VehicleSnapshot::default()
        };
        let (front, dir) = vehicle_pose(&Placement::new(net), &snap, 0, ahead_s);
        vehicle_quad(front, dir, 4.5, VEHICLE_WIDTH_M)
    }

    #[test]
    fn vehicles_stay_on_their_lane_of_the_drawn_road() {
        // East-bound, so the drawn road and lane are bands in y (mesh `y` = south).
        // (lanes, twin, lane, band of the lane in mesh y)
        for (lanes, twin, lane, band) in [
            (1, false, 0, -1.75..=1.75),
            (2, false, 0, 0.0..=3.5), // rightmost = south
            (2, false, 1, -3.5..=0.0),
            (1, true, 0, 0.0..=3.5), // twin: right of the shared line
            (2, true, 1, 0.0..=3.5),
            (2, true, 0, 3.5..=7.0),
        ] {
            let net = east_west(lanes, twin);
            for (offset, ahead) in [(4.5, 0.0), (50.0, 0.05), (99.0, 0.09), (100.0, 0.09)] {
                for c in body(&net, lane, offset, ahead) {
                    let what =
                        format!("{lanes} lanes, twin {twin}, lane {lane}, {offset} m: {c:?}");
                    assert!(band.contains(&c.y), "{what}");
                    // Extrapolation is clamped to the road end.
                    assert!((0.0..=100.0 + 1e-3).contains(&c.x), "{what}");
                }
            }
        }
        // The front bumper leads, east.
        let [front, ..] = body(&east_west(1, false), 0, 50.0, 0.0);
        assert!((front.x - 50.0).abs() < 1e-3);
    }

    #[test]
    fn twin_roads_are_drawn_right_of_their_shared_line() {
        let p = |x, y| Point { x, y };
        // East then north: the shifted line keeps its distance at the corner.
        let s = shifted(&[p(0.0, 0.0), p(10.0, 0.0), p(10.0, 10.0)], 1.0);
        let close = |a: Point, b: Point| (a.x - b.x).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9;
        assert!(close(s[0], p(0.0, -1.0)), "{s:?}");
        assert!(close(s[1], p(11.0, -1.0)), "{s:?}");
        assert!(close(s[2], p(11.0, 10.0)), "{s:?}");
    }

    #[test]
    fn demo_mesh_is_plausible() {
        use rusthour_api::{Config, InProcess, Simulation};
        let sim = InProcess::with_network(Config::default(), 0, crate::demo::map().network)
            .expect("the demo network is valid");
        let network = sim.network();
        let mesh = build_mesh(network, &Placement::new(network));
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
