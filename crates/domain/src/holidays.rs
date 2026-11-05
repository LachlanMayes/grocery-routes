//! Australian public holidays, computed from the calendar.
//!
//! Two layers are offered:
//!
//! - [`holidays_in_year`] / [`national_public_holidays`]: the *national*
//!   calendar (New Year's Day, Australia Day, the Easter days, ANZAC Day,
//!   Christmas and Boxing Day, plus observed dates).
//! - [`state_holidays_in_year`]: a full jurisdiction calendar
//!   ([`Jurisdiction`]) adding state/territory holidays — Labour Day,
//!   King's Birthday, Melbourne Cup, the AFL Grand Final Friday and the
//!   other state-specific dates.
//!
//! Substituted ("observed") dates follow the common rule: when a fixed-date
//! holiday falls on a Saturday or Sunday, the next weekday not already taken
//! is observed. If two holidays ever coincide (the real-world case is Easter
//! Monday falling on ANZAC Day, e.g. 25 April 2011), the following
//! non-holiday weekday becomes a "Substitute public holiday".
//!
//! ANZAC Day substitution is jurisdiction-specific: ACT, NSW and WA observe
//! the following Monday whenever 25 April falls on a weekend; QLD and NT
//! observe it when it falls on a Sunday; SA, TAS and VIC never substitute.
//!
//! # Sources and limits
//!
//! Dates follow each jurisdiction's published rules and were cross-checked
//! against the NT Government's public holiday page (nt.gov.au/nt-public-holidays,
//! accessed 2026-11-05) and the timeanddate.com Australia 2026–2027 holiday
//! pages (accessed 2026-11-05). **Every authority's own calendar is
//! authoritative.**
//!
//! Irregular dates that cannot be computed (the AFL Grand Final Friday in
//! Victoria) come from a small sourced table covering 2026–2027; in other
//! years that holiday is *omitted* from the returned list. Regional show
//! days, part-day holidays (e.g. Christmas Eve from 19:00), Easter Tuesday
//! (Tasmanian public sector) and one-off declarations are out of scope.
//! This is a scheduling aid, **not legal advice**.
//!
//! No data files are required; Easter is computed with the standard
//! Gregorian computus (valid for 1900–2099 and beyond).

use std::fmt;

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

/// One of the eight Australian state/territory jurisdictions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Jurisdiction {
    Act,
    Nsw,
    Nt,
    Qld,
    Sa,
    Tas,
    Vic,
    Wa,
}

impl Jurisdiction {
    /// Every jurisdiction, in a stable order.
    pub const ALL: [Self; 8] = [
        Self::Act,
        Self::Nsw,
        Self::Nt,
        Self::Qld,
        Self::Sa,
        Self::Tas,
        Self::Vic,
        Self::Wa,
    ];

    /// The uppercase code used on the command line (`ACT`, `NSW`, ...).
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Act => "ACT",
            Self::Nsw => "NSW",
            Self::Nt => "NT",
            Self::Qld => "QLD",
            Self::Sa => "SA",
            Self::Tas => "TAS",
            Self::Vic => "VIC",
            Self::Wa => "WA",
        }
    }

    /// Parses a jurisdiction code (case-insensitively).
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|jurisdiction| jurisdiction.code().eq_ignore_ascii_case(code))
    }

    /// The full name, e.g. `"Victoria"`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Act => "Australian Capital Territory",
            Self::Nsw => "New South Wales",
            Self::Nt => "Northern Territory",
            Self::Qld => "Queensland",
            Self::Sa => "South Australia",
            Self::Tas => "Tasmania",
            Self::Vic => "Victoria",
            Self::Wa => "Western Australia",
        }
    }
}

impl fmt::Display for Jurisdiction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.code())
    }
}

