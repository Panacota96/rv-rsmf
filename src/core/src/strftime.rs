//! Reading times with a chrono `strftime` pattern (CSV `timestamp_format`) and RFC 3339, exactly as chrono 0.4
//! did: same specifiers, same leniency (spaces before numbers, 1-digit fields, case-insensitive names, `%.f`
//! optional), same consistency checks (weekday, ISO week, ordinal, quarter, `%s`) and the same error kinds and
//! messages, so a mapping that worked before reads the same instants and a cell refused before is refused
//! with the same text. Golden-tested in `golden/` against chrono's own results.
//!
//! The parsing rules follow chrono's `format::{strftime, parse, scan, parsed}` (MIT/Apache-2.0, © Kang Seonghoon
//! and contributors), rewritten here on rv-rsmf's own date types.

use crate::datetime::{FixedOffset, NaiveDate, NaiveDateTime, UtcTime};
use std::fmt;

/// chrono's `ParseErrorKind` (same messages).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParseError {
    OutOfRange,
    Impossible,
    NotEnough,
    Invalid,
    TooShort,
    TooLong,
    BadFormat,
}

use ParseError::*;

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self {
            OutOfRange => "input is out of range",
            Impossible => "no possible date and time matching input",
            NotEnough => "input is not enough for unique date and time",
            Invalid => "input contains invalid characters",
            TooShort => "premature end of input",
            TooLong => "trailing input",
            BadFormat => "bad or unsupported format string",
        })
    }
}

impl std::error::Error for ParseError {}

type Res<T> = Result<T, ParseError>;

// ------------------------------------------------------------------ format items

