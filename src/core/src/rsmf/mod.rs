//! RSMF 2.0 output: slicing, manifest, deterministic writing, reading back and validation. FROZEN API:
//! [`slice`], [`write_slice`], [`read_rsmf`], [`validate_file`].
//!
//! An .rsmf file is an RFC 5322 message (CRLF) with a text/plain body (extracted text, events in time order)
//! and exactly one attachment, `rsmf.zip`, holding `rsmf_manifest.json` and every attachment it references.
//! Writing is deterministic — sorted zip entries, fixed zip times, a boundary derived from the content — so
//! the same export and options always give byte-identical files.

pub mod shortcodes;

use crate::analyzer::{Finding, Severity};
use crate::model::{AttachmentData, Event, ParsedExport, SourceKind};
use chrono::{DateTime, NaiveDate, TimeZone, Timelike, Utc};
use chrono_tz::Tz;
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

pub const RSMF_VERSION: &str = "2.0.0";
/// Relativity's limit per file.
pub const MAX_EVENTS: usize = 10_000;
pub const SCHEMA: &str = include_str!("../../../../schemas/rsmf_schema_2_0_0.json");
const MANIFEST: &str = "rsmf_manifest.json";

/// One output file: a conversation's events for one calendar day (in the examiner's zone), or part of a day.
#[derive(Clone, Debug, PartialEq)]
pub struct Slice {
    pub conversation: String,
    /// None = events without any timestamp ("undated" file).
    pub day: Option<NaiveDate>,
    /// 0 = whole day; 1.. = sub-day part when the day exceeds MAX_EVENTS.
    pub part: u32,
    /// Hours per part when split (24 = not split).
    pub hours: u32,
    /// Indexes into ParsedExport::events, in time order.
    pub events: Vec<usize>,
}

impl Slice {
    pub fn file_stem(&self) -> String {
        let day = self
            .day
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "undated".into());
        if self.part == 0 {
            day
        } else {
            format!("{day}-part{:02}", self.part)
        }
    }
}

/// Splits every conversation into one slice per calendar day in `tz`; a day with more than MAX_EVENTS events
/// is split into 12-hour, then 8-hour windows, then into consecutive chunks of MAX_EVENTS.
pub fn slice(p: &ParsedExport, tz: Tz) -> Vec<Slice> {
    let mut by_day: BTreeMap<(String, Option<NaiveDate>), Vec<usize>> = BTreeMap::new();
    for (i, e) in p.events.iter().enumerate() {
        let day = e.timestamp.map(|t| t.with_timezone(&tz).date_naive());
        by_day
            .entry((e.conversation.clone(), day))
            .or_default()
            .push(i);
    }
    let mut out = vec![];
    for ((conv, day), mut idx) in by_day {
        idx.sort_by(|a, b| {
            let (ea, eb) = (&p.events[*a], &p.events[*b]);
            (ea.timestamp, *a).cmp(&(eb.timestamp, *b))
        });
        if idx.len() <= MAX_EVENTS || day.is_none() {
            for (n, chunk) in idx.chunks(MAX_EVENTS).enumerate() {
                out.push(Slice {
                    conversation: conv.clone(),
                    day,
                    part: if idx.len() <= MAX_EVENTS {
                        0
                    } else {
                        n as u32 + 1
                    },
                    hours: 24,
                    events: chunk.to_vec(),
                });
            }
            continue;
        }
        let window = |hours: u32| -> Vec<Vec<usize>> {
            let mut w: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
            for &i in &idx {
                let h = p.events[i]
                    .timestamp
                    .map(|t| t.with_timezone(&tz).hour())
                    .unwrap_or(0);
                w.entry(h / hours).or_default().push(i);
            }
            w.into_values().collect()
        };
        let (hours, parts) = [12u32, 8]
            .into_iter()
            .map(|h| (h, window(h)))
            .find(|(_, w)| w.iter().all(|x| x.len() <= MAX_EVENTS))
            .unwrap_or_else(|| (0, idx.chunks(MAX_EVENTS).map(|c| c.to_vec()).collect()));
        for (n, events) in parts.into_iter().enumerate() {
            out.push(Slice {
                conversation: conv.clone(),
                day,
                part: n as u32 + 1,
                hours,
                events,
            });
        }
    }
    out
}

/// Options that shape the written files (all recorded in the audit log by the caller).
#[derive(Clone, Debug, PartialEq)]
pub struct WriteOptions {
    /// Zone used for slicing (and recorded as custom "source_timezone" when the source had no offset).
    pub tz: Tz,
    pub custodian: Option<String>,
    pub platform: String,
    /// "rv-rsmf X.Y.Z" → X-RSMF-Generator.
    pub generator: String,
    /// Seed for the event collection id: e.g. case id + item SHA-256. Same seed → same id.
    pub collection_seed: String,
}

/// One packed attachment.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct PackedAttachment {
    pub event: String,
    pub zip_name: String,
    pub display: String,
    pub size: u64,
    pub sha256: String,
}

/// What was written for one slice.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct WrittenSlice {
    pub path: PathBuf,
    pub conversation: String,
    pub day: Option<String>,
    pub part: u32,
    pub events: usize,
    pub attachments: Vec<PackedAttachment>,
    pub missing_attachments: usize,
    pub unsupported_reactions: BTreeSet<String>,
    pub event_collection_id: String,
    pub begin: Option<String>,
    pub end: Option<String>,
}