/// The full public holiday calendar for one jurisdiction and year, in date
/// order: the national holidays adjusted to that jurisdiction plus its
/// state/territory-specific dates.
///
/// See the module docs for sources and what is deliberately excluded.
#[must_use]
pub fn state_holidays_in_year(year: i16, jurisdiction: Jurisdiction) -> Vec<Holiday> {
    use Jurisdiction::{Act, Nsw, Nt, Qld, Sa, Tas, Vic, Wa};

    let mut holidays: Vec<Holiday> = Vec::new();

    // Fixed national dates with observed shifts; SA calls 26 December
    // Proclamation Day.
    for holiday in fixed_holidays(year) {
        let name = if jurisdiction == Sa && holiday.name == "Boxing Day" {
            "Proclamation Day"
        } else {
            holiday.name
        };
        holidays.push(Holiday {
            date: holiday.date,
            name,
        });
    }

    holidays.push(Holiday {
        date: shift_days(easter_sunday(year), -2),
        name: "Good Friday",
    });
    holidays.push(Holiday {
        date: shift_days(easter_sunday(year), 1),
        name: "Easter Monday",
    });
    if matches!(jurisdiction, Act | Nsw | Nt | Qld | Sa | Vic) {
        holidays.push(Holiday {
            date: shift_days(easter_sunday(year), -1),
            name: "Easter Saturday",
        });
    }
    if matches!(jurisdiction, Act | Nsw | Nt | Qld | Sa | Vic | Wa) {
        holidays.push(Holiday {
            date: easter_sunday(year),
            name: "Easter Sunday",
        });
    }

    // ANZAC Day, with jurisdiction-specific substitution.
    let anzac = date(year, 4, 25);
    holidays.push(Holiday {
        date: anzac,
        name: "ANZAC Day",
    });
    let weekend_shift = match jurisdiction {
        Act | Nsw | Wa => {
            anzac.weekday() == Weekday::Saturday || anzac.weekday() == Weekday::Sunday
        }
        Qld | Nt => anzac.weekday() == Weekday::Sunday,
        Sa | Tas | Vic => false,
    };
    if weekend_shift {
        let mut observed = shift_days(anzac, 1);
        while is_weekend(observed) {
            observed = shift_days(observed, 1);
        }
        holidays.push(Holiday {
            date: observed,
            name: "ANZAC Day (observed)",
        });
    }

    // King's Birthday: 2nd Monday in June for most jurisdictions, 1st Monday
    // in October for QLD, and the proclaimed (recently: last) Monday in
    // September for WA.
    let kings_birthday = match jurisdiction {
        Qld => nth_weekday(year, 10, Weekday::Monday, 1),
        Wa => last_weekday(year, 9, Weekday::Monday),
        Act | Nsw | Nt | Sa | Tas | Vic => nth_weekday(year, 6, Weekday::Monday, 2),
    };
    holidays.push(Holiday {
        date: kings_birthday,
        name: "King's Birthday",
    });

    // Labour Day under its local name and date.
    let (labour_day, labour_name) = match jurisdiction {
        Wa => (nth_weekday(year, 3, Weekday::Monday, 1), "Labour Day"),
        Tas => (nth_weekday(year, 3, Weekday::Monday, 2), "Eight Hours Day"),
        Vic => (nth_weekday(year, 3, Weekday::Monday, 2), "Labour Day"),
        Nt | Qld => (nth_weekday(year, 5, Weekday::Monday, 1), "May Day"),
        Act | Nsw | Sa => (nth_weekday(year, 10, Weekday::Monday, 1), "Labour Day"),
    };
    holidays.push(Holiday {
        date: labour_day,
        name: labour_name,
    });

    // Jurisdiction-specific extras.
    match jurisdiction {
        Act => {
            holidays.push(Holiday {
                date: nth_weekday(year, 3, Weekday::Monday, 2),
                name: "Canberra Day",
            });
            holidays.push(Holiday {
                date: monday_on_or_after(year, 5, 27),
                name: "Reconciliation Day",
            });
        }
        Nsw => holidays.push(Holiday {
            date: nth_weekday(year, 8, Weekday::Monday, 1),
            name: "Bank Holiday",
        }),
        Nt => holidays.push(Holiday {
            date: nth_weekday(year, 8, Weekday::Monday, 1),
            name: "Picnic Day",
        }),
        Qld => {}
        Sa => holidays.push(Holiday {
            date: nth_weekday(year, 3, Weekday::Monday, 2),
            name: "Adelaide Cup",
        }),
        Tas => holidays.push(Holiday {
            date: nth_weekday(year, 11, Weekday::Monday, 1),
            name: "Recreation Day",
        }),
        Vic => {
            holidays.push(Holiday {
                date: nth_weekday(year, 11, Weekday::Tuesday, 1),
                name: "Melbourne Cup",
            });
            if let Some(friday) = victoria_afl_grand_final_friday(year) {
                holidays.push(Holiday {
                    date: friday,
                    name: "Friday before the AFL Grand Final",
                });
            }
        }
        Wa => holidays.push(Holiday {
            date: nth_weekday(year, 6, Weekday::Monday, 1),
            name: "WA Day",
        }),
    }

    holidays.sort_by_key(|holiday| holiday.date);
    substitute_for_coincidences(holidays)
}