#[derive(Clone, Copy, Debug, PartialEq)]
enum Num {
    Year,
    YearDiv100,
    YearMod100,
    IsoYear,
    IsoYearMod100,
    Quarter,
    Month,
    Day,
    WeekFromSun,
    WeekFromMon,
    IsoWeek,
    DaysFromSun,
    WeekdayFromMon,
    Ordinal,
    Hour,
    Hour12,
    Minute,
    Second,
    Nanosecond,
    Timestamp,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Fix {
    ShortMonthName,
    LongMonthName,
    ShortWeekdayName,
    LongWeekdayName,
    AmPm,
    Nanosecond,
    Nanosecond3,
    Nanosecond6,
    Nanosecond9,
    Nanosecond3NoDot,
    Nanosecond6NoDot,
    Nanosecond9NoDot,
    TimezoneName,
    TimezoneOffset,
    TimezoneOffsetPermissive,
    Rfc3339,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Item<'a> {
    Literal(&'a str),
    Space,
    Numeric(Num),
    Fixed(Fix),
    Error,
}

use Item::{Fixed, Literal, Numeric, Space};

/// chrono's `StrftimeItems` (non-lenient, no locale): the whole item list of a pattern.
fn items(fmt: &str) -> Vec<Item<'_>> {
    let mut out = vec![];
    let mut rest = fmt;
    while let Some(c) = rest.chars().next() {
        if c != '%' {
            let pred: fn(char) -> bool = if c.is_whitespace() {
                |c| !c.is_whitespace()
            } else {
                |c| c.is_whitespace() || c == '%'
            };
            let end = rest.find(pred).unwrap_or(rest.len());
            out.push(if c.is_whitespace() {
                Space
            } else {
                Literal(&rest[..end])
            });
            rest = &rest[end..];
            continue;
        }
        let mut r = &rest[1..];
        let mut next = || -> Option<char> {
            let c = r.chars().next()?;
            r = &r[c.len_utf8()..];
            Some(c)
        };
        let (head, queue) = spec(&mut next);
        out.push(head);
        out.extend(queue);
        rest = r;
    }
    out
}

/// One `%` specifier: the first item and the items queued after it. `next` yields the pattern's characters.
fn spec(next: &mut dyn FnMut() -> Option<char>) -> (Item<'static>, Vec<Item<'static>>) {
    use Num::*;
    let err = (Item::Error, vec![]);
    let Some(mut c) = next() else { return err };
    let pad_override = matches!(c, '-' | '0' | '_');
    let alternate = c == '#';
    if pad_override || alternate {
        match next() {
            Some(x) => c = x,
            None => return err,
        }
    }
    if alternate && c != 'z' {
        return err;
    }
    let lit = Literal;
    let (head, queue): (Item<'static>, Vec<Item<'static>>) = match c {
        'A' => (Fixed(Fix::LongWeekdayName), vec![]),
        'B' => (Fixed(Fix::LongMonthName), vec![]),
        'C' => (Numeric(YearDiv100), vec![]),
        'D' | 'x' => (
            Numeric(Month),
            vec![lit("/"), Numeric(Day), lit("/"), Numeric(YearMod100)],
        ),
        'F' => (
            Numeric(Year),
            vec![lit("-"), Numeric(Month), lit("-"), Numeric(Day)],
        ),
        'G' => (Numeric(IsoYear), vec![]),
        'H' | 'k' => (Numeric(Hour), vec![]),
        'I' | 'l' => (Numeric(Hour12), vec![]),
        'M' => (Numeric(Minute), vec![]),
        'P' | 'p' => (Fixed(Fix::AmPm), vec![]),
        'R' => (Numeric(Hour), vec![lit(":"), Numeric(Minute)]),
        'S' => (Numeric(Second), vec![]),
        'T' | 'X' => (
            Numeric(Hour),
            vec![lit(":"), Numeric(Minute), lit(":"), Numeric(Second)],
        ),
        'U' => (Numeric(WeekFromSun), vec![]),
        'V' => (Numeric(IsoWeek), vec![]),
        'W' => (Numeric(WeekFromMon), vec![]),
        'Y' => (Numeric(Year), vec![]),
        'Z' => (Fixed(Fix::TimezoneName), vec![]),
        'a' => (Fixed(Fix::ShortWeekdayName), vec![]),
        'b' | 'h' => (Fixed(Fix::ShortMonthName), vec![]),
        'c' => (
            Fixed(Fix::ShortWeekdayName),
            vec![
                Space,
                Fixed(Fix::ShortMonthName),
                Space,
                Numeric(Day),
                Space,
                Numeric(Hour),
                lit(":"),
                Numeric(Minute),
                lit(":"),
                Numeric(Second),
                Space,
                Numeric(Year),
            ],
        ),
        'd' | 'e' => (Numeric(Day), vec![]),
        'f' => (Numeric(Nanosecond), vec![]),
        'g' => (Numeric(IsoYearMod100), vec![]),
        'j' => (Numeric(Ordinal), vec![]),
        'm' => (Numeric(Month), vec![]),
        'n' | 't' => (Space, vec![]),
        'q' => (Numeric(Quarter), vec![]),
        'r' => (
            Numeric(Hour12),
            vec![
                lit(":"),
                Numeric(Minute),
                lit(":"),
                Numeric(Second),
                Space,
                Fixed(Fix::AmPm),
            ],
        ),
        's' => (Numeric(Timestamp), vec![]),
        'u' => (Numeric(WeekdayFromMon), vec![]),
        'v' => (
            Numeric(Day),
            vec![
                lit("-"),
                Fixed(Fix::ShortMonthName),
                lit("-"),
                Numeric(Year),
            ],
        ),
        'w' => (Numeric(DaysFromSun), vec![]),
        'y' => (Numeric(YearMod100), vec![]),
        'z' if alternate => (Fixed(Fix::TimezoneOffsetPermissive), vec![]),
        'z' => (Fixed(Fix::TimezoneOffset), vec![]),
        '+' => (Fixed(Fix::Rfc3339), vec![]),
        ':' => {
            // ":z", "::z", ":::z" (chrono looks ahead without consuming on failure; any other text is an error).
            let mut colons = 0;
            loop {
                match next() {
                    Some(':') if colons < 2 => colons += 1,
                    Some('z') => break (Fixed(Fix::TimezoneOffset), vec![]),
                    _ => return err,
                }
            }
        }
        '.' => match next() {
            Some('3') if next() == Some('f') => (Fixed(Fix::Nanosecond3), vec![]),
            Some('6') if next() == Some('f') => (Fixed(Fix::Nanosecond6), vec![]),
            Some('9') if next() == Some('f') => (Fixed(Fix::Nanosecond9), vec![]),
            Some('f') => (Fixed(Fix::Nanosecond), vec![]),
            _ => return err,
        },
        '3' if next() == Some('f') => (Fixed(Fix::Nanosecond3NoDot), vec![]),
        '6' if next() == Some('f') => (Fixed(Fix::Nanosecond6NoDot), vec![]),
        '9' if next() == Some('f') => (Fixed(Fix::Nanosecond9NoDot), vec![]),
        '%' => (lit("%"), vec![]),
        _ => return err,
    };
    if pad_override && (!matches!(head, Numeric(_)) || !queue.is_empty()) {
        return err;
    }
    (head, queue)
}

// ------------------------------------------------------------------ scanning (chrono's format::scan)

fn number(s: &str, min: usize, max: usize) -> Res<(&str, i64)> {
    let b = s.as_bytes();
    if b.len() < min {
        return Err(TooShort);
    }
    let mut n = 0i64;
    for (i, c) in b.iter().take(max).copied().enumerate() {
        if !c.is_ascii_digit() {
            return if i < min {
                Err(Invalid)
            } else {
                Ok((&s[i..], n))
            };
        }
        n = n
            .checked_mul(10)
            .and_then(|n| n.checked_add(i64::from(c - b'0')))
            .ok_or(OutOfRange)?;
    }
    Ok((&s[max.min(b.len())..], n))
}

const SCALE: [i64; 10] = [
    0,
    100_000_000,
    10_000_000,
    1_000_000,
    100_000,
    10_000,
    1_000,
    100,
    10,
    1,
];

fn nanosecond(s: &str) -> Res<(&str, i64)> {
    let len = s.len();
    let (s, v) = number(s, 1, 9)?;
    let v = v * SCALE[len - s.len()];
    Ok((s.trim_start_matches(|c: char| c.is_ascii_digit()), v))
}

fn nanosecond_fixed(s: &str, digits: usize) -> Res<(&str, i64)> {
    let (s, v) = number(s, digits, digits)?;
    Ok((s, v * SCALE[digits]))
}

const MONTHS: [&[u8; 3]; 12] = [
    b"jan", b"feb", b"mar", b"apr", b"may", b"jun", b"jul", b"aug", b"sep", b"oct", b"nov", b"dec",
];
const MONTH_SUFFIXES: [&[u8]; 12] = [
    b"uary", b"ruary", b"ch", b"il", b"", b"e", b"y", b"ust", b"tember", b"ober", b"ember",
    b"ember",
];
/// Monday first.
const WEEKDAYS: [&[u8; 3]; 7] = [b"mon", b"tue", b"wed", b"thu", b"fri", b"sat", b"sun"];
const WEEKDAY_SUFFIXES: [&[u8]; 7] = [
    b"day", b"sday", b"nesday", b"rsday", b"day", b"urday", b"day",
];

fn short_name<'a>(s: &'a str, names: &[&[u8; 3]]) -> Res<(&'a str, usize)> {
    let b = s.as_bytes();
    if b.len() < 3 {
        return Err(TooShort);
    }
    let key = [b[0] | 32, b[1] | 32, b[2] | 32];
    let i = names.iter().position(|n| **n == key).ok_or(Invalid)?;
    Ok((&s[3..], i))
}

