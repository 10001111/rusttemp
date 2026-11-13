//! End-to-end tests of the `gpu` executable against mock Linux DRM trees.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;
use tempfile::TempDir;

fn bin() -> PathBuf {
    let built = PathBuf::from(env!("CARGO_BIN_EXE_gpu"));
    if built.exists() {
        return built;
    }
    let exe = std::env::current_exe().unwrap();
    exe.parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(format!("gpu{}", std::env::consts::EXE_SUFFIX))
}

fn write(root: &Path, rel: &str, content: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, content).unwrap();
}

fn run(root: &Path, args: &[&str]) -> Output {
    Command::new(bin())
        .arg("--sysfs-root")
        .arg(root)
        .args(args)
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run gpu")
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn intel_igpu(r: &Path) {
    write(r, "sys/class/drm/card0/device/vendor", "0x8086\n");
    write(r, "sys/class/drm/card0/device/device", "0xa7a8\n");
    write(
        r,
        "sys/class/drm/card0/device/uevent",
        "DRIVER=i915\nPCI_SLOT_NAME=0000:00:02.0\n",
    );
}

fn amd_card(r: &Path) {
    let c = "sys/class/drm/card1/device";
    write(r, &format!("{c}/vendor"), "0x1002\n");
    write(r, &format!("{c}/device"), "0x73df\n");
    write(
        r,
        &format!("{c}/uevent"),
        "DRIVER=amdgpu\nPCI_SLOT_NAME=0000:03:00.0\n",
    );
    write(r, &format!("{c}/gpu_busy_percent"), "42\n");
    write(r, &format!("{c}/mem_info_vram_total"), "12884901888\n");
    write(r, &format!("{c}/mem_info_vram_used"), "2147483648\n");
    write(r, &format!("{c}/hwmon/hwmon3/temp1_label"), "edge\n");
    write(r, &format!("{c}/hwmon/hwmon3/temp1_input"), "61000\n");
    write(r, &format!("{c}/hwmon/hwmon3/fan1_input"), "1320\n");
    write(
        r,
        &format!("{c}/hwmon/hwmon3/power1_average"),
        "123000000\n",
    );
}

#[test]
fn not_detected_without_any_gpu() {
    let t = TempDir::new().unwrap();
    let o = run(t.path(), &[]);
    assert!(o.status.success());
    assert!(stdout(&o).contains("Dedicated GPU: Not detected."));
}

#[test]
fn integrated_gpu_is_never_shown_as_dedicated() {
    let t = TempDir::new().unwrap();
    intel_igpu(t.path());
    let s = stdout(&run(t.path(), &[]));
    assert!(s.contains("Dedicated GPU: Not detected."), "{s}");
    assert!(s.contains("integrated GPU is reported by `cpu`"), "{s}");
    assert!(!s.contains("Xe-LP") && !s.contains("i915"), "{s}");
    let s = stdout(&run(t.path(), &["--all"]));
    assert!(s.contains("INTEGRATED GPU 1 (shown with --all)"), "{s}");
    assert!(s.contains("Dedicated GPU: Not detected."), "{s}");
}

#[test]
fn unclassified_card_gets_its_own_section_with_sensors() {
    let t = TempDir::new().unwrap();
    intel_igpu(t.path());
    amd_card(t.path());
    let s = stdout(&run(t.path(), &[]));
    assert!(s.contains("UNCLASSIFIED GPU 1"), "{s}");
    assert!(s.contains("not as a dedicated GPU"), "{s}");
    assert!(s.contains("GPU Utilization:      42%"), "{s}");
    assert!(s.contains("GPU Temperature:      61.0 °C"), "{s}");
    assert!(s.contains("Fan Speed:            1320 RPM"), "{s}");
    assert!(s.contains("GPU Power:            123.00 W"), "{s}");
    assert!(
        s.contains("VRAM:                 2.00 GiB used of 12.00 GiB"),
        "{s}"
    );
    // Not in the tree: unavailable, never 0.
    assert!(s.contains("Memory Frequency:     Unavailable"), "{s}");
    let v: Value = serde_json::from_str(&stdout(&run(t.path(), &["--json"]))).unwrap();
    assert_eq!(v["command"], "gpu");
    assert_eq!(v["dedicated"].as_array().unwrap().len(), 0);
    assert_eq!(v["unclassified"][0]["kind"], "unknown");
    assert_eq!(v["unclassified"][0]["utilization_pct"]["value"], 42.0);
    assert_eq!(
        v["unclassified"][0]["memory_clock_mhz"]["status"],
        "unavailable"
    );
    assert_eq!(v["integrated_count"], 1);
}

#[test]
fn nvidia_card_without_nvml_is_explained() {
    let t = TempDir::new().unwrap();
    let c = "sys/class/drm/card0/device";
    write(t.path(), &format!("{c}/vendor"), "0x10de\n");
    write(
        t.path(),
        &format!("{c}/uevent"),
        "DRIVER=nvidia\nPCI_SLOT_NAME=0000:01:00.0\n",
    );
    let o = run(t.path(), &["--json"]);
    assert!(o.status.success());
    let v: Value = serde_json::from_str(&stdout(&o)).unwrap();
    let g = v["dedicated"]
        .as_array()
        .unwrap()
        .iter()
        .chain(v["unclassified"].as_array().unwrap())
        .next()
        .expect("the NVIDIA card is listed")
        .clone();
    // Without an NVIDIA driver on the test machine NVML is missing; the
    // reason must say so instead of inventing values.
    if g["utilization_pct"]["status"] == "unavailable" {
        assert!(
            g["utilization_pct"]["reason"]
                .as_str()
                .unwrap()
                .contains("NVML"),
            "{g}"
        );
    }
}

#[test]
fn watch_json_and_invalid_arguments() {
    let t = TempDir::new().unwrap();
    amd_card(t.path());
    let o = run(
        t.path(),
        &["--watch", "--interval", "0.25", "--count", "2", "--json"],
    );
    assert!(o.status.success());
    let lines: Vec<Value> = stdout(&o)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[1]["sample"], 2);
    for args in [
        &["--interval", "2"][..],
        &["--bogus"],
        &["--watch", "--interval", "0"],
    ] {
        assert_eq!(run(t.path(), args).status.code(), Some(2), "{args:?}");
    }
}
