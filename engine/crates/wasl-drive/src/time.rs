//! RFC 3339 timestamps as Drive sends and expects them (port of `TimeFmt`).

use time::format_description::well_known::Rfc3339;
use time::macros::format_description;
use time::{Duration, OffsetDateTime};

/// Epoch milliseconds to RFC 3339 UTC, like Java's `ISO_INSTANT`: whole seconds print without a
/// fraction (`2025-01-02T03:04:05Z`), others with milliseconds (`...05.120Z`).
pub fn to_rfc3339(epoch_ms: i64) -> String {
    // RFC 3339 and Drive take years 0001-9999; a file dated beyond them (any filesystem allows it)
    // is sent at the nearest end instead of panicking the run (robustness review 2026-10).
    const FIRST_MS: i64 = -62_135_596_800_000; // 0001-01-01T00:00:00Z
    const LAST_MS: i64 = 253_402_300_799_999; // 9999-12-31T23:59:59.999Z
    let epoch_ms = epoch_ms.clamp(FIRST_MS, LAST_MS);
    let t = OffsetDateTime::UNIX_EPOCH + Duration::milliseconds(epoch_ms);
    let result = if epoch_ms.rem_euclid(1000) == 0 {
        t.format(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second]Z"
        ))
    } else {
        t.format(format_description!(
            "[year]-[month]-[day]T[hour]:[minute]:[second].[subsecond digits:3]Z"
        ))
    };
    result.expect("a UTC timestamp always formats")
}

/// RFC 3339 to epoch milliseconds; `None` when absent or unparseable.
pub fn parse_rfc3339(s: Option<&str>) -> Option<i64> {
    let t = OffsetDateTime::parse(s?, &Rfc3339).ok()?;
    i64::try_from(t.unix_timestamp_nanos() / 1_000_000).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        assert_eq!("2025-01-02T03:04:05Z", to_rfc3339(1_735_787_045_000));
        assert_eq!("2025-01-02T03:04:05.120Z", to_rfc3339(1_735_787_045_120));
        assert_eq!(
            Some(1_767_323_045_000),
            parse_rfc3339(Some("2026-01-02T03:04:05.000Z"))
        );
        assert_eq!(
            Some(1_767_323_045_000),
            parse_rfc3339(Some("2026-01-02T03:04:05Z"))
        );
        assert_eq!(None, parse_rfc3339(Some("yesterday")));
        assert_eq!(None, parse_rfc3339(None));
    }
}