fn long_name<'a>(s: &'a str, names: &[&[u8; 3]], suffixes: &[&[u8]]) -> Res<(&'a str, usize)> {
    let (mut s, i) = short_name(s, names)?;
    let suf = suffixes[i];
    if s.len() >= suf.len() && s.as_bytes()[..suf.len()].eq_ignore_ascii_case(suf) {
        s = &s[suf.len()..];
    }
    Ok((s, i))
}

fn colon_or_space(s: &str) -> &str {
    s.trim_start_matches(|c: char| c == ':' || c.is_whitespace())
}

/// chrono's `scan::timezone_offset` with the minus sign U+2212 allowed.
fn timezone_offset(
    mut s: &str,
    colon: fn(&str) -> Res<&str>,
    allow_zulu: bool,
    allow_missing_minutes: bool,
) -> Res<(&str, i32)> {
    if allow_zulu && matches!(s.as_bytes().first(), Some(b'Z' | b'z')) {
        return Ok((&s[1..], 0));
    }
    let digits = |s: &str| {
        let b = s.as_bytes();
        if b.len() < 2 {
            Err(TooShort)
        } else {
            Ok((b[0], b[1]))
        }
    };
    let negative = match s.chars().next() {
        Some('+') => {
            s = &s[1..];
            false
        }
        Some('-') => {
            s = &s[1..];
            true
        }
        Some('\u{2212}') => {
            s = &s['\u{2212}'.len_utf8()..];
            true
        }
        Some(_) => return Err(Invalid),
        None => return Err(TooShort),
    };
    let hours = match digits(s)? {
        (h1 @ b'0'..=b'9', h2 @ b'0'..=b'9') => i32::from((h1 - b'0') * 10 + (h2 - b'0')),
        _ => return Err(Invalid),
    };
    s = colon(&s[2..])?;
    let minutes = if let Ok(ds) = digits(s) {
        match ds {
            (m1 @ b'0'..=b'5', m2 @ b'0'..=b'9') => i32::from((m1 - b'0') * 10 + (m2 - b'0')),
            (b'6'..=b'9', b'0'..=b'9') => return Err(OutOfRange),
            _ => return Err(Invalid),
        }
    } else if allow_missing_minutes {
        0
    } else {
        return Err(TooShort);
    };
    s = match s.len() {
        len if len >= 2 => &s[2..],
        0 => s,
        _ => return Err(TooShort),
    };
    let secs = hours * 3600 + minutes * 60;
    Ok((s, if negative { -secs } else { secs }))
}

// ------------------------------------------------------------------ parsed fields (chrono's format::Parsed)