/// AFL Grand Final Fridays in Victoria, which the league fixes too late to
/// compute. Sourced from the timeanddate.com Australia 2027 page (accessed
/// 2026-11-05; 2027 marked tentative there) and its 2026 page; YEARS OUTSIDE
/// THIS TABLE RETURN `None` and the holiday is omitted.
fn victoria_afl_grand_final_friday(year: i16) -> Option<Date> {
    match year {
        2026 => Some(date(2026, 9, 25)),
        2027 => Some(date(2027, 9, 24)),
        _ => None,
    }
}

/// Sunday-zero weekday number.
fn weekday_number(weekday: Weekday) -> u8 {
    match weekday {
        Weekday::Sunday => 0,
        Weekday::Monday => 1,
        Weekday::Tuesday => 2,
        Weekday::Wednesday => 3,
        Weekday::Thursday => 4,
        Weekday::Friday => 5,
        Weekday::Saturday => 6,
    }
}

/// The `n`th `weekday` of `month` (1-based).
fn nth_weekday(year: i16, month: i8, weekday: Weekday, n: u8) -> Date {
    let first = date(year, month, 1);
    let offset = (i64::from(weekday_number(weekday)) - i64::from(weekday_number(first.weekday())))
        .rem_euclid(7);
    let day = 1 + offset + 7 * (i64::from(n) - 1);
    date(year, month, day as i8)
}

/// The last `weekday` of `month`.
fn last_weekday(year: i16, month: i8, weekday: Weekday) -> Date {
    let next_month = if month == 12 {
        date(year + 1, 1, 1)
    } else {
        date(year, month + 1, 1)
    };
    let last = shift_days(next_month, -1);
    let offset = (i64::from(weekday_number(last.weekday())) - i64::from(weekday_number(weekday)))
        .rem_euclid(7);
    shift_days(last, -offset)
}

