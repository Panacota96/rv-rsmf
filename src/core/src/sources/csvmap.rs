//! Generic CSV/TSV chat exports (XRY/XAMN, Cellebrite, spreadsheets) read through a column mapping.
//!
//! ```text
//! bytes ─► encoding (BOM, else [options] encoding / auto) ─► skip_rows ─► header ─► mapping → column indexes
//!       ─► rows: time (format, offset / timezone column / --timezone, DST check) ─► events per conversation value
//! ```
//!
//! The mapping is, in this order: the TOML file given with `--csv-map`; a file `<csv stem>.map.toml` next to the
//! CSV (so one run can convert several CSVs with different mappings); one of the built-in profiles below.
//! Format:
//!
//! ```toml
//! [columns]                      # a name or a list of alternative names; matched case-insensitively
//! timestamp = "Time"             # required
//! timestamp_format = "auto"      # chrono strftime, "epoch_s", "epoch_ms", "iso" or "auto"
//! timezone_column = "Zone"       # optional: IANA name or UTC offset per row
//! conversation = "Chat ID"       # required: one conversation per distinct value
//! sender = "From"                # required ("Name", "+57 300 000 0001" or "Name <+57 300 000 0001>")
//! sender_number = "From number"  # optional
//! recipients = "To"              # optional, several separated by ';'
//! body = "Body"                  # required
//! direction = "Direction"        # optional, values mapped by [direction]
//! application = "Source"         # optional → conversation custom field "application"
//! deleted = "Deleted"            # optional, true when the cell is one of [deleted] values
//! attachment = "Attachment(s)"   # optional, path relative to the CSV folder (several separated by ';')
//! [direction]
//! incoming = ["Incoming", "Received"]
//! outgoing = ["Outgoing", "Sent"]
//! [deleted]
//! values = ["yes", "true", "1", "deleted"]
//! [options]
//! delimiter = "auto"             # "auto", ",", ";", "\t" (or "tab"), "|"
//! encoding = "auto"              # "auto", "utf-8", "utf-16le", "windows-1252" (a BOM always wins)
//! skip_rows = 0                  # lines before the header
//! date_order = "dmy"             # for timestamp_format "auto" with numeric dates (--date-order wins)
//! ```
//!
//! A missing required column is never guessed: the parser returns `NeedsMapping` naming the missing columns and
//! the header it found. Times without an offset need `--timezone` (or a timezone column), else `NeedsTimezone`.

use super::{DateOrder, ExportInput, ParseError, ParseOptions};
use crate::analyzer::{Finding, Severity};
use crate::model::{
    Attachment, AttachmentData, Conversation, ConversationType, Direction, Event, ParsedExport,
    Participant, SourceLoc, stable_id,
};
use crate::report::capped_list;
use crate::rsmf::local_to_utc;
use chrono::{DateTime, FixedOffset, NaiveDate, NaiveDateTime, TimeZone, Utc};
use chrono_tz::Tz;
use regex::Regex;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Component, Path};
use std::str::FromStr;
use std::sync::OnceLock;

/// Built-in profile for CSV files that use rv-rsmf's own column names.
pub const PROFILE_GENERIC: &str = r#"
# rv-rsmf generic profile: the header uses rv-rsmf's own column names.
[columns]
timestamp = "timestamp"
timestamp_format = "auto"
timezone_column = "timezone"
conversation = "conversation"
sender = "sender"
sender_number = "sender_number"
recipients = "recipients"
body = "body"
direction = "direction"
application = "application"
deleted = "deleted"
attachment = "attachment"
[options]
delimiter = "auto"
encoding = "auto"
"#;

/// DRAFT — UNVERIFIED. MSAB XRY/XAMN column layouts are not publicly documented; these names are a best guess
/// from common XAMN "export to CSV" views and MUST be checked against a real export (then shipped as a
/// verified profile or given as --csv-map). Using it adds CSV_MAPPING_DRAFT_PROFILE.
pub const PROFILE_XRY_DRAFT: &str = r#"
# DRAFT, UNVERIFIED: XRY/XAMN chat export (best-guess column names).
[columns]
timestamp = ["Time", "Date", "Timestamp", "Date/Time"]
timestamp_format = "auto"
conversation = ["Chat ID", "Chat", "Conversation", "Thread"]
sender = ["From", "Sender"]
recipients = ["To", "Recipients", "Recipient"]
body = ["Body", "Text", "Message"]
direction = ["Direction"]
application = ["Source", "Application", "App"]
deleted = ["Deleted", "Deleted?"]
attachment = ["Attachment(s)", "Attachments", "Attachment"]
[options]
delimiter = "auto"
encoding = "auto"
"#;

/// DRAFT — UNVERIFIED. Cellebrite (Physical Analyzer / Reader) chat exports to CSV; column names are a best guess
/// and MUST be checked against a real export. Using it adds CSV_MAPPING_DRAFT_PROFILE.
pub const PROFILE_CELLEBRITE_DRAFT: &str = r#"
# DRAFT, UNVERIFIED: Cellebrite chat export (best-guess column names).
[columns]
timestamp = ["Timestamp: Time", "Timestamp", "Time", "Date"]
timestamp_format = "auto"
conversation = ["Chat ID", "Chat #", "Chat", "Identifier"]
sender = ["From", "Sender", "Participant"]
recipients = ["To", "Participants", "Recipients"]
body = ["Body", "Message", "Text"]
direction = ["Direction"]
application = ["Source", "Source App", "Application"]
deleted = ["Deleted", "Deleted - Instant Message"]
attachment = ["Attachment(s)", "Attachments", "Attachment #1"]
[options]
delimiter = "auto"
encoding = "auto"
"#;

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum Names {
    One(String),
    Many(Vec<String>),
}

