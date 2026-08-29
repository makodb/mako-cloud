//! Five-field cron expressions evaluated in UTC.
//!
//! `minute hour day-of-month month day-of-week`, with `*`, lists (`1,15`),
//! ranges (`1-5`), steps (`*/15`, `1-10/2`), three-letter month and weekday
//! names, and `0` or `7` for Sunday. Day-of-month and day-of-week follow the
//! classic vixie rule: when both are restricted a day matches if *either*
//! does; when one of them starts with `*` both must match. There is no
//! timezone field and no daylight-saving arithmetic: every expression is
//! evaluated against UTC wall-clock time, so a schedule fires at the same
//! instant every day of the year.
//!
//! Anything else -- six fields, `?`, `L`, `W`, `#`, `@daily`, seconds,
//! years, wrapped ranges -- is refused with a message that names the field.

use std::{error::Error, fmt};

use time::OffsetDateTime;

/// An expression is at most this long; the public schema promises the same.
pub const MAXIMUM_EXPRESSION_BYTES: usize = 128;
/// `next_after` gives up after this many days without a match. Five years
/// covers every leap-day schedule and refuses the ones that never fire
/// (`0 0 31 2 *`).
pub const SEARCH_HORIZON_DAYS: u64 = 5 * 366;

const SECONDS_PER_MINUTE: u64 = 60;
const SECONDS_PER_HOUR: u64 = 60 * 60;
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;
const MONTH_NAMES: [&str; 12] = [
    "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
];
const WEEKDAY_NAMES: [&str; 7] = ["sun", "mon", "tue", "wed", "thu", "fri", "sat"];

/// The five fields, in order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CronField {
    Minute,
    Hour,
    DayOfMonth,
    Month,
    DayOfWeek,
}

impl CronField {
    const ALL: [Self; 5] = [
        Self::Minute,
        Self::Hour,
        Self::DayOfMonth,
        Self::Month,
        Self::DayOfWeek,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Minute => "minute",
            Self::Hour => "hour",
            Self::DayOfMonth => "day-of-month",
            Self::Month => "month",
            Self::DayOfWeek => "day-of-week",
        }
    }

    /// Inclusive bounds of the numeric values the field accepts. Day-of-week
    /// admits `7` as a second spelling of Sunday on top of this range.
    #[must_use]
    pub const fn bounds(self) -> (u8, u8) {
        match self {
            Self::Minute => (0, 59),
            Self::Hour => (0, 23),
            Self::DayOfMonth => (1, 31),
            Self::Month => (1, 12),
            Self::DayOfWeek => (0, 6),
        }
    }

    const fn names(self) -> &'static [&'static str] {
        match self {
            Self::Month => &MONTH_NAMES,
            Self::DayOfWeek => &WEEKDAY_NAMES,
            Self::Minute | Self::Hour | Self::DayOfMonth => &[],
        }
    }
}

impl fmt::Display for CronField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.name())
    }
}

/// Why an expression was refused: which field, and a stable reason a
/// developer can act on.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CronError {
    field: Option<CronField>,
    reason: &'static str,
}

impl CronError {
    const fn at(field: CronField, reason: &'static str) -> Self {
        Self {
            field: Some(field),
            reason,
        }
    }

    const fn whole(reason: &'static str) -> Self {
        Self {
            field: None,
            reason,
        }
    }

    #[must_use]
    pub const fn field(&self) -> Option<CronField> {
        self.field
    }

    #[must_use]
    pub const fn reason(&self) -> &'static str {
        self.reason
    }
}

impl fmt::Display for CronError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.field {
            Some(field) => write!(
                formatter,
                "cron expression is invalid at {field}: {}",
                self.reason
            ),
            None => write!(formatter, "cron expression is invalid: {}", self.reason),
        }
    }
}

impl Error for CronError {}

