//! Everything the window shows and every rule it enforces, without any drawing code.
//! Kept separate from `app.rs` so it can be unit-tested headless (CI has no display).

use rv_rsmf_core::{
    CASE_ROLE, Event, HashAlgo, RunConfig, RunSummary, TOOL_NAME, TOOL_VERSION, case,
    convert::{self, ConversionTotals, ConvertOptions, PreflightError},
    hash, i18n,
    intake::IntakeTool,
    model::SourceKind,
    rsmf::rsmf_time,
    sources::{self, CsvProfile, DateOrder, ParseError, ParseOptions},
    time::human_duration,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

/// Time zones offered next to the free-text field (any IANA name can still be typed).
pub const COMMON_ZONES: [&str; 6] = [
    "Europe/Paris",
    "Europe/Madrid",
    "America/Bogota",
    "America/Mexico_City",
    "America/New_York",
    "UTC",
];

/// Messages shown per export in the preview panel.
pub const PREVIEW_ROWS: usize = 20;
/// Longest message text shown in the preview table (characters).
pub const PREVIEW_BODY_CHARS: usize = 120;

/// One row of the preview table: the time as written in the export, the UTC time it becomes, who, what.
#[derive(Clone, Debug, PartialEq)]
pub struct PreviewRow {
    pub source_time: String,
    pub utc: String,
    pub sender: String,
    pub body: String,
}

/// What the preview read from one export.
#[derive(Clone, Debug, PartialEq)]
pub struct ExportPreview {
    pub path: String,
    pub source: SourceKind,
    pub facts: BTreeMap<String, String>,
    pub conversations: usize,
    pub participants: Vec<String>,
    pub events: usize,
    pub first: Vec<PreviewRow>,
    /// The export could not be read (not a usage problem: the run reports PARSER_FAILED).
    pub error: Option<String>,
}

/// Result of the last "Preview" for the current settings (cleared whenever a relevant setting changes).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PreviewState {
    pub exports: Vec<ExportPreview>,
    /// Something the examiner must decide before converting (date order, time zone, CSV mapping).
    pub usage: Option<PreflightError>,
    /// The inputs could not be listed (missing path…); the run reports it itself.
    pub input_error: Option<String>,
}

impl PreviewState {
    /// Short name of the usage error, for the audit log ("needs_date_order"…).
    pub fn error_kind(&self) -> Option<&'static str> {
        match &self.usage {
            Some(PreflightError { error, .. }) => Some(match error {
                ParseError::NeedsDateOrder { .. } => "needs_date_order",
                ParseError::NeedsTimezone => "needs_timezone",
                ParseError::NeedsMapping(_) => "needs_mapping",
                ParseError::Invalid(_) => "invalid",
            }),
            None if self.input_error.is_some() => Some("inputs"),
            None => None,
        }
    }
}

/// Live conversion counters of the running job.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConversionProgress {
    /// Export being converted: (item index, path, source).
    pub current: Option<(u64, String, String)>,
    pub exports_done: u64,
    pub conversations: u64,
    pub events: u64,
    pub files: u64,
    pub invalid: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Idle,
    Running,
    Finished,
    Cancelled,
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub elapsed_ms: u64,
    pub eta_ms: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Outcome {
    pub items: u64,
    pub errors: u64,
    pub high: u64,
    pub medium: u64,
    pub low: u64,
    pub info: u64,
    pub report: PathBuf,
    pub log_dir: PathBuf,
    /// Conversion totals (from the run summary; live counters until it arrives).
    pub conversion: ConversionTotals,
    /// Folder with the .rsmf files, trace.csv/json and conversions.json.
    pub rsmf_dir: Option<PathBuf>,
}

/// One item's link to the case registry, shown in the result panel.
#[derive(Clone, Debug, PartialEq)]
pub struct EvidenceLink {
    pub index: u64,
    pub path: String,
    pub evidence_id: Option<String>,
    /// registered | already_registered | verified | mismatch | not_in_registry
    pub status: String,
}

pub const LANG_CHOICES: [(&str, &str); 4] = [
    ("en", "English"),
    ("es", "Español"),
    ("fr", "Français"),
    ("all", "All three (EN + ES + FR)"),
];

#[derive(Clone, Debug)]
pub struct GuiState {
    pub inputs: Vec<PathBuf>,
    pub path_entry: String,
    pub recursive: bool,
    pub selected: Vec<HashAlgo>,
    pub lang: String,
    pub out_dir: String,
    pub actor: String,
    pub case_id: String,
    pub phase: Phase,
    pub progress: Progress,
    pub current_item: String,
    pub warnings: Vec<String>,
    pub outcome: Option<Outcome>,
    /// User actions recorded before the run; handed to the core as audit preamble entries.
    pub ui_actions: Vec<(String, Value)>,
    /// Interface language (en/es/fr), independent from the report language.
    pub ui_lang: String,
    pub jurisdiction: String,
    pub timestamp: bool,
    pub tsa_url: String,
    pub sign_cert: String,
    /// Kept in memory only; never logged.
    pub sign_password: String,
    /// Shared case folder; when set, the output folder is derived from it and locked.
    pub case_dir: String,
    /// Evidence ids reported by the last run (case folder only).
    pub evidence: Vec<EvidenceLink>,
    /// Case folder only: register the evidence with RV HashInv before converting (`--intake hashinv`).
    pub intake_hashinv: bool,
    /// Result of looking for rv-hashinv when the option was switched on (path, version, SHA-256) or why not.
    pub hashinv: Option<Result<IntakeTool, String>>,
    /// Intake progress of the running job: (done, total).
    pub intake_progress: Option<(u64, u64)>,
    /// Where rv-hashinv wrote its hash reports in the last run.
    pub intake_report_dir: Option<String>,
    // --- Chat export (conversion) settings ---
    /// Forced source type; None = detect.
    pub source: Option<SourceKind>,
    /// IANA time zone name (validated); empty = none given.
    pub timezone: String,
    /// Why the last typed time zone was refused.
    pub timezone_error: Option<String>,
    pub date_order: Option<DateOrder>,
    pub custodian: String,
    pub csv_map: String,
    pub csv_profile: CsvProfile,
    pub platform_name: String,
    /// Last preview for the current settings (None = not run since the last relevant change).
    pub preview: Option<PreviewState>,
    /// A preview found dates whose day/month order cannot be decided: the selector must be set.
    pub date_order_required: bool,
    /// The lines that showed the ambiguity.
    pub date_order_evidence: Vec<String>,
    /// A preview found local times without offset: the time zone must be given.
    pub timezone_required: bool,
    /// Participants found by previews of the current inputs (custodian suggestions).
    pub known_participants: Vec<String>,
    /// Source types found by previews of the current inputs.
    pub known_sources: BTreeSet<SourceKind>,
    /// Conversion counters of the running job.
    pub conversion: ConversionProgress,
}

impl Default for GuiState {
    fn default() -> Self {
        GuiState {
            inputs: vec![],
            path_entry: String::new(),
            recursive: true,
            selected: vec![HashAlgo::Sha256],
            lang: "en".into(),
            out_dir: "rakvor-out".into(),
            actor: rv_rsmf_core::env::os_user(),
            case_id: String::new(),
            phase: Phase::Idle,
            progress: Progress::default(),
            current_item: String::new(),
            warnings: vec![],
            outcome: None,
            ui_actions: vec![],
            ui_lang: "en".into(),
            jurisdiction: "co".into(),
            timestamp: true,
            tsa_url: rv_rsmf_core::trust::DEFAULT_TSA.into(),
            sign_cert: String::new(),
            sign_password: String::new(),
            case_dir: String::new(),
            evidence: vec![],
            intake_hashinv: false,
            hashinv: None,
            intake_progress: None,
            intake_report_dir: None,
            source: None,
            timezone: String::new(),
            timezone_error: None,
            date_order: None,
            custodian: String::new(),
            csv_map: String::new(),
            csv_profile: CsvProfile::Generic,
            platform_name: String::new(),
            preview: None,
            date_order_required: false,
            date_order_evidence: vec![],
            timezone_required: false,
            known_participants: vec![],
            known_sources: BTreeSet::new(),
            conversion: ConversionProgress::default(),
        }
    }
}