impl Names {
    pub fn list(&self) -> Vec<&str> {
        match self {
            Names::One(s) => vec![s.as_str()],
            Names::Many(v) => v.iter().map(String::as_str).collect(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Columns {
    pub timestamp: Option<Names>,
    pub timestamp_format: Option<String>,
    pub timezone_column: Option<Names>,
    pub conversation: Option<Names>,
    pub sender: Option<Names>,
    pub sender_number: Option<Names>,
    pub recipients: Option<Names>,
    pub body: Option<Names>,
    pub direction: Option<Names>,
    pub application: Option<Names>,
    pub deleted: Option<Names>,
    pub attachment: Option<Names>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DirectionValues {
    #[serde(default)]
    pub incoming: Vec<String>,
    #[serde(default)]
    pub outgoing: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DeletedValues {
    #[serde(default)]
    pub values: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MapOptions {
    pub delimiter: Option<String>,
    pub encoding: Option<String>,
    pub skip_rows: Option<usize>,
    pub date_order: Option<String>,
}

/// A column mapping (TOML).
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Mapping {
    pub columns: Columns,
    #[serde(default)]
    pub direction: DirectionValues,
    #[serde(default)]
    pub deleted: DeletedValues,
    #[serde(default)]
    pub options: MapOptions,
}

impl Mapping {
    pub fn from_toml(text: &str) -> Result<Mapping, String> {
        toml::from_str(text).map_err(|e| e.to_string().trim().to_string())
    }
}

const DEFAULT_INCOMING: &[&str] = &["incoming", "in", "received", "inbox", "rx", "entrante"];
const DEFAULT_OUTGOING: &[&str] = &["outgoing", "out", "sent", "outbox", "tx", "saliente"];
const DEFAULT_DELETED: &[&str] = &[
    "1", "true", "yes", "y", "x", "deleted", "si", "sí", "oui", "vrai",
];

// ---------------------------------------------------------------- encoding

/// Decodes the file: a BOM always decides; otherwise the declared encoding, strictly (a byte sequence that is not
/// valid in it is a mapping problem, never silently replaced). "auto" = UTF-8, else UTF-16LE when it clearly is
/// (zero high bytes), else windows-1252. Returns (text, description).
pub fn decode(bytes: &[u8], declared: &str) -> Result<(String, String), String> {
    if let Some((enc, bom)) = encoding_rs::Encoding::for_bom(bytes) {
        let body = &bytes[bom..];
        let text = enc
            .decode_without_bom_handling_and_without_replacement(body)
            .ok_or_else(|| {
                format!(
                    "the file starts with a {} BOM but is not valid {}",
                    enc.name(),
                    enc.name()
                )
            })?;
        return Ok((text.into_owned(), format!("{} (BOM)", enc.name())));
    }
    let strict = |enc: &'static encoding_rs::Encoding| -> Result<(String, String), String> {
        enc.decode_without_bom_handling_and_without_replacement(bytes)
            .map(|t| (t.into_owned(), enc.name().to_string()))
            .ok_or_else(|| {
                format!(
                    "the file is not valid {}; set [options] encoding in the mapping (utf-8, utf-16le, windows-1252)",
                    enc.name()
                )
            })
    };
    match declared.trim().to_ascii_lowercase().as_str() {
        "" | "auto" => {
            // Text CSVs never contain NUL; many NULs in odd positions mean UTF-16LE without a BOM (checked first,
            // because ASCII-only UTF-16LE is also valid UTF-8).
            let odd_zero = bytes.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
            if bytes.len() >= 4
                && bytes.len().is_multiple_of(2)
                && odd_zero * 10 >= bytes.len() / 2 * 3
                && let Ok((t, _)) = strict(encoding_rs::UTF_16LE)
            {
                return Ok((t, "UTF-16LE (auto, no BOM)".into()));
            }
            if let Ok(t) = std::str::from_utf8(bytes) {
                return Ok((t.to_string(), "UTF-8 (auto)".into()));
            }
            let (t, _) = encoding_rs::WINDOWS_1252.decode_without_bom_handling(bytes);
            Ok((
                t.into_owned(),
                "windows-1252 (auto: not valid UTF-8)".into(),
            ))
        }
        "utf-8" | "utf8" => strict(encoding_rs::UTF_8),
        "utf-16le" | "utf-16" | "utf16le" => strict(encoding_rs::UTF_16LE),
        "utf-16be" => strict(encoding_rs::UTF_16BE),
        other => match encoding_rs::Encoding::for_label(other.as_bytes()) {
            Some(enc) => strict(enc),
            None => Err(format!("unknown encoding \"{other}\" in the mapping")),
        },
    }
}

fn delimiter_of(opt: Option<&str>, header_line: &str, ext_tsv: bool) -> Result<u8, String> {
    match opt.unwrap_or("auto") {
        "auto" | "" => {
            let best = b",;\t|"
                .iter()
                .copied()
                .map(|d| (header_line.bytes().filter(|b| *b == d).count(), d))
                .max_by_key(|(n, d)| (*n, std::cmp::Reverse(*d)))
                .filter(|(n, _)| *n > 0)
                .map(|(_, d)| d);
            Ok(best.unwrap_or(if ext_tsv { b'\t' } else { b',' }))
        }
        "\\t" | "tab" | "\t" => Ok(b'\t'),
        d if d.len() == 1 && d.is_ascii() => Ok(d.as_bytes()[0]),
        d => Err(format!(
            "delimiter \"{d}\" must be one ASCII character, \"tab\" or \"auto\""
        )),
    }
}

// ---------------------------------------------------------------- times

/// A UTC offset as written in exports: `Z`, `+02:00`, `-0500`, `UTC+2`, `(UTC-05:00)`, `GMT`.
pub fn parse_offset(s: &str) -> Option<FixedOffset> {
    let t = s
        .trim()
        .trim_start_matches('(')
        .trim_end_matches(')')
        .trim();
    let t = t
        .strip_prefix("UTC")
        .or_else(|| t.strip_prefix("GMT"))
        .unwrap_or(t)
        .trim();
    if t.is_empty() || t == "Z" {
        return FixedOffset::east_opt(0);
    }
    let (sign, rest) = match t.as_bytes()[0] {
        b'+' => (1, &t[1..]),
        b'-' => (-1, &t[1..]),
        _ => return None,
    };
    let (h, m) = match rest.split_once(':') {
        Some((h, m)) => (h, m),
        None if rest.len() > 2 => (&rest[..rest.len() - 2], &rest[rest.len() - 2..]),
        None => (rest, "0"),
    };
    let (h, m): (i32, i32) = (h.parse().ok()?, m.parse().ok()?);
    if !(0..=14).contains(&h) || !(0..60).contains(&m) {
        return None;
    }
    FixedOffset::east_opt(sign * (h * 3600 + m * 60))
}

#[derive(Clone, Debug, PartialEq)]
pub enum TsFormat {
    Auto,
    Iso,
    EpochS,
    EpochMs,
    Strftime { fmt: String, has_offset: bool },
}

impl TsFormat {
    pub fn parse(s: Option<&str>) -> TsFormat {
        match s.map(str::trim).unwrap_or("auto") {
            "" | "auto" => TsFormat::Auto,
            "iso" => TsFormat::Iso,
            "epoch_s" => TsFormat::EpochS,
            "epoch_ms" => TsFormat::EpochMs,
            f => TsFormat::Strftime {
                fmt: f.to_string(),
                has_offset: ["%z", "%:z", "%::z", "%#z"].iter().any(|z| f.contains(z)),
            },
        }
    }
}

/// A numeric date-time split into parts, before the day/month order is known.
#[derive(Clone, Debug, PartialEq)]
pub struct AutoTs {
    pub parts: [u32; 3],
    /// Year first (4 digits in first position).
    pub ymd: bool,
    pub hms: (u32, u32, u32),
    pub nanos: u32,
    pub offset: Option<FixedOffset>,
}

fn auto_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?ix)^
            (\d{1,4})[./-](\d{1,2})[./-](\d{1,4})        # date
            (?:\s*,\s*|\s+|T)
            (\d{1,2})[:.](\d{2})(?:[:.](\d{2})(?:[.,](\d{1,9}))?)?   # time
            \s*(?:([ap])\.?\s?m\.?)?                     # am / pm
            \s*(Z|[+-]\d{2}:?\d{2}|\(?\s*(?:UTC|GMT)\s*(?:[+-]\d{1,2}(?::?\d{2})?)?\s*\)?)?   # offset
            $",
        )
        .expect("valid regex")
    })
}

/// Splits a numeric date-time ("14/03/2026 10:02:11", "2026-03-14T10:02:11Z", "3/14/26, 1:55 p. m.",
/// "02/03/2026 09:15:00(UTC+0)"). Every Unicode space counts as a space; LRM/RLM marks are ignored.
pub fn split_auto(s: &str) -> Option<AutoTs> {
    let norm: String = s
        .chars()
        .filter(|c| !matches!(c, '\u{200E}' | '\u{200F}'))
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    let c = auto_re().captures(norm.trim())?;
    let g = |i: usize| c.get(i).map(|m| m.as_str());
    let num = |i: usize| g(i).and_then(|x| x.parse::<u32>().ok());
    let first = g(1)?;
    if first.len() == 3 {
        return None;
    }
    let mut h = num(4)?;
    if let Some(ap) = g(8) {
        if h == 0 || h > 12 {
            return None;
        }
        let pm = ap.eq_ignore_ascii_case("p");
        h = match (pm, h) {
            (false, 12) => 0,
            (true, 12) => 12,
            (true, h) => h + 12,
            (false, h) => h,
        };
    }
    let nanos = match g(7) {
        Some(f) => format!("{f:0<9}").parse().ok()?,
        None => 0,
    };
    let offset = match g(9) {
        Some(o) => Some(parse_offset(o)?),
        None => None,
    };
    Some(AutoTs {
        parts: [num(1)?, num(2)?, num(3)?],
        ymd: first.len() == 4,
        hms: (h, num(5)?, num(6).unwrap_or(0)),
        nanos,
        offset,
    })
}

impl AutoTs {
    pub fn naive(&self, order: DateOrder) -> Option<NaiveDateTime> {
        let [a, b, c] = self.parts;
        let (y, m, d) = if self.ymd {
            (a, b, c)
        } else {
            match order {
                DateOrder::Dmy => (c, b, a),
                DateOrder::Mdy => (c, a, b),
                DateOrder::Ymd => (a, b, c),
            }
        };
        let y = if y < 100 { 2000 + y } else { y };
        NaiveDate::from_ymd_opt(y as i32, m, d)?
            .and_hms_nano_opt(self.hms.0, self.hms.1, self.hms.2, self.nanos)
    }
}

/// A time cell after the first look: an instant, a wall-clock time still needing a zone, or a numeric date
/// still needing the day/month order.
#[derive(Clone, Debug, PartialEq)]
enum Cell {
    Instant(DateTime<Utc>),
    Local(NaiveDateTime),
    Auto(AutoTs),
}

fn first_look(s: &str, f: &TsFormat) -> Result<Cell, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("empty time".into());
    }
    let bad = || format!("time \"{s}\" does not match the format");
    match f {
        TsFormat::EpochS => {
            let (secs, frac) = s.split_once('.').unwrap_or((s, ""));
            let secs: i64 = secs.parse().map_err(|_| bad())?;
            let nanos: u32 = if frac.is_empty() {
                0
            } else if frac.len() <= 9 && frac.bytes().all(|b| b.is_ascii_digit()) {
                format!("{frac:0<9}").parse().map_err(|_| bad())?
            } else {
                return Err(bad());
            };
            DateTime::from_timestamp(secs, nanos)
                .map(Cell::Instant)
                .ok_or_else(bad)
        }
        TsFormat::EpochMs => s
            .parse::<i64>()
            .ok()
            .and_then(DateTime::from_timestamp_millis)
            .map(Cell::Instant)
            .ok_or_else(bad),
        TsFormat::Strftime { fmt, has_offset } => {
            if *has_offset {
                DateTime::parse_from_str(s, fmt)
                    .map(|t| Cell::Instant(t.with_timezone(&Utc)))
                    .map_err(|e| format!("{} ({e})", bad()))
            } else {
                NaiveDateTime::parse_from_str(s, fmt)
                    .or_else(|e| {
                        NaiveDate::parse_from_str(s, fmt)
                            .map(|d| d.and_hms_opt(0, 0, 0).expect("midnight"))
                            .map_err(|_| e)
                    })
                    .map(Cell::Local)
                    .map_err(|e| format!("{} ({e})", bad()))
            }
        }
        TsFormat::Iso | TsFormat::Auto => {
            if let Ok(t) = DateTime::parse_from_rfc3339(s) {
                return Ok(Cell::Instant(t.with_timezone(&Utc)));
            }
            let a = split_auto(s).ok_or_else(bad)?;
            if *f == TsFormat::Iso && !a.ymd {
                return Err(bad());
            }
            Ok(Cell::Auto(a))
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Zone {
    Iana(Tz),
    Fixed(FixedOffset),
}

impl Zone {
    fn parse(s: &str) -> Option<Zone> {
        let s = s.trim();
        Tz::from_str(s)
            .ok()
            .map(Zone::Iana)
            .or_else(|| parse_offset(s).map(Zone::Fixed))
    }
    fn name(&self) -> String {
        match self {
            Zone::Iana(t) => t.name().to_string(),
            Zone::Fixed(o) => format!("UTC{o}"),
        }
    }
}

// ---------------------------------------------------------------- participants

/// One sender/recipient cell: "Name", "+57 300 000 0001", "a@b.c", "Name <+57 300 000 0001>".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Party {
    pub name: Option<String>,
    pub number: Option<String>,
    pub email: Option<String>,
}

pub fn is_number(s: &str) -> bool {
    let s = s.trim();
    let digits = s.chars().filter(|c| c.is_ascii_digit()).count();
    digits >= 5
        && s.chars()
            .all(|c| c.is_ascii_digit() || " +-().".contains(c))
        && !s[1..].contains('+')
}

fn norm_number(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_digit() || *c == '+')
        .collect()
}

