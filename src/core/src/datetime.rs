//! Calendar dates, wall-clock times and UTC instants for the chat model, with exactly the ranges and rules
//! rv-rsmf had with chrono 0.4 (golden-tested in `golden/`): years -262143..=262142, a leap second kept as a
//! 60th second (nanoseconds 1e9..2e9 on the 59th second), RFC 3339 / RSMF text written the same way.
//!
//! jiff is used for time-zone data (`crate::zone`); the arithmetic here is plain proleptic Gregorian so that
//! every instant a chat export or a FILETIME can carry (up to year 60056) keeps working as before.

use std::fmt;

/// Year range of chrono's `NaiveDate` (kept so the same inputs are accepted and rejected).
pub const MIN_YEAR: i32 = (i32::MIN >> 13) + 1;
pub const MAX_YEAR: i32 = (i32::MAX >> 13) - 1;

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant's `days_from_civil`).
pub fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let m = i64::from(m);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Proleptic Gregorian (year, month, day) of a day count since 1970-01-01 (H. Hinnant's `civil_from_days`).
pub fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

pub fn is_leap(y: i64) -> bool {
    y % 4 == 0 && (y % 100 != 0 || y % 400 == 0)
}

pub fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if is_leap(y) => 29,
        _ => 28,
    }
}

/// `%Y` as chrono writes it: 4 digits inside 0000–9999, else a sign and at least 4 digits.
fn write_year(f: &mut impl fmt::Write, y: i64) -> fmt::Result {
    if (0..=9999).contains(&y) {
        write!(f, "{y:04}")
    } else {
        write!(f, "{y:+05}")
    }
}

/// A calendar date (chrono `NaiveDate`). Ordered chronologically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NaiveDate {
    y: i32,
    m: u8,
    d: u8,
}

impl NaiveDate {
    pub fn from_ymd_opt(y: i32, m: u32, d: u32) -> Option<NaiveDate> {
        if !(MIN_YEAR..=MAX_YEAR).contains(&y)
            || !(1..=12).contains(&m)
            || d == 0
            || d > days_in_month(i64::from(y), m)
        {
            return None;
        }
        Some(NaiveDate {
            y,
            m: m as u8,
            d: d as u8,
        })
    }

    /// Date of a day count since 1970-01-01, None outside the year range.
    pub fn from_days(days: i64) -> Option<NaiveDate> {
        let (y, m, d) = civil_from_days(days);
        let y = i32::try_from(y).ok()?;
        NaiveDate::from_ymd_opt(y, m, d)
    }

    pub fn year(&self) -> i32 {
        self.y
    }
    pub fn month(&self) -> u32 {
        u32::from(self.m)
    }
    pub fn day(&self) -> u32 {
        u32::from(self.d)
    }
    pub fn days(&self) -> i64 {
        days_from_civil(i64::from(self.y), self.month(), self.day())
    }
    /// Day of the year, 1..=366.
    pub fn ordinal(&self) -> u32 {
        (self.days() - days_from_civil(i64::from(self.y), 1, 1)) as u32 + 1
    }
    /// 0 = Monday … 6 = Sunday.
    pub fn weekday_from_monday(&self) -> u32 {
        // 1970-01-01 was a Thursday.
        (self.days() + 3).rem_euclid(7) as u32
    }

    /// Midnight-based time of day; chrono's `and_hms_opt`.
    pub fn and_hms_opt(&self, h: u32, mi: u32, s: u32) -> Option<NaiveDateTime> {
        self.and_hms_nano_opt(h, mi, s, 0)
    }

    /// chrono's `and_hms_nano_opt`: a nanosecond value of 1e9..2e9 is a leap second, only on second 59.
    pub fn and_hms_nano_opt(&self, h: u32, mi: u32, s: u32, nano: u32) -> Option<NaiveDateTime> {
        if h >= 24
            || mi >= 60
            || s >= 60
            || (nano >= 1_000_000_000 && s != 59)
            || nano >= 2_000_000_000
        {
            return None;
        }
        Some(NaiveDateTime {
            date: *self,
            secs: h * 3600 + mi * 60 + s,
            frac: nano,
        })
    }

