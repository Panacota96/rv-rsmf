//! Golden vectors for every date and time path of rv-rsmf, frozen from chrono 0.4.45 + chrono-tz 0.10.4
//! (IANA 2025b) before the move to jiff. Every line of `expected.txt` is `case => result`; the cases cover
//! zone names, local → UTC (DST gaps and overlaps), UTC → local day (slicing), RSMF / JSON time formatting,
//! the WhatsApp, Telegram, Teams and CSV time readers (every chrono strftime specifier, RFC 3339, epochs,
//! offsets, zone columns) and the RSMF bytes written for every sample export.
//!
//! Regenerate (only when a behaviour change is intended):
//! `cargo test -p rv-rsmf-core golden::tests::print -- --ignored --nocapture > src/core/src/golden/expected.txt`
//! (then drop cargo's own lines). Only the adapters below may depend on the date library.

use crate::model::{Event, ParsedExport, SourceKind, SourceLoc};
use crate::rsmf::{self, WriteOptions, local_to_utc, rsmf_time};
use crate::sources::{
    self, DateOrder, ExportInput, ParseError, ParseOptions, csvmap, teams, telegram,
};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// ------------------------------------------------------------------ adapters (date library specific)

use crate::datetime::{NaiveDate, NaiveDateTime, UtcTime};
type Zone = crate::zone::Tz;
type Instant = UtcTime;
type Local = NaiveDateTime;

fn zone(s: &str) -> Option<Zone> {
    s.parse().ok()
}
fn zone_name(z: &Zone) -> String {
    z.name().to_string()
}
fn instant(secs: i64, nanos: u32) -> Option<Instant> {
    UtcTime::from_timestamp(secs, nanos)
}
fn local(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Option<Local> {
    NaiveDate::from_ymd_opt(y, mo, d)?.and_hms_opt(h, mi, s)
}
fn show(t: &Instant) -> String {
    format!(
        "{} {}.{:09} {}",
        rsmf_time(t),
        t.timestamp(),
        t.timestamp_subsec_nanos(),
        serde_json::to_string(t).unwrap()
    )
}
fn show_local(n: &Local) -> String {
    show(&n.and_utc())
}
/// Local calendar day and hour of `t` in `z` (what slicing uses).
fn local_day_hour(t: &Instant, z: &Zone) -> String {
    let l = z.to_local(t);
    format!("{} h{:02}", l.date().ymd(), l.hour())
}
fn utc_of(t: &Local, z: &Zone) -> (Instant, Option<&'static str>) {
    local_to_utc(*t, z)
}
fn offset_secs(o: &csvmap::AutoTs) -> String {
    o.offset
        .map(|o| o.local_minus_utc().to_string())
        .unwrap_or_else(|| "-".into())
}

// ------------------------------------------------------------------ cases

const SAMPLE_ZONES: [&str; 34] = [
    "UTC",
    "Europe/Paris",
    "America/New_York",
    "Asia/Kolkata",
    "Australia/Lord_Howe",
    "Europe/Dublin",
    "America/Sao_Paulo",
    "Pacific/Apia",
    "Africa/Casablanca",
    "America/Bogota",
    "Asia/Tehran",
    "Pacific/Chatham",
    "Antarctica/Troll",
    "America/St_Johns",
    "Pacific/Kiritimati",
    "Europe/Moscow",
    "Asia/Gaza",
    "America/Nuuk",
    "Europe/Kyiv",
    "America/Mexico_City",
    "Australia/Sydney",
    "Europe/Madrid",
    "America/Santiago",
    "Asia/Calcutta",
    "US/Eastern",
    "Etc/GMT+5",
    "EST5EDT",
    "Europe/London",
    "America/Los_Angeles",
    "Asia/Tokyo",
    "Africa/Cairo",
    "Asia/Almaty",
    "America/Asuncion",
    "America/Coyhaique",
];

const TRANSITION_YEARS: [i16; 12] = [
    1916, 1945, 1971, 1996, 2011, 2022, 2024, 2026, 2027, 2037, 2038, 2099,
];

/// Fixed wall-clock times tried in every sample zone (far past and future included).
const LOCAL_TIMES: [(i32, u32, u32, u32, u32, u32); 24] = [
    (1, 1, 1, 0, 0, 0),
    (1850, 6, 1, 12, 0, 0),
    (1900, 1, 1, 0, 0, 0),
    (1945, 7, 1, 12, 0, 0),
    (1970, 1, 1, 0, 0, 0),
    (2000, 1, 1, 0, 0, 0),
    (2024, 2, 29, 12, 0, 0),
    (2026, 1, 15, 12, 0, 0),
    (2026, 3, 29, 2, 30, 0),
    (2026, 7, 15, 12, 0, 0),
    (2026, 10, 25, 2, 30, 0),
    (2037, 7, 1, 12, 0, 0),
    (2038, 7, 1, 12, 0, 0),
    (2050, 1, 15, 12, 0, 0),
    (2050, 7, 15, 12, 0, 0),
    (2099, 7, 15, 12, 0, 0),
    (2099, 12, 31, 23, 59, 59),
    (2100, 1, 15, 12, 0, 0),
    (2100, 3, 28, 2, 30, 0),
    (2100, 7, 15, 12, 0, 0),
    (2200, 7, 15, 12, 0, 0),
    (9999, 12, 31, 23, 59, 59),
    (30000, 6, 1, 12, 0, 0),
    (-100, 6, 1, 12, 0, 0),
];

/// Fixed instants (Unix seconds, nanoseconds) tried in every sample zone and for formatting.
const INSTANTS: [(i64, u32); 24] = [
    (0, 0),
    (-1, 0),
    (-1, 999_999_999),
    (1_774_747_800, 0),
    (1_774_747_800, 1_000_000),
    (1_774_747_800, 123_000),
    (1_774_747_800, 123),
    (1_774_747_800, 999_999_999),
    (1_774_747_799, 1_500_000_000),
    (253_402_300_799, 0),
    (253_402_300_800, 0),
    (910_692_730_085, 477_580_700),
    (-62_135_596_800, 0),
    (-62_167_219_200, 0),
    (-62_167_219_201, 0),
    (4_102_444_800, 0),
    (4_118_083_200, 0),
    (7_258_118_400, 0),
    (8_210_298_412_799, 0),
    (8_210_298_412_800, 0),
    (-8_334_632_851_200, 0),
    (-8_334_632_851_201, 0),
    (i64::MAX, 0),
    (0, 2_000_000_000),
];

fn sample_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sample")
}

