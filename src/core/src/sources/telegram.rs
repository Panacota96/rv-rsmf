//! Telegram Desktop "Export chat history" JSON parser (`result.json`).
//!
//! ```text
//!  result.json ─┬─ { chats: { list: [chat…] }, left_chats: { list: [chat…] } }   full export
//!               └─ { name, type, id, messages: [...] }                          one chat
//!  chat ──► Conversation (direct: personal/bot/saved; channel: groups, channels)
//!  message ──► Event: date_unixtime (UTC) · from_id → participant · reply_to_message_id → parent
//!              text (string | [string | {type,text,href}]) → body · photo/file → attachment
//!              reactions → Reaction · service actions → join / leave / history
//! ```
//!
//! Field names follow Telegram Desktop's exporter (export_output_json.cpp, read for reference only).

use super::{ExportInput, ParseError, ParseOptions};
use crate::analyzer::{Finding, Severity};
use crate::model::{
    Attachment, AttachmentData, Conversation, ConversationType, Direction, Event, EventType,
    ParsedExport, Participant, Reaction, SourceLoc, stable_id,
};
use crate::report::capped_list;
use crate::rsmf::{local_to_utc, rsmf_time};
use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Component, Path};

/// Text of a JSON scalar (ids and epochs are numbers in some versions, strings in others).
fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// `text` (string, or array of strings and {type, text, href} pieces) → plain text; links keep their
/// target in brackets after the visible text.
fn flat_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Array(parts) => parts
            .iter()
            .map(|p| match p {
                Value::String(s) => s.clone(),
                Value::Object(_) => {
                    let t = p["text"].as_str().unwrap_or("");
                    match p["href"].as_str() {
                        Some(h) if !h.is_empty() && h != t => format!("{t} [{h}]"),
                        _ => t.to_string(),
                    }
                }
                _ => String::new(),
            })
            .collect(),
        _ => String::new(),
    }
}

fn add_member(id: &Option<String>, members: &mut Vec<String>) {
    if let Some(id) = id
        && !members.contains(id)
    {
        members.push(id.clone());
    }
}

fn is_placeholder(s: &str) -> bool {
    s.starts_with("(File ")
}

struct Registry {
    list: Vec<Participant>,
    index: HashMap<String, usize>,
}

impl Registry {
    /// Participant for (from, from_id); None when the export names nobody.
    fn get(&mut self, name: Option<&str>, from_id: Option<&str>) -> Option<String> {
        let key = from_id
            .map(str::to_string)
            .or_else(|| name.map(|n| format!("name:{n}")))?;
        let id = stable_id("p", &key);
        if !self.index.contains_key(&id) {
            self.index.insert(id.clone(), self.list.len());
            self.list.push(Participant {
                id: id.clone(),
                display: name.or(from_id).unwrap_or_default().to_string(),
                email: None,
                account_id: from_id.map(str::to_string),
                custom: BTreeMap::new(),
            });
        } else if let (Some(n), Some(&i)) = (name, self.index.get(&id))
            && self.list[i].display == from_id.unwrap_or_default()
        {
            self.list[i].display = n.to_string(); // a later message gives the name
        }
        Some(id)
    }
}

#[derive(Default)]
struct Tally {
    not_included: usize,
    not_found: Vec<String>,
    edited: usize,
    dst: Vec<String>,
}

/// Resolves a media path relative to the result.json folder.
fn media(export: &ExportInput, path: &str, display: String, t: &mut Tally) -> Attachment {
    if is_placeholder(path) {
        t.not_included += 1;
        return Attachment {
            display,
            data: AttachmentData::Missing {
                reason: path.to_string(),
            },
        };
    }
    let rel = Path::new(path);
    let safe = !path.is_empty()
        && rel
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
    let full = export.root.join(rel);
    if safe && full.is_file() {
        let member = export.members.iter().find(|m| **m == full).cloned();
        return Attachment {
            display,
            data: AttachmentData::File {
                path: member.unwrap_or(full),
            },
        };
    }
    t.not_found.push(path.to_string());
    Attachment {
        display,
        data: AttachmentData::Missing {
            reason: if safe {
                "named in the export but the file is not in it".into()
            } else {
                "path points outside the export folder".into()
            },
        },
    }
}