#[derive(Clone, Debug, Default, PartialEq)]
struct Parsed {
    year: Option<i32>,
    year_div_100: Option<i32>,
    year_mod_100: Option<i32>,
    isoyear: Option<i32>,
    isoyear_div_100: Option<i32>,
    isoyear_mod_100: Option<i32>,
    quarter: Option<u32>,
    month: Option<u32>,
    week_from_sun: Option<u32>,
    week_from_mon: Option<u32>,
    isoweek: Option<u32>,
    /// 0 = Monday.
    weekday: Option<u32>,
    ordinal: Option<u32>,
    day: Option<u32>,
    hour_div_12: Option<u32>,
    hour_mod_12: Option<u32>,
    minute: Option<u32>,
    second: Option<u32>,
    nanosecond: Option<u32>,
    timestamp: Option<i64>,
    offset: Option<i32>,
}

fn set<T: PartialEq>(old: &mut Option<T>, new: T) -> Res<()> {
    match old {
        Some(o) if *o != new => Err(Impossible),
        _ => {
            *old = Some(new);
            Ok(())
        }
    }
}

fn ranged(v: i64, lo: i64, hi: i64) -> Res<u32> {
    if (lo..=hi).contains(&v) {
        Ok(v as u32)
    } else {
        Err(OutOfRange)
    }
}

fn days_since(lhs: u32, rhs: u32) -> u32 {
    if lhs < rhs { 7 + lhs - rhs } else { lhs - rhs }
}

const SUN: u32 = 6;
const MON: u32 = 0;

/// chrono date helpers on rv-rsmf's NaiveDate.
fn from_yo(year: i32, ordinal: u32) -> Option<NaiveDate> {
    let jan1 = NaiveDate::from_ymd_opt(year, 1, 1)?;
    let days_in_year = if crate::datetime::is_leap(i64::from(year)) {
        366
    } else {
        365
    };
    if ordinal == 0 || ordinal > days_in_year {
        return None;
    }
    NaiveDate::from_days(jan1.days() + i64::from(ordinal) - 1)
}

fn iso_weeks_in_year(year: i64) -> u32 {
    let jan1 = crate::datetime::days_from_civil(year, 1, 1);
    let wd = (jan1 + 3).rem_euclid(7); // 0 = Monday
    if wd == 3 || (wd == 2 && crate::datetime::is_leap(year)) {
        53
    } else {
        52
    }
}

/// (ISO year, ISO week).
fn iso_week(d: &NaiveDate) -> (i32, u32) {
    let y = i64::from(d.year());
    let w = (d.ordinal() as i64 - i64::from(d.weekday_from_monday()) + 9) / 7;
    if w < 1 {
        ((y - 1) as i32, iso_weeks_in_year(y - 1))
    } else if w as u32 > iso_weeks_in_year(y) {
        ((y + 1) as i32, 1)
    } else {
        (y as i32, w as u32)
    }
}

fn from_isoywd(year: i32, week: u32, weekday: u32) -> Option<NaiveDate> {
    if week < 1 || week > iso_weeks_in_year(i64::from(year)) {
        return None;
    }
    NaiveDate::from_ymd_opt(year, 1, 1)?;
    let jan4 = crate::datetime::days_from_civil(i64::from(year), 1, 4);
    let monday = jan4 - (jan4 + 3).rem_euclid(7);
    NaiveDate::from_days(monday + i64::from(week - 1) * 7 + i64::from(weekday))
}

fn weeks_from(d: &NaiveDate, start: u32) -> i32 {
    (d.ordinal() as i32 - days_since(d.weekday_from_monday(), start) as i32 + 6) / 7
}

fn resolve_week_date(year: i32, week: u32, weekday: u32, start: u32) -> Res<NaiveDate> {
    if week > 53 {
        return Err(OutOfRange);
    }
    let jan1 = from_yo(year, 1).ok_or(OutOfRange)?;
    let first_week_start = 1 + days_since(start, jan1.weekday_from_monday()) as i32;
    let wd = days_since(weekday, start) as i32;
    let ordinal = first_week_start + (week as i32 - 1) * 7 + wd;
    if ordinal <= 0 {
        return Err(Impossible);
    }
    from_yo(year, ordinal as u32).ok_or(Impossible)
}

