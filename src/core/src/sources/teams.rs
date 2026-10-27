//! Microsoft Teams messages exported as one Outlook `.msg` file per message (Microsoft Purview eDiscovery).
//!
//! A `.msg` is an OLE compound file ([MS-OXMSG]). Every MAPI property of the message is either a stream
//! `__substg1.0_IIIITTTT` (variable-size values: `IIII` = property id, `TTTT` = type, 001F = UTF-16LE string,
//! 001E = 8-bit string, 0102 = binary) or a 16-byte record in the `__properties_version1.0` stream
//! (fixed-size values such as PT_SYSTIME FILETIMEs). Attachments live in `__attach_version1.0_#XXXXXXXX`
//! storages and recipients in `__recip_version1.0_#XXXXXXXX` storages, each with their own property stream.
//!
//! ```text
//! folder of .msg ─► one file at a time (corrupt / non-Teams class → UNPARSED_LINES, keep going)
//!                    ├─ class IPM.SkypeTeams.Message        → message event
//!                    └─ class IPM.SkypeTeams.SystemMessage  → history event
//!               ─► group by PR_CONVERSATION_TOPIC (→ subject → "Teams conversation"), > 2 people = channel
//! ```
//!
//! UNVERIFIED: the property choices below follow Microsoft's MAPI property reference ([MS-OXPROPS],
//! [MS-OXOMSG], [MS-OXCMSG]) — they are the standard properties of any message item. Which of them a real
//! Purview Teams export actually fills (and with what: e.g. whether PR_CONVERSATION_TOPIC holds the chat or
//! channel name, whether replies carry PR_IN_REPLY_TO_ID) must be confirmed on real Purview exports before
//! this parser's output is relied on in court. Times are FILETIMEs (UTC by definition), so this source never
//! needs `--timezone`.

use super::{ExportInput, ParseError, ParseOptions};
use crate::analyzer::{Finding, Severity};
use crate::model::{
    Attachment, AttachmentData, Conversation, ConversationType, Event, EventType, ParsedExport,
    Participant, SourceLoc, stable_id,
};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{Cursor, Read};
use std::path::Path;

// MAPI property ids (MS-OXPROPS).
pub const PR_MESSAGE_CLASS: u16 = 0x001A;
pub const PR_SUBJECT: u16 = 0x0037;
pub const PR_CLIENT_SUBMIT_TIME: u16 = 0x0039;
pub const PR_CONVERSATION_TOPIC: u16 = 0x0070;
pub const PR_SENDER_NAME: u16 = 0x0C1A;
pub const PR_SENDER_EMAIL_ADDRESS: u16 = 0x0C1F;
pub const PR_DISPLAY_TO: u16 = 0x0E04;
pub const PR_MESSAGE_DELIVERY_TIME: u16 = 0x0E06;
pub const PR_BODY: u16 = 0x1000;
pub const PR_HTML: u16 = 0x1013;
pub const PR_INTERNET_MESSAGE_ID: u16 = 0x1035;
pub const PR_IN_REPLY_TO_ID: u16 = 0x1042;
pub const PR_DISPLAY_NAME: u16 = 0x3001;
pub const PR_EMAIL_ADDRESS: u16 = 0x3003;
pub const PR_ATTACH_DATA_BIN: u16 = 0x3701;
pub const PR_ATTACH_FILENAME: u16 = 0x3704;
pub const PR_ATTACH_METHOD: u16 = 0x3705;
pub const PR_ATTACH_LONG_FILENAME: u16 = 0x3707;
pub const PR_SMTP_ADDRESS: u16 = 0x39FE;
pub const PR_INTERNET_CPID: u16 = 0x3FDE;
pub const PR_SENDER_SMTP_ADDRESS: u16 = 0x5D01;

