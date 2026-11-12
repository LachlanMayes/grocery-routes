//! Horizon timestamps.

use std::fmt;
use std::ops::{Add, AddAssign, Sub, SubAssign};

use serde::{Deserialize, Serialize};

/// A timestamp, in whole minutes, measured from the start of the planning
/// horizon.
///
/// The horizon may span several days; [`Minutes::DAY`] exists so that
/// multi-day timestamps stay readable (e.g. `1d 05:30`).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(transparent)]
pub struct Minutes(i64);

impl Minutes {
    /// Zero minutes: the start of the horizon.
    pub const ZERO: Self = Self(0);

    /// One day, in minutes.
    pub const DAY: Self = Self(1440);

    /// Builds a timestamp from a raw minute count.
    pub const fn from_minutes(minutes: i64) -> Self {
        Self(minutes)
    }

    /// Builds a timestamp from a number of hours, rounded to the nearest
    /// minute.
    pub fn from_hours(hours: f64) -> Self {
        Self((hours * 60.0).round() as i64)
    }

    /// The raw minute count.
    pub const fn minutes(self) -> i64 {
        self.0
    }

    /// The value in hours, as a float.
    pub fn hours(self) -> f64 {
        self.0 as f64 / 60.0
    }
}

impl From<i64> for Minutes {
    fn from(value: i64) -> Self {
        Self(value)
    }
}

impl From<Minutes> for i64 {
    fn from(value: Minutes) -> Self {
        value.0
    }
}

impl fmt::Display for Minutes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let days = self.0.div_euclid(Self::DAY.0);
        let remainder = self.0.rem_euclid(Self::DAY.0);
        let hours = remainder / 60;
        let minutes = remainder % 60;
        if days > 0 {
            write!(f, "{days}d {hours:02}:{minutes:02}")
        } else {
            write!(f, "{hours:02}:{minutes:02}")
        }
    }
}

impl Add for Minutes {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self(self.0 + rhs.0)
    }
}

impl Sub for Minutes {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self {
        Self(self.0 - rhs.0)
    }
}

impl AddAssign for Minutes {
    fn add_assign(&mut self, rhs: Self) {
        self.0 += rhs.0;
    }
}

impl SubAssign for Minutes {
    fn sub_assign(&mut self, rhs: Self) {
        self.0 -= rhs.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_within_the_first_day() {
        assert_eq!(Minutes::from_minutes(0).to_string(), "00:00");
        assert_eq!(Minutes::from_minutes(105).to_string(), "01:45");
        assert_eq!(Minutes::from_minutes(1439).to_string(), "23:59");
    }

    #[test]
    fn displays_multi_day_timestamps() {
        assert_eq!(Minutes::from_minutes(1440).to_string(), "1d 00:00");
        assert_eq!(Minutes::from_minutes(1770).to_string(), "1d 05:30");
    }

    #[test]
    fn arithmetic_round_trips() {
        let a = Minutes::from_hours(1.5);
        let b = Minutes::from_minutes(30);
        assert_eq!((a + b).minutes(), 120);
        assert_eq!((a - b).minutes(), 60);
        assert_eq!(a.hours(), 1.5);
    }
}