impl Parsed {
    fn set_num(&mut self, n: Num, v: i64) -> Res<()> {
        use Num::*;
        match n {
            Year => set(&mut self.year, i32::try_from(v).map_err(|_| OutOfRange)?),
            YearDiv100 => set(
                &mut self.year_div_100,
                ranged(v, 0, i64::from(i32::MAX))? as i32,
            ),
            YearMod100 => set(&mut self.year_mod_100, ranged(v, 0, 99)? as i32),
            IsoYear => set(&mut self.isoyear, i32::try_from(v).map_err(|_| OutOfRange)?),
            IsoYearMod100 => set(&mut self.isoyear_mod_100, ranged(v, 0, 99)? as i32),
            Quarter => set(&mut self.quarter, ranged(v, 1, 4)?),
            Month => set(&mut self.month, ranged(v, 1, 12)?),
            Day => set(&mut self.day, ranged(v, 1, 31)?),
            WeekFromSun => set(&mut self.week_from_sun, ranged(v, 0, 53)?),
            WeekFromMon => set(&mut self.week_from_mon, ranged(v, 0, 53)?),
            IsoWeek => set(&mut self.isoweek, ranged(v, 1, 53)?),
            DaysFromSun => {
                let d = ranged(v, 0, 6)?;
                set(&mut self.weekday, (d + 6) % 7)
            }
            WeekdayFromMon => set(&mut self.weekday, ranged(v, 1, 7)? - 1),
            Ordinal => set(&mut self.ordinal, ranged(v, 1, 366)?),
            Hour => {
                let h = ranged(v, 0, 23)?;
                set(&mut self.hour_div_12, h / 12)?;
                set(&mut self.hour_mod_12, h % 12)
            }
            Hour12 => {
                let h = ranged(v, 1, 12)?;
                set(&mut self.hour_mod_12, if h == 12 { 0 } else { h })
            }
            Minute => set(&mut self.minute, ranged(v, 0, 59)?),
            Second => set(&mut self.second, ranged(v, 0, 60)?),
            Nanosecond => set(&mut self.nanosecond, ranged(v, 0, 999_999_999)?),
            Timestamp => set(&mut self.timestamp, v),
        }
    }

    fn set_offset(&mut self, v: i64) -> Res<()> {
        set(&mut self.offset, i32::try_from(v).map_err(|_| OutOfRange)?)
    }

    fn to_naive_date(&self) -> Res<NaiveDate> {
        fn resolve_year(y: Option<i32>, q: Option<i32>, r: Option<i32>) -> Res<Option<i32>> {
            match (y, q, r) {
                (y, None, None) => Ok(y),
                (Some(y), q, r @ Some(0..=99)) | (Some(y), q, r @ None) => {
                    if y < 0 {
                        return Err(Impossible);
                    }
                    let (q_, r_) = (y / 100, y % 100);
                    if q.unwrap_or(q_) == q_ && r.unwrap_or(r_) == r_ {
                        Ok(Some(y))
                    } else {
                        Err(Impossible)
                    }
                }
                (None, Some(q), Some(r @ 0..=99)) => {
                    if q < 0 {
                        return Err(Impossible);
                    }
                    let y = q.checked_mul(100).and_then(|v| v.checked_add(r));
                    Ok(Some(y.ok_or(OutOfRange)?))
                }
                (None, None, Some(r @ 0..=99)) => Ok(Some(r + if r < 70 { 2000 } else { 1900 })),
                (None, Some(_), None) => Err(NotEnough),
                (_, _, Some(_)) => Err(OutOfRange),
            }
        }
        let given_year = resolve_year(self.year, self.year_div_100, self.year_mod_100)?;
        let given_isoyear = resolve_year(self.isoyear, self.isoyear_div_100, self.isoyear_mod_100)?;
        let split = |y: i32| {
            if y >= 0 {
                (Some(y / 100), Some(y % 100))
            } else {
                (None, None)
            }
        };
        let verify_ymd = |d: NaiveDate| {
            let y = d.year();
            let (q, r) = split(y);
            self.year.unwrap_or(y) == y
                && self.year_div_100.or(q) == q
                && self.year_mod_100.or(r) == r
                && self.month.unwrap_or(d.month()) == d.month()
                && self.day.unwrap_or(d.day()) == d.day()
        };
        let verify_isoweekdate = |d: NaiveDate| {
            let (iy, iw) = iso_week(&d);
            let (q, r) = split(iy);
            let wd = d.weekday_from_monday();
            self.isoyear.unwrap_or(iy) == iy
                && self.isoyear_div_100.or(q) == q
                && self.isoyear_mod_100.or(r) == r
                && self.isoweek.unwrap_or(iw) == iw
                && self.weekday.unwrap_or(wd) == wd
        };
        let verify_ordinal = |d: NaiveDate| {
            let (ws, wm) = (weeks_from(&d, SUN), weeks_from(&d, MON));
            self.ordinal.unwrap_or(d.ordinal()) == d.ordinal()
                && self.week_from_sun.map_or(ws, |v| v as i32) == ws
                && self.week_from_mon.map_or(wm, |v| v as i32) == wm
        };
        let (verified, date) = match (given_year, given_isoyear) {
            (Some(year), _) if self.month.is_some() && self.day.is_some() => {
                let d = NaiveDate::from_ymd_opt(year, self.month.unwrap(), self.day.unwrap())
                    .ok_or(OutOfRange)?;
                (verify_isoweekdate(d) && verify_ordinal(d), d)
            }
            (Some(year), _) if self.ordinal.is_some() => {
                let d = from_yo(year, self.ordinal.unwrap()).ok_or(OutOfRange)?;
                (
                    verify_ymd(d) && verify_isoweekdate(d) && verify_ordinal(d),
                    d,
                )
            }
            (Some(year), _) if self.week_from_sun.is_some() && self.weekday.is_some() => {
                let d = resolve_week_date(
                    year,
                    self.week_from_sun.unwrap(),
                    self.weekday.unwrap(),
                    SUN,
                )?;
                (
                    verify_ymd(d) && verify_isoweekdate(d) && verify_ordinal(d),
                    d,
                )
            }
            (Some(year), _) if self.week_from_mon.is_some() && self.weekday.is_some() => {
                let d = resolve_week_date(
                    year,
                    self.week_from_mon.unwrap(),
                    self.weekday.unwrap(),
                    MON,
                )?;
                (
                    verify_ymd(d) && verify_isoweekdate(d) && verify_ordinal(d),
                    d,
                )
            }
            (_, Some(iy)) if self.isoweek.is_some() && self.weekday.is_some() => {
                let d = from_isoywd(iy, self.isoweek.unwrap(), self.weekday.unwrap())
                    .ok_or(OutOfRange)?;
                (verify_ymd(d) && verify_ordinal(d), d)
            }
            _ => return Err(NotEnough),
        };
        if !verified {
            return Err(Impossible);
        }
        if let Some(q) = self.quarter
            && q != (date.month() - 1) / 3 + 1
        {
            return Err(Impossible);
        }
        Ok(date)
    }

