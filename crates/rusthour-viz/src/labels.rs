//! Street-name labels (ADR-0003 "Map labels and fonts"): one candidate per
//! named road, found once per map; each frame only culls them.

use std::f32::consts::{FRAC_PI_2, PI};

use eframe::egui::{Pos2, Rect, Vec2, pos2, vec2};
use rusthour_api::Network;

use crate::camera::Camera;

/// Label text size in logical points.
pub const FONT_SIZE: f32 = 12.0;
/// ADR-0003: smaller labels are hidden. Labels are drawn at a fixed screen
/// size, so they never are; zooming out hides them instead (see [`place`]).
const MIN_FONT_SIZE: f32 = 11.0;
const _: () = assert!(FONT_SIZE >= MIN_FONT_SIZE);
/// Bounds the per-frame drawing cost.
pub const MAX_LABELS: usize = 200;
/// Segments shorter than this on screen can't hold any name; skipped before
/// laying out their text.
const MIN_ROOM: f32 = 24.0;

/// Where a road's name may go: the middle of its longest segment.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// The road's name.
    pub text: String,
    /// Segment midpoint, world meters (`y` north).
    pub mid: Pos2,
    /// Screen rotation of the text in radians, kept upright [−π/2, π/2):
    /// vertical names read bottom to top, as on most maps.
    pub angle: f32,
    /// Segment length in meters: the text must fit along it.
    pub length_m: f32,
}

/// A label to draw this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    /// Index into the candidates.
    pub index: usize,
    /// Screen position of the text's center.
    pub center: Pos2,
    /// Screen bounds of the rotated text, used for overlap culling.
    pub rect: Rect,
}

/// One candidate per named road, longest segments first, so that main
/// streets win when labels compete for room.
#[expect(clippy::cast_possible_truncation, reason = "local frame, meters")]
pub fn candidates(network: &Network) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = network
        .roads
        .iter()
        .filter_map(|road| {
            let text = road.name.as_deref().filter(|n| !n.trim().is_empty())?;
            let (a, b) =
                road.geometry
                    .windows(2)
                    .map(|w| (w[0], w[1]))
                    .max_by(|(a, b), (c, d)| {
                        let len = |p: rusthour_api::Point, q: rusthour_api::Point| {
                            (q.x - p.x).hypot(q.y - p.y)
                        };
                        len(*a, *b).total_cmp(&len(*c, *d))
                    })?;
            let (dx, dy) = ((b.x - a.x) as f32, (b.y - a.y) as f32);
            let length_m = dx.hypot(dy);
            (length_m > 0.0).then(|| Candidate {
                text: text.to_owned(),
                mid: pos2(
                    f64::midpoint(a.x, b.x) as f32,
                    f64::midpoint(a.y, b.y) as f32,
                ),
                angle: upright((-dy).atan2(dx)),
                length_m,
            })
        })
        .collect();
    out.sort_by(|a, b| b.length_m.total_cmp(&a.length_m));
    out
}

/// The same line direction, turned so text never reads upside down.
fn upright(angle: f32) -> f32 {
    if angle >= FRAC_PI_2 {
        angle - PI
    } else if angle < -FRAC_PI_2 {
        angle + PI
    } else {
        angle
    }
}

