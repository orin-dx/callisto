//! Whole-second durations written as `90s`, `6m` or `1h`.

use std::time::Duration;

/// The longest duration a config value may name.
pub const MAX_DURATION: Duration = Duration::from_secs(3600);

/// Parses `<digits><s|m|h>`; zero and anything over [`MAX_DURATION`] are rejected.
pub fn parse_duration(text: &str) -> Option<Duration> {
    let (digits, unit) = text.split_at(text.len().checked_sub(1)?);
    let seconds_per_unit = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        _ => return None,
    };
    if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let duration = Duration::from_secs(digits.parse::<u64>().ok()?.checked_mul(seconds_per_unit)?);
    (!duration.is_zero() && duration <= MAX_DURATION).then_some(duration)
}

/// The shortest exact spelling: `6m` for 360 seconds, `90s` for 90.
pub fn render_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds != 0 && seconds.is_multiple_of(3600) {
        format!("{}h", seconds / 3600)
    } else if seconds != 0 && seconds.is_multiple_of(60) {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_unit_parses_to_whole_seconds() {
        assert_eq!(parse_duration("90s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("6m"), Some(Duration::from_secs(360)));
        assert_eq!(parse_duration("1h"), Some(Duration::from_secs(3600)));
    }

    #[test]
    fn zero_oversize_and_malformed_values_are_rejected() {
        for bad in [
            "",
            "0s",
            "61m",
            "2h",
            "6",
            "m",
            "-5m",
            "1.5m",
            " 6m",
            "6 m",
            "6M",
            "99999999999999999999s",
        ] {
            assert_eq!(parse_duration(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn rendering_round_trips_through_parsing() {
        for seconds in [1, 59, 60, 90, 120, 360, 3600] {
            let duration = Duration::from_secs(seconds);
            assert_eq!(parse_duration(&render_duration(duration)), Some(duration));
        }
        assert_eq!(render_duration(Duration::from_secs(360)), "6m");
        assert_eq!(render_duration(Duration::from_secs(90)), "90s");
    }
}
