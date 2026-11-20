//! Vehicle colours: one colour, or by speed, driver profile or vehicle
//! class, each with a legend (M4.4).
//!
//! Each mode has its own palette (M4 trial): drivers in white and warm
//! pinks and lilacs, vehicles in a light stone and cool blues and cyans, speed
//! in one lavender-blue ramp, the most visible step for stopped. None is
//! red, amber or green, the signal colours. In the dark theme every colour is
//! lighter than the grey road, in the light theme darker, by at least 1.5:1,
//! so moving traffic doesn't sink into the road, and 3:1 against the
//! background. Within a mode the colours stay apart under protan, deutan
//! and tritan simulation. The tests check all of it.

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

/// Aggressive pink, normal white, cautious pale pink, erratic lilac: white
/// and warm hues, the rarest (erratic) the most saturated.
const DRIVER_DARK: [Color32; 4] = [
    Color32::from_rgb(0xff, 0x96, 0xb5),
    Color32::from_rgb(0xff, 0xff, 0xff),
    Color32::from_rgb(0xf8, 0xd1, 0xeb),
    Color32::from_rgb(0xd0, 0x9c, 0xff),
];
/// The same roles, dark for the light background.
const DRIVER_LIGHT: [Color32; 4] = [
    Color32::from_rgb(0xa0, 0x10, 0x7a),
    Color32::from_rgb(0x00, 0x00, 0x00),
    Color32::from_rgb(0x3f, 0x06, 0x33),
    Color32::from_rgb(0x64, 0x18, 0xc0),
];
/// Car light stone, van cyan, truck pale aqua, bus steel blue: a neutral and
/// cool hues; the car still stands out from the road (2.2:1).
const VEHICLE_DARK: [Color32; 4] = [
    Color32::from_rgb(0xd3, 0xd0, 0xb3),
    Color32::from_rgb(0x75, 0xdf, 0xff),
    Color32::from_rgb(0xb0, 0xff, 0xf4),
    Color32::from_rgb(0x66, 0xbd, 0xd0),
];
/// The same roles, dark for the light background.
const VEHICLE_LIGHT: [Color32; 4] = [
    Color32::from_rgb(0x55, 0x4a, 0x40),
    Color32::from_rgb(0x00, 0x00, 0x86),
    Color32::from_rgb(0x04, 0x15, 0x13),
    Color32::from_rgb(0x00, 0x52, 0xc7),
];
/// Stopped, slow, moving, free: one lavender-blue hue, lightest (most
/// visible) for stopped.
const SPEED_DARK: [Color32; 4] = [
    Color32::from_rgb(0xef, 0xf1, 0xfc),
    Color32::from_rgb(0xcf, 0xd7, 0xfb),
    Color32::from_rgb(0xb5, 0xc1, 0xf9),
    Color32::from_rgb(0x9f, 0xad, 0xf6),
];
/// The same steps for the light background, darkest for stopped.
const SPEED_LIGHT: [Color32; 4] = [
    Color32::from_rgb(0x0c, 0x0a, 0x28),
    Color32::from_rgb(0x22, 0x21, 0x53),
    Color32::from_rgb(0x3b, 0x39, 0x83),
    Color32::from_rgb(0x56, 0x56, 0xb0),
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
            (Self::Driver, true) => DRIVER_DARK,
            (Self::Driver, false) => DRIVER_LIGHT,
            (Self::Vehicle | Self::Plain, true) => VEHICLE_DARK,
            (Self::Vehicle | Self::Plain, false) => VEHICLE_LIGHT,
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

/// Profiles by colour slot, in the CLI's and the mix table's order.
const PROFILES: [DriverProfile; 4] = [
    DriverProfile::Aggressive,
    DriverProfile::Normal,
    DriverProfile::Cautious,
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
    use crate::marks::signal_style;
    use crate::world::ROAD_COLOR;
    use rusthour_api::{DriverMix, FleetMix, SignalState};

    fn linear(c: Color32) -> [f32; 3] {
        [c.r(), c.g(), c.b()].map(|v| {
            let v = f32::from(v) / 255.0;
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        })
    }

    fn luminance(c: Color32) -> f32 {
        let [r, g, b] = linear(c);
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    /// WCAG contrast ratio of two colours.
    fn contrast(a: Color32, b: Color32) -> f32 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// Colour-vision deficiency simulation (Machado et al. 2009, severity 1)
    /// on linear RGB: none, protan, deutan, tritan.
    const CVD: [[[f32; 3]; 3]; 4] = [
        [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        [
            [0.152_286, 1.052_583, -0.204_868],
            [0.114_503, 0.786_281, 0.099_216],
            [-0.003_882, -0.048_116, 1.051_998],
        ],
        [
            [0.367_322, 0.860_646, -0.227_968],
            [0.280_085, 0.672_501, 0.047_413],
            [-0.011_820, 0.042_940, 0.968_881],
        ],
        [
            [1.255_528, -0.076_749, -0.178_779],
            [-0.078_411, 0.930_809, 0.147_602],
            [0.004_733, 0.691_367, 0.303_900],
        ],
    ];

    fn mul(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
        m.map(|row| row[0] * v[0] + row[1] * v[1] + row[2] * v[2])
    }

    /// Oklab of `c` as seen with `cvd` (an index into [`CVD`]).
    fn oklab(c: Color32, cvd: usize) -> [f32; 3] {
        let rgb = mul(&CVD[cvd], linear(c)).map(|v| v.clamp(0.0, 1.0));
        let lms = mul(
            &[
                [0.412_221_46, 0.536_332_55, 0.051_445_99],
                [0.211_903_5, 0.680_699_5, 0.107_396_96],
                [0.088_302_46, 0.281_718_85, 0.629_978_7],
            ],
            rgb,
        );
        mul(
            &[
                [0.210_454_26, 0.793_617_8, -0.004_072_047],
                [1.977_998_5, -2.428_592_2, 0.450_593_7],
                [0.025_904_037, 0.782_771_77, -0.808_675_77],
            ],
            lms.map(f32::cbrt),
        )
    }

    /// Oklab distance × 100 with `cvd`.
    fn delta_e(a: Color32, b: Color32, cvd: usize) -> f32 {
        let (x, y) = (oklab(a, cvd), oklab(b, cvd));
        100.0
            * x.iter()
                .zip(y)
                .map(|(p, q)| (p - q).powi(2))
                .sum::<f32>()
                .sqrt()
    }

    /// The smallest distance between two colours of `palette` under any
    /// simulation.
    fn closest_pair(palette: [Color32; 4]) -> f32 {
        let mut min = f32::INFINITY;
        for (k, &a) in palette.iter().enumerate() {
            for &b in &palette[k + 1..] {
                for cvd in 0..CVD.len() {
                    min = min.min(delta_e(a, b, cvd));
                }
            }
        }
        min
    }

    const MODES: [ColorBy; 3] = [ColorBy::Speed, ColorBy::Driver, ColorBy::Vehicle];

    #[test]
    fn each_mode_has_its_own_palette_distinct_under_colour_blindness() {
        for dark in [true, false] {
            let [speed, driver, vehicle] = MODES.map(|by| by.palette(dark));
            assert!(closest_pair(driver) >= 9.0, "{dark}");
            assert!(closest_pair(vehicle) >= 9.0, "{dark}");
            // An ordered ramp in a narrow band of lightness: closer steps.
            assert!(closest_pair(speed) >= 5.5, "{dark}");
            let apart = |a: [Color32; 4], b: [Color32; 4]| {
                a.iter()
                    .flat_map(|&x| b.iter().map(move |&y| delta_e(x, y, 0)))
                    .fold(f32::INFINITY, f32::min)
            };
            assert!(
                apart(driver, vehicle) >= 9.0,
                "an orange truck isn't an aggressive driver"
            );
            assert!(apart(speed, driver) >= 4.0 && apart(speed, vehicle) >= 4.0);
        }
    }

    #[test]
    fn palettes_stay_clear_of_the_signal_colours() {
        let signals = [SignalState::Red, SignalState::Amber, SignalState::Green]
            .map(|s| signal_style(s, 1.0).color);
        for dark in [true, false] {
            for by in MODES {
                for c in by.palette(dark) {
                    for s in signals {
                        assert!(delta_e(c, s, 0) >= 14.0, "{by:?} {c:?} near signal {s:?}");
                    }
                }
            }
        }
    }

    #[test]
    fn colours_read_on_the_road_and_the_background() {
        for (visuals, dark) in [(Visuals::dark(), true), (Visuals::light(), false)] {
            let bg = visuals.panel_fill;
            for by in MODES {
                for c in by.palette(dark) {
                    let lighter = luminance(c) > luminance(ROAD_COLOR);
                    assert_eq!(lighter, dark, "{by:?} {c:?}: like text, against the road");
                    assert!(contrast(c, ROAD_COLOR) >= 1.55, "{by:?} {c:?} on the road");
                    assert!(contrast(c, bg) >= 3.0, "{by:?} {c:?} on {bg:?}");
                }
            }
        }
    }

    #[test]
    fn legends_list_profiles_and_classes_in_the_cli_order() {
        let names = |by: ColorBy| {
            by.legend(true)
                .into_iter()
                .map(|(n, _)| n)
                .collect::<Vec<_>>()
        };
        assert_eq!(names(ColorBy::Driver), DriverMix::NAMES);
        assert_eq!(names(ColorBy::Vehicle), FleetMix::NAMES);
        assert_eq!(
            ColorBy::Driver.legend(false)[profile_slot(DriverProfile::Erratic)].0,
            "erratic"
        );
        assert_eq!(
            ColorBy::Vehicle.legend(false)[class_slot(VehicleClass::Bus)].0,
            "bus"
        );
        assert!(ColorBy::Plain.legend(true).is_empty());
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
}
