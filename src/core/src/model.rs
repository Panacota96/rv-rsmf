//! The neutral chat model every parser produces and the RSMF writer consumes. FROZEN: parsers may add
//! optional fields only with a default; renaming or removing a field changes every parser at once.
//!
//! ```text
//!  WhatsApp .txt/.zip ─┐
//!  Telegram JSON ──────┼─► ParsedExport { conversations, participants, events, attachments, findings }
//!  Teams .msg ─────────┤        │  (times in UTC; every event knows where it came from)
//!  CSV + mapping ──────┘        ▼
//!                       slice per conversation per day ─► rsmf_manifest.json + rsmf.zip + .rsmf (EML)
//! ```

use crate::analyzer::Finding;
use chrono::{DateTime, Utc};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Which parser read the export.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    Whatsapp,
    Telegram,
    TeamsMsg,
    Csv,
}

impl SourceKind {
    pub const ALL: [SourceKind; 4] = [
        SourceKind::Whatsapp,
        SourceKind::Telegram,
        SourceKind::TeamsMsg,
        SourceKind::Csv,
    ];
    pub fn name(&self) -> &'static str {
        match self {
            SourceKind::Whatsapp => "whatsapp",
            SourceKind::Telegram => "telegram",
            SourceKind::TeamsMsg => "teams-msg",
            SourceKind::Csv => "csv",
        }
    }
    pub fn parse(s: &str) -> Option<SourceKind> {
        SourceKind::ALL.into_iter().find(|k| k.name() == s)
    }
    /// RSMF `platform` value (overridable with --platform-name).
    pub fn platform(&self) -> &'static str {
        match self {
            SourceKind::Whatsapp => "WhatsApp",
            SourceKind::Telegram => "Telegram",
            SourceKind::TeamsMsg => "Microsoft Teams",
            SourceKind::Csv => "Mobile export",
        }
    }
    /// Whether timestamps in this source carry no offset (then `--timezone` is required).
    pub fn needs_timezone(&self) -> bool {
        matches!(self, SourceKind::Whatsapp | SourceKind::Csv)
    }
}

/// Where in the export an event (or attachment) came from — the basis of `trace.csv`.
#[derive(Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct SourceLoc {
    /// File inside the export (e.g. `_chat.txt`, `result.json`, `0001.msg`), relative to the export root.
    pub file: String,
    /// 1-based first line (text sources); 0 when not applicable.
    pub line: u64,
    /// Last line for multi-line messages; equals `line` otherwise.
    pub line_end: u64,
    /// Native message id when the source has one (Telegram `id`, Teams message id, CSV row number).
    pub native_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Participant {
    /// Stable id inside the export (e.g. `p-<sha256 of the display name>[..12]`); unique per ParsedExport.
    pub id: String,
    pub display: String,
    pub email: Option<String>,
    /// Phone number, Telegram `user123`, Teams AAD id…
    pub account_id: Option<String>,
    pub custom: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ConversationType {
    Direct,
    Channel,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Conversation {
    /// Unique per ParsedExport; also used for the output folder slug.
    pub id: String,
    pub display: String,
    pub kind: ConversationType,
    /// Participant ids (all who appear or are listed as members).
    pub participants: Vec<String>,
    pub custom: BTreeMap<String, String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EventType {
    Message,
    Disclaimer,
    Join,
    Leave,
    History,
    Unknown,
}

impl EventType {
    pub fn name(&self) -> &'static str {
        match self {
            EventType::Message => "message",
            EventType::Disclaimer => "disclaimer",
            EventType::Join => "join",
            EventType::Leave => "leave",
            EventType::History => "history",
            EventType::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Incoming,
    Outgoing,
}

/// Where the bytes of an attachment are.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AttachmentData {
    /// A file on disk (unzipped export, Telegram media folder).
    File { path: PathBuf },
    /// A member of a zip export (WhatsApp .zip).
    ZipMember { zip: PathBuf, name: String },
    /// Bytes already in memory (Teams .msg attachment streams).
    Inline {
        #[serde(skip)]
        bytes: Vec<u8>,
    },
    /// Referenced by the export but not present (media omitted, file not included…).
    Missing { reason: String },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Attachment {
    /// File name shown to the reviewer (keeps the original name and extension).
    pub display: String,
    pub data: AttachmentData,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Reaction {
    /// Unicode emoji or text as in the export; the writer maps it to a Relativity shortcode.
    pub value: String,
    pub count: Option<u64>,
    pub participants: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Edit {
    pub participant: String,
    pub timestamp: Option<DateTime<Utc>>,
    pub previous: Option<String>,
    pub new: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Event {
    /// Unique per ParsedExport (e.g. `e-000001`); RSMF `id`, also the target of `parent`.
    pub id: String,
    pub kind: EventType,
    pub conversation: String,
    pub participant: Option<String>,
    /// UTC. None only when the source has no time at all (the event is then reported and kept last).
    pub timestamp: Option<DateTime<Utc>>,
    /// The time exactly as written in the export (kept for the trace and the report).
    pub source_time: String,
    pub body: String,
    pub parent: Option<String>,
    pub deleted: bool,
    pub direction: Option<Direction>,
    pub attachments: Vec<Attachment>,
    pub reactions: Vec<Reaction>,
    pub edits: Vec<Edit>,
    pub custom: BTreeMap<String, String>,
    pub source: SourceLoc,
}

impl Event {
    pub fn message(id: String, conversation: String, source: SourceLoc) -> Event {
        Event {
            id,
            kind: EventType::Message,
            conversation,
            participant: None,
            timestamp: None,
            source_time: String::new(),
            body: String::new(),
            parent: None,
            deleted: false,
            direction: None,
            attachments: vec![],
            reactions: vec![],
            edits: vec![],
            custom: BTreeMap::new(),
            source,
        }
    }
}

/// What a parser returns for one export.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ParsedExport {
    pub source: Option<SourceKind>,
    pub participants: Vec<Participant>,
    pub conversations: Vec<Conversation>,
    pub events: Vec<Event>,
    /// Signals about the export itself (UNPARSED_LINES, MEDIA_OMITTED_IN_EXPORT…). Codes in i18n.
    pub findings: Vec<Finding>,
    /// Facts for the technical report: detected format variant, date order used and why, lines read…
    pub facts: BTreeMap<String, String>,
}

impl ParsedExport {
    pub fn participant(&self, id: &str) -> Option<&Participant> {
        self.participants.iter().find(|p| p.id == id)
    }
}

/// Stable short id from any text (participants, conversations): `prefix-` + first 12 hex of SHA-256.
pub fn stable_id(prefix: &str, text: &str) -> String {
    let h = crate::hash::hash_bytes(text.as_bytes(), crate::hash::HashAlgo::Sha256);
    format!("{prefix}-{}", &h[..12])
}