fn push(out: &mut Vec<String>, case: impl AsRef<str>, result: impl AsRef<str>) {
    let line = format!("{} => {}", case.as_ref(), result.as_ref());
    out.push(line.replace('\n', "\\n").replace('\r', "\\r"));
}

fn zone_names(out: &mut Vec<String>) {
    let extra = [
        "",
        " Europe/Paris",
        "Europe/Paris ",
        "europe/paris",
        "EUROPE/PARIS",
        "utc",
        "Utc",
        "Z",
        "GMT",
        "Zulu",
        "UCT",
        "Etc/UTC",
        "Etc/Unknown",
        "Factory",
        "localtime",
        "posixrules",
        "+02:00",
        "UTC+2",
        "CET",
        "EST",
        "Paris",
        "America/Coyhaique",
        "Europe/Kiev",
        "Asia/Calcutta",
        "US/Pacific-New",
        "Asia/Rangoon",
        "Europe/Paris/",
        "Europe//Paris",
        "Europe\\Paris",
        "Etc/GMT-14",
        "Etc/GMT+12",
        "Etc/GMT+13",
    ];
    let listed = include_str!("tz_names.txt");
    for n in listed.lines().chain(extra) {
        push(
            out,
            format!("zone {n:?}"),
            zone(n).map(|z| zone_name(&z)).unwrap_or("ERR".into()),
        );
    }
}

/// Wall-clock probes around every transition of the sample zones in the sample years (computed with the
/// IANA data bundled in jiff: the inputs only, never the expected results).
fn transition_probes(name: &str) -> (Vec<(i64, String)>, Vec<i64>) {
    let mut locals = vec![];
    let mut instants = vec![];
    let Ok(tz) = jiff::tz::TimeZoneDatabase::bundled().get(name) else {
        return (locals, instants);
    };
    for y in TRANSITION_YEARS {
        let start = jiff::civil::date(y, 1, 1)
            .at(0, 0, 0, 0)
            .in_tz("UTC")
            .unwrap()
            .timestamp();
        let end = jiff::civil::date(y + 1, 1, 1)
            .at(0, 0, 0, 0)
            .in_tz("UTC")
            .unwrap()
            .timestamp();
        for tr in tz
            .following(start)
            .take_while(|t| t.timestamp() < end)
            .take(8)
        {
            let t = tr.timestamp().as_second();
            let after = i64::from(tr.offset().seconds());
            let before = i64::from(
                tz.to_offset(jiff::Timestamp::from_second(t - 1).unwrap())
                    .seconds(),
            );
            instants.extend([t - 1, t, t + 1]);
            let (l1, l2) = (t + before.min(after), t + before.max(after));
            for l in [l1 - 1, l1, (l1 + l2) / 2, l2 - 1, l2] {
                locals.push((l, format!("{y} transition {t}")));
            }
        }
    }
    (locals, instants)
}

fn civil_of(secs: i64) -> (i32, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let sod = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (yoe + era * 400 + i64::from(m <= 2)) as i32;
    (
        y,
        m,
        d,
        (sod / 3600) as u32,
        (sod % 3600 / 60) as u32,
        (sod % 60) as u32,
    )
}

