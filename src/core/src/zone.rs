//! IANA time zones for wall-clock times in chat exports: names and UTC offsets come from jiff's time-zone
//! database (bundled on Windows); the rules for turning a wall-clock time into an instant are exactly the
//! ones rv-rsmf had with chrono-tz 0.10 (golden-tested in `golden/`):
//!
//! - names are matched exactly (case-sensitive), every name chrono-tz knew is accepted (links keep their own
//!   name: `Asia/Calcutta` stays `Asia/Calcutta`), and `Factory` is refused as before;
//! - the zone's offsets are a table of transitions up to the end of 2099; after the last one the offset in
//!   force stays forever (chrono-tz's table stopped there, so a summer time in 2100 or later is not applied);
//! - a wall-clock time is resolved against that table with chrono-tz's own search, so an overlap gives the
//!   earlier instant and a gap uses the offset before the change (see `rsmf::local_to_utc`).

use crate::datetime::{NaiveDateTime, UtcTime};
use std::cmp::Ordering;
use std::fmt;
use std::str::FromStr;
use std::sync::Arc;

/// First second of 2100 (UTC): chrono-tz's transition tables ended before it.
const TABLE_END: i64 = 4_102_444_800;

/// A named IANA time zone.
#[derive(Clone)]
pub struct Tz {
    name: Arc<str>,
    /// Offset (seconds east of UTC) before the first transition.
    first: i32,
    /// (UTC second of the transition, offset from then on), ascending.
    rest: Arc<[(i64, i32)]>,
}

/// The result of reading a wall-clock time in a zone (chrono's `LocalResult`, offsets in seconds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalOffset {
    Single(i32),
    /// Earlier instant's offset first.
    Ambiguous(i32, i32),
    None,
}

impl LocalOffset {
    pub fn earliest(&self) -> Option<i32> {
        match *self {
            LocalOffset::Single(o) | LocalOffset::Ambiguous(o, _) => Some(o),
            LocalOffset::None => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnknownZone;

impl fmt::Display for UnknownZone {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("unknown time zone")
    }
}

impl std::error::Error for UnknownZone {}

impl FromStr for Tz {
    type Err = UnknownZone;
    fn from_str(name: &str) -> Result<Tz, UnknownZone> {
        let tz = jiff::tz::TimeZoneDatabase::bundled()
            .get(name)
            .map_err(|_| UnknownZone)?;
        // jiff matches names case-insensitively; chrono-tz did not, and had no `Factory` zone.
        if tz.iana_name() != Some(name) || name == "Factory" {
            return Err(UnknownZone);
        }
        let first = tz.to_offset(jiff::Timestamp::MIN).seconds();
        let mut rest = vec![];
        for t in tz.following(jiff::Timestamp::MIN) {
            let at = t.timestamp().as_second();
            if at >= TABLE_END {
                break;
            }
            rest.push((at, t.offset().seconds()));
        }
        Ok(Tz {
            name: name.into(),
            first,
            rest: rest.into(),
        })
    }
}

impl Tz {
    pub fn utc() -> Tz {
        "UTC".parse().expect("UTC is in the database")
    }

    /// The name as given (e.g. "Europe/Paris").
    pub fn name(&self) -> &str {
        &self.name
    }

    fn get(&self, i: usize) -> i32 {
        if i == 0 {
            self.first
        } else {
            self.rest[i - 1].1
        }
    }

    /// Offset in force at a UTC second.
    pub fn offset_at(&self, utc_secs: i64) -> i32 {
        let i = self.rest.partition_point(|&(at, _)| at <= utc_secs);
        self.get(i)
    }

    /// Wall-clock reading of an instant in this zone.
    pub fn to_local(&self, t: &UtcTime) -> NaiveDateTime {
        t.naive_at(self.offset_at(t.timestamp()))
    }

    /// Local span of timespan `i`: [begin, end) in wall-clock seconds, None = unbounded.
    fn local_span(&self, i: usize) -> (Option<i64>, Option<i64>) {
        let off = i64::from(self.get(i));
        let begin = (i > 0).then(|| self.rest[i - 1].0 + off);
        let end = (i < self.rest.len()).then(|| self.rest[i].0 + off);
        (begin, end)
    }

    fn contains(span: (Option<i64>, Option<i64>), x: i64) -> bool {
        span.0.is_none_or(|a| a <= x) && span.1.is_none_or(|b| x < b)
    }

    fn cmp_span(span: (Option<i64>, Option<i64>), x: i64) -> Ordering {
        match span {
            (Some(a), Some(b)) if a <= x && x < b => Ordering::Equal,
            (Some(a), Some(b)) if a <= x && b <= x => Ordering::Less,
            (Some(_), Some(_)) => Ordering::Greater,
            (Some(a), None) if a <= x => Ordering::Equal,
            (Some(_), None) => Ordering::Greater,
            (None, Some(b)) if b <= x => Ordering::Less,
            (None, Some(_)) => Ordering::Equal,
            (None, None) => Ordering::Equal,
        }
    }

    /// chrono-tz's `offset_from_local_datetime` (same search, same tie-breaking) on wall-clock seconds.
    pub fn from_local_seconds(&self, local: i64) -> LocalOffset {
        let len = self.rest.len() + 1;
        let (mut start, mut end) = (0usize, len);
        let index = loop {
            let mid = start + (end - start) / 2;
            if mid == end {
                break Err(start);
            }
            match Tz::cmp_span(self.local_span(mid), local) {
                Ordering::Less => start = mid + 1,
                Ordering::Greater => end = mid,
                Ordering::Equal => break Ok(mid),
            }
        };
        let has = |i: usize| Tz::contains(self.local_span(i), local);
        match index {
            Ok(0) if len == 1 => LocalOffset::Single(self.get(0)),
            Ok(0) if has(1) => LocalOffset::Ambiguous(self.get(0), self.get(1)),
            Ok(0) => LocalOffset::Single(self.get(0)),
            Ok(i) if has(i - 1) => LocalOffset::Ambiguous(self.get(i - 1), self.get(i)),
            Ok(i) if i == len - 1 => LocalOffset::Single(self.get(i)),
            Ok(i) if has(i + 1) => LocalOffset::Ambiguous(self.get(i), self.get(i + 1)),
            Ok(i) => LocalOffset::Single(self.get(i)),
            Err(_) => LocalOffset::None,
        }
    }

    pub fn from_local(&self, local: &NaiveDateTime) -> LocalOffset {
        self.from_local_seconds(local.local_seconds())
    }
}

impl PartialEq for Tz {
    fn eq(&self, other: &Tz) -> bool {
        self.name == other.name
    }
}

impl Eq for Tz {}

impl fmt::Debug for Tz {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "Tz({})", self.name)
    }
}

impl fmt::Display for Tz {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&self.name)
    }
}
