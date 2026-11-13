//! End-to-end tests of the `cpu` executable against mock Linux sensor trees.
//! They need no real hardware and run on any OS.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

fn bin() -> PathBuf {
    let built = PathBuf::from(env!("CARGO_BIN_EXE_cpu"));
    if built.exists() {
        return built;
    }
    let exe = std::env::current_exe().unwrap();
    exe.parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(format!("cpu{}", std::env::consts::EXE_SUFFIX))
}

fn write(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

/// Intel laptop: coretemp package + 2 cores, cpuinfo, topology, cpufreq,
/// /proc/stat and kernel throttle counters.
fn tree() -> TempDir {
    let t = TempDir::new().unwrap();
    let r = t.path();
    write(r, "sys/class/hwmon/hwmon0/name", "coretemp\n");
    write(r, "sys/class/hwmon/hwmon0/temp1_label", "Package id 0\n");
    write(r, "sys/class/hwmon/hwmon0/temp1_input", "61000\n");
    write(r, "sys/class/hwmon/hwmon0/temp1_crit", "100000\n");
    write(r, "sys/class/hwmon/hwmon0/temp2_label", "Core 0\n");
    write(r, "sys/class/hwmon/hwmon0/temp2_input", "58000\n");
    write(r, "sys/class/hwmon/hwmon0/temp3_label", "Core 1\n");
    write(r, "sys/class/hwmon/hwmon0/temp3_input", "88000\n");
    let mut cpuinfo = String::new();
    for (cpu, core) in [(0, 0), (1, 0), (2, 1), (3, 1)] {
        cpuinfo.push_str(&format!(
            "processor\t: {cpu}\nvendor_id\t: GenuineIntel\ncpu family\t: 6\nmodel\t\t: 154\n\
             model name\t: 12th Gen Intel(R) Core(TM) i5-1240P\nstepping\t: 3\n\
             physical id\t: 0\ncore id\t\t: {core}\n\n"
        ));
        let b = format!("sys/devices/system/cpu/cpu{cpu}");
        write(r, &format!("{b}/topology/physical_package_id"), "0");
        write(r, &format!("{b}/topology/core_id"), &core.to_string());
        write(r, &format!("{b}/cpufreq/scaling_cur_freq"), "3000000");
        write(r, &format!("{b}/cpufreq/base_frequency"), "1700000");
        write(r, &format!("{b}/cpufreq/cpuinfo_max_freq"), "4400000");
        write(r, &format!("{b}/thermal_throttle/core_throttle_count"), "0");
        write(
            r,
            &format!("{b}/thermal_throttle/package_throttle_count"),
            "2",
        );
    }
    write(r, "proc/cpuinfo", &cpuinfo);
    write(
        r,
        "proc/stat",
        "cpu  10 0 10 80 0 0 0 0\ncpu0 1 0 1 8\ncpu1 1 0 1 8\ncpu2 1 0 1 8\ncpu3 1 0 1 8\n",
    );
    t
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .arg("--sysfs-root")
        .arg(root)
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run cpu")
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

#[test]
fn report_sections_and_values() {
    let t = tree();
    let o = run(t.path(), &[]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let s = stdout(&o);
    for section in [
        "RUST HARDWARE MONITOR — CPU",
        "CPU INFORMATION",
        "TEMPERATURE",
        "PER-CORE",
        "POWER & VOLTAGE",
        "THROTTLING",
    ] {
        assert!(s.contains(section), "missing {section}\n{s}");
    }
    assert!(
        s.contains("Manufacturer:         Intel (GenuineIntel)  [/proc/cpuinfo]"),
        "{s}"
    );
    assert!(s.contains("Microarchitecture:    Alder Lake"), "{s}");
    assert!(s.contains("Physical Cores:       2"), "{s}");
    assert!(
        s.contains("Base Frequency:       1700 MHz  [cpufreq base_frequency]"),
        "{s}"
    );
    assert!(s.contains("Max Boost Frequency:  4400 MHz"), "{s}");
    assert!(
        s.contains("CPU Temperature:      61.0 °C  [coretemp: Package id 0]"),
        "{s}"
    );
    assert!(s.contains("Thermal Limit:        100.0 °C"), "{s}");
    assert!(s.contains("Sensors:              3 CPU sensors"), "{s}");
    // No RAPL in the tree: power is unavailable with a reason, never 0.
    assert!(
        s.contains("Package Power:        Unavailable — no RAPL powercap zones"),
        "{s}"
    );
    assert!(!s.contains("0.0 W"), "{s}");
    assert!(
        s.contains("Core Voltage:         Unavailable — not exposed through sysfs"),
        "{s}"
    );
    assert!(
        s.contains("Thermal Throttling:   Unknown (no live flag) (has occurred since boot)"),
        "{s}"
    );
    assert!(s.contains("Power History:        Unavailable"), "{s}");
}

#[test]
fn json_is_valid_and_marks_unavailable_values() {
    let t = tree();
    let o = run(t.path(), &["--json"]);
    assert!(o.status.success());
    let v: Value = serde_json::from_str(&stdout(&o)).expect("valid JSON");
    assert_eq!(v["command"], "cpu");
    assert_eq!(v["identification"]["manufacturer"]["status"], "measured");
    assert_eq!(v["identification"]["physical_cores"]["value"], 2);
    assert_eq!(v["temperature"]["package_c"]["value"], 61.0);
    assert_eq!(v["temperature"]["package_c"]["unit"], "°C");
    assert_eq!(v["power"]["package_w"]["status"], "unavailable");
    assert!(
        v["power"]["package_w"]["reason"]
            .as_str()
            .unwrap()
            .contains("RAPL")
    );
    assert!(v["power"]["package_w"].get("value").is_none());
    assert_eq!(v["throttling"]["thermal"]["value"]["since_boot"], true);
    assert!(v["throttling"]["thermal"]["value"]["now"].is_null());
}

#[test]
fn watch_with_count_terminates_and_streams_json() {
    let t = tree();
    let o = run(
        t.path(),
        &["--watch", "--interval", "0.25", "--count", "3", "--json"],
    );
    assert!(o.status.success());
    let lines: Vec<Value> = stdout(&o)
        .lines()
        .map(|l| serde_json::from_str(l).expect("NDJSON line"))
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[2]["sample"], 3);
    let o = run(t.path(), &["--watch", "--interval", "0.25", "--count", "2"]);
    let s = stdout(&o);
    assert_eq!(s.matches("CPU INFORMATION").count(), 2);
    assert!(s.contains("refresh 0.25 s"));
    assert!(s.contains("Monitoring stopped after"));
}

#[test]
fn invalid_arguments() {
    let t = tree();
    for args in [
        &["--interval", "2"][..],
        &["--watch", "--interval", "0.1"],
        &["--bogus"],
        &["--count", "2"],
    ] {
        let o = run(t.path(), args);
        assert_eq!(o.status.code(), Some(2), "{args:?}");
        assert!(
            String::from_utf8_lossy(&o.stderr).contains("error"),
            "{args:?}"
        );
    }
    let o = run(t.path(), &["--elevated", "90", "--high", "80"]);
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn help_lists_options() {
    let o = Command::new(bin()).arg("--help").output().unwrap();
    let s = stdout(&o);
    for opt in [
        "--watch",
        "--interval",
        "--count",
        "--json",
        "--no-color",
        "--utc",
        "--elevated",
    ] {
        assert!(s.contains(opt), "help missing {opt}");
    }
    assert!(
        !s.contains("--sysfs-root"),
        "diagnostic option should be hidden"
    );
}

#[test]
fn integrated_gpu_not_detected_line() {
    let t = tree(); // no /sys/class/drm in the tree
    let s = stdout(&run(t.path(), &[]));
    assert!(s.contains("INTEGRATED GPU"), "{s}");
    assert!(s.contains("Integrated GPU: Not detected."), "{s}");
    let v: Value = serde_json::from_str(&stdout(&run(t.path(), &["--json"]))).unwrap();
    assert_eq!(v["integrated_gpu"]["detected"], false);
    assert_eq!(v["integrated_gpu"]["devices"].as_array().unwrap().len(), 0);
}

#[test]
fn integrated_gpu_detected_from_drm_tree() {
    let t = tree();
    let r = t.path();
    write(r, "sys/class/drm/card0/device/vendor", "0x8086\n");
    write(r, "sys/class/drm/card0/device/device", "0xa7a8\n");
    write(
        r,
        "sys/class/drm/card0/device/uevent",
        "DRIVER=i915\nPCI_SLOT_NAME=0000:00:02.0\n",
    );
    write(r, "sys/class/drm/card0/gt_act_freq_mhz", "1250\n");
    // A second, unclassifiable card must not appear in the iGPU section.
    write(r, "sys/class/drm/card1/device/vendor", "0x1002\n");
    write(
        r,
        "sys/class/drm/card1/device/uevent",
        "DRIVER=amdgpu\nPCI_SLOT_NAME=0000:03:00.0\n",
    );
    let s = stdout(&run(r, &[]));
    assert!(s.contains("Detection:            Detected"), "{s}");
    assert!(
        s.contains("Architecture:         Xe-LP (Gen12, Raptor Lake-P graphics)"),
        "{s}"
    );
    assert!(s.contains("Core Frequency:       1250 MHz"), "{s}");
    assert!(s.contains("Memory Frequency:     Not applicable"), "{s}");
    assert!(s.contains("GPU Temperature:      Unavailable"), "{s}");
    assert!(
        s.contains("1 graphics device(s) could not be confirmed"),
        "{s}"
    );
    assert!(
        !s.contains("amdgpu"),
        "unclassified card leaked into the iGPU section\n{s}"
    );
    let v: Value = serde_json::from_str(&stdout(&run(r, &["--json"]))).unwrap();
    let d = &v["integrated_gpu"]["devices"][0];
    assert_eq!(d["kind"], "integrated");
    assert_eq!(d["core_clock_mhz"]["value"], 1250.0);
    assert_eq!(d["memory_clock_mhz"]["status"], "not_applicable");
    assert_eq!(v["integrated_gpu"]["unclassified"], 1);
}
