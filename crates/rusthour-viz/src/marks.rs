//! Road markings and map furniture (ADR-0005 §9): lane dividers, signal
//! bars at stop lines, stop and give-way signs, and the scale bar.

use eframe::egui::{Align2, Color32, FontId, Mesh, Painter, Pos2, Rect, Shape, Stroke, Vec2, vec2};
use rusthour_api::{EndControl, Network, Placement, Point, RoadId, SignalSnapshot, SignalState};

use crate::camera::{Camera, LANE_WIDTH_M};
use crate::world::{shifted, to_mesh, vehicle_quad};

/// Painted length of a lane divider dash, in meters (German town roads).
const DASH_M: f32 = 3.0;
/// One dash and its gap, in meters.
const DASH_PERIOD_M: f32 = 9.0;
/// Divider width, in meters.
const DIVIDER_WIDTH_M: f32 = 0.3;
/// Divider paint, light against the grey roads.
const DIVIDER_COLOR: Color32 = Color32::from_gray(0xe0);
/// Signal bars show from this zoom, in screen points per meter.
const SIGNALS_FROM_SCALE: f32 = 0.5;
/// Signs show from this zoom, in screen points per meter.
const SIGNS_FROM_SCALE: f32 = 3.0;
/// How far before a road's end its sign stands, in meters.
const SIGN_BACK_M: f64 = 10.0;
/// Sign size (radius), in screen points.
const SIGN_PT: f32 = 7.0;
/// A signal bar within this many points of the pointer gets its tooltip.
const SIGNAL_HOVER_PT: f32 = 10.0;
/// Longest scale bar, in screen points.
const SCALE_BAR_PT: f32 = 120.0;
const SIGN_RED: Color32 = Color32::from_rgb(0xc8, 0x10, 0x2e);
const PRIORITY_YELLOW: Color32 = Color32::from_rgb(0xff, 0xcc, 0x00);

/// A sign drawn at a road end.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SignKind {
    Stop,
    GiveWay,
    /// The road has right of way at its end (`priority_road`).
    Priority,
}

/// A sign beside `road`, `offset_m` along it.
#[derive(Debug, Clone, Copy)]
pub struct Sign {
    road: RoadId,
    offset_m: f32,
    kind: SignKind,
}

/// The signs of `network`, [`SIGN_BACK_M`] before each road's end.
pub fn signs(network: &Network) -> Vec<Sign> {
    (0u32..)
        .zip(&network.roads)
        .filter_map(|(i, r)| {
            let kind = match r.end_control {
                EndControl::Stop => SignKind::Stop,
                EndControl::GiveWay => SignKind::GiveWay,
                EndControl::None if r.priority_road => SignKind::Priority,
                _ => return None,
            };
            let length: f64 = r
                .geometry
                .windows(2)
                .map(|w| (w[1].x - w[0].x).hypot(w[1].y - w[0].y))
                .sum();
            // Back from the node, which lies inside the junction.
            #[expect(clippy::cast_possible_truncation, reason = "meters")]
            let offset_m = (length - SIGN_BACK_M).max(length / 2.0) as f32;
            Some(Sign {
                road: RoadId(i),
                offset_m,
                kind,
            })
        })
        .collect()
}

/// What signals and signs are drawn with, as drawn this frame.
pub struct Marks<'a> {
    pub placement: &'a Placement,
    pub network: &'a Network,
    pub camera: Camera,
    pub viewport: Rect,
}

