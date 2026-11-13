//! Combines several backends into one.

use crate::sensors::{Reading, SensorBackend, SensorInfo, ThrottleCounters, ThrottleFlags};

/// Concatenates the sensors of several backends, in order.
pub struct CompositeBackend {
    name: &'static str,
    parts: Vec<Box<dyn SensorBackend>>,
    sensors: Vec<SensorInfo>,
    notes: Vec<String>,
}

impl CompositeBackend {
    /// `notes` describe sources that could not be opened.
    pub fn new(name: &'static str, parts: Vec<Box<dyn SensorBackend>>, notes: Vec<String>) -> Self {
        let sensors = parts
            .iter()
            .flat_map(|p| p.sensors().iter().cloned())
            .collect();
        CompositeBackend {
            name,
            parts,
            sensors,
            notes,
        }
    }
}

impl SensorBackend for CompositeBackend {
    fn name(&self) -> &'static str {
        self.name
    }

    fn sensors(&self) -> &[SensorInfo] {
        &self.sensors
    }

    fn read(&mut self) -> Vec<Reading> {
        let mut out = Vec::with_capacity(self.sensors.len());
        for part in &mut self.parts {
            let n = part.sensors().len();
            let mut r = part.read();
            // Keep alignment with `sensors` even if a part misbehaves.
            r.resize(
                n,
                Reading::Unavailable("backend returned no reading".into()),
            );
            out.extend(r);
        }
        out
    }

    fn notes(&self) -> Vec<String> {
        let mut notes = self.notes.clone();
        for p in &self.parts {
            notes.extend(p.notes());
        }
        notes
    }

    fn throttle_flags(&self) -> Vec<Option<ThrottleFlags>> {
        if self.parts.iter().all(|p| p.throttle_flags().is_empty()) {
            return Vec::new();
        }
        let mut out = Vec::with_capacity(self.sensors.len());
        for part in &self.parts {
            let mut f = part.throttle_flags();
            f.resize(part.sensors().len(), None);
            out.extend(f);
        }
        out
    }

    fn throttle_counters(&self) -> Option<ThrottleCounters> {
        self.parts.iter().find_map(|p| p.throttle_counters())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::{HardwareLimits, SensorKind};

    struct Fixed(Vec<SensorInfo>, Vec<Reading>);

    impl SensorBackend for Fixed {
        fn name(&self) -> &'static str {
            "fixed"
        }
        fn sensors(&self) -> &[SensorInfo] {
            &self.0
        }
        fn read(&mut self) -> Vec<Reading> {
            self.1.clone()
        }
        fn notes(&self) -> Vec<String> {
            vec!["part note".into()]
        }
    }

    fn info(id: &str) -> SensorInfo {
        SensorInfo {
            id: id.into(),
            label: id.into(),
            chip: "x".into(),
            kind: SensorKind::Other,
            source: "x".into(),
            limits: HardwareLimits::default(),
        }
    }

    #[test]
    fn concatenates_sensors_readings_and_notes() {
        let a = Fixed(vec![info("a1"), info("a2")], vec![Reading::Celsius(1.0)]); // one short
        let b = Fixed(vec![info("b1")], vec![Reading::Celsius(3.0)]);
        let mut c =
            CompositeBackend::new("combo", vec![Box::new(a), Box::new(b)], vec!["top".into()]);
        let ids: Vec<&str> = c.sensors().iter().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["a1", "a2", "b1"]);
        let r = c.read();
        assert_eq!(r.len(), 3);
        assert_eq!(r[0], Reading::Celsius(1.0));
        assert!(matches!(r[1], Reading::Unavailable(_)));
        assert_eq!(r[2], Reading::Celsius(3.0));
        assert_eq!(c.notes(), ["top", "part note", "part note"]);
        assert!(c.throttle_flags().is_empty());
        assert_eq!(c.throttle_counters(), None);
        assert_eq!(c.name(), "combo");
    }
}