pub fn rsmf_time(t: &DateTime<Utc>) -> String {
    t.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

/// A GUID-shaped id from a seed (SHA-256 based, version nibble 5 style), deterministic.
pub fn collection_id(seed: &str, conversation: &str) -> String {
    let h = crate::hash::hash_bytes(
        format!("{seed}\u{1f}{conversation}").as_bytes(),
        crate::hash::HashAlgo::Sha256,
    );
    format!(
        "{}-{}-5{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[13..16],
        &h[16..20],
        &h[20..32]
    )
}

/// Folder-safe name for a conversation (display name, then id).
pub fn slug(display: &str, id: &str) -> String {
    let mut s: String = display
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    s = s.chars().take(48).collect();
    if s.is_empty() {
        id.to_string()
    } else {
        format!("{s}-{}", &id[id.len().saturating_sub(6)..])
    }
}

fn read_attachment(data: &AttachmentData) -> Result<Option<Vec<u8>>, String> {
    match data {
        AttachmentData::File { path } => std::fs::read(path)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
        AttachmentData::ZipMember { zip, name } => {
            let f = std::fs::File::open(zip).map_err(|e| format!("{}: {e}", zip.display()))?;
            let mut z = zip::ZipArchive::new(f).map_err(|e| format!("{}: {e}", zip.display()))?;
            let mut m = z
                .by_name(name)
                .map_err(|e| format!("{}!{name}: {e}", zip.display()))?;
            let mut v = vec![];
            m.read_to_end(&mut v).map_err(|e| e.to_string())?;
            Ok(Some(v))
        }
        AttachmentData::Inline { bytes } => Ok(Some(bytes.clone())),
        AttachmentData::Missing { .. } => Ok(None),
    }
}

fn attachment_zip_name(event: &Event, n: usize, display: &str) -> String {
    let safe: String = display
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || ".-_".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let safe: String = safe
        .chars()
        .rev()
        .take(80)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{}-{n}-{safe}", event.id)
}

/// Builds the manifest and the zip entries for one slice.
#[allow(clippy::type_complexity)]
pub fn build_slice(
    p: &ParsedExport,
    s: &Slice,
    o: &WriteOptions,
) -> Result<(Value, BTreeMap<String, Vec<u8>>, WrittenSlice), String> {
    let conv = p
        .conversations
        .iter()
        .find(|c| c.id == s.conversation)
        .ok_or_else(|| format!("unknown conversation {}", s.conversation))?;
    let events: Vec<&Event> = s.events.iter().map(|&i| &p.events[i]).collect();
    // Participants: the conversation's members plus anyone referenced by an event in the slice.
    let mut pids: BTreeSet<String> = conv.participants.iter().cloned().collect();
    for e in &events {
        pids.extend(e.participant.iter().cloned());
        for r in &e.reactions {
            pids.extend(r.participants.iter().cloned());
        }
        for ed in &e.edits {
            pids.insert(ed.participant.clone());
        }
    }
    let custom = |m: &BTreeMap<String, String>| -> Value {
        Value::Array(
            m.iter()
                .filter(|(k, v)| !k.is_empty() && !v.is_empty())
                .map(|(k, v)| json!({"name": k, "value": v}))
                .collect(),
        )
    };
    let mut participants = vec![];
    for id in &pids {
        let mut o = Map::new();
        o.insert("id".into(), json!(id));
        if let Some(pp) = p.participant(id) {
            if !pp.display.is_empty() {
                o.insert("display".into(), json!(pp.display));
            }
            if let Some(e) = &pp.email {
                o.insert("email".into(), json!(e));
            }
            if let Some(a) = &pp.account_id {
                o.insert("account_id".into(), json!(a));
            }
            if !pp.custom.is_empty() {
                o.insert("custom".into(), custom(&pp.custom));
            }
        } else {
            o.insert("display".into(), json!(id));
        }
        participants.push(Value::Object(o));
    }
    let mut conv_custom = conv.custom.clone();
    if p.source.is_some_and(|k| k.needs_timezone()) {
        conv_custom.insert("source_timezone".into(), o.tz.name().to_string());
    }
    let mut c = Map::new();
    c.insert("id".into(), json!(conv.id));
    c.insert("display".into(), json!(conv.display));
    c.insert("platform".into(), json!(o.platform));
    c.insert(
        "type".into(),
        json!(match conv.kind {
            crate::model::ConversationType::Direct => "direct",
            crate::model::ConversationType::Channel => "channel",
        }),
    );
    c.insert(
        "participants".into(),
        json!(
            pids.iter()
                .filter(|id| conv.participants.contains(id))
                .collect::<Vec<_>>()
        ),
    );
    if let Some(cust) = &o.custodian
        && let Some(pp) = p
            .participants
            .iter()
            .find(|pp| &pp.display == cust || pp.account_id.as_deref() == Some(cust.as_str()))
    {
        c.insert("custodian".into(), json!(pp.id));
    }
    if !conv_custom.is_empty() {
        c.insert("custom".into(), custom(&conv_custom));
    }

    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut packed = vec![];
    let mut missing = 0usize;
    let mut unsupported = BTreeSet::new();
    let mut ev_json = vec![];
    for e in &events {
        let mut m = Map::new();
        m.insert("id".into(), json!(e.id));
        m.insert("type".into(), json!(e.kind.name()));
        m.insert("conversation".into(), json!(e.conversation));
        if let Some(pid) = &e.participant {
            m.insert("participant".into(), json!(pid));
        }
        if let Some(t) = &e.timestamp {
            m.insert("timestamp".into(), json!(rsmf_time(t)));
        }
        let mut body = e.body.clone();
        let mut atts = vec![];
        for (n, a) in e.attachments.iter().enumerate() {
            match read_attachment(&a.data)? {
                Some(bytes) => {
                    let name = attachment_zip_name(e, n + 1, &a.display);
                    let sha = crate::hash::hash_bytes(&bytes, crate::hash::HashAlgo::Sha256);
                    atts.push(json!({"id": name, "display": a.display, "size": bytes.len()}));
                    packed.push(PackedAttachment {
                        event: e.id.clone(),
                        zip_name: name.clone(),
                        display: a.display.clone(),
                        size: bytes.len() as u64,
                        sha256: sha,
                    });
                    files.insert(name, bytes);
                }
                None => {
                    missing += 1;
                    let reason = match &a.data {
                        AttachmentData::Missing { reason } => reason.as_str(),
                        _ => "",
                    };
                    if !body.is_empty() {
                        body.push('\n');
                    }
                    body.push_str(&format!(
                        "[attachment not in export: {} — {reason}]",
                        a.display
                    ));
                }
            }
        }
        if !body.is_empty() {
            m.insert("body".into(), json!(body));
        }
        if !atts.is_empty() {
            m.insert("attachments".into(), Value::Array(atts));
        }
        if let Some(par) = &e.parent {
            m.insert("parent".into(), json!(par));
        }
        if e.deleted {
            m.insert("deleted".into(), json!(true));
        }
        if let Some(d) = e.direction {
            m.insert(
                "direction".into(),
                json!(match d {
                    crate::model::Direction::Incoming => "incoming",
                    crate::model::Direction::Outgoing => "outgoing",
                }),
            );
        }
        if !e.reactions.is_empty() {
            let rs: Vec<Value> = e
                .reactions
                .iter()
                .map(|r| {
                    let (v, ok) = shortcodes::map(&r.value);
                    if !ok {
                        unsupported.insert(r.value.clone());
                    }
                    let mut o = Map::new();
                    o.insert("value".into(), json!(v));
                    if let Some(c) = r.count {
                        o.insert("count".into(), json!(c));
                    }
                    if !r.participants.is_empty() {
                        o.insert("participants".into(), json!(r.participants));
                    }
                    Value::Object(o)
                })
                .collect();
            m.insert("reactions".into(), Value::Array(rs));
        }
        if !e.edits.is_empty() {
            let es: Vec<Value> = e
                .edits
                .iter()
                .map(|ed| {
                    let mut o = Map::new();
                    o.insert("participant".into(), json!(ed.participant));
                    if let Some(t) = &ed.timestamp {
                        o.insert("timestamp".into(), json!(rsmf_time(t)));
                    }
                    if let Some(v) = &ed.previous {
                        o.insert("previous".into(), json!(v));
                    }
                    if let Some(v) = &ed.new {
                        o.insert("new".into(), json!(v));
                    }
                    Value::Object(o)
                })
                .collect();
            m.insert("edits".into(), Value::Array(es));
        }
        let mut cu = e.custom.clone();
        if !e.source_time.is_empty() {
            cu.insert("source_time".into(), e.source_time.clone());
        }
        cu.insert("source_file".into(), e.source.file.clone());
        if e.source.line > 0 {
            cu.insert(
                "source_lines".into(),
                if e.source.line_end > e.source.line {
                    format!("{}-{}", e.source.line, e.source.line_end)
                } else {
                    e.source.line.to_string()
                },
            );
        }
        if let Some(n) = &e.source.native_id {
            cu.insert("source_id".into(), n.clone());
        }
        m.insert("custom".into(), custom(&cu));
        ev_json.push(Value::Object(m));
    }
    let ecid = collection_id(&o.collection_seed, &conv.id);
    let manifest = json!({
        "version": RSMF_VERSION,
        "eventcollectionid": ecid,
        "participants": participants,
        "conversations": [Value::Object(c)],
        "events": ev_json,
    });
    let times: Vec<String> = events
        .iter()
        .filter_map(|e| e.timestamp.as_ref())
        .map(rsmf_time)
        .collect();
    let written = WrittenSlice {
        path: PathBuf::new(),
        conversation: conv.id.clone(),
        day: s.day.map(|d| d.format("%Y-%m-%d").to_string()),
        part: s.part,
        events: events.len(),
        attachments: packed,
        missing_attachments: missing,
        unsupported_reactions: unsupported,
        event_collection_id: ecid,
        begin: times.iter().min().cloned(),
        end: times.iter().max().cloned(),
    };
    Ok((manifest, files, written))
}

/// The extracted text (EML body): events in time order — sender, then body, then reactions — as in
/// Relativity's reference generator, so dtSearch proximity hits follow the conversation.
pub fn body_text(manifest: &Value) -> String {
    let names: BTreeMap<&str, &str> = manifest["participants"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            Some((
                p["id"].as_str()?,
                p["display"].as_str().unwrap_or(p["id"].as_str()?),
            ))
        })
        .collect();
    let mut out = String::new();
    for e in manifest["events"].as_array().into_iter().flatten() {
        if let Some(n) = e["participant"].as_str().and_then(|id| names.get(id)) {
            out.push_str(n);
            out.push_str("\r\n\r\n");
        }
        if let Some(b) = e["body"].as_str() {
            out.push_str(&b.replace("\r\n", "\n").replace('\n', "\r\n"));
            out.push_str("\r\n");
        }
        out.push_str("\r\n");
        for r in e["reactions"].as_array().into_iter().flatten() {
            if let Some(v) = r["value"].as_str() {
                out.push_str(v);
                out.push_str("\r\n\r\n");
            }
        }
        out.push_str("\r\n");
    }
    out
}