    /// (hour, minute, second, nanosecond) checked like chrono's `to_naive_time`.
    fn to_naive_time(&self) -> Res<(u32, u32, u32, u32)> {
        let div = match self.hour_div_12 {
            Some(v @ 0..=1) => v,
            Some(_) => return Err(OutOfRange),
            None => return Err(NotEnough),
        };
        let m12 = match self.hour_mod_12 {
            Some(v @ 0..=11) => v,
            Some(_) => return Err(OutOfRange),
            None => return Err(NotEnough),
        };
        let minute = match self.minute {
            Some(v @ 0..=59) => v,
            Some(_) => return Err(OutOfRange),
            None => return Err(NotEnough),
        };
        let (second, mut nano) = match self.second.unwrap_or(0) {
            v @ 0..=59 => (v, 0),
            60 => (59, 1_000_000_000),
            _ => return Err(OutOfRange),
        };
        nano += match self.nanosecond {
            Some(v @ 0..=999_999_999) if self.second.is_some() => v,
            Some(0..=999_999_999) => return Err(NotEnough),
            Some(_) => return Err(OutOfRange),
            None => 0,
        };
        Ok((div * 12 + m12, minute, second, nano))
    }

    fn to_naive_datetime_with_offset(&self, offset: i32) -> Res<NaiveDateTime> {
        let date = self.to_naive_date();
        let time = self.to_naive_time();
        if let (Ok(d), Ok((h, mi, s, n))) = (date, time) {
            let dt = d.and_hms_nano_opt(h, mi, s, n).ok_or(OutOfRange)?;
            let ts = dt.local_seconds() - i64::from(offset);
            if let Some(given) = self.timestamp
                && given != ts
                && !(dt.nanosecond() >= 1_000_000_000 && given == ts + 1)
            {
                return Err(Impossible);
            }
            Ok(dt)
        } else if let Some(timestamp) = self.timestamp {
            match (date, time) {
                (Err(OutOfRange), _) | (_, Err(OutOfRange)) => return Err(OutOfRange),
                (Err(Impossible), _) | (_, Err(Impossible)) => return Err(Impossible),
                _ => {}
            }
            let ts = timestamp.checked_add(i64::from(offset)).ok_or(OutOfRange)?;
            let mut dt = UtcTime::from_timestamp(ts, 0)
                .ok_or(OutOfRange)?
                .naive_utc();
            let mut p = self.clone();
            if p.second == Some(60) {
                match dt.second() {
                    59 => {}
                    0 => {
                        dt = NaiveDateTime::from_local_seconds(
                            dt.local_seconds() - 1,
                            dt.nanosecond(),
                        )
                        .ok_or(OutOfRange)?;
                    }
                    _ => return Err(Impossible),
                }
            } else {
                p.set_num(Num::Second, i64::from(dt.second()))?;
            }
            p.set_num(Num::Year, i64::from(dt.date().year()))?;
            p.set_num(Num::Ordinal, i64::from(dt.date().ordinal()))?;
            p.set_num(Num::Hour, i64::from(dt.hour()))?;
            p.set_num(Num::Minute, i64::from(dt.minute()))?;
            let d = p.to_naive_date()?;
            let (h, mi, s, n) = p.to_naive_time()?;
            d.and_hms_nano_opt(h, mi, s, n).ok_or(OutOfRange)
        } else {
            date?;
            time?;
            unreachable!("one of them failed")
        }
    }