    /// "2026-03-29" (`%Y-%m-%d`).
    pub fn ymd(&self) -> String {
        let mut s = String::new();
        write_year(&mut s, i64::from(self.y)).expect("string");
        format!("{s}-{:02}-{:02}", self.m, self.d)
    }
}

/// A wall-clock date and time without zone (chrono `NaiveDateTime`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NaiveDateTime {
    date: NaiveDate,
    /// Seconds since midnight.
    secs: u32,
    /// Nanoseconds; 1e9.. = leap second.
    frac: u32,
}

impl NaiveDateTime {
    pub fn date(&self) -> NaiveDate {
        self.date
    }
    pub fn hour(&self) -> u32 {
        self.secs / 3600
    }
    pub fn minute(&self) -> u32 {
        self.secs / 60 % 60
    }
    pub fn second(&self) -> u32 {
        self.secs % 60
    }
    pub fn nanosecond(&self) -> u32 {
        self.frac
    }
    /// Seconds since 1970-01-01T00:00:00 of this wall-clock time read as UTC.
    pub fn local_seconds(&self) -> i64 {
        self.date.days() * 86_400 + i64::from(self.secs)
    }

    /// From seconds since the epoch read as wall-clock time (None outside the year range).
    pub fn from_local_seconds(secs: i64, frac: u32) -> Option<NaiveDateTime> {
        let date = NaiveDate::from_days(secs.div_euclid(86_400))?;
        Some(NaiveDateTime {
            date,
            secs: secs.rem_euclid(86_400) as u32,
            frac,
        })
    }

    /// The same wall-clock reading as a UTC instant.
    pub fn and_utc(&self) -> UtcTime {
        UtcTime {
            secs: self.local_seconds(),
            nanos: self.frac,
        }
    }

    /// The instant of this wall-clock time at a fixed offset; None when it falls outside the range
    /// (chrono's `FixedOffset::from_local_datetime(..).single()`).
    pub fn at_offset(&self, offset: FixedOffset) -> Option<UtcTime> {
        UtcTime::from_parts(
            self.local_seconds() - i64::from(offset.local_minus_utc()),
            self.frac,
        )
    }

    /// "2026-03-29 02:30:00" (`%Y-%m-%d %H:%M:%S`; a leap second shows as :60).
    pub fn ymd_hms(&self) -> String {
        format!(
            "{} {:02}:{:02}:{:02}",
            self.date.ymd(),
            self.hour(),
            self.minute(),
            self.second() + u32::from(self.frac >= 1_000_000_000)
        )
    }
}

/// "2026-03-29" (chrono's `Display`).
impl fmt::Display for NaiveDate {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.ymd())
    }
}

/// "2026-03-29 02:30:00", with the shortest of 3/6/9 fraction digits when not zero (chrono's `Display`).
impl fmt::Display for NaiveDateTime {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.ymd_hms())?;
        write_fraction(f, self.frac % 1_000_000_000)
    }
}

fn write_fraction(f: &mut fmt::Formatter, frac: u32) -> fmt::Result {
    if frac == 0 {
        Ok(())
    } else if frac.is_multiple_of(1_000_000) {
        write!(f, ".{:03}", frac / 1_000_000)
    } else if frac.is_multiple_of(1_000) {
        write!(f, ".{:06}", frac / 1_000)
    } else {
        write!(f, ".{frac:09}")
    }
}

/// An instant in UTC (chrono `DateTime<Utc>`): seconds since the Unix epoch and nanoseconds, where
/// 1e9..2e9 nanoseconds on a `:59` second is a leap second. Ordered chronologically.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UtcTime {
    secs: i64,
    nanos: u32,
}

impl UtcTime {
    /// Within chrono's date range, with a valid (leap) nanosecond; the caller checked the nanoseconds.
    fn from_parts(secs: i64, nanos: u32) -> Option<UtcTime> {
        NaiveDate::from_days(secs.div_euclid(86_400))?;
        Some(UtcTime { secs, nanos })
    }