pub fn parse_party(s: &str) -> Option<Party> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    let mut p = Party::default();
    let (name, id) = match (s.rfind('<'), s.ends_with('>')) {
        (Some(i), true) => (Some(s[..i].trim()), Some(s[i + 1..s.len() - 1].trim())),
        _ => (None, Some(s)),
    };
    p.name = name.filter(|n| !n.is_empty()).map(str::to_string);
    if let Some(id) = id.filter(|i| !i.is_empty()) {
        if is_number(id) {
            p.number = Some(id.to_string());
        } else if id.contains('@') && !id.contains(' ') {
            p.email = Some(id.to_string());
        } else if p.name.is_none() {
            p.name = Some(id.to_string());
        } else {
            p.name = Some(s.to_string());
        }
    }
    Some(p)
}

impl Party {
    fn key(&self) -> String {
        if let Some(n) = &self.number {
            format!("tel:{}", norm_number(n))
        } else if let Some(e) = &self.email {
            format!("email:{}", e.to_lowercase())
        } else {
            format!("name:{}", self.name.clone().unwrap_or_default())
        }
    }
}

// ---------------------------------------------------------------- parsing

struct Cols {
    ts: usize,
    tz: Option<usize>,
    conv: usize,
    sender: usize,
    number: Option<usize>,
    recip: Option<usize>,
    body: usize,
    dir: Option<usize>,
    app: Option<usize>,
    del: Option<usize>,
    att: Option<usize>,
}

