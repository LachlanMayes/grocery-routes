//! Australian national public holidays, computed from the calendar.
//!
//! **Scope: national holidays only.** State/territory-specific holidays
//! (e.g. Melbourne Cup Day, varying King's Birthday dates, the AFL Grand
//! Final Friday in some states) and one-off declarations are **not**
//! modelled. ANZAC Day is observed on 25 April even on a weekend (Sunday
//! observance varies by jurisdiction and is not modelled). This is a
//! scheduling aid, **not legal advice**; consult the Fair Work Ombudsman
//! and the relevant state/territory authority for the authoritative
//! calendar.
//!
//! Substituted ("observed") dates follow the common national rule: when a
//! fixed-date national holiday falls on a Saturday or Sunday, the next
//! weekday not already taken by another national holiday is observed. If
//! two national holidays ever coincide (the real-world case is Easter
//! Monday falling on ANZAC Day, e.g. 25 April 2011), the following
//! non-holiday weekday becomes a "Substitute public holiday".
//!
//! No data files are required; Easter is computed with the standard
//! Gregorian computus (valid for 1900–2099 and beyond).

use jiff::Span;
use jiff::civil::{Date, Weekday};

/// A national public holiday on a given date.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Holiday {
    pub date: Date,
    /// e.g. `"Good Friday"`. The same name is reused for the observed date
    /// of a weekend holiday.
    pub name: &'static str,
}

/// All national public holidays (actual and observed) for `year`, in date
/// order.
#[must_use]
pub fn holidays_in_year(year: i16) -> Vec<Holiday> {
    let mut holidays = fixed_holidays(year);
    for (offset, name) in [
        (-2, "Good Friday"),
        (-1, "Easter Saturday"),
        (0, "Easter Sunday"),
        (1, "Easter Monday"),
    ] {
        holidays.push(Holiday {
            date: shift_days(easter_sunday(year), offset),
            name,
        });
    }
    // ANZAC Day is fixed to 25 April and is never weekend-shifted.
    holidays.push(Holiday {
        date: date(year, 4, 25),
        name: "ANZAC Day",
    });
    holidays.sort_by_key(|holiday| holiday.date);
    substitute_for_coincidences(holidays)
}

/// Names of the national public holidays observed on `date` (usually zero
/// or one; two when holidays coincide).
#[must_use]
pub fn national_public_holidays(date: Date) -> Vec<&'static str> {
    holidays_in_year(date.year())
        .into_iter()
        .filter(|holiday| holiday.date == date)
        .map(|holiday| holiday.name)
        .collect()
}

/// True when `date` is a national public holiday (actual or observed).
#[must_use]
pub fn is_public_holiday(date: Date) -> bool {
    !national_public_holidays(date).is_empty()
}

/// Gregorian Easter Sunday for `year` (Meeus' anonymous Gregorian
/// algorithm).
fn easter_sunday(year: i16) -> Date {
    let y = i64::from(year);
    let a = y % 19;
    let b = y / 100;
    let c = y % 100;
    let d = b / 4;
    let e = b % 4;
    let f = (b + 8) / 25;
    let g = (b - f + 1) / 3;
    let h = (19 * a + b - d - g + 15) % 30;
    let i = c / 4;
    let k = c % 4;
    let l = (32 + 2 * e + 2 * i - h - k) % 7;
    let m = (a + 11 * h + 22 * l) / 451;
    let month = (h + l - 7 * m + 114) / 31;
    let day = (h + l - 7 * m + 114) % 31 + 1;
    date(year, month as i8, day as i8)
}

