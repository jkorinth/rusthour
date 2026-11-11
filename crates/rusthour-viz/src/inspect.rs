//! Select a vehicle or road, inspect it and follow it with the camera
//! (ADR-0005, addendum M3.11). Picking is on the CPU against the drawn
//! positions, with no egui input types (ADR-0003 §4).

use eframe::egui::{self, Align2, Key, Pos2, Rect, Shape, Stroke, Vec2, pos2};
use rusthour_api::{
    Gone, Network, Placement, Point, RoadClass, RoadId, RoadInfo, Simulation, StopCause, TurnType,
    VehicleId, VehicleInfo, VehicleSnapshot, WaitReason,
};

use crate::camera::{Camera, LANE_WIDTH_M};
use crate::traffic::Traffic;
use crate::world::{MIN_VEHICLE_PT, VEHICLE_WIDTH_M, shifted, to_mesh, vehicle_pose};

/// Pick radius for a mouse click, in screen points.
pub const MOUSE_RADIUS_PT: f32 = 8.0;
/// Pick radius for a tap, in screen points.
pub const TOUCH_RADIUS_PT: f32 = 22.0;
/// The panel text refreshes this often (s): about 5 Hz.
const REFRESH_S: f64 = 0.2;
/// Above this time scale, following shows a hint: the view jumps.
pub const FOLLOW_HINT_ABOVE: f64 = 30.0;
/// A pan by more than this per frame stops following, in screen points.
const FOLLOW_BREAK_PT: f32 = 2.0;
/// Below this speed a following vehicle reads as standing in a queue (m/s).
const STANDING_MPS: f32 = 0.1;
/// Panel width on wide screens, in points.
const PANEL_WIDTH_PT: f32 = 280.0;
/// Narrower screens get a bottom sheet: the control strip (about 560 pt) takes the top.
const NARROW_PT: f32 = 880.0;

/// What the user selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Vehicle(VehicleId),
    Road(RoadId),
}

/// What the map view hands to picking and drawing, as drawn this frame.
pub struct View<'a> {
    pub placement: &'a Placement,
    pub network: &'a Network,
    /// Each road's drawn center line (world, `y` north), twins shifted.
    pub lines: &'a [Vec<Point>],
    pub vehicles: &'a VehicleSnapshot,
    pub ahead_s: f32,
    pub camera: Camera,
    pub viewport: Rect,
}

impl View<'_> {
    fn to_screen(&self, p: Point) -> Pos2 {
        self.camera.mesh_transform(self.viewport) * to_mesh(p)
    }

    /// Screen front bumper, unit heading and length of snapshot vehicle `i`.
    fn vehicle(&self, i: usize) -> (Pos2, Vec2, f32) {
        let (front, dir) = vehicle_pose(self.placement, self.vehicles, i, self.ahead_s);
        let front = self.camera.mesh_transform(self.viewport) * front;
        let length = (self.vehicles.length_m[i] * self.camera.scale).max(MIN_VEHICLE_PT.x);
        (front, dir, length)
    }
}

/// The selection, its last inspection, and whether the camera follows it.
#[derive(Default)]
pub struct Inspector {
    pub selected: Option<Selection>,
    pub following: bool,
    collapsed: bool,
    /// The last description of the selected vehicle while it was alive.
    info: Option<VehicleInfo>,
    /// Set once the selected vehicle left; it is not asked again, so the
    /// panel never jumps to the vehicle that reuses its slot.
    gone: Option<Gone>,
    road: Option<RoadInfo>,
    refreshed_s: Option<f64>,
    /// [`Traffic::epoch`] the selection belongs to.
    epoch: u32,
    /// Where the last tap picked something, and which candidate it took.
    last_tap: Option<(Pos2, usize)>,
}

impl Inspector {
    /// Selects `s`; a new selection stops following and is inspected at once.
    pub fn select(&mut self, s: Option<Selection>) {
        if s != self.selected {
            *self = Self {
                selected: s,
                collapsed: self.collapsed,
                epoch: self.epoch,
                last_tap: self.last_tap,
                ..Self::default()
            };
        }
    }