fn resolve_columns(
    m: &Mapping,
    header: &[String],
    strict_optional: bool,
) -> Result<(Cols, Vec<String>, Vec<String>), String> {
    let find = |n: &Option<Names>| -> Option<usize> {
        let names = n.as_ref()?.list();
        names.iter().find_map(|want| {
            header
                .iter()
                .position(|h| h.trim().eq_ignore_ascii_case(want.trim()))
        })
    };
    let describe = |key: &str, n: &Option<Names>| -> String {
        match n {
            Some(n) => format!(
                "{key} ({})",
                n.list()
                    .iter()
                    .map(|x| format!("\"{x}\""))
                    .collect::<Vec<_>>()
                    .join(" | ")
            ),
            None => format!("{key} (no column named in the mapping)"),
        }
    };
    let c = &m.columns;
    let mut missing = vec![];
    let mut not_found = vec![];
    let mut used = vec![];
    let mut req = |key: &str, n: &Option<Names>| -> usize {
        match find(n) {
            Some(i) => {
                used.push(format!("{key}={}", header[i].trim()));
                i
            }
            None => {
                missing.push(describe(key, n));
                0
            }
        }
    };
    let ts = req("timestamp", &c.timestamp);
    let conv = req("conversation", &c.conversation);
    let sender = req("sender", &c.sender);
    let body = req("body", &c.body);
    let mut opt = |key: &str, n: &Option<Names>| -> Option<usize> {
        let i = find(n);
        match (i, n) {
            (Some(i), _) => used.push(format!("{key}={}", header[i].trim())),
            (None, Some(_)) if strict_optional => missing.push(describe(key, n)),
            (None, Some(_)) => not_found.push(describe(key, n)),
            (None, None) => {}
        }
        i
    };
    let cols = Cols {
        ts,
        conv,
        sender,
        body,
        tz: opt("timezone_column", &c.timezone_column),
        number: opt("sender_number", &c.sender_number),
        recip: opt("recipients", &c.recipients),
        dir: opt("direction", &c.direction),
        app: opt("application", &c.application),
        del: opt("deleted", &c.deleted),
        att: opt("attachment", &c.attachment),
    };
    if !missing.is_empty() {
        return Err(format!(
            "missing columns: {}; header has: {}",
            missing.join(", "),
            header
                .iter()
                .map(|h| h.trim())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    Ok((cols, used, not_found))
}

struct Row {
    row: u64,
    line: u64,
    line_end: u64,
    fields: Vec<String>,
    raw: String,
}

impl Row {
    fn get(&self, i: Option<usize>) -> &str {
        i.and_then(|i| self.fields.get(i))
            .map(|s| s.as_str())
            .unwrap_or("")
    }
    fn example(&self, why: &str) -> String {
        let mut t: String = self.raw.chars().take(60).collect();
        if self.raw.chars().count() > 60 {
            t.push('\u{2026}');
        }
        format!("row {} (line {}) [{why}]: {t}", self.row, self.line)
    }
}

/// Where the mapping came from: `--csv-map`, a sibling `<csv stem>.map.toml`, or a built-in profile.
#[derive(Clone, Debug, PartialEq)]
pub enum MappingSource {
    Option(std::path::PathBuf),
    Sibling(std::path::PathBuf),
    Profile(super::CsvProfile),
}

impl MappingSource {
    /// Fact "mapping": the file name, or "built-in".
    pub fn name(&self) -> String {
        match self {
            MappingSource::Option(p) | MappingSource::Sibling(p) => p
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| p.display().to_string()),
            MappingSource::Profile(_) => "built-in".into(),
        }
    }
    /// Fact "mapping_source": "option", "sibling" or "profile:<name>".
    pub fn kind(&self) -> String {
        match self {
            MappingSource::Option(_) => "option".into(),
            MappingSource::Sibling(_) => "sibling".into(),
            MappingSource::Profile(p) => format!("profile:{}", p.name()),
        }
    }
}

/// `<folder>/<csv stem>.map.toml` next to the CSV.
pub fn sibling_mapping(csv: &Path) -> std::path::PathBuf {
    let stem = csv
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    csv.with_file_name(format!("{stem}.map.toml"))
}

/// Loads the mapping: `--csv-map`, else a sibling `<csv stem>.map.toml`, else the built-in profile.
fn load_mapping(
    export: &ExportInput,
    opts: &ParseOptions,
) -> Result<(Mapping, MappingSource), ParseError> {
    let sibling = sibling_mapping(&export.primary);
    let src = match &opts.csv_map {
        Some(p) => MappingSource::Option(p.clone()),
        None if sibling.is_file() => MappingSource::Sibling(sibling),
        None => MappingSource::Profile(opts.csv_profile),
    };
    match &src {
        MappingSource::Option(path) | MappingSource::Sibling(path) => {
            let bytes = export.read(path).map_err(|e| {
                ParseError::NeedsMapping(format!("cannot read mapping {}: {e}", path.display()))
            })?;
            let text = String::from_utf8(bytes).map_err(|_| {
                ParseError::NeedsMapping(format!("mapping {} is not UTF-8", path.display()))
            })?;
            let m = Mapping::from_toml(&text).map_err(|e| {
                ParseError::NeedsMapping(format!("mapping {} is not valid: {e}", path.display()))
            })?;
            Ok((m, src))
        }
        MappingSource::Profile(p) => {
            let text = match p {
                super::CsvProfile::Generic => PROFILE_GENERIC,
                super::CsvProfile::XryDraft => PROFILE_XRY_DRAFT,
                super::CsvProfile::CellebriteDraft => PROFILE_CELLEBRITE_DRAFT,
            };
            let m = Mapping::from_toml(text).expect("built-in profiles are valid TOML");
            Ok((m, src))
        }
    }
}

/// Parses with the mapping chosen by [`load_mapping`]. When nothing was chosen (no --csv-map, no sibling
/// mapping, default generic profile) and the generic columns are not in the header, the built-in tool
/// profiles are tried in turn; a header that matches one of them is read with it (fact
/// "profile_detected" = "yes") and still reported as a draft profile (CSV_MAPPING_DRAFT_PROFILE).
pub fn parse(export: &ExportInput, opts: &ParseOptions) -> Result<ParsedExport, ParseError> {
    let first = parse_with(export, opts);
    let auto = opts.csv_map.is_none()
        && opts.csv_profile == super::CsvProfile::Generic
        && !sibling_mapping(&export.primary).is_file();
    match first {
        Err(ParseError::NeedsMapping(m)) if auto => {
            for p in [
                super::CsvProfile::XryDraft,
                super::CsvProfile::CellebriteDraft,
            ] {
                let o = ParseOptions {
                    csv_profile: p,
                    ..opts.clone()
                };
                match parse_with(export, &o) {
                    Ok(mut x) => {
                        x.facts.insert("profile_detected".into(), "yes".into());
                        return Ok(x);
                    }
                    Err(ParseError::NeedsMapping(_)) => continue,
                    Err(e) => return Err(e),
                }
            }
            Err(ParseError::NeedsMapping(m))
        }
        r => r,
    }
}

fn parse_with(export: &ExportInput, opts: &ParseOptions) -> Result<ParsedExport, ParseError> {
    let (map, map_src) = load_mapping(export, opts)?;
    let user_map = !matches!(map_src, MappingSource::Profile(_));
    let bytes = export
        .read(&export.primary)
        .map_err(|e| ParseError::Invalid(format!("{}: {e}", export.primary.display())))?;
    let (text, enc_desc) = decode(&bytes, map.options.encoding.as_deref().unwrap_or("auto"))
        .map_err(ParseError::NeedsMapping)?;
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(&text);
    let skip = map.options.skip_rows.unwrap_or(0);
    let mut body = text;
    for _ in 0..skip {
        body = match body.find('\n') {
            Some(i) => &body[i + 1..],
            None => {
                return Err(ParseError::NeedsMapping(format!(
                    "skip_rows = {skip} skips the whole file"
                )));
            }
        };
    }
    let header_line = body.lines().next().unwrap_or("");
    if header_line.trim().is_empty() {
        return Err(ParseError::Invalid("the CSV has no header line".into()));
    }
    let ext_tsv = export
        .primary
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("tsv"));
    let delim = delimiter_of(map.options.delimiter.as_deref(), header_line, ext_tsv)
        .map_err(ParseError::NeedsMapping)?;
    let mut rdr = csv::ReaderBuilder::new()
        .delimiter(delim)
        .flexible(true)
        .has_headers(true)
        .from_reader(body.as_bytes());
    let header: Vec<String> = rdr
        .headers()
        .map_err(|e| ParseError::Invalid(format!("CSV header: {e}")))?
        .iter()
        .map(str::to_string)
        .collect();
    let (cols, used, not_found) =
        resolve_columns(&map, &header, user_map).map_err(ParseError::NeedsMapping)?;
    let file = export
        .primary
        .strip_prefix(&export.root)
        .unwrap_or(&export.primary)
        .to_string_lossy()
        .replace('\\', "/");
    let delim_char = delim as char;

    // Pass 1: records.
    let mut rows = vec![];
    let mut unparsed: Vec<String> = vec![];
    let mut blank = 0usize;
    let need = [cols.ts, cols.conv, cols.sender, cols.body]
        .into_iter()
        .max()
        .unwrap_or(0);
    for (n, rec) in rdr.records().enumerate() {
        let row_no = n as u64 + 1;
        match rec {
            Ok(r) => {
                let line = r.position().map(|p| p.line()).unwrap_or(0) + skip as u64;
                let fields: Vec<String> = r.iter().map(str::to_string).collect();
                let newlines: u64 = fields.iter().map(|f| f.matches('\n').count() as u64).sum();
                let row = Row {
                    row: row_no,
                    line,
                    line_end: line + newlines,
                    raw: fields.join(&delim_char.to_string()),
                    fields,
                };
                if row.fields.iter().all(|f| f.trim().is_empty()) {
                    blank += 1;
                } else if row.fields.len() <= need {
                    unparsed.push(row.example("missing fields"));
                } else {
                    rows.push(row);
                }
            }
            Err(e) => {
                let line = e.position().map(|p| p.line()).unwrap_or(0) + skip as u64;
                unparsed.push(format!("row {row_no} (line {line}) [unreadable]: {e}"));
            }
        }
    }

    // Pass 2: first look at every time cell.
    let fmt = TsFormat::parse(map.columns.timestamp_format.as_deref());
    let mut cells: Vec<Option<Cell>> = Vec::with_capacity(rows.len());
    for r in &rows {
        match first_look(r.get(Some(cols.ts)), &fmt) {
            Ok(c) => cells.push(Some(c)),
            Err(why) => {
                unparsed.push(r.example(&why));
                cells.push(None);
            }
        }
    }

    // Day/month order for numeric dates that are not year-first.
    let ambiguous: Vec<(usize, &AutoTs)> = cells
        .iter()
        .enumerate()
        .filter_map(|(i, c)| match c {
            Some(Cell::Auto(a)) if !a.ymd => Some((i, a)),
            _ => None,
        })
        .collect();
    let mut order_fact = None;
    let order = if ambiguous.is_empty() {
        None
    } else if let Some(o) = opts.date_order {
        order_fact = Some(format!("{} (examiner, --date-order)", o.name()));
        Some(o)
    } else if let Some(o) = map.options.date_order.as_deref() {
        let o = DateOrder::parse(o).ok_or_else(|| {
            ParseError::NeedsMapping(format!(
                "[options] date_order \"{o}\" must be dmy, mdy or ymd"
            ))
        })?;
        order_fact = Some(format!("{} (mapping)", o.name()));
        Some(o)
    } else {
        let first_gt = ambiguous.iter().find(|(_, a)| a.parts[0] > 12);
        let second_gt = ambiguous.iter().find(|(_, a)| a.parts[1] > 12);
        let raw = |i: usize| format!("row {}: {}", rows[i].row, rows[i].get(Some(cols.ts)));
        match (first_gt, second_gt) {
            (Some((i, _)), None) => {
                order_fact = Some(format!("dmy (inferred: first number > 12 in {})", raw(*i)));
                Some(DateOrder::Dmy)
            }
            (None, Some((i, _))) => {
                order_fact = Some(format!("mdy (inferred: second number > 12 in {})", raw(*i)));
                Some(DateOrder::Mdy)
            }
            (Some((i, _)), Some((j, _))) => {
                return Err(ParseError::NeedsDateOrder {
                    candidates: vec![DateOrder::Dmy, DateOrder::Mdy],
                    evidence: vec![raw(*i), raw(*j)],
                });
            }
            (None, None) => {
                return Err(ParseError::NeedsDateOrder {
                    candidates: vec![DateOrder::Dmy, DateOrder::Mdy],
                    evidence: ambiguous.iter().take(3).map(|(i, _)| raw(*i)).collect(),
                });
            }
        }
    };

    // Pass 3: instants. Wall-clock times take the row's zone column, else --timezone.
    struct Timed {
        utc: DateTime<Utc>,
        zone: Option<String>,
    }
    let mut timed: Vec<Option<Timed>> = Vec::with_capacity(rows.len());
    let mut need_tz = 0usize;
    let mut dst: Vec<String> = vec![];
    let mut dst_zones: BTreeSet<String> = BTreeSet::new();
    let mut zones_used: BTreeSet<String> = BTreeSet::new();
    for (r, c) in rows.iter().zip(&cells) {
        let local = match c {
            None => {
                timed.push(None);
                continue;
            }
            Some(Cell::Instant(t)) => {
                timed.push(Some(Timed {
                    utc: *t,
                    zone: None,
                }));
                continue;
            }
            Some(Cell::Local(n)) => *n,
            Some(Cell::Auto(a)) => {
                let Some(n) = a.naive(order.unwrap_or(DateOrder::Ymd)) else {
                    unparsed.push(r.example("invalid date"));
                    timed.push(None);
                    continue;
                };
                if let Some(off) = a.offset {
                    let utc = off
                        .from_local_datetime(&n)
                        .single()
                        .map(|t| t.with_timezone(&Utc));
                    timed.push(utc.map(|utc| Timed { utc, zone: None }));
                    continue;
                }
                n
            }
        };
        let cell_zone = r.get(cols.tz).trim();
        let zone = if cell_zone.is_empty() {
            opts.timezone.map(Zone::Iana)
        } else {
            match Zone::parse(cell_zone) {
                Some(z) => Some(z),
                None => {
                    unparsed.push(r.example(&format!("unknown time zone \"{cell_zone}\"")));
                    timed.push(None);
                    continue;
                }
            }
        };
        let Some(zone) = zone else {
            need_tz += 1;
            timed.push(None);
            continue;
        };
        zones_used.insert(zone.name());
        let utc = match zone {
            Zone::Iana(tz) => {
                let (utc, flag) = local_to_utc(local, tz);
                if let Some(flag) = flag {
                    dst.push(format!(
                        "row {}: {} ({flag})",
                        r.row,
                        local.format("%Y-%m-%d %H:%M:%S")
                    ));
                    dst_zones.insert(tz.name().to_string());
                }
                utc
            }
            Zone::Fixed(off) => off
                .from_local_datetime(&local)
                .single()
                .expect("fixed offsets are never ambiguous")
                .with_timezone(&Utc),
        };
        let from_column = !cell_zone.is_empty();
        timed.push(Some(Timed {
            utc,
            zone: from_column.then(|| zone.name()),
        }));
    }
    if need_tz > 0 {
        return Err(ParseError::NeedsTimezone);
    }

    // Participants: a name seen with a number or address anywhere is linked to it where it appears alone.
    let party_of = |r: &Row| -> Option<Party> {
        let mut p = parse_party(r.get(Some(cols.sender))).unwrap_or_default();
        let num = r.get(cols.number).trim();
        if !num.is_empty() {
            p.number = Some(num.to_string());
        }
        if p == Party::default() {
            let outgoing = matches!(
                direction_of(&map, r.get(cols.dir)),
                Some(Direction::Outgoing)
            );
            return match (&opts.custodian, outgoing) {
                (Some(c), true) => parse_party(c),
                _ => None,
            };
        }
        Some(p)
    };
    let recipients_of = |r: &Row| -> Vec<Party> {
        r.get(cols.recip)
            .split(';')
            .filter_map(parse_party)
            .collect()
    };
    let mut name_keys: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut key_name: HashMap<String, String> = HashMap::new();
    for (r, t) in rows.iter().zip(&timed) {
        if t.is_none() {
            continue;
        }
        for p in party_of(r).into_iter().chain(recipients_of(r)) {
            if let Some(n) = &p.name
                && (p.number.is_some() || p.email.is_some())
            {
                name_keys.entry(n.clone()).or_default().insert(p.key());
                key_name.entry(p.key()).or_insert_with(|| n.clone());
            }
        }
    }
    let mut out = ParsedExport::default();
    let mut pindex: HashMap<String, usize> = HashMap::new();
    let mut person = |out: &mut ParsedExport, p: &Party| -> String {
        let mut key = p.key();
        if p.number.is_none()
            && p.email.is_none()
            && let Some(k) = p
                .name
                .as_ref()
                .and_then(|n| name_keys.get(n))
                .filter(|s| s.len() == 1)
        {
            key = k.iter().next().cloned().unwrap_or(key);
        }
        if let Some(&i) = pindex.get(&key) {
            let pp = &mut out.participants[i];
            if pp.account_id.is_none() {
                pp.account_id = p.number.clone();
            }
            if pp.email.is_none() {
                pp.email = p.email.clone();
            }
            return pp.id.clone();
        }
        let display = key_name
            .get(&key)
            .cloned()
            .or_else(|| p.name.clone())
            .or_else(|| p.number.clone())
            .or_else(|| p.email.clone())
            .unwrap_or_default();
        let id = stable_id("p", &key);
        pindex.insert(key, out.participants.len());
        out.participants.push(Participant {
            id: id.clone(),
            display,
            email: p.email.clone(),
            account_id: p.number.clone(),
            custom: BTreeMap::new(),
        });
        id
    };

    // Pass 4: events.
    let mut conv_index: HashMap<String, usize> = HashMap::new();
    let mut apps: Vec<BTreeSet<String>> = vec![];
    let mut missing_att: Vec<String> = vec![];
    let file_stem = export
        .primary
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "CSV".into());
    for (r, t) in rows.iter().zip(timed) {
        let Some(t) = t else { continue };
        let conv_value = r.get(Some(cols.conv)).trim().to_string();
        let ci = *conv_index.entry(conv_value.clone()).or_insert_with(|| {
            out.conversations.push(Conversation {
                id: stable_id("c", &format!("csv:{conv_value}")),
                display: if conv_value.is_empty() {
                    file_stem.clone()
                } else {
                    conv_value.clone()
                },
                kind: ConversationType::Direct,
                participants: vec![],
                custom: BTreeMap::new(),
            });
            apps.push(BTreeSet::new());
            out.conversations.len() - 1
        });
        let app = r.get(cols.app).trim();
        if !app.is_empty() {
            apps[ci].insert(app.to_string());
        }
        let sender = party_of(r).map(|p| person(&mut out, &p));
        let mut members: Vec<String> = sender.iter().cloned().collect();
        for p in recipients_of(r) {
            members.push(person(&mut out, &p));
        }
        for id in members {
            if !out.conversations[ci].participants.contains(&id) {
                out.conversations[ci].participants.push(id);
            }
        }
        let mut e = Event::message(
            format!("e-{:06}", out.events.len() + 1),
            out.conversations[ci].id.clone(),
            SourceLoc {
                file: file.clone(),
                line: r.line,
                line_end: r.line_end,
                native_id: Some(r.row.to_string()),
            },
        );
        e.participant = sender;
        e.timestamp = Some(t.utc);
        e.source_time = r.get(Some(cols.ts)).trim().to_string();
        if let Some(z) = t.zone {
            e.custom.insert("source_timezone_column".into(), z);
        }
        e.body = r.get(Some(cols.body)).replace("\r\n", "\n");
        let dir_raw = r.get(cols.dir).trim();
        e.direction = direction_of(&map, dir_raw);
        if e.direction.is_none() && !dir_raw.is_empty() {
            e.custom.insert("direction_raw".into(), dir_raw.to_string());
        }
        let del = r.get(cols.del).trim();
        e.deleted = !del.is_empty() && is_truthy(&map, del);
        for a in r
            .get(cols.att)
            .split([';', '\n'])
            .map(str::trim)
            .filter(|a| !a.is_empty())
        {
            let norm = a.replace('\\', "/");
            let rel = Path::new(&norm);
            let display = rel
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| norm.clone());
            let outside = rel.is_absolute()
                || norm.as_bytes().get(1) == Some(&b':')
                || rel
                    .components()
                    .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir));
            let full = export.root.join(rel);
            let data = if outside {
                AttachmentData::Missing {
                    reason: format!("path outside the export folder: {a}"),
                }
            } else if full.is_file() {
                AttachmentData::File { path: full }
            } else {
                AttachmentData::Missing {
                    reason: format!("not found next to the CSV: {a}"),
                }
            };
            if matches!(data, AttachmentData::Missing { .. }) {
                missing_att.push(format!("row {}: {a}", r.row));
            }
            e.attachments.push(Attachment { display, data });
        }
        out.events.push(e);
    }
    for (c, a) in out.conversations.iter_mut().zip(apps) {
        if c.participants.len() > 2 {
            c.kind = ConversationType::Channel;
        }
        if !a.is_empty() {
            c.custom.insert(
                "application".into(),
                a.into_iter().collect::<Vec<_>>().join("; "),
            );
        }
    }
    if out.events.is_empty() {
        return Err(ParseError::Invalid(if unparsed.is_empty() {
            "the CSV has no data rows".into()
        } else {
            format!(
                "no row of the CSV could be read: {}",
                capped_list(&unparsed, 3)
            )
        }));
    }

    // Findings.
    if let MappingSource::Profile(p) = &map_src
        && p.is_draft()
    {
        out.findings.push(
            Finding::new("CSV_MAPPING_DRAFT_PROFILE", Severity::Medium).with("profile", p.name()),
        );
    }
    if !dst.is_empty() {
        out.findings.push(
            Finding::new("DST_AMBIGUOUS_OR_MISSING_TIME", Severity::Medium)
                .with("count", dst.len().to_string())
                .with(
                    "timezone",
                    dst_zones.into_iter().collect::<Vec<_>>().join(", "),
                )
                .with("examples", capped_list(&dst, 3)),
        );
    }
    if !unparsed.is_empty() {
        out.findings.push(
            Finding::new("UNPARSED_LINES", Severity::Medium)
                .with("count", unparsed.len().to_string())
                .with("examples", capped_list(&unparsed, 3)),
        );
    }
    if !missing_att.is_empty() {
        out.findings.push(
            Finding::new("ATTACHMENT_REFERENCED_NOT_FOUND", Severity::Medium)
                .with("count", missing_att.len().to_string())
                .with("examples", capped_list(&missing_att, 3)),
        );
    }
    let numbers_only: Vec<String> = out
        .participants
        .iter()
        .filter(|p| p.account_id.as_deref() == Some(p.display.as_str()))
        .map(|p| p.display.clone())
        .collect();
    if !numbers_only.is_empty() {
        out.findings.push(
            Finding::new("PARTICIPANT_IDENTIFIED_BY_NUMBER_ONLY", Severity::Info)
                .with("count", numbers_only.len().to_string())
                .with("examples", capped_list(&numbers_only, 5)),
        );
    }

    let f = &mut out.facts;
    f.insert("mapping".into(), map_src.name());
    f.insert("mapping_source".into(), map_src.kind());
    f.insert("csv.encoding".into(), enc_desc);
    f.insert(
        "csv.delimiter".into(),
        if delim == b'\t' {
            "tab".into()
        } else {
            delim_char.to_string()
        },
    );
    f.insert("csv.header_line".into(), (skip + 1).to_string());
    f.insert("csv.columns".into(), used.join(", "));
    if !not_found.is_empty() {
        f.insert(
            "csv.optional_columns_not_found".into(),
            not_found.join(", "),
        );
    }
    f.insert(
        "csv.timestamp_format".into(),
        map.columns
            .timestamp_format
            .clone()
            .unwrap_or_else(|| "auto".into()),
    );
    if let Some(o) = order_fact {
        f.insert("csv.date_order".into(), o);
    }
    if !zones_used.is_empty() {
        f.insert(
            "csv.timezones_applied".into(),
            zones_used.into_iter().collect::<Vec<_>>().join(", "),
        );
    }
    f.insert("csv.rows".into(), (rows.len() + unparsed.len()).to_string());
    f.insert("csv.rows_blank".into(), blank.to_string());
    f.insert("csv.rows_unparsed".into(), unparsed.len().to_string());
    Ok(out)
}