fn zones_local_and_utc(out: &mut Vec<String>) {
    for name in SAMPLE_ZONES {
        let Some(z) = zone(name) else {
            push(out, format!("zone {name}"), "ERR");
            continue;
        };
        let (probes, instants) = transition_probes(name);
        let fixed = LOCAL_TIMES.iter().map(|&t| (t, String::from("fixed")));
        let around = probes.into_iter().map(|(l, why)| (civil_of(l), why));
        for ((y, mo, d, h, mi, s), why) in fixed.chain(around) {
            let case = format!("local {name} {y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} ({why})");
            match local(y, mo, d, h, mi, s) {
                None => push(out, case, "invalid"),
                Some(l) => {
                    let (t, why) = utc_of(&l, &z);
                    push(
                        out,
                        case,
                        format!("{} {:?} | {}", show(&t), why, local_day_hour(&t, &z)),
                    );
                }
            }
        }
        let fixed = INSTANTS.iter().map(|&(s, n)| (s, n));
        for (s, n) in fixed.chain(instants.into_iter().map(|s| (s, 0))) {
            if let Some(t) = instant(s, n) {
                push(
                    out,
                    format!("utc→local {name} {s}.{n:09}"),
                    local_day_hour(&t, &z),
                );
            }
        }
    }
}

fn instants_and_order(out: &mut Vec<String>) {
    let mut all = vec![];
    for (s, n) in INSTANTS {
        match instant(s, n) {
            None => push(out, format!("instant {s}.{n:09}"), "None"),
            Some(t) => {
                push(out, format!("instant {s}.{n:09}"), show(&t));
                all.push(t);
            }
        }
    }
    // A leap second (only RFC 3339 text can produce one).
    if let Ok(csvmap::Cell::Instant(t)) =
        csvmap::first_look("2016-12-31T23:59:60.5Z", &csvmap::TsFormat::Iso)
    {
        all.push(t);
    }
    if let Ok(csvmap::Cell::Instant(t)) =
        csvmap::first_look("2016-12-31T23:59:59.9Z", &csvmap::TsFormat::Iso)
    {
        all.push(t);
    }
    if let Ok(csvmap::Cell::Instant(t)) =
        csvmap::first_look("2017-01-01T00:00:00Z", &csvmap::TsFormat::Iso)
    {
        all.push(t);
    }
    all.sort();
    for (i, t) in all.iter().enumerate() {
        push(out, format!("sorted {i}"), show(t));
    }
    for (a, b) in all.iter().zip(all.iter().skip(1)) {
        push(
            out,
            format!("cmp {} {}", show(a), show(b)),
            format!("{:?} {}", a.cmp(b), a == b),
        );
    }
}

fn whatsapp_text(stamp: &str) -> String {
    if stamp.starts_with('[') {
        format!("{stamp} Ana: hola\n")
    } else {
        format!("{stamp} - Ana: hola\n")
    }
}

fn describe(r: &Result<ParsedExport, ParseError>) -> String {
    match r {
        Err(e) => format!("ERR {e:?}"),
        Ok(p) => {
            let mut s = vec![];
            for e in &p.events {
                s.push(format!(
                    "{} {} src={:?} {:?}",
                    e.id,
                    e.timestamp.as_ref().map(show).unwrap_or("-".into()),
                    e.source_time,
                    e.custom
                ));
                for ed in &e.edits {
                    s.push(format!(
                        "edit {}",
                        ed.timestamp.as_ref().map(show).unwrap_or("-".into())
                    ));
                }
            }
            for f in &p.findings {
                s.push(format!("{} {:?}", f.code, f.params));
            }
            s.push(format!("facts {:?}", p.facts));
            s.join(" ; ")
        }
    }
}

fn run_export(
    kind: SourceKind,
    name: &str,
    bytes: &[u8],
    opts: &ParseOptions,
) -> Result<ParsedExport, ParseError> {
    let d = tempfile::tempdir().unwrap();
    let p = d.path().join(name);
    std::fs::write(&p, bytes).unwrap();
    let e = ExportInput {
        kind,
        primary: p.clone(),
        root: d.path().to_path_buf(),
        members: vec![p],
    };
    sources::parse(&e, opts)
}