    /// Esc: closes the help first, then clears the selection.
    pub fn escape(&mut self, show_help: &mut bool) {
        if *show_help {
            *show_help = false;
        } else {
            self.select(None);
        }
    }

    /// Follow on or off, for a vehicle still on the map.
    pub fn toggle_follow(&mut self) {
        let live = matches!(self.selected, Some(Selection::Vehicle(_))) && self.gone.is_none();
        self.following = live && !self.following;
    }

    /// `C` toggles follow; Esc closes the help or clears the selection.
    pub fn keys(&mut self, ctx: &egui::Context, show_help: &mut bool) {
        // Esc in a text field only leaves the field.
        if ctx.input(|i| i.key_pressed(Key::Escape))
            && (*show_help || !ctx.egui_wants_keyboard_input())
        {
            self.escape(show_help);
        }
        if !ctx.egui_wants_keyboard_input()
            && ctx.input(|i| !i.modifiers.command && !i.modifiers.alt && i.key_pressed(Key::C))
        {
            self.toggle_follow();
        }
    }

    /// A click or tap at `click` (screen) without movement: selects the
    /// nearest candidate, or the next one on a repeated tap on the same spot.
    pub fn tap(&mut self, view: &View, click: Pos2, touch: bool) {
        let radius = if touch {
            TOUCH_RADIUS_PT
        } else {
            MOUSE_RADIUS_PT
        };
        let found = candidates(view, click, radius);
        let i = match self.last_tap {
            Some((p, i)) if p.distance(click) <= radius / 2.0 && !found.is_empty() => {
                (i + 1) % found.len()
            }
            _ => 0,
        };
        self.last_tap = (!found.is_empty()).then_some((click, i));
        self.select(found.get(i).copied());
    }

    /// The selected vehicle's drawn position (world, `y` north), if on the map.
    pub fn target(&self, view: &View) -> Option<Pos2> {
        let Some(Selection::Vehicle(id)) = self.selected else {
            return None;
        };
        let i = view.vehicles.id.iter().position(|&v| v == id)?;
        let (front, _) = vehicle_pose(view.placement, view.vehicles, i, view.ahead_s);
        Some(pos2(front.x, -front.y))
    }

    /// Where the camera centers this frame, if following `target`; a manual
    /// pan or "fit" stops following.
    pub fn follow(&mut self, pan: Vec2, fit: bool, target: Option<Pos2>) -> Option<Pos2> {
        // A pinch's own small drift is not a pan, so zooming keeps following.
        if pan.length() > FOLLOW_BREAK_PT || fit || target.is_none() {
            self.following = false;
        }
        target.filter(|_| self.following)
    }

    /// Drops a selection from before a reset, and asks the simulation
    /// again about every [`REFRESH_S`].
    pub fn update(&mut self, t: &Traffic, now_s: f64) {
        if t.epoch != self.epoch {
            // Slots name other vehicles after a restart.
            self.select(None);
            self.last_tap = None;
            self.epoch = t.epoch;
        }
        if self.refreshed_s.is_some_and(|r| now_s - r < REFRESH_S) {
            return;
        }
        self.refreshed_s = Some(now_s);
        match self.selected {
            Some(Selection::Vehicle(id)) if self.gone.is_none() => {
                match t.sim().inspect_vehicle(id) {
                    Ok(info) => self.info = Some(info),
                    Err(g) => {
                        self.gone = Some(g);
                        self.following = false;
                    }
                }
            }
            Some(Selection::Road(r)) => self.road = t.sim().inspect_road(r),
            _ => {}
        }
    }