/// A parsed expression: one bit per admissible value of each field, plus
/// whether the two day fields were written as `*`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CronSchedule {
    minutes: u64,
    hours: u32,
    days_of_month: u32,
    months: u16,
    days_of_week: u8,
    day_of_month_star: bool,
    day_of_week_star: bool,
}

impl CronSchedule {
    /// Parses five whitespace-separated fields.
    pub fn parse(expression: &str) -> Result<Self, CronError> {
        if expression.is_empty() {
            return Err(CronError::whole("expression is empty"));
        }
        if expression.len() > MAXIMUM_EXPRESSION_BYTES {
            return Err(CronError::whole("expression is longer than 128 bytes"));
        }
        if expression
            .chars()
            .any(|character| character.is_control() && character != '\t')
        {
            return Err(CronError::whole("expression contains control characters"));
        }
        let fields: Vec<&str> = expression.split_ascii_whitespace().collect();
        if fields.len() != 5 {
            return Err(CronError::whole(
                "expression must have exactly five fields: minute hour day-of-month month day-of-week",
            ));
        }
        let mut parsed = [0_u64; 5];
        for (index, field) in CronField::ALL.into_iter().enumerate() {
            parsed[index] = parse_field(field, fields[index])?;
        }
        Ok(Self {
            minutes: parsed[0],
            hours: u32::try_from(parsed[1]).expect("hour bits fit"),
            days_of_month: u32::try_from(parsed[2]).expect("day-of-month bits fit"),
            months: u16::try_from(parsed[3]).expect("month bits fit"),
            days_of_week: u8::try_from(parsed[4]).expect("day-of-week bits fit"),
            day_of_month_star: fields[2].starts_with('*'),
            day_of_week_star: fields[4].starts_with('*'),
        })
    }

    /// The first due time strictly after `unix_seconds`, on a whole minute,
    /// or `None` when nothing matches within [`SEARCH_HORIZON_DAYS`].
    #[must_use]
    pub fn next_after(&self, unix_seconds: u64) -> Option<u64> {
        // Due times sit on whole minutes; the search starts at the next one.
        let start = (unix_seconds / SECONDS_PER_MINUTE)
            .checked_add(1)?
            .checked_mul(SECONDS_PER_MINUTE)?;
        let first_day = start / SECONDS_PER_DAY;
        for offset in 0..=SEARCH_HORIZON_DAYS {
            let day = first_day.checked_add(offset)?;
            let day_start = day.checked_mul(SECONDS_PER_DAY)?;
            let civil = civil_date(day_start)?;
            if !self.day_matches(civil) {
                continue;
            }
            let earliest = if offset == 0 { start } else { day_start };
            for hour in 0..24_u64 {
                if self.hours & (1 << hour) == 0 {
                    continue;
                }
                let hour_start = day_start + hour * SECONDS_PER_HOUR;
                if hour_start + 59 * SECONDS_PER_MINUTE < earliest {
                    continue;
                }
                for minute in 0..60_u64 {
                    if self.minutes & (1 << minute) == 0 {
                        continue;
                    }
                    let candidate = hour_start + minute * SECONDS_PER_MINUTE;
                    if candidate >= earliest {
                        return Some(candidate);
                    }
                }
            }
        }
        None
    }

    /// Whether the minute at `unix_seconds` is a due time.
    #[must_use]
    pub fn matches(&self, unix_seconds: u64) -> bool {
        let minute = (unix_seconds / SECONDS_PER_MINUTE) % 60;
        let hour = (unix_seconds / SECONDS_PER_HOUR) % 24;
        let Some(civil) = civil_date(unix_seconds) else {
            return false;
        };
        self.minutes & (1 << minute) != 0
            && self.hours & (1 << hour) != 0
            && self.day_matches(civil)
    }