fn whatsapp(out: &mut Vec<String>) {
    let stamps = [
        "29/03/2026, 02:30",
        "25/10/2026, 02:30",
        "03/04/2026, 10:00",
        "29/02/2024, 12:00",
        "29/02/2026, 12:00",
        "31/04/2026, 12:00",
        "3/4/26, 9:05",
        "13.06.2018, 21:25:15",
        "13. 6. 2018., 21.25.15",
        "2026-03-29, 02:30",
        "2026/03/29, 02:30",
        "01/01/00, 00:00",
        "1/1/99, 23:59:59",
        "12/12/2026, 24:00",
        "12/12/2026, 23:60",
        "12/12/2026, 23:59:60",
        "2/3/26, 12:00 AM",
        "2/3/26, 12:00 PM",
        "2/3/26, 0:30 AM",
        "2/3/26, 13:00 PM",
        "2/3/26, 1:55 p. m.",
        "2/3/26, 9:10 a.m.",
        "2/3/26, 1:30\u{202F}PM",
        "[4/3/26, 9:15:02 p. m.]",
        "[29/03/2026, 02:30:00]",
        "31/12/9999, 23:59",
        "01/01/0001, 00:00",
        "01/01/0100, 00:00",
        "01/07/2150, 12:00",
        "28/03/2100, 02:30",
        "13/13/2026, 10:00",
        "00/01/2026, 10:00",
        "29/12/2011, 12:00",
        "30/12/2011, 12:00",
    ];
    let settings: [(&str, Option<DateOrder>); 5] = [
        ("Europe/Paris", Some(DateOrder::Dmy)),
        ("America/New_York", Some(DateOrder::Mdy)),
        ("Asia/Kolkata", Some(DateOrder::Dmy)),
        ("UTC", None),
        ("Pacific/Apia", Some(DateOrder::Dmy)),
    ];
    for stamp in stamps {
        for (z, order) in settings {
            let opts = ParseOptions {
                timezone: zone(z),
                date_order: order,
                ..ParseOptions::default()
            };
            let r = run_export(
                SourceKind::Whatsapp,
                "chat.txt",
                whatsapp_text(stamp).as_bytes(),
                &opts,
            );
            push(
                out,
                format!("whatsapp {stamp:?} {z} {order:?}"),
                describe(&r),
            );
        }
    }
}

fn telegram_readers(out: &mut Vec<String>) {
    let locals = [
        "2026-03-29T02:30:00",
        "2026-10-25T02:30:00",
        "2026-03-29T02:30:60",
        "2026-3-9T2:3:4",
        " 2026-03-29T02:30:00",
        "2026-03-29T02:30:00 ",
        "2026-03-29 02:30:00",
        "2026-03-29T 02:30:00",
        "+2026-03-29T02:30:00",
        "-0001-03-29T02:30:00",
        "+12026-03-29T02:30:00",
        "20260-03-29T02:30:00",
        "2026-03-29T02:30",
        "2026-03-29T02:30:00.5",
        "2026-03-29T02:30:00Z",
        "2024-02-29T00:00:00",
        "2026-02-29T00:00:00",
        "2026-03-29T24:00:00",
        "0000-01-01T00:00:00",
        "+262142-12-31T23:59:59",
        "+262143-01-01T00:00:00",
        "-262143-01-01T00:00:00",
        "-262144-12-31T23:59:59",
        "2026-03-29T02:30:0",
        "2026-03-29T02:30:005",
        "26-03-29T02:30:00",
        "2026-003-29T02:30:00",
        "２０２６-03-29T02:30:00",
        "",
    ];
    for s in locals {
        let v = serde_json::Value::String(s.into());
        push(
            out,
            format!("telegram local {s:?}"),
            telegram::local(&v)
                .map(|l| show_local(&l))
                .unwrap_or("None".into()),
        );
    }
    let epochs: [serde_json::Value; 16] = [
        "0".into(),
        "1774747800".into(),
        "-1".into(),
        "  12  ".into(),
        "1.5".into(),
        "".into(),
        "+5".into(),
        "9223372036854775807".into(),
        "-9223372036854775808".into(),
        "8210298412799".into(),
        "8210298412800".into(),
        "-8334632851200".into(),
        "-8334632851201".into(),
        serde_json::json!(1774747800),
        serde_json::json!(1.5),
        serde_json::json!(null),
    ];
    for v in epochs {
        push(
            out,
            format!("telegram epoch {v}"),
            telegram::epoch(&v)
                .map(|t| show(&t))
                .unwrap_or("None".into()),
        );
    }
    // A small export: local dates (zone needed), unixtime, edits, a DST gap and overlap.
    let json = r#"{"name":"Ana","type":"personal_chat","id":1,"messages":[
        {"id":1,"type":"message","date":"2026-03-29T02:30:00","from":"Ana","from_id":"user1","text":"gap","edited":"2026-10-25T02:30:00"},
        {"id":2,"type":"message","date":"2026-10-25T02:30:00","from":"Bob","from_id":"user2","text":"overlap"},
        {"id":3,"type":"message","date":"2026-07-01T12:00:00","date_unixtime":"1782900000","from":"Ana","from_id":"user1","text":"unix","edited_unixtime":"1782900060"},
        {"id":4,"type":"message","date":"bad","from":"Ana","from_id":"user1","text":"no time"}]}"#;
    for z in ["Europe/Paris", "America/New_York", "UTC", ""] {
        let opts = ParseOptions {
            timezone: zone(z),
            ..ParseOptions::default()
        };
        let r = run_export(SourceKind::Telegram, "result.json", json.as_bytes(), &opts);
        push(out, format!("telegram export {z:?}"), describe(&r));
    }
}

