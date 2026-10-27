//! Chat-export sources: detection, grouping input files into exports, and the parser interface. FROZEN:
//! parsers implement `parse(&ExportInput, &ParseOptions) -> Result<ParsedExport, ParseError>`.
//!
//! ```text
//! input files ──► plan_exports() ──► Export { kind, primary, members } ──► parser ──► ParsedExport
//!   (hashed and checked             one per WhatsApp .txt/.zip, Telegram result.json,
//!    against the case registry)     Teams .msg folder, CSV file; media files become members
//! ```

pub mod csvmap;
pub mod teams;
pub mod telegram;
pub mod whatsapp;

use crate::model::{ParsedExport, SourceKind};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Day/month/year order of numeric dates (WhatsApp, some CSV exports).
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DateOrder {
    Dmy,
    Mdy,
    Ymd,
}

impl DateOrder {
    pub fn name(&self) -> &'static str {
        match self {
            DateOrder::Dmy => "dmy",
            DateOrder::Mdy => "mdy",
            DateOrder::Ymd => "ymd",
        }
    }
    pub fn parse(s: &str) -> Option<DateOrder> {
        match s.to_ascii_lowercase().as_str() {
            "dmy" => Some(DateOrder::Dmy),
            "mdy" => Some(DateOrder::Mdy),
            "ymd" => Some(DateOrder::Ymd),
            _ => None,
        }
    }
}

/// Built-in CSV column profiles. The XRY and Cellebrite ones are drafts until checked against real exports.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CsvProfile {
    Generic,
    XryDraft,
    CellebriteDraft,
}

impl CsvProfile {
    pub fn name(&self) -> &'static str {
        match self {
            CsvProfile::Generic => "generic",
            CsvProfile::XryDraft => "xry-draft",
            CsvProfile::CellebriteDraft => "cellebrite-draft",
        }
    }
    pub fn parse(s: &str) -> Option<CsvProfile> {
        [
            CsvProfile::Generic,
            CsvProfile::XryDraft,
            CsvProfile::CellebriteDraft,
        ]
        .into_iter()
        .find(|p| p.name() == s)
    }
    pub fn is_draft(&self) -> bool {
        !matches!(self, CsvProfile::Generic)
    }
}

/// Examiner choices that parsers need. Every one of them is recorded in the audit log by run.rs.
#[derive(Clone, Debug, PartialEq)]
pub struct ParseOptions {
    /// Forced source type; None = detect.
    pub source: Option<SourceKind>,
    /// IANA zone for sources whose times have no offset (WhatsApp, CSV). Required for them.
    pub timezone: Option<chrono_tz::Tz>,
    /// Required when a WhatsApp/CSV file is ambiguous (every day and month ≤ 12).
    pub date_order: Option<DateOrder>,
    /// CSV: mapping file (TOML) overriding the profile.
    pub csv_map: Option<PathBuf>,
    pub csv_profile: CsvProfile,
    /// Name or number of the device owner, when known (WhatsApp exports call the owner "You"/"Tú"/"Vous").
    pub custodian: Option<String>,
}

impl Default for ParseOptions {
    fn default() -> Self {
        ParseOptions {
            source: None,
            timezone: None,
            date_order: None,
            csv_map: None,
            csv_profile: CsvProfile::Generic,
            custodian: None,
        }
    }
}

/// Why a parser could not produce a ParsedExport.
#[derive(Clone, Debug, PartialEq)]
pub enum ParseError {
    /// The examiner must choose: exit 2 / GUI prompt. `evidence` = lines showing the ambiguity.
    NeedsDateOrder {
        candidates: Vec<DateOrder>,
        evidence: Vec<String>,
    },
    /// The source has local times without offset and no --timezone was given (exit 2).
    NeedsTimezone,
    /// CSV without a usable mapping (exit 2): missing columns are listed.
    NeedsMapping(String),
    /// The export is malformed or unsupported → finding PARSER_FAILED (the run continues).
    Invalid(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::NeedsDateOrder {
                candidates,
                evidence,
            } => write!(
                f,
                "the day/month order of the dates cannot be decided from the file (possible: {}); give --date-order. Lines: {}",
                candidates
                    .iter()
                    .map(|c| c.name())
                    .collect::<Vec<_>>()
                    .join(", "),
                evidence.join(" | ")
            ),
            ParseError::NeedsTimezone => write!(
                f,
                "the export's times have no UTC offset; give the device's time zone with --timezone (IANA name, e.g. Europe/Paris)"
            ),
            ParseError::NeedsMapping(m) => write!(f, "CSV mapping incomplete: {m}"),
            ParseError::Invalid(m) => write!(f, "{m}"),
        }
    }
}

