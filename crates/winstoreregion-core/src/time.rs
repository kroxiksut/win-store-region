//! Validated UTC timestamps used by durable records.

/// A UTC timestamp supplied by the operation layer for the recovery record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UtcTimestamp(String);

impl UtcTimestamp {
    /// Validate the RFC 3339 UTC representation used by the recovery schema.
    #[must_use]
    pub fn parse(value: impl Into<String>) -> Option<Self> {
        let value = value.into();
        let bytes = value.as_bytes();
        if bytes.len() < 20 || bytes.last() != Some(&b'Z') {
            return None;
        }

        let has_fixed_separators = bytes.get(4) == Some(&b'-')
            && bytes.get(7) == Some(&b'-')
            && bytes.get(10) == Some(&b'T')
            && bytes.get(13) == Some(&b':')
            && bytes.get(16) == Some(&b':');
        let has_numeric_components =
            [0..4, 5..7, 8..10, 11..13, 14..16, 17..19]
                .into_iter()
                .all(|range| {
                    bytes
                        .get(range)
                        .is_some_and(|part| part.iter().all(u8::is_ascii_digit))
                });
        let fraction = bytes.get(19..bytes.len().saturating_sub(1));
        let has_valid_fraction = fraction.is_some_and(|fraction| {
            fraction.is_empty()
                || (fraction.first() == Some(&b'.')
                    && fraction[1..].iter().all(u8::is_ascii_digit)
                    && fraction.len() > 1)
        });
        if !has_fixed_separators || !has_numeric_components || !has_valid_fraction {
            return None;
        }

        let year = decimal_component(&bytes[0..4]);
        let month = decimal_component(&bytes[5..7]);
        let day = decimal_component(&bytes[8..10]);
        let hour = decimal_component(&bytes[11..13]);
        let minute = decimal_component(&bytes[14..16]);
        let second = decimal_component(&bytes[17..19]);
        let days_in_month = match month {
            1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
            4 | 6 | 9 | 11 => 30,
            2 if year % 400 == 0 || (year % 4 == 0 && year % 100 != 0) => 29,
            2 => 28,
            _ => return None,
        };
        (year > 0 && (1..=days_in_month).contains(&day) && hour < 24 && minute < 60 && second < 60)
            .then_some(Self(value))
    }

    /// Return the validated timestamp for JSON serialization.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Build a timestamp from seconds since the Unix epoch.
    ///
    /// The platform supplies the instant; the calendar arithmetic stays here so
    /// it is testable without a clock. Values before the epoch are not
    /// representable and clamp to it.
    #[must_use]
    pub fn from_unix_seconds(seconds: u64) -> Self {
        let days = i64::try_from(seconds / 86_400).unwrap_or(0);
        let time_of_day = seconds % 86_400;
        let (year, month, day) = civil_from_days(days);
        Self(format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
            time_of_day / 3_600,
            (time_of_day % 3_600) / 60,
            time_of_day % 60,
        ))
    }
}

/// Convert days since the Unix epoch into a proleptic Gregorian date.
///
/// This is the inverse of the days-since-epoch computation used when reading
/// Windows event timestamps, kept in one place so both directions agree.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (
        year,
        u32::try_from(month).unwrap_or(1),
        u32::try_from(day).unwrap_or(1),
    )
}

fn decimal_component(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0, |value, byte| {
        value * 10 + u32::from(byte.saturating_sub(b'0'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_timestamps_require_utc_rfc_3339_text() {
        assert!(UtcTimestamp::parse("2026-08-15T12:34:56Z").is_some());
        assert!(UtcTimestamp::parse("2026-08-15T12:34:56.123Z").is_some());
        assert_eq!(UtcTimestamp::parse("2026-08-15T12:34:56+03:00"), None);
        assert_eq!(UtcTimestamp::parse("text-textTtext:text:textZ"), None);
        assert_eq!(UtcTimestamp::parse("2026-02-29T12:34:56Z"), None);
    }

    #[test]
    fn unix_seconds_render_as_a_timestamp_this_crate_accepts() {
        for (seconds, expected) in [
            (0_u64, "1970-01-01T00:00:00Z"),
            (1, "1970-01-01T00:00:01Z"),
            (86_399, "1970-01-01T23:59:59Z"),
            (86_400, "1970-01-02T00:00:00Z"),
            // A leap day, so the calendar arithmetic cannot drift silently.
            (1_709_209_496, "2024-02-29T12:24:56Z"),
            (1_755_433_496, "2025-08-17T12:24:56Z"),
        ] {
            let timestamp = UtcTimestamp::from_unix_seconds(seconds);
            assert_eq!(timestamp.as_str(), expected);
            assert_eq!(
                UtcTimestamp::parse(timestamp.as_str()).as_ref(),
                Some(&timestamp),
                "a generated timestamp must pass this crate's own validation"
            );
        }
    }
}