/// Deterministic zip: entries sorted by name, fixed time (1980-01-01), deflate.
pub fn zip_bytes(manifest: &Value, files: &BTreeMap<String, Vec<u8>>) -> Result<Vec<u8>, String> {
    let mut all: BTreeMap<String, Vec<u8>> = files.clone();
    let mut mj = serde_json::to_vec_pretty(manifest).map_err(|e| e.to_string())?;
    mj.push(b'\n');
    all.insert(MANIFEST.into(), mj);
    let mut cur = std::io::Cursor::new(Vec::new());
    {
        let mut z = zip::ZipWriter::new(&mut cur);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .compression_level(Some(6))
            .last_modified_time(zip::DateTime::default())
            .unix_permissions(0o644);
        for (name, bytes) in &all {
            z.start_file(name.as_str(), opts)
                .map_err(|e| e.to_string())?;
            z.write_all(bytes).map_err(|e| e.to_string())?;
        }
        z.finish().map_err(|e| e.to_string())?;
    }
    Ok(cur.into_inner())
}

fn b64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::with_capacity(data.len() * 4 / 3 + data.len() / 57 * 2 + 4);
    for (i, c) in data.chunks(3).enumerate() {
        if i > 0 && i % 19 == 0 {
            s.push_str("\r\n"); // 76 characters per line
        }
        let n = (c[0] as u32) << 16
            | (*c.get(1).unwrap_or(&0) as u32) << 8
            | *c.get(2).unwrap_or(&0) as u32;
        s.push(T[(n >> 18) as usize & 63] as char);
        s.push(T[(n >> 12) as usize & 63] as char);
        s.push(if c.len() > 1 {
            T[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        s.push(if c.len() > 2 {
            T[n as usize & 63] as char
        } else {
            '='
        });
    }
    s
}

fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut buf = 0u32;
    let mut bits = 0;
    for ch in s.bytes() {
        let v = match ch {
            b'A'..=b'Z' => ch - b'A',
            b'a'..=b'z' => ch - b'a' + 26,
            b'0'..=b'9' => ch - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' | b'\r' | b'\n' | b' ' | b'\t' => continue,
            _ => return Err(format!("invalid base64 byte {ch:#x}")),
        };
        buf = buf << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    Ok(out)
}

/// RFC 2047 encoded word for non-ASCII header text; quoted string otherwise.
fn header_phrase(s: &str) -> String {
    if s.is_ascii() {
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        format!("=?UTF-8?B?{}?=", b64(s.as_bytes()).replace("\r\n", ""))
    }
}

fn header_text(s: &str) -> String {
    if s.is_ascii() {
        s.to_string()
    } else {
        format!("=?UTF-8?B?{}?=", b64(s.as_bytes()).replace("\r\n", ""))
    }
}

/// Folds a header at ", " so lines stay under 998 characters (RFC 5322).
fn fold(name: &str, value: &str) -> String {
    let mut out = format!("{name}: ");
    let mut line_len = out.len();
    for (i, part) in value.split(", ").enumerate() {
        let piece = if i == 0 {
            part.to_string()
        } else {
            format!(", {part}")
        };
        if line_len + piece.len() > 76 && i > 0 {
            out.push_str(",\r\n ");
            out.push_str(part);
            line_len = 1 + part.len();
        } else {
            out.push_str(&piece);
            line_len += piece.len();
        }
    }
    out.push_str("\r\n");
    out
}

/// The complete .rsmf bytes for one slice.
pub fn eml_bytes(manifest: &Value, zip: &[u8], ws: &WrittenSlice, o: &WriteOptions) -> String {
    let parts = manifest["participants"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mailbox = |p: &Value| -> String {
        let display = p["display"].as_str().or(p["id"].as_str()).unwrap_or("");
        let email = p["email"].as_str().unwrap_or("");
        format!("{} <{}>", header_phrase(display), email)
    };
    let boundary = format!(
        "rsmf-{}",
        &crate::hash::hash_bytes(zip, crate::hash::HashAlgo::Sha256)[..32]
    );
    let mut h = String::new();
    let custodian_id = manifest["conversations"][0]["custodian"].as_str();
    if let Some(cid) = custodian_id
        && let Some(p) = parts.iter().find(|p| p["id"].as_str() == Some(cid))
    {
        h.push_str(&format!("From: {}\r\n", mailbox(p)));
    }
    h.push_str(&fold(
        "To",
        &parts.iter().map(mailbox).collect::<Vec<_>>().join(", "),
    ));
    h.push_str(&format!("X-RSMF-Version: {RSMF_VERSION}\r\n"));
    h.push_str(&format!("X-RSMF-Generator: {}\r\n", o.generator));
    h.push_str(&format!(
        "X-RSMF-Application: {}\r\n",
        header_text(&o.platform)
    ));
    if let Some(c) = &o.custodian {
        h.push_str(&format!("X-RSMF-Custodian: {}\r\n", header_text(c)));
    }
    let names: Vec<String> = parts
        .iter()
        .map(|p| header_text(p["display"].as_str().or(p["id"].as_str()).unwrap_or("")))
        .collect();
    h.push_str(&fold("X-RSMF-Participants", &names.join(", ")));
    if let Some(b) = &ws.begin {
        h.push_str(&format!("X-RSMF-BeginDate: {b}\r\n"));
    }
    if let Some(e) = &ws.end {
        h.push_str(&format!("X-RSMF-EndDate: {e}\r\n"));
    }
    h.push_str(&format!("X-RSMF-EventCount: {}\r\n", ws.events));
    h.push_str(&format!(
        "X-RSMF-AttachmentCount: {}\r\n",
        ws.attachments.len()
    ));
    h.push_str(&format!(
        "X-RSMF-EventCollectionID: {}\r\n",
        ws.event_collection_id
    ));
    h.push_str("MIME-Version: 1.0\r\n");
    h.push_str(&format!(
        "Content-Type: multipart/mixed; boundary=\"{boundary}\"\r\n\r\n"
    ));
    h.push_str(&format!("--{boundary}\r\n"));
    h.push_str(
        "Content-Type: text/plain; charset=utf-8\r\nContent-Transfer-Encoding: base64\r\n\r\n",
    );
    h.push_str(&b64(body_text(manifest).as_bytes()));
    h.push_str(&format!("\r\n--{boundary}\r\n"));
    h.push_str("Content-Type: application/zip; name=\"rsmf.zip\"\r\n");
    h.push_str("Content-Disposition: attachment; filename=\"rsmf.zip\"\r\n");
    h.push_str("Content-Transfer-Encoding: base64\r\n\r\n");
    h.push_str(&b64(zip));
    h.push_str(&format!("\r\n--{boundary}--\r\n"));
    h
}

/// Builds and writes one slice to `<dir>/<conversation-slug>/<stem>.rsmf`.
pub fn write_slice(
    p: &ParsedExport,
    s: &Slice,
    o: &WriteOptions,
    dir: &Path,
) -> Result<(WrittenSlice, Vec<u8>), String> {
    let (manifest, files, mut ws) = build_slice(p, s, o)?;
    let zip = zip_bytes(&manifest, &files)?;
    let eml = eml_bytes(&manifest, &zip, &ws, o);
    let conv = p
        .conversations
        .iter()
        .find(|c| c.id == s.conversation)
        .expect("checked in build_slice");
    let folder = dir.join(slug(&conv.display, &conv.id));
    std::fs::create_dir_all(&folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let path = folder.join(format!("{}.rsmf", s.file_stem()));
    std::fs::write(&path, eml.as_bytes()).map_err(|e| format!("{}: {e}", path.display()))?;
    ws.path = path;
    Ok((ws, eml.into_bytes()))
}

// ---------------------------------------------------------------- reading back and validation

/// An .rsmf file read back from disk.
#[derive(Clone, Debug, Default)]
pub struct RsmfFile {
    /// Header names lower-cased, values unfolded (as written, encoded words not decoded).
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub manifest: Value,
    /// zip entry name → SHA-256.
    pub entries: BTreeMap<String, String>,
}

impl RsmfFile {
    pub fn header(&self, name: &str) -> Option<&str> {
        let n = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == n)
            .map(|(_, v)| v.as_str())
    }
}

fn split_headers(block: &str) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = vec![];
    for line in block.split("\r\n").flat_map(|l| l.split('\n')) {
        if line.starts_with([' ', '\t']) {
            if let Some(last) = out.last_mut() {
                last.1.push(' ');
                last.1.push_str(line.trim());
            }
        } else if let Some((k, v)) = line.split_once(':') {
            out.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    out
}

/// Reads any RSMF file (ours or a third party's): MIME multipart with a base64 `rsmf.zip` attachment.
pub fn read_rsmf(path: &Path) -> Result<RsmfFile, String> {
    let raw = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&raw).to_string();
    let (head, rest) = text
        .split_once("\r\n\r\n")
        .or_else(|| text.split_once("\n\n"))
        .ok_or("no header/body separator")?;
    let headers = split_headers(head);
    let ctype = headers
        .iter()
        .find(|(k, _)| k == "content-type")
        .map(|(_, v)| v.clone())
        .ok_or("no Content-Type header")?;
    let boundary = ctype
        .split(';')
        .find_map(|p| p.trim().strip_prefix("boundary="))
        .map(|b| b.trim_matches('"').to_string())
        .ok_or("not a multipart message (no boundary)")?;
    let mut body = String::new();
    let mut zip_data: Option<Vec<u8>> = None;
    for part in rest.split(&format!("--{boundary}")).skip(1) {
        if part.starts_with("--") {
            break;
        }
        let part = part.trim_start_matches(['\r', '\n']);
        let Some((ph, pb)) = part
            .split_once("\r\n\r\n")
            .or_else(|| part.split_once("\n\n"))
        else {
            continue;
        };
        let phs = split_headers(ph);
        let get = |n: &str| {
            phs.iter()
                .find(|(k, _)| k == n)
                .map(|(_, v)| v.to_ascii_lowercase())
        };
        let b64enc = get("content-transfer-encoding").is_some_and(|v| v.contains("base64"));
        let is_zip = get("content-disposition").is_some_and(|v| v.contains("rsmf.zip"))
            || get("content-type").is_some_and(|v| v.contains("rsmf.zip"));
        let data = if b64enc {
            b64_decode(pb)?
        } else {
            pb.as_bytes().to_vec()
        };
        if is_zip {
            zip_data = Some(data);
        } else if get("content-type").is_none_or(|v| v.starts_with("text/plain")) {
            body = String::from_utf8_lossy(&data).to_string();
        }
    }
    let zip_data = zip_data.ok_or("no rsmf.zip attachment")?;
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(zip_data))
        .map_err(|e| format!("rsmf.zip: {e}"))?;
    let mut entries = BTreeMap::new();
    let mut manifest = Value::Null;
    for i in 0..z.len() {
        let mut f = z.by_index(i).map_err(|e| e.to_string())?;
        let mut v = vec![];
        f.read_to_end(&mut v).map_err(|e| e.to_string())?;
        let name = f.name().map_err(|e| e.to_string())?.to_string();
        if name == MANIFEST {
            manifest = serde_json::from_slice(&v).map_err(|e| format!("{MANIFEST}: {e}"))?;
        }
        entries.insert(
            name,
            crate::hash::hash_bytes(&v, crate::hash::HashAlgo::Sha256),
        );
    }
    if manifest.is_null() {
        return Err(format!("rsmf.zip has no {MANIFEST}"));
    }
    Ok(RsmfFile {
        headers,
        body,
        manifest,
        entries,
    })
}

/// Result of validating one file.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct Validation {
    /// Official schema violations.
    pub schema_errors: Vec<String>,
    /// rv-rsmf's stricter checks (unknown keys, references, counts, UTC times…).
    pub strict_errors: Vec<String>,
    pub events: usize,
    pub attachments: usize,
}

impl Validation {
    pub fn ok(&self) -> bool {
        self.schema_errors.is_empty() && self.strict_errors.is_empty()
    }
}

fn known_keys(kind: &str) -> &'static [&'static str] {
    match kind {
        "root" => &[
            "version",
            "participants",
            "conversations",
            "events",
            "eventcollectionid",
        ],
        "participant" => &["id", "display", "email", "avatar", "account_id", "custom"],
        "conversation" => &[
            "id",
            "display",
            "platform",
            "type",
            "participants",
            "custodian",
            "custom",
            "icon",
        ],
        "event" => &[
            "id",
            "type",
            "conversation",
            "participant",
            "timestamp",
            "body",
            "parent",
            "deleted",
            "importance",
            "direction",
            "attachments",
            "reactions",
            "edits",
            "read_receipts",
            "custom",
        ],
        _ => &[],
    }
}

