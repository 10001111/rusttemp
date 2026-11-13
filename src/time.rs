//! Time formatting helpers.
//!
//! Times are shown in the local time zone by default (with an explicit UTC
//! offset label) or in UTC with `--utc`. Local-time conversion uses the
//! operating system (`SystemTimeToTzSpecificLocalTime` on Windows,
//! `localtime_r` on Unix), so daylight-saving rules for each instant are
//! applied by the OS. If conversion fails RustTemp falls back to UTC and says
//! so in the label. JSON timestamps are always UTC (plus a local variant).

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Which clock to display.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Zone {
    /// Local time zone of the machine.
    #[default]
    Local,
    /// Coordinated Universal Time.
    Utc,
}

/// Milliseconds since the Unix epoch (0 for times before 1970).
pub fn unix_millis(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Offset of local time from UTC at instant `t`, in minutes, if the OS can
/// tell.
pub fn utc_offset_minutes(t: SystemTime) -> Option<i32> {
    platform::utc_offset_minutes(t)
}

/// Offset to apply for `zone` (None = UTC fallback).
fn offset_for(t: SystemTime, zone: Zone) -> Option<i32> {
    match zone {
        Zone::Utc => None,
        Zone::Local => utc_offset_minutes(t),
    }
}

/// `HH:MM:SS` in the requested zone (falls back to UTC).
pub fn format_clock(t: SystemTime, zone: Zone) -> String {
    let local_ms = shifted_millis(t, offset_for(t, zone).unwrap_or(0));
    let day_secs = (local_ms.div_euclid(1000)).rem_euclid(86_400);
    format!(
        "{:02}:{:02}:{:02}",
        day_secs / 3600,
        (day_secs % 3600) / 60,
        day_secs % 60
    )
}

/// Label for the zone used by [`format_clock`]: `UTC` or `UTC+01:00`.
pub fn zone_label(t: SystemTime, zone: Zone) -> String {
    match offset_for(t, zone) {
        None => "UTC".to_string(),
        Some(off) => format!("UTC{}", format_offset(off)),
    }
}

/// `HH:MM:SS` in UTC.
pub fn format_utc_clock(t: SystemTime) -> String {
    format_clock(t, Zone::Utc)
}

/// RFC 3339 timestamp in UTC with millisecond precision, e.g. `2026-11-12T09:34:00.123Z`.
pub fn format_rfc3339_utc(t: SystemTime) -> String {
    format_rfc3339(unix_millis(t) as i64, "Z")
}

/// RFC 3339 timestamp at the given UTC offset, e.g. `2026-11-12T03:34:00.123-06:00`.
pub fn format_rfc3339_with_offset(t: SystemTime, offset_minutes: i32) -> String {
    format_rfc3339(
        shifted_millis(t, offset_minutes),
        &format_offset(offset_minutes),
    )
}

/// Local RFC 3339 timestamp, if the local offset is known.
pub fn format_rfc3339_local(t: SystemTime) -> Option<String> {
    utc_offset_minutes(t).map(|off| format_rfc3339_with_offset(t, off))
}

/// `HH:MM:SS` for an elapsed duration (hours may exceed 24).
pub fn format_duration(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

fn shifted_millis(t: SystemTime, offset_minutes: i32) -> i64 {
    unix_millis(t) as i64 + offset_minutes as i64 * 60_000
}

fn format_offset(minutes: i32) -> String {
    let sign = if minutes < 0 { '-' } else { '+' };
    let m = minutes.unsigned_abs();
    format!("{sign}{:02}:{:02}", m / 60, m % 60)
}

fn format_rfc3339(ms: i64, suffix: &str) -> String {
    let secs = ms.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let day_secs = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{:03}{suffix}",
        day_secs / 3600,
        (day_secs % 3600) / 60,
        day_secs % 60,
        ms.rem_euclid(1000)
    )
}

/// Converts days since 1970-01-01 into a proleptic Gregorian (year, month, day).
///
/// Algorithm from Howard Hinnant, "chrono-Compatible Low-Level Date Algorithms".
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Inverse of [`civil_from_days`].
#[cfg_attr(not(windows), allow(dead_code))]
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(windows)]
mod platform {
    use super::{civil_from_days, days_from_civil, unix_millis};
    use std::ptr;
    use std::time::SystemTime;
    use windows_sys::Win32::Foundation::SYSTEMTIME;
    use windows_sys::Win32::System::Time::SystemTimeToTzSpecificLocalTime;

