//! The driver and fleet mix of a run (M4.3): a share per driver profile and
//! per vehicle class. One field per name, so a scenario file's `[drivers]`
//! and `[fleet]` tables (M5.5) map onto them 1:1.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Why a mix was rejected.
#[derive(Debug, Clone, PartialEq)]
pub enum MixError {
    /// A share is negative, NaN or infinite.
    BadShare {
        /// The profile or class.
        name: &'static str,
        /// The rejected share.
        share: f64,
    },
    /// Every share is 0.
    Empty,
    /// The text is not `name=share,…` with known names.
    Syntax(String),
}

impl fmt::Display for MixError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadShare { name, share } => {
                write!(f, "{name} must be a number ≥ 0, got {share}")
            }
            Self::Empty => write!(f, "at least one share must be above 0"),
            Self::Syntax(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for MixError {}

/// Shares scaled to parts per million for the core, which draws by their
/// ratios. A share below 0.5 ppm of the total rounds to 0.
const WEIGHT_SCALE: f64 = 1e6;

/// `shares` divided by their sum, after checking each is finite and ≥ 0
/// and one is above 0.
fn normalise(names: [&'static str; 4], shares: [f64; 4]) -> Result<[f64; 4], MixError> {
    if let Some((&name, &share)) = names
        .iter()
        .zip(&shares)
        .find(|&(_, &s)| !(s.is_finite() && s >= 0.0))
    {
        return Err(MixError::BadShare { name, share });
    }
    let max = shares.iter().copied().fold(0.0, f64::max);
    if max == 0.0 {
        return Err(MixError::Empty);
    }
    let sum: f64 = shares.iter().sum();
    if sum.is_finite() {
        // Exact for percentages: 15 / 100 is 0.15.
        return Ok(shares.map(|s| s / sum));
    }
    // Huge shares: scaled by the largest first, so the sum can't overflow.
    let sum: f64 = shares.iter().map(|s| s / max).sum();
    Ok(shares.map(|s| s / max / sum))
}

/// Parses `name=share,…`; a name left out gets 0.
fn parse(kind: &str, names: [&'static str; 4], text: &str) -> Result<[f64; 4], MixError> {
    let mut shares = [None; 4];
    for entry in text.split(',') {
        let (name, value) = entry
            .split_once('=')
            .ok_or_else(|| MixError::Syntax(format!("“{}” is not name=share", entry.trim())))?;
        let name = name.trim();
        let i = names.iter().position(|&n| n == name).ok_or_else(|| {
            MixError::Syntax(format!("unknown {kind} “{name}”; use {}", names.join(", ")))
        })?;
        let value = value.trim();
        let share = value
            .parse::<f64>()
            .map_err(|_| MixError::Syntax(format!("{name}: “{value}” is not a number")))?;
        // A number too big for f64 parses as inf; say so rather than "got inf".
        if share.is_infinite() && !value.to_ascii_lowercase().contains("inf") {
            return Err(MixError::Syntax(format!(
                "{name}: “{value}” is out of range"
            )));
        }
        if shares[i].replace(share).is_some() {
            return Err(MixError::Syntax(format!("{name} is given twice")));
        }
    }
    Ok(shares.map(|s| s.unwrap_or(0.0)))
}

macro_rules! mix {
    (
        $(#[$doc:meta])*
        $name:ident, $kind:literal, $what:literal,
        [$($field:ident = $default:literal),+ $(,)?]
    ) => {
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        pub struct $name {
            $(
                #[doc = concat!("Share of ", stringify!($field), " ", $what, ".")]
                pub $field: f64,
            )+
        }

        impl Default for $name {
            /// The shares of the ADR, as fractions.
            fn default() -> Self {
                Self { $($field: $default),+ }
            }
        }

        impl $name {
            /// The names, as they appear in text and JSON.
            pub const NAMES: [&'static str; 4] = [$(stringify!($field)),+];

            /// The shares in [`Self::NAMES`] order.
            #[must_use]
            pub fn shares(self) -> [f64; 4] {
                [$(self.$field),+]
            }

            /// The mix with `shares` in [`Self::NAMES`] order.
            #[must_use]
            pub fn from_shares(shares: [f64; 4]) -> Self {
                let [$($field),+] = shares;
                Self { $($field),+ }
            }

            /// The mix with shares that sum to 1.
            ///
            /// # Errors
            ///
            /// If a share is negative, NaN or infinite, or all are 0.
            pub fn normalised(self) -> Result<Self, MixError> {
                normalise(Self::NAMES, self.shares()).map(Self::from_shares)
            }

            /// The weights for [`Config`](crate::Config), in the core's
            /// order (the same as [`Self::NAMES`]).
            ///
            /// # Errors
            ///
            /// As [`normalised`](Self::normalised).
            pub fn weights(self) -> Result<[u32; 4], MixError> {
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0..=1e6
                let w = self.normalised()?.shares().map(|s| (s * WEIGHT_SCALE).round() as u32);
                Ok(w)
            }
        }

        impl FromStr for $name {
            type Err = MixError;

            /// Parses `name=share,…`, e.g. the CLI's argument; a name left
            /// out gets 0. The shares are validated but not normalised.
            fn from_str(s: &str) -> Result<Self, MixError> {
                let shares = parse($kind, Self::NAMES, s)?;
                normalise(Self::NAMES, shares)?;
                Ok(Self::from_shares(shares))
            }
        }
    };
}

mix!(
    /// The share of each driver profile (ADR-0006 §2); only the ratios
    /// matter.
    DriverMix, "driver profile", "drivers",
    [aggressive = 0.15, normal = 0.60, cautious = 0.20, erratic = 0.05]
);

mix!(
    /// The share of each vehicle class (ADR-0007 §2); only the ratios
    /// matter.
    FleetMix, "vehicle class", "vehicles",
    [car = 0.85, van = 0.08, truck = 0.05, bus = 0.02]
);

// The core draws in `Profile::ALL` and `Class::ALL` order.
const _: () = {
    use rusthour_core::behavior::profile::Profile;
    use rusthour_core::vehicle::class::Class;
    assert!(matches!(
        Profile::ALL,
        [
            Profile::Aggressive,
            Profile::Normal,
            Profile::Cautious,
            Profile::Erratic
        ]
    ));
    assert!(matches!(
        Class::ALL,
        [Class::Car, Class::Van, Class::Truck, Class::Bus]
    ));
};