fn base_name(p: &str) -> String {
    p.rsplit(['/', '\\']).next().unwrap_or(p).to_string()
}

/// Readable body and event type for a service message.
fn service_text(m: &Value, actor: &str) -> (EventType, String) {
    let action = m["action"].as_str().unwrap_or("");
    let title = m["title"].as_str().unwrap_or("");
    let members: Vec<String> = m["members"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|x| x.as_str().unwrap_or("Deleted Account").to_string())
        .collect();
    let who = members.join(", ");
    match action {
        "create_group" => (
            EventType::History,
            if who.is_empty() {
                format!("{actor} created group \"{title}\"")
            } else {
                format!("{actor} created group \"{title}\" with {who}")
            },
        ),
        "create_channel" => (
            EventType::History,
            format!("{actor} created channel \"{title}\""),
        ),
        "migrate_to_supergroup" => (
            EventType::History,
            format!("{actor} converted the group to a supergroup"),
        ),
        "migrate_from_group" => (
            EventType::History,
            format!("{actor} created the supergroup from group \"{title}\""),
        ),
        "invite_members" => (EventType::Join, format!("{actor} invited {who}")),
        "join_group_by_link" => (
            EventType::Join,
            match m["inviter"].as_str() {
                Some(i) => format!("{actor} joined the group via an invite link from {i}"),
                None => format!("{actor} joined the group via an invite link"),
            },
        ),
        "join_group_by_request" => (
            EventType::Join,
            format!("{actor} joined the group (request approved)"),
        ),
        "remove_members" if members.len() == 1 && members[0] == actor => {
            (EventType::Leave, format!("{actor} left the group"))
        }
        "remove_members" => (EventType::Leave, format!("{actor} removed {who}")),
        "pin_message" => (
            EventType::History,
            match m["message_id"].as_i64() {
                Some(id) => format!("{actor} pinned message {id}"),
                None => format!("{actor} pinned a message"),
            },
        ),
        "phone_call" | "group_call" => {
            let kind = if action == "phone_call" {
                "phone call"
            } else {
                "group call"
            };
            let mut s = match m["duration_seconds"].as_i64() {
                Some(d) => format!("{actor} {kind} ({d} s)"),
                None => format!("{actor} {kind}"),
            };
            if let Some(r) = m["discard_reason"].as_str() {
                s.push_str(&format!(", {r}"));
            }
            (EventType::History, s)
        }
        "edit_group_title" => (
            EventType::History,
            format!("{actor} changed the group title to \"{title}\""),
        ),
        "edit_group_photo" => (
            EventType::History,
            format!("{actor} changed the group photo"),
        ),
        "delete_group_photo" => (
            EventType::History,
            format!("{actor} removed the group photo"),
        ),
        "clear_history" => (EventType::History, format!("{actor} cleared the history")),
        "" => (EventType::History, format!("{actor}: service message")),
        other => (
            EventType::History,
            format!("{actor}: {}", other.replace('_', " ")),
        ),
    }
}