    fn to_datetime(&self) -> Res<UtcTime> {
        let offset = match (self.offset, self.timestamp) {
            (Some(o), _) => o,
            (None, Some(_)) => 0,
            (None, None) => return Err(NotEnough),
        };
        let dt = self.to_naive_datetime_with_offset(offset)?;
        let off = FixedOffset::east_opt(offset).ok_or(OutOfRange)?;
        dt.at_offset(off).ok_or(Impossible)
    }
}

// ------------------------------------------------------------------ parsing (chrono's format::parse)

fn parse_items<'s>(p: &mut Parsed, mut s: &'s str, items: &[Item]) -> Res<&'s str> {
    for item in items {
        match *item {
            Literal(prefix) => {
                if s.len() < prefix.len() {
                    return Err(TooShort);
                }
                if !s.starts_with(prefix) {
                    return Err(Invalid);
                }
                s = &s[prefix.len()..];
            }
            Space => s = s.trim_start(),
            Numeric(n) => {
                use Num::*;
                let width = match n {
                    Year | IsoYear => 4,
                    YearDiv100 | YearMod100 | IsoYearMod100 | Month | Day | WeekFromSun
                    | WeekFromMon | IsoWeek | Hour | Hour12 | Minute | Second => 2,
                    Quarter | DaysFromSun | WeekdayFromMon => 1,
                    Ordinal => 3,
                    Nanosecond => 9,
                    Timestamp => usize::MAX,
                };
                s = s.trim_start();
                let (rest, v) = if matches!(n, Year | IsoYear) {
                    if let Some(r) = s.strip_prefix('-') {
                        let (r, v) = number(r, 1, usize::MAX)?;
                        (r, 0i64.checked_sub(v).ok_or(OutOfRange)?)
                    } else if let Some(r) = s.strip_prefix('+') {
                        number(r, 1, usize::MAX)?
                    } else {
                        number(s, 1, width)?
                    }
                } else {
                    number(s, 1, width)?
                };
                s = rest;
                p.set_num(n, v)?;
            }
            Fixed(f) => match f {
                Fix::ShortMonthName => {
                    let (r, m) = short_name(s, &MONTHS)?;
                    s = r;
                    p.set_num(Num::Month, m as i64 + 1)?;
                }
                Fix::LongMonthName => {
                    let (r, m) = long_name(s, &MONTHS, &MONTH_SUFFIXES)?;
                    s = r;
                    p.set_num(Num::Month, m as i64 + 1)?;
                }
                Fix::ShortWeekdayName => {
                    let (r, d) = short_name(s, &WEEKDAYS)?;
                    s = r;
                    set(&mut p.weekday, d as u32)?;
                }
                Fix::LongWeekdayName => {
                    let (r, d) = long_name(s, &WEEKDAYS, &WEEKDAY_SUFFIXES)?;
                    s = r;
                    set(&mut p.weekday, d as u32)?;
                }
                Fix::AmPm => {
                    let b = s.as_bytes();
                    if b.len() < 2 {
                        return Err(TooShort);
                    }
                    let pm = match (b[0] | 32, b[1] | 32) {
                        (b'a', b'm') => false,
                        (b'p', b'm') => true,
                        _ => return Err(Invalid),
                    };
                    set(&mut p.hour_div_12, u32::from(pm))?;
                    s = &s[2..];
                }
                Fix::Nanosecond => {
                    if let Some(r) = s.strip_prefix('.') {
                        let (r, v) = nanosecond(r)?;
                        s = r;
                        p.set_num(Num::Nanosecond, v)?;
                    }
                }
                Fix::Nanosecond3 | Fix::Nanosecond6 | Fix::Nanosecond9 => {
                    if let Some(r) = s.strip_prefix('.') {
                        let digits = match f {
                            Fix::Nanosecond3 => 3,
                            Fix::Nanosecond6 => 6,
                            _ => 9,
                        };
                        let (r, v) = nanosecond_fixed(r, digits)?;
                        s = r;
                        p.set_num(Num::Nanosecond, v)?;
                    }
                }
                Fix::Nanosecond3NoDot | Fix::Nanosecond6NoDot | Fix::Nanosecond9NoDot => {
                    let digits = match f {
                        Fix::Nanosecond3NoDot => 3,
                        Fix::Nanosecond6NoDot => 6,
                        _ => 9,
                    };
                    if s.len() < digits {
                        return Err(TooShort);
                    }
                    let (r, v) = nanosecond_fixed(s, digits)?;
                    s = r;
                    p.set_num(Num::Nanosecond, v)?;
                }
                Fix::TimezoneName => s = s.trim_start_matches(|c: char| !c.is_whitespace()),
                Fix::TimezoneOffset | Fix::TimezoneOffsetPermissive => {
                    let permissive = f == Fix::TimezoneOffsetPermissive;
                    let (r, o) = timezone_offset(
                        s.trim_start(),
                        |s| Ok(colon_or_space(s)),
                        permissive,
                        permissive,
                    )?;
                    s = r;
                    p.set_offset(i64::from(o))?;
                }
                Fix::Rfc3339 => s = parse_rfc3339_relaxed(p, s)?,
            },
            Item::Error => return Err(BadFormat),
        }
    }
    Ok(s)
}