fn teams_times(out: &mut Vec<String>) {
    let fts: [u64; 12] = [
        0,
        1,
        116_444_736_000_000_000,
        116_444_736_000_000_001,
        116_444_735_999_999_999,
        134_000_000_000_000_000,
        2_650_467_743_999_999_999,
        2_650_467_744_000_000_000,
        9_223_372_036_854_775_807,
        9_999_999_999_999_999_999,
        u64::MAX - 1,
        u64::MAX,
    ];
    for ft in fts {
        let r = teams::filetime_to_utc(ft)
            .map(|t| {
                format!(
                    "{} | {} | back {}",
                    show(&t),
                    teams::source_time(ft, &t, "PR_CLIENT_SUBMIT_TIME"),
                    if ft < 18_000_000_000_000_000_000 {
                        teams::utc_to_filetime(t).to_string()
                    } else {
                        "-".into()
                    }
                )
            })
            .unwrap_or("None".into());
        push(out, format!("filetime {ft}"), r);
    }
}

fn csv_first_look(out: &mut Vec<String>) {
    let formats = [
        "auto",
        "iso",
        "epoch_s",
        "epoch_ms",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
        "%d/%m/%Y %H:%M",
        "%d/%m/%Y",
        "%Y-%m-%dT%H:%M:%S%z",
        "%Y-%m-%dT%H:%M:%S%:z",
        "%Y-%m-%dT%H:%M:%S%.f%:z",
        "%Y-%m-%d %H:%M:%S %z",
        "%Y-%m-%d %H:%M:%S%#z",
        "%Y-%m-%d %H:%M:%S %::z",
        "%Y-%m-%d %H:%M:%S %:::z",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.3f",
        "%Y-%m-%d %H:%M:%S%.6f",
        "%Y-%m-%d %H:%M:%S%.9f",
        "%Y-%m-%d %H:%M:%S.%f",
        "%Y-%m-%d %H:%M:%S.%3f",
        "%Y-%m-%d %H:%M:%S.%6f",
        "%Y-%m-%d %H:%M:%S.%9f",
        "%m/%d/%y %I:%M %p",
        "%m/%d/%y %l:%M %P",
        "%e %b %Y %H:%M",
        "%d %B %Y %H:%M",
        "%a %d %b %Y %T",
        "%A, %d %B %Y %R",
        "%F %T",
        "%D %T",
        "%c",
        "%x %X",
        "%r",
        "%s",
        "%s%.f",
        "%Y-%j %H:%M",
        "%G-W%V-%u %H:%M",
        "%g-W%V-%u %H:%M",
        "%Y-%U-%w %H:%M",
        "%Y-%W-%u %H:%M",
        "%C%y-%m-%d %H:%M",
        "%y-%m-%d %H:%M",
        "%Y-%m-%d %k:%M",
        "%Y-%m-%d %H:%M:%S %Z",
        "%+",
        "%-d/%-m/%Y %H:%M",
        "%_d/%m/%Y %H:%M",
        "%0d/%m/%Y %H:%M",
        "%Y-%m-%d %H:%M %q",
        "%Y%m%d%H%M%S",
        "%Y-%m-%d%n%H:%M",
        "%Y-%m-%d%t%H:%M",
        "%Y-%m-%d %H:%M %%",
        "%Y-%m-%d %H:%M %Q",
        "%Y-%m-%d %H:%M %",
        "%-D",
        "%#d",
        "%Y-%m-%d %H:%M:%S%",
        "%h %d %Y",
        "%v %H:%M",
        "%H:%M:%S",
        "%Y-%m-%d %I:%M:%S",
        "%Y-%m-%d %H:%M:%S %p",
        "%d.%m.%Y %H:%M:%S",
        "%d/%m/%Y, %H:%M",
        "  %Y-%m-%d %H:%M  ",
        "%Y-%m-%d  %H:%M",
        "%Y-%m-%d %H:%M:%S%:z %Z",
        "%Y-%m-%d %H:%M:%S %z %s",
        "%s %Y",
        "%Y-%m-%d %H:%M:%S %s",
    ];
    let inputs = [
        "2026-03-29 02:30:00",
        "2026-03-29 02:30",
        "2026-03-29T02:30:00Z",
        "2026-03-29T02:30:00+02:00",
        "2026-03-29T02:30:00+0200",
        "2026-03-29T02:30:00.123456789+02:00",
        "2026-03-29T02:30:00.1234567891+02:00",
        "2026-03-29 02:30:00.5",
        "2026-03-29 02:30:00.123",
        "2026-03-29 02:30:00.123456",
        "2026-03-29 02:30:60",
        "2016-12-31 23:59:60",
        "29/03/2026 02:30",
        "29/03/2026",
        "03/04/2026 10:00",
        "3/4/26 9:05",
        "03/04/26 09:05 PM",
        "03/04/26 12:05 am",
        "03/04/26 9:05 pm",
        "29 Mar 2026 02:30",
        "29 March 2026 02:30",
        " 9 Mar 2026 02:30",
        "9 mar 2026 02:30",
        "Sun 29 Mar 2026 02:30:00",
        "Mon 29 Mar 2026 02:30:00",
        "Sunday, 29 March 2026 02:30",
        "Sun Mar 29 02:30:00 2026",
        "03/29/26 02:30:00",
        "02:30:00 PM",
        "02:30:00",
        "1774747800",
        "1774747800.5",
        "1774747800123",
        "-1",
        "2026-088 02:30",
        "2026-W13-7 02:30",
        "26-W13-7 02:30",
        "2026-13-0 02:30",
        "2026-12-7 02:30",
        "2026-03-29 2:30",
        "2026-03-29 02:30:00 CET",
        "2026-03-29 02:30:00 +02:00",
        "2026-03-29 02:30:00 +02",
        "2026-03-29 02:30:00 +02:00:00",
        "2026-03-29 02:30:00 \u{2212}02:00",
        "2026-03-29 02:30:00+02:00 CEST",
        "2026-03-29 02:30:00 +0200 1774744200",
        "2026-03-29 02:30:00 1774747800",
        "1774747800 2026",
        "20260329023000",
        "2024-02-29 12:00:00",
        "2026-02-29 12:00:00",
        "2026-03-29 24:00:00",
        "",
        "  2026-03-29 02:30:00  ",
        "2026-03-29 02:30 %",
        "2026-03-29\t02:30",
        "2026-03-29\n02:30",
        "2026-03-29  02:30",
        "29/03/2026 02:30(UTC+2)",
        "29.03.2026 02:30:00 GMT",
        "29.03.2026 02:30:00",
        "29/03/2026, 02:30",
        "2026-03-29T02:30:00z",
        "2026-03-29t02:30:00Z",
        "2026-03-29 02:30:00Z",
        "2026-03-29T02:30:00UTC",
        "2026-03-29T02:30:00 UTC",
        "2026-10-25 02:30:00",
        "+2026-03-29 02:30:00",
        "-2026-03-29 02:30:00",
        "12026-03-29 02:30:00",
        "2026-03-29 02:30:00,5",
        "29/03/2026, 2:30 p. m.",
        "2026-03-29 02:30 Q1",
        "2026-03-29 02:30 1",
        "Mar 29 2026",
        "29-Mar-2026 02:30",
        "1/1/2026 0:00",
        "2026-03-29 14:30:00 PM",
        "2026-03-29 02:30:00 AM",
        "2026-03-29 00:30:00 AM",
        "9999-12-31 23:59:59",
        "0000-01-01 00:00:00",
        "2026-03-29T02:30:00.Z",
        "2026-03-29T02:30:00+24:00",
        "2026-03-29T02:30:00+23:59",
        "2026-03-29T02:30:00+02:60",
        "2026-03-29T02:30:00-00:00",
    ];
    for f in formats {
        let tf = csvmap::TsFormat::parse(Some(f));
        for s in inputs {
            let r = match csvmap::first_look(s, &tf) {
                Err(e) => format!("E {e}"),
                Ok(csvmap::Cell::Instant(t)) => format!("I {}", show(&t)),
                Ok(csvmap::Cell::Local(l)) => format!("L {}", show_local(&l)),
                Ok(csvmap::Cell::Auto(a)) => {
                    let n = |o| a.naive(o).map(|l| show_local(&l)).unwrap_or("None".into());
                    format!(
                        "A {:?} ymd={} {:?} {} off={} | dmy {} | mdy {} | ymd {}",
                        a.parts,
                        a.ymd,
                        a.hms,
                        a.nanos,
                        offset_secs(&a),
                        n(DateOrder::Dmy),
                        n(DateOrder::Mdy),
                        n(DateOrder::Ymd)
                    )
                }
            };
            push(out, format!("csv {f:?} {s:?}"), r);
        }
    }
    for o in [
        "Z",
        "+02:00",
        "-0530",
        "(UTC-5)",
        "UTC",
        "GMT",
        "UTC+2",
        "UTC+14",
        "UTC+15",
        "+1:5",
        "+01:60",
        "+0",
        "+123",
        "+12345",
        "",
        "utc+2",
        "Z ",
        "(UTC+05:30)",
        "\u{2212}02:00",
        "+02:00:00",
        "GMT-3",
        "+ 2",
        "-00:00",
        "+14:59",
        "UTC-14",
        "(GMT+01:00)",
        "+2:30",
    ] {
        push(
            out,
            format!("offset {o:?}"),
            csvmap::parse_offset(o)
                .map(|x| x.local_minus_utc().to_string())
                .unwrap_or("None".into()),
        );
        push(
            out,
            format!("zone column {o:?}"),
            csvmap::Zone::parse(o)
                .map(|z| z.name())
                .unwrap_or("None".into()),
        );
    }
    for z in [
        "Europe/Paris",
        " Europe/Paris ",
        "europe/paris",
        "Asia/Calcutta",
        "UTC",
        "Paris",
    ] {
        push(
            out,
            format!("zone column {z:?}"),
            csvmap::Zone::parse(z)
                .map(|z| z.name())
                .unwrap_or("None".into()),
        );
    }
}