impl ParseError {
    /// True when the examiner has to decide something before converting (usage error, exit 2).
    pub fn is_usage(&self) -> bool {
        !matches!(self, ParseError::Invalid(_))
    }
}

/// One export = one parser call. Media and other files that belong to it are `members`.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportInput {
    pub kind: SourceKind,
    /// The file the parser starts from: `_chat.txt`/`.zip`, `result.json`, first `.msg`, the `.csv`.
    pub primary: PathBuf,
    /// Folder that relative references are resolved against (media, attachments).
    pub root: PathBuf,
    /// Every input file that belongs to this export, primary included (Teams: all .msg files), sorted.
    pub members: Vec<PathBuf>,
}

impl ExportInput {
    /// Read a member (or any file under `root`) completely. Parsers read evidence only through this
    /// or `std::fs::File::open` (read-only).
    pub fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        let mut v = Vec::new();
        std::fs::File::open(path)?.read_to_end(&mut v)?;
        Ok(v)
    }
}

/// Detect the kind of one file from its name and first bytes. None = not a chat export by itself
/// (media, attachments, unknown).
pub fn detect(path: &Path, head: &[u8]) -> Option<SourceKind> {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "zip" if head.starts_with(b"PK\x03\x04") => {
            whatsapp::zip_has_chat(path).then_some(SourceKind::Whatsapp)
        }
        "txt" => whatsapp::sniff(head).then_some(SourceKind::Whatsapp),
        "json" => {
            let text = String::from_utf8_lossy(head);
            (name == "result.json" || text.contains("\"messages\""))
                .then_some(SourceKind::Telegram)
                .filter(|_| text.contains("\"date") || text.contains("\"chats\""))
        }
        "msg" if head.starts_with(b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1") => {
            Some(SourceKind::TeamsMsg)
        }
        "csv" | "tsv" => Some(SourceKind::Csv),
        _ => None,
    }
}

/// Reads the first 64 KiB (detection never needs more).
pub fn head_of(path: &Path) -> Vec<u8> {
    let mut buf = vec![0u8; 64 * 1024];
    let n = std::fs::File::open(path)
        .and_then(|mut f| f.read(&mut buf))
        .unwrap_or(0);
    buf.truncate(n);
    buf
}

/// Groups input files into exports.
/// - each WhatsApp .txt/.zip, Telegram JSON and CSV file is its own export;
/// - all Teams .msg files in one folder form one export;
/// - any other file is a member of the export whose primary sits in the same folder or an ancestor
///   (media next to `_chat.txt`, Telegram `photos/`, `files/`…); files that belong to no export are
///   returned in the second list.
pub fn plan_exports(
    files: &[PathBuf],
    forced: Option<SourceKind>,
) -> (Vec<ExportInput>, Vec<PathBuf>) {
    let mut sorted = files.to_vec();
    sorted.sort();
    let mut exports: Vec<ExportInput> = vec![];
    let mut teams: BTreeMap<PathBuf, Vec<PathBuf>> = BTreeMap::new();
    let mut rest = vec![];
    for f in &sorted {
        let kind = match forced {
            // A forced source applies to files that look like that source's primary.
            Some(k) => detect(f, &head_of(f)).filter(|d| *d == k).or_else(|| {
                let ext = f
                    .extension()
                    .map(|e| e.to_string_lossy().to_ascii_lowercase());
                match (k, ext.as_deref()) {
                    (SourceKind::Whatsapp, Some("txt" | "zip"))
                    | (SourceKind::Telegram, Some("json"))
                    | (SourceKind::Csv, Some("csv" | "tsv" | "txt"))
                    | (SourceKind::TeamsMsg, Some("msg")) => Some(k),
                    _ => None,
                }
            }),
            None => detect(f, &head_of(f)),
        };
        let root = f.parent().map(Path::to_path_buf).unwrap_or_default();
        match kind {
            Some(SourceKind::TeamsMsg) => teams.entry(root).or_default().push(f.clone()),
            Some(k) => exports.push(ExportInput {
                kind: k,
                primary: f.clone(),
                root,
                members: vec![f.clone()],
            }),
            None => rest.push(f.clone()),
        }
    }
    for (root, msgs) in teams {
        exports.push(ExportInput {
            kind: SourceKind::TeamsMsg,
            primary: msgs[0].clone(),
            root,
            members: msgs,
        });
    }
    exports.sort_by(|a, b| a.primary.cmp(&b.primary));
    let mut orphans = vec![];
    for f in rest {
        // The deepest export root that contains the file owns it.
        let owner = exports
            .iter_mut()
            .filter(|e| f.starts_with(&e.root))
            .max_by_key(|e| e.root.components().count());
        match owner {
            Some(e) => e.members.push(f),
            None => orphans.push(f),
        }
    }
    for e in &mut exports {
        e.members.sort();
    }
    (exports, orphans)
}