/// Fixed-date national holidays for `year`, including weekend-shifted
/// observed dates. ANZAC Day is handled separately (never shifted).
fn fixed_holidays(year: i16) -> Vec<Holiday> {
    const FIXED: [(i8, i8, &str); 4] = [
        (1, 1, "New Year's Day"),
        (1, 26, "Australia Day"),
        (12, 25, "Christmas Day"),
        (12, 26, "Boxing Day"),
    ];
    let actuals: Vec<Date> = FIXED
        .iter()
        .map(|&(month, day, _)| date(year, month, day))
        .collect();
    let mut taken: Vec<Date> = actuals.clone();
    let mut holidays: Vec<Holiday> = Vec::with_capacity(8);
    for ((_, _, name), actual) in FIXED.into_iter().zip(actuals) {
        holidays.push(Holiday { date: actual, name });
        if !is_weekend(actual) {
            continue;
        }
        // Observed: the next weekday not already taken.
        let mut observed = shift_days(actual, 1);
        while is_weekend(observed) || taken.contains(&observed) {
            observed = shift_days(observed, 1);
        }
        taken.push(observed);
        holidays.push(Holiday {
            date: observed,
            name,
        });
    }
    holidays
}

/// When two or more holidays share a date, a "Substitute public holiday" is
/// added on the next weekday not otherwise taken. Substitutes land on free
/// weekdays by construction, so no iterative cascade is needed.
fn substitute_for_coincidences(mut holidays: Vec<Holiday>) -> Vec<Holiday> {
    holidays.sort_by_key(|holiday| holiday.date);
    let mut taken: Vec<Date> = holidays.iter().map(|holiday| holiday.date).collect();
    let mut substitutes: Vec<Holiday> = Vec::new();
    let mut index = 0;
    while index < holidays.len() {
        let mut end = index + 1;
        while end < holidays.len() && holidays[end].date == holidays[index].date {
            end += 1;
        }
        if end - index >= 2 {
            let mut candidate = shift_days(holidays[index].date, 1);
            while is_weekend(candidate) || taken.contains(&candidate) {
                candidate = shift_days(candidate, 1);
            }
            taken.push(candidate);
            substitutes.push(Holiday {
                date: candidate,
                name: "Substitute public holiday",
            });
        }
        index = end;
    }
    holidays.extend(substitutes);
    holidays.sort_by_key(|holiday| holiday.date);
    holidays
}

fn is_weekend(date: Date) -> bool {
    matches!(date.weekday(), Weekday::Saturday | Weekday::Sunday)
}

fn shift_days(date: Date, days: i64) -> Date {
    date.checked_add(Span::new().days(days))
        .expect("date arithmetic stays within the representable range")
}