    /// Outlines the selection; for a vehicle also its route ahead, or the
    /// marker where it was last seen.
    pub fn draw(&self, painter: &egui::Painter, view: &View, visuals: &egui::Visuals) {
        let ink = visuals.strong_text_color();
        let outline = Stroke::new(2.0, ink);
        match self.selected {
            Some(Selection::Road(r)) => {
                let line = &view.lines[r.0 as usize];
                let half = f32::from(view.network.roads[r.0 as usize].lanes) * LANE_WIDTH_M / 2.0
                    + 2.0 / view.camera.scale;
                for side in [half, -half] {
                    let edge = shifted(line, f64::from(side));
                    let points = edge.into_iter().map(|p| view.to_screen(p)).collect();
                    painter.add(Shape::line(points, outline));
                }
            }
            Some(Selection::Vehicle(id)) => {
                if let Some(i) = view.vehicles.id.iter().position(|&v| v == id) {
                    let (front, dir, length) = view.vehicle(i);
                    if let Some(info) = &self.info {
                        draw_route(painter, view, i, info, outline);
                    }
                    let width = (VEHICLE_WIDTH_M * view.camera.scale).max(MIN_VEHICLE_PT.y);
                    let body = crate::world::vehicle_quad(
                        front + dir * 3.0,
                        dir,
                        length + 6.0,
                        width + 6.0,
                    );
                    painter.add(Shape::closed_line(body.to_vec(), outline));
                } else if let Some(at) = self.last_seen(view) {
                    let at = view.to_screen(at);
                    painter.circle_filled(at, 6.0, visuals.warn_fg_color);
                    painter.circle_stroke(at, 8.0, outline);
                }
            }
            None => {}
        }
    }

    /// Where a gone vehicle was last seen: its exit record, else its last
    /// description.
    fn last_seen(&self, view: &View) -> Option<Point> {
        let lane_on = |road| {
            self.info
                .as_ref()
                .filter(|i| i.road == road)
                .map_or(0, |i| i.lane)
        };
        let (road, lane, offset) = match self.gone? {
            Gone::Arrived { road, offset_m } | Gone::RemovedStuck { road, offset_m } => {
                (road, lane_on(road), offset_m)
            }
            _ => {
                let i = self.info.as_ref()?;
                (i.road, i.lane, i.offset_m)
            }
        };
        Some(view.placement.place(road, lane, offset, LANE_WIDTH_M).0)
    }

    /// The panel: top right on wide screens, a bottom sheet on narrow ones.
    pub fn show_panel(&mut self, ctx: &egui::Context, network: &Network) {
        let Some(selected) = self.selected else {
            return;
        };
        let (title, lines) = match selected {
            Selection::Vehicle(_) => {
                let title = if self.following {
                    "Vehicle · following"
                } else {
                    "Vehicle"
                };
                let lines = match (self.gone, &self.info) {
                    (Some(g), info) => vec![gone_text(g, info.as_ref(), network)],
                    (None, Some(info)) => vehicle_lines(info, network),
                    (None, None) => vec![],
                };
                (title.to_owned(), lines)
            }
            Selection::Road(r) => (
                road_name(network, r),
                self.road
                    .as_ref()
                    .map(|info| road_lines(info, network))
                    .unwrap_or_default(),
            ),
        };
        let screen = ctx.content_rect();
        let narrow = screen.width() < NARROW_PT;
        let area = egui::Area::new(egui::Id::unique("inspect_panel"));
        let area = if narrow {
            area.anchor(Align2::CENTER_BOTTOM, [0.0, -36.0])
        } else {
            area.anchor(Align2::RIGHT_TOP, [-8.0, 8.0])
        };
        let mut close = false;
        area.show(ctx, |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                let width = if narrow {
                    screen.width() - 32.0
                } else {
                    PANEL_WIDTH_PT
                };
                ui.set_width(width);
                ui.horizontal(|ui| {
                    let (arrow, tip) = if self.collapsed {
                        ("▸", "Show details")
                    } else {
                        ("▾", "Hide details")
                    };
                    if ui.button(arrow).on_hover_text(tip).clicked() {
                        self.collapsed = !self.collapsed;
                    }
                    ui.strong(&title);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        close = ui
                            .button("✕")
                            .on_hover_text("Clear the selection (Esc)")
                            .clicked();
                        if matches!(selected, Selection::Vehicle(_)) && self.gone.is_none() {
                            let follow = ui
                                .selectable_label(self.following, "Follow")
                                .on_hover_text("Keep the camera on this vehicle (C)");
                            if follow.clicked() {
                                self.toggle_follow();
                            }
                        }
                    });
                });
                if self.collapsed {
                    if narrow && let Some(first) = lines.first() {
                        ui.label(first);
                    }
                    return;
                }
                let max_height = if narrow {
                    (0.4 * screen.height() - 40.0).max(40.0)
                } else {
                    0.6 * screen.height()
                };
                egui::ScrollArea::vertical()
                    .max_height(max_height)
                    .show(ui, |ui| {
                        for line in &lines {
                            ui.label(line);
                        }
                    });
            });
        });
        if close {
            self.select(None);
        }
    }
}