/// Location, contact and poll content as readable lines.
fn extra_lines(m: &Value) -> Vec<String> {
    let mut out = vec![];
    let loc = &m["location_information"];
    if loc.is_object() {
        let mut s = format!(
            "Location: {}, {}",
            scalar(&loc["latitude"]).unwrap_or_default(),
            scalar(&loc["longitude"]).unwrap_or_default()
        );
        if let Some(p) = m["live_location_period_seconds"].as_i64() {
            s.push_str(&format!(" (live location, {p} s)"));
        }
        out.push(s);
    }
    let venue: Vec<&str> = [m["place_name"].as_str(), m["address"].as_str()]
        .into_iter()
        .flatten()
        .filter(|s| !s.is_empty())
        .collect();
    if !venue.is_empty() {
        out.push(format!("Venue: {}", venue.join(", ")));
    }
    let c = &m["contact_information"];
    if c.is_object() {
        let name = [c["first_name"].as_str(), c["last_name"].as_str()]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let phone = c["phone_number"].as_str().unwrap_or("");
        out.push(
            format!("Contact: {name}, {phone}")
                .trim_end_matches([',', ' '])
                .to_string(),
        );
    }
    let p = &m["poll"];
    if p.is_object() {
        let mut s = format!("Poll: {}", p["question"].as_str().unwrap_or(""));
        if p["closed"].as_bool() == Some(true) {
            s.push_str(" (closed)");
        }
        for a in p["answers"].as_array().into_iter().flatten() {
            s.push_str(&format!(
                "\n- {} ({} votes)",
                flat_text(&a["text"]),
                a["voters"].as_i64().unwrap_or(0)
            ));
        }
        if let Some(t) = p["total_voters"].as_i64() {
            s.push_str(&format!("\nTotal voters: {t}"));
        }
        out.push(s);
    }
    out
}

fn epoch(v: &Value) -> Option<DateTime<Utc>> {
    scalar(v)?
        .trim()
        .parse::<i64>()
        .ok()
        .and_then(|n| Utc.timestamp_opt(n, 0).single())
}

fn local(v: &Value) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(v.as_str()?, "%Y-%m-%dT%H:%M:%S").ok()
}