fn opt_str(s: &str) -> Value {
    if s.is_empty() { Value::Null } else { json!(s) }
}

/// What the preview panel shows for one export.
pub fn summarize(
    primary: &std::path::Path,
    kind: SourceKind,
    parsed: Option<&rv_rsmf_core::model::ParsedExport>,
    error: Option<String>,
) -> ExportPreview {
    let mut out = ExportPreview {
        path: primary.display().to_string(),
        source: kind,
        facts: BTreeMap::new(),
        conversations: 0,
        participants: vec![],
        events: 0,
        first: vec![],
        error,
    };
    if let Some(p) = parsed {
        out.facts = p.facts.clone();
        out.conversations = p.conversations.len();
        out.participants = p.participants.iter().map(|x| x.display.clone()).collect();
        out.events = p.events.len();
        out.first = sources::preview_events(p, PREVIEW_ROWS)
            .into_iter()
            .map(|ev| PreviewRow {
                source_time: ev.source_time.clone(),
                utc: ev
                    .timestamp
                    .as_ref()
                    .map(rsmf_time)
                    .unwrap_or_else(|| "\u{2014}".into()),
                sender: ev
                    .participant
                    .as_ref()
                    .and_then(|id| p.participant(id))
                    .map(|x| x.display.clone())
                    .unwrap_or_else(|| "\u{2014}".into()),
                body: preview_body(&ev.body),
            })
            .collect();
    }
    out
}

/// Message text for the preview table: one line, at most PREVIEW_BODY_CHARS characters.
pub fn preview_body(body: &str) -> String {
    let one_line = body.replace(['\r', '\n'], " ");
    if one_line.chars().count() <= PREVIEW_BODY_CHARS {
        one_line
    } else {
        let mut s: String = one_line.chars().take(PREVIEW_BODY_CHARS - 1).collect();
        s.push('\u{2026}');
        s
    }
}

impl GuiState {
    /// Records a user action with the time it happened (written to the audit log when the run starts).
    fn note(&mut self, action: &str, mut details: Value) {
        if let Some(o) = details.as_object_mut() {
            o.insert("at_utc".into(), json!(rv_rsmf_core::time::utc_now()));
        }
        self.ui_actions.push((action.to_string(), details));
    }

    pub fn is_running(&self) -> bool {
        self.phase == Phase::Running
    }

    /// A checkbox is clickable if it can be unticked (more than one ticked) or ticked (fewer than two ticked).
    pub fn can_toggle(&self, a: HashAlgo) -> bool {
        if self.selected.contains(&a) {
            self.selected.len() > 1
        } else {
            self.selected.len() < HashAlgo::MAX_SELECTED
        }
    }

    pub fn toggle_algo(&mut self, a: HashAlgo) -> Result<(), String> {
        if !self.can_toggle(a) {
            return Err(if self.selected.contains(&a) {
                "At least one hash algorithm is required.".into()
            } else {
                format!(
                    "At most {} hash algorithms can be selected.",
                    HashAlgo::MAX_SELECTED
                )
            });
        }
        if let Some(i) = self.selected.iter().position(|x| *x == a) {
            self.selected.remove(i);
        } else {
            self.selected.push(a);
        }
        self.note(
            "ui_hash_selection_changed",
            json!({"algorithms": self.selected}),
        );
        Ok(())
    }

    pub fn add_input(&mut self, p: PathBuf) {
        if !self.inputs.contains(&p) {
            self.note("ui_input_added", json!({"path": p.display().to_string()}));
            self.inputs.push(p);
            self.inputs_changed();
        }
    }

    /// Different evidence: what earlier previews found no longer applies.
    fn inputs_changed(&mut self) {
        self.preview = None;
        self.date_order_required = false;
        self.date_order_evidence.clear();
        self.timezone_required = false;
        self.known_participants.clear();
        self.known_sources.clear();
    }

    // ----- Chat export settings (each one audited; a change that affects parsing clears the preview) -----

    pub fn set_source(&mut self, s: Option<SourceKind>) {
        if self.source != s {
            self.source = s;
            self.preview = None;
            self.note(
                "ui_source_changed",
                json!({"source": s.map(|k| k.name()).unwrap_or("auto")}),
            );
        }
    }

    /// Sets the device time zone. An unknown IANA name is refused (and the refusal is audited);
    /// an empty value means "none given".
    pub fn set_timezone(&mut self, tz: &str) -> Result<(), String> {
        let tz = tz.trim().to_string();
        if !tz.is_empty() {
            // The target type (chrono_tz::Tz) is taken from the ParseOptions field.
            let probe = ParseOptions {
                timezone: tz.parse().ok(),
                ..Default::default()
            };
            if probe.timezone.is_none() {
                let msg = format!(
                    "Unknown time zone '{tz}': use an IANA name such as Europe/Paris or America/Bogota."
                );
                self.timezone_error = Some(msg.clone());
                self.note("ui_timezone_rejected", json!({"value": tz}));
                return Err(msg);
            }
        }
        self.timezone_error = None;
        if self.timezone != tz {
            self.timezone = tz.clone();
            self.preview = None;
            self.note("ui_timezone_changed", json!({"timezone": opt_str(&tz)}));
        }
        Ok(())
    }

    pub fn set_date_order(&mut self, d: Option<DateOrder>) {
        if self.date_order != d {
            self.date_order = d;
            self.preview = None;
            self.note(
                "ui_date_order_changed",
                json!({"date_order": d.map(|o| o.name()).unwrap_or("auto")}),
            );
        }
    }

    pub fn set_custodian(&mut self, c: &str) {
        let c = c.trim().to_string();
        if self.custodian != c {
            self.custodian = c.clone();
            self.preview = None;
            self.note("ui_custodian_changed", json!({"custodian": opt_str(&c)}));
        }
    }

    pub fn set_csv_map(&mut self, p: &str) {
        let p = p.trim().trim_matches('"').to_string();
        if self.csv_map != p {
            self.csv_map = p.clone();
            self.preview = None;
            self.note("ui_csv_map_changed", json!({"path": opt_str(&p)}));
        }
    }

    pub fn set_csv_profile(&mut self, p: CsvProfile) {
        if self.csv_profile != p {
            self.csv_profile = p;
            self.preview = None;
            self.note(
                "ui_csv_profile_changed",
                json!({"profile": p.name(), "draft": p.is_draft()}),
            );
        }
    }

    /// Only changes the RSMF platform label: the preview stays valid.
    pub fn set_platform_name(&mut self, n: &str) {
        let n = n.trim().to_string();
        if self.platform_name != n {
            self.platform_name = n.clone();
            self.note(
                "ui_platform_name_changed",
                json!({"platform_name": opt_str(&n)}),
            );
        }
    }