    /// chrono's `DateTime::from_timestamp`.
    pub fn from_timestamp(secs: i64, nanos: u32) -> Option<UtcTime> {
        let sod = secs.rem_euclid(86_400);
        if nanos >= 2_000_000_000 || (nanos >= 1_000_000_000 && sod % 60 != 59) {
            return None;
        }
        UtcTime::from_parts(secs, nanos)
    }

    /// chrono's `DateTime::from_timestamp_millis`.
    pub fn from_timestamp_millis(ms: i64) -> Option<UtcTime> {
        UtcTime::from_timestamp(ms.div_euclid(1000), ms.rem_euclid(1000) as u32 * 1_000_000)
    }

    /// Seconds since the Unix epoch.
    pub fn timestamp(&self) -> i64 {
        self.secs
    }
    /// Nanoseconds past the second (1e9.. during a leap second).
    pub fn timestamp_subsec_nanos(&self) -> u32 {
        self.nanos
    }

    /// The wall-clock reading in UTC.
    pub fn naive_utc(&self) -> NaiveDateTime {
        NaiveDateTime::from_local_seconds(self.secs, self.nanos).expect("in range by construction")
    }

    /// The wall-clock reading at `offset_secs` east of UTC (the date may leave the year range only for
    /// instants at its very ends, which then clamp like a UTC reading).
    pub fn naive_at(&self, offset_secs: i32) -> NaiveDateTime {
        NaiveDateTime::from_local_seconds(self.secs + i64::from(offset_secs), self.nanos)
            .unwrap_or_else(|| self.naive_utc())
    }

    /// A UTC date and time (tests and sample generators).
    pub fn from_ymd_hms(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Option<UtcTime> {
        Some(
            NaiveDate::from_ymd_opt(y, mo, d)?
                .and_hms_opt(h, mi, s)?
                .and_utc(),
        )
    }

    /// RFC 3339 with `+00:00` (chrono's `to_rfc3339`): "2026-03-29T01:30:00.5+00:00".
    pub fn to_rfc3339(&self) -> String {
        let z = self.to_string();
        format!("{}+00:00", &z[..z.len() - 1])
    }

    /// Shifted by whole seconds (tests and sample generators).
    pub fn plus_seconds(&self, s: i64) -> UtcTime {
        UtcTime::from_parts(self.secs + s, self.nanos).expect("in range")
    }

    /// "2026-03-29T01:30:00" (`%Y-%m-%dT%H:%M:%S`, UTC; a leap second shows as :60).
    pub fn ymd_t_hms(&self) -> String {
        let n = self.naive_utc();
        format!(
            "{}T{:02}:{:02}:{:02}",
            n.date.ymd(),
            n.hour(),
            n.minute(),
            n.second() + u32::from(n.frac >= 1_000_000_000)
        )
    }
}

/// RFC 3339 with `Z` and the shortest of 0/3/6/9 fraction digits, as chrono's serde wrote `DateTime<Utc>`.
impl fmt::Display for UtcTime {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.ymd_t_hms())?;
        write_fraction(f, self.nanos % 1_000_000_000)?;
        f.write_str("Z")
    }
}

impl serde::Serialize for UtcTime {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

/// A UTC offset in seconds, strictly within ±24 h (chrono `FixedOffset`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FixedOffset(i32);

impl FixedOffset {
    pub fn east_opt(secs: i32) -> Option<FixedOffset> {
        (-86_400 < secs && secs < 86_400).then_some(FixedOffset(secs))
    }
    pub fn local_minus_utc(&self) -> i32 {
        self.0
    }
}

/// "+02:00", "-05:30", "+00:00:15" (as chrono wrote it).
impl fmt::Display for FixedOffset {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let sign = if self.0 < 0 { '-' } else { '+' };
        let o = self.0.unsigned_abs();
        let (h, m, s) = (o / 3600, o / 60 % 60, o % 60);
        if s == 0 {
            write!(f, "{sign}{h:02}:{m:02}")
        } else {
            write!(f, "{sign}{h:02}:{m:02}:{s:02}")
        }
    }
}