fn parse_rfc3339_relaxed<'s>(p: &mut Parsed, s: &'s str) -> Res<&'s str> {
    use Num::*;
    const DATE: &[Item] = &[
        Numeric(Year),
        Space,
        Literal("-"),
        Numeric(Month),
        Space,
        Literal("-"),
        Numeric(Day),
    ];
    const TIME: &[Item] = &[
        Numeric(Hour),
        Space,
        Literal(":"),
        Numeric(Minute),
        Space,
        Literal(":"),
        Numeric(Second),
        Fixed(Fix::Nanosecond),
        Space,
    ];
    let s = parse_items(p, s, DATE)?;
    let s = match s.as_bytes().first() {
        Some(b't' | b'T' | b' ') => &s[1..],
        Some(_) => return Err(Invalid),
        None => return Err(TooShort),
    };
    let s = parse_items(p, s, TIME)?.trim_start();
    let (s, offset) = if s.len() >= 3 && s.as_bytes()[..3].eq_ignore_ascii_case(b"UTC") {
        (&s[3..], 0)
    } else {
        timezone_offset(s, |s| Ok(colon_or_space(s)), true, false)?
    };
    p.set_offset(i64::from(offset))?;
    Ok(s)
}

fn parse_all(s: &str, fmt: &str) -> Res<Parsed> {
    let mut p = Parsed::default();
    match parse_items(&mut p, s, &items(fmt))? {
        "" => Ok(p),
        _ => Err(TooLong),
    }
}

/// chrono's `NaiveDateTime::parse_from_str`.
pub fn parse_naive_datetime(s: &str, fmt: &str) -> Res<NaiveDateTime> {
    parse_all(s, fmt)?.to_naive_datetime_with_offset(0)
}

/// chrono's `NaiveDate::parse_from_str`.
pub fn parse_naive_date(s: &str, fmt: &str) -> Res<NaiveDate> {
    parse_all(s, fmt)?.to_naive_date()
}

/// chrono's `DateTime::parse_from_str` (the pattern gives an offset), as a UTC instant.
pub fn parse_datetime(s: &str, fmt: &str) -> Res<UtcTime> {
    parse_all(s, fmt)?.to_datetime()
}

/// chrono's `DateTime::parse_from_rfc3339` (strict: `YYYY-MM-DD`, `T`/`t`/space, `hh:mm:ss`, optional
/// fraction, `Z`/`z`/`±hh:mm`; `:60` is a leap second), as a UTC instant.
pub fn parse_rfc3339(s: &str) -> Res<UtcTime> {
    let b = s.as_bytes();
    if b.len() < 19 {
        return Err(TooShort);
    }
    let digit = |i: usize| {
        if b[i].is_ascii_digit() {
            Ok(u32::from(b[i] - b'0'))
        } else {
            Err(Invalid)
        }
    };
    let year = digit(0)? * 1000 + digit(1)? * 100 + digit(2)? * 10 + digit(3)?;
    if b[4] != b'-' {
        return Err(Invalid);
    }
    let month = digit(5)? * 10 + digit(6)?;
    if b[7] != b'-' {
        return Err(Invalid);
    }
    let day = digit(8)? * 10 + digit(9)?;
    let date = NaiveDate::from_ymd_opt(year as i32, month, day).ok_or(OutOfRange)?;
    if !matches!(b[10], b't' | b'T' | b' ') {
        return Err(Invalid);
    }
    let hour = digit(11)? * 10 + digit(12)?;
    if b[13] != b':' {
        return Err(Invalid);
    }
    let min = digit(14)? * 10 + digit(15)?;
    if b[16] != b':' {
        return Err(Invalid);
    }
    let sec = digit(17)? * 10 + digit(18)?;
    let (sec, extra) = if sec == 60 {
        (59, 1_000_000_000)
    } else {
        (sec, 0)
    };
    let (rest, nano) = if b.get(19) == Some(&b'.') {
        let (r, n) = nanosecond(&s[20..])?;
        (r, extra + n as u32)
    } else {
        (&s[19..], extra)
    };
    let dt = date
        .and_hms_nano_opt(hour, min, sec, nano)
        .ok_or(OutOfRange)?;
    fn colon(s: &str) -> Res<&str> {
        match s.as_bytes().first() {
            Some(b':') => Ok(&s[1..]),
            Some(_) => Err(Invalid),
            None => Err(TooShort),
        }
    }
    let (rest, offset) = timezone_offset(rest, colon, true, false)?;
    if !rest.is_empty() {
        return Err(TooLong);
    }
    let off = FixedOffset::east_opt(offset).ok_or(OutOfRange)?;
    Ok(dt.at_offset(off).expect("RFC 3339 years stay in range"))
}