    /// The conversion options the settings stand for.
    pub fn convert_options(&self) -> ConvertOptions {
        let some = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
        ConvertOptions {
            parse: ParseOptions {
                source: self.source,
                // Validated by set_timezone; an unparsable value cannot be stored.
                timezone: self.timezone.parse().ok(),
                date_order: self.date_order,
                csv_map: some(&self.csv_map).map(PathBuf::from),
                csv_profile: self.csv_profile,
                custodian: some(&self.custodian),
            },
            platform_name: some(&self.platform_name),
        }
    }

    /// CSV options are shown when a CSV export is among the inputs (or CSV is forced).
    pub fn show_csv_options(&self) -> bool {
        self.source == Some(SourceKind::Csv)
            || self.known_sources.contains(&SourceKind::Csv)
            || self.inputs.iter().any(|p| {
                p.extension()
                    .map(|e| e.to_string_lossy().to_ascii_lowercase())
                    .is_some_and(|e| e == "csv" || e == "tsv")
            })
            || matches!(
                self.preview.as_ref().and_then(|p| p.usage.as_ref()),
                Some(PreflightError {
                    error: ParseError::NeedsMapping(_),
                    ..
                })
            )
    }

    /// The date-order selector is highlighted: a preview found ambiguous dates and no order is chosen.
    pub fn date_order_missing(&self) -> bool {
        self.date_order_required && self.date_order.is_none()
    }

    /// The time-zone field is highlighted: a preview found local times and no zone is given.
    pub fn timezone_missing(&self) -> bool {
        self.timezone_required && self.timezone.is_empty()
    }

    /// Reads the exports with the current settings (nothing is written) and keeps what the panel shows.
    pub fn preview(&mut self) {
        self.preview_with("button");
    }

    fn preview_with(&mut self, trigger: &str) {
        let opts = self.convert_options();
        let result = match rv_rsmf_core::input::collect(&self.inputs, None, self.recursive) {
            Err(e) => Err(e),
            Ok(items) => {
                let files: Vec<PathBuf> = items.into_iter().map(|i| i.path).collect();
                Ok(convert::preflight(&files, &opts).map(|exports| {
                    exports
                        .into_iter()
                        .map(|(e, parsed)| {
                            let error = match &parsed {
                                Some(_) => None,
                                // Re-read only to say why (fast for damaged files; nothing is written).
                                None => Some(
                                    sources::parse(&e, &opts.parse)
                                        .err()
                                        .map(|x| x.to_string())
                                        .unwrap_or_default(),
                                ),
                            };
                            summarize(&e.primary, e.kind, parsed.as_ref(), error)
                        })
                        .collect::<Vec<_>>()
                }))
            }
        };
        self.set_preview(result, trigger);
    }

    /// Stores a preview result (separate from `preview` so the rules are testable with any parser).
    pub fn set_preview(
        &mut self,
        result: Result<Result<Vec<ExportPreview>, PreflightError>, String>,
        trigger: &str,
    ) {
        let state = match result {
            Err(e) => PreviewState {
                input_error: Some(e),
                ..Default::default()
            },
            Ok(Err(pe)) => PreviewState {
                usage: Some(pe),
                ..Default::default()
            },
            Ok(Ok(exports)) => PreviewState {
                exports,
                ..Default::default()
            },
        };
        if let Some(PreflightError { error, .. }) = &state.usage {
            match error {
                ParseError::NeedsDateOrder { evidence, .. } => {
                    self.date_order_required = true;
                    self.date_order_evidence = evidence.clone();
                }
                ParseError::NeedsTimezone => self.timezone_required = true,
                _ => {}
            }
        }
        for e in &state.exports {
            self.known_sources.insert(e.source);
            for p in &e.participants {
                if !self.known_participants.contains(p) {
                    self.known_participants.push(p.clone());
                }
            }
        }
        self.note(
            "ui_preview_run",
            json!({
                "trigger": trigger,
                "exports": state.exports.len(),
                "failed": state.exports.iter().filter(|e| e.error.is_some()).count(),
                "error": state.error_kind(),
                "path": state.usage.as_ref().map(|u| u.path.display().to_string()),
                "options": self.convert_options().to_json(),
            }),
        );
        self.preview = Some(state);
    }

    /// The last preview (for the current settings) found something the examiner must decide.
    pub fn preview_blocks(&self) -> bool {
        self.preview.as_ref().is_some_and(|p| p.usage.is_some())
            || self.date_order_missing()
            || self.timezone_missing()
    }

    /// Before starting: preview if the settings changed since the last one; refuse on a usage problem.
    pub fn check_exports(&mut self) -> Result<(), String> {
        if self.preview.is_none() && !self.inputs.is_empty() {
            self.preview_with("start");
        }
        match self.preview.as_ref().and_then(|p| p.usage.as_ref()) {
            Some(u) => Err(u.to_string()),
            None if self.preview_blocks() => Err(self.tr("preview_blocked")),
            None => Ok(()),
        }
    }

    /// Switches the RV HashInv intake on or off; switching it on looks for rv-hashinv right away so the
    /// window can say which copy will run (or why none can).
    pub fn set_intake_hashinv(&mut self, on: bool) {
        if self.intake_hashinv == on {
            return;
        }
        self.intake_hashinv = on;
        self.hashinv = on.then(|| rv_rsmf_core::intake::locate(None));
        let found = match &self.hashinv {
            Some(Ok(t)) => {
                json!({"path": t.path.display().to_string(), "version": t.version, "sha256": t.sha256})
            }
            Some(Err(e)) => json!({"error": e}),
            None => Value::Null,
        };
        self.note(
            "ui_intake_changed",
            json!({"intake": if on { "hashinv" } else { "none" }, "hashinv": found}),
        );
    }

    /// The intake runs only with a case folder.
    pub fn intake_active(&self) -> bool {
        self.intake_hashinv && self.out_locked()
    }

    /// Running-job line: "Converting chat.txt (whatsapp) · 3 RSMF files written (1 invalid)".
    pub fn conversion_text(&self) -> Option<String> {
        let c = &self.conversion;
        if c.current.is_none() && c.files == 0 && c.exports_done == 0 {
            return None;
        }
        let mut s = match &c.current {
            Some((_, path, source)) => {
                let name = std::path::Path::new(path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone());
                format!(
                    "{}: {} ({})  \u{00B7}  ",
                    self.tr("converting"),
                    name,
                    source
                )
            }
            None => String::new(),
        };
        s.push_str(&format!("{}: {}", self.tr("rsmf_written"), c.files));
        if c.invalid > 0 {
            s.push_str(&format!(" ({} {})", c.invalid, self.tr("invalid")));
        }
        Some(s)
    }

    /// Keeps the run summary's conversion totals and RSMF folder in the outcome.
    pub fn finish(&mut self, s: &RunSummary) {
        if let Some(o) = self.outcome.as_mut() {
            o.conversion = s.conversion.clone();
            o.rsmf_dir = Some(s.rsmf_dir.clone());
        }
    }

    pub fn add_typed_path(&mut self) {
        let t = self.path_entry.trim().trim_matches('"').to_string();
        if !t.is_empty() {
            self.add_input(PathBuf::from(t));
            self.path_entry.clear();
        }
    }

    pub fn remove_input(&mut self, i: usize) {
        if i < self.inputs.len() {
            let p = self.inputs.remove(i);
            self.note("ui_input_removed", json!({"path": p.display().to_string()}));
            self.inputs_changed();
        }
    }

    pub fn set_lang(&mut self, l: &str) {
        if self.lang != l {
            self.lang = l.to_string();
            self.note("ui_language_changed", json!({"lang": l}));
        }
    }

    pub fn set_recursive(&mut self, r: bool) {
        if self.recursive != r {
            self.recursive = r;
            self.note("ui_recursive_changed", json!({"recursive": r}));
            self.inputs_changed();
        }
    }

