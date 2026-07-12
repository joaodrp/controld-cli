//! Timestamp rendering (commands.md conventions): the API sends Unix
//! seconds; JSON gets RFC-3339 UTC, tables get a relative phrase. Hand-rolled
//! civil-date math — a calendar dependency buys nothing for UTC-only output.

/// Unix seconds as RFC-3339 UTC (`2026-03-11T12:07:28Z`). Pre-epoch inputs
/// clamp to the epoch — last-resort display safety, not the primary
/// defense: normalization (`Profile::from_api`) rejects negative
/// timestamps before they ever reach this function.
pub fn rfc3339(unix_secs: i64) -> String {
    let secs = unix_secs.max(0);
    let (days, rem) = (secs / 86_400, secs % 86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60,
    )
}

/// Days since 1970-01-01 to a civil (year, month, day) — Howard Hinnant's
/// algorithm, valid across the full range we can receive.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // day of era [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // year of era
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // day of year [0, 365]
    let mp = (5 * doy + 2) / 153; // month, March-based [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (
        year,
        u32::try_from(month).expect("month is 1-12"),
        u32::try_from(day).expect("day is 1-31"),
    )
}

/// The current time as Unix seconds, clamped to fit — `relative`'s `now`
/// parameter in production.
///
/// `CONTROLD_UNIX_NOW` freezes it for tests, the same override pattern as
/// `CONTROLD_API_URL` (api/client.rs): undocumented, and any failure to read
/// or parse it falls through to the real clock rather than erroring — an
/// unrecognized test-only override must never break a real user.
pub fn now() -> i64 {
    // Nested (not a let-chain): let-chains stabilized after the 1.85 MSRV.
    if let Ok(Some(raw)) = crate::config::env_var("CONTROLD_UNIX_NOW") {
        if let Ok(frozen) = raw.parse::<i64>() {
            return frozen;
        }
    }
    i64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    )
    .unwrap_or(i64::MAX)
}

/// A relative phrase for tables ("3 days ago"); `now` is a parameter so
/// rendering stays deterministic under test.
pub fn relative(unix_secs: i64, now: i64) -> String {
    let delta = now - unix_secs;
    if delta < 0 {
        // Future timestamps do not occur in `updated`; render the bare
        // truth rather than lying with "just now".
        return format!("in {}", span(-delta));
    }
    if delta < 45 {
        return "just now".to_owned();
    }
    format!("{} ago", span(delta))
}

fn span(secs: i64) -> String {
    const UNITS: &[(i64, &str)] = &[
        (365 * 86_400, "year"),
        (30 * 86_400, "month"),
        (86_400, "day"),
        (3600, "hour"),
        (60, "minute"),
        (1, "second"),
    ];
    for (size, name) in UNITS {
        if secs >= *size {
            let n = secs / size;
            let plural = if n == 1 { "" } else { "s" };
            return format!("{n} {name}{plural}");
        }
    }
    "0 seconds".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc3339_renders_known_instants() {
        assert_eq!(rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(rfc3339(951_827_696), "2000-02-29T12:34:56Z"); // leap day
        assert_eq!(rfc3339(1_741_693_261), "2025-03-11T11:41:01Z");
        assert_eq!(rfc3339(4_102_444_799), "2099-12-31T23:59:59Z");
    }

    #[test]
    fn rfc3339_clamps_pre_epoch_garbage() {
        assert_eq!(rfc3339(-1), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn relative_buckets_read_naturally() {
        let now = 1_800_000_000;
        assert_eq!(relative(now - 10, now), "just now");
        assert_eq!(relative(now + 1, now), "in 1 second");
        assert_eq!(relative(now - 90, now), "1 minute ago");
        assert_eq!(relative(now - 7200, now), "2 hours ago");
        assert_eq!(relative(now - 3 * 86_400, now), "3 days ago");
        assert_eq!(relative(now - 40 * 86_400, now), "1 month ago");
        assert_eq!(relative(now - 800 * 86_400, now), "2 years ago");
        assert_eq!(relative(now + 3600, now), "in 1 hour");
    }
}