/// Everything within `radius` points of `click`, nearest first: vehicles,
/// or roads if no vehicle is that close.
pub fn candidates(view: &View, click: Pos2, radius: f32) -> Vec<Selection> {
    let scale = view.camera.scale;
    let half_width = (VEHICLE_WIDTH_M * scale).max(MIN_VEHICLE_PT.y) / 2.0;
    let mut hits: Vec<(f32, Selection)> = (0..view.vehicles.len())
        .filter_map(|i| {
            let (front, dir, length) = view.vehicle(i);
            let d = segment_distance(click, front, front - dir * length) - half_width;
            (d <= radius).then_some((d, Selection::Vehicle(view.vehicles.id[i])))
        })
        .collect();
    if hits.is_empty() {
        for (i, line) in view.lines.iter().enumerate() {
            let half = f32::from(view.network.roads[i].lanes) * LANE_WIDTH_M * scale / 2.0;
            let d = line
                .windows(2)
                .map(|w| segment_distance(click, view.to_screen(w[0]), view.to_screen(w[1])))
                .fold(f32::INFINITY, f32::min)
                - half;
            if d <= radius {
                let id = RoadId(u32::try_from(i).expect("road ids are u32"));
                hits.push((d, Selection::Road(id)));
            }
        }
    }
    hits.sort_by(|a, b| a.0.total_cmp(&b.0));
    hits.into_iter().map(|(_, s)| s).collect()
}

