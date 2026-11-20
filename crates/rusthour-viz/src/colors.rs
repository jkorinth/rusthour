//! Vehicle colours: one colour, or by speed, driver profile or vehicle
//! class, each with a legend (M4.4).
//!
//! The categorical colours are Okabe–Ito hues with a neutral (white or
//! black) for the biggest group, so the minorities stand out; the speed
//! ramp is one blue hue, darkest (light theme) or lightest (dark theme)
//! for stopped vehicles. Both were checked for colour-vision deficiency
//! (all pairs, Oklab ΔE ≥ 8 under protan, deutan and tritan simulation) and
//! for at least 3:1 (categorical) or 2:1 (ramp) against each theme's panel
//! background.

use eframe::egui::{Align2, Color32, FontId, Painter, Rect, Visuals, pos2, vec2};
use rusthour_api::{DriverProfile, Network, VehicleClass, VehicleSnapshot};

/// What the vehicle colour shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorBy {
    /// One colour for all.
    #[default]
    Plain,
    /// Speed against the road's limit.
    Speed,
    /// Driver profile.
    Driver,
    /// Vehicle class.
    Vehicle,
}

/// Vehicle fill in the dark theme: amber, apart from the gray roads in hue.
const PLAIN_DARK: Color32 = Color32::from_rgb(0xff, 0xc8, 0x3c);
/// Vehicle fill in the light theme: a burnt orange that reads on the light
/// background where zoomed-out vehicles overhang their roads.
const PLAIN_LIGHT: Color32 = Color32::from_rgb(0x8f, 0x35, 0x00);

/// Neutral, blue, vermilion, green.
const CATEGORICAL_DARK: [Color32; 4] = [
    Color32::WHITE,
    Color32::from_rgb(0x56, 0xb4, 0xe9),
    Color32::from_rgb(0xd5, 0x5e, 0x00),
    Color32::from_rgb(0x00, 0x9e, 0x73),
];
/// The same hues, stepped for the light background.
const CATEGORICAL_LIGHT: [Color32; 4] = [
    Color32::BLACK,
    Color32::from_rgb(0x00, 0x72, 0xb2),
    Color32::from_rgb(0xd5, 0x5e, 0x00),
    Color32::from_rgb(0x00, 0x9e, 0x73),
];
/// Stopped, slow, moving, free; the most visible step for stopped.
const SPEED_DARK: [Color32; 4] = [
    Color32::from_rgb(0xcd, 0xe2, 0xfb),
    Color32::from_rgb(0x86, 0xb6, 0xef),
    Color32::from_rgb(0x39, 0x87, 0xe5),
    Color32::from_rgb(0x18, 0x4f, 0x95),
];
const SPEED_LIGHT: [Color32; 4] = [
    Color32::from_rgb(0x0d, 0x36, 0x6b),
    Color32::from_rgb(0x1c, 0x5c, 0xab),
    Color32::from_rgb(0x39, 0x87, 0xe5),
    Color32::from_rgb(0x6d, 0xa7, 0xec),
];
const SPEED_NAMES: [&str; 4] = [
    "stopped",
    "below 40% of the limit",
    "below 80%",
    "80% or more",
];
/// Below this a vehicle counts as stopped (m/s), about 7 km/h.
const STOPPED_MPS: f32 = 2.0;

impl ColorBy {
    /// All modes, in the selector's order.
    pub const ALL: [Self; 4] = [Self::Plain, Self::Speed, Self::Driver, Self::Vehicle];

    /// The selector's name for the mode.
    pub fn label(self) -> &'static str {
        match self {
            Self::Plain => "one colour",
            Self::Speed => "speed",
            Self::Driver => "driver",
            Self::Vehicle => "vehicle",
        }
    }

    /// The legend: one entry per colour; empty for [`Self::Plain`].
    pub fn legend(self, dark: bool) -> Vec<(&'static str, Color32)> {
        let names: [&str; 4] = match self {
            Self::Plain => return Vec::new(),
            Self::Speed => SPEED_NAMES,
            Self::Driver => [0, 1, 2, 3].map(|k| crate::inspect::profile_text(PROFILES[k])),
            Self::Vehicle => [0, 1, 2, 3].map(|k| crate::inspect::vehicle_class_text(CLASSES[k])),
        };
        names.into_iter().zip(self.palette(dark)).collect()
    }

    fn palette(self, dark: bool) -> [Color32; 4] {
        match (self, dark) {
            (Self::Speed, true) => SPEED_DARK,
            (Self::Speed, false) => SPEED_LIGHT,
            (_, true) => CATEGORICAL_DARK,
            (_, false) => CATEGORICAL_LIGHT,
        }
    }

    /// The colour of vehicle `i` of `snap`. Columns an older server leaves
    /// empty count as a normal driver in a car.
    pub fn color(self, dark: bool, snap: &VehicleSnapshot, i: usize, network: &Network) -> Color32 {
        let slot = match self {
            Self::Plain => return if dark { PLAIN_DARK } else { PLAIN_LIGHT },
            Self::Speed => {
                let limit = network
                    .roads
                    .get(snap.road[i].0 as usize)
                    .map_or(f64::INFINITY, |r| r.speed_limit_mps);
                #[expect(clippy::cast_possible_truncation, reason = "a speed")]
                speed_bin(snap.speed_mps[i], limit as f32)
            }
            Self::Driver => profile_slot(snap.profile.get(i).copied().unwrap_or_default()),
            Self::Vehicle => class_slot(snap.class.get(i).copied().unwrap_or_default()),
        };
        self.palette(dark)[slot]
    }
}