/// Schema + strict checks of a manifest and its zip entries.
pub fn validate_manifest(m: &Value, entries: &BTreeMap<String, String>) -> Validation {
    let mut v = Validation::default();
    let schema: Value = serde_json::from_str(SCHEMA).expect("bundled schema is valid JSON");
    match jsonschema::draft7::new(&schema) {
        Ok(validator) => {
            for e in validator.iter_errors(m) {
                v.schema_errors
                    .push(format!("{} at {}", e, e.instance_path()));
            }
        }
        Err(e) => v
            .schema_errors
            .push(format!("schema could not be compiled: {e}")),
    }
    let mut strict = |s: String| v.strict_errors.push(s);
    let check_keys = |obj: &Value, kind: &str, at: &str, out: &mut dyn FnMut(String)| {
        if let Some(o) = obj.as_object() {
            for k in o.keys() {
                if !known_keys(kind).contains(&k.as_str()) {
                    out(format!("unknown key '{k}' in {at}"));
                }
            }
        }
    };
    check_keys(m, "root", "manifest", &mut strict);
    let pids: BTreeSet<&str> = m["participants"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["id"].as_str())
        .collect();
    let cids: BTreeSet<&str> = m["conversations"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| c["id"].as_str())
        .collect();
    for (i, p) in m["participants"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        check_keys(p, "participant", &format!("participants[{i}]"), &mut strict);
        if let Some(a) = p["avatar"].as_str()
            && !entries.contains_key(a)
        {
            strict(format!("participants[{i}].avatar '{a}' is not in rsmf.zip"));
        }
    }
    for (i, c) in m["conversations"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
    {
        check_keys(
            c,
            "conversation",
            &format!("conversations[{i}]"),
            &mut strict,
        );
        for p in c["participants"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            if !pids.contains(p) {
                strict(format!("conversations[{i}] lists unknown participant {p}"));
            }
        }
        if let Some(cu) = c["custodian"].as_str()
            && !pids.contains(cu)
        {
            strict(format!(
                "conversations[{i}].custodian '{cu}' is not a participant"
            ));
        }
    }
    let eids: BTreeSet<&str> = m["events"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|e| e["id"].as_str())
        .collect();
    let mut seen = BTreeSet::new();
    for (i, e) in m["events"].as_array().into_iter().flatten().enumerate() {
        v.events += 1;
        let at = format!("events[{i}]");
        check_keys(e, "event", &at, &mut strict);
        if let Some(id) = e["id"].as_str()
            && !seen.insert(id)
        {
            strict(format!("{at}: duplicate event id {id}"));
        }
        if let Some(p) = e["participant"].as_str()
            && !pids.contains(p)
        {
            strict(format!("{at}: unknown participant {p}"));
        }
        if let Some(c) = e["conversation"].as_str()
            && !cids.contains(c)
        {
            strict(format!("{at}: unknown conversation {c}"));
        }
        if let Some(p) = e["parent"].as_str()
            && !eids.contains(p)
        {
            strict(format!("{at}: parent {p} is not an event in this file"));
        }
        if let Some(t) = e["timestamp"].as_str()
            && (!t.ends_with('Z') || DateTime::parse_from_rfc3339(t).is_err())
        {
            strict(format!("{at}: timestamp '{t}' is not RFC 3339 UTC (…Z)"));
        }
        for a in e["attachments"].as_array().into_iter().flatten() {
            v.attachments += 1;
            if let Some(id) = a["id"].as_str()
                && !entries.contains_key(id)
            {
                strict(format!("{at}: attachment '{id}' is not in rsmf.zip"));
            }
        }
        for r in e["reactions"].as_array().into_iter().flatten() {
            if r["value"].as_str().is_some_and(|s| s.chars().count() > 30) {
                strict(format!("{at}: reaction longer than 30 characters"));
            }
            for p in r["participants"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
            {
                if !pids.contains(p) {
                    strict(format!("{at}: reaction by unknown participant {p}"));
                }
            }
        }
    }
    if v.events > MAX_EVENTS {
        v.strict_errors.push(format!(
            "{} events: Relativity's limit is {MAX_EVENTS} per file",
            v.events
        ));
    }
    v
}

/// Full validation of a file on disk: parse, schema, strict checks and header/manifest agreement.
pub fn validate_file(path: &Path) -> Result<(Validation, RsmfFile), String> {
    let f = read_rsmf(path)?;
    let mut v = validate_manifest(&f.manifest, &f.entries);
    let want = |h: &str, n: usize, v: &mut Validation| match f.header(h) {
        Some(x) if x.trim() == n.to_string() => {}
        Some(x) => v
            .strict_errors
            .push(format!("{h}: header says {x}, manifest has {n}")),
        None => {}
    };
    match f.header("X-RSMF-Version") {
        Some(x) if x == f.manifest["version"].as_str().unwrap_or("") => {}
        Some(x) => v.strict_errors.push(format!(
            "X-RSMF-Version {x} differs from the manifest version"
        )),
        None => v
            .strict_errors
            .push("X-RSMF-Version header missing (required)".into()),
    }
    want("X-RSMF-EventCount", v.events, &mut v);
    want("X-RSMF-AttachmentCount", v.attachments, &mut v);
    Ok((v, f))
}

/// Round trip: what was written must be what is read back (events, attachments and their hashes).
pub fn roundtrip(ws: &WrittenSlice) -> Result<Vec<String>, String> {
    let (v, f) = validate_file(&ws.path)?;
    let mut problems = vec![];
    if v.events != ws.events {
        problems.push(format!("events: wrote {}, read {}", ws.events, v.events));
    }
    for a in &ws.attachments {
        match f.entries.get(&a.zip_name) {
            Some(h) if *h == a.sha256 => {}
            Some(_) => problems.push(format!("attachment {} changed in the zip", a.zip_name)),
            None => problems.push(format!("attachment {} missing from the zip", a.zip_name)),
        }
    }
    Ok(problems)
}

/// Findings for one written slice (validation + round trip).
pub fn slice_findings(ws: &WrittenSlice, v: &Validation, rt: &[String]) -> Vec<Finding> {
    let file = ws
        .path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let mut out = vec![];
    if !v.ok() {
        let all: Vec<String> = v
            .schema_errors
            .iter()
            .chain(&v.strict_errors)
            .cloned()
            .collect();
        out.push(
            Finding::new("RSMF_SCHEMA_INVALID", Severity::High)
                .with("file", file.clone())
                .with("errors", crate::report::capped_list(&all, 5)),
        );
    }
    if !rt.is_empty() {
        out.push(
            Finding::new("RSMF_ROUNDTRIP_MISMATCH", Severity::High)
                .with("file", file)
                .with("problems", crate::report::capped_list(rt, 5)),
        );
    }
    out
}

/// Local wall-clock time → UTC in `tz`, reporting DST problems: Ok((utc, None)) normally,
/// Ok((utc, Some("ambiguous"|"nonexistent"))) when the clock change makes the time ambiguous (the earlier
/// instant is used) or impossible (shifted forward by the gap).
pub fn local_to_utc(local: chrono::NaiveDateTime, tz: Tz) -> (DateTime<Utc>, Option<&'static str>) {
    match tz.from_local_datetime(&local) {
        chrono::LocalResult::Single(t) => (t.with_timezone(&Utc), None),
        chrono::LocalResult::Ambiguous(a, _) => (a.with_timezone(&Utc), Some("ambiguous")),
        chrono::LocalResult::None => {
            let shifted = local + chrono::Duration::hours(1);
            let t = tz
                .from_local_datetime(&shifted)
                .earliest()
                .map(|t| t.with_timezone(&Utc))
                .unwrap_or_else(|| Utc.from_utc_datetime(&local));
            (t, Some("nonexistent"))
        }
    }
}

/// Platform shown in RSMF for a source, unless overridden.
pub fn platform_for(kind: SourceKind, overridden: Option<&str>) -> String {
    overridden
        .filter(|s| !s.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| kind.platform().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::*;

    fn sample(n: usize, tz: Tz) -> ParsedExport {
        let a = Participant {
            id: "p-a".into(),
            display: "Ana Ñúñez".into(),
            email: None,
            account_id: Some("+33 6 00 00 00 01".into()),
            custom: BTreeMap::new(),
        };
        let b = Participant {
            id: "p-b".into(),
            display: "Bob".into(),
            email: Some("bob@example.test".into()),
            account_id: None,
            custom: BTreeMap::new(),
        };
        let conv = Conversation {
            id: "c-1".into(),
            display: "Ana & Bob".into(),
            kind: ConversationType::Direct,
            participants: vec!["p-a".into(), "p-b".into()],
            custom: BTreeMap::new(),
        };
        let start = local_to_utc(
            NaiveDate::from_ymd_opt(2026, 3, 2)
                .unwrap()
                .and_hms_opt(8, 0, 0)
                .unwrap(),
            tz,
        )
        .0;
        let events = (0..n)
            .map(|i| {
                let mut e = Event::message(
                    format!("e-{i:06}"),
                    "c-1".into(),
                    SourceLoc {
                        file: "_chat.txt".into(),
                        line: i as u64 + 1,
                        line_end: i as u64 + 1,
                        native_id: None,
                    },
                );
                e.participant = Some(if i % 2 == 0 { "p-a" } else { "p-b" }.into());
                e.timestamp = Some(start + chrono::Duration::seconds(i as i64 * 3));
                e.body = format!("message {i}");
                e
            })
            .collect();
        ParsedExport {
            source: Some(SourceKind::Whatsapp),
            participants: vec![a, b],
            conversations: vec![conv],
            events,
            findings: vec![],
            facts: BTreeMap::new(),
        }
    }

    fn opts(tz: Tz) -> WriteOptions {
        WriteOptions {
            tz,
            custodian: Some("Ana Ñúñez".into()),
            platform: "WhatsApp".into(),
            generator: "rv-rsmf test".into(),
            collection_seed: "seed".into(),
        }
    }

    #[test]
    fn written_file_validates_round_trips_and_is_deterministic() {
        let tz: Tz = "Europe/Paris".parse().unwrap();
        let mut p = sample(5, tz);
        p.events[1].reactions.push(Reaction {
            value: "👍".into(),
            count: Some(1),
            participants: vec!["p-a".into()],
        });
        p.events[2].attachments.push(Attachment {
            display: "IMG-0001.jpg".into(),
            data: AttachmentData::Inline {
                bytes: b"\xFF\xD8\xFFfake".to_vec(),
            },
        });
        p.events[3].attachments.push(Attachment {
            display: "VID-0002.mp4".into(),
            data: AttachmentData::Missing {
                reason: "<Media omitted>".into(),
            },
        });
        let slices = slice(&p, tz);
        assert_eq!(slices.len(), 1);
        let d1 = tempfile::tempdir().unwrap();
        let d2 = tempfile::tempdir().unwrap();
        let (w1, b1) = write_slice(&p, &slices[0], &opts(tz), d1.path()).unwrap();
        let (_, b2) = write_slice(&p, &slices[0], &opts(tz), d2.path()).unwrap();
        assert_eq!(b1, b2, "same input → byte-identical .rsmf");
        let (v, f) = validate_file(&w1.path).unwrap();
        assert!(v.ok(), "{v:?}");
        assert_eq!(v.events, 5);
        assert_eq!(v.attachments, 1);
        assert_eq!(w1.missing_attachments, 1);
        assert!(roundtrip(&w1).unwrap().is_empty());
        assert_eq!(f.header("X-RSMF-Version"), Some("2.0.0"));
        assert!(f.header("From").unwrap().contains("=?UTF-8?B?"));
        assert!(f.body.contains("message 4"));
        assert!(f.body.contains(":+1:"));
        assert_eq!(f.manifest["conversations"][0]["custodian"], "p-a");
        assert!(w1.path.ends_with("2026-03-02.rsmf"));
    }

    #[test]
    fn dense_days_are_split_and_share_the_collection_id() {
        let tz: Tz = "UTC".parse().unwrap();
        let p = sample(MAX_EVENTS + 500, tz);
        let s = slice(&p, tz);
        assert!(s.len() >= 2);
        assert!(
            s.iter()
                .all(|x| x.events.len() <= MAX_EVENTS && x.part >= 1)
        );
        assert_eq!(
            s.iter().map(|x| x.events.len()).sum::<usize>(),
            MAX_EVENTS + 500
        );
        assert_eq!(collection_id("k", "c-1"), collection_id("k", "c-1"));
        assert_ne!(collection_id("k", "c-1"), collection_id("k", "c-2"));
    }

    #[test]
    fn strict_checks_catch_what_the_schema_lets_through() {
        let m = json!({"version": "2.0.0", "participants": [{"id": "p1"}], "conversations": [{"id": "c1", "platform": "x", "participants": ["p1", "p9"]}],
            "events": [{"type": "message", "participant": "p2", "timestamp": "2026-01-01T10:00:00+02:00", "attachmnts": [], "attachments": [{"id": "a.jpg"}]}]});
        let v = validate_manifest(&m, &BTreeMap::new());
        assert!(
            v.schema_errors.is_empty(),
            "the official schema accepts it: {:?}",
            v.schema_errors
        );
        let all = v.strict_errors.join("\n");
        for needle in [
            "unknown key 'attachmnts'",
            "unknown participant p9",
            "unknown participant p2",
            "not RFC 3339 UTC",
            "a.jpg' is not in rsmf.zip",
        ] {
            assert!(all.contains(needle), "{needle} missing from:\n{all}");
        }
    }

    #[test]
    fn dst_gap_and_overlap_are_reported() {
        let tz: Tz = "Europe/Paris".parse().unwrap();
        let gap = NaiveDate::from_ymd_opt(2026, 3, 29)
            .unwrap()
            .and_hms_opt(2, 30, 0)
            .unwrap();
        let overlap = NaiveDate::from_ymd_opt(2026, 10, 25)
            .unwrap()
            .and_hms_opt(2, 30, 0)
            .unwrap();
        assert_eq!(local_to_utc(gap, tz).1, Some("nonexistent"));
        assert_eq!(local_to_utc(overlap, tz).1, Some("ambiguous"));
        assert_eq!(
            rsmf_time(&local_to_utc(overlap, tz).0),
            "2026-10-25T00:30:00Z"
        );
    }
}
