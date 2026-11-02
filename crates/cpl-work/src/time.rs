//! RFC 3339 instants for [`crate::model::Timestamp`] (`spec/toolchain.md`
//! 10.1: "RFC 3339, UTC"): parsing with any offset, formatting in UTC, and
//! the arithmetic leases need, without a date-time dependency (Section 1.3
//! lists none, T4).

/// A point in time: seconds since 1970-01-01T00:00:00Z plus a fraction.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Instant {
    pub seconds: i64,
    pub nanos: u32,
}

/// `full-date "T" full-time` of RFC 3339 section 5.6: `YYYY-MM-DDTHH:MM:SS`,
/// an optional fraction of any length (nanoseconds are kept), and `Z` or a
/// `+HH:MM` / `-HH:MM` offset. `T` and `Z` may be lowercase (RFC 3339
/// 5.6 note). A leap second (`:60`) is accepted as the following second.
pub(crate) fn parse(text: &str) -> Option<Instant> {
    let b = text.as_bytes();
    let number = |start: usize, len: usize| -> Option<i64> {
        let digits = text.get(start..start + len)?;
        if digits.bytes().all(|c| c.is_ascii_digit()) {
            digits.parse().ok()
        } else {
            None
        }
    };
    let at = |i: usize, c: u8| b.get(i) == Some(&c);
    let year = number(0, 4)?;
    let month = number(5, 2)?;
    let day = number(8, 2)?;
    let hour = number(11, 2)?;
    let minute = number(14, 2)?;
    let second = number(17, 2)?;
    if !(at(4, b'-')
        && at(7, b'-')
        && (at(10, b'T') || at(10, b't'))
        && at(13, b':')
        && at(16, b':'))
    {
        return None;
    }
    if !(1..=12).contains(&month)
        || day < 1
        || day > days_in_month(year, month)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let mut i = 19;
    let mut nanos = 0u32;
    if at(i, b'.') {
        i += 1;
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == start {
            return None;
        }
        let mut scale = 100_000_000u32;
        for &digit in &b[start..i.min(start + 9)] {
            nanos += u32::from(digit - b'0') * scale;
            scale /= 10;
        }
    }
    let offset = match b.get(i)? {
        b'Z' | b'z' => {
            i += 1;
            0
        }
        &sign @ (b'+' | b'-') => {
            let hours = number(i + 1, 2)?;
            let minutes = number(i + 4, 2)?;
            if !at(i + 3, b':') || hours > 23 || minutes > 59 {
                return None;
            }
            i += 6;
            let total = hours * 3600 + minutes * 60;
            if sign == b'+' {
                total
            } else {
                -total
            }
        }
        _ => return None,
    };
    if i != b.len() {
        return None;
    }
    let seconds =
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset;
    Some(Instant { seconds, nanos })
}

/// `YYYY-MM-DDTHH:MM:SS[.fraction]Z`, the fraction only when non-zero and
/// without trailing zeros; `None` outside the years 0000-9999.
pub(crate) fn format(instant: Instant) -> Option<String> {
    let days = instant.seconds.div_euclid(86_400);
    let of_day = instant.seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    if !(0..=9999).contains(&year) {
        return None;
    }
    let mut text = format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    );
    if instant.nanos != 0 {
        let fraction = format!("{:09}", instant.nanos);
        text.push('.');
        text.push_str(fraction.trim_end_matches('0'));
    }
    text.push('Z');
    Some(text)
}

fn is_leap(year: i64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(year) => 29,
        _ => 28,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// The inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(seconds: i64) -> Instant {
        Instant { seconds, nanos: 0 }
    }

    #[test]
    fn known_instants() {
        assert_eq!(parse("1970-01-01T00:00:00Z"), Some(at(0)));
        assert_eq!(parse("2000-03-01T00:00:00Z"), Some(at(951_868_800)));
        assert_eq!(parse("2026-10-28T12:30:05Z"), Some(at(1_793_190_605)));
        assert_eq!(
            format(at(1_793_190_605)).as_deref(),
            Some("2026-10-28T12:30:05Z")
        );
        assert_eq!(format(at(-1)).as_deref(), Some("1969-12-31T23:59:59Z"));
        assert_eq!(parse("2024-02-29T23:59:59Z"), Some(at(1_709_251_199)));
    }

    #[test]
    fn offsets_fractions_and_case() {
        assert_eq!(
            parse("2026-10-28T14:30:05+02:00"),
            parse("2026-10-28T12:30:05Z")
        );
        assert_eq!(
            parse("2026-10-28T07:30:05-05:00"),
            parse("2026-10-28t12:30:05z")
        );
        let precise = parse("2026-10-28T12:30:05.250Z").unwrap();
        assert_eq!(precise.nanos, 250_000_000);
        assert_eq!(format(precise).as_deref(), Some("2026-10-28T12:30:05.25Z"));
        assert_eq!(
            parse("2026-10-28T12:30:05.1234567891Z").unwrap().nanos,
            123_456_789
        );
    }

    #[test]
    fn rejects_malformed() {
        for text in [
            "",
            "2026-10-28",
            "2026-10-28 12:30:05Z",
            "2026-13-01T00:00:00Z",
            "2025-02-29T00:00:00Z",
            "2026-10-28T24:00:00Z",
            "2026-10-28T12:30:05",
            "2026-10-28T12:30:05.Z",
            "2026-10-28T12:30:05+0200",
            "2026-10-28T12:30:05Zjunk",
        ] {
            assert_eq!(parse(text), None, "{text:?}");
        }
    }

    #[test]
    fn civil_round_trip() {
        for days in [-800_000, -1, 0, 1, 10_957, 20_000, 2_932_896] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
    }
}