    pub fn utc_offset_minutes(t: SystemTime) -> Option<i32> {
        let secs = (unix_millis(t) / 1000) as i64;
        let (y, m, d) = civil_from_days(secs.div_euclid(86_400));
        let ds = secs.rem_euclid(86_400);
        let utc = SYSTEMTIME {
            wYear: u16::try_from(y).ok()?,
            wMonth: m as u16,
            wDayOfWeek: 0,
            wDay: d as u16,
            wHour: (ds / 3600) as u16,
            wMinute: ((ds % 3600) / 60) as u16,
            wSecond: (ds % 60) as u16,
            wMilliseconds: 0,
        };
        let mut local = SYSTEMTIME::default();
        // SAFETY: valid pointers; null selects the active time zone.
        if unsafe { SystemTimeToTzSpecificLocalTime(ptr::null(), &utc, &mut local) } == 0 {
            return None;
        }
        let local_secs =
            days_from_civil(local.wYear as i64, local.wMonth as u32, local.wDay as u32) * 86_400
                + local.wHour as i64 * 3600
                + local.wMinute as i64 * 60
                + local.wSecond as i64;
        i32::try_from((local_secs - secs) / 60).ok()
    }
}

#[cfg(unix)]
mod platform {
    use super::unix_millis;
    use std::time::SystemTime;

    pub fn utc_offset_minutes(t: SystemTime) -> Option<i32> {
        // Cast via inference to avoid naming `libc::time_t` (deprecated alias on musl).
        let secs = (unix_millis(t) / 1000) as _;
        // SAFETY: zeroed `tm` is a valid out-parameter for localtime_r.
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        // SAFETY: valid pointers; localtime_r is thread-safe.
        if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
            return None;
        }
        i32::try_from(tm.tm_gmtoff / 60).ok()
    }
}

#[cfg(not(any(windows, unix)))]
mod platform {
    pub fn utc_offset_minutes(_t: std::time::SystemTime) -> Option<i32> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(ms)
    }

    #[test]
    fn epoch_formats() {
        assert_eq!(format_rfc3339_utc(at(0)), "1970-01-01T00:00:00.000Z");
        assert_eq!(format_utc_clock(at(0)), "00:00:00");
    }

    #[test]
    fn known_timestamps() {
        // 2000-02-29T12:34:56.789Z (leap day)
        assert_eq!(
            format_rfc3339_utc(at(951_827_696_789)),
            "2000-02-29T12:34:56.789Z"
        );
        // 2026-11-12T09:34:00.000Z
        assert_eq!(
            format_rfc3339_utc(at(1_794_476_040_000)),
            "2026-11-12T09:34:00.000Z"
        );
        assert_eq!(format_utc_clock(at(1_794_476_040_000)), "09:34:00");
    }

    #[test]
    fn offsets() {
        let t = at(1_794_476_040_123);
        assert_eq!(
            format_rfc3339_with_offset(t, -360),
            "2026-11-12T03:34:00.123-06:00"
        );
        assert_eq!(
            format_rfc3339_with_offset(t, 330),
            "2026-11-12T15:04:00.123+05:30"
        );
        // Crossing midnight backwards.
        assert_eq!(
            format_rfc3339_with_offset(at(3_600_000), -120),
            "1969-12-31T23:00:00.000-02:00"
        );
        assert_eq!(format_offset(0), "+00:00");
    }

    #[test]
    fn days_round_trip() {
        for days in [-800_000i64, -1, 0, 1, 11_016, 20_769, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days, "{y}-{m}-{d}");
        }
    }

    #[test]
    fn utc_zone_label() {
        assert_eq!(zone_label(at(0), Zone::Utc), "UTC");
    }

    /// The OS must report a local offset, and the local clock must equal UTC
    /// shifted by it.
    #[test]
    fn local_time_is_consistent_with_offset() {
        let now = SystemTime::now();
        let off = utc_offset_minutes(now).expect("OS should report a UTC offset");
        assert!(off.abs() <= 14 * 60, "offset {off} out of range");
        let label = zone_label(now, Zone::Local);
        assert!(
            label.starts_with("UTC+") || label.starts_with("UTC-"),
            "{label}"
        );
        let local = format_rfc3339_local(now).unwrap();
        assert!(local.ends_with(&format_offset(off)), "{local}");
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(Duration::from_secs(0)), "00:00:00");
        assert_eq!(format_duration(Duration::from_secs(3_723)), "01:02:03");
        assert_eq!(format_duration(Duration::from_secs(90_000)), "25:00:00");
    }
}