/// Picks the labels to draw: centered inside `viewport` (the painter clips
/// the rest, so labels don't pop out at the edges), fitting along their segment
/// at the current zoom, and not overlapping a label placed before them.
/// `text_size` gives a candidate's text size in points; it is only asked
/// for candidates that pass the cheap checks.
pub fn place(
    candidates: &[Candidate],
    camera: &Camera,
    viewport: Rect,
    mut text_size: impl FnMut(usize) -> Vec2,
) -> Vec<Placed> {
    let mut placed: Vec<Placed> = Vec::new();
    for (index, c) in candidates.iter().enumerate() {
        if placed.len() == MAX_LABELS {
            break;
        }
        let room = c.length_m * camera.scale;
        let center = camera.world_to_screen(viewport, c.mid);
        if room < MIN_ROOM || !viewport.contains(center) {
            continue;
        }
        let size = text_size(index);
        if size.x > room {
            continue;
        }
        // shortcut: overlap uses the rotated text's axis-aligned bounds, which
        // hides more diagonal labels than needed; use oriented boxes if the
        // M1 trial finds too few labels.
        let (sin, cos) = (c.angle.sin().abs(), c.angle.cos().abs());
        let half = vec2(cos * size.x + sin * size.y, sin * size.x + cos * size.y) / 2.0;
        let rect = Rect::from_center_size(center, 2.0 * half);
        if !placed.iter().any(|p| p.rect.intersects(rect)) {
            placed.push(Placed {
                index,
                center,
                rect,
            });
        }
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use rusthour_api::{Node, NodeId, Point, Road};

    fn viewport() -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(800.0, 600.0))
    }

    /// Fake text metrics: 7 points per character, one line high.
    fn size(c: &Candidate) -> Vec2 {
        #[expect(clippy::cast_precision_loss, reason = "test metrics")]
        let chars = c.text.chars().count() as f32;
        vec2(7.0 * chars, FONT_SIZE + 2.0)
    }

    fn candidate() -> impl Strategy<Value = Candidate> {
        (
            "[a-zäöüß]{1,20}",
            -600f32..600.0,
            -600f32..600.0,
            -FRAC_PI_2..FRAC_PI_2 - 1e-3,
            1f32..400.0,
        )
            .prop_map(|(text, x, y, angle, length_m)| Candidate {
                text,
                mid: pos2(x, y),
                angle,
                length_m,
            })
    }

    proptest! {
        #[test]
        fn placed_labels_fit_and_never_overlap(
            cands in prop::collection::vec(candidate(), 0..300),
            cx in -300f32..300.0, cy in -300f32..300.0, scale in 0.05f32..20.0,
        ) {
            let camera = Camera { center: pos2(cx, cy), scale };
            let placed = place(&cands, &camera, viewport(), |i| size(&cands[i]));
            prop_assert!(placed.len() <= MAX_LABELS);
            for (n, p) in placed.iter().enumerate() {
                let c = &cands[p.index];
                prop_assert!(viewport().contains(p.center));
                // Never wider than its road segment on screen: zooming out
                // hides labels instead of shrinking them below the minimum.
                prop_assert!(size(c).x <= c.length_m * scale);
                for q in &placed[..n] {
                    prop_assert!(!p.rect.intersects(q.rect), "{p:?} overlaps {q:?}");
                }
            }
        }
    }

    #[test]
    fn default_fonts_have_german_letters() {
        let ctx = eframe::egui::Context::default();
        let mut out = ctx.run_ui(eframe::egui::RawInput::default(), |_| {});
        out.textures_delta.clear();
        let font = eframe::egui::FontId::proportional(FONT_SIZE);
        assert!(ctx.fonts_mut(|f| f.has_glyphs(&font, "äöüÄÖÜß")));
    }

    #[test]
    fn one_upright_candidate_per_named_road_on_its_longest_segment() {
        let p = |x, y| Point { x, y };
        let road = |name: Option<&str>, geometry| Road {
            from_node: NodeId(0),
            to_node: NodeId(1),
            geometry,
            speed_limit_mps: 13.9,
            lanes: 1,
            name: name.map(Into::into),
            osm: None,
            roundabout: false,
        };
        let network = Network {
            nodes: vec![
                Node {
                    position: p(0.0, 0.0),
                    osm_node: None,
                },
                Node {
                    position: p(-100.0, 10.0),
                    osm_node: None,
                },
            ],
            roads: vec![
                road(None, vec![p(0.0, 0.0), p(50.0, 0.0)]),
                // Westward: the text must still read left to right.
                road(
                    Some("Müllerweg"),
                    vec![p(0.0, 0.0), p(-10.0, 0.0), p(-100.0, 10.0)],
                ),
                // Southward and vertical: reads bottom to top.
                road(Some("Bahnhofstraße"), vec![p(0.0, 0.0), p(0.0, -50.0)]),
            ],
            connections: vec![],
        };
        let c = candidates(&network);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].text, "Müllerweg", "longest first");
        assert!((c[0].length_m - 90f32.hypot(10.0)).abs() < 1e-3);
        assert!((c[0].mid.x + 55.0).abs() < 1e-3 && (c[0].mid.y - 5.0).abs() < 1e-3);
        assert!(c[0].angle.abs() < FRAC_PI_2);
        assert!((c[1].angle + FRAC_PI_2).abs() < 1e-6, "{}", c[1].angle);
    }
}