/// Runs the right parser. Panics inside a parser are caught and reported as `Invalid` (PARSER_FAILED):
/// one bad export never stops the run.
pub fn parse(export: &ExportInput, opts: &ParseOptions) -> Result<ParsedExport, ParseError> {
    let e = export.clone();
    let o = opts.clone();
    let r = std::panic::catch_unwind(move || match e.kind {
        SourceKind::Whatsapp => whatsapp::parse(&e, &o),
        SourceKind::Telegram => telegram::parse(&e, &o),
        SourceKind::TeamsMsg => teams::parse(&e, &o),
        SourceKind::Csv => csvmap::parse(&e, &o),
    });
    match r {
        Ok(Ok(mut p)) => {
            p.source = Some(export.kind);
            Ok(p)
        }
        Ok(Err(e)) => Err(e),
        Err(panic) => Err(ParseError::Invalid(format!(
            "parser crashed: {}",
            panic
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_else(|| "unknown panic".into())
        ))),
    }
}

/// Up to `n` events in time order, for the preview (CLI `preview`, GUI preview panel).
pub fn preview_events(p: &ParsedExport, n: usize) -> Vec<&crate::model::Event> {
    let mut v: Vec<&crate::model::Event> = p.events.iter().collect();
    v.sort_by(|a, b| (a.timestamp, &a.id).cmp(&(b.timestamp, &b.id)));
    v.truncate(n);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_order_and_profiles_round_trip() {
        for d in [DateOrder::Dmy, DateOrder::Mdy, DateOrder::Ymd] {
            assert_eq!(DateOrder::parse(d.name()), Some(d));
        }
        assert!(CsvProfile::parse("xry-draft").unwrap().is_draft());
        assert!(!CsvProfile::Generic.is_draft());
        assert!(ParseError::NeedsTimezone.is_usage());
        assert!(!ParseError::Invalid("x".into()).is_usage());
    }

    #[test]
    fn media_files_join_the_export_in_their_folder() {
        let d = tempfile::tempdir().unwrap();
        let chat = d.path().join("chat.csv");
        std::fs::write(&chat, "a,b\n").unwrap();
        let media = d.path().join("media");
        std::fs::create_dir(&media).unwrap();
        std::fs::write(media.join("IMG-1.jpg"), b"\xFF\xD8\xFF").unwrap();
        let other = tempfile::tempdir().unwrap();
        std::fs::write(other.path().join("x.bin"), b"x").unwrap();
        let (exports, orphans) = plan_exports(
            &[
                media.join("IMG-1.jpg"),
                chat.clone(),
                other.path().join("x.bin"),
            ],
            None,
        );
        assert_eq!(exports.len(), 1);
        assert_eq!(exports[0].kind, SourceKind::Csv);
        assert_eq!(exports[0].members.len(), 2);
        assert_eq!(orphans, vec![other.path().join("x.bin")]);
    }
}