pub fn parse(export: &ExportInput, opts: &ParseOptions) -> Result<ParsedExport, ParseError> {
    let bytes = export
        .read(&export.primary)
        .map_err(|e| ParseError::Invalid(format!("{}: {e}", export.primary.display())))?;
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(&bytes);
    let root: Value = serde_json::from_slice(bytes).map_err(|e| {
        ParseError::Invalid(format!("{}: not valid JSON: {e}", export.primary.display()))
    })?;
    let file_label = export
        .primary
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "result.json".into());

    // (chat, left?) in export order.
    let (variant, chats): (&str, Vec<(&Value, bool)>) =
        if root["chats"]["list"].is_array() || root["left_chats"]["list"].is_array() {
            let mut v: Vec<(&Value, bool)> = vec![];
            v.extend(
                root["chats"]["list"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| (c, false)),
            );
            v.extend(
                root["left_chats"]["list"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|c| (c, true)),
            );
            ("full-export", v)
        } else if root["messages"].is_array() {
            ("single-chat", vec![(&root, false)])
        } else {
            return Err(ParseError::Invalid(format!(
                "{file_label}: not a Telegram Desktop export (no chats.list and no messages)"
            )));
        };

    let msgs = |c: &Value| -> Vec<Value> { c["messages"].as_array().cloned().unwrap_or_default() };
    // Times without date_unixtime are local wall-clock times: the examiner's zone is then required.
    let local_only = chats
        .iter()
        .flat_map(|(c, _)| msgs(c))
        .filter(|m| epoch(&m["date_unixtime"]).is_none() && local(&m["date"]).is_some())
        .count();
    let tz = if local_only > 0 {
        Some(opts.timezone.ok_or(ParseError::NeedsTimezone)?)
    } else {
        opts.timezone
    };

    let owner = scalar(&root["personal_information"]["user_id"]).map(|u| format!("user{u}"));
    let mut reg = Registry {
        list: vec![],
        index: HashMap::new(),
    };
    let mut out = ParsedExport::default();
    let mut tally = Tally::default();
    let mut conv_ids = BTreeSet::new();
    let mut n_events = 0usize;

    for (ci, (chat, left)) in chats.iter().enumerate() {
        let chat_id = scalar(&chat["id"]).unwrap_or_else(|| format!("chat-{}", ci + 1));
        let mut conv_id = stable_id("c", &chat_id);
        let mut k = 2;
        while !conv_ids.insert(conv_id.clone()) {
            conv_id = stable_id("c", &format!("{chat_id}#{k}"));
            k += 1;
        }
        let ctype = chat["type"].as_str().unwrap_or("");
        let display = chat["name"]
            .as_str()
            .filter(|n| !n.is_empty())
            .unwrap_or("Saved Messages")
            .to_string();
        let mut members: Vec<String> = vec![];
        let mut by_native: HashMap<String, String> = HashMap::new();
        let mut replies: Vec<(usize, String)> = vec![];
        for m in chat["messages"].as_array().into_iter().flatten() {
            n_events += 1;
            let native = scalar(&m["id"]);
            let mut e = Event::message(
                format!("e-{n_events:06}"),
                conv_id.clone(),
                SourceLoc {
                    file: file_label.clone(),
                    line: 0,
                    line_end: 0,
                    native_id: native.clone(),
                },
            );
            if let Some(n) = &native {
                by_native.insert(n.clone(), e.id.clone());
            }
            // Time.
            e.source_time = m["date"]
                .as_str()
                .map(str::to_string)
                .or_else(|| scalar(&m["date_unixtime"]))
                .unwrap_or_default();
            if let Some(t) = epoch(&m["date_unixtime"]) {
                e.timestamp = Some(t);
            } else if let (Some(l), Some(tz)) = (local(&m["date"]), tz) {
                let (t, problem) = local_to_utc(l, tz);
                e.timestamp = Some(t);
                if let Some(p) = problem {
                    tally.dst.push(e.source_time.clone());
                    e.custom.insert("local_time_issue".into(), p.into());
                }
            }
            if let Some(t) = epoch(&m["edited_unixtime"]).or_else(|| {
                local(&m["edited"])
                    .zip(tz)
                    .map(|(l, tz)| local_to_utc(l, tz).0)
            }) {
                tally.edited += 1;
                e.custom.insert("edited_utc".into(), rsmf_time(&t));
            }
            let mtype = m["type"].as_str().unwrap_or("message");
            let mut lines: Vec<String> = vec![];
            match mtype {
                "service" => {
                    let actor = m["actor"].as_str();
                    let actor_id = scalar(&m["actor_id"]);
                    e.participant = reg.get(actor, actor_id.as_deref());
                    let (kind, body) =
                        service_text(m, actor.or(actor_id.as_deref()).unwrap_or("Someone"));
                    e.kind = kind;
                    lines.push(body);
                    let t = flat_text(&m["text"]);
                    if !t.is_empty() {
                        lines.push(t);
                    }
                }
                _ => {
                    if mtype != "message" {
                        e.kind = EventType::Unknown;
                        e.custom.insert("telegram_type".into(), mtype.into());
                    }
                    let from_id = scalar(&m["from_id"]);
                    e.participant = reg.get(m["from"].as_str(), from_id.as_deref());
                    if let (Some(o), Some(f)) = (&owner, &from_id) {
                        e.direction = Some(if o == f {
                            Direction::Outgoing
                        } else {
                            Direction::Incoming
                        });
                    }
                    let t = flat_text(&m["text"]);
                    if !t.is_empty() {
                        lines.push(t);
                    }
                    if mtype != "message" && lines.is_empty() {
                        lines.push(format!("(Telegram message of type \"{mtype}\")"));
                    }
                }
            }
            lines.extend(extra_lines(m));
            e.body = lines.join("\n");
            add_member(&e.participant, &mut members);
            for (k, key) in [
                ("forwarded_from", "forwarded_from"),
                ("via_bot", "via_bot"),
                ("author", "signature"),
                ("media_type", "media_type"),
                ("mime_type", "mime_type"),
                ("duration_seconds", "duration_seconds"),
                ("sticker_emoji", "sticker_emoji"),
            ] {
                if let Some(v) = scalar(&m[k]).filter(|v| !v.is_empty()) {
                    e.custom.insert(key.into(), v);
                }
            }
            // Media.
            if let Some(p) = m["photo"].as_str() {
                let display = if is_placeholder(p) {
                    "photo".to_string()
                } else {
                    base_name(p)
                };
                e.attachments.push(media(export, p, display, &mut tally));
            }
            for key in ["file", "contact_vcard"] {
                if let Some(p) = m[key].as_str() {
                    let display = m["file_name"]
                        .as_str()
                        .filter(|n| key == "file" && !n.is_empty())
                        .map(str::to_string)
                        .unwrap_or_else(|| {
                            if is_placeholder(p) {
                                m["media_type"].as_str().unwrap_or(key).to_string()
                            } else {
                                base_name(p)
                            }
                        });
                    let a = media(export, p, display, &mut tally);
                    let usable = !matches!(a.data, AttachmentData::Missing { .. });
                    e.attachments.push(a);
                    // A thumbnail stands in for a file that is not in the export.
                    if key == "file"
                        && !usable
                        && let Some(th) = m["thumbnail"].as_str().filter(|t| !is_placeholder(t))
                    {
                        let full = export.root.join(th);
                        if full.is_file() {
                            e.attachments
                                .push(media(export, th, base_name(th), &mut tally));
                        }
                    }
                }
            }
            // Reactions.
            for r in m["reactions"].as_array().into_iter().flatten() {
                let value = match r["type"].as_str() {
                    Some("emoji") => r["emoji"].as_str().unwrap_or("emoji").to_string(),
                    Some("custom_emoji") => "custom_emoji".into(),
                    Some("paid") => "paid".into(),
                    Some(other) => other.to_string(),
                    None => r["emoji"].as_str().unwrap_or("reaction").to_string(),
                };
                let mut who = vec![];
                for x in r["recent"].as_array().into_iter().flatten() {
                    let id = reg.get(x["from"].as_str(), scalar(&x["from_id"]).as_deref());
                    add_member(&id, &mut members);
                    if let Some(id) = id
                        && !who.contains(&id)
                    {
                        who.push(id);
                    }
                }
                e.reactions.push(Reaction {
                    value,
                    count: r["count"].as_u64(),
                    participants: who,
                });
            }
            if let Some(r) = scalar(&m["reply_to_message_id"]) {
                replies.push((out.events.len(), r));
            }
            if let Some(p) = scalar(&m["reply_to_peer_id"]) {
                e.custom.insert("reply_to_peer_id".into(), p);
            }
            out.events.push(e);
        }
        // Replies point to the event id when the replied-to message is in the same chat.
        for (i, r) in replies {
            match by_native.get(&r) {
                Some(p) => out.events[i].parent = Some(p.clone()),
                None => {
                    out.events[i].custom.insert("reply_to_message_id".into(), r);
                }
            }
        }
        let mut custom = BTreeMap::new();
        custom.insert("chat_type".into(), ctype.to_string());
        custom.insert("chat_id".into(), chat_id.clone());
        if *left {
            custom.insert("left_chat".into(), "true".into());
        }
        out.conversations.push(Conversation {
            id: conv_id,
            display,
            kind: match ctype {
                "personal_chat" | "bot_chat" | "saved_messages" => ConversationType::Direct,
                _ => ConversationType::Channel,
            },
            participants: members,
            custom,
        });
    }
    out.participants = reg.list;

    if tally.not_included > 0 {
        out.findings.push(
            Finding::new("TELEGRAM_FILE_NOT_INCLUDED", Severity::Low)
                .with("count", tally.not_included.to_string()),
        );
    }
    if !tally.not_found.is_empty() {
        out.findings.push(
            Finding::new("ATTACHMENT_REFERENCED_NOT_FOUND", Severity::Medium)
                .with("count", tally.not_found.len().to_string())
                .with("examples", capped_list(&tally.not_found, 3)),
        );
    }
    if tally.edited > 0 {
        out.findings.push(
            Finding::new("EDIT_HISTORY_NOT_IN_EXPORT", Severity::Info)
                .with("count", tally.edited.to_string()),
        );
    }
    if let (false, Some(tz)) = (tally.dst.is_empty(), tz) {
        out.findings.push(
            Finding::new("DST_AMBIGUOUS_OR_MISSING_TIME", Severity::Medium)
                .with("count", tally.dst.len().to_string())
                .with("timezone", tz.name())
                .with("examples", capped_list(&tally.dst, 3)),
        );
    }
    let f = &mut out.facts;
    f.insert("variant".into(), variant.into());
    f.insert("chats".into(), out.conversations.len().to_string());
    f.insert("messages_read".into(), out.events.len().to_string());
    f.insert(
        "time_basis".into(),
        if local_only == 0 {
            "date_unixtime (UTC epoch seconds)".into()
        } else {
            format!(
                "{local_only} message(s) without date_unixtime read as local time in {}",
                tz.map(|t| t.name()).unwrap_or("?")
            )
        },
    );
    if let Some(o) = owner {
        f.insert("export_owner".into(), o);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SourceKind;
    use serde_json::json;

    fn run(v: &Value, files: &[&str], opts: &ParseOptions) -> Result<ParsedExport, ParseError> {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("result.json");
        std::fs::write(&p, serde_json::to_vec(v).unwrap()).unwrap();
        let mut members = vec![p.clone()];
        for f in files {
            let fp = d.path().join(f);
            std::fs::create_dir_all(fp.parent().unwrap()).unwrap();
            std::fs::write(&fp, b"\xFF\xD8\xFF").unwrap();
            members.push(fp);
        }
        let e = ExportInput {
            kind: SourceKind::Telegram,
            primary: p,
            root: d.path().to_path_buf(),
            members,
        };
        parse(&e, opts)
    }

    fn single(messages: Value) -> Value {
        json!({"name": "Hugo Martin", "type": "personal_chat", "id": 42, "messages": messages})
    }

    #[test]
    fn text_entities_and_links() {
        let t = json!(["Voir ", {"type": "text_link", "text": "le dossier", "href": "https://example.org/d"},
            " ou ", {"type": "link", "text": "https://example.org/x"}, {"type": "bold", "text": "!"}]);
        assert_eq!(
            flat_text(&t),
            "Voir le dossier [https://example.org/d] ou https://example.org/x!"
        );
        assert_eq!(flat_text(&json!("plain")), "plain");
        assert_eq!(flat_text(&Value::Null), "");
    }

    #[test]
    fn single_chat_messages_replies_reactions() {
        let v = single(json!([
            {"id": 1, "type": "message", "date": "2026-03-02T10:00:00", "date_unixtime": "1772442000",
             "from": "Ana", "from_id": "user1", "text": "hola", "photo": "photos/p1.jpg"},
            {"id": 2, "type": "message", "date": "2026-03-02T10:01:00", "date_unixtime": 1772442060,
             "from": "Bob", "from_id": "user2", "text": "ok", "reply_to_message_id": 1,
             "edited": "2026-03-02T10:02:00", "edited_unixtime": "1772442120",
             "reactions": [{"type": "emoji", "count": 2, "emoji": "👍", "recent": [{"from": "Ana", "from_id": "user1"}, {"from": "Cleo", "from_id": "user3"}]},
                           {"type": "custom_emoji", "count": 1, "document_id": "x.webp"}, {"type": "paid", "count": 5}]},
            {"id": 3, "type": "message", "date": "2026-03-02T10:03:00", "date_unixtime": "1772442180",
             "from": "Bob", "from_id": "user2", "text": "", "reply_to_message_id": 999,
             "file": "(File exceeds maximum size. Change data exporting settings to download.)", "file_name": "big.mp4", "media_type": "video_file"},
            {"id": 4, "type": "message", "date": "2026-03-02T10:04:00", "date_unixtime": "1772442240",
             "from": "Bob", "from_id": "user2", "text": "", "file": "../outside.txt"},
            {"id": 5, "type": "message", "date": "2026-03-02T10:05:00", "date_unixtime": "1772442300",
             "from": "Bob", "from_id": "user2", "text": "", "media_type": "sticker", "sticker_emoji": "😀",
             "file": "stickers/sticker.webp", "thumbnail": "stickers/sticker.webp_thumb.jpg"}
        ]));
        let p = run(
            &v,
            &["photos/p1.jpg", "stickers/sticker.webp"],
            &ParseOptions::default(),
        )
        .unwrap();
        assert_eq!(p.facts["variant"], "single-chat");
        assert_eq!(p.conversations.len(), 1);
        assert_eq!(p.conversations[0].id, stable_id("c", "42"));
        assert_eq!(p.conversations[0].kind, ConversationType::Direct);
        assert_eq!(p.conversations[0].custom["chat_type"], "personal_chat");
        let e = &p.events;
        assert_eq!(rsmf_time(&e[0].timestamp.unwrap()), "2026-03-02T09:00:00Z");
        assert_eq!(e[0].source_time, "2026-03-02T10:00:00");
        assert_eq!(e[0].source.native_id.as_deref(), Some("1"));
        assert_eq!(e[0].source.file, "result.json");
        assert!(matches!(
            e[0].attachments[0].data,
            AttachmentData::File { .. }
        ));
        assert_eq!(e[0].attachments[0].display, "p1.jpg");
        assert_eq!(e[1].parent.as_deref(), Some("e-000001"));
        assert_eq!(e[1].custom["edited_utc"], "2026-03-02T09:02:00Z");
        assert_eq!(e[1].reactions.len(), 3);
        assert_eq!(e[1].reactions[0].value, "👍");
        assert_eq!(e[1].reactions[0].count, Some(2));
        assert_eq!(
            e[1].reactions[0].participants,
            [stable_id("p", "user1"), stable_id("p", "user3")]
        );
        assert_eq!(e[1].reactions[1].value, "custom_emoji");
        assert_eq!(e[1].reactions[2].value, "paid");
        assert_eq!(e[2].parent, None);
        assert_eq!(e[2].custom["reply_to_message_id"], "999");
        assert_eq!(e[2].attachments[0].display, "big.mp4");
        assert!(
            matches!(&e[3].attachments[0].data, AttachmentData::Missing { reason } if reason.contains("outside"))
        );
        assert_eq!(e[4].attachments[0].display, "sticker.webp");
        assert_eq!(e[4].custom["sticker_emoji"], "😀");
        let pp = p.participant(&stable_id("p", "user3")).unwrap();
        assert_eq!(
            (pp.display.as_str(), pp.account_id.as_deref()),
            ("Cleo", Some("user3"))
        );
        let codes: Vec<&str> = p.findings.iter().map(|f| f.code.as_str()).collect();
        assert_eq!(
            codes,
            [
                "TELEGRAM_FILE_NOT_INCLUDED",
                "ATTACHMENT_REFERENCED_NOT_FOUND",
                "EDIT_HISTORY_NOT_IN_EXPORT"
            ]
        );
        assert_eq!(p.findings[1].params["examples"], "../outside.txt");
    }

    #[test]
    fn service_messages_are_readable() {
        let s = |action: &str, extra: Value| {
            let mut m = json!({"id": 1, "type": "service", "actor": "Ana", "actor_id": "user1", "action": action});
            for (k, v) in extra.as_object().unwrap() {
                m[k] = v.clone();
            }
            service_text(&m, "Ana")
        };
        assert_eq!(
            s("invite_members", json!({"members": ["Bob", "Cleo"]})),
            (EventType::Join, "Ana invited Bob, Cleo".into())
        );
        assert_eq!(
            s("create_group", json!({"title": "Atlas"})),
            (EventType::History, "Ana created group \"Atlas\"".into())
        );
        assert_eq!(
            s("remove_members", json!({"members": ["Ana"]})),
            (EventType::Leave, "Ana left the group".into())
        );
        assert_eq!(
            s("remove_members", json!({"members": ["Bob"]})),
            (EventType::Leave, "Ana removed Bob".into())
        );
        assert_eq!(
            s("pin_message", json!({"message_id": 7})),
            (EventType::History, "Ana pinned message 7".into())
        );
        assert_eq!(
            s("phone_call", json!({"duration_seconds": 125})),
            (EventType::History, "Ana phone call (125 s)".into())
        );
        assert_eq!(s("join_group_by_link", json!({})).0, EventType::Join);
        assert_eq!(s("set_ttl", json!({})).1, "Ana: set ttl");
    }

    #[test]
    fn location_contact_poll() {
        let m = json!({"location_information": {"latitude": 48.85, "longitude": 2.35},
            "contact_information": {"first_name": "Julie", "last_name": "Petit", "phone_number": "+33 6 00 00 00 03"},
            "poll": {"question": "Jeudi ?", "closed": true, "total_voters": 1, "answers": [{"text": "Oui", "voters": 1}]}});
        assert_eq!(
            extra_lines(&m),
            [
                "Location: 48.85, 2.35",
                "Contact: Julie Petit, +33 6 00 00 00 03",
                "Poll: Jeudi ? (closed)\n- Oui (1 votes)\nTotal voters: 1"
            ]
        );
    }

    #[test]
    fn full_export_left_chats_and_saved_messages() {
        let v = json!({"personal_information": {"user_id": 1},
            "chats": {"list": [
                {"type": "saved_messages", "id": 1, "messages": [{"id": 1, "type": "message", "date_unixtime": "1772442000", "from": "Me", "from_id": "user1", "text": "note"}]},
                {"name": "Atlas", "type": "private_supergroup", "id": 7, "messages": []}]},
            "left_chats": {"list": [{"name": "Old", "type": "private_group", "id": 8, "messages": []}]}});
        let p = run(&v, &[], &ParseOptions::default()).unwrap();
        assert_eq!(p.facts["variant"], "full-export");
        let names: Vec<&str> = p.conversations.iter().map(|c| c.display.as_str()).collect();
        assert_eq!(names, ["Saved Messages", "Atlas", "Old"]);
        assert_eq!(p.conversations[1].kind, ConversationType::Channel);
        assert_eq!(p.conversations[2].custom["left_chat"], "true");
        assert_eq!(p.events[0].direction, Some(Direction::Outgoing));
    }

    #[test]
    fn local_dates_need_a_timezone() {
        let v = single(
            json!([{"id": 1, "type": "message", "date": "2026-10-25T02:30:00", "from": "Ana", "from_id": "user1", "text": "x"}]),
        );
        assert_eq!(
            run(&v, &[], &ParseOptions::default()),
            Err(ParseError::NeedsTimezone)
        );
        let o = ParseOptions {
            timezone: Some("Europe/Paris".parse().unwrap()),
            ..ParseOptions::default()
        };
        let p = run(&v, &[], &o).unwrap();
        assert_eq!(
            rsmf_time(&p.events[0].timestamp.unwrap()),
            "2026-10-25T00:30:00Z"
        );
        assert_eq!(p.findings[0].code, "DST_AMBIGUOUS_OR_MISSING_TIME");
    }

    #[test]
    fn not_telegram() {
        assert!(matches!(
            run(&json!({"a": 1}), &[], &ParseOptions::default()),
            Err(ParseError::Invalid(_))
        ));
    }
}