    /// The vixie rule: a `*` in either day field makes both fields
    /// conjunctive; two restricted fields are disjunctive.
    fn day_matches(&self, civil: CivilDate) -> bool {
        if self.months & (1 << civil.month) == 0 {
            return false;
        }
        let day_of_month = self.days_of_month & (1 << civil.day) != 0;
        let day_of_week = self.days_of_week & (1 << civil.weekday) != 0;
        if self.day_of_month_star || self.day_of_week_star {
            day_of_month && day_of_week
        } else {
            day_of_month || day_of_week
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CivilDate {
    /// 1..=12
    month: u8,
    /// 1..=31
    day: u8,
    /// 0 = Sunday ..= 6 = Saturday
    weekday: u8,
}

fn civil_date(unix_seconds: u64) -> Option<CivilDate> {
    let time = OffsetDateTime::from_unix_timestamp(i64::try_from(unix_seconds).ok()?).ok()?;
    Some(CivilDate {
        month: time.month() as u8,
        day: time.day(),
        weekday: time.weekday().number_days_from_sunday(),
    })
}

/// One field as a bit set over its numeric range.
fn parse_field(field: CronField, text: &str) -> Result<u64, CronError> {
    if text.is_empty() {
        return Err(CronError::at(field, "field is empty"));
    }
    let (low, high) = field.bounds();
    let mut bits = 0_u64;
    for item in text.split(',') {
        if item.is_empty() {
            return Err(CronError::at(field, "list has an empty entry"));
        }
        let (range, step) = match item.split_once('/') {
            Some((range, step)) => {
                let step =
                    step.parse::<u8>()
                        .ok()
                        .filter(|step| *step >= 1)
                        .ok_or(CronError::at(
                            field,
                            "step must be a whole number of at least 1",
                        ))?;
                if step > high - low + 1 {
                    return Err(CronError::at(
                        field,
                        "step is larger than the field's range",
                    ));
                }
                (range, step)
            }
            None => (item, 1),
        };
        let (from, to) = if range == "*" {
            (low, high)
        } else if let Some((from, to)) = range.split_once('-') {
            let from = parse_value(field, from)?;
            let to = parse_value(field, to)?;
            if to < from {
                return Err(CronError::at(field, "range end is before its start"));
            }
            (from, to)
        } else {
            if item.contains('/') {
                return Err(CronError::at(
                    field,
                    "a step needs a range such as `*/5` or `1-10/2`",
                ));
            }
            let value = parse_value(field, range)?;
            (value, value)
        };
        let mut value = from;
        while value <= to {
            // Day-of-week `7` is Sunday's second name; it lands on bit 0.
            let normalized = if field == CronField::DayOfWeek && value == 7 {
                0
            } else {
                value
            };
            bits |= 1 << normalized;
            value += step;
        }
    }
    Ok(bits)
}

/// A single value: a number within the field's bounds, a three-letter name
/// where the field has names, or `7` for Sunday.
fn parse_value(field: CronField, text: &str) -> Result<u8, CronError> {
    if text.is_empty() {
        return Err(CronError::at(field, "value is empty"));
    }
    let (low, high) = field.bounds();
    if text.bytes().all(|byte| byte.is_ascii_digit()) {
        if text.len() > 2 {
            return Err(CronError::at(field, "value is out of range"));
        }
        let value: u8 = text
            .parse()
            .map_err(|_| CronError::at(field, "value is out of range"))?;
        let sunday = field == CronField::DayOfWeek && value == 7;
        if (value < low || value > high) && !sunday {
            return Err(CronError::at(field, "value is out of range"));
        }
        return Ok(value);
    }
    let names = field.names();
    if names.is_empty() {
        return Err(CronError::at(field, "value must be a number"));
    }
    let lowered = text.to_ascii_lowercase();
    names
        .iter()
        .position(|name| *name == lowered)
        .map(|position| u8::try_from(position).expect("name index fits") + low)
        .ok_or(CronError::at(
            field,
            "value must be a number or a three-letter name",
        ))
}

#[cfg(test)]
mod tests {
    use time::{Date, Month, Time};

    use super::*;

    fn at(year: i32, month: u8, day: u8, hour: u8, minute: u8) -> u64 {
        let date = Date::from_calendar_date(year, Month::try_from(month).expect("month"), day)
            .expect("calendar date");
        let time = Time::from_hms(hour, minute, 0).expect("time of day");
        u64::try_from(date.with_time(time).assume_utc().unix_timestamp()).expect("after 1970")
    }

    fn next(expression: &str, from: u64) -> u64 {
        CronSchedule::parse(expression)
            .expect("valid expression")
            .next_after(from)
            .expect("a due time within the horizon")
    }

    #[test]
    fn nightly_at_three_is_the_same_instant_every_day() {
        let schedule = "0 3 * * *";
        // Before three o'clock: today.
        assert_eq!(
            next(schedule, at(2026, 3, 28, 1, 30)),
            at(2026, 3, 28, 3, 0)
        );
        // Exactly at three: strictly after, so tomorrow.
        assert_eq!(next(schedule, at(2026, 3, 28, 3, 0)), at(2026, 3, 29, 3, 0));
        // Seconds inside the due minute count as past it.
        assert_eq!(
            next(schedule, at(2026, 3, 28, 3, 0) + 30),
            at(2026, 3, 29, 3, 0)
        );
        // No daylight-saving shift: the European clock change on 2026-03-29
        // does not move a UTC schedule.
        assert_eq!(
            next(schedule, at(2026, 3, 29, 3, 0)) - at(2026, 3, 29, 3, 0),
            SECONDS_PER_DAY
        );
        // Across a year boundary.
        assert_eq!(
            next(schedule, at(2026, 12, 31, 23, 59)),
            at(2027, 1, 1, 3, 0)
        );
    }

    #[test]
    fn every_fifteen_minutes_on_weekdays_skips_the_weekend() {
        let schedule = "*/15 * * * 1-5";
        // 2026-08-28 is a Friday: 23:50 rolls to 00:00 on Monday.
        assert_eq!(
            next(schedule, at(2026, 8, 28, 23, 50)),
            at(2026, 8, 31, 0, 0)
        );
        assert_eq!(
            next(schedule, at(2026, 8, 26, 10, 7)),
            at(2026, 8, 26, 10, 15)
        );
        assert_eq!(
            next(schedule, at(2026, 8, 26, 10, 15)),
            at(2026, 8, 26, 10, 30)
        );
        assert_eq!(
            next(schedule, at(2026, 8, 26, 10, 45)),
            at(2026, 8, 26, 11, 0)
        );
        let parsed = CronSchedule::parse(schedule).expect("valid");
        assert!(parsed.matches(at(2026, 8, 26, 10, 30)));
        assert!(!parsed.matches(at(2026, 8, 29, 10, 30)), "Saturday");
        assert!(!parsed.matches(at(2026, 8, 26, 10, 31)));
    }

    #[test]
    fn leap_day_schedules_find_the_next_leap_year_within_the_horizon() {
        assert_eq!(
            next("0 0 29 2 *", at(2026, 8, 29, 12, 0)),
            at(2028, 2, 29, 0, 0)
        );
        assert_eq!(
            next("0 0 29 2 *", at(2028, 2, 29, 0, 0)),
            at(2032, 2, 29, 0, 0)
        );
        // February 30 never comes; the search gives up at the horizon.
        assert_eq!(
            CronSchedule::parse("0 0 30 2 *")
                .expect("syntactically valid")
                .next_after(at(2026, 1, 1, 0, 0)),
            None
        );
        // 31st of a 30-day month only: skips to the next month that has one.
        assert_eq!(
            next("0 12 31 * *", at(2026, 4, 1, 0, 0)),
            at(2026, 5, 31, 12, 0)
        );
    }

    #[test]
    fn vixie_day_semantics_use_either_field_when_both_are_restricted() {
        // 2026-09-01 is a Tuesday. "1st of the month OR any Monday".
        let either = "0 9 1 * mon";
        assert_eq!(next(either, at(2026, 8, 30, 0, 0)), at(2026, 8, 31, 9, 0)); // Monday
        assert_eq!(next(either, at(2026, 8, 31, 9, 0)), at(2026, 9, 1, 9, 0)); // the 1st
        assert_eq!(next(either, at(2026, 9, 1, 9, 0)), at(2026, 9, 7, 9, 0)); // next Monday
        // With a star in day-of-month, only Mondays: the 1st is skipped.
        let weekday_only = "0 9 * * 1";
        assert_eq!(
            next(weekday_only, at(2026, 8, 31, 9, 0)),
            at(2026, 9, 7, 9, 0)
        );
        // With a star in day-of-week, only the 1st.
        let monthday_only = "0 9 1 * *";
        assert_eq!(
            next(monthday_only, at(2026, 8, 31, 9, 0)),
            at(2026, 9, 1, 9, 0)
        );
        assert_eq!(
            next(monthday_only, at(2026, 9, 1, 9, 0)),
            at(2026, 10, 1, 9, 0)
        );
        // A stepped star still counts as a star: every other day AND Monday.
        let stepped = "0 9 */2 * 1";
        let parsed = CronSchedule::parse(stepped).expect("valid");
        assert!(parsed.day_of_month_star);
        assert!(parsed.matches(at(2026, 9, 7, 9, 0)), "Monday the 7th");
        assert!(!parsed.matches(at(2026, 9, 14, 9, 0)), "Monday the 14th");
        assert!(!parsed.matches(at(2026, 9, 9, 9, 0)), "Wednesday the 9th");
    }

    #[test]
    fn names_ranges_lists_steps_and_both_sundays_are_understood() {
        let parsed = CronSchedule::parse("0 0 * JAN,jun-Aug,dec Sun,7,fri-sat").expect("valid");
        assert_eq!(
            parsed.months,
            (1 << 1) | (1 << 6) | (1 << 7) | (1 << 8) | (1 << 12)
        );
        assert_eq!(parsed.days_of_week, 1 | (1 << 5) | (1 << 6));
        let sunday_seven = CronSchedule::parse("30 6 * * 7").expect("valid");
        let sunday_zero = CronSchedule::parse("30 6 * * 0").expect("valid");
        assert_eq!(sunday_seven, sunday_zero);
        // A range ending in 7 wraps Sunday onto bit 0.
        assert_eq!(
            CronSchedule::parse("0 0 * * 5-7")
                .expect("valid")
                .days_of_week,
            1 | (1 << 5) | (1 << 6)
        );
        let stepped = CronSchedule::parse("1-10/3 */6 1,15 * *").expect("valid");
        assert_eq!(stepped.minutes, (1 << 1) | (1 << 4) | (1 << 7) | (1 << 10));
        assert_eq!(stepped.hours, 1 | (1 << 6) | (1 << 12) | (1 << 18));
        assert_eq!(stepped.days_of_month, (1 << 1) | (1 << 15));
        assert_eq!(
            next("1-10/3 */6 1,15 * *", at(2026, 9, 1, 6, 4)),
            at(2026, 9, 1, 6, 7)
        );
        // Tabs and repeated spaces separate fields like single spaces.
        assert!(CronSchedule::parse("0\t3  *\t* *").is_ok());
        assert_eq!(
            CronSchedule::parse("59 23 31 12 6").expect("valid").minutes,
            1 << 59
        );
    }

    #[test]
    fn invalid_expressions_are_refused_with_the_field_named() {
        let cases: [(&str, Option<CronField>, &str); 22] = [
            ("", None, "expression is empty"),
            (
                "0 3 * *",
                None,
                "expression must have exactly five fields: minute hour day-of-month month day-of-week",
            ),
            (
                "0 0 3 * * *",
                None,
                "expression must have exactly five fields: minute hour day-of-month month day-of-week",
            ),
            (
                "@daily",
                None,
                "expression must have exactly five fields: minute hour day-of-month month day-of-week",
            ),
            (
                "60 * * * *",
                Some(CronField::Minute),
                "value is out of range",
            ),
            ("* 24 * * *", Some(CronField::Hour), "value is out of range"),
            (
                "* * 0 * *",
                Some(CronField::DayOfMonth),
                "value is out of range",
            ),
            (
                "* * 32 * *",
                Some(CronField::DayOfMonth),
                "value is out of range",
            ),
            (
                "* * * 13 *",
                Some(CronField::Month),
                "value is out of range",
            ),
            (
                "* * * * 8",
                Some(CronField::DayOfWeek),
                "value is out of range",
            ),
            (
                "* * * * 007",
                Some(CronField::DayOfWeek),
                "value is out of range",
            ),
            (
                "* * ? * *",
                Some(CronField::DayOfMonth),
                "value must be a number",
            ),
            (
                "* * L * *",
                Some(CronField::DayOfMonth),
                "value must be a number",
            ),
            (
                "* * * * 1#2",
                Some(CronField::DayOfWeek),
                "value must be a number or a three-letter name",
            ),
            (
                "* * * monday *",
                Some(CronField::Month),
                "value must be a number or a three-letter name",
            ),
            (
                "* * * * saturday",
                Some(CronField::DayOfWeek),
                "value must be a number or a three-letter name",
            ),
            (
                "10-5 * * * *",
                Some(CronField::Minute),
                "range end is before its start",
            ),
            (
                "*/0 * * * *",
                Some(CronField::Minute),
                "step must be a whole number of at least 1",
            ),
            (
                "*/61 * * * *",
                Some(CronField::Minute),
                "step is larger than the field's range",
            ),
            (
                "5/15 * * * *",
                Some(CronField::Minute),
                "a step needs a range such as `*/5` or `1-10/2`",
            ),
            (
                "1,,2 * * * *",
                Some(CronField::Minute),
                "list has an empty entry",
            ),
            (
                "0 3 * * *\n",
                None,
                "expression contains control characters",
            ),
        ];
        for (expression, field, reason) in cases {
            let error = CronSchedule::parse(expression)
                .expect_err(&format!("{expression:?} must be refused"));
            assert_eq!(error.field(), field, "{expression:?}");
            assert_eq!(error.reason(), reason, "{expression:?}");
        }
        let long = "0 3 * * *".to_owned() + &" ".repeat(MAXIMUM_EXPRESSION_BYTES);
        assert_eq!(
            CronSchedule::parse(&long).expect_err("too long").reason(),
            "expression is longer than 128 bytes"
        );
        assert_eq!(
            CronSchedule::parse("60 * * * *")
                .expect_err("refused")
                .to_string(),
            "cron expression is invalid at minute: value is out of range"
        );
        assert_eq!(
            CronSchedule::parse("").expect_err("refused").to_string(),
            "cron expression is invalid: expression is empty"
        );
    }

    #[test]
    fn every_minute_and_the_horizon_are_exact() {
        let now = at(2026, 8, 29, 10, 0) + 17;
        assert_eq!(next("* * * * *", now), at(2026, 8, 29, 10, 1));
        assert_eq!(
            next("* * * * *", at(2026, 8, 29, 10, 1)),
            at(2026, 8, 29, 10, 2)
        );
        // The horizon is inclusive of a match exactly five leap-years out.
        assert_eq!(
            CronSchedule::parse("0 0 29 2 *")
                .expect("valid")
                .next_after(at(2029, 3, 1, 0, 0)),
            Some(at(2032, 2, 29, 0, 0))
        );
        // A timestamp that cannot be represented yields nothing rather than a panic.
        assert_eq!(
            CronSchedule::parse("* * * * *")
                .expect("valid")
                .next_after(u64::MAX - 1),
            None
        );
    }
}