// MAPI property types.
pub const PT_LONG: u16 = 0x0003;
pub const PT_BOOLEAN: u16 = 0x000B;
pub const PT_STRING8: u16 = 0x001E;
pub const PT_UNICODE: u16 = 0x001F;
pub const PT_SYSTIME: u16 = 0x0040;
pub const PT_BINARY: u16 = 0x0102;

/// Header size of `__properties_version1.0` (MS-OXMSG 2.4.1): top-level message 32, attachment/recipient 8.
pub const PROPS_HEADER_MESSAGE: usize = 32;
pub const PROPS_HEADER_SUB: usize = 8;

const TEAMS_MESSAGE: &str = "IPM.SkypeTeams.Message";
const TEAMS_SYSTEM: &str = "IPM.SkypeTeams.SystemMessage";

/// The properties of one storage (message, attachment or recipient).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Props {
    /// Strings by property id (001F decoded from UTF-16LE, 001E from windows-1252).
    pub strings: HashMap<u16, String>,
    pub binaries: HashMap<u16, Vec<u8>>,
    /// Fixed-size values from `__properties_version1.0`: id → (type, raw 8 bytes as u64 LE).
    pub fixed: HashMap<u16, (u16, u64)>,
}

impl Props {
    pub fn string(&self, id: u16) -> Option<&str> {
        self.strings
            .get(&id)
            .map(|s| s.as_str())
            .filter(|s| !s.trim().is_empty())
    }
    pub fn long(&self, id: u16) -> Option<i32> {
        match self.fixed.get(&id) {
            Some((PT_LONG, v)) => Some(*v as u32 as i32),
            _ => None,
        }
    }
    pub fn time(&self, id: u16) -> Option<(u64, DateTime<Utc>)> {
        match self.fixed.get(&id) {
            Some((PT_SYSTIME, v)) => filetime_to_utc(*v).map(|t| (*v, t)),
            _ => None,
        }
    }
}

/// `__substg1.0_0037001F` → (0x0037, 0x001F). Multi-valued (0x1000 flag) and malformed names → None.
pub fn parse_stream_name(name: &str) -> Option<(u16, u16)> {
    let hex = name.strip_prefix("__substg1.0_")?;
    if hex.len() != 8 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let id = u16::from_str_radix(&hex[..4], 16).ok()?;
    let ty = u16::from_str_radix(&hex[4..], 16).ok()?;
    (ty & 0x1000 == 0).then_some((id, ty))
}

/// Decodes the fixed-size records of a `__properties_version1.0` stream (16 bytes each after the header).
pub fn parse_fixed_props(stream: &[u8], header: usize) -> HashMap<u16, (u16, u64)> {
    let mut out = HashMap::new();
    let Some(body) = stream.get(header..) else {
        return out;
    };
    for rec in body.chunks_exact(16) {
        let tag = u32::from_le_bytes(rec[0..4].try_into().expect("4 bytes"));
        let (id, ty) = ((tag >> 16) as u16, (tag & 0xFFFF) as u16);
        let value = u64::from_le_bytes(rec[8..16].try_into().expect("8 bytes"));
        if matches!(
            ty,
            PT_LONG | PT_BOOLEAN | PT_SYSTIME | 0x0002 | 0x0005 | 0x0014
        ) {
            out.insert(id, (ty, value));
        }
    }
    out
}

/// FILETIME (100-ns intervals since 1601-01-01 UTC) → UTC. 0 and out-of-range values → None.
pub fn filetime_to_utc(ft: u64) -> Option<DateTime<Utc>> {
    if ft == 0 || ft == u64::MAX {
        return None;
    }
    const EPOCH_DIFF_SECS: i64 = 11_644_473_600;
    let secs = (ft / 10_000_000) as i64 - EPOCH_DIFF_SECS;
    let nanos = ((ft % 10_000_000) * 100) as u32;
    DateTime::from_timestamp(secs, nanos)
}