fn direction_of(map: &Mapping, v: &str) -> Option<Direction> {
    let v = v.trim();
    if v.is_empty() {
        return None;
    }
    let hit = |user: &[String], default: &[&str]| -> bool {
        if user.is_empty() {
            default.iter().any(|d| d.eq_ignore_ascii_case(v))
        } else {
            user.iter().any(|d| d.trim().eq_ignore_ascii_case(v))
        }
    };
    if hit(&map.direction.incoming, DEFAULT_INCOMING) {
        Some(Direction::Incoming)
    } else if hit(&map.direction.outgoing, DEFAULT_OUTGOING) {
        Some(Direction::Outgoing)
    } else {
        None
    }
}

fn is_truthy(map: &Mapping, v: &str) -> bool {
    if map.deleted.values.is_empty() {
        DEFAULT_DELETED.iter().any(|d| d.eq_ignore_ascii_case(v))
    } else {
        map.deleted
            .values
            .iter()
            .any(|d| d.trim().eq_ignore_ascii_case(v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SourceKind;
    use crate::sources::CsvProfile;
    use chrono::Timelike;

    fn export_of(dir: &Path, name: &str, bytes: &[u8]) -> ExportInput {
        let p = dir.join(name);
        std::fs::write(&p, bytes).unwrap();
        ExportInput {
            kind: SourceKind::Csv,
            primary: p.clone(),
            root: dir.to_path_buf(),
            members: vec![p],
        }
    }

    fn madrid() -> ParseOptions {
        ParseOptions {
            timezone: Some(chrono_tz::Europe::Madrid),
            ..Default::default()
        }
    }

    #[test]
    fn built_in_profiles_parse() {
        for t in [PROFILE_GENERIC, PROFILE_XRY_DRAFT, PROFILE_CELLEBRITE_DRAFT] {
            let m = Mapping::from_toml(t).unwrap();
            assert!(m.columns.timestamp.is_some() && m.columns.body.is_some());
        }
        assert!(Mapping::from_toml("[columns]\ntimestmp = \"x\"\n").is_err());
    }

    #[test]
    fn encodings_are_detected_strictly() {
        let utf16: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("año".encode_utf16().flat_map(|u| u.to_le_bytes()))
            .collect();
        assert_eq!(decode(&utf16, "utf-8").unwrap().0, "año");
        assert_eq!(decode(&utf16, "auto").unwrap().1, "UTF-16LE (BOM)");
        assert_eq!(decode(b"\xEF\xBB\xBFa\xC3\xB1o", "auto").unwrap().0, "año");
        let latin = b"a\xF1o";
        assert!(decode(latin, "utf-8").is_err());
        assert_eq!(decode(latin, "windows-1252").unwrap().0, "año");
        let (t, d) = decode(latin, "auto").unwrap();
        assert_eq!(t, "año");
        assert!(d.starts_with("windows-1252"));
        let no_bom: Vec<u8> = "time,body"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        assert_eq!(decode(&no_bom, "auto").unwrap().0, "time,body");
        assert!(decode(b"x", "klingon").is_err());
    }

    #[test]
    fn offsets_and_auto_times() {
        assert_eq!(parse_offset("Z").unwrap().local_minus_utc(), 0);
        assert_eq!(parse_offset("+02:00").unwrap().local_minus_utc(), 7200);
        assert_eq!(parse_offset("-0530").unwrap().local_minus_utc(), -19800);
        assert_eq!(parse_offset("(UTC-5)").unwrap().local_minus_utc(), -18000);
        assert_eq!(parse_offset("UTC").unwrap().local_minus_utc(), 0);
        assert!(parse_offset("Paris").is_none());

        let a = split_auto("14/03/2026 10:02:11").unwrap();
        assert!(!a.ymd && a.offset.is_none());
        assert_eq!(
            a.naive(DateOrder::Dmy).unwrap().to_string(),
            "2026-03-14 10:02:11"
        );
        assert!(a.naive(DateOrder::Mdy).is_none());
        let b = split_auto("3/6/18, 1:55 p.\u{202F}m.").unwrap();
        assert_eq!(
            b.naive(DateOrder::Mdy).unwrap().to_string(),
            "2018-03-06 13:55:00"
        );
        let c = split_auto("02/03/2026 09:15:00(UTC+0)").unwrap();
        assert_eq!(c.offset.unwrap().local_minus_utc(), 0);
        let d = split_auto("2026-03-14T10:02:11.5+01:00").unwrap();
        assert!(d.ymd && d.nanos == 500_000_000);
        assert!(split_auto("12/03/2026").is_none());
        assert!(split_auto("13:00 12/03/2026").is_none());
    }

    #[test]
    fn explicit_format_with_timezone_and_dst() {
        let d = tempfile::tempdir().unwrap();
        let map = d.path().join("m.toml");
        std::fs::write(
            &map,
            "[columns]\ntimestamp=\"t\"\ntimestamp_format=\"%d/%m/%Y %H:%M\"\nconversation=\"c\"\nsender=\"s\"\nbody=\"b\"\n",
        )
        .unwrap();
        // 05/03 is unambiguous with an explicit format; 25/10/2026 02:30 is repeated in Madrid.
        let e = export_of(
            d.path(),
            "x.csv",
            b"t,c,s,b\n05/03/2026 10:00,g,Ana,hola\n25/10/2026 02:30,g,Ana,otra\n",
        );
        let mut o = madrid();
        o.csv_map = Some(map.clone());
        let p = parse(&e, &o).unwrap();
        assert_eq!(p.events.len(), 2);
        assert_eq!(p.events[0].timestamp.unwrap().hour(), 9); // CET = UTC+1
        assert_eq!(
            p.events[1].timestamp.unwrap().to_rfc3339(),
            "2026-10-25T00:30:00+00:00"
        );
        let f = p
            .findings
            .iter()
            .find(|f| f.code == "DST_AMBIGUOUS_OR_MISSING_TIME")
            .unwrap();
        assert_eq!(f.params["count"], "1");
        assert_eq!(f.params["timezone"], "Europe/Madrid");
        assert!(f.params["examples"].contains("row 2"));
        assert_eq!(p.events[1].source.line, 3);
        assert_eq!(p.events[1].source.native_id.as_deref(), Some("2"));
        // Without --timezone: never silently guessed.
        o.timezone = None;
        assert_eq!(parse(&e, &o), Err(ParseError::NeedsTimezone));
    }

    #[test]
    fn missing_columns_need_a_mapping() {
        let d = tempfile::tempdir().unwrap();
        let e = export_of(d.path(), "x.csv", b"when;who;what\n1;2;3\n");
        let err = parse(&e, &madrid()).unwrap_err();
        match err {
            ParseError::NeedsMapping(m) => {
                assert!(
                    m.contains("missing columns: timestamp (\"timestamp\")"),
                    "{m}"
                );
                assert!(m.contains("header has: when, who, what"), "{m}");
            }
            other => panic!("{other:?}"),
        }
        // A user mapping that names an optional column the file lacks is also refused.
        let map = d.path().join("m.toml");
        std::fs::write(
            &map,
            "[columns]\ntimestamp=\"when\"\nconversation=\"who\"\nsender=\"who\"\nbody=\"what\"\nattachment=\"file\"\n",
        )
        .unwrap();
        let o = ParseOptions {
            csv_map: Some(map.clone()),
            ..madrid()
        };
        assert!(
            matches!(parse(&e, &o), Err(ParseError::NeedsMapping(m)) if m.contains("attachment (\"file\")"))
        );
        std::fs::write(&map, "[columns\n").unwrap();
        assert!(matches!(parse(&e, &o), Err(ParseError::NeedsMapping(_))));
    }

    #[test]
    fn date_order_is_inferred_or_required() {
        let d = tempfile::tempdir().unwrap();
        let e = export_of(
            d.path(),
            "x.csv",
            b"timestamp,conversation,sender,body\n01/02/2026 10:00,g,Ana,a\n03/04/2026 10:00,g,Ana,b\n",
        );
        assert!(matches!(
            parse(&e, &madrid()),
            Err(ParseError::NeedsDateOrder { .. })
        ));
        let o = ParseOptions {
            date_order: Some(DateOrder::Mdy),
            ..madrid()
        };
        let p = parse(&e, &o).unwrap();
        assert_eq!(
            p.events[0].timestamp.unwrap().to_rfc3339(),
            "2026-01-02T09:00:00+00:00"
        );
        assert!(p.facts["csv.date_order"].contains("--date-order"));
        let e2 = export_of(
            d.path(),
            "y.csv",
            b"timestamp,conversation,sender,body\n01/02/2026 10:00,g,Ana,a\n23/04/2026 10:00,g,Ana,b\n",
        );
        let p = parse(&e2, &madrid()).unwrap();
        assert!(p.facts["csv.date_order"].starts_with("dmy (inferred"));
        assert_eq!(
            p.events[0].timestamp.unwrap().to_rfc3339(),
            "2026-02-01T09:00:00+00:00"
        );
    }

    #[test]
    fn offsets_epochs_zone_column_and_bad_rows() {
        let d = tempfile::tempdir().unwrap();
        let e = export_of(
            d.path(),
            "x.csv",
            "timestamp,timezone,conversation,sender,body,direction,deleted,attachment\n\
             2026-03-14T10:00:00Z,,g,+57 300 000 0001,a,Incoming,,\n\
             2026-03-14 10:00:00,America/Bogota,g,Ana <+57 300 000 0002>,\"b\nc\",Sent,yes,media/x.jpg\n\
             2026-03-14 10:00:00,Mars/Base,g,Ana,c,,,\n\
             not a date,,g,Ana,d,,,\n\
             2026-03-14 11:00:00,+05:00,g,Ana,e,sideways,,../secret.txt\n"
                .as_bytes(),
        );
        // No --timezone needed: every wall-clock time has a zone in its row.
        let p = parse(&e, &ParseOptions::default()).unwrap();
        assert_eq!(p.events.len(), 3);
        assert_eq!(
            p.events[1].timestamp.unwrap().to_rfc3339(),
            "2026-03-14T15:00:00+00:00"
        );
        assert_eq!(p.events[1].body, "b\nc");
        assert_eq!(
            (p.events[1].source.line, p.events[1].source.line_end),
            (3, 4)
        );
        assert!(p.events[1].deleted);
        assert_eq!(p.events[1].direction, Some(Direction::Outgoing));
        assert_eq!(
            p.events[2].timestamp.unwrap().to_rfc3339(),
            "2026-03-14T06:00:00+00:00"
        );
        assert_eq!(p.events[2].custom["direction_raw"], "sideways");
        let codes: Vec<&str> = p.findings.iter().map(|f| f.code.as_str()).collect();
        assert!(codes.contains(&"UNPARSED_LINES"));
        assert!(codes.contains(&"ATTACHMENT_REFERENCED_NOT_FOUND"));
        assert!(codes.contains(&"PARTICIPANT_IDENTIFIED_BY_NUMBER_ONLY"));
        let u = p
            .findings
            .iter()
            .find(|f| f.code == "UNPARSED_LINES")
            .unwrap();
        assert_eq!(u.params["count"], "2");
        assert!(
            u.params["examples"].starts_with("row 4 (line 6)"),
            "{}",
            u.params["examples"]
        );
        // "Ana" alone is the Ana with +57 300 000 0002: one participant.
        assert_eq!(p.participants.len(), 2);
        assert!(matches!(
            &p.events[2].attachments[0].data,
            AttachmentData::Missing { reason } if reason.starts_with("path outside")
        ));
    }

    #[test]
    fn draft_profile_is_flagged_and_epochs_need_no_zone() {
        let d = tempfile::tempdir().unwrap();
        let e = export_of(
            d.path(),
            "x.tsv",
            b"Time\tChat ID\tFrom\tBody\n2026-03-14T10:00:00+01:00\tc1\tAna\thola\n",
        );
        let o = ParseOptions {
            csv_profile: CsvProfile::XryDraft,
            ..Default::default()
        };
        let p = parse(&e, &o).unwrap();
        assert_eq!(p.findings[0].code, "CSV_MAPPING_DRAFT_PROFILE");
        assert_eq!(p.findings[0].params["profile"], "xry-draft");
        assert_eq!(p.facts["csv.delimiter"], "tab");

        let map = d.path().join("m.toml");
        std::fs::write(
            &map,
            "[columns]\ntimestamp=\"ms\"\ntimestamp_format=\"epoch_ms\"\nconversation=\"c\"\nsender=\"s\"\nbody=\"b\"\n[options]\nskip_rows=2\n",
        )
        .unwrap();
        let e = export_of(
            d.path(),
            "y.csv",
            b"Report\nexported\nms,c,s,b\n1773482400000,c,Ana,x\n",
        );
        let o = ParseOptions {
            csv_map: Some(map),
            csv_profile: CsvProfile::XryDraft,
            ..Default::default()
        };
        let p = parse(&e, &o).unwrap();
        assert!(p.findings.is_empty(), "{:?}", p.findings);
        assert_eq!(
            p.events[0].timestamp.unwrap().to_rfc3339(),
            "2026-03-14T10:00:00+00:00"
        );
        assert_eq!(p.events[0].source.line, 4);
    }

    #[test]
    fn sibling_map_toml_wins_over_the_profile() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("chat.map.toml"),
            "[columns]\ntimestamp=\"w\"\nconversation=\"c\"\nsender=\"s\"\nbody=\"b\"\n",
        )
        .unwrap();
        let e = export_of(
            d.path(),
            "chat.csv",
            b"w,c,s,b\n2026-03-14T10:00:00Z,c,Ana,x\n",
        );
        let o = ParseOptions {
            csv_profile: CsvProfile::CellebriteDraft,
            ..Default::default()
        };
        let p = parse(&e, &o).unwrap();
        assert!(p.findings.is_empty(), "{:?}", p.findings);
        assert_eq!(p.facts["mapping"], "chat.map.toml");
        assert_eq!(p.facts["mapping_source"], "sibling");
        // --csv-map wins over the sibling.
        let map = d.path().join("other.toml");
        std::fs::write(&map, "[columns]\ntimestamp=\"nope\"\n").unwrap();
        let o = ParseOptions {
            csv_map: Some(map),
            ..o
        };
        assert!(matches!(parse(&e, &o), Err(ParseError::NeedsMapping(_))));
        // No sibling: the profile.
        let e2 = export_of(
            d.path(),
            "x.csv",
            b"Time,Chat,From,Body\n2026-03-14T10:00:00Z,c,Ana,x\n",
        );
        let p = parse(
            &e2,
            &ParseOptions {
                csv_profile: CsvProfile::CellebriteDraft,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(p.facts["mapping_source"], "profile:cellebrite-draft");
        assert_eq!(p.findings[0].code, "CSV_MAPPING_DRAFT_PROFILE");
    }

    #[test]
    fn parties() {
        assert_eq!(
            parse_party("Laura Pérez <+57 300 000 0001>").unwrap(),
            Party {
                name: Some("Laura Pérez".into()),
                number: Some("+57 300 000 0001".into()),
                email: None
            }
        );
        assert_eq!(
            parse_party("+57 300 000 0002").unwrap().key(),
            "tel:+573000000002"
        );
        assert_eq!(parse_party("a@b.co").unwrap().key(), "email:a@b.co");
        assert!(parse_party("  ").is_none());
        assert!(!is_number("12-03"));
        assert!(!is_number("Room 12345"));
    }
}