impl Marks<'_> {
    /// Screen position and unit heading of `lane` (fractional) at `offset_m` on `road`.
    fn at(&self, road: RoadId, lane: f32, offset_m: f32) -> (Pos2, Vec2) {
        let (p, [hx, hy]) = self.placement.place(road, lane, offset_m, LANE_WIDTH_M);
        let screen = self.camera.mesh_transform(self.viewport) * to_mesh(p);
        (screen, vec2(hx, -hy))
    }

    /// A bar across each signal arm's stop line; returns the road whose bar
    /// is under `hover`, for the tooltip.
    pub fn signals(
        &self,
        painter: &Painter,
        signals: &SignalSnapshot,
        time_scale: f64,
        hover: Option<Pos2>,
    ) -> Option<RoadId> {
        if self.camera.scale < SIGNALS_FROM_SCALE {
            return None;
        }
        let scale = self.camera.scale;
        let full_thick = (1.2 * scale).clamp(4.0, 12.0);
        let mut hovered = None;
        let mut nearest = SIGNAL_HOVER_PT;
        for (i, &road) in signals.road.iter().enumerate() {
            let lanes = f32::from(self.network.roads[road.0 as usize].lanes);
            let (line, dir) = self.at(road, (lanes - 1.0) / 2.0, f32::MAX);
            if !self.viewport.expand(SIGN_PT).contains(line) {
                continue;
            }
            let style = signal_style(signals.state[i], time_scale);
            let thick = full_thick * style.thick;
            let half = (lanes * LANE_WIDTH_M * scale / 2.0).max(4.0) * style.length;
            // On the road, just before the line.
            let center = line - dir * (full_thick / 2.0);
            let (a, t) = (dir.rot90() * half, dir * (thick / 2.0));
            let corners = vec![
                center + a + t,
                center - a + t,
                center - a - t,
                center + a - t,
            ];
            if style.filled {
                painter.add(Shape::convex_polygon(corners, style.color, Stroke::NONE));
            } else {
                painter.add(Shape::closed_line(corners, Stroke::new(2.0, style.color)));
            }
            if let Some(d) = hover.map(|p| p.distance(center))
                && d < nearest
            {
                nearest = d;
                hovered = Some(road);
            }
        }
        hovered
    }

    /// Stop, give-way and priority-road signs beside the road ends, when zoomed in.
    pub fn signs(&self, painter: &Painter, signs: &[Sign]) {
        if self.camera.scale < SIGNS_FROM_SCALE {
            return;
        }
        let white = Stroke::new(1.5, Color32::WHITE);
        let r = SIGN_PT;
        for s in signs {
            // One lane right of the rightmost lane: the verge.
            let (c, _) = self.at(s.road, -1.0, s.offset_m);
            if !self.viewport.expand(r).contains(c) {
                continue;
            }
            let shape = match s.kind {
                SignKind::Stop => {
                    let octagon = (0..8u8)
                        .map(|k| {
                            let a = (f32::from(k) + 0.5) * std::f32::consts::FRAC_PI_4;
                            c + vec2(a.cos(), a.sin()) * r
                        })
                        .collect();
                    Shape::convex_polygon(octagon, SIGN_RED, white)
                }
                SignKind::GiveWay => Shape::convex_polygon(
                    vec![
                        c + vec2(-r, -0.7 * r),
                        c + vec2(r, -0.7 * r),
                        c + vec2(0.0, r),
                    ],
                    Color32::WHITE,
                    Stroke::new(2.0, SIGN_RED),
                ),
                SignKind::Priority => Shape::convex_polygon(
                    vec![
                        c + vec2(0.0, -r),
                        c + vec2(r, 0.0),
                        c + vec2(0.0, r),
                        c + vec2(-r, 0.0),
                    ],
                    PRIORITY_YELLOW,
                    white,
                ),
            };
            painter.add(shape);
        }
    }
}

