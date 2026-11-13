//! End-to-end tests of the `rusttemp` binary against mock sysfs trees.
//!
//! These run on any OS: `--sysfs-root` points the Linux sysfs backend at a
//! temporary directory, so no physical sensors are required.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

/// Path of the binary under test. Falls back to a path relative to the test
/// executable so test binaries cross-compiled on another host still work.
fn bin() -> PathBuf {
    let built = PathBuf::from(env!("CARGO_BIN_EXE_rusttemp"));
    if built.exists() {
        return built;
    }
    let exe = std::env::current_exe().unwrap();
    let dir = exe.parent().unwrap().parent().unwrap();
    dir.join(format!("rusttemp{}", std::env::consts::EXE_SUFFIX))
}

fn write(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

/// Intel-style tree: coretemp package + 2 cores, an NVMe drive, and an
/// acpitz zone that is also registered as hwmon.
fn mock_tree() -> TempDir {
    let t = TempDir::new().unwrap();
    let r = t.path();
    write(r, "sys/class/hwmon/hwmon0/name", "acpitz\n");
    write(r, "sys/class/hwmon/hwmon0/temp1_input", "30000\n");
    write(r, "sys/class/hwmon/hwmon1/name", "nvme\n");
    write(r, "sys/class/hwmon/hwmon1/temp1_label", "Composite\n");
    write(r, "sys/class/hwmon/hwmon1/temp1_input", "38850\n");
    write(r, "sys/class/hwmon/hwmon4/name", "coretemp\n");
    write(r, "sys/class/hwmon/hwmon4/temp1_label", "Package id 0\n");
    write(r, "sys/class/hwmon/hwmon4/temp1_input", "52000\n");
    write(r, "sys/class/hwmon/hwmon4/temp1_crit", "100000\n");
    write(r, "sys/class/hwmon/hwmon4/temp2_label", "Core 0\n");
    write(r, "sys/class/hwmon/hwmon4/temp2_input", "86500\n");
    write(r, "sys/class/hwmon/hwmon4/temp3_label", "Core 1\n");
    write(r, "sys/class/hwmon/hwmon4/temp3_input", "garbage\n");
    write(r, "sys/class/thermal/thermal_zone0/type", "acpitz\n");
    write(r, "sys/class/thermal/thermal_zone0/temp", "30000\n");
    t
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .arg("--sysfs-root")
        .arg(root)
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run rusttemp")
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8(o.stderr.clone()).unwrap()
}

#[test]
fn sensors_lists_identity_information() {
    let t = mock_tree();
    let o = run(t.path(), &["--sensors"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let s = stdout(&o);
    assert!(s.contains("5 sensor(s) discovered via linux-sysfs"), "{s}");
    assert!(s.contains("Package id 0"));
    assert!(s.contains("CPU package (CPU sensor: yes)"));
    assert!(s.contains("ACPI thermal zone (location not confirmed) (CPU sensor: no)"));
    assert!(s.contains("crit 100.0 °C"));
}

#[test]
fn sensors_json() {
    let t = mock_tree();
    let o = run(t.path(), &["--sensors", "--json"]);
    assert!(o.status.success());
    let v: Value = serde_json::from_str(&stdout(&o)).expect("valid JSON");
    assert_eq!(v["backend"], "linux-sysfs");
    assert_eq!(v["sensor_count"], 5);
    assert_eq!(v["cpu_sensor_count"], 3);
    let kinds: Vec<&str> = v["sensors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["kind"].as_str().unwrap())
        .collect();
    assert_eq!(
        kinds,
        [
            "acpi_thermal_zone",
            "other",
            "cpu_package",
            "cpu_core",
            "cpu_core"
        ]
    );
}

#[test]
fn once_shows_readings_levels_and_unavailable_sensors() {
    let t = mock_tree();
    let o = run(t.path(), &["--once"]);
    assert!(o.status.success());
    let s = stdout(&o);
    assert!(s.contains("CPU SENSORS"));
    assert!(s.contains("OTHER SENSORS"));
    assert!(s.contains("52.0 °C"));
    assert!(s.contains("86.5 °C"));
    assert!(s.contains("HIGH"));
    assert!(s.contains("UNAVAILABLE: invalid sensor value \"garbage\""));
    assert!(!s.contains('\x1b'), "no escapes when stdout is a pipe");
}

#[test]
fn once_json_is_valid_and_uses_null_for_unavailable() {
    let t = mock_tree();
    let o = run(
        t.path(),
        &[
            "--once",
            "--json",
            "--elevated",
            "50",
            "--high",
            "60",
            "--critical",
            "86",
        ],
    );
    assert!(o.status.success(), "{}", stderr(&o));
    let v: Value = serde_json::from_str(&stdout(&o)).expect("valid JSON");
    let sensors = v["sensors"].as_array().unwrap();
    assert_eq!(sensors.len(), 5);
    let pkg = &sensors[2];
    assert_eq!(pkg["label"], "Package id 0");
    assert_eq!(pkg["temperature_c"], 52.0);
    assert_eq!(pkg["level"], "elevated");
    assert_eq!(sensors[3]["level"], "critical");
    let broken = &sensors[4];
    assert_eq!(broken["status"], "unavailable");
    assert!(broken["temperature_c"].is_null());
    assert!(broken["error"].as_str().unwrap().contains("garbage"));
    assert_eq!(v["thresholds"]["critical_c"], 86.0);
}

#[test]
fn monitoring_emits_one_json_line_per_sample_and_terminates() {
    let t = mock_tree();
    let start = Instant::now();
    let o = run(t.path(), &["--json", "--count", "3", "--interval", "200"]);
    let elapsed = start.elapsed();
    assert!(o.status.success(), "{}", stderr(&o));
    let lines: Vec<Value> = stdout(&o)
        .lines()
        .map(|l| serde_json::from_str(l).expect("each line is a JSON document"))
        .collect();
    assert_eq!(lines.len(), 3);
    for (i, v) in lines.iter().enumerate() {
        assert_eq!(v["sample"], i as u64 + 1);
    }
    assert_eq!(lines[2]["sensors"][2]["stats"]["valid_samples"], 3);
    assert_eq!(lines[2]["sensors"][4]["stats"]["unavailable_samples"], 3);
    assert!(elapsed >= Duration::from_millis(400), "{elapsed:?}");
}

#[test]
fn monitoring_tracks_changing_values() {
    let t = mock_tree();
    let input = t.path().join("sys/class/hwmon/hwmon4/temp1_input");
    fs::write(&input, "40000\n").unwrap();
    let child = Command::new(bin())
        .arg("--sysfs-root")
        .arg(t.path())
        .args(["--json", "--count", "2", "--interval", "2000"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(900));
    fs::write(&input, "60000\n").unwrap();
    let o = child.wait_with_output().unwrap();
    assert!(o.status.success());
    let lines: Vec<Value> = stdout(&o)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines[0]["sensors"][2]["temperature_c"], 40.0);
    let pkg = &lines[1]["sensors"][2];
    assert_eq!(pkg["temperature_c"], 60.0);
    assert_eq!(pkg["stats"]["min_c"], 40.0);
    assert_eq!(pkg["stats"]["max_c"], 60.0);
    assert_eq!(pkg["stats"]["avg_c"], 50.0);
}

#[test]
fn text_monitoring_prints_summary() {
    let t = mock_tree();
    let o = run(t.path(), &["--count", "2", "--interval", "100"]);
    assert!(o.status.success());
    let s = stdout(&o);
    assert_eq!(s.matches("CPU SENSORS").count(), 2);
    assert!(s.contains("MIN"));
    assert!(s.contains("Monitoring stopped after"));
    assert!(s.contains("(2 samples)"));
}

#[test]
fn empty_system_reports_no_sensors_without_inventing_data() {
    let t = TempDir::new().unwrap();
    write(t.path(), "sys/class/hwmon/hwmon0/name", "AC1\n");
    let o = run(t.path(), &["--once"]);
    assert!(o.status.success());
    assert!(stdout(&o).contains("No temperature sensors were found"));
    let o = run(t.path(), &["--once", "--json"]);
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    assert_eq!(v["sensors"].as_array().unwrap().len(), 0);
}

#[test]
fn invalid_arguments_fail_with_usage_errors() {
    let t = mock_tree();
    for args in [
        &["--interval", "abc"][..],
        &["--interval", "10"],
        &["--bogus"],
        &["--once", "--sensors"],
        &["--count", "0"],
    ] {
        let o = run(t.path(), args);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        assert!(stderr(&o).contains("error"), "{args:?}: {}", stderr(&o));
        assert!(stdout(&o).is_empty());
    }
    let o = run(t.path(), &["--interval", "50"]);
    assert!(stderr(&o).contains("100..=3600000"), "{}", stderr(&o));
}

#[test]
fn invalid_thresholds_fail_with_exit_code_2() {
    let t = mock_tree();
    let o = run(t.path(), &["--elevated", "90", "--high", "80"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("elevated < high < critical"));
}

#[test]
fn missing_sysfs_root_fails_cleanly() {
    let t = TempDir::new().unwrap();
    let o = run(&t.path().join("missing"), &["--once"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("rusttemp: error:"));
}

#[test]
fn help_documents_every_option() {
    let o = Command::new(bin()).arg("--help").output().unwrap();
    assert!(o.status.success());
    let s = stdout(&o);
    for opt in [
        "--once",
        "--sensors",
        "--interval",
        "--json",
        "--count",
        "--elevated",
        "--high",
        "--critical",
        "--no-color",
        "--help",
        "--version",
    ] {
        assert!(s.contains(opt), "help is missing {opt}");
    }
}

/// Ctrl+C delivers SIGINT on Unix; the monitor must stop promptly and exit 0.
#[cfg(unix)]
#[test]
fn sigint_stops_monitoring_gracefully() {
    let t = mock_tree();
    let child = Command::new(bin())
        .arg("--sysfs-root")
        .arg(t.path())
        .args(["--interval", "60000"])
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(500));
    let start = Instant::now();
    let status = Command::new("kill")
        .args(["-INT", &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let o = child.wait_with_output().unwrap();
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(o.status.success(), "{:?}", o.status);
    let s = stdout(&o);
    assert!(s.contains("Monitoring stopped after"), "{s}");
    assert!(s.contains("(1 samples)"));
}

#[test]
fn utc_flag_and_local_time_labels() {
    let t = mock_tree();
    let utc = stdout(&run(t.path(), &["--once", "--utc"]));
    let header = utc.lines().next().unwrap();
    assert!(header.ends_with(" UTC"), "{header}");
    let local = stdout(&run(t.path(), &["--once"]));
    let header = local.lines().next().unwrap();
    let label = header.rsplit(' ').next().unwrap();
    assert!(
        label.len() == 9 && (label.starts_with("UTC+") || label.starts_with("UTC-")),
        "local header should carry an explicit offset: {header}"
    );
    assert!(local.contains("UPDATED"));
}

#[test]
fn json_has_utc_and_local_timestamps_and_notes() {
    let t = mock_tree();
    let v: Value = serde_json::from_str(&stdout(&run(t.path(), &["--once", "--json"]))).unwrap();
    assert!(v["timestamp"].as_str().unwrap().ends_with('Z'));
    let local = v["timestamp_local"].as_str().unwrap();
    assert!(
        local.len() == "2026-11-12T09:34:00.000+00:00".len(),
        "{local}"
    );
    assert!(v["notes"].as_array().unwrap().is_empty());
    let v: Value = serde_json::from_str(&stdout(&run(t.path(), &["--sensors", "--json"]))).unwrap();
    assert!(v["notes"].is_array());
}

/// Adds /proc/cpuinfo, caches and kernel throttle counters to a mock tree.
fn add_cpu_files(root: &Path) {
    let mut cpuinfo = String::new();
    for (cpu, core) in [(0, 0), (1, 0), (2, 1), (3, 1)] {
        cpuinfo.push_str(&format!(
            "processor\t: {cpu}\nvendor_id\t: GenuineIntel\ncpu family\t: 6\nmodel\t\t: 154\n\
             model name\t: 12th Gen Intel(R) Core(TM) i5-1240P\nstepping\t: 3\n\
             microcode\t: 0x430\nphysical id\t: 0\ncore id\t\t: {core}\n\n"
        ));
    }
    write(root, "proc/cpuinfo", &cpuinfo);
    for cpu in 0..4 {
        let base = format!("sys/devices/system/cpu/cpu{cpu}");
        write(root, &format!("{base}/cache/index3/level"), "3");
        write(root, &format!("{base}/cache/index3/type"), "Unified");
        write(root, &format!("{base}/cache/index3/size"), "12288K");
        write(root, &format!("{base}/cache/index3/shared_cpu_list"), "0-3");
        write(root, &format!("{base}/cpufreq/base_frequency"), "1700000");
        write(root, &format!("{base}/cpufreq/cpuinfo_max_freq"), "4400000");
        write(
            root,
            &format!("{base}/thermal_throttle/core_throttle_count"),
            if cpu == 2 { "7" } else { "0" },
        );
        write(
            root,
            &format!("{base}/thermal_throttle/package_throttle_count"),
            "3",
        );
    }
}

#[test]
fn info_shows_identity_limit_and_throttle_counters() {
    let t = mock_tree();
    add_cpu_files(t.path());
    let o = run(t.path(), &["--info"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let s = stdout(&o);
    assert!(s.contains("Manufacturer    Intel (GenuineIntel)"), "{s}");
    assert!(s.contains("Model           12th Gen Intel(R) Core(TM) i5-1240P"));
    assert!(s.contains("family 6 · model 154 (0x9A) · stepping 3"));
    assert!(s.contains("Microcode       0x430"));
    assert!(s.contains("2 physical · 4 logical · 1 package"));
    assert!(s.contains("L3: 12 MiB"));
    assert!(s.contains("base 1700 MHz · max 4400 MHz"));
    assert!(s.contains("Thermal limit   100.0 °C"));
    assert!(s.contains("CPU HEALTH"));
    assert!(s.contains("since boot: package 3 · core up to 7 per CPU (1 of 4 CPUs affected)"));
    // Hottest CPU sensor is Core 0 at 86.5 °C with crit unknown for cores in the mock.
    assert!(
        s.contains("Hottest CPU sensor      Core 0 at 86.5 °C"),
        "{s}"
    );
}

#[test]
fn info_json_is_valid() {
    let t = mock_tree();
    add_cpu_files(t.path());
    let o = run(t.path(), &["--info", "--json"]);
    assert!(o.status.success());
    let v: Value = serde_json::from_str(&stdout(&o)).expect("valid JSON");
    assert_eq!(v["cpu"]["vendor"], "Intel");
    assert_eq!(v["cpu"]["brand"], "12th Gen Intel(R) Core(TM) i5-1240P");
    assert_eq!(v["cpu"]["physical_cores"], 2);
    assert_eq!(v["cpu"]["caches"][0]["instances"][0]["size_kib"], 12288);
    assert_eq!(v["thermal_limit_c"], 100.0);
    assert_eq!(v["health"]["flags_available"], false);
    assert_eq!(v["health"]["throttle_counters"]["package_events"], 3);
    assert_eq!(v["health"]["hottest_cpu_sensor"]["label"], "Core 0");
}

#[test]
fn once_and_monitor_include_cpu_line_and_health() {
    let t = mock_tree();
    add_cpu_files(t.path());
    let s = stdout(&run(t.path(), &["--once"]));
    assert!(
        s.contains("CPU: 12th Gen Intel(R) Core(TM) i5-1240P · Intel · 2 cores / 4 threads"),
        "{s}"
    );
    assert!(s.contains("CPU HEALTH"));
    assert!(
        !s.contains("This session"),
        "single readings have no session line"
    );
    let s = stdout(&run(t.path(), &["--count", "2", "--interval", "100"]));
    assert!(
        s.contains("This session            thermal throttling in 0 of 2 samples"),
        "{s}"
    );
    let v: Value = serde_json::from_str(&stdout(&run(t.path(), &["--once", "--json"]))).unwrap();
    assert_eq!(v["cpu"]["vendor"], "Intel");
    assert_eq!(v["health"]["session"]["samples"], 1);
    // coretemp Package id 0 has crit 100 -> headroom 48 at 52 °C.
    assert_eq!(v["sensors"][2]["headroom_c"], 48.0);
    assert!(v["sensors"][2]["throttle"].is_null());
    assert_eq!(v["sensors"][2]["stats"]["level_samples"]["normal"], 1);
}

#[test]
fn info_conflicts_with_other_modes() {
    let t = mock_tree();
    let o = run(t.path(), &["--info", "--once"]);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn speed_and_power_section_from_mock_tree() {
    let t = mock_tree();
    let r = t.path();
    for (cpu, core, khz) in [(0, 0, 3_800_000), (1, 0, 3_700_000), (2, 1, 1_200_000)] {
        let b = format!("sys/devices/system/cpu/cpu{cpu}");
        write(r, &format!("{b}/topology/physical_package_id"), "0");
        write(r, &format!("{b}/topology/core_id"), &core.to_string());
        write(
            r,
            &format!("{b}/cpufreq/scaling_cur_freq"),
            &khz.to_string(),
        );
        write(r, &format!("{b}/cpufreq/cpuinfo_max_freq"), "4400000");
    }
    let s = stdout(&run(r, &["--once"]));
    assert!(s.contains("CPU SPEED & POWER"), "{s}");
    assert!(
        s.contains("Package power           unavailable (no RAPL powercap zones"),
        "{s}"
    );
    assert!(s.contains("avg 2500 MHz · max turbo 4400 MHz"), "{s}");
    assert!(s.contains("Core 0   3800 MHz"), "{s}");
    let v: Value = serde_json::from_str(&stdout(&run(r, &["--once", "--json"]))).unwrap();
    assert_eq!(v["perf"]["cores"][1]["mhz"], 1200.0);
    assert!(v["perf"]["package_w"].is_null());
    assert_eq!(v["perf"]["speed_source"], "cpufreq scaling_cur_freq");
    let v: Value = serde_json::from_str(&stdout(&run(r, &["--info", "--json"]))).unwrap();
    assert_eq!(v["perf"]["max_turbo_mhz"], 4400.0);
}

#[test]
fn cpu_and_gpu_subcommands_run_the_same_reports() {
    let t = mock_tree();
    let o = run_sub(t.path(), "cpu");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(stdout(&o).contains("RUST HARDWARE MONITOR — CPU"));
    let o = run_sub(t.path(), "gpu");
    assert!(o.status.success());
    assert!(stdout(&o).contains("Dedicated GPU: Not detected."));
}

fn run_sub(root: &Path, sub: &str) -> Output {
    Command::new(bin())
        .arg(sub)
        .arg("--sysfs-root")
        .arg(root)
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run rusttemp")
}
