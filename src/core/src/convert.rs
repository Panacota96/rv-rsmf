//! The conversion stage of a run: after every input file is hashed (and checked against the case registry),
//! the files are grouped into exports, each export is parsed, sliced per conversation per day, written as
//! RSMF, validated, read back, and traced line by line. Every step is in the audit log.
//!
//! ```mermaid
//! flowchart LR
//!     H["items hashed<br/>(audit: item_hashed)"] --> P["plan_exports<br/>(export_planned)"] --> R["parse<br/>(export_parsed)"]
//!     R --> S["slice per conversation per day"] --> W["write .rsmf<br/>(rsmf_written, attachment_packed)"]
//!     W --> V["schema + strict + read back<br/>(rsmf_validated)"] --> T["trace.csv / trace.json<br/>(trace_written)"]
//! ```

use crate::analyzer::{Finding, Severity};
use crate::audit::AuditLog;
use crate::events::Event;
use crate::hash::HashAlgo;
use crate::model::{ParsedExport, SourceKind};
use crate::report::{self, ItemResult, capped_list};
use crate::rsmf::{self, Slice, WriteOptions, WrittenSlice};
use crate::sources::{self, ExportInput, ParseError, ParseOptions};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

/// Conversion settings of a run (CLI options / GUI settings map 1:1 onto these fields).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConvertOptions {
    pub parse: ParseOptions,
    /// Overrides the RSMF `platform` / X-RSMF-Application value.
    pub platform_name: Option<String>,
}

impl ConvertOptions {
    /// Audit/environment description (strings only).
    pub fn to_json(&self) -> Value {
        json!({
            "source": self.parse.source.map(|s| s.name()),
            "timezone": self.parse.timezone.map(|t| t.name().to_string()),
            "date_order": self.parse.date_order.map(|d| d.name()),
            "csv_map": self.parse.csv_map.as_ref().map(|p| p.display().to_string()),
            "csv_profile": self.parse.csv_profile.name(),
            "custodian": self.parse.custodian,
            "platform_name": self.platform_name,
            "slice": "day",
        })
    }
}

/// One RSMF file in an item's report.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct FileRow {
    /// Path relative to the run's rsmf folder, forward slashes.
    pub file: String,
    pub conversation: String,
    pub day: Option<String>,
    pub part: u32,
    pub events: usize,
    pub attachments: usize,
    pub missing_attachments: usize,
    pub valid: bool,
    pub sha256: String,
}

/// What the conversion produced for one export (attached to its primary item for the reports).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct ItemConversion {
    pub source: String,
    /// Primary file of the export this item belongs to (for media members).
    pub export: String,
    pub primary: bool,
    pub conversations: Vec<(String, String)>,
    pub participants: usize,
    pub events: usize,
    pub files: Vec<FileRow>,
    pub facts: BTreeMap<String, String>,
    pub error: Option<String>,
}

/// A usage problem found before the run starts (exit 2 in the CLI; a prompt in the GUI).
#[derive(Clone, Debug, PartialEq)]
pub struct PreflightError {
    pub path: PathBuf,
    pub error: ParseError,
}

impl std::fmt::Display for PreflightError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.error)
    }
}

/// Checks, before any output is written, that every export can be converted with the given options:
/// time zone given where needed, date order decidable, CSV mapping complete. Parse failures that are not
/// the examiner's to fix (damaged files) are left to the run (PARSER_FAILED).
pub fn preflight(
    files: &[PathBuf],
    opts: &ConvertOptions,
) -> Result<Vec<(ExportInput, Option<ParsedExport>)>, PreflightError> {
    let (exports, _) = sources::plan_exports(files, opts.parse.source);
    let mut out = vec![];
    for e in exports {
        match sources::parse(&e, &opts.parse) {
            Ok(p) => out.push((e, Some(p))),
            Err(err) if err.is_usage() => {
                return Err(PreflightError {
                    path: e.primary.clone(),
                    error: err,
                });
            }
            Err(_) => out.push((e, None)),
        }
    }
    Ok(out)
}