fn csv_export(out: &mut Vec<String>) {
    let map = |fmt: &str| {
        format!(
            "[columns]\ntimestamp=\"t\"\ntimestamp_format=\"{fmt}\"\ntimezone_column=\"z\"\nconversation=\"c\"\nsender=\"s\"\nbody=\"b\"\n"
        )
    };
    let rows = [
        ("29/03/2026 02:30:00", ""),
        ("25/10/2026 02:30:00", ""),
        ("15/07/2026 12:00:00", "Europe/Paris"),
        ("15/07/2026 12:00:00", "+02:00"),
        ("15/07/2026 12:00:00", "UTC+2"),
        ("15/07/2026 12:00:00", "(UTC-05:00)"),
        ("15/07/2026 12:00:00", "GMT"),
        ("15/07/2026 12:00:00", "Paris"),
        ("15/07/2026 12:00:00", "europe/paris"),
        ("15/07/2026 12:00:00", "-0530"),
        ("15/07/2026 12:00:00", "UTC-14"),
        ("15/07/2026 12:00:00", " Asia/Kolkata "),
        ("29/03/2026 02:30:00", "America/New_York"),
        ("08/03/2026 02:30:00", "America/New_York"),
        ("01/11/2026 01:30:00", "America/New_York"),
        ("15/07/2026 12:00:00 +02:00", ""),
        ("15/07/2026 12:00:00 +02:00", "Asia/Tokyo"),
        ("2026-07-15T12:00:00+02:00", "Asia/Tokyo"),
        ("2026-07-15T12:00:00.250Z", ""),
        ("31/12/9999 23:59:59", "-14:00"),
        ("31/12/9999 23:59:59", "Pacific/Kiritimati"),
        ("31/12/9999 23:59:59", "America/New_York"),
        ("30/02/2026 10:00:00", ""),
        ("x", ""),
    ];
    let mut csv = String::from("t,z,c,s,b\n");
    for (t, z) in rows {
        csv.push_str(&format!("\"{t}\",\"{z}\",c1,Ana,hola\n"));
    }
    for z in ["Europe/Paris", "UTC", ""] {
        let d = tempfile::tempdir().unwrap();
        let m = d.path().join("m.toml");
        std::fs::write(&m, map("auto")).unwrap();
        let opts = ParseOptions {
            timezone: zone(z),
            date_order: Some(DateOrder::Dmy),
            csv_map: Some(m),
            ..ParseOptions::default()
        };
        let r = run_export(SourceKind::Csv, "chat.csv", csv.as_bytes(), &opts);
        push(out, format!("csv export auto {z:?}"), describe(&r));
    }
    let mut csv = String::from("t,z,c,s,b\n");
    for t in [
        "29/03/2026 02:30",
        "25/10/2026 02:30",
        "31/02/2026 02:30",
        "29/03/2026",
    ] {
        csv.push_str(&format!("\"{t}\",,c1,Ana,hola\n"));
    }
    let d = tempfile::tempdir().unwrap();
    let m = d.path().join("m.toml");
    std::fs::write(&m, map("%d/%m/%Y %H:%M")).unwrap();
    let opts = ParseOptions {
        timezone: zone("Europe/Paris"),
        csv_map: Some(m),
        ..ParseOptions::default()
    };
    let r = run_export(SourceKind::Csv, "chat.csv", csv.as_bytes(), &opts);
    push(out, "csv export strftime Europe/Paris", describe(&r));
}