fn date(year: i16, month: i8, day: i8) -> Date {
    Date::new(year, month, day).expect("fixed calendar dates are valid")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(year: i16, month: i8, day: i8) -> Date {
        Date::new(year, month, day).unwrap()
    }

    #[test]
    fn easter_2026_is_april_5() {
        assert_eq!(easter_sunday(2026), d(2026, 4, 5));
    }

    #[test]
    fn easter_2027_is_march_28() {
        assert_eq!(easter_sunday(2027), d(2027, 3, 28));
    }

    #[test]
    fn easter_2028_is_april_16() {
        assert_eq!(easter_sunday(2028), d(2028, 4, 16));
    }

    #[test]
    fn easter_2011_is_april_24() {
        // 2011: Easter Sunday 24 April, Easter Monday = ANZAC Day (25 April).
        assert_eq!(easter_sunday(2011), d(2011, 4, 24));
    }

    #[test]
    fn easter_based_holidays_2026() {
        let holidays = holidays_in_year(2026);
        let find = |name| holidays.iter().find(|h| h.name == name).map(|h| h.date);
        assert_eq!(find("Good Friday"), Some(d(2026, 4, 3)));
        assert_eq!(find("Easter Saturday"), Some(d(2026, 4, 4)));
        assert_eq!(find("Easter Sunday"), Some(d(2026, 4, 5)));
        assert_eq!(find("Easter Monday"), Some(d(2026, 4, 6)));
    }

    #[test]
    fn fixed_holidays_2026() {
        let holidays = holidays_in_year(2026);
        let find = |name| holidays.iter().find(|h| h.name == name).map(|h| h.date);
        // 1 Jan 2026 is a Thursday; Australia Day 2026 is a Monday.
        assert_eq!(find("New Year's Day"), Some(d(2026, 1, 1)));
        assert_eq!(find("Australia Day"), Some(d(2026, 1, 26)));
        // Christmas Day 2026 is a Friday; Boxing Day a Saturday, observed
        // Monday 28 December.
        assert_eq!(find("Christmas Day"), Some(d(2026, 12, 25)));
        assert!(
            holidays
                .iter()
                .any(|h| h.date == d(2026, 12, 26) && h.name == "Boxing Day")
        );
        assert!(
            holidays
                .iter()
                .any(|h| h.date == d(2026, 12, 28) && h.name == "Boxing Day")
        );
    }

    #[test]
    fn fixed_holidays_2027_christmas_cascade() {
        let holidays = holidays_in_year(2027);
        // Christmas Day 2027 is a Saturday → observed Monday 27 December.
        // Boxing Day 2027 is a Sunday → observed Tuesday 28 December.
        assert!(
            holidays
                .iter()
                .any(|h| h.date == d(2027, 12, 27) && h.name == "Christmas Day")
        );
        assert!(
            holidays
                .iter()
                .any(|h| h.date == d(2027, 12, 28) && h.name == "Boxing Day")
        );
        // The actual weekend dates are also holidays.
        assert!(
            holidays
                .iter()
                .any(|h| h.date == d(2027, 12, 25) && h.name == "Christmas Day")
        );
        assert!(
            holidays
                .iter()
                .any(|h| h.date == d(2027, 12, 26) && h.name == "Boxing Day")
        );
    }

    #[test]
    fn weekend_shift_uses_next_free_weekday() {
        // 2022: Christmas Day was a Sunday, Boxing Day a Monday. The
        // Christmas observed date must not displace Boxing Day's actual
        // Monday holiday, so it lands on Tuesday.
        let holidays = holidays_in_year(2022);
        assert!(
            holidays
                .iter()
                .any(|h| h.date == d(2022, 12, 26) && h.name == "Boxing Day")
        );
        assert!(
            holidays
                .iter()
                .any(|h| h.date == d(2022, 12, 27) && h.name == "Christmas Day")
        );
    }

    #[test]
    fn new_years_day_weekend_shift() {
        // 1 January 2022 was a Saturday; Monday 3 January was observed.
        let holidays = holidays_in_year(2022);
        assert!(
            holidays
                .iter()
                .any(|h| h.date == d(2022, 1, 3) && h.name == "New Year's Day")
        );
    }

    #[test]
    fn an_zac_day_is_never_shifted() {
        // 25 April 2026 is a Saturday; ANZAC Day stays on the day itself.
        let holidays = holidays_in_year(2026);
        assert!(
            holidays
                .iter()
                .any(|h| h.date == d(2026, 4, 25) && h.name == "ANZAC Day")
        );
        // No extra Monday 27 April ANZAC entry is invented.
        assert!(
            !holidays
                .iter()
                .any(|h| h.date == d(2026, 4, 27) && h.name == "ANZAC Day")
        );
    }

    #[test]
    fn coincident_holidays_get_substitute_day_2011() {
        // 2011: Easter Monday fell on ANZAC Day (Monday 25 April); Australia
        // declared a substitute public holiday on Tuesday 26 April 2011.
        let holidays = holidays_in_year(2011);
        let names = national_public_holidays(d(2011, 4, 25));
        assert!(names.contains(&"ANZAC Day"));
        assert!(names.contains(&"Easter Monday"));
        assert!(
            holidays
                .iter()
                .any(|h| h.date == d(2011, 4, 26) && h.name == "Substitute public holiday")
        );
    }

    #[test]
    fn ordinary_date_is_not_a_holiday() {
        assert!(!is_public_holiday(d(2026, 6, 17)));
        assert!(is_public_holiday(d(2026, 12, 28)));
    }

    #[test]
    fn holidays_are_sorted_and_unique_dates_ok() {
        let holidays = holidays_in_year(2027);
        for pair in holidays.windows(2) {
            assert!(pair[0].date <= pair[1].date);
        }
    }
}
