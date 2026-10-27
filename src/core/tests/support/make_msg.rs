//! Test helper: writes minimal but structurally valid Outlook `.msg` files ([MS-OXMSG]) with the MAPI
//! properties the Teams parser reads. Deterministic: storages get a fixed time, so the same spec always gives
//! the same bytes. Only for synthetic samples and tests — never for evidence.
#![allow(dead_code)]

use chrono::{DateTime, Utc};
use std::io::{Cursor, Write};

const PT_LONG: u16 = 0x0003;
const PT_UNICODE: u16 = 0x001F;
const PT_SYSTIME: u16 = 0x0040;
const PT_BINARY: u16 = 0x0102;

/// One message to write.
#[derive(Clone, Debug, Default)]
pub struct Msg {
    pub class: String,
    pub subject: Option<String>,
    pub topic: Option<String>,
    pub sender_name: Option<String>,
    pub sender_smtp: Option<String>,
    pub display_to: Option<String>,
    /// (display name, SMTP address) → `__recip_version1.0_#` storages.
    pub recipients: Vec<(String, String)>,
    pub body: Option<String>,
    /// Written as PR_HTML (binary, UTF-8, PR_INTERNET_CPID 65001).
    pub html: Option<String>,
    pub submit_time: Option<DateTime<Utc>>,
    pub delivery_time: Option<DateTime<Utc>>,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    /// (long file name, bytes) → `__attach_version1.0_#` storages with PR_ATTACH_DATA_BIN.
    pub attachments: Vec<(String, Vec<u8>)>,
}

pub fn filetime(t: DateTime<Utc>) -> u64 {
    ((t.timestamp() + 11_644_473_600) as u64) * 10_000_000
        + (t.timestamp_subsec_nanos() / 100) as u64
}

enum Val {
    Str(String),
    Bin(Vec<u8>),
    Long(i32),
    Time(u64),
}

fn utf16(s: &str) -> Vec<u8> {
    s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
}

/// Writes the property streams of one storage: `__substg1.0_*` for variable values and the
/// `__properties_version1.0` table (header of `header_len` bytes, then 16-byte records).
fn write_props(
    cf: &mut cfb::CompoundFile<Cursor<Vec<u8>>>,
    storage: &str,
    header: &[u8],
    props: &[(u16, Val)],
) {
    let mut table = header.to_vec();
    for (id, v) in props {
        let (ty, value): (u16, u64) = match v {
            Val::Str(s) => {
                let b = utf16(s);
                stream(
                    cf,
                    storage,
                    &format!("__substg1.0_{id:04X}{PT_UNICODE:04X}"),
                    &b,
                );
                (PT_UNICODE, (b.len() as u64) + 2)
            }
            Val::Bin(b) => {
                stream(
                    cf,
                    storage,
                    &format!("__substg1.0_{id:04X}{PT_BINARY:04X}"),
                    b,
                );
                (PT_BINARY, b.len() as u64)
            }
            Val::Long(n) => (PT_LONG, *n as u32 as u64),
            Val::Time(ft) => (PT_SYSTIME, *ft),
        };
        table.extend_from_slice(&(((*id as u32) << 16) | ty as u32).to_le_bytes());
        table.extend_from_slice(&6u32.to_le_bytes()); // PROPATTR_READABLE | PROPATTR_WRITABLE
        table.extend_from_slice(&value.to_le_bytes());
    }
    stream(cf, storage, "__properties_version1.0", &table);
}

fn stream(cf: &mut cfb::CompoundFile<Cursor<Vec<u8>>>, storage: &str, name: &str, bytes: &[u8]) {
    let path = format!("{storage}/{name}");
    cf.create_stream(&path)
        .expect("create stream")
        .write_all(bytes)
        .expect("write stream");
}