    /// GUI text in the interface language.
    pub fn tr(&self, key: &str) -> String {
        i18n::catalog(&self.ui_lang).ui(key)
    }

    /// Combo text for a source choice (None = detect).
    pub fn source_label(&self, s: Option<SourceKind>) -> String {
        match s {
            None => self.tr("source_auto"),
            Some(k) => self.tr(&format!("source_{}", k.name().replace('-', "_"))),
        }
    }

    /// Combo text for a date-order choice (None = detect from the file).
    pub fn date_order_label(&self, d: Option<DateOrder>) -> String {
        match d {
            None => self.tr("date_auto"),
            Some(o) => self.tr(&format!("date_{}", o.name())),
        }
    }

    pub fn csv_profile_label(&self, p: CsvProfile) -> String {
        self.tr(&format!("csv_{}", p.name().replace('-', "_")))
    }

    /// The help line shown in amber for the preview's usage error.
    pub fn usage_help(&self) -> Option<String> {
        let u = self.preview.as_ref()?.usage.as_ref()?;
        Some(match &u.error {
            ParseError::NeedsDateOrder { .. } => self.tr("needs_date_order"),
            ParseError::NeedsTimezone => self.tr("needs_timezone"),
            ParseError::NeedsMapping(m) => format!("{} ({m})", self.tr("needs_mapping")),
            ParseError::Invalid(m) => m.clone(),
        })
    }

    pub fn set_ui_lang(&mut self, l: &str) {
        if self.ui_lang != l && ["en", "es", "fr"].contains(&l) {
            self.ui_lang = l.to_string();
            self.note("ui_interface_language_changed", json!({"ui_lang": l}));
        }
    }

    pub fn set_jurisdiction(&mut self, j: &str) {
        if self.jurisdiction != j && (j == "co" || j == "fr") {
            self.jurisdiction = j.to_string();
            self.note("ui_jurisdiction_changed", json!({"jurisdiction": j}));
        }
    }

    pub fn set_timestamp(&mut self, on: bool, url: &str) {
        let url = url.trim().to_string();
        if self.timestamp != on || self.tsa_url != url {
            self.timestamp = on;
            self.tsa_url = url.clone();
            self.note(
                "ui_timestamp_changed",
                json!({"enabled": on, "tsa_url": url}),
            );
        }
    }

    pub fn set_sign_cert(&mut self, path: &str) {
        let path = path.trim().trim_matches('"').to_string();
        if self.sign_cert != path {
            self.sign_cert = path.clone();
            self.note(
                "ui_signing_certificate_changed",
                json!({"path": if path.is_empty() { Value::Null } else { json!(path) }}),
            );
        }
    }

    /// Rule: with a case folder the output folder is `<case>/runs/<tool>/<RUN-ID>` and cannot be edited.
    pub fn set_case_dir(&mut self, path: &str) {
        let path = path.trim().trim_matches('"').to_string();
        if self.case_dir != path {
            self.case_dir = path.clone();
            self.note(
                "ui_case_dir_changed",
                json!({"path": if path.is_empty() { Value::Null } else { json!(path) }, "role": CASE_ROLE.name()}),
            );
        }
    }

    pub fn out_locked(&self) -> bool {
        !self.case_dir.trim().is_empty()
    }

    /// What the output-folder field shows: the derived run folder when a case folder is set.
    pub fn out_dir_display(&self) -> String {
        if self.out_locked() {
            PathBuf::from(self.case_dir.trim())
                .join("runs")
                .join(TOOL_NAME)
                .join("<RUN-ID>")
                .display()
                .to_string()
        } else {
            self.out_dir.clone()
        }
    }

    /// Result-panel lines: "E0001  acta.txt  — Verified: identical to the registered evidence".
    pub fn evidence_lines(&self) -> Vec<String> {
        let c = i18n::catalog(&self.ui_lang);
        self.evidence
            .iter()
            .map(|e| {
                let name = std::path::Path::new(&e.path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| e.path.clone());
                format!(
                    "{}  {}  \u{2014} {}",
                    e.evidence_id.as_deref().unwrap_or("\u{2014}"),
                    name,
                    c.label(&format!("case_status_{}", e.status))
                )
            })
            .collect()
    }

    pub fn can_start(&self) -> bool {
        !self.is_running()
            && (!self.inputs.is_empty() || (self.out_locked() && !self.intake_active()))
            && (self.out_locked() || !self.out_dir.trim().is_empty())
            && !self.preview_blocks()
    }