fn validator(out: &mut Vec<String>) {
    for t in [
        "2026-01-01T10:00:00Z",
        "2026-01-01T10:00:00+02:00",
        "2026-01-01T10:00:00.5Z",
        "2016-12-31T23:59:60Z",
        "2026-01-01 10:00:00Z",
        "2026-01-01t10:00:00z",
        "2026-01-01T10:00Z",
        "2026-02-30T10:00:00Z",
        "2026-01-01T10:00:00.Z",
        "+2026-01-01T10:00:00Z",
        "2026-01-01T24:00:00Z",
        "2026-01-01T10:00:00ZZ",
        "2026-01-01T10:00:00+00:00Z",
        "2026-01-01T10:00:00.1234567891Z",
        "0000-01-01T00:00:00Z",
        "Z",
    ] {
        let m = serde_json::json!({"version": "2.0.0", "participants": [{"id": "p1"}],
            "conversations": [{"id": "c1", "platform": "x", "participants": ["p1"]}],
            "events": [{"type": "message", "participant": "p1", "conversation": "c1", "timestamp": t}]});
        let v = rsmf::validate_manifest(&m, &BTreeMap::new());
        let bad = v
            .strict_errors
            .iter()
            .any(|e| e.contains("is not RFC 3339 UTC"));
        push(
            out,
            format!("validate timestamp {t:?}"),
            if bad { "rejected" } else { "accepted" },
        );
    }
}

