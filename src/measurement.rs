//! A value that was measured, could not be measured, or does not apply.
//!
//! Every hardware value in the `cpu` / `gpu` reports is a [`Measurement`].
//! This makes "unavailable", "not applicable" and a genuine zero different
//! values, so a missing sensor can never be shown as `0`. It also attaches
//! the source of every number.
//!
//! JSON form:
//! - `{"status":"measured","value":45.2,"unit":"W","source":"RAPL via PawnIO"}`
//! - `{"status":"unavailable","reason":"..."}`
//! - `{"status":"not_applicable","reason":"..."}`

use std::borrow::Cow;

use serde::Serialize;

/// Outcome of trying to obtain one hardware value.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Measurement<T> {
    /// A value read from the hardware or OS.
    Measured {
        value: T,
        /// Unit symbol (`"°C"`, `"W"`, `"MHz"`, ...); empty for counts and text.
        unit: &'static str,
        /// Where the value came from.
        source: Cow<'static, str>,
    },
    /// The value exists in principle but could not be obtained here.
    Unavailable { reason: String },
    /// The value does not exist for this device (e.g. VRAM clock of an iGPU).
    NotApplicable { reason: String },
}

impl<T> Measurement<T> {
    /// A measured value.
    pub fn measured(value: T, unit: &'static str, source: impl Into<Cow<'static, str>>) -> Self {
        Measurement::Measured {
            value,
            unit,
            source: source.into(),
        }
    }

    /// An unavailable value with the reason.
    pub fn unavailable(reason: impl Into<String>) -> Self {
        Measurement::Unavailable {
            reason: reason.into(),
        }
    }

    /// A value that does not apply, with the reason.
    pub fn not_applicable(reason: impl Into<String>) -> Self {
        Measurement::NotApplicable {
            reason: reason.into(),
        }
    }

    /// Measured when `value` is `Some`, otherwise unavailable with `reason()`.
    pub fn from_option(
        value: Option<T>,
        unit: &'static str,
        source: impl Into<Cow<'static, str>>,
        reason: impl FnOnce() -> String,
    ) -> Self {
        match value {
            Some(v) => Measurement::measured(v, unit, source),
            None => Measurement::unavailable(reason()),
        }
    }

    /// The value, if measured.
    pub fn value(&self) -> Option<&T> {
        match self {
            Measurement::Measured { value, .. } => Some(value),
            _ => None,
        }
    }

    /// The source, if measured.
    pub fn source(&self) -> Option<&str> {
        match self {
            Measurement::Measured { source, .. } => Some(source),
            _ => None,
        }
    }

    /// True when measured.
    pub fn is_measured(&self) -> bool {
        matches!(self, Measurement::Measured { .. })
    }
}

impl<T: Copy> Measurement<T> {
    /// The value, if measured, by copy.
    pub fn get(&self) -> Option<T> {
        self.value().copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn zero_is_not_unavailable() {
        let zero = Measurement::measured(0.0, "W", "test");
        let missing: Measurement<f64> = Measurement::unavailable("no sensor");
        assert_eq!(zero.get(), Some(0.0));
        assert_eq!(missing.get(), None);
        assert_ne!(zero, missing);
    }

    #[test]
    fn json_forms() {
        let m = Measurement::measured(45.25, "W", "RAPL via PawnIO");
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            json!({"status":"measured","value":45.25,"unit":"W","source":"RAPL via PawnIO"})
        );
        let u: Measurement<f64> = Measurement::unavailable("needs administrator");
        assert_eq!(
            serde_json::to_value(&u).unwrap(),
            json!({"status":"unavailable","reason":"needs administrator"})
        );
        let n: Measurement<f64> = Measurement::not_applicable("uses system memory");
        assert_eq!(
            serde_json::to_value(&n).unwrap(),
            json!({"status":"not_applicable","reason":"uses system memory"})
        );
    }

    #[test]
    fn from_option() {
        let m = Measurement::from_option(Some(3), "", "x", || unreachable!());
        assert_eq!(m.get(), Some(3));
        assert_eq!(m.source(), Some("x"));
        let n: Measurement<i32> = Measurement::from_option(None, "", "x", || "why".into());
        assert_eq!(n, Measurement::unavailable("why"));
        assert!(!n.is_measured());
    }
}