fn segment_distance(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 {
        ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    p.distance(a + ab * t)
}

/// A dashed line from vehicle `i` along its route ahead, and a destination marker.
fn draw_route(painter: &egui::Painter, view: &View, i: usize, info: &VehicleInfo, stroke: Stroke) {
    // The description can be a refresh old: start on the road the vehicle is drawn on.
    let road = view.vehicles.road[i];
    let Some(k) = info.route_ahead.iter().position(|&r| r == road) else {
        return;
    };
    let (front, _, _) = view.vehicle(i);
    let offset = view.vehicles.offset_m[i] + view.vehicles.speed_mps[i] * view.ahead_s;
    let mut path = vec![front];
    let current = &view.lines[road.0 as usize];
    let mut along = 0.0;
    for w in current.windows(2) {
        along += (w[1].x - w[0].x).hypot(w[1].y - w[0].y);
        if along > f64::from(offset) {
            path.push(view.to_screen(w[1]));
        }
    }
    for r in &info.route_ahead[k + 1..] {
        path.extend(view.lines[r.0 as usize].iter().map(|&p| view.to_screen(p)));
    }
    let end = *path.last().expect("starts at the vehicle");
    painter.extend(Shape::dashed_line(&path, stroke, 6.0, 4.0));
    painter.circle_filled(end, 5.0, stroke.color);
    painter.circle_stroke(end, 9.0, stroke);
}

fn road_name(network: &Network, road: RoadId) -> String {
    network
        .roads
        .get(road.0 as usize)
        .and_then(|r| r.name.clone())
        .unwrap_or_else(|| format!("road {}", road.0))
}

/// Why the vehicle drives as it does, in plain words.
// shortcut: no in-0.1/out-0.05 hysteresis on "following", the api reports the
// core's 0.1 cut; the 5 Hz refresh limits flicker. Add it if trials see flicker.
pub fn wait_text(info: &VehicleInfo, network: &Network) -> String {
    match info.wait {
        WaitReason::FreeRoad => "driving freely".into(),
        WaitReason::Following { .. } if info.speed_mps < STANDING_MPS => "stuck in a queue".into(),
        WaitReason::Following { gap_m, .. } => format!("following traffic, {gap_m:.0} m gap"),
        WaitReason::StopLine { cause } => match cause {
            StopCause::Junction => "waiting at a junction",
            StopCause::Yield => "giving way",
            StopCause::Stop => "stopped at a stop sign",
            // shortcut: no seconds left yet; add them from the signal snapshot with M3.4.
            StopCause::Red => "red light",
            StopCause::WrongLane => "in the wrong lane for its turn, waiting for a gap",
            _ => "waiting at the stop line",
        }
        .into(),
        WaitReason::NoRoom { next_road } => format!(
            "waiting for space on {} ahead",
            road_name(network, next_road)
        ),
        _ => "waiting".into(),
    }
}

/// What became of a vehicle that left; `last` is its last description.
pub fn gone_text(gone: Gone, last: Option<&VehicleInfo>, network: &Network) -> String {
    match gone {
        Gone::Arrived { road, .. } => {
            format!("Reached its destination on {}.", road_name(network, road))
        }
        Gone::RemovedStuck { road, .. } => {
            let limit = last
                .map(|i| i.stuck_limit_s)
                .filter(|&s| s > 0.0)
                .map_or_else(|| "too long".to_owned(), |s| format!("{s:.0} s"));
            format!(
                "Removed after standing still {limit} (gridlock safety net) on {}.",
                road_name(network, road)
            )
        }
        _ => "No longer on the map.".into(),
    }
}

/// The vehicle panel's lines.
pub fn vehicle_lines(info: &VehicleInfo, network: &Network) -> Vec<String> {
    let lanes = network
        .roads
        .get(info.road.0 as usize)
        .map_or(1, |r| r.lanes);
    let trip = info.trip_time_s.max(0.0);
    let mut out = vec![
        format!(
            "{:.0} km/h, {}",
            info.speed_mps * 3.6,
            wait_text(info, network)
        ),
        format!(
            "on {}, lane {} of {lanes}",
            road_name(network, info.road),
            info.lane + 1
        ),
    ];
    if let Some(&dest) = info.route_ahead.last() {
        out.push(format!("going to {}", road_name(network, dest)));
    }
    out.push(format!(
        "trip time {}:{:02}",
        (trip / 60.0).floor(),
        (trip % 60.0).floor()
    ));
    if info.stuck_limit_s > 0.0 && info.standing_s > 0.5 * info.stuck_limit_s {
        out.push(format!(
            "standing {:.0} s of {:.0} s before removal",
            info.standing_s, info.stuck_limit_s
        ));
    }
    let d = info.driver;
    out.push(format!(
        "driver: wants {:.0}% of the limit, headway {:.1} s, accel {:.1} m/s², \
         braking {:.1} m/s², min gap {:.1} m",
        d.desired_speed_factor * 100.0,
        d.time_headway_s,
        d.max_accel_mps2,
        d.comfort_decel_mps2,
        d.min_gap_m
    ));
    out
}

fn turn_text(t: TurnType) -> &'static str {
    match t {
        TurnType::Straight => "straight",
        TurnType::Right => "right",
        TurnType::Left | TurnType::LeftAcross => "left",
        TurnType::UTurn => "U-turn",
        _ => "turn",
    }
}

fn class_text(c: RoadClass) -> &'static str {
    match c {
        RoadClass::Motorway => "motorway, ",
        RoadClass::Trunk => "trunk road, ",
        RoadClass::Primary => "primary road, ",
        RoadClass::Secondary => "secondary road, ",
        RoadClass::Tertiary => "tertiary road, ",
        RoadClass::Unclassified => "minor road, ",
        RoadClass::Residential => "residential street, ",
        RoadClass::LivingStreet => "living street, ",
        RoadClass::Unknown => "",
    }
}