fn slicing(out: &mut Vec<String>) {
    for z in [
        "Europe/Paris",
        "Pacific/Kiritimati",
        "America/St_Johns",
        "UTC",
    ] {
        let tz = zone(z).unwrap();
        let mut p = ParsedExport::default();
        for (i, (s, n)) in INSTANTS.iter().enumerate() {
            let mut e = Event::message(format!("e-{i:06}"), "c".into(), SourceLoc::default());
            e.timestamp = instant(*s, *n);
            p.events.push(e);
        }
        let slices = rsmf::slice(&p, &tz);
        let stems: Vec<String> = slices
            .iter()
            .map(|s| format!("{}:{:?}", s.file_stem(), s.events))
            .collect();
        push(out, format!("slice {z}"), stems.join(" "));
    }
}

/// Every sample export: parsed (Paris, day-first), sliced and written; the RSMF bytes are hashed.
fn samples(out: &mut Vec<String>) {
    let mut files: Vec<PathBuf> = walkdir::WalkDir::new(sample_dir().join("evidence"))
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.into_path())
        .collect();
    files.sort();
    let (exports, _) = sources::plan_exports(&files, None);
    for (zn, order) in [
        ("Europe/Paris", DateOrder::Dmy),
        ("America/New_York", DateOrder::Mdy),
    ] {
        for ex in &exports {
            let label = ex
                .primary
                .strip_prefix(sample_dir())
                .unwrap_or(&ex.primary)
                .to_string_lossy()
                .replace('\\', "/");
            let opts = ParseOptions {
                timezone: zone(zn),
                date_order: Some(order),
                ..ParseOptions::default()
            };
            let r = sources::parse(ex, &opts);
            push(out, format!("sample {label} {zn}"), describe(&r));
            let Ok(p) = r else { continue };
            let tz = zone(zn).unwrap();
            let wo = WriteOptions {
                tz: tz.clone(),
                custodian: None,
                platform: rsmf::platform_for(ex.kind, None),
                generator: "rv-rsmf golden".into(),
                collection_seed: "GOLDEN".into(),
            };
            let d = tempfile::tempdir().unwrap();
            for s in rsmf::slice(&p, &tz) {
                let (w, bytes) = rsmf::write_slice(&p, &s, &wo, d.path()).unwrap();
                push(
                    out,
                    format!(
                        "sample {label} {zn} slice {} {}",
                        s.conversation,
                        s.file_stem()
                    ),
                    format!(
                        "{} events, {:?}..{:?}, sha256 {}",
                        w.events,
                        w.begin,
                        w.end,
                        hex(&Sha256::digest(&bytes))
                    ),
                );
            }
        }
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn all_cases() -> Vec<String> {
    let mut out = vec![];
    zone_names(&mut out);
    zones_local_and_utc(&mut out);
    instants_and_order(&mut out);
    whatsapp(&mut out);
    telegram_readers(&mut out);
    teams_times(&mut out);
    csv_first_look(&mut out);
    csv_export(&mut out);
    validator(&mut out);
    slicing(&mut out);
    samples(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn golden_dates_and_times() {
        let got = all_cases();
        let want: Vec<&str> = include_str!("expected.txt").lines().collect();
        let mut diffs = 0;
        for (i, (g, w)) in got.iter().zip(&want).enumerate() {
            if g != w {
                diffs += 1;
                if diffs <= 20 {
                    eprintln!("line {}:\n  want {w}\n  got  {g}", i + 1);
                }
            }
        }
        assert_eq!(got.len(), want.len(), "number of golden lines");
        assert_eq!(diffs, 0, "{diffs} golden lines differ");
    }

    /// Prints the golden lines (see the module documentation).
    #[test]
    #[ignore]
    fn print() {
        for l in all_cases() {
            println!("{l}");
        }
    }
}
