//! World ↔ screen transform for the map view (ADR-0003: our own camera type).
//!
//! World coordinates are meters with `y` pointing north; screen coordinates
//! are egui points with `y` pointing down.

use eframe::egui::{Pos2, Rect, Vec2, emath::TSTransform, pos2, vec2};

/// Width of one lane in meters, for rendering and the zoom limit.
pub const LANE_WIDTH_M: f32 = 3.5;

/// Pan and zoom state: which world point is at the viewport center, and how
/// many screen points one meter takes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Camera {
    /// World point (meters) shown at the viewport center.
    pub center: Pos2,
    /// Screen points per meter.
    pub scale: f32,
}

impl Camera {
    /// The camera that shows all of `bounds` (world, meters) centered in `viewport`.
    pub fn fit(bounds: Rect, viewport: Rect) -> Self {
        let size = bounds.size().max(Vec2::splat(1.0));
        let scale = (viewport.size() / size).min_elem();
        Self {
            center: bounds.center(),
            scale,
        }
    }

    /// World point under a screen position.
    pub fn screen_to_world(&self, viewport: Rect, screen: Pos2) -> Pos2 {
        let d = (screen - viewport.center()) / self.scale;
        pos2(self.center.x + d.x, self.center.y - d.y)
    }

    /// Screen position of a world point.
    pub fn world_to_screen(&self, viewport: Rect, world: Pos2) -> Pos2 {
        let d = (world - self.center) * self.scale;
        viewport.center() + vec2(d.x, -d.y)
    }

    /// Moves the map by `screen_delta` points, as when dragging it.
    pub fn pan(&mut self, screen_delta: Vec2) {
        self.center += vec2(-screen_delta.x, screen_delta.y) / self.scale;
    }

    /// Multiplies the zoom by `factor`, clamped to `min_scale..=max_scale`,
    /// keeping the world point under `anchor` (screen) where it is.
    pub fn zoom_around(
        &mut self,
        viewport: Rect,
        anchor: Pos2,
        factor: f32,
        min_scale: f32,
        max_scale: f32,
    ) {
        let before = self.screen_to_world(viewport, anchor);
        self.scale = (self.scale * factor).clamp(min_scale, max_scale.max(min_scale));
        let after = self.screen_to_world(viewport, anchor);
        self.center += before - after;
    }

    /// The transform that maps the map mesh (built with `y` negated, see
    /// `world::build_mesh`) onto the screen.
    pub fn mesh_transform(&self, viewport: Rect) -> TSTransform {
        let translation =
            viewport.center().to_vec2() + vec2(-self.center.x, self.center.y) * self.scale;
        TSTransform::new(translation, self.scale)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn viewport() -> Rect {
        Rect::from_min_size(pos2(10.0, 20.0), vec2(800.0, 600.0))
    }

    /// World → screen as the map is drawn: the mesh stores `y` negated.
    fn to_screen(cam: &Camera, w: Pos2) -> Pos2 {
        cam.mesh_transform(viewport()) * pos2(w.x, -w.y)
    }

    fn close(a: Pos2, b: Pos2, tol: f32) -> bool {
        (a - b).length() <= tol
    }

    proptest! {
        #[test]
        fn mesh_transform_inverts_screen_to_world(
            cx in -5e3f32..5e3, cy in -5e3f32..5e3, scale in 0.01f32..100.0,
            wx in -5e3f32..5e3, wy in -5e3f32..5e3,
        ) {
            let cam = Camera { center: pos2(cx, cy), scale };
            let w = pos2(wx, wy);
            let s = to_screen(&cam, w);
            let tol = 1e-3 * (1.0 + (w - cam.center).length());
            prop_assert!(close(cam.screen_to_world(viewport(), s), w, tol));
            prop_assert!(close(cam.world_to_screen(viewport(), w), s, 1e-3 * (1.0 + s.to_vec2().length())));
        }

        #[test]
        fn zoom_keeps_the_anchor_fixed(
            cx in -5e3f32..5e3, cy in -5e3f32..5e3, scale in 0.1f32..10.0,
            ax in 10f32..810.0, ay in 20f32..620.0, factor in 0.2f32..5.0,
        ) {
            let mut cam = Camera { center: pos2(cx, cy), scale };
            let anchor = pos2(ax, ay);
            let before = cam.screen_to_world(viewport(), anchor);
            cam.zoom_around(viewport(), anchor, factor, 0.01, 1000.0);
            prop_assert!((cam.scale - scale * factor).abs() <= 1e-4 * cam.scale);
            let tol = 1e-3 * (1.0 + (before - cam.center).length());
            prop_assert!(close(cam.screen_to_world(viewport(), anchor), before, tol));
        }
    }

    #[test]
    fn fit_shows_all_of_the_bounds() {
        let bounds = Rect::from_min_max(pos2(-300.0, 50.0), pos2(900.0, 250.0));
        let cam = Camera::fit(bounds, viewport());
        for corner in [
            bounds.left_top(),
            bounds.right_bottom(),
            bounds.left_bottom(),
            bounds.right_top(),
        ] {
            let s = to_screen(&cam, corner);
            assert!(viewport().expand(1e-3).contains(s), "{corner:?} -> {s:?}");
        }
        // North is up.
        assert!(to_screen(&cam, bounds.max).y < to_screen(&cam, bounds.min).y);
        // The wide map fills the viewport's width.
        assert!((cam.scale - 800.0 / 1200.0).abs() < 1e-6);
    }

    #[test]
    fn zoom_is_clamped() {
        let mut cam = Camera {
            center: Pos2::ZERO,
            scale: 1.0,
        };
        cam.zoom_around(viewport(), viewport().center(), 100.0, 0.5, 4.0);
        assert!((cam.scale - 4.0).abs() < f32::EPSILON);
        cam.zoom_around(viewport(), viewport().center(), 0.001, 0.5, 4.0);
        assert!((cam.scale - 0.5).abs() < f32::EPSILON);
    }
}
