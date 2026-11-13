//! Sampling loop and monitoring state.

pub mod stats;
pub mod thresholds;

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime};

use crate::errors::Result;
use crate::hardware::gpu::GpuDevice;
use crate::perf::{PerfSample, PerfSource, PerfStats};
use crate::sensors::gpu::{GpuSample, GpuSource};
use crate::sensors::{Reading, SensorBackend, SensorInfo, ThrottleCounters, ThrottleFlags};
pub use stats::SensorStats;
pub use thresholds::{Level, Thresholds};

/// Owns a backend and accumulates statistics across samples.
pub struct Monitor {
    backend: Box<dyn SensorBackend>,
    stats: Vec<SensorStats>,
    readings: Vec<Reading>,
    started: Instant,
    samples: u64,
    last_sample_at: SystemTime,
    notes: Vec<String>,
    thresholds: Thresholds,
    flags: Vec<Option<ThrottleFlags>>,
    counters: Option<ThrottleCounters>,
    throttle_samples: u64,
    perf: Option<Box<dyn PerfSource>>,
    perf_sample: Option<PerfSample>,
    perf_stats: PerfStats,
    gpu: Option<Box<dyn GpuSource>>,
    gpu_devices: Vec<GpuDevice>,
    gpu_samples: Vec<GpuSample>,
}

/// A view of the monitor state right after a sample.
pub struct Snapshot<'a> {
    /// Backend name.
    pub backend: &'static str,
    /// Wall-clock time of the sample.
    pub at: SystemTime,
    /// Time since monitoring started.
    pub elapsed: Duration,
    /// 1-based index of this sample.
    pub sample: u64,
    pub sensors: &'a [SensorInfo],
    pub readings: &'a [Reading],
    pub stats: &'a [SensorStats],
    /// Backend diagnostic notes (e.g. why CPU sensors are unavailable).
    pub notes: &'a [String],
    /// Throttle flags per sensor from this sample (empty = not supported).
    pub flags: &'a [Option<ThrottleFlags>],
    /// OS throttle counters from this sample.
    pub counters: Option<ThrottleCounters>,
    /// Samples so far in which any CPU sensor reported active thermal throttling.
    pub throttle_samples: u64,
    /// Speed, load and power from this sample, if a source is attached.
    pub perf: Option<&'a PerfSample>,
    /// Session statistics for speed, load and power.
    pub perf_stats: &'a PerfStats,
    /// Identified graphics devices (empty when no GPU source is attached).
    pub gpu_devices: &'a [GpuDevice],
    /// GPU readings from this sample, aligned with `gpu_devices`.
    pub gpu_samples: &'a [GpuSample],
}

impl Monitor {
    /// Starts monitoring the sensors discovered by `backend`; `thresholds`
    /// are used to count time spent at each warning level.
    pub fn new(backend: Box<dyn SensorBackend>, thresholds: Thresholds) -> Self {
        let n = backend.sensors().len();
        let notes = backend.notes();
        Monitor {
            backend,
            stats: vec![SensorStats::default(); n],
            readings: Vec::with_capacity(n),
            started: Instant::now(),
            samples: 0,
            last_sample_at: SystemTime::now(),
            notes,
            thresholds,
            flags: Vec::new(),
            counters: None,
            throttle_samples: 0,
            perf: None,
            perf_sample: None,
            perf_stats: PerfStats::default(),
            gpu: None,
            gpu_devices: Vec::new(),
            gpu_samples: Vec::new(),
        }
    }

    /// Attaches a GPU sensor source; its notes join the backend notes.
    pub fn with_gpu(mut self, gpu: Option<Box<dyn GpuSource>>) -> Self {
        if let Some(g) = &gpu {
            self.notes.extend(g.notes());
            self.gpu_devices = g.devices().to_vec();
        }
        self.gpu = gpu;
        self
    }

    /// True when a GPU source is attached.
    pub fn has_gpu(&self) -> bool {
        self.gpu.is_some()
    }

    /// Attaches a speed/load/power source; its notes join the backend notes.
    pub fn with_perf(mut self, perf: Option<Box<dyn PerfSource>>) -> Self {
        if let Some(p) = &perf {
            self.notes.extend(p.notes());
        }
        self.perf = perf;
        self
    }

    /// True when a speed/load/power source is attached.
    pub fn has_perf(&self) -> bool {
        self.perf.is_some()
    }

    /// Discovered sensors.
    pub fn sensors(&self) -> &[SensorInfo] {
        self.backend.sensors()
    }