fn storage(cf: &mut cfb::CompoundFile<Cursor<Vec<u8>>>, path: &str) {
    cf.create_storage(path).expect("create storage");
    // Fixed times (2026-01-01T00:00:00Z) so the output is byte-for-byte reproducible.
    let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_767_225_600);
    cf.set_created_time(path, t).expect("time");
    cf.set_modified_time(path, t).expect("time");
}

/// The bytes of a `.msg` for `m`.
pub fn msg_bytes(m: &Msg) -> Vec<u8> {
    let mut cf = cfb::CompoundFile::create(Cursor::new(Vec::new())).expect("create cfb");
    let mut props: Vec<(u16, Val)> = vec![(0x001A, Val::Str(m.class.clone()))];
    let mut s = |id: u16, v: &Option<String>| {
        if let Some(v) = v {
            props.push((id, Val::Str(v.clone())));
        }
    };
    s(0x0037, &m.subject);
    s(0x0070, &m.topic);
    s(0x0C1A, &m.sender_name);
    s(0x0C1F, &m.sender_smtp);
    s(0x5D01, &m.sender_smtp);
    s(0x0E04, &m.display_to);
    s(0x1000, &m.body);
    s(0x1035, &m.message_id);
    s(0x1042, &m.in_reply_to);
    if m.sender_smtp.is_some() {
        props.push((0x0C1E, Val::Str("SMTP".into()))); // PR_SENDER_ADDRTYPE
    }
    if let Some(h) = &m.html {
        props.push((0x1013, Val::Bin(h.as_bytes().to_vec())));
        props.push((0x3FDE, Val::Long(65001)));
    }
    if let Some(t) = m.submit_time {
        props.push((0x0039, Val::Time(filetime(t))));
    }
    if let Some(t) = m.delivery_time {
        props.push((0x0E06, Val::Time(filetime(t))));
    }
    props.sort_by_key(|(id, _)| *id);
    // Top-level header: reserved 8, next recipient id, next attachment id, recipient count, attachment count,
    // reserved 8 (MS-OXMSG 2.4.1.1).
    let (nr, na) = (m.recipients.len() as u32, m.attachments.len() as u32);
    let mut header = vec![0u8; 8];
    for v in [nr, na, nr, na] {
        header.extend_from_slice(&v.to_le_bytes());
    }
    header.extend_from_slice(&[0u8; 8]);
    write_props(&mut cf, "", &header, &props);

    storage(&mut cf, "/__nameid_version1.0");
    for n in ["00020102", "00030102", "00040102"] {
        stream(
            &mut cf,
            "/__nameid_version1.0",
            &format!("__substg1.0_{n}"),
            &[],
        );
    }
    for (i, (name, smtp)) in m.recipients.iter().enumerate() {
        let st = format!("/__recip_version1.0_#{i:08X}");
        storage(&mut cf, &st);
        write_props(
            &mut cf,
            &st,
            &[0u8; 8],
            &[
                (0x0C15, Val::Long(1)), // PR_RECIPIENT_TYPE = MAPI_TO
                (0x3001, Val::Str(name.clone())),
                (0x3002, Val::Str("SMTP".into())),
                (0x3003, Val::Str(smtp.clone())),
                (0x39FE, Val::Str(smtp.clone())),
            ],
        );
    }
    for (i, (name, bytes)) in m.attachments.iter().enumerate() {
        let st = format!("/__attach_version1.0_#{i:08X}");
        storage(&mut cf, &st);
        write_props(
            &mut cf,
            &st,
            &[0u8; 8],
            &[
                (0x0E20, Val::Long(bytes.len() as i32)), // PR_ATTACH_SIZE
                (0x3701, Val::Bin(bytes.clone())),
                (0x3704, Val::Str(name.clone())),
                (0x3705, Val::Long(1)), // ATTACH_BY_VALUE
                (0x3707, Val::Str(name.clone())),
            ],
        );
    }
    cf.flush().expect("flush");
    cf.into_inner().into_inner()
}