/// Pipeline-level findings computed on the neutral model.
pub fn model_findings(p: &ParsedExport, opts: &ConvertOptions) -> Vec<Finding> {
    let mut f = vec![];
    if let (Some(k), Some(tz)) = (p.source, opts.parse.timezone)
        && (k.needs_timezone() || p.facts.get("timezone_used").is_some_and(|v| v == "yes"))
    {
        f.push(Finding::new("TIMEZONE_APPLIED", Severity::Info).with("timezone", tz.name()));
    }
    // Out of order within a conversation, in export order.
    let mut last: BTreeMap<&str, chrono::DateTime<chrono::Utc>> = BTreeMap::new();
    let mut back = 0usize;
    let mut first = None;
    for e in &p.events {
        if let Some(t) = e.timestamp {
            if let Some(prev) = last.get(e.conversation.as_str())
                && t < *prev
            {
                back += 1;
                first.get_or_insert_with(|| e.source_time.clone());
            }
            last.insert(e.conversation.as_str(), t);
        }
    }
    if back > 0 {
        f.push(
            Finding::new("NON_MONOTONIC_TIMESTAMPS", Severity::Medium)
                .with("count", back.to_string())
                .with("first", first.unwrap_or_default()),
        );
    }
    let mut seen = BTreeSet::new();
    let dups = p
        .events
        .iter()
        .filter(|e| e.kind == crate::model::EventType::Message && !e.body.is_empty())
        .filter(|e| {
            !seen.insert((
                e.conversation.clone(),
                e.participant.clone(),
                e.timestamp,
                e.body.clone(),
            ))
        })
        .count();
    if dups > 0 {
        f.push(Finding::new("DUPLICATE_MESSAGES", Severity::Low).with("count", dups.to_string()));
    }
    if let Some(k @ (SourceKind::Whatsapp | SourceKind::Telegram)) = p.source {
        f.push(
            Finding::new("DELETED_MESSAGES_NOT_IN_EXPORT", Severity::Info)
                .with("source", k.platform()),
        );
    }
    f
}

/// Where the run writes its RSMF files.
pub fn rsmf_dir_for(out_root: &Path) -> PathBuf {
    out_root.join("rsmf")
}

fn rel(base: &Path, p: &Path) -> String {
    p.strip_prefix(base)
        .unwrap_or(p)
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// One row of trace.csv: where a message came from and where it went.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct TraceRow {
    pub export: String,
    pub source_file: String,
    pub line: u64,
    pub line_end: u64,
    pub native_id: Option<String>,
    pub source_time: String,
    pub utc: Option<String>,
    pub rsmf_file: String,
    pub event_id: String,
    pub conversation: String,
    pub kind: String,
    pub attachments: usize,
}

/// Totals for the statistics report.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct ConversionTotals {
    pub exports: usize,
    pub exports_failed: usize,
    pub conversations: usize,
    pub events: usize,
    pub files: usize,
    pub files_invalid: usize,
    pub attachments_packed: usize,
    pub attachments_missing: usize,
}