/// The scale bar, bottom left of `viewport`, for `scale` points per meter.
pub fn draw_scale_bar(
    painter: &Painter,
    scale: f32,
    viewport: Rect,
    visuals: &eframe::egui::Visuals,
) {
    let (label, width) = scale_bar(scale, SCALE_BAR_PT);
    let left = viewport.left_bottom() + vec2(12.0, -12.0);
    let right = left + vec2(width, 0.0);
    let tick = vec2(0.0, -5.0);
    let line = vec![left + tick, left, right, right + tick];
    let (ink, halo) = (visuals.strong_text_color(), visuals.extreme_bg_color);
    painter.add(Shape::line(line.clone(), Stroke::new(4.0, halo)));
    painter.add(Shape::line(line, Stroke::new(1.5, ink)));
    let at = left + vec2(width / 2.0, -3.0);
    let font = FontId::proportional(12.0);
    for offset in [
        vec2(-1.0, 0.0),
        vec2(1.0, 0.0),
        vec2(0.0, -1.0),
        vec2(0.0, 1.0),
    ] {
        painter.text(
            at + offset,
            Align2::CENTER_BOTTOM,
            &label,
            font.clone(),
            halo,
        );
    }
    painter.text(at, Align2::CENTER_BOTTOM, label, font, ink);
}

/// Above this time scale, signals are drawn neutral: phases would strobe
/// (more than 3 flashes a second, a photosensitivity risk).
pub const NEUTRAL_SIGNALS_ABOVE: f64 = 30.0;

/// The time scale that decides whether signals strobe: the one reached, as
/// the step budget may hold the wanted one back; 0 while paused.
// shortcut: no hysteresis, a speed reached that hovers around 30× can switch the
// bars every half second; add one if a trial sees it.
pub fn signal_time_scale(wanted: f64, achieved: f64, paused: bool) -> f64 {
    if paused { 0.0 } else { wanted.min(achieved) }
}

/// How a signal bar is drawn: never by colour alone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BarStyle {
    /// Length as a share of the road's width.
    pub length: f32,
    /// Thickness as a share of the full bar's.
    pub thick: f32,
    /// Filled, or only outlined.
    pub filled: bool,
    pub color: Color32,
}

/// Red a solid bar, amber a short one, green an outline; above
/// [`NEUTRAL_SIGNALS_ABOVE`] one thin grey bar whatever the state.
pub fn signal_style(state: SignalState, time_scale: f64) -> BarStyle {
    let bar = |length, thick, filled, color| BarStyle {
        length,
        thick,
        filled,
        color,
    };
    if time_scale > NEUTRAL_SIGNALS_ABOVE {
        return bar(1.0, 0.4, true, NEUTRAL_SIGNAL);
    }
    match state {
        SignalState::Red => bar(1.0, 1.0, true, Color32::from_rgb(0xe8, 0x1c, 0x1c)),
        SignalState::Amber => bar(0.5, 1.0, true, Color32::from_rgb(0xff, 0xa0, 0x00)),
        SignalState::Green => bar(1.0, 1.0, false, Color32::from_rgb(0x1c, 0xd0, 0x4c)),
    }
}

/// Signals above [`NEUTRAL_SIGNALS_ABOVE`]: lighter than the roads.
const NEUTRAL_SIGNAL: Color32 = Color32::from_gray(0xd0);

/// "red, green in 12 s": what `road`'s signal shows and when it changes.
pub fn signal_text(signals: &SignalSnapshot, road: RoadId) -> Option<String> {
    let i = signals.road.iter().position(|&r| r == road)?;
    let (now, next) = match signals.state[i] {
        SignalState::Red => ("red", "green"),
        SignalState::Amber => ("amber", "red"),
        SignalState::Green => ("green", "amber"),
    };
    Some(format!(
        "{now}, {next} in {:.0} s",
        signals.next_change_s[i]
    ))
}

/// The scale bar for `scale` screen points per meter: the longest round
/// length (1, 2 or 5 × 10ⁿ m) that fits in `max_pt`, its label and width in points.
pub fn scale_bar(scale: f32, max_pt: f32) -> (String, f32) {
    let most = max_pt / scale;
    let mut m = 10f32.powf(most.log10().floor());
    for step in [5.0, 2.0] {
        if m * step <= most {
            m *= step;
            break;
        }
    }
    let label = if m >= 1000.0 {
        format!("{} km", m / 1000.0)
    } else {
        format!("{m} m")
    };
    (label, m * scale)
}