/// The Monday on or after `month`/`day` (ACT's Reconciliation Day rule).
fn monday_on_or_after(year: i16, month: i8, day: i8) -> Date {
    let start = date(year, month, day);
    let offset = (1 - i64::from(weekday_number(start.weekday()))).rem_euclid(7);
    shift_days(start, offset)
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

    /// Date of the holiday called `name`, if present.
    fn state_date(holidays: &[Holiday], name: &str) -> Option<Date> {
        holidays
            .iter()
            .find(|holiday| holiday.name == name)
            .map(|holiday| holiday.date)
    }

    /// True when `name` is observed on `date`.
    fn state_has(holidays: &[Holiday], date: Date, name: &str) -> bool {
        holidays
            .iter()
            .any(|holiday| holiday.date == date && holiday.name == name)
    }

    #[test]
    fn state_calendars_always_contain_the_national_core() {
        for jurisdiction in Jurisdiction::ALL {
            let holidays = state_holidays_in_year(2026, jurisdiction);
            for name in [
                "New Year's Day",
                "Australia Day",
                "Good Friday",
                "ANZAC Day",
                "Christmas Day",
            ] {
                assert!(
                    state_date(&holidays, name).is_some(),
                    "{jurisdiction} 2026 is missing {name}"
                );
            }
            for pair in holidays.windows(2) {
                assert!(
                    pair[0].date <= pair[1].date,
                    "{jurisdiction} calendar is not sorted"
                );
            }
        }
    }

    #[test]
    fn victoria_2026_matches_published_calendar() {
        let holidays = state_holidays_in_year(2026, Jurisdiction::Vic);
        assert_eq!(state_date(&holidays, "Labour Day"), Some(d(2026, 3, 9)));
        assert_eq!(
            state_date(&holidays, "King's Birthday"),
            Some(d(2026, 6, 8))
        );
        assert_eq!(
            state_date(&holidays, "Friday before the AFL Grand Final"),
            Some(d(2026, 9, 25))
        );
        assert_eq!(state_date(&holidays, "Melbourne Cup"), Some(d(2026, 11, 3)));
        assert!(state_has(&holidays, d(2026, 4, 4), "Easter Saturday"));
        assert!(state_has(&holidays, d(2026, 4, 5), "Easter Sunday"));
        assert!(
            !state_has(&holidays, d(2026, 4, 27), "ANZAC Day (observed)"),
            "Victoria does not substitute ANZAC Day"
        );
    }

    #[test]
    fn northern_territory_2026_and_2027_match_the_nt_government_page() {
        let holidays = state_holidays_in_year(2026, Jurisdiction::Nt);
        assert!(state_has(&holidays, d(2026, 4, 4), "Easter Saturday"));
        assert!(state_has(&holidays, d(2026, 4, 5), "Easter Sunday"));
        assert_eq!(state_date(&holidays, "May Day"), Some(d(2026, 5, 4)));
        assert_eq!(
            state_date(&holidays, "King's Birthday"),
            Some(d(2026, 6, 8))
        );
        assert_eq!(state_date(&holidays, "Picnic Day"), Some(d(2026, 8, 3)));
        assert!(
            !state_has(&holidays, d(2026, 4, 27), "ANZAC Day (observed)"),
            "NT does not substitute a Saturday ANZAC Day"
        );

        let holidays = state_holidays_in_year(2027, Jurisdiction::Nt);
        // Sunday 25 April 2027 -> Monday 26 April observed (NT Government).
        assert!(state_has(&holidays, d(2027, 4, 26), "ANZAC Day (observed)"));
        assert!(state_has(&holidays, d(2027, 12, 27), "Christmas Day"));
        assert!(state_has(&holidays, d(2027, 12, 28), "Boxing Day"));
    }

    #[test]
    fn western_australia_2026_matches_published_calendar() {
        let holidays = state_holidays_in_year(2026, Jurisdiction::Wa);
        assert_eq!(state_date(&holidays, "Labour Day"), Some(d(2026, 3, 2)));
        assert_eq!(state_date(&holidays, "WA Day"), Some(d(2026, 6, 1)));
        assert_eq!(
            state_date(&holidays, "King's Birthday"),
            Some(d(2026, 9, 28))
        );
        assert!(state_has(&holidays, d(2026, 4, 5), "Easter Sunday"));
        assert!(
            state_date(&holidays, "Easter Saturday").is_none(),
            "WA does not observe Easter Saturday"
        );
        assert!(state_has(&holidays, d(2026, 4, 27), "ANZAC Day (observed)"));
    }

    #[test]
    fn queensland_and_act_2026_match_published_calendars() {
        let holidays = state_holidays_in_year(2026, Jurisdiction::Qld);
        assert_eq!(state_date(&holidays, "May Day"), Some(d(2026, 5, 4)));
        assert_eq!(
            state_date(&holidays, "King's Birthday"),
            Some(d(2026, 10, 5))
        );
        assert!(state_has(&holidays, d(2026, 4, 4), "Easter Saturday"));
        assert!(state_has(&holidays, d(2026, 4, 5), "Easter Sunday"));
        assert!(!state_has(
            &holidays,
            d(2026, 4, 27),
            "ANZAC Day (observed)"
        ));

        let holidays = state_holidays_in_year(2026, Jurisdiction::Act);
        assert_eq!(state_date(&holidays, "Canberra Day"), Some(d(2026, 3, 9)));
        assert_eq!(
            state_date(&holidays, "Reconciliation Day"),
            Some(d(2026, 6, 1))
        );
        assert_eq!(state_date(&holidays, "Labour Day"), Some(d(2026, 10, 5)));
        assert!(state_has(&holidays, d(2026, 4, 27), "ANZAC Day (observed)"));
    }

    #[test]
    fn nsw_sa_tas_2026_match_published_calendars() {
        let holidays = state_holidays_in_year(2026, Jurisdiction::Nsw);
        assert_eq!(state_date(&holidays, "Bank Holiday"), Some(d(2026, 8, 3)));
        assert_eq!(state_date(&holidays, "Labour Day"), Some(d(2026, 10, 5)));
        assert!(state_has(&holidays, d(2026, 4, 27), "ANZAC Day (observed)"));

        let holidays = state_holidays_in_year(2026, Jurisdiction::Sa);
        assert_eq!(state_date(&holidays, "Adelaide Cup"), Some(d(2026, 3, 9)));
        assert_eq!(state_date(&holidays, "Labour Day"), Some(d(2026, 10, 5)));
        assert!(state_has(&holidays, d(2026, 12, 26), "Proclamation Day"));
        assert!(
            state_date(&holidays, "Boxing Day").is_none(),
            "SA calls 26 December Proclamation Day"
        );

        let holidays = state_holidays_in_year(2026, Jurisdiction::Tas);
        assert_eq!(
            state_date(&holidays, "Eight Hours Day"),
            Some(d(2026, 3, 9))
        );
        assert_eq!(
            state_date(&holidays, "King's Birthday"),
            Some(d(2026, 6, 8))
        );
        assert_eq!(
            state_date(&holidays, "Recreation Day"),
            Some(d(2026, 11, 2))
        );
        assert!(state_date(&holidays, "Easter Saturday").is_none());
        assert!(state_date(&holidays, "Easter Sunday").is_none());
    }

    #[test]
    fn irregular_dates_are_omitted_outside_the_sourced_table() {
        // 2027's AFL Grand Final Friday is in the sourced table (tentative).
        let vic_2027 = state_holidays_in_year(2027, Jurisdiction::Vic);
        assert_eq!(
            state_date(&vic_2027, "Friday before the AFL Grand Final"),
            Some(d(2027, 9, 24))
        );
        // Outside the table the holiday is omitted rather than guessed.
        let vic_2028 = state_holidays_in_year(2028, Jurisdiction::Vic);
        assert!(
            state_date(&vic_2028, "Friday before the AFL Grand Final").is_none(),
            "unsourced years must not guess the AFL date"
        );
        // WA 2027 King's Birthday follows the last-Monday-of-September rule.
        let wa_2027 = state_holidays_in_year(2027, Jurisdiction::Wa);
        assert_eq!(
            state_date(&wa_2027, "King's Birthday"),
            Some(d(2027, 9, 27))
        );
    }

    #[test]
    fn jurisdiction_codes_round_trip() {
        for jurisdiction in Jurisdiction::ALL {
            assert_eq!(
                Jurisdiction::from_code(jurisdiction.code()),
                Some(jurisdiction)
            );
            assert_eq!(
                Jurisdiction::from_code(&jurisdiction.code().to_lowercase()),
                Some(jurisdiction)
            );
        }
        assert_eq!(Jurisdiction::from_code("nsw"), Some(Jurisdiction::Nsw));
        assert_eq!(Jurisdiction::from_code("VIC"), Some(Jurisdiction::Vic));
        assert_eq!(Jurisdiction::from_code("xx"), None);
    }
}
