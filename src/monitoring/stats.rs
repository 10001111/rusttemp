//! Per-sensor running statistics.

use std::time::SystemTime;

use super::thresholds::Level;
use crate::sensors::Reading;

/// Running statistics over the valid readings of one sensor.
///
/// Unavailable readings are counted but never contribute to min/max/average,
/// and they clear `current` so a stale value is never shown as current.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SensorStats {
    current: Option<f64>,
    min: Option<f64>,
    max: Option<f64>,
    sum: f64,
    valid_samples: u64,
    unavailable_samples: u64,
    last_valid_at: Option<SystemTime>,
    level_samples: [u64; 4],
    thermal_throttle_samples: u64,
}

impl SensorStats {
    /// Records one reading taken at `at`.
    pub fn record(&mut self, reading: &Reading, at: SystemTime) {
        match reading.celsius() {
            Some(c) => {
                self.current = Some(c);
                self.min = Some(self.min.map_or(c, |m| m.min(c)));
                self.max = Some(self.max.map_or(c, |m| m.max(c)));
                self.sum += c;
                self.valid_samples += 1;
                self.last_valid_at = Some(at);
            }
            None => {
                self.current = None;
                self.unavailable_samples += 1;
            }
        }
    }

    /// Counts a valid reading at warning `level`.
    pub fn record_level(&mut self, level: Level) {
        self.level_samples[level as usize] += 1;
    }

    /// Counts a sample in which the CPU reported active thermal throttling.
    pub fn record_thermal_throttle(&mut self) {
        self.thermal_throttle_samples += 1;
    }

    /// Valid samples per level: `[normal, elevated, high, critical]`.
    pub fn level_samples(&self) -> [u64; 4] {
        self.level_samples
    }

    /// Percentage of valid samples at or above `level`.
    pub fn percent_at_least(&self, level: Level) -> Option<f64> {
        (self.valid_samples > 0).then(|| {
            let n: u64 = self.level_samples[level as usize..].iter().sum();
            n as f64 * 100.0 / self.valid_samples as f64
        })
    }

    /// Samples in which this sensor reported active thermal throttling.
    pub fn thermal_throttle_samples(&self) -> u64 {
        self.thermal_throttle_samples
    }

    /// Most recent reading, if it was valid.
    pub fn current(&self) -> Option<f64> {
        self.current
    }
    /// Lowest valid reading.
    pub fn min(&self) -> Option<f64> {
        self.min
    }
    /// Highest valid reading.
    pub fn max(&self) -> Option<f64> {
        self.max
    }
    /// Arithmetic mean of all valid readings.
    pub fn average(&self) -> Option<f64> {
        (self.valid_samples > 0).then(|| self.sum / self.valid_samples as f64)
    }
    /// Number of valid readings.
    pub fn valid_samples(&self) -> u64 {
        self.valid_samples
    }
    /// Number of failed readings.
    pub fn unavailable_samples(&self) -> u64 {
        self.unavailable_samples
    }
    /// Time of the most recent valid reading.
    pub fn last_valid_at(&self) -> Option<SystemTime> {
        self.last_valid_at
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, UNIX_EPOCH};

    fn t(s: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(s)
    }

    #[test]
    fn empty_stats() {
        let s = SensorStats::default();
        assert_eq!(s.current(), None);
        assert_eq!(s.min(), None);
        assert_eq!(s.max(), None);
        assert_eq!(s.average(), None);
        assert_eq!(s.valid_samples(), 0);
    }

    #[test]
    fn min_max_average() {
        let mut s = SensorStats::default();
        for (i, c) in [50.0, 42.5, 61.0, 48.5].into_iter().enumerate() {
            s.record(&Reading::Celsius(c), t(i as u64));
        }
        assert_eq!(s.current(), Some(48.5));
        assert_eq!(s.min(), Some(42.5));
        assert_eq!(s.max(), Some(61.0));
        assert_eq!(s.average(), Some(50.5)); // (50 + 42.5 + 61 + 48.5) / 4
        assert_eq!(s.valid_samples(), 4);
        assert_eq!(s.last_valid_at(), Some(t(3)));
    }

    #[test]
    fn single_and_negative_values() {
        let mut s = SensorStats::default();
        s.record(&Reading::Celsius(-10.0), t(0));
        assert_eq!(
            (s.min(), s.max(), s.average()),
            (Some(-10.0), Some(-10.0), Some(-10.0))
        );
        s.record(&Reading::Celsius(-20.0), t(1));
        assert_eq!(s.min(), Some(-20.0));
        assert_eq!(s.max(), Some(-10.0));
        assert_eq!(s.average(), Some(-15.0));
    }

    #[test]
    fn unavailable_readings_do_not_skew_statistics() {
        let mut s = SensorStats::default();
        s.record(&Reading::Celsius(40.0), t(0));
        s.record(&Reading::Unavailable("io".into()), t(1));
        assert_eq!(
            s.current(),
            None,
            "stale value must not be shown as current"
        );
        assert_eq!(s.min(), Some(40.0));
        assert_eq!(s.average(), Some(40.0));
        assert_eq!(s.last_valid_at(), Some(t(0)));
        s.record(&Reading::Celsius(60.0), t(2));
        assert_eq!(s.current(), Some(60.0));
        assert_eq!(s.average(), Some(50.0));
        assert_eq!(s.valid_samples(), 2);
        assert_eq!(s.unavailable_samples(), 1);
    }

    #[test]
    fn only_unavailable_readings() {
        let mut s = SensorStats::default();
        s.record(&Reading::Unavailable("x".into()), t(0));
        assert_eq!(s.average(), None);
        assert_eq!(s.min(), None);
        assert_eq!(s.last_valid_at(), None);
        assert_eq!(s.unavailable_samples(), 1);
    }

    #[test]
    fn level_and_throttle_counters() {
        let mut s = SensorStats::default();
        assert_eq!(s.percent_at_least(Level::High), None);
        for l in [Level::Normal, Level::Normal, Level::Elevated, Level::High] {
            s.record(&Reading::Celsius(1.0), t(0));
            s.record_level(l);
        }
        s.record_thermal_throttle();
        assert_eq!(s.level_samples(), [2, 1, 1, 0]);
        assert_eq!(s.percent_at_least(Level::Normal), Some(100.0));
        assert_eq!(s.percent_at_least(Level::Elevated), Some(50.0));
        assert_eq!(s.percent_at_least(Level::High), Some(25.0));
        assert_eq!(s.percent_at_least(Level::Critical), Some(0.0));
        assert_eq!(s.thermal_throttle_samples(), 1);
    }

    #[test]
    fn long_run_average_stays_accurate() {
        let mut s = SensorStats::default();
        for i in 0..100_000u64 {
            let c = if i % 2 == 0 { 45.125 } else { 54.875 };
            s.record(&Reading::Celsius(c), t(i));
        }
        assert!((s.average().unwrap() - 50.0).abs() < 1e-9);
    }
}
