//! Horizon timestamps anchored to an absolute clock.
//!
//! [`crate::Minutes`] is measured from the start of the planning horizon and
//! carries no absolute meaning on its own. This module converts horizon
//! minutes to local civil time, but only when the problem sets
//! [`crate::Problem::horizon_start`] and the node sets an IANA `timezone`.
//! When either is absent, every timestamp stays horizon-relative and the
//! timezone fields remain inert import/export data.
//!
//! Conversions go *instant → local civil time*: the anchor is an absolute
//! instant and each horizon minute is a fixed offset from it, so the result
//! is always unambiguous — DST transitions simply show up in the rendered
//! offset. (The ambiguous direction, civil → instant, would only arise if
//! callers entered wall-clock times; the model stores only horizon offsets,
//! so it is not offered.)

use jiff::{SignedDuration, Timestamp, Zoned, tz::TimeZone};
use thiserror::Error;

use crate::model::Problem;
use crate::time::Minutes;

/// Why a horizon minute could not be converted to a local civil time.
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ClockError {
    /// The problem declares no absolute anchor for the horizon.
    #[error("problem has no horizon_start anchor")]
    NoAnchor,
    /// The timezone name is not a known IANA identifier.
    #[error("invalid IANA timezone: {0}")]
    InvalidTimezone(String),
    /// The horizon anchor is not a valid RFC 3339 instant.
    #[error("invalid horizon_start anchor: {0}")]
    InvalidAnchor(String),
    /// The minute count cannot be represented as a whole number of seconds.
    #[error("horizon minute {0} overflows the clock")]
    Overflow(i64),
    /// The resulting instant is not representable.
    #[error("horizon minute {0} is outside the representable range")]
    OutOfRange(i64),
}

/// Parses and validates an IANA timezone name (e.g. `Australia/Sydney`).
///
/// # Errors
/// Returns [`ClockError::InvalidTimezone`] for names not present in the IANA
/// database.
pub fn parse_timezone(name: &str) -> Result<TimeZone, ClockError> {
    TimeZone::get(name).map_err(|_| ClockError::InvalidTimezone(name.to_owned()))
}

/// True when `name` is a known IANA timezone identifier.
#[must_use]
pub fn is_valid_timezone(name: &str) -> bool {
    TimeZone::get(name).is_ok()
}

/// Converts a horizon minute to local civil time in `timezone`.
///
/// # Errors
/// Returns [`ClockError::InvalidTimezone`] for an unknown IANA name, or
/// [`ClockError::Overflow`]/[`ClockError::OutOfRange`] for timestamps that
/// cannot be represented.
pub fn local_zoned(
    horizon_start: Timestamp,
    at: Minutes,
    timezone: &str,
) -> Result<Zoned, ClockError> {
    let tz = parse_timezone(timezone)?;
    let seconds = at
        .minutes()
        .checked_mul(60)
        .ok_or(ClockError::Overflow(at.minutes()))?;
    let absolute = horizon_start
        .checked_add(SignedDuration::from_secs(seconds))
        .map_err(|_| ClockError::OutOfRange(at.minutes()))?;
    Ok(absolute.to_zoned(tz))
}

/// Parses an RFC 3339 horizon anchor (e.g. `2026-11-05T00:00:00Z`).
///
/// # Errors
/// Returns [`ClockError::InvalidAnchor`] when `raw` is not a valid RFC 3339
/// instant.
pub fn parse_horizon_start(raw: &str) -> Result<Timestamp, ClockError> {
    raw.parse()
        .map_err(|_| ClockError::InvalidAnchor(raw.to_owned()))
}

/// Local civil time for a node, when both the problem and the node declare
/// the data needed to resolve it.
///
/// Returns `None` when the problem has no anchor, the node has no timezone,
/// or the name fails to resolve; bad names are reported separately by
/// [`crate::Problem::validate_timezones`] at the system boundary.
#[must_use]
pub fn local_time(problem: &Problem, timezone: Option<&str>, at: Minutes) -> Option<Zoned> {
    let anchor = problem.horizon_start?;
    let name = timezone?;
    local_zoned(anchor, at, name).ok()
}