/// The road panel's lines; the class comes from the map.
pub fn road_lines(info: &RoadInfo, network: &Network) -> Vec<String> {
    let class = network
        .roads
        .get(info.road.0 as usize)
        .map_or(RoadClass::Unknown, |r| r.class);
    let mut lines = vec![
        format!(
            "{}{} lane{}, limit {:.0} km/h, {:.0} m",
            class_text(class),
            info.lanes,
            if info.lanes == 1 { "" } else { "s" },
            info.speed_limit_mps * 3.6,
            info.length_m
        ),
        match info.mean_speed_mps {
            Some(v) => format!("{} vehicles, mean speed {:.0} km/h", info.vehicles, v * 3.6),
            None => "no vehicles".into(),
        },
        format!("{} removed (stuck)", info.removed_stuck),
    ];
    for m in &info.movements {
        lines.push(format!(
            "→ {} ({}): {:.0} s, expected {:.0} s",
            road_name(network, m.to_road),
            turn_text(m.turn),
            m.delay_s,
            m.prior_s
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::World;
    use rusthour_api::{DriverInfo, MovementInfo, Node, NodeId, Road};

    /// One road of `lanes` lanes, 100 m east from the origin, named "Hauptstraße".
    fn east(lanes: u8) -> Network {
        let p = |x| Point { x, y: 0.0 };
        Network {
            nodes: vec![
                Node {
                    position: p(0.0),
                    osm_node: None,
                },
                Node {
                    position: p(100.0),
                    osm_node: None,
                },
            ],
            roads: vec![Road {
                from_node: NodeId(0),
                to_node: NodeId(1),
                geometry: vec![p(0.0), p(100.0)],
                speed_limit_mps: 13.9,
                lanes,
                name: Some("Hauptstraße".into()),
                osm: None,
                roundabout: false,
                class: RoadClass::Residential,
                priority_road: false,
                end_control: rusthour_api::EndControl::None,
                turn_lanes: vec![],
            }],
            connections: vec![],
        }
    }

    /// Vehicles on road 0, `(lane, offset)` each, standing.
    fn snapshot(at: &[(u8, f32)]) -> VehicleSnapshot {
        VehicleSnapshot {
            id: (0..at.len() as u64).map(VehicleId).collect(),
            road: vec![RoadId(0); at.len()],
            lane: at.iter().map(|a| a.0).collect(),
            offset_m: at.iter().map(|a| a.1).collect(),
            speed_mps: vec![0.0; at.len()],
            length_m: vec![4.5; at.len()],
            ..VehicleSnapshot::default()
        }
    }

    /// Picks at world point `(x, y)` (meters, `y` north) at 1 point per meter,
    /// the camera on the origin.
    fn pick(net: &Network, snap: &VehicleSnapshot, x: f32, y: f32, radius: f32) -> Vec<Selection> {
        let placement = Placement::new(net);
        let lines: Vec<_> = net.roads.iter().map(|r| r.geometry.clone()).collect();
        let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 400.0));
        let view = View {
            placement: &placement,
            network: net,
            lines: &lines,
            vehicles: snap,
            ahead_s: 0.0,
            camera: Camera {
                center: Pos2::ZERO,
                scale: 1.0,
            },
            viewport,
        };
        candidates(&view, viewport.center() + egui::vec2(x, -y), radius)
    }

    #[test]
    fn the_nearest_vehicle_in_the_radius_beats_the_road() {
        let net = east(1);
        let snap = snapshot(&[(0, 50.0), (0, 60.0)]);
        // On the second vehicle's body: both within the touch radius, the nearer first.
        assert_eq!(
            pick(&net, &snap, 58.0, 0.0, TOUCH_RADIUS_PT),
            [
                Selection::Vehicle(VehicleId(1)),
                Selection::Vehicle(VehicleId(0))
            ]
        );
        // Far from any vehicle, still on the road.
        assert_eq!(
            pick(&net, &snap, 10.0, 0.0, MOUSE_RADIUS_PT),
            [Selection::Road(RoadId(0))]
        );
        // 15 points beside a vehicle: only a tap reaches it.
        assert_eq!(
            pick(&net, &snap, 48.0, 15.0, TOUCH_RADIUS_PT)[0],
            Selection::Vehicle(VehicleId(0))
        );
        assert!(pick(&net, &snap, 48.0, 15.0, MOUSE_RADIUS_PT).is_empty());
    }

    #[test]
    fn a_repeated_tap_cycles_through_the_candidates() {
        let net = east(2);
        // Side by side; the tap, just south of the line, is nearer lane 0 (vehicle 1).
        let snap = snapshot(&[(1, 50.0), (0, 50.0)]);
        let placement = Placement::new(&net);
        let lines: Vec<_> = net.roads.iter().map(|r| r.geometry.clone()).collect();
        let viewport = Rect::from_min_size(Pos2::ZERO, egui::vec2(400.0, 400.0));
        let view = View {
            placement: &placement,
            network: &net,
            lines: &lines,
            vehicles: &snap,
            ahead_s: 0.0,
            camera: Camera {
                center: Pos2::ZERO,
                scale: 1.0,
            },
            viewport,
        };
        let at = viewport.center() + egui::vec2(48.0, 1.0);
        let mut ins = Inspector::default();
        let mut picked = vec![];
        for _ in 0..3 {
            ins.tap(&view, at, true);
            picked.push(ins.selected);
        }
        let (a, b) = (
            Some(Selection::Vehicle(VehicleId(1))),
            Some(Selection::Vehicle(VehicleId(0))),
        );
        assert_eq!(picked, [a, b, a]);
        // A tap elsewhere starts again from the nearest; one on empty map clears.
        ins.tap(&view, at + egui::vec2(0.0, -13.0), true);
        assert_eq!(ins.selected, b);
        ins.tap(&view, viewport.center() + egui::vec2(-150.0, 150.0), true);
        assert_eq!(ins.selected, None);
    }

    fn info(wait: WaitReason, speed_mps: f32) -> VehicleInfo {
        VehicleInfo {
            id: VehicleId(0),
            road: RoadId(0),
            lane: 0,
            offset_m: 50.0,
            speed_mps,
            accel_mps2: 0.0,
            trip_time_s: 75.0,
            route_ahead: vec![RoadId(0)],
            driver: DriverInfo {
                desired_speed_factor: 1.0,
                time_headway_s: 1.5,
                max_accel_mps2: 1.4,
                comfort_decel_mps2: 2.0,
                min_gap_m: 2.0,
            },
            wait,
            standing_s: 200.0,
            stuck_limit_s: 300.0,
        }
    }

    #[test]
    fn wait_reasons_and_departures_read_as_plain_words() {
        let net = east(1);
        let following = WaitReason::Following {
            leader: VehicleId(1),
            gap_m: 12.2,
        };
        for (wait, speed, text) in [
            (WaitReason::FreeRoad, 10.0, "driving freely"),
            (following, 5.0, "following traffic, 12 m gap"),
            (following, 0.0, "stuck in a queue"),
            (
                WaitReason::StopLine {
                    cause: StopCause::Junction,
                },
                0.0,
                "waiting at a junction",
            ),
            (
                WaitReason::NoRoom {
                    next_road: RoadId(0),
                },
                0.0,
                "waiting for space on Hauptstraße ahead",
            ),
        ] {
            assert_eq!(wait_text(&info(wait, speed), &net), text);
        }
        let lines = vehicle_lines(&info(WaitReason::FreeRoad, 10.0), &net);
        assert_eq!(
            lines[1..4],
            [
                "on Hauptstraße, lane 1 of 1",
                "going to Hauptstraße",
                "trip time 1:15"
            ]
        );
        assert_eq!(lines[4], "standing 200 s of 300 s before removal");

        let last = info(WaitReason::FreeRoad, 0.0);
        let at = |road| (road, 40.0);
        let (road, offset_m) = at(RoadId(0));
        assert_eq!(
            gone_text(Gone::Arrived { road, offset_m }, None, &net),
            "Reached its destination on Hauptstraße."
        );
        assert_eq!(
            gone_text(Gone::RemovedStuck { road, offset_m }, Some(&last), &net),
            "Removed after standing still 300 s (gridlock safety net) on Hauptstraße."
        );
        assert_eq!(
            gone_text(Gone::Unknown, None, &net),
            "No longer on the map."
        );
    }

    #[test]
    fn movements_read_as_delay_against_the_prior() {
        let net = east(1);
        let road = RoadInfo {
            road: RoadId(0),
            name: None,
            lanes: 1,
            speed_limit_mps: 13.9,
            length_m: 100.0,
            vehicles: 0,
            mean_speed_mps: None,
            removed_stuck: 2,
            movements: vec![MovementInfo {
                to_road: RoadId(0),
                turn: TurnType::LeftAcross,
                delay_s: 12.0,
                prior_s: 4.0,
            }],
        };
        assert!(road_lines(&road, &net)[0].starts_with("residential street, 1 lane,"));
        assert_eq!(
            road_lines(&road, &net)[3],
            "→ Hauptstraße (left): 12 s, expected 4 s"
        );
    }

    #[test]
    fn esc_closes_the_help_first_then_the_selection() {
        let mut ins = Inspector::default();
        ins.select(Some(Selection::Road(RoadId(0))));
        let mut help = true;
        ins.escape(&mut help);
        assert!(!help);
        assert!(ins.selected.is_some());
        ins.escape(&mut help);
        assert_eq!(ins.selected, None);
    }

    #[test]
    fn reset_and_a_new_vehicle_count_clear_the_selection() {
        let mut t = Traffic::new(crate::demo::map().network).expect("valid");
        for _ in 0..50 {
            t.frame(0.1);
        }
        let id = t.snapshot.id[0];
        let mut ins = Inspector::default();
        ins.select(Some(Selection::Vehicle(id)));
        ins.update(&t, 0.0);
        assert!(ins.info.is_some(), "a live vehicle is described");
        t.reset();
        ins.update(&t, 0.1);
        assert_eq!(ins.selected, None);
        ins.select(Some(Selection::Road(RoadId(0))));
        t.set_target_vehicles(3);
        ins.update(&t, 0.2);
        assert_eq!(ins.selected, None);
    }

    /// Runs one frame of `world` with `events` and the vehicles of `snap`.
    fn frame(
        ctx: &egui::Context,
        world: &mut World,
        net: &Network,
        snap: &VehicleSnapshot,
        ins: &mut Inspector,
        events: Vec<egui::Event>,
    ) {
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, egui::vec2(800.0, 600.0))),
            events,
            ..Default::default()
        };
        let mut out = ctx.run_ui(input, |ui| world.show(ui, snap, 0.0, (net, &[]), ins));
        out.textures_delta.clear();
    }

    #[test]
    fn follow_centers_on_the_vehicle_until_a_manual_pan() {
        let net = east(1);
        let mut world = World::new(&net);
        let ctx = egui::Context::default();
        let mut ins = Inspector::default();
        let mut snap = snapshot(&[(0, 80.0)]);
        ins.select(Some(Selection::Vehicle(VehicleId(0))));
        ins.toggle_follow();
        frame(&ctx, &mut world, &net, &snap, &mut ins, vec![]);
        let center = world.camera_center().expect("set");
        assert!((center - pos2(80.0, 0.0)).length() < 0.1, "{center:?}");
        snap.offset_m[0] = 90.0;
        frame(&ctx, &mut world, &net, &snap, &mut ins, vec![]);
        assert!(ins.following);
        let center = world.camera_center().expect("set");
        assert!((center - pos2(90.0, 0.0)).length() < 0.1, "{center:?}");
        // An arrow key pans by hand and stops following.
        let key = |pressed| egui::Event::Key {
            key: Key::ArrowLeft,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&ctx, &mut world, &net, &snap, &mut ins, vec![key(true)]);
        frame(&ctx, &mut world, &net, &snap, &mut ins, vec![key(false)]);
        assert!(!ins.following);
        snap.offset_m[0] = 30.0;
        frame(&ctx, &mut world, &net, &snap, &mut ins, vec![]);
        let center = world.camera_center().expect("set");
        assert!((center - pos2(30.0, 0.0)).length() > 10.0, "{center:?}");
    }
}