/// Dashed dividers between the lanes of multi-lane roads, in mesh space
/// (world meters, `y` negated), on each road's drawn center line `lines`.
pub fn build_dividers(network: &Network, lines: &[Vec<Point>]) -> Mesh {
    let mut mesh = Mesh::default();
    for (road, line) in network.roads.iter().zip(lines) {
        let n = f64::from(road.lanes);
        for k in 1..road.lanes {
            // Divider k lies k lanes left of the right edge, at n/2 lanes.
            let right = (n / 2.0 - f64::from(k)) * f64::from(LANE_WIDTH_M);
            dashes(&shifted(line, right), &mut mesh);
        }
    }
    mesh
}

/// Adds `line`'s dashes to `mesh`: [`DASH_M`] painted every [`DASH_PERIOD_M`].
fn dashes(line: &[Point], mesh: &mut Mesh) {
    // Distance into the current period, carried across the polyline's corners.
    let mut phase = 0.0;
    for w in line.windows(2) {
        let (a, b) = (to_mesh(w[0]), to_mesh(w[1]));
        let len = a.distance(b);
        if len <= 0.0 {
            continue;
        }
        let dir = (b - a) / len;
        // Counting down reaches exactly 0, so float steps always end the loop.
        let mut rest = len;
        while rest > 0.0 {
            let in_dash = phase < DASH_M;
            let left = if in_dash {
                DASH_M - phase
            } else {
                DASH_PERIOD_M - phase
            };
            let step = left.min(rest);
            if in_dash {
                let front = a + dir * (len - rest + step);
                #[expect(clippy::cast_possible_truncation, reason = "far below u32::MAX")]
                let base = mesh.vertices.len() as u32;
                for corner in vehicle_quad(front, dir, step, DIVIDER_WIDTH_M) {
                    mesh.colored_vertex(corner, DIVIDER_COLOR);
                }
                mesh.add_triangle(base, base + 1, base + 2);
                mesh.add_triangle(base, base + 2, base + 3);
            }
            rest -= step;
            // Snap to the boundary, so no sliver step is left over.
            phase = match (step < left, in_dash) {
                (true, _) => phase + step,
                (false, true) => DASH_M,
                (false, false) => 0.0,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signal_states_differ_in_shape_and_go_neutral_when_fast() {
        let states = [SignalState::Red, SignalState::Amber, SignalState::Green];
        let shape = |s: BarStyle| (s.length.to_bits(), s.thick.to_bits(), s.filled);
        let slow: Vec<_> = states.iter().map(|&s| signal_style(s, 1.0)).collect();
        for (i, a) in slow.iter().enumerate() {
            for b in &slow[i + 1..] {
                assert_ne!(shape(*a), shape(*b), "{a:?} vs {b:?}: colour alone");
            }
        }
        let fast: Vec<_> = states.iter().map(|&s| signal_style(s, 100.0)).collect();
        assert!(fast.iter().all(|&s| s == fast[0]), "{fast:?}");
        assert!(
            slow.iter().all(|&s| shape(s) != shape(fast[0])),
            "neutral is its own shape"
        );
        assert_eq!(
            signal_style(SignalState::Red, NEUTRAL_SIGNALS_ABOVE),
            slow[0]
        );
    }

    #[test]
    fn signals_go_neutral_by_the_speed_reached_not_the_one_asked() {
        let red = |wanted, achieved, paused| {
            signal_style(
                SignalState::Red,
                signal_time_scale(wanted, achieved, paused),
            )
        };
        let (shown, neutral) = (red(1.0, 1.0, false), red(100.0, 100.0, false));
        assert_ne!(shown, neutral);
        // Asked 251×, held to 22× by the step budget: the phases are readable.
        assert_eq!(red(251.0, 22.0, false), shown);
        assert_eq!(red(251.0, 251.0, false), neutral);
        // Paused, nothing strobes.
        assert_eq!(red(1000.0, 1000.0, true), shown);
    }

    #[test]
    fn signal_text_names_the_state_and_the_next_one() {
        let signals = SignalSnapshot {
            tick: 0,
            road: vec![RoadId(3), RoadId(5), RoadId(7)],
            state: vec![SignalState::Red, SignalState::Amber, SignalState::Green],
            next_change_s: vec![12.3, 1.6, 20.0],
        };
        let text = |r| signal_text(&signals, RoadId(r));
        assert_eq!(text(3).as_deref(), Some("red, green in 12 s"));
        assert_eq!(text(5).as_deref(), Some("amber, red in 2 s"));
        assert_eq!(text(7).as_deref(), Some("green, amber in 20 s"));
        assert_eq!(text(4), None);
    }

    #[test]
    fn scale_bar_picks_a_round_length_that_fits() {
        assert_eq!(scale_bar(1.0, 120.0), ("100 m".into(), 100.0));
        assert_eq!(scale_bar(30.0, 120.0), ("2 m".into(), 60.0));
        let (label, pt) = scale_bar(0.11, 120.0);
        assert_eq!(label, "1 km");
        assert!((pt - 110.0).abs() < 1e-3);
        let (label, pt) = scale_bar(0.05, 120.0);
        assert_eq!(label, "2 km");
        assert!(pt <= 120.0 && pt > 120.0 / 2.5);
    }

    fn road(lanes: u8, geometry: Vec<Point>) -> rusthour_api::Road {
        rusthour_api::Road {
            from_node: rusthour_api::NodeId(0),
            to_node: rusthour_api::NodeId(1),
            geometry,
            speed_limit_mps: 13.9,
            lanes,
            name: None,
            osm: None,
            roundabout: false,
            class: rusthour_api::RoadClass::Unknown,
            priority_road: false,
            end_control: rusthour_api::EndControl::None,
            turn_lanes: vec![],
        }
    }

    /// Mesh `y` (= south) of every divider vertex of one 100 m east-bound road.
    fn divider_ys(lane_count: u8, shift_m: f64) -> Vec<f32> {
        let g = vec![Point { x: 0.0, y: 0.0 }, Point { x: 100.0, y: 0.0 }];
        let lines = vec![shifted(&g, shift_m)];
        let net = Network {
            nodes: vec![],
            roads: vec![road(lane_count, g)],
            connections: vec![],
        };
        let mesh = build_dividers(&net, &lines);
        assert!(mesh.is_valid());
        mesh.vertices.iter().map(|v| v.pos.y).collect()
    }

    #[test]
    fn dividers_lie_between_lanes_and_are_dashed() {
        assert!(divider_ys(1, 0.0).is_empty(), "one lane: no divider");
        let near = |ys: Vec<f32>, at: &[f32]| {
            !ys.is_empty() && ys.iter().all(|y| at.iter().any(|a| (y - a).abs() < 0.2))
        };
        assert!(near(divider_ys(2, 0.0), &[0.0]));
        assert!(near(divider_ys(3, 0.0), &[-1.75, 1.75]));
        // A twin's line is shifted right (south) by half its width.
        assert!(near(divider_ys(2, 3.5), &[3.5]));
        // Dashed: about a third of the length is painted.
        let g = vec![Point { x: 0.0, y: 0.0 }, Point { x: 100.0, y: 0.0 }];
        let net = Network {
            nodes: vec![],
            roads: vec![road(2, g.clone())],
            connections: vec![],
        };
        let mesh = build_dividers(&net, &[g]);
        let painted: f32 = mesh
            .vertices
            .chunks(4)
            .map(|q| (q[2].pos.x - q[0].pos.x).abs())
            .sum();
        assert!((25.0..=40.0).contains(&painted), "{painted}");
    }
}