/// Draws `legend` at the bottom left of `viewport`, above the scale bar.
pub fn draw_legend(
    painter: &Painter,
    viewport: Rect,
    visuals: &Visuals,
    legend: &[(&str, Color32)],
) {
    const ROW_PT: f32 = 16.0;
    if legend.is_empty() {
        return;
    }
    let font = FontId::proportional(12.0);
    let ink = visuals.strong_text_color();
    let width = legend
        .iter()
        .map(|(name, _)| {
            painter
                .layout_no_wrap((*name).to_owned(), font.clone(), ink)
                .size()
                .x
        })
        .fold(0.0, f32::max);
    #[expect(clippy::cast_precision_loss, reason = "four rows")]
    let height = ROW_PT * legend.len() as f32;
    let top_left = viewport.left_bottom() + vec2(8.0, -44.0 - height);
    let frame = Rect::from_min_size(top_left, vec2(width + 30.0, height + 8.0));
    painter.rect_filled(frame, 4.0, visuals.extreme_bg_color.gamma_multiply(0.85));
    for (k, (name, color)) in (0u8..).zip(legend) {
        let y = top_left.y + 4.0 + ROW_PT * f32::from(k);
        let swatch = Rect::from_min_size(pos2(top_left.x + 6.0, y + 3.0), vec2(10.0, 10.0));
        painter.rect_filled(swatch, 2.0, *color);
        painter.text(
            pos2(swatch.right() + 6.0, y + ROW_PT / 2.0),
            Align2::LEFT_CENTER,
            *name,
            font.clone(),
            ink,
        );
    }
}

/// Profiles by colour slot: the most common gets the neutral colour.
const PROFILES: [DriverProfile; 4] = [
    DriverProfile::Normal,
    DriverProfile::Cautious,
    DriverProfile::Aggressive,
    DriverProfile::Erratic,
];
/// Classes by colour slot.
const CLASSES: [VehicleClass; 4] = [
    VehicleClass::Car,
    VehicleClass::Van,
    VehicleClass::Truck,
    VehicleClass::Bus,
];

fn profile_slot(p: DriverProfile) -> usize {
    PROFILES.iter().position(|&q| q == p).unwrap_or(0)
}

fn class_slot(c: VehicleClass) -> usize {
    CLASSES.iter().position(|&k| k == c).unwrap_or(0)
}

/// 0 stopped, 1 below 40% of the limit, 2 below 80%, 3 above.
fn speed_bin(speed_mps: f32, limit_mps: f32) -> usize {
    if speed_mps < STOPPED_MPS {
        0
    } else if speed_mps < 0.4 * limit_mps {
        1
    } else if speed_mps < 0.8 * limit_mps {
        2
    } else {
        3
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// WCAG contrast ratio of two colours.
    fn contrast(a: Color32, b: Color32) -> f32 {
        let lum = |c: Color32| {
            let lin = |v: u8| {
                let v = f32::from(v) / 255.0;
                if v <= 0.040_45 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * lin(c.r()) + 0.7152 * lin(c.g()) + 0.0722 * lin(c.b())
        };
        let (x, y) = (lum(a), lum(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn each_group_has_its_own_colour_and_legend_entry() {
        for dark in [true, false] {
            for by in [ColorBy::Speed, ColorBy::Driver, ColorBy::Vehicle] {
                let legend = by.legend(dark);
                assert_eq!(legend.len(), 4);
                for (k, (_, c)) in legend.iter().enumerate() {
                    assert!(legend[..k].iter().all(|(_, d)| d != c), "{by:?} {dark}");
                }
            }
            assert!(ColorBy::Plain.legend(dark).is_empty());
        }
        let legend = ColorBy::Driver.legend(true);
        let slot = profile_slot(DriverProfile::Aggressive);
        assert_eq!(legend[slot].0, "aggressive");
        assert_eq!(
            ColorBy::Vehicle.legend(false)[class_slot(VehicleClass::Bus)].0,
            "bus"
        );
        assert_eq!(
            profile_slot(DriverProfile::Normal),
            0,
            "the majority is neutral"
        );
        assert_eq!(class_slot(VehicleClass::Car), 0);
    }

    #[test]
    fn speed_bins_go_by_the_share_of_the_limit() {
        let limit = 13.9; // 50 km/h
        assert_eq!(speed_bin(0.0, limit), 0);
        assert_eq!(speed_bin(1.9, limit), 0);
        assert_eq!(speed_bin(5.0, limit), 1);
        assert_eq!(speed_bin(8.0, limit), 2);
        assert_eq!(speed_bin(12.0, limit), 3);
        assert_eq!(
            speed_bin(30.0, f32::INFINITY),
            1,
            "unknown road: never free"
        );
    }

    #[test]
    fn colours_read_on_both_themes_backgrounds() {
        for (visuals, dark) in [(Visuals::dark(), true), (Visuals::light(), false)] {
            let bg = visuals.panel_fill;
            for c in ColorBy::Driver.palette(dark) {
                assert!(contrast(c, bg) >= 3.0, "{c:?} on {bg:?}");
            }
            for c in ColorBy::Speed.palette(dark) {
                assert!(contrast(c, bg) >= 2.0, "{c:?} on {bg:?}");
            }
        }
    }
}