    /// Builds the run configuration; the recorded UI actions become audit preamble entries.
    pub fn to_config(&self) -> Result<RunConfig, String> {
        if self.inputs.is_empty() && (!self.out_locked() || self.intake_active()) {
            return Err("Add at least one file or folder.".into());
        }
        let hashinv_exe = if self.intake_active() {
            match &self.hashinv {
                Some(Ok(t)) => Some(t.path.clone()),
                Some(Err(e)) => return Err(e.clone()),
                None => None,
            }
        } else {
            None
        };
        hash::validate_selection(&self.selected)?;
        let convert = self.convert_options();
        if let Some(m) = &convert.parse.csv_map
            && !m.is_file()
        {
            return Err(format!("CSV mapping file not found: {}", m.display()));
        }
        let case_id = Some(self.case_id.trim().to_string()).filter(|s| !s.is_empty());
        // With the intake the case may not exist yet: rv-hashinv creates it and the core checks it afterwards.
        if self.out_locked() && !self.intake_active() {
            case::preflight(
                std::path::Path::new(self.case_dir.trim()),
                case_id.as_deref(),
                &[],
            )
            .map_err(|e| e.message)?;
        }
        let langs = i18n::parse_langs(&self.lang)?;
        if !self.sign_cert.is_empty() && !std::path::Path::new(&self.sign_cert).is_file() {
            return Err(format!("Signing certificate not found: {}", self.sign_cert));
        }
        let mut preamble = self.ui_actions.clone();
        preamble.push(("ui_start_clicked".into(), json!({"algorithms": self.selected, "lang": self.lang, "recursive": self.recursive,
            "inputs": self.inputs.len(), "jurisdiction": self.jurisdiction, "timestamp": self.timestamp,
            "pdf_signing": !self.sign_cert.is_empty(), "case_dir": if self.out_locked() { json!(self.case_dir.trim()) } else { Value::Null },
            "intake": if self.intake_active() { "hashinv" } else { "none" }, "convert": convert.to_json(),
            "at_utc": rv_rsmf_core::time::utc_now()})));
        let cv = convert.to_json();
        let cv_str = |k: &str| match &cv[k] {
            Value::String(s) => s.clone(),
            _ => "none".into(),
        };
        let settings = vec![
            format!("{} (gui)", TOOL_NAME),
            format!(
                "inputs={}",
                self.inputs
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(";")
            ),
            format!(
                "hash={}",
                self.selected
                    .iter()
                    .map(|a| a.name())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            format!("lang={}", self.lang),
            format!("recursive={}", self.recursive),
            format!("out={}", self.out_dir_display().trim()),
            format!(
                "case_dir={}",
                if self.out_locked() {
                    self.case_dir.trim()
                } else {
                    "none"
                }
            ),
            format!(
                "intake={}",
                if self.intake_active() {
                    "hashinv"
                } else {
                    "none"
                }
            ),
            format!("source={}", self.source.map(|k| k.name()).unwrap_or("auto")),
            format!("timezone={}", cv_str("timezone")),
            format!(
                "date_order={}",
                self.date_order.map(|d| d.name()).unwrap_or("auto")
            ),
            format!("custodian={}", cv_str("custodian")),
            format!("csv_map={}", cv_str("csv_map")),
            format!("csv_profile={}", self.csv_profile.name()),
            format!("platform_name={}", cv_str("platform_name")),
            format!("jurisdiction={}", self.jurisdiction),
            format!(
                "tsa={}",
                if self.timestamp {
                    self.tsa_url.as_str()
                } else {
                    "off"
                }
            ),
            format!(
                "sign_cert={}",
                if self.sign_cert.is_empty() {
                    "none"
                } else {
                    self.sign_cert.as_str()
                }
            ),
        ];
        Ok(RunConfig {
            inputs: self.inputs.clone(),
            input_list: None,
            recursive: self.recursive,
            algorithms: self.selected.clone(),
            out_dir: PathBuf::from(self.out_dir.trim()),
            langs,
            actor: if self.actor.trim().is_empty() {
                rv_rsmf_core::env::os_user()
            } else {
                self.actor.trim().to_string()
            },
            case_id,
            frontend: "gui".into(),
            preamble,
            command_line: settings,
            tool_name: TOOL_NAME.into(),
            tool_version: TOOL_VERSION.into(),
            jurisdiction: self.jurisdiction.clone(),
            tsa_url: if self.timestamp {
                Some(self.tsa_url.clone())
            } else {
                None
            },
            sign_cert: if self.sign_cert.is_empty() {
                None
            } else {
                Some(PathBuf::from(&self.sign_cert))
            },
            sign_password: if self.sign_password.is_empty() {
                None
            } else {
                Some(self.sign_password.clone())
            },
            case_dir: if self.out_locked() {
                Some(PathBuf::from(self.case_dir.trim()))
            } else {
                None
            },
            evidence_ids: vec![],
            case_role: CASE_ROLE,
            convert,
            intake_hashinv: self.intake_active(),
            hashinv_exe,
        })
    }

    pub fn start(&mut self) {
        self.phase = Phase::Running;
        self.progress = Progress::default();
        self.current_item.clear();
        self.warnings.clear();
        self.outcome = None;
        self.evidence.clear();
        self.intake_progress = None;
        self.intake_report_dir = None;
        self.conversion = ConversionProgress::default();
        // Recorded actions now belong to this run's audit log.
        self.ui_actions.clear();
    }

    pub fn apply(&mut self, e: &Event) {
        match e {
            Event::RunStarted {
                total_items,
                total_bytes,
                ..
            } => {
                self.progress.total = *total_items;
                self.progress.bytes_total = *total_bytes;
            }
            Event::ItemStarted { path, .. } => self.current_item = path.clone(),
            Event::IntakeStarted { version, .. } => {
                self.current_item = format!("RV HashInv {version}");
                self.intake_progress = Some((0, 0));
            }
            Event::IntakeProgress { done, total, .. } => {
                self.intake_progress = Some((*done, *total))
            }
            Event::IntakeFinished { report_dir, .. } => self.intake_report_dir = report_dir.clone(),
            Event::ExportStarted {
                index,
                path,
                source,
            } => {
                self.conversion.current = Some((*index, path.clone(), source.clone()));
                self.current_item = path.clone();
            }
            Event::RsmfWritten { valid, .. } => {
                self.conversion.files += 1;
                if !valid {
                    self.conversion.invalid += 1;
                }
            }
            Event::ExportFinished {
                conversations,
                events,
                ..
            } => {
                self.conversion.current = None;
                self.conversion.exports_done += 1;
                self.conversion.conversations += conversations;
                self.conversion.events += events;
            }
            Event::EvidenceLinked {
                index,
                path,
                evidence_id,
                status,
            } => self.evidence.push(EvidenceLink {
                index: *index,
                path: path.clone(),
                evidence_id: evidence_id.clone(),
                status: status.clone(),
            }),
            Event::Progress {
                done,
                total,
                bytes_done,
                bytes_total,
                elapsed_ms,
                eta_ms,
            } => {
                self.progress = Progress {
                    done: *done,
                    total: *total,
                    bytes_done: *bytes_done,
                    bytes_total: *bytes_total,
                    elapsed_ms: *elapsed_ms,
                    eta_ms: *eta_ms,
                };
            }
            Event::Error { path, message } => {
                self.warnings
                    .push(format!("{}: {}", path.as_deref().unwrap_or("-"), message))
            }
            Event::RunFinished {
                items,
                errors,
                high,
                medium,
                low,
                info,
                report_dir,
                log_dir,
                cancelled,
                elapsed_ms,
                ..
            } => {
                self.progress.elapsed_ms = *elapsed_ms;
                self.progress.eta_ms = None;
                let lang = if self.lang == "all" {
                    "en".to_string()
                } else {
                    self.lang.clone()
                };
                self.outcome = Some(Outcome {
                    items: *items,
                    errors: *errors,
                    high: *high,
                    medium: *medium,
                    low: *low,
                    info: *info,
                    report: PathBuf::from(report_dir).join(format!("statistics.{}.html", lang)),
                    log_dir: PathBuf::from(log_dir),
                    // Live counters until the run summary arrives (`finish`).
                    conversion: ConversionTotals {
                        exports: self.conversion.exports_done as usize,
                        conversations: self.conversion.conversations as usize,
                        events: self.conversion.events as usize,
                        files: self.conversion.files as usize,
                        files_invalid: self.conversion.invalid as usize,
                        ..Default::default()
                    },
                    rsmf_dir: None,
                });
                self.conversion.current = None;
                self.phase = if *cancelled {
                    Phase::Cancelled
                } else {
                    Phase::Finished
                };
                self.current_item.clear();
            }
            _ => {}
        }
    }

    pub fn fail(&mut self, msg: String) {
        self.phase = Phase::Failed(msg);
    }

    /// 0.0..=1.0, by bytes (or by files when every file is empty).
    pub fn fraction(&self) -> f32 {
        let p = &self.progress;
        if p.bytes_total > 0 {
            (p.bytes_done as f64 / p.bytes_total as f64).clamp(0.0, 1.0) as f32
        } else if p.total > 0 {
            (p.done as f64 / p.total as f64) as f32
        } else if matches!(self.phase, Phase::Finished) {
            1.0
        } else {
            0.0
        }
    }

    /// `live_elapsed_ms` is measured by the window clock so the timer keeps moving between events.
    pub fn timing_text(&self, live_elapsed_ms: Option<u64>) -> String {
        let elapsed = live_elapsed_ms
            .unwrap_or(self.progress.elapsed_ms)
            .max(self.progress.elapsed_ms);
        let eta = match self.progress.eta_ms {
            Some(ms) if self.is_running() => human_duration(ms),
            _ => "\u{2014}".into(),
        };
        format!(
            "{}: {}   \u{00B7}   {}: {}",
            self.tr("elapsed"),
            human_duration(elapsed),
            self.tr("remaining"),
            eta
        )
    }

    pub fn progress_text(&self) -> String {
        format!(
            "{} / {} {}  \u{00B7}  {:.0}%",
            self.progress.done,
            self.progress.total,
            self.tr("files"),
            self.fraction() * 100.0
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn at_most_two_and_at_least_one_algorithm() {
        let mut s = GuiState::default();
        assert_eq!(s.selected, vec![HashAlgo::Sha256]);
        assert!(
            !s.can_toggle(HashAlgo::Sha256),
            "cannot untick the last one"
        );
        assert!(s.toggle_algo(HashAlgo::Sha256).is_err());
        s.toggle_algo(HashAlgo::Md5).unwrap();
        assert!(
            !s.can_toggle(HashAlgo::Blake3),
            "third checkbox is disabled"
        );
        assert!(s.toggle_algo(HashAlgo::Blake3).is_err());
        assert!(s.can_toggle(HashAlgo::Sha256));
        s.toggle_algo(HashAlgo::Sha256).unwrap();
        assert_eq!(s.selected, vec![HashAlgo::Md5]);
    }

    #[test]
    fn settings_changes_become_audit_preamble() {
        let mut s = GuiState::default();
        assert!(s.to_config().is_err(), "no input yet");
        s.path_entry = "\"C:\\evidence\"".into();
        s.add_typed_path();
        s.toggle_algo(HashAlgo::Blake3).unwrap();
        s.set_lang("es");
        let cfg = s.to_config().unwrap();
        assert_eq!(cfg.inputs, vec![PathBuf::from("C:\\evidence")]);
        assert_eq!(cfg.frontend, "gui");
        let actions: Vec<&str> = cfg.preamble.iter().map(|(a, _)| a.as_str()).collect();
        assert_eq!(
            actions,
            vec![
                "ui_input_added",
                "ui_hash_selection_changed",
                "ui_language_changed",
                "ui_start_clicked"
            ]
        );
        s.start();
        assert!(s.ui_actions.is_empty());
    }

    #[test]
    fn events_drive_progress_and_outcome() {
        let mut s = GuiState::default();
        s.start();
        s.apply(&Event::RunStarted {
            run_id: "R".into(),
            tool: "t".into(),
            version: "v".into(),
            frontend: "gui".into(),
            algorithms: vec![HashAlgo::Sha256],
            langs: vec!["en".into()],
            total_items: 4,
            total_bytes: 400,
        });
        s.apply(&Event::Progress {
            done: 1,
            total: 4,
            bytes_done: 100,
            bytes_total: 400,
            elapsed_ms: 2000,
            eta_ms: Some(6000),
        });
        assert!((s.fraction() - 0.25).abs() < 1e-6);
        assert!(s.timing_text(Some(2500)).contains("6.0 s"));
        assert!(s.progress_text().starts_with("1 / 4"));
        s.apply(&Event::RunFinished {
            items: 4,
            bytes: 400,
            errors: 0,
            elapsed_ms: 8000,
            high: 1,
            medium: 0,
            low: 0,
            info: 0,
            report_dir: "out/reports/R".into(),
            log_dir: "out/logs/R".into(),
            cancelled: false,
        });
        assert_eq!(s.phase, Phase::Finished);
        assert_eq!(
            s.outcome.as_ref().unwrap().report,
            PathBuf::from("out/reports/R/statistics.en.html")
        );
    }

    #[test]
    fn interface_language_and_trust_settings_are_audited() {
        let mut s = GuiState::default();
        s.set_ui_lang("fr");
        assert_eq!(s.tr("start"), "Lancer l'analyse");
        s.set_ui_lang("es");
        assert_eq!(s.tr("start"), "Iniciar revisión");
        s.set_jurisdiction("fr");
        s.set_timestamp(false, "http://tsa.example");
        s.set_sign_cert("missing.p12");
        s.add_input(PathBuf::from("."));
        assert!(s.to_config().is_err(), "missing certificate refused");
        s.set_sign_cert("");
        let cfg = s.to_config().unwrap();
        assert_eq!(cfg.jurisdiction, "fr");
        assert!(cfg.tsa_url.is_none());
        let actions: Vec<&str> = cfg.preamble.iter().map(|(a, _)| a.as_str()).collect();
        assert!(
            actions.contains(&"ui_interface_language_changed")
                && actions.contains(&"ui_timestamp_changed")
        );
        assert!(
            cfg.preamble
                .iter()
                .all(|(_, d)| !d.to_string().contains("password"))
        );
    }

    #[test]
    fn case_folder_locks_the_output_folder_and_shows_evidence_ids() {
        let tmp = std::env::temp_dir().join(format!("rv-gui-case-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(tmp.join("ev")).unwrap();
        std::fs::write(tmp.join("ev/a.txt"), "a").unwrap();
        std::fs::write(tmp.join("ev/b.txt"), "b").unwrap();
        let mut s = GuiState {
            timestamp: false,
            ..Default::default()
        };
        assert!(!s.can_start());
        let case_dir = tmp.join("CASE-G");
        s.set_case_dir(&format!("\"{}\"", case_dir.display()));
        assert!(
            s.out_locked() && s.can_start(),
            "a case folder alone can start (registered items)"
        );
        assert!(s.out_dir_display().contains("runs") && s.out_dir_display().ends_with("<RUN-ID>"));
        assert!(s.to_config().is_err(), "creating a case needs a case id");
        s.case_id = "CASE-G".into();
        s.add_input(tmp.join("ev"));
        if CASE_ROLE == rv_rsmf_core::CaseRole::Analysis {
            // An analysis tool verifies against a registry: create it with an explicit intake run first.
            let seed = RunConfig {
                inputs: vec![tmp.join("ev")],
                case_id: Some("CASE-G".into()),
                case_dir: Some(case_dir.clone()),
                case_role: rv_rsmf_core::CaseRole::Intake,
                ..Default::default()
            };
            let off = std::sync::atomic::AtomicBool::new(false);
            rv_rsmf_core::run(&seed, &mut |_| {}, &off).unwrap();
        }
        let cfg = s.to_config().unwrap();
        assert_eq!(cfg.case_dir.as_deref(), Some(case_dir.as_path()));
        assert_eq!(cfg.case_role, CASE_ROLE);
        let actions: Vec<&str> = cfg.preamble.iter().map(|(a, _)| a.as_str()).collect();
        assert_eq!(actions[0], "ui_case_dir_changed");
        s.start();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let summary = rv_rsmf_core::run(&cfg, &mut |e| s.apply(e), &cancel).unwrap();
        assert_eq!(s.phase, Phase::Finished);
        assert!(summary.log_dir.starts_with(&case_dir));
        let ids: Vec<Option<&str>> = s
            .evidence
            .iter()
            .map(|e| e.evidence_id.as_deref())
            .collect();
        assert_eq!(ids, vec![Some("E0001"), Some("E0002")]);
        assert!(s.evidence_lines()[0].starts_with("E0001  a.txt"));
        let log = std::fs::read_to_string(summary.log_dir.join("audit.jsonl")).unwrap();
        assert!(log.contains("\"ui_case_dir_changed\"") && log.contains("\"case_linked\""));
        s.set_case_dir("");
        assert!(!s.out_locked() && s.out_dir_display() == "rakvor-out");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn a_real_run_from_gui_settings_is_audited() {
        let ev = std::env::temp_dir().join(format!("rv-gui-test-{}", std::process::id()));
        std::fs::create_dir_all(&ev).unwrap();
        std::fs::write(ev.join("a.txt"), "x").unwrap();
        let mut s = GuiState {
            timestamp: false, // tests stay offline
            ..Default::default()
        };
        s.add_input(ev.clone());
        s.out_dir = ev.join("out").display().to_string();
        let cfg = s.to_config().unwrap();
        s.start();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let summary = rv_rsmf_core::run(&cfg, &mut |e| s.apply(e), &cancel).unwrap();
        assert_eq!(s.phase, Phase::Finished);
        let log = std::fs::read_to_string(summary.log_dir.join("audit.jsonl")).unwrap();
        assert!(
            log.contains("\"ui_input_added\"")
                && log.contains("\"ui_start_clicked\"")
                && log.contains("\"frontend\":\"gui\"")
        );
        let _ = std::fs::remove_dir_all(&ev);
    }

    fn tmp_dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rv-gui-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn actions(s: &GuiState) -> Vec<&str> {
        s.ui_actions.iter().map(|(a, _)| a.as_str()).collect()
    }

    #[test]
    fn conversion_settings_are_audited_with_time() {
        let mut s = GuiState::default();
        s.set_source(Some(SourceKind::Whatsapp));
        s.set_timezone("America/Bogota").unwrap();
        s.set_date_order(Some(DateOrder::Dmy));
        s.set_custodian(" Ana ");
        s.set_csv_map("\"C:\\maps\\xry.toml\"");
        s.set_csv_profile(CsvProfile::XryDraft);
        s.set_platform_name("WhatsApp Business");
        // Setting the same value again records nothing.
        s.set_source(Some(SourceKind::Whatsapp));
        s.set_timezone("America/Bogota").unwrap();
        assert_eq!(
            actions(&s),
            vec![
                "ui_source_changed",
                "ui_timezone_changed",
                "ui_date_order_changed",
                "ui_custodian_changed",
                "ui_csv_map_changed",
                "ui_csv_profile_changed",
                "ui_platform_name_changed"
            ]
        );
        assert!(s.ui_actions.iter().all(|(_, d)| d["at_utc"].is_string()));
        assert_eq!(s.ui_actions[1].1["timezone"], "America/Bogota");
        assert_eq!(s.ui_actions[3].1["custodian"], "Ana");
        assert_eq!(s.ui_actions[4].1["path"], "C:\\maps\\xry.toml");
        assert_eq!(s.ui_actions[5].1["profile"], "xry-draft");
        s.set_source(None);
        assert_eq!(s.ui_actions.last().unwrap().1["source"], "auto");
    }

    #[test]
    fn invalid_time_zone_is_rejected_and_empty_means_none() {
        let mut s = GuiState::default();
        s.set_timezone("Europe/Paris").unwrap();
        let err = s.set_timezone("Mars/Olympus").unwrap_err();
        assert!(err.contains("Mars/Olympus"));
        assert_eq!(s.timezone, "Europe/Paris", "the valid zone is kept");
        assert!(s.timezone_error.is_some());
        assert_eq!(
            actions(&s),
            vec!["ui_timezone_changed", "ui_timezone_rejected"]
        );
        s.set_timezone("").unwrap();
        assert!(s.timezone.is_empty() && s.timezone_error.is_none());
        assert!(s.convert_options().parse.timezone.is_none());
        for z in COMMON_ZONES {
            s.set_timezone(z).unwrap();
            assert_eq!(
                s.convert_options()
                    .parse
                    .timezone
                    .map(|t| t.name().to_string()),
                Some(z.to_string())
            );
        }
    }

    #[test]
    fn ambiguous_dates_block_start_until_a_date_order_is_chosen() {
        let ev = tmp_dir("dmy");
        let chat = ev.join("chat.txt");
        std::fs::write(
            &chat,
            "01/02/2024, 10:15 - Ana: Hola\n03/04/2024, 11:20 - Luis: Buenas\n",
        )
        .unwrap();
        let mut s = GuiState::default();
        s.add_input(chat.clone());
        assert!(s.can_start());
        // The rule, independent of the parser: a NeedsDateOrder preview blocks the start.
        s.set_preview(
            Ok(Err(PreflightError {
                path: chat.clone(),
                error: ParseError::NeedsDateOrder {
                    candidates: vec![DateOrder::Dmy, DateOrder::Mdy],
                    evidence: vec!["line 1: 01/02/2024, 10:15".into()],
                },
            })),
            "button",
        );
        assert!(!s.can_start() && s.date_order_missing());
        assert_eq!(s.date_order_evidence, vec!["line 1: 01/02/2024, 10:15"]);
        assert!(s.usage_help().is_some());
        let rec = &s.ui_actions.last().unwrap();
        assert_eq!(rec.0, "ui_preview_run");
        assert_eq!(rec.1["error"], "needs_date_order");
        assert_eq!(rec.1["exports"], 0);
        assert!(rec.1["at_utc"].is_string());
        s.set_date_order(Some(DateOrder::Dmy));
        assert!(s.preview.is_none(), "a relevant change clears the preview");
        assert!(s.can_start() && !s.date_order_missing());
        s.set_date_order(None);
        assert!(!s.can_start(), "back to auto: still required");
        s.set_date_order(Some(DateOrder::Dmy));

        // The real parser on the same file.
        s.set_source(Some(SourceKind::Whatsapp));
        s.set_timezone("Europe/Paris").unwrap();
        s.set_date_order(None);
        s.date_order_required = false;
        s.preview();
        let p = s.preview.clone().unwrap();
        match &p.usage {
            Some(PreflightError {
                error: ParseError::NeedsDateOrder { .. },
                ..
            }) => {
                assert!(!s.can_start() && s.date_order_missing());
                assert!(s.check_exports().is_err());
                s.set_date_order(Some(DateOrder::Dmy));
                s.preview();
                let p = s.preview.as_ref().unwrap();
                assert!(p.usage.is_none(), "{:?}", p.usage);
                assert_eq!(p.exports.len(), 1);
                assert!(p.exports[0].first.len() <= PREVIEW_ROWS);
                assert!(s.can_start() && s.check_exports().is_ok());
            }
            None => panic!("the ambiguous WhatsApp file must ask for a date order"),
            Some(other) => panic!("unexpected usage error {other}"),
        }
        let _ = std::fs::remove_dir_all(&ev);
    }

    #[test]
    fn time_zone_and_mapping_errors_highlight_and_block() {
        let mut s = GuiState::default();
        s.add_input(PathBuf::from("x.csv"));
        assert!(s.show_csv_options(), "a CSV input shows the CSV options");
        s.set_preview(
            Ok(Err(PreflightError {
                path: "x.csv".into(),
                error: ParseError::NeedsTimezone,
            })),
            "button",
        );
        assert!(s.timezone_missing() && !s.can_start());
        s.set_timezone("UTC").unwrap();
        assert!(!s.timezone_missing() && s.can_start());
        s.set_preview(
            Ok(Err(PreflightError {
                path: "x.csv".into(),
                error: ParseError::NeedsMapping("missing: timestamp".into()),
            })),
            "button",
        );
        assert!(!s.can_start());
        assert!(s.usage_help().unwrap().contains("missing: timestamp"));
        assert_eq!(s.ui_actions.last().unwrap().1["error"], "needs_mapping");
        s.set_csv_profile(CsvProfile::CellebriteDraft);
        assert!(s.can_start());
        // New evidence forgets what the earlier previews required.
        s.timezone_required = true;
        s.set_timezone("").unwrap();
        assert!(!s.can_start());
        s.remove_input(0);
        assert!(!s.timezone_required);
    }

    #[test]
    fn preview_lists_exports_and_suggests_custodians() {
        let mut p = rv_rsmf_core::model::ParsedExport::default();
        p.participants.push(rv_rsmf_core::model::Participant {
            id: "p-1".into(),
            display: "Ana".into(),
            email: None,
            account_id: None,
            custom: Default::default(),
        });
        let mut ev =
            rv_rsmf_core::model::Event::message("e-1".into(), "c".into(), Default::default());
        ev.participant = Some("p-1".into());
        ev.source_time = "01/02/2024, 10:15".into();
        ev.timestamp = Some("2024-02-01T09:15:00Z".parse().unwrap());
        ev.body = "x".repeat(300);
        p.events.push(ev);
        p.facts.insert("date_order".into(), "dmy".into());
        let e = summarize(
            std::path::Path::new("chat.txt"),
            SourceKind::Whatsapp,
            Some(&p),
            None,
        );
        assert_eq!(e.participants, vec!["Ana"]);
        assert_eq!(e.first[0].utc, "2024-02-01T09:15:00Z");
        assert_eq!(e.first[0].sender, "Ana");
        assert_eq!(e.first[0].body.chars().count(), PREVIEW_BODY_CHARS);
        let mut s = GuiState::default();
        s.set_preview(Ok(Ok(vec![e])), "button");
        assert_eq!(s.known_participants, vec!["Ana"]);
        assert!(s.can_start() || s.inputs.is_empty());
        assert_eq!(s.ui_actions.last().unwrap().1["exports"], 1);
        assert!(s.ui_actions.last().unwrap().1["error"].is_null());
        // The suggestion list survives the custodian change that clears the preview.
        s.set_custodian("Ana");
        assert!(s.preview.is_none() && s.known_participants == vec!["Ana"]);
        // Platform name does not affect parsing: the preview stays.
        s.set_preview(Ok(Ok(vec![])), "button");
        s.set_platform_name("Signal");
        assert!(s.preview.is_some());
    }

    #[test]
    fn to_config_carries_every_conversion_option() {
        let dir = tmp_dir("cfg");
        let map = dir.join("map.toml");
        std::fs::write(&map, "").unwrap();
        let mut s = GuiState::default();
        s.add_input(dir.clone());
        s.set_source(Some(SourceKind::Csv));
        s.set_timezone("America/Mexico_City").unwrap();
        s.set_date_order(Some(DateOrder::Ymd));
        s.set_custodian("+57 300 000 0000");
        s.set_csv_map(&map.display().to_string());
        s.set_csv_profile(CsvProfile::XryDraft);
        s.set_platform_name("XRY export");
        let cfg = s.to_config().unwrap();
        let o = &cfg.convert;
        assert_eq!(o.parse.source, Some(SourceKind::Csv));
        assert_eq!(
            o.parse.timezone.map(|t| t.name()),
            Some("America/Mexico_City")
        );
        assert_eq!(o.parse.date_order, Some(DateOrder::Ymd));
        assert_eq!(o.parse.custodian.as_deref(), Some("+57 300 000 0000"));
        assert_eq!(o.parse.csv_map.as_deref(), Some(map.as_path()));
        assert_eq!(o.parse.csv_profile, CsvProfile::XryDraft);
        assert_eq!(o.platform_name.as_deref(), Some("XRY export"));
        assert!(!cfg.intake_hashinv && cfg.hashinv_exe.is_none());
        let start = &cfg.preamble.last().unwrap().1;
        assert_eq!(start["convert"]["timezone"], "America/Mexico_City");
        assert!(cfg.command_line.iter().any(|l| l == "date_order=ymd"));
        assert!(
            cfg.command_line
                .iter()
                .any(|l| l == "csv_profile=xry-draft")
        );
        s.set_csv_map(&dir.join("missing.toml").display().to_string());
        assert!(s.to_config().is_err(), "missing mapping file refused");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn intake_only_with_a_case_folder() {
        let mut s = GuiState::default();
        s.add_input(PathBuf::from("."));
        s.set_intake_hashinv(true);
        assert_eq!(actions(&s).last(), Some(&"ui_intake_changed"));
        assert!(s.hashinv.is_some(), "switching on looks for rv-hashinv");
        assert!(!s.intake_active(), "no case folder: no intake");
        let cfg = s.to_config().unwrap();
        assert!(!cfg.intake_hashinv && cfg.hashinv_exe.is_none());
        assert!(cfg.command_line.iter().any(|l| l == "intake=none"));
        let case_dir = tmp_dir("intake").join("CASE-I");
        s.set_case_dir(&case_dir.display().to_string());
        assert!(s.intake_active());
        let fake = PathBuf::from("/opt/rakvor/rv-hashinv");
        s.hashinv = Some(Ok(IntakeTool {
            path: fake.clone(),
            version: "0.3.0".into(),
            sha256: "0".repeat(64),
            found_by: "option".into(),
        }));
        let cfg = s.to_config().unwrap();
        assert!(cfg.intake_hashinv);
        assert_eq!(cfg.hashinv_exe.as_deref(), Some(fake.as_path()));
        assert_eq!(cfg.preamble.last().unwrap().1["intake"], "hashinv");
        s.hashinv = Some(Err("rv-hashinv not found".into()));
        assert!(s.to_config().is_err(), "intake asked but not available");
        s.remove_input(0);
        assert!(!s.can_start(), "the intake needs evidence to register");
        s.set_case_dir("");
        assert!(!s.intake_active());
    }

    #[test]
    fn conversion_events_and_summary_reach_the_outcome() {
        let mut s = GuiState::default();
        s.start();
        s.apply(&Event::IntakeStarted {
            tool: "rv-hashinv".into(),
            version: "0.3.0".into(),
            path: "x".into(),
            sha256: "y".into(),
        });
        s.apply(&Event::IntakeProgress {
            done: 1,
            total: 2,
            elapsed_ms: 5,
        });
        assert_eq!(s.intake_progress, Some((1, 2)));
        s.apply(&Event::IntakeFinished {
            run_id: "H".into(),
            exit_code: 0,
            items: 2,
            errors: 0,
            report_dir: Some("case/runs/rv-hashinv/H/reports".into()),
        });
        assert!(s.intake_report_dir.is_some());
        s.apply(&Event::ExportStarted {
            index: 1,
            path: "D:/ev/chat.txt".into(),
            source: "whatsapp".into(),
        });
        s.apply(&Event::RsmfWritten {
            path: "a.rsmf".into(),
            conversation: "c".into(),
            day: Some("2024-02-01".into()),
            events: 3,
            valid: true,
        });
        s.apply(&Event::RsmfWritten {
            path: "b.rsmf".into(),
            conversation: "c".into(),
            day: Some("2024-02-02".into()),
            events: 1,
            valid: false,
        });
        let t = s.conversion_text().unwrap();
        assert!(
            t.contains("chat.txt (whatsapp)") && t.contains(": 2") && t.contains("1 invalid"),
            "{t}"
        );
        s.apply(&Event::ExportFinished {
            index: 1,
            path: "D:/ev/chat.txt".into(),
            conversations: 1,
            events: 4,
            files: 2,
            invalid: 1,
        });
        assert!(s.conversion.current.is_none());
        s.apply(&Event::RunFinished {
            items: 1,
            bytes: 1,
            errors: 0,
            elapsed_ms: 1,
            high: 0,
            medium: 0,
            low: 0,
            info: 0,
            report_dir: "out/reports/R".into(),
            log_dir: "out/logs/R".into(),
            cancelled: false,
        });
        let o = s.outcome.clone().unwrap();
        assert_eq!(
            (
                o.conversion.exports,
                o.conversion.files,
                o.conversion.files_invalid
            ),
            (1, 2, 1)
        );
        assert!(o.rsmf_dir.is_none());
        let summary = RunSummary {
            conversion: ConversionTotals {
                exports: 1,
                conversations: 1,
                events: 4,
                files: 2,
                files_invalid: 1,
                attachments_packed: 3,
                ..Default::default()
            },
            rsmf_dir: "out/rsmf".into(),
            ..Default::default()
        };
        s.finish(&summary);
        let o = s.outcome.unwrap();
        assert_eq!(o.rsmf_dir, Some(PathBuf::from("out/rsmf")));
        assert_eq!(o.conversion.attachments_packed, 3);
    }
}