/// Formats the local civil time at a node as `Thu 05 Nov 14:30`, for
/// human-facing output.
///
/// Returns `None` under the same conditions as [`local_time`].
#[must_use]
pub fn local_time_short(problem: &Problem, timezone: Option<&str>, at: Minutes) -> Option<String> {
    local_time(problem, timezone, at).map(|zoned| zoned.strftime("%a %d %b %H:%M").to_string())
}

/// The local calendar date at a node for a horizon minute, if resolvable.
///
/// Callers pair this with [`crate::holidays`] to flag deliveries that fall
/// on a national public holiday.
#[must_use]
pub fn local_date(
    problem: &Problem,
    timezone: Option<&str>,
    at: Minutes,
) -> Option<jiff::civil::Date> {
    local_time(problem, timezone, at).map(|zoned| zoned.date())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anchor() -> Timestamp {
        "2026-01-15T00:00:00Z".parse().expect("valid RFC 3339")
    }

    #[test]
    fn validates_iana_names() {
        assert!(is_valid_timezone("Australia/Sydney"));
        assert!(is_valid_timezone("Australia/Perth"));
        assert!(is_valid_timezone("UTC"));
        assert!(!is_valid_timezone("Australia/Sydnee"));
        assert!(!is_valid_timezone(""));
        assert!(!is_valid_timezone("Sydney"));
    }

    #[test]
    fn summer_offsets_differ_between_eastern_and_western_australia() {
        // 15 January 2026 is AEDT (UTC+11) and AWST (UTC+8).
        let sydney = local_zoned(anchor(), Minutes::ZERO, "Australia/Sydney").unwrap();
        let perth = local_zoned(anchor(), Minutes::ZERO, "Australia/Perth").unwrap();
        assert_eq!(sydney.offset().seconds() / 3600, 11);
        assert_eq!(perth.offset().seconds() / 3600, 8);
    }

    #[test]
    fn winter_offsets_drop_in_eastern_australia() {
        // 15 July 2026 is AEST (UTC+10); AWST stays UTC+8 (no DST in WA).
        let winter = "2026-07-15T00:00:00Z".parse().expect("valid RFC 3339");
        let sydney = local_zoned(winter, Minutes::ZERO, "Australia/Sydney").unwrap();
        let perth = local_zoned(winter, Minutes::ZERO, "Australia/Perth").unwrap();
        assert_eq!(sydney.offset().seconds() / 3600, 10);
        assert_eq!(perth.offset().seconds() / 3600, 8);
    }

    #[test]
    fn adds_horizon_minutes() {
        let zoned = local_zoned(
            anchor(),
            Minutes::from_minutes(3 * 60 + 30),
            "Australia/Sydney",
        )
        .unwrap();
        // 00:00Z + 3.5h = 03:30Z, which is 14:30 in AEDT (UTC+11).
        assert_eq!(zoned.hour(), 14);
        assert_eq!(zoned.minute(), 30);
    }

    #[test]
    fn rejects_unknown_timezone_and_keeps_error_kind() {
        let err = local_zoned(anchor(), Minutes::ZERO, "Australia/Sydnee").unwrap_err();
        assert_eq!(
            err,
            ClockError::InvalidTimezone("Australia/Sydnee".to_string())
        );
    }

    #[test]
    fn no_anchor_yields_no_local_time() {
        let problem = Problem::default();
        assert_eq!(
            local_time(&problem, Some("Australia/Sydney"), Minutes::ZERO),
            None
        );
    }

    #[test]
    fn local_time_follows_node_timezone() {
        let problem = Problem {
            horizon_start: Some(anchor()),
            ..Problem::default()
        };
        let zoned = local_time(
            &problem,
            Some("Australia/Sydney"),
            Minutes::from_minutes(60),
        )
        .expect("anchor and timezone are set");
        assert_eq!(zoned.hour(), 12); // 01:00Z = 12:00 AEDT
    }
}