    /// Backend name.
    pub fn backend_name(&self) -> &'static str {
        self.backend.name()
    }

    /// Backend diagnostic notes.
    pub fn notes(&self) -> &[String] {
        &self.notes
    }

    /// Reads every sensor once and updates statistics.
    pub fn sample(&mut self) -> Snapshot<'_> {
        let at = SystemTime::now();
        let mut readings = self.backend.read();
        // Defend against a backend breaking the one-reading-per-sensor contract.
        readings.resize(
            self.stats.len(),
            Reading::Unavailable("backend returned no reading".into()),
        );
        let mut flags = self.backend.throttle_flags();
        if !flags.is_empty() {
            flags.resize(self.stats.len(), None);
        }
        let mut any_cpu_throttling = false;
        for (i, (stat, reading)) in self.stats.iter_mut().zip(&readings).enumerate() {
            stat.record(reading, at);
            if let Some(c) = reading.celsius() {
                stat.record_level(self.thresholds.classify(c));
            }
            if flags
                .get(i)
                .copied()
                .flatten()
                .is_some_and(|f| f.thermal_now)
            {
                stat.record_thermal_throttle();
                if self.backend.sensors()[i].kind.is_cpu() {
                    any_cpu_throttling = true;
                }
            }
        }
        if any_cpu_throttling {
            self.throttle_samples += 1;
        }
        self.flags = flags;
        self.counters = self.backend.throttle_counters();
        if let Some(g) = &mut self.gpu {
            self.gpu_samples = g.sample();
        }
        if let Some(p) = &mut self.perf {
            let ps = p.sample();
            self.perf_stats.record(&ps);
            self.perf_sample = Some(ps);
        }
        self.readings = readings;
        self.samples += 1;
        self.last_sample_at = at;
        self.snapshot()
    }

    /// The state after the most recent sample.
    pub fn snapshot(&self) -> Snapshot<'_> {
        Snapshot {
            backend: self.backend.name(),
            at: self.last_sample_at,
            elapsed: self.started.elapsed(),
            sample: self.samples,
            sensors: self.backend.sensors(),
            readings: &self.readings,
            stats: &self.stats,
            notes: &self.notes,
            flags: &self.flags,
            counters: self.counters,
            throttle_samples: self.throttle_samples,
            perf: self.perf_sample.as_ref(),
            perf_stats: &self.perf_stats,
            gpu_devices: &self.gpu_devices,
            gpu_samples: &self.gpu_samples,
        }
    }

    /// Samples every `interval` until a message (or disconnection) arrives on
    /// `stop`, or `max_samples` samples have been taken. `on_sample` is called
    /// after every sample; an error from it ends the loop. Returns the number
    /// of samples taken.
    pub fn run<F>(
        &mut self,
        interval: Duration,
        max_samples: Option<u64>,
        stop: &Receiver<()>,
        mut on_sample: F,
    ) -> Result<u64>
    where
        F: FnMut(&Snapshot<'_>) -> Result<()>,
    {
        let mut taken = 0u64;
        loop {
            let tick = Instant::now();
            on_sample(&self.sample())?;
            taken += 1;
            if max_samples.is_some_and(|max| taken >= max) {
                break;
            }
            // Keep a steady cadence: subtract the time spent sampling/rendering.
            let wait = interval.saturating_sub(tick.elapsed());
            match stop.recv_timeout(wait) {
                Err(RecvTimeoutError::Timeout) => continue,
                Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
            }
        }
        Ok(taken)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sensors::{HardwareLimits, SensorKind};
    use std::collections::VecDeque;
    use std::sync::mpsc;

    /// Backend replaying scripted readings; repeats the last frame when exhausted.
    struct ScriptedBackend {
        sensors: Vec<SensorInfo>,
        frames: VecDeque<Vec<Reading>>,
        last: Vec<Reading>,
    }

    impl ScriptedBackend {
        fn new(n: usize, frames: Vec<Vec<Reading>>) -> Self {
            let sensors = (0..n)
                .map(|i| SensorInfo {
                    id: format!("mock/temp{i}"),
                    label: format!("Core {i}"),
                    chip: "mock".into(),
                    kind: SensorKind::CpuCore,
                    source: "mock".into(),
                    limits: HardwareLimits::default(),
                })
                .collect();
            ScriptedBackend {
                sensors,
                frames: frames.into(),
                last: vec![Reading::Unavailable("no data".into()); n],
            }
        }
    }

    impl SensorBackend for ScriptedBackend {
        fn name(&self) -> &'static str {
            "scripted"
        }
        fn sensors(&self) -> &[SensorInfo] {
            &self.sensors
        }
        fn read(&mut self) -> Vec<Reading> {
            if let Some(f) = self.frames.pop_front() {
                self.last = f;
            }
            self.last.clone()
        }
    }

    fn c(v: f64) -> Reading {
        Reading::Celsius(v)
    }

    #[test]
    fn sample_updates_statistics_per_sensor() {
        let backend = ScriptedBackend::new(
            2,
            vec![
                vec![c(40.0), c(50.0)],
                vec![c(44.0), Reading::Unavailable("x".into())],
            ],
        );
        let mut m = Monitor::new(Box::new(backend), Thresholds::default());
        m.sample();
        let snap = m.sample();
        assert_eq!(snap.sample, 2);
        assert_eq!(snap.stats[0].average(), Some(42.0));
        assert_eq!(snap.stats[0].max(), Some(44.0));
        assert_eq!(snap.stats[1].current(), None);
        assert_eq!(snap.stats[1].max(), Some(50.0));
        assert!(matches!(snap.readings[1], Reading::Unavailable(_)));
    }

    #[test]
    fn run_stops_after_max_samples() {
        let (_tx, rx) = mpsc::channel();
        let mut m = Monitor::new(
            Box::new(ScriptedBackend::new(1, vec![vec![c(30.0)]])),
            Thresholds::default(),
        );
        let mut seen = Vec::new();
        let n = m
            .run(Duration::from_millis(1), Some(5), &rx, |s| {
                seen.push(s.sample);
                Ok(())
            })
            .unwrap();
        assert_eq!(n, 5);
        assert_eq!(seen, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn stop_signal_sent_during_sampling_ends_loop() {
        let (tx, rx) = mpsc::channel();
        let mut m = Monitor::new(
            Box::new(ScriptedBackend::new(1, vec![vec![c(30.0)]])),
            Thresholds::default(),
        );
        let n = m
            .run(Duration::from_millis(5), None, &rx, |s| {
                if s.sample == 3 {
                    tx.send(()).unwrap();
                }
                Ok(())
            })
            .unwrap();
        assert_eq!(n, 3);
    }

    #[test]
    fn disconnected_stop_channel_ends_loop() {
        let (tx, rx) = mpsc::channel::<()>();
        drop(tx);
        let mut m = Monitor::new(
            Box::new(ScriptedBackend::new(1, vec![vec![c(30.0)]])),
            Thresholds::default(),
        );
        let n = m
            .run(Duration::from_secs(60), None, &rx, |_| Ok(()))
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn pending_stop_signal_ends_loop_immediately() {
        let (tx, rx) = mpsc::channel();
        tx.send(()).unwrap();
        let mut m = Monitor::new(
            Box::new(ScriptedBackend::new(1, vec![vec![c(30.0)]])),
            Thresholds::default(),
        );
        let start = Instant::now();
        let n = m
            .run(Duration::from_secs(60), None, &rx, |_| Ok(()))
            .unwrap();
        assert_eq!(n, 1);
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn signal_from_another_thread_interrupts_wait() {
        let (tx, rx) = mpsc::channel();
        let mut m = Monitor::new(
            Box::new(ScriptedBackend::new(1, vec![vec![c(30.0)]])),
            Thresholds::default(),
        );
        let handle = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            tx.send(()).unwrap();
        });
        let start = Instant::now();
        let n = m
            .run(Duration::from_secs(30), None, &rx, |_| Ok(()))
            .unwrap();
        handle.join().unwrap();
        assert_eq!(n, 1);
        assert!(start.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn callback_error_ends_loop() {
        let (_tx, rx) = mpsc::channel();
        let mut m = Monitor::new(
            Box::new(ScriptedBackend::new(1, vec![vec![c(30.0)]])),
            Thresholds::default(),
        );
        let r = m.run(Duration::from_millis(1), None, &rx, |s| {
            if s.sample == 3 {
                Err(crate::Error::Platform("boom".into()))
            } else {
                Ok(())
            }
        });
        assert!(r.is_err());
        assert_eq!(m.snapshot().sample, 3);
    }

    #[test]
    fn interval_is_respected() {
        let (_tx, rx) = mpsc::channel();
        let mut m = Monitor::new(
            Box::new(ScriptedBackend::new(1, vec![vec![c(30.0)]])),
            Thresholds::default(),
        );
        let start = Instant::now();
        m.run(Duration::from_millis(100), Some(4), &rx, |_| Ok(()))
            .unwrap();
        // 4 samples => 3 waits of ~100 ms.
        let e = start.elapsed();
        assert!(e >= Duration::from_millis(290), "{e:?}");
        assert!(e < Duration::from_secs(3), "{e:?}");
    }
}
