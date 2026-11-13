//! User-configurable thermal warning thresholds.
//!
//! These are generic thresholds chosen by the user (or the defaults below).
//! They are NOT processor-specific safety limits: maximum junction
//! temperatures differ between CPU models and RustTemp does not look them up.
//! Where the driver exposes its own limits they are shown separately as
//! hardware-reported values.

use serde::Serialize;

use crate::errors::{Error, Result};

/// Default "elevated" threshold in °C.
pub const DEFAULT_ELEVATED_C: f64 = 70.0;
/// Default "high" threshold in °C.
pub const DEFAULT_HIGH_C: f64 = 85.0;
/// Default "critical" threshold in °C.
pub const DEFAULT_CRITICAL_C: f64 = 95.0;

/// Warning level of a reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    Normal,
    Elevated,
    High,
    Critical,
}

impl Level {
    /// Upper-case label used in the terminal.
    pub fn label(self) -> &'static str {
        match self {
            Level::Normal => "NORMAL",
            Level::Elevated => "ELEVATED",
            Level::High => "HIGH",
            Level::Critical => "CRITICAL",
        }
    }
}

/// Lower bounds (inclusive, °C) of the elevated, high and critical levels.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Thresholds {
    pub elevated_c: f64,
    pub high_c: f64,
    pub critical_c: f64,
}

impl Default for Thresholds {
    fn default() -> Self {
        Thresholds {
            elevated_c: DEFAULT_ELEVATED_C,
            high_c: DEFAULT_HIGH_C,
            critical_c: DEFAULT_CRITICAL_C,
        }
    }
}

impl Thresholds {
    /// Validates that all values are finite and strictly increasing.
    pub fn new(elevated_c: f64, high_c: f64, critical_c: f64) -> Result<Self> {
        for (name, v) in [
            ("elevated", elevated_c),
            ("high", high_c),
            ("critical", critical_c),
        ] {
            if !v.is_finite() {
                return Err(Error::InvalidConfig(format!(
                    "--{name} must be a finite temperature in °C (got {v})"
                )));
            }
        }
        if !(elevated_c < high_c && high_c < critical_c) {
            return Err(Error::InvalidConfig(format!(
                "thresholds must satisfy elevated < high < critical \
                 (got {elevated_c} / {high_c} / {critical_c} °C)"
            )));
        }
        Ok(Thresholds {
            elevated_c,
            high_c,
            critical_c,
        })
    }

    /// Warning level for a temperature in °C.
    pub fn classify(&self, celsius: f64) -> Level {
        if celsius >= self.critical_c {
            Level::Critical
        } else if celsius >= self.high_c {
            Level::High
        } else if celsius >= self.elevated_c {
            Level::Elevated
        } else {
            Level::Normal
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_classification_and_boundaries() {
        let t = Thresholds::default();
        assert_eq!(t.classify(-10.0), Level::Normal);
        assert_eq!(t.classify(69.999), Level::Normal);
        assert_eq!(t.classify(70.0), Level::Elevated);
        assert_eq!(t.classify(84.9), Level::Elevated);
        assert_eq!(t.classify(85.0), Level::High);
        assert_eq!(t.classify(94.99), Level::High);
        assert_eq!(t.classify(95.0), Level::Critical);
        assert_eq!(t.classify(120.0), Level::Critical);
    }

    #[test]
    fn custom_thresholds() {
        let t = Thresholds::new(50.0, 60.0, 65.5).unwrap();
        assert_eq!(t.classify(49.0), Level::Normal);
        assert_eq!(t.classify(55.0), Level::Elevated);
        assert_eq!(t.classify(65.0), Level::High);
        assert_eq!(t.classify(65.5), Level::Critical);
    }

    #[test]
    fn invalid_thresholds_are_rejected() {
        assert!(Thresholds::new(80.0, 70.0, 90.0).is_err());
        assert!(Thresholds::new(70.0, 70.0, 90.0).is_err());
        assert!(Thresholds::new(70.0, 85.0, 85.0).is_err());
        assert!(Thresholds::new(f64::NAN, 85.0, 95.0).is_err());
        assert!(Thresholds::new(70.0, f64::INFINITY, 95.0).is_err());
    }

    #[test]
    fn levels_are_ordered() {
        assert!(Level::Normal < Level::Elevated);
        assert!(Level::High < Level::Critical);
    }
}