/// UTC → FILETIME (used by tests and the sample generator).
pub fn utc_to_filetime(t: DateTime<Utc>) -> u64 {
    ((t.timestamp() + 11_644_473_600) as u64) * 10_000_000
        + (t.timestamp_subsec_nanos() / 100) as u64
}

/// UTF-16LE without the trailing NULs MAPI writers often keep.
pub fn utf16le(bytes: &[u8]) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|c| u16::from_le_bytes([c[0], c[1]]))
        .collect();
    String::from_utf16_lossy(&units)
        .trim_end_matches('\0')
        .to_string()
}

/// Turns an HTML body into the text a reviewer sees: tags removed, block ends and `<br>` as new lines,
/// entities decoded, `<script>`/`<style>`/`<head>` contents dropped.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut rest = html;
    while let Some(lt) = rest.find('<') {
        out.push_str(&decode_entities(&rest[..lt]));
        let after = &rest[lt + 1..];
        if let Some(c) = after.strip_prefix("!--") {
            rest = c.find("-->").map(|i| &c[i + 3..]).unwrap_or("");
            continue;
        }
        let Some(gt) = after.find('>') else {
            rest = "";
            break;
        };
        let tag = after[..gt].trim();
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        let closing = tag.starts_with('/');
        rest = &after[gt + 1..];
        if !closing && matches!(name.as_str(), "script" | "style" | "head" | "title") {
            let end = format!("</{name}");
            let lower = rest.to_ascii_lowercase();
            rest = match lower.find(&end) {
                Some(i) => rest[i..]
                    .find('>')
                    .map(|j| &rest[i + j + 1..])
                    .unwrap_or(""),
                None => "",
            };
            continue;
        }
        match name.as_str() {
            "br" => out.push('\n'),
            "p" | "div" | "li" | "tr" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "blockquote"
            | "pre" | "table" | "ul" | "ol"
                if closing || name == "li" =>
            {
                if !out.ends_with('\n') && !out.is_empty() {
                    out.push('\n');
                }
            }
            "td" | "th" if closing => out.push('\t'),
            _ => {}
        }
    }
    out.push_str(&decode_entities(rest));
    // Normalise: trim each line's trailing spaces, at most one blank line in a row.
    let mut lines: Vec<&str> = vec![];
    for l in out.lines().map(|l| l.trim_end()) {
        if l.trim().is_empty() && lines.last().is_none_or(|p| p.trim().is_empty()) {
            continue;
        }
        lines.push(l);
    }
    lines.join("\n").trim().to_string()
}

fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp + 1..];
        let semi = after.find(';').filter(|&i| i <= 10);
        let decoded = semi.and_then(|i| {
            let ent = &after[..i];
            let c = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                    u32::from_str_radix(&ent[2..], 16)
                        .ok()
                        .and_then(char::from_u32)
                }
                _ if ent.starts_with('#') => ent[1..].parse().ok().and_then(char::from_u32),
                _ => None,
            };
            c.map(|c| (c, i))
        });
        match decoded {
            Some((c, i)) => {
                out.push(c);
                rest = &after[i + 1..];
            }
            None => {
                out.push('&');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Reads every property of one storage (`""` = the message root).
fn read_props<F: Read + std::io::Seek>(
    cf: &mut cfb::CompoundFile<F>,
    storage: &str,
    header: usize,
) -> std::io::Result<Props> {
    let base = if storage.is_empty() {
        "/".to_string()
    } else {
        format!("/{storage}")
    };
    let entries: Vec<(String, String)> = cf
        .read_storage(&base)?
        .filter(|e| e.is_stream())
        .map(|e| (e.name().to_string(), e.path().to_string_lossy().to_string()))
        .collect();
    let mut p = Props::default();
    for (name, path) in entries {
        let mut buf = vec![];
        if name == "__properties_version1.0" {
            cf.open_stream(&path)?.read_to_end(&mut buf)?;
            p.fixed = parse_fixed_props(&buf, header);
            continue;
        }
        let Some((id, ty)) = parse_stream_name(&name) else {
            continue;
        };
        if !matches!(ty, PT_UNICODE | PT_STRING8 | PT_BINARY) {
            continue;
        }
        cf.open_stream(&path)?.read_to_end(&mut buf)?;
        match ty {
            PT_UNICODE => {
                p.strings.insert(id, utf16le(&buf));
            }
            PT_STRING8 => {
                // 8-bit strings are in the message code page; windows-1252 is the Western default.
                let (s, _, _) = encoding_rs::WINDOWS_1252.decode(&buf);
                p.strings.insert(id, s.trim_end_matches('\0').to_string());
            }
            _ => {
                p.binaries.insert(id, buf);
            }
        }
    }
    Ok(p)
}

fn sub_storages<F: Read + std::io::Seek>(
    cf: &cfb::CompoundFile<F>,
    prefix: &str,
) -> std::io::Result<Vec<String>> {
    let mut v: Vec<String> = cf
        .read_storage("/")?
        .filter(|e| e.is_storage() && e.name().starts_with(prefix))
        .map(|e| e.name().to_string())
        .collect();
    v.sort();
    Ok(v)
}

/// What one .msg yields before participants and conversations are resolved across the export.
#[derive(Clone, Debug, Default)]
struct RawMsg {
    file: String,
    class: String,
    topic: Option<(String, &'static str)>,
    subject: Option<String>,
    sender: Option<(String, Option<String>)>,
    recipients: Vec<(String, Option<String>)>,
    body: String,
    body_source: &'static str,
    time: Option<(u64, DateTime<Utc>, &'static str)>,
    message_id: Option<String>,
    in_reply_to: Option<String>,
    attachments: Vec<Attachment>,
}

fn is_email(s: &str) -> bool {
    s.contains('@') && !s.starts_with("/O=") && !s.starts_with("/o=")
}

fn read_msg(bytes: &[u8], file: &str) -> Result<RawMsg, String> {
    let mut cf = cfb::CompoundFile::open(Cursor::new(bytes))
        .map_err(|e| format!("not a readable compound file ({e})"))?;
    if !cf.is_stream("/__properties_version1.0") {
        return Err("no __properties_version1.0 stream (not an Outlook .msg)".into());
    }
    let p = read_props(&mut cf, "", PROPS_HEADER_MESSAGE).map_err(|e| e.to_string())?;
    let class = p
        .string(PR_MESSAGE_CLASS)
        .ok_or("no PR_MESSAGE_CLASS")?
        .trim()
        .to_string();
    let mut m = RawMsg {
        file: file.to_string(),
        class,
        ..Default::default()
    };
    m.subject = p.string(PR_SUBJECT).map(|s| s.trim().to_string());
    m.topic = p
        .string(PR_CONVERSATION_TOPIC)
        .map(|s| (s.trim().to_string(), "PR_CONVERSATION_TOPIC"))
        .or_else(|| m.subject.clone().map(|s| (s, "PR_SUBJECT")));
    let email = p
        .string(PR_SENDER_SMTP_ADDRESS)
        .filter(|s| is_email(s))
        .or_else(|| p.string(PR_SENDER_EMAIL_ADDRESS).filter(|s| is_email(s)))
        .map(|s| s.trim().to_string());
    m.sender = match (p.string(PR_SENDER_NAME), email) {
        (Some(n), e) => Some((n.trim().to_string(), e)),
        (None, Some(e)) => Some((e.clone(), Some(e))),
        (None, None) => None,
    };
    for r in sub_storages(&cf, "__recip_version1.0_#").map_err(|e| e.to_string())? {
        let rp = read_props(&mut cf, &r, PROPS_HEADER_SUB).map_err(|e| e.to_string())?;
        let email = rp
            .string(PR_SMTP_ADDRESS)
            .filter(|s| is_email(s))
            .or_else(|| rp.string(PR_EMAIL_ADDRESS).filter(|s| is_email(s)))
            .map(|s| s.trim().to_string());
        if let Some(n) = rp.string(PR_DISPLAY_NAME).map(|s| s.trim().to_string()) {
            m.recipients.push((n, email));
        } else if let Some(e) = email {
            m.recipients.push((e.clone(), Some(e)));
        }
    }
    for n in p
        .string(PR_DISPLAY_TO)
        .unwrap_or("")
        .split(';')
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        if !m.recipients.iter().any(|(rn, _)| rn == n) {
            m.recipients.push((n.to_string(), None));
        }
    }
    if let Some(b) = p.string(PR_BODY) {
        m.body = b.replace("\r\n", "\n").trim_end().to_string();
        m.body_source = "PR_BODY";
    } else if let Some(h) = p.string(PR_HTML) {
        m.body = html_to_text(h);
        m.body_source = "PR_HTML (tags removed)";
    } else if let Some(h) = p.binaries.get(&PR_HTML) {
        let enc = match p.long(PR_INTERNET_CPID) {
            Some(1252) | Some(28591) | Some(20127) => encoding_rs::WINDOWS_1252,
            Some(1200) => encoding_rs::UTF_16LE,
            _ => encoding_rs::UTF_8,
        };
        let (s, _, _) = enc.decode(h);
        m.body = html_to_text(&s);
        m.body_source = "PR_HTML (tags removed)";
    }
    m.time = p
        .time(PR_CLIENT_SUBMIT_TIME)
        .map(|(raw, t)| (raw, t, "PR_CLIENT_SUBMIT_TIME"))
        .or_else(|| {
            p.time(PR_MESSAGE_DELIVERY_TIME)
                .map(|(raw, t)| (raw, t, "PR_MESSAGE_DELIVERY_TIME"))
        });
    m.message_id = p
        .string(PR_INTERNET_MESSAGE_ID)
        .map(|s| s.trim().to_string());
    m.in_reply_to = p.string(PR_IN_REPLY_TO_ID).map(|s| s.trim().to_string());
    for (n, a) in sub_storages(&cf, "__attach_version1.0_#")
        .map_err(|e| e.to_string())?
        .into_iter()
        .enumerate()
    {
        let ap = read_props(&mut cf, &a, PROPS_HEADER_SUB).map_err(|e| e.to_string())?;
        let display = ap
            .string(PR_ATTACH_LONG_FILENAME)
            .or_else(|| ap.string(PR_ATTACH_FILENAME))
            .or_else(|| ap.string(PR_DISPLAY_NAME))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|| format!("attachment-{}", n + 1));
        let data = match ap.binaries.get(&PR_ATTACH_DATA_BIN) {
            Some(b) => AttachmentData::Inline { bytes: b.clone() },
            None => AttachmentData::Missing {
                reason: match ap.long(PR_ATTACH_METHOD) {
                    Some(5) => "embedded message (not extracted)".into(),
                    Some(2..=4) | Some(7) => "linked file: the .msg holds only a reference".into(),
                    _ => "no PR_ATTACH_DATA_BIN in the .msg".into(),
                },
            },
        };
        m.attachments.push(Attachment { display, data });
    }
    Ok(m)
}

/// Parses every `.msg` member of the export (sorted). See the module documentation.
pub fn parse(export: &ExportInput, _opts: &ParseOptions) -> Result<ParsedExport, ParseError> {
    let mut files: Vec<&std::path::PathBuf> = export
        .members
        .iter()
        .filter(|f| {
            f.extension()
                .is_some_and(|e| e.to_string_lossy().eq_ignore_ascii_case("msg"))
        })
        .collect();
    files.sort();
    let rel = |f: &Path| -> String {
        f.strip_prefix(&export.root)
            .unwrap_or(f)
            .to_string_lossy()
            .replace('\\', "/")
    };
    let mut msgs = vec![];
    let mut corrupt = 0usize;
    let mut skipped: Vec<String> = vec![];
    for f in &files {
        let name = rel(f);
        let bytes = match export.read(f) {
            Ok(b) => b,
            Err(e) => {
                corrupt += 1;
                skipped.push(format!("{name}: {e}"));
                continue;
            }
        };
        // A malformed compound file must not stop the other messages: contain any panic in the reader.
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| read_msg(&bytes, &name)))
            .unwrap_or_else(|_| Err("the compound-file reader crashed on this file".into()));
        match r {
            Ok(m) if m.class.starts_with(TEAMS_MESSAGE) || m.class.starts_with(TEAMS_SYSTEM) => {
                msgs.push(m)
            }
            Ok(m) => skipped.push(format!("{name}: message class {} is not Teams", m.class)),
            Err(e) => {
                corrupt += 1;
                skipped.push(format!("{name}: {e}"));
            }
        }
    }
    if msgs.is_empty() {
        return Err(ParseError::Invalid(if files.is_empty() {
            "no .msg file in the Teams export".into()
        } else if corrupt == files.len() {
            format!(
                "no readable .msg file: {}",
                crate::report::capped_list(&skipped, 3)
            )
        } else {
            format!(
                "no Teams message (IPM.SkypeTeams.*) among the .msg files: {}",
                crate::report::capped_list(&skipped, 3)
            )
        }));
    }

    // Participants: one per e-mail address when known, else per display name. A name seen once with an
    // address (sender or recipient table) is linked to it everywhere it appears without one.
    let mut name_email: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for m in &msgs {
        for (n, e) in m.sender.iter().chain(&m.recipients) {
            if let Some(e) = e {
                name_email
                    .entry(n.clone())
                    .or_default()
                    .insert(e.to_ascii_lowercase());
            }
        }
    }
    let mut out = ParsedExport::default();
    let mut pindex: HashMap<String, usize> = HashMap::new();
    let mut person = |out: &mut ParsedExport, name: &str, email: &Option<String>| -> String {
        let email = email.clone().or_else(|| {
            name_email
                .get(name)
                .filter(|s| s.len() == 1)
                .and_then(|s| s.iter().next().cloned())
        });
        let key = match &email {
            Some(e) => format!("email:{}", e.to_ascii_lowercase()),
            None => format!("name:{name}"),
        };
        if let Some(&i) = pindex.get(&key) {
            return out.participants[i].id.clone();
        }
        let id = stable_id("p", &key);
        pindex.insert(key, out.participants.len());
        out.participants.push(Participant {
            id: id.clone(),
            display: name.to_string(),
            email,
            account_id: None,
            custom: BTreeMap::new(),
        });
        id
    };

    let mut conv_index: HashMap<String, usize> = HashMap::new();
    let mut by_msgid: HashMap<String, String> = HashMap::new();
    let mut system = 0usize;
    for (n, m) in msgs.iter().enumerate() {
        let (topic, topic_src) = m
            .topic
            .clone()
            .unwrap_or_else(|| ("Teams conversation".into(), "default"));
        let ci = *conv_index.entry(topic.clone()).or_insert_with(|| {
            let mut custom = BTreeMap::new();
            custom.insert("topic_source".into(), topic_src.to_string());
            out.conversations.push(Conversation {
                id: stable_id("c", &format!("teams:{topic}")),
                display: topic.clone(),
                kind: ConversationType::Direct,
                participants: vec![],
                custom,
            });
            out.conversations.len() - 1
        });
        let sender = m.sender.as_ref().map(|(n, e)| person(&mut out, n, e));
        let mut members: Vec<String> = sender.iter().cloned().collect();
        for (rn, re) in &m.recipients {
            members.push(person(&mut out, rn, re));
        }
        for id in members {
            if !out.conversations[ci].participants.contains(&id) {
                out.conversations[ci].participants.push(id);
            }
        }
        let id = format!("e-{:06}", n + 1);
        let mut e = Event::message(
            id.clone(),
            out.conversations[ci].id.clone(),
            SourceLoc {
                file: m.file.clone(),
                line: 0,
                line_end: 0,
                native_id: m.message_id.clone(),
            },
        );
        if m.class.starts_with(TEAMS_SYSTEM) {
            e.kind = EventType::History;
            system += 1;
        }
        e.participant = sender;
        if let Some((raw, t, prop)) = &m.time {
            e.timestamp = Some(*t);
            e.source_time = format!(
                "{}.{:07}Z = FILETIME {raw} ({prop})",
                t.format("%Y-%m-%dT%H:%M:%S"),
                t.timestamp_subsec_nanos() / 100
            );
        }
        e.body = m.body.clone();
        e.attachments = m.attachments.clone();
        e.custom.insert("message_class".into(), m.class.clone());
        if !m.body_source.is_empty() {
            e.custom.insert("body_source".into(), m.body_source.into());
        }
        if let Some(s) = m
            .subject
            .as_ref()
            .filter(|s| Some(*s) != m.topic.as_ref().map(|t| &t.0))
        {
            e.custom.insert("subject".into(), s.clone());
        }
        if let Some(r) = &m.in_reply_to {
            match by_msgid.get(r) {
                Some(p) => e.parent = Some(p.clone()),
                None => {
                    e.custom.insert("in_reply_to".into(), r.clone());
                }
            }
        }
        if let Some(mid) = &m.message_id {
            by_msgid.entry(mid.clone()).or_insert(id);
        }
        out.events.push(e);
    }
    // Replies that precede their parent in file order.
    for e in out.events.iter_mut() {
        if let Some(r) = e.custom.get("in_reply_to").cloned()
            && let Some(p) = by_msgid.get(&r).filter(|p| **p != e.id)
        {
            e.parent = Some(p.clone());
            e.custom.remove("in_reply_to");
        }
    }
    for c in out.conversations.iter_mut() {
        if c.participants.len() > 2 {
            c.kind = ConversationType::Channel;
        }
    }

    let missing: Vec<String> = out
        .events
        .iter()
        .flat_map(|e| {
            e.attachments.iter().filter_map(move |a| match &a.data {
                AttachmentData::Missing { .. } => Some(format!("{}: {}", e.source.file, a.display)),
                _ => None,
            })
        })
        .collect();
    if !missing.is_empty() {
        out.findings.push(
            Finding::new("ATTACHMENT_REFERENCED_NOT_FOUND", Severity::Medium)
                .with("count", missing.len().to_string())
                .with("examples", crate::report::capped_list(&missing, 3)),
        );
    }
    if !skipped.is_empty() {
        out.findings.push(
            Finding::new("UNPARSED_LINES", Severity::Medium)
                .with("count", skipped.len().to_string())
                .with("examples", crate::report::capped_list(&skipped, 3)),
        );
    }
    let f = &mut out.facts;
    f.insert("teams.msg_files".into(), files.len().to_string());
    f.insert("teams.messages".into(), (msgs.len() - system).to_string());
    f.insert("teams.system_messages".into(), system.to_string());
    f.insert("teams.files_skipped".into(), skipped.len().to_string());
    f.insert(
        "teams.time_property".into(),
        "PR_CLIENT_SUBMIT_TIME, else PR_MESSAGE_DELIVERY_TIME (FILETIME, UTC)".into(),
    );
    f.insert(
        "teams.property_mapping".into(),
        "standard MAPI properties per Microsoft's reference; not yet confirmed on real Purview exports"
            .into(),
    );
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn stream_names_decode_to_id_and_type() {
        assert_eq!(
            parse_stream_name("__substg1.0_0037001F"),
            Some((PR_SUBJECT, PT_UNICODE))
        );
        assert_eq!(
            parse_stream_name("__substg1.0_37010102"),
            Some((PR_ATTACH_DATA_BIN, PT_BINARY))
        );
        assert_eq!(
            parse_stream_name("__substg1.0_0E04001f"),
            Some((0x0E04, 0x1F))
        );
        assert_eq!(parse_stream_name("__substg1.0_8000101F"), None); // multi-valued
        assert_eq!(parse_stream_name("__substg1.0_00ZZ001F"), None);
        assert_eq!(parse_stream_name("__properties_version1.0"), None);
    }

    #[test]
    fn filetime_round_trips_with_100ns_precision() {
        // 2026-03-02T09:15:00Z
        let t = Utc.with_ymd_and_hms(2026, 3, 2, 9, 15, 0).unwrap();
        let ft = utc_to_filetime(t);
        assert_eq!(ft, 134_169_165_000_000_000);
        assert_eq!(filetime_to_utc(ft), Some(t));
        let t2 = filetime_to_utc(ft + 1_234_567).unwrap();
        assert_eq!(t2.timestamp_subsec_nanos(), 123_456_700);
        assert_eq!(filetime_to_utc(0), None);
        // Unix epoch.
        assert_eq!(
            filetime_to_utc(116_444_736_000_000_000),
            Some(Utc.timestamp_opt(0, 0).unwrap())
        );
    }

    #[test]
    fn fixed_properties_are_read_after_the_header() {
        let mut s = vec![0u8; PROPS_HEADER_MESSAGE];
        let rec = |id: u16, ty: u16, v: u64| {
            let mut r = vec![];
            r.extend_from_slice(&(((id as u32) << 16) | ty as u32).to_le_bytes());
            r.extend_from_slice(&6u32.to_le_bytes());
            r.extend_from_slice(&v.to_le_bytes());
            r
        };
        s.extend(rec(
            PR_CLIENT_SUBMIT_TIME,
            PT_SYSTIME,
            134_169_165_000_000_000,
        ));
        s.extend(rec(PR_INTERNET_CPID, PT_LONG, 65001));
        s.extend(rec(PR_SUBJECT, PT_UNICODE, 12)); // variable size: ignored here
        let p = Props {
            fixed: parse_fixed_props(&s, PROPS_HEADER_MESSAGE),
            ..Default::default()
        };
        assert_eq!(p.long(PR_INTERNET_CPID), Some(65001));
        assert_eq!(
            p.time(PR_CLIENT_SUBMIT_TIME).unwrap().1,
            Utc.with_ymd_and_hms(2026, 3, 2, 9, 15, 0).unwrap()
        );
        assert!(!p.fixed.contains_key(&PR_SUBJECT));
        // Truncated stream: no panic, nothing read.
        assert!(parse_fixed_props(&s[..10], PROPS_HEADER_MESSAGE).is_empty());
    }

    #[test]
    fn utf16_strings_lose_trailing_nuls() {
        let b: Vec<u8> = "Año 1\0"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        assert_eq!(utf16le(&b), "Año 1");
    }

    #[test]
    fn html_is_reduced_to_text() {
        let h = "<html><head><style>p{color:red}</style><title>x</title></head><body>\
                 <div><p>Hola&nbsp;equipo,</p><p>ver &lt;anexo&gt; &amp; <b>confirmar</b>&#33;<br>Gracias &#x2014; Ana</p>\
                 <!-- comment <b> --><script>alert(1)</script></div></body></html>";
        assert_eq!(
            html_to_text(h),
            "Hola equipo,\nver <anexo> & confirmar!\nGracias \u{2014} Ana"
        );
        assert_eq!(html_to_text("a &unknown; b & c"), "a &unknown; b & c");
        assert_eq!(html_to_text("plain"), "plain");
    }

    #[test]
    fn garbage_is_not_a_message() {
        assert!(read_msg(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1garbage", "x.msg").is_err());
        assert!(read_msg(b"", "x.msg").is_err());
    }
}