/// Runs the conversion stage over the hashed items. Findings and conversion details are attached to
/// `results`; every output file is hashed and logged.
#[allow(clippy::too_many_arguments)]
pub fn convert_items(
    results: &mut [ItemResult],
    opts: &ConvertOptions,
    algorithms: &[HashAlgo],
    generator: &str,
    seed: &str,
    rsmf_dir: &Path,
    log: &mut AuditLog,
    sink: &mut dyn FnMut(&Event),
    cancel: &AtomicBool,
) -> Result<ConversionTotals, String> {
    let io = |e: std::io::Error| format!("audit log write failed: {e}");
    let mut totals = ConversionTotals::default();
    let readable: Vec<PathBuf> = results
        .iter()
        .filter(|r| r.error.is_none())
        .map(|r| r.path.clone())
        .collect();
    let (exports, orphans) = sources::plan_exports(&readable, opts.parse.source);
    let idx_of = |results: &[ItemResult], p: &Path| results.iter().position(|r| r.path == p);
    for o in &orphans {
        if let Some(i) = idx_of(results, o) {
            results[i]
                .findings
                .push(Finding::new("NOT_A_CHAT_EXPORT", Severity::Info));
        }
    }
    std::fs::create_dir_all(rsmf_dir).map_err(|e| format!("{}: {e}", rsmf_dir.display()))?;
    let mut trace: Vec<TraceRow> = vec![];
    let mut manifest_rows: Vec<Value> = vec![];
    for (n, ex) in exports.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        totals.exports += 1;
        let primary_name = ex
            .primary
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(pi) = idx_of(results, &ex.primary) else {
            continue;
        };
        log.record(
            "export_planned",
            Some(&ex.primary.display().to_string()),
            json!({"export": n + 1, "source": ex.kind.name(), "item_index": results[pi].index,
                   "members": ex.members.len(), "root": ex.root.display().to_string()}),
        )
        .map_err(io)?;
        for m in ex.members.iter().filter(|m| **m != ex.primary) {
            if let Some(i) = idx_of(results, m)
                && !(ex.kind == SourceKind::TeamsMsg
                    && m.extension().is_some_and(|e| e.eq_ignore_ascii_case("msg")))
            {
                results[i].findings.push(
                    Finding::new("PART_OF_EXPORT", Severity::Info)
                        .with("export", primary_name.clone()),
                );
            }
        }
        sink(&Event::ExportStarted {
            index: results[pi].index,
            path: ex.primary.display().to_string(),
            source: ex.kind.name().into(),
        });
        let parsed = match sources::parse(ex, &opts.parse) {
            Ok(p) => p,
            Err(e) => {
                totals.exports_failed += 1;
                let msg = e.to_string();
                log.record(
                    "export_failed",
                    Some(&ex.primary.display().to_string()),
                    json!({"source": ex.kind.name(), "error": msg}),
                )
                .map_err(io)?;
                results[pi].findings.push(
                    Finding::new("PARSER_FAILED", Severity::High)
                        .with("source", ex.kind.platform())
                        .with("error", msg.clone()),
                );
                results[pi].conversion = Some(ItemConversion {
                    source: ex.kind.name().into(),
                    export: primary_name.clone(),
                    primary: true,
                    error: Some(msg.clone()),
                    ..Default::default()
                });
                sink(&Event::Error {
                    path: Some(ex.primary.display().to_string()),
                    message: msg,
                });
                continue;
            }
        };
        log.record(
            "export_parsed",
            Some(&ex.primary.display().to_string()),
            json!({"source": ex.kind.name(), "conversations": parsed.conversations.len(),
                   "participants": parsed.participants.len(), "events": parsed.events.len(),
                   "facts": parsed.facts, "findings": parsed.findings}),
        )
        .map_err(io)?;
        let mut findings = parsed.findings.clone();
        findings.extend(model_findings(&parsed, opts));
        let tz = opts.parse.timezone.unwrap_or(chrono_tz::UTC);
        let wo = WriteOptions {
            tz,
            custodian: opts.parse.custodian.clone(),
            platform: rsmf::platform_for(ex.kind, opts.platform_name.as_deref()),
            generator: generator.to_string(),
            collection_seed: format!(
                "{seed}\u{1f}{}",
                results[pi]
                    .hashes
                    .first()
                    .map(|h| h.1.as_str())
                    .unwrap_or("")
            ),
        };
        let slices: Vec<Slice> = rsmf::slice(&parsed, tz);
        // Busy days split into parts.
        let mut split: BTreeMap<(String, String), u32> = BTreeMap::new();
        for s in slices.iter().filter(|s| s.part > 0) {
            *split
                .entry((
                    s.conversation.clone(),
                    s.day.map(|d| d.to_string()).unwrap_or_default(),
                ))
                .or_default() += 1;
        }
        for ((c, d), parts) in &split {
            let disp = parsed
                .conversations
                .iter()
                .find(|x| &x.id == c)
                .map(|x| x.display.clone())
                .unwrap_or_default();
            findings.push(
                Finding::new("DAY_SPLIT_OVER_10000_EVENTS", Severity::Info)
                    .with("conversation", disp)
                    .with("day", d.clone())
                    .with("parts", parts.to_string()),
            );
        }
        let export_dir = rsmf_dir.join(format!(
            "{:03}-{}",
            n + 1,
            rsmf::slug(&primary_name, &format!("{:06}", results[pi].index))
        ));
        let mut rows = vec![];
        let mut unsupported: BTreeSet<String> = BTreeSet::new();
        for s in &slices {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let (ws, bytes): (WrittenSlice, Vec<u8>) =
                rsmf::write_slice(&parsed, s, &wo, &export_dir)?;
            let digests: serde_json::Map<String, Value> = algorithms
                .iter()
                .map(|a| {
                    (
                        a.name().to_string(),
                        json!(crate::hash::hash_bytes(&bytes, *a)),
                    )
                })
                .collect();
            let sha256 = crate::hash::hash_bytes(&bytes, HashAlgo::Sha256);
            let relp = rel(rsmf_dir, &ws.path);
            log.record(
                "rsmf_written",
                Some(&relp),
                json!({"conversation": ws.conversation, "day": ws.day, "part": ws.part, "events": ws.events,
                       "attachments": ws.attachments.len(), "missing_attachments": ws.missing_attachments,
                       "event_collection_id": ws.event_collection_id, "bytes": bytes.len(),
                       "hashes": digests, "sha256": sha256}),
            )
            .map_err(io)?;
            for a in &ws.attachments {
                log.record(
                    "attachment_packed",
                    Some(&relp),
                    json!({"event": a.event, "zip_name": a.zip_name, "display": a.display, "size": a.size, "sha256": a.sha256}),
                )
                .map_err(io)?;
            }
            let (v, rt) = match rsmf::roundtrip(&ws) {
                Ok(problems) => (
                    rsmf::validate_file(&ws.path)
                        .map(|x| x.0)
                        .unwrap_or_default(),
                    problems,
                ),
                Err(e) => (rsmf::Validation::default(), vec![e]),
            };
            let valid = v.ok() && rt.is_empty();
            log.record(
                "rsmf_validated",
                Some(&relp),
                json!({"valid": valid, "schema_errors": v.schema_errors, "strict_errors": v.strict_errors,
                       "roundtrip_problems": rt, "schema": "rsmf_schema_2_0_0.json"}),
            )
            .map_err(io)?;
            findings.extend(rsmf::slice_findings(&ws, &v, &rt));
            unsupported.extend(ws.unsupported_reactions.iter().cloned());
            totals.files += 1;
            totals.events += ws.events;
            totals.attachments_packed += ws.attachments.len();
            totals.attachments_missing += ws.missing_attachments;
            if !valid {
                totals.files_invalid += 1;
            }
            for &i in &s.events {
                let e = &parsed.events[i];
                trace.push(TraceRow {
                    export: primary_name.clone(),
                    source_file: e.source.file.clone(),
                    line: e.source.line,
                    line_end: e.source.line_end,
                    native_id: e.source.native_id.clone(),
                    source_time: e.source_time.clone(),
                    utc: e.timestamp.as_ref().map(rsmf::rsmf_time),
                    rsmf_file: relp.clone(),
                    event_id: e.id.clone(),
                    conversation: e.conversation.clone(),
                    kind: e.kind.name().into(),
                    attachments: e.attachments.len(),
                });
            }
            sink(&Event::RsmfWritten {
                path: ws.path.display().to_string(),
                conversation: ws.conversation.clone(),
                day: ws.day.clone(),
                events: ws.events as u64,
                valid,
            });
            rows.push(FileRow {
                file: relp,
                conversation: parsed
                    .conversations
                    .iter()
                    .find(|c| c.id == ws.conversation)
                    .map(|c| c.display.clone())
                    .unwrap_or_default(),
                day: ws.day.clone(),
                part: ws.part,
                events: ws.events,
                attachments: ws.attachments.len(),
                missing_attachments: ws.missing_attachments,
                valid,
                sha256,
            });
        }
        if !unsupported.is_empty() {
            let v: Vec<String> = unsupported.into_iter().collect();
            findings.push(
                Finding::new("REACTION_NOT_SUPPORTED_BY_RELATIVITY", Severity::Info)
                    .with("values", capped_list(&v, 10)),
            );
        }
        totals.conversations += parsed.conversations.len();
        manifest_rows.push(json!({"export": primary_name, "source": ex.kind.name(), "item_index": results[pi].index,
            "conversations": parsed.conversations.len(), "events": parsed.events.len(), "files": rows}));
        let conv = ItemConversion {
            source: ex.kind.name().into(),
            export: primary_name.clone(),
            primary: true,
            conversations: parsed
                .conversations
                .iter()
                .map(|c| (c.id.clone(), c.display.clone()))
                .collect(),
            participants: parsed.participants.len(),
            events: parsed.events.len(),
            files: rows,
            facts: parsed.facts.clone(),
            error: None,
        };
        sink(&Event::ExportFinished {
            index: results[pi].index,
            path: ex.primary.display().to_string(),
            conversations: parsed.conversations.len() as u64,
            events: parsed.events.len() as u64,
            files: conv.files.len() as u64,
            invalid: conv.files.iter().filter(|f| !f.valid).count() as u64,
        });
        results[pi].findings.extend(findings);
        results[pi].conversion = Some(conv);
    }

    // trace.csv + trace.json + conversions.json, hashed and logged.
    let mut csv = String::from(
        "export,source_file,line,line_end,native_id,source_time,utc,rsmf_file,event_id,conversation,kind,attachments\n",
    );
    for t in &trace {
        csv.push_str(&format!(
            "{},{},{},{},{},{},{},{},{},{},{},{}\n",
            crate::run::csv_field(&t.export),
            crate::run::csv_field(&t.source_file),
            t.line,
            t.line_end,
            crate::run::csv_field(t.native_id.as_deref().unwrap_or("")),
            crate::run::csv_field(&t.source_time),
            t.utc.as_deref().unwrap_or(""),
            crate::run::csv_field(&t.rsmf_file),
            t.event_id,
            t.conversation,
            t.kind,
            t.attachments
        ));
    }
    for (name, data) in [
        ("trace.csv", csv.into_bytes()),
        ("trace.json", report::pretty(&json!(trace))),
        (
            "conversions.json",
            report::pretty(&json!({"totals": totals, "exports": manifest_rows,
            "shortcode_table": rsmf::shortcodes::TABLE_VERSION, "rsmf_version": rsmf::RSMF_VERSION})),
        ),
    ] {
        let (_, d) = report::write_hashed(rsmf_dir, name, &data, algorithms)?;
        log.record("trace_written", Some(name), d).map_err(io)?;
    }
    Ok(totals)
}
