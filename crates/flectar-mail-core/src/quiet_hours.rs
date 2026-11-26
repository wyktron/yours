//! The do-not-disturb window for every sound and spoken line the app makes.
//!
//! The window is two times on the local clock and may wrap midnight: the
//! 22:00-08:00 default is quiet late in the evening and again until morning.
//! The start is inclusive and the end exclusive, so 22:00 is quiet and 08:00
//! is not. A malformed time falls back to the default for that end of the
//! window, so a hand-typed value can neither silence the app around the clock
//! nor leave it loud all night by accident.

use chrono::{NaiveTime, Timelike};

use crate::models::Settings;

/// When quiet hours begin unless the user chose another time.
pub const DEFAULT_START: &str = "22:00";
/// When quiet hours end unless the user chose another time.
pub const DEFAULT_END: &str = "08:00";

/// Parse "HH:MM" into the time of day, or `None` when it is not one.
///
/// One or two digits are accepted for each part, so "8:05" and "08:05" both
/// name 08:05. Seconds, signs and suffixes are not accepted.
pub fn parse_hhmm(value: &str) -> Option<NaiveTime> {
    let (hours, minutes) = value.trim().split_once(':')?;
    if !(1..=2).contains(&hours.len()) || !(1..=2).contains(&minutes.len()) {
        return None;
    }
    if !hours.bytes().all(|byte| byte.is_ascii_digit())
        || !minutes.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let hours: u32 = hours.parse().ok()?;
    let minutes: u32 = minutes.parse().ok()?;
    NaiveTime::from_hms_opt(hours, minutes, 0)
}

/// A time of day in the "HH:MM" form Settings stores.
pub fn format_hhmm(time: NaiveTime) -> String {
    format!("{:02}:{:02}", time.hour(), time.minute())
}

/// `value` as Settings stores a time, or `default` when it is malformed.
pub fn normalize_hhmm(value: &str, default: &str) -> String {
    parse_hhmm(value)
        .or_else(|| parse_hhmm(default))
        .map_or_else(|| default.to_owned(), format_hhmm)
}

fn window_start(start: &str) -> NaiveTime {
    parse_hhmm(start)
        .unwrap_or_else(|| parse_hhmm(DEFAULT_START).expect("the default quiet-hours start parses"))
}

fn window_end(end: &str) -> NaiveTime {
    parse_hhmm(end)
        .unwrap_or_else(|| parse_hhmm(DEFAULT_END).expect("the default quiet-hours end parses"))
}

/// Whether `now` falls inside the quiet window that runs from `start`
/// (inclusive) to `end` (exclusive) on the local clock.
///
/// A window whose end is before its start wraps midnight: 22:00-08:00 is
/// quiet from 22:00 to midnight and again until 08:00. A malformed time falls
/// back to the default for that end of the window. A window with equal ends
/// covers nothing, so it is never quiet.
pub fn is_quiet(now: NaiveTime, start: &str, end: &str) -> bool {
    let start = window_start(start);
    let end = window_end(end);
    if start <= end {
        now >= start && now < end
    } else {
        now >= start || now < end
    }
}

/// Whether these settings ask for quiet at `now` on the local clock.
pub fn is_quiet_at(now: NaiveTime, settings: &Settings) -> bool {
    settings.quiet_hours && is_quiet(now, &settings.quiet_start, &settings.quiet_end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(hours: u32, minutes: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(hours, minutes, 0).expect("a valid time of day")
    }

    #[test]
    fn the_default_window_wraps_midnight() {
        assert!(is_quiet(at(22, 0), DEFAULT_START, DEFAULT_END));
        assert!(is_quiet(at(23, 30), DEFAULT_START, DEFAULT_END));
        assert!(is_quiet(at(0, 0), DEFAULT_START, DEFAULT_END));
        assert!(is_quiet(at(7, 59), DEFAULT_START, DEFAULT_END));
        // The end is exclusive and the start is not.
        assert!(!is_quiet(at(8, 0), DEFAULT_START, DEFAULT_END));
        assert!(!is_quiet(at(12, 0), DEFAULT_START, DEFAULT_END));
        assert!(!is_quiet(at(21, 59), DEFAULT_START, DEFAULT_END));
    }

    #[test]
    fn a_window_within_one_day_keeps_its_bounds() {
        assert!(is_quiet(at(9, 0), "09:00", "17:00"));
        assert!(is_quiet(at(16, 59), "09:00", "17:00"));
        assert!(!is_quiet(at(17, 0), "09:00", "17:00"));
        assert!(!is_quiet(at(8, 59), "09:00", "17:00"));
    }

    #[test]
    fn a_window_may_end_at_midnight() {
        assert!(is_quiet(at(23, 59), "22:00", "00:00"));
        assert!(!is_quiet(at(0, 0), "22:00", "00:00"));
        assert!(!is_quiet(at(21, 0), "22:00", "00:00"));
    }

    #[test]
    fn a_window_with_equal_ends_covers_nothing() {
        for now in [at(0, 0), at(8, 0), at(22, 0), at(23, 59)] {
            assert!(!is_quiet(now, "08:00", "08:00"));
        }
    }

    #[test]
    fn malformed_times_fall_back_to_the_default_window() {
        // Both ends malformed: the whole default window is used.
        assert!(is_quiet(at(23, 0), "not a time", "25:00"));
        assert!(is_quiet(at(3, 0), "not a time", "25:00"));
        assert!(!is_quiet(at(12, 0), "not a time", "25:00"));
        // Only the start is malformed: it becomes 22:00.
        assert!(is_quiet(at(23, 0), "", "08:00"));
        assert!(!is_quiet(at(9, 0), "", "08:00"));
        // Only the end is malformed: it becomes 08:00.
        assert!(is_quiet(at(7, 0), "22:00", "later"));
        assert!(!is_quiet(at(12, 0), "22:00", "later"));
    }

    #[test]
    fn times_are_parsed_as_hh_mm_only() {
        assert_eq!(parse_hhmm("22:00"), Some(at(22, 0)));
        assert_eq!(parse_hhmm(" 8:05 "), Some(at(8, 5)));
        assert_eq!(parse_hhmm("08:5"), Some(at(8, 5)));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(parse_hhmm("22:60"), None);
        assert_eq!(parse_hhmm("2200"), None);
        assert_eq!(parse_hhmm("22:00:00"), None);
        assert_eq!(parse_hhmm(""), None);
        assert_eq!(normalize_hhmm("8:5", DEFAULT_START), "08:05");
        assert_eq!(normalize_hhmm("nope", DEFAULT_END), "08:00");
    }

    #[test]
    fn default_settings_are_quiet_from_22_to_08() {
        let settings = Settings::default();
        assert!(settings.quiet_hours);
        assert_eq!(settings.quiet_start, "22:00");
        assert_eq!(settings.quiet_end, "08:00");
        assert!(is_quiet_at(at(23, 0), &settings));
        assert!(!is_quiet_at(at(12, 0), &settings));

        let switched_off = Settings {
            quiet_hours: false,
            ..Settings::default()
        };
        assert!(!is_quiet_at(at(23, 0), &switched_off));
    }
}
