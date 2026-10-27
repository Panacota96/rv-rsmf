//! rv-rsmf command-line interface. All work happens in `rv_rsmf_core`; this file only
//! parses arguments, renders events (progress bar or JSON Lines) and maps results to exit codes.

use clap::{Args, CommandFactory, Parser, Subcommand, ValueEnum};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use rv_rsmf_core::convert::{self, ConvertOptions};
use rv_rsmf_core::model::SourceKind;
use rv_rsmf_core::sources::{self, CsvProfile, DateOrder, ParseOptions};
use rv_rsmf_core::trust::{self, DEFAULT_TSA};
use rv_rsmf_core::{
    CASE_ROLE, Event, HashAlgo, RunConfig, TOOL_NAME, TOOL_VERSION, audit, case, hash, i18n, rsmf,
    run, verify,
};
use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const EXIT_OK: u8 = 0;
const EXIT_ERROR: u8 = 1;
const EXIT_USAGE: u8 = 2;
const EXIT_FINDINGS: u8 = 3;
const EXIT_CANCELLED: u8 = 130;

const ABOUT: &str = "Convert chat exports (WhatsApp, Telegram, Microsoft Teams .msg, mobile-tool CSV) to RSMF 2.0 files for Relativity, validated, traced message by message and recorded in a tamper-evident audit log.";

const LONG_ABOUT: &str = "\
DESCRIPTION:
  rv-rsmf converts chat exports into RSMF 2.0 (Relativity Short Message Format) files. It:
    1. opens every input file read-only and hashes it with one or two algorithms (and, in a case
       folder, checks it against the evidence registry);
    2. groups the files into exports: a WhatsApp .txt/.zip, a Telegram Desktop result.json, a folder
       of Microsoft Teams .msg files (Purview), or a CSV with a column mapping; media next to an
       export belong to it;
    3. reads each export into one model: participants, conversations, messages, attachments,
       reactions, with the line or id each message came from; local times are converted to UTC
       with --timezone (required when the export has no UTC offset);
    4. writes one .rsmf per conversation per day (days over 10,000 events are split), byte-for-byte
       reproducible, then checks each file against the official RSMF 2.0 schema plus stricter rules
       and reads it back;
    5. writes trace.csv (source line -> RSMF file and event), and per-export technical reports,
       customer summaries and custody forms (HTML, PDF and JSON).
  It does NOT authenticate the export or recover deleted messages: the RSMF shows what the export
  contains. Every action is appended to a hash-chained log that `rv-rsmf verify-log` checks.

  CASE FOLDER (--case-dir): Rakvor tools share one folder per case. rv-rsmf is an analysis tool: it
  re-hashes every file and compares it with evidence/registry.json, written by the intake tool
  rv-hashinv (add --intake hashinv to run rv-hashinv first). `rv-rsmf verify-case <CASE>` checks it all.";

const SCAN_AFTER_HELP: &str = "\
EXAMPLES:
  Convert an Android WhatsApp export (phone set to Paris time), Spanish reports:
    rv-rsmf convert -i \"D:\\export\\WhatsApp Chat with Ana.txt\" --timezone Europe/Paris --lang es
  Check the dates first, then convert an ambiguous export (all days <= 12) read as day/month/year:
    rv-rsmf preview -i chat.txt --timezone America/Bogota
    rv-rsmf convert -i chat.txt --timezone America/Bogota --date-order dmy --custodian \"Ana Ruiz\"
  Convert a Telegram Desktop export (times are UTC in the export) and a folder of Teams .msg files:
    rv-rsmf convert -i D:\\telegram\\result.json -i D:\\purview\\teams -r
  Convert an XRY-style CSV with your own column mapping:
    rv-rsmf convert -i chats.csv --csv-map xry.toml --timezone Europe/Madrid
  In a case folder, registering the evidence with rv-hashinv first, then verify the case:
    rv-rsmf convert -i D:\\export -r --case-dir E:\\CASE-014 --case CASE-014 --timezone Europe/Paris --intake hashinv
    rv-rsmf verify-case E:\\CASE-014
  Check any .rsmf file (ours or another tool's):
    rv-rsmf validate out\\rsmf\\<RUN-ID>\\001-chat\\Ana-Bob-1a2b3c\\2026-03-02.rsmf

EXIT CODES:
  0    Completed; no high-severity finding.
  1    Runtime error (input not found, output not writable, case log locked, ...).
       See the audit log.
  2    Usage error (unknown option, more than two hash algorithms, unsupported language), or a
       decision only the examiner can make: --timezone missing for an export without UTC
       offsets, --date-order needed for ambiguous dates, CSV columns not mapped.
       With --case-dir: folder of another case or schema, --case missing when creating the
       case, unknown evidence id.
  3    Completed with at least one high-severity finding (an export that could not be read, an
       RSMF file that fails validation or read-back, evidence that differs from the case
       registry), or the case folder fails verification (nothing is written to it).
  130  Cancelled with Ctrl+C. Partial reports and the cancellation are written to the audit log.

FILES WRITTEN:
  <OUT>/logs/<RUN-ID>/audit.jsonl         Hash-chained log of every action (one JSON object per line).
  <OUT>/logs/<RUN-ID>/audit.log           The same actions, human-readable.
  <OUT>/logs/<RUN-ID>/hashes.csv          Hash register: index, path, size, digests, status.
  <OUT>/logs/<RUN-ID>/environment.json    Tool and dependency versions, OS, host, user, command line.
  <OUT>/reports/<RUN-ID>/statistics.json  Run statistics (+ statistics.<LANG>.html and .pdf).
  <OUT>/reports/<RUN-ID>/items/<NNNNN>-<NAME>/technical.json|.<LANG>.html|.<LANG>.pdf   Technical review.
  <OUT>/reports/<RUN-ID>/items/<NNNNN>-<NAME>/summary.json|.<LANG>.html|.<LANG>.pdf     Customer summary.
  <OUT>/reports/<RUN-ID>/items/<NNNNN>-<NAME>/custody.json|.<es|fr>.html|.pdf|.en.html|.pdf  Custody form.
  <OUT>/logs/<RUN-ID>/timestamp-<SEQ>.tsr|.json  RFC 3161 token for the audit chain head.
  <OUT>/rsmf/<RUN-ID>/<NNN>-<EXPORT>/<CONVERSATION>/<YYYY-MM-DD>[-partNN].rsmf  RSMF 2.0 files.
  <OUT>/rsmf/<RUN-ID>/trace.csv|.json     Every message: source file and line -> RSMF file and event id.
  <OUT>/rsmf/<RUN-ID>/conversions.json    Exports, conversations, files, validation results.
  With --case-dir <CASE>, logs/, reports/ and rsmf/ go to <CASE>/runs/rv-rsmf/<RUN-ID>/, plus:
  <CASE>/case.json                        Case id, jurisdiction, creation (written once).
  <CASE>/case-log.jsonl                   Hash-chained case log of every tool: case_created,
                                          run_started, evidence_registered, run_finished.
  <CASE>/evidence/registry.json|.csv      Evidence registry: E0001..., path, size, hashes.

FORENSIC NOTES:
  - Evidence is opened read-only and never modified; modification times are compared before and after.
  - Work on a verified copy (write-blocked image or export), not on original media.
  - Each audit entry stores the SHA-256 of the previous one; editing, deleting or re-ordering any
    line is detected by `verify-log`. Removing the last lines is detected only against a record
    kept outside the log: the RFC 3161 timestamp or, with --case-dir, the case log. Keep the log
    folder with the reports.
  - A chat export is an editable text/JSON file: rv-rsmf converts, it does not authenticate. The
    RSMF reproduces the export, not the phone: deleted messages and unexported media are absent.
  - Times without a UTC offset are converted with --timezone; the original text of every time is
    kept in the event's custom field source_time and in trace.csv. DST changes are reported.
  - The same export and options always produce byte-identical .rsmf files (sorted zip entries,
    fixed zip times, content-derived MIME boundary): anyone can re-run and compare the SHA-256.
  - Each .rsmf is checked against the official RSMF 2.0 JSON schema (Relativity, BSD-3) plus
    stricter rules (unknown keys, references, UTC times, counts) and read back before reporting.
  - MD5 and SHA-1 are accepted for compatibility with older case files; pair them with SHA-256 or BLAKE3.
  - The chain head is timestamped by an RFC 3161 authority (proves when the log existed). Offline? The run
    finishes with `timestamp_pending`; run `rv-rsmf timestamp <LOG>` later.
  - With --sign-cert every PDF is signed with the examiner's certificate; the password is never logged.
  - Case folder: every tool re-verifies case-log.jsonl and the registry digest before
    writing, and refuses a broken case (exit 3). Appends happen under case-log.lock; a lock
    older than 10 minutes is reported as stale and never deleted automatically.
  - Each run's audit.jsonl records case_linked (the case-log entry it started from); the case
    log records the run's final audit head, so the two chains vouch for each other.";

#[derive(Parser)]
#[command(name = TOOL_NAME, version = TOOL_VERSION, about = ABOUT, long_about = LONG_ABOUT, max_term_width = 100,
          after_long_help = "Run `rv-rsmf <COMMAND> --help` for the options, examples and exit codes of each command.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Convert chat exports to RSMF 2.0 files (validated, traced, with reports and custody forms).
    #[command(long_about = LONG_ABOUT, after_long_help = SCAN_AFTER_HELP, alias = "scan")]
    Convert(Box<ScanArgs>),
    /// Show how an export will be read (source, date order, time zone, first messages) without writing anything.
    #[command(after_long_help = PREVIEW_AFTER_HELP)]
    Preview(Box<PreviewArgs>),
    /// Check .rsmf files against the RSMF 2.0 schema and rv-rsmf's stricter rules, and read them back.
    #[command(after_long_help = VALIDATE_AFTER_HELP)]
    Validate {
        /// .rsmf files (ours or another tool's). Folders are searched recursively for *.rsmf.
        /// Default: none (required). Example: rv-rsmf validate out\rsmf\<RUN-ID>
        #[arg(required = true, value_name = "FILE", verbatim_doc_comment)]
        files: Vec<PathBuf>,
    },
    /// Verify an audit log (audit.jsonl): edited, deleted or re-ordered entries, and truncation.
    #[command(after_long_help = "\
CHECKS:
  - every entry links to the previous one (hash chain): an edited, deleted or re-ordered entry
    breaks it;
  - every timestamp token (timestamp-<SEQ>.tsr) covers the hash of the entry it names, and every
    timestamp_obtained entry still has its token file;
  - truncation: a chain alone cannot show that entries were removed from its end. The log must
    still contain the entry covered by its RFC 3161 timestamp (covers_seq) and, for a run inside
    a case folder (<CASE>/runs/<TOOL>/<RUN-ID>/logs), end with the audit head that the case log
    recorded at run_finished (only timestamp_obtained may follow). Entries after the last
    anchored one are listed as not provable; with neither record the output says
    \"truncation not detectable without a timestamp or case record\".

EXAMPLES:
  rv-rsmf verify-log rakvor-out/logs/20261019T101500Z-a1b2c3/audit.jsonl
  rv-rsmf verify-log rakvor-out/logs/20261019T101500Z-a1b2c3
  rv-rsmf verify-log E:\\CASE-014\\runs\\rv-rsmf\\20261019T101500Z-a1b2c3\\logs

EXIT CODES:
  0  The chain is intact and agrees with its timestamps and case record (if any).
  1  The log cannot be read.
  3  The chain is broken (an entry was edited, deleted or re-ordered), a timestamp does not
     match, or the log is shorter than its timestamp or case record says (truncated).")]
    VerifyLog {
        /// Path to audit.jsonl, or to the run's log folder that contains it.
        ///
        /// Example: rakvor-out/logs/20261019T101500Z-a1b2c3
        log: PathBuf,
    },
    /// Check every hash algorithm against known-answer test vectors.
    #[command(after_long_help = "\
EXAMPLES:
  rv-rsmf self-test

EXIT CODES:
  0  All algorithms give the published answer for the input \"abc\".
  1  At least one algorithm failed: do not use this build for evidence.")]
    SelfTest,
    /// Add an RFC 3161 timestamp to a finished run (e.g. after working offline).
    #[command(after_long_help = "\
EXAMPLES:
  rv-rsmf timestamp rakvor-out/logs/20261019T101500Z-a1b2c3
  rv-rsmf timestamp rakvor-out/logs/20261019T101500Z-a1b2c3 --tsa-url https://freetsa.org/tsr

EXIT CODES:
  0  Timestamp obtained and logged.
  1  The TSA could not be reached (nothing changed; try again).
  3  The log is missing or broken: no timestamp is added.")]
    Timestamp {
        /// The run's log folder (or its audit.jsonl).
        ///
        /// Example: rakvor-out/logs/20261019T101500Z-a1b2c3
        log: PathBuf,
        /// RFC 3161 time-stamping authority.
        /// Default: http://timestamp.digicert.com
        /// Example: --tsa-url https://freetsa.org/tsr
        #[arg(long, value_name = "URL", default_value = DEFAULT_TSA, verbatim_doc_comment, hide_default_value = true)]
        tsa_url: String,
        /// Name written in the new audit entry.
        /// Default: the operating-system user name
        /// Example: --actor "Ana Ruiz"
        #[arg(short, long, value_name = "NAME", verbatim_doc_comment)]
        actor: Option<String>,
    },
    /// Verify a whole case folder: case log, evidence registry and every run's audit log.
    #[command(after_long_help = "\
CHECKS:
  - case-log.jsonl is an intact hash chain starting with case_created; case.json is unchanged;
  - evidence/registry.json has the SHA-256 recorded at the last registry change;
  - every run announced in the case log has an intact audit.jsonl (and valid timestamp
    tokens), its case_linked entry points to its run_started entry, and the audit head
    recorded at run_finished is its final entry (only timestamp_obtained may follow);
  - with --rehash: every registered item still reachable at its registered path is read again
    and its digests are compared with the registry (the latest registration of a path).

EXAMPLES:
  rv-rsmf verify-case E:\\CASE-014
  rv-rsmf verify-case E:\\CASE-014 --rehash

EXIT CODES:
  0  The case folder is intact (warnings, e.g. a run still in progress or an item not
     reachable for --rehash, are printed).
  1  The folder is not a case folder (no case.json).
  3  At least one check failed: the case was edited, truncated or re-ordered, or (--rehash)
     an evidence file differs from the registry.")]
    VerifyCase {
        /// The case folder (the one that holds case.json).
        ///
        /// Example: E:\CASE-014
        case: PathBuf,
        /// Also re-hash every registered item still reachable and compare it with the registry.
        /// Default: off (only the registry's digest is checked)
        /// Example: --rehash
        #[arg(long, verbatim_doc_comment)]
        rehash: bool,
    },
    /// Print the full help of every command as Markdown (used to generate docs/usage-cli.md).
    #[command(hide = true)]
    DocsCli,
}

#[derive(Clone, Copy, ValueEnum)]
enum ProgressMode {
    /// Progress bar with elapsed time, throughput and ETA on stderr.
    Bar,
    /// One JSON event per line on stdout (for scripts and other tools).
    Jsonl,
    /// No progress output; only the final summary.
    None,
}

#[derive(Args)]
struct ScanArgs {
    /// File or folder to review. Repeat for several.
    ///
    /// Folders are reviewed one level deep unless --recursive is given. Symbolic links are not followed.
    /// Default: none (required unless --input-list, --case-dir or --evidence is given).
    /// Example: --input D:\evidence --input E:\phone-export\chat.txt
    #[arg(
        short,
        long = "input",
        value_name = "PATH",
        required_unless_present_any = ["input_list", "case_dir", "evidence"],
        verbatim_doc_comment
    )]
    inputs: Vec<PathBuf>,

    /// Text file with one path per line (bulk review). Lines starting with # are ignored.
    ///
    /// Relative paths are resolved from the list file's folder.
    /// Default: none. Example: --input-list case-014-files.txt
    #[arg(short = 'L', long, value_name = "FILE", verbatim_doc_comment)]
    input_list: Option<PathBuf>,

    /// Also review files in subfolders.
    ///
    /// Default: off (only the first level of each folder). Example: -i D:\evidence --recursive
    #[arg(short, long, verbatim_doc_comment)]
    recursive: bool,

    /// Hash algorithm(s): one or two of md5, sha1, sha256, sha512, sha3-256, blake3.
    ///
    /// Both are computed in the same read of each file. More than two is refused (exit code 2).
    /// Default: sha256. Example: --hash sha256,md5   or   --hash sha256 --hash blake3
    #[arg(
        short = 'H',
        long = "hash",
        value_name = "ALGO",
        value_delimiter = ',',
        default_value = "sha256",
        verbatim_doc_comment,
        hide_default_value = true
    )]
    hashes: Vec<String>,

    /// Folder where logs/ and reports/ are created. Created if missing. Ignored with --case-dir.
    ///
    /// Default: ./rakvor-out. Example: --out E:\case-014\rv-rsmf
    #[arg(
        short,
        long,
        value_name = "DIR",
        default_value = "rakvor-out",
        verbatim_doc_comment,
        hide_default_value = true
    )]
    out: PathBuf,

    /// Report language: en, es, fr, or all (one HTML and PDF per language). JSON is language-neutral.
    ///
    /// Default: en. Example: --lang es   or   --lang all
    #[arg(
        short,
        long,
        value_name = "LANG",
        default_value = "en",
        verbatim_doc_comment,
        hide_default_value = true
    )]
    lang: String,

    /// Name of the examiner, written in every audit entry and report.
    ///
    /// Default: the operating-system user name. Example: --actor "Ana Ruiz"
    #[arg(short, long, value_name = "NAME", verbatim_doc_comment)]
    actor: Option<String>,

    /// Case or engagement identifier printed on the reports. With --case-dir it names the case
    /// when the folder is created, and must match case.json afterwards (it can be left out).
    /// Default: none. Example: --case CASE-2026-014
    #[arg(short, long = "case", value_name = "ID", verbatim_doc_comment)]
    case_id: Option<String>,

    /// How progress is shown: bar, jsonl or none.
    ///
    /// Default: bar (hidden automatically when stderr is not a terminal).
    /// Example: --progress jsonl
    #[arg(
        short,
        long,
        value_enum,
        value_name = "MODE",
        default_value = "bar",
        verbatim_doc_comment,
        hide_default_value = true
    )]
    progress: ProgressMode,

    /// Chain-of-custody form per item: co (Colombia, Fiscalía style, Spanish) or fr (France, scellés, French).
    /// An English copy is always added. With --case-dir the case's jurisdiction applies.
    /// Default: co
    /// Example: --jurisdiction fr
    #[arg(short, long, value_name = "CC", default_value = "co", value_parser = ["co", "fr"], verbatim_doc_comment, hide_default_value = true, hide_possible_values = true)]
    jurisdiction: String,

    /// RFC 3161 time-stamping authority for the audit chain head.
    /// Default: http://timestamp.digicert.com
    /// Example: --tsa-url https://freetsa.org/tsr
    #[arg(long, value_name = "URL", default_value = DEFAULT_TSA, verbatim_doc_comment, hide_default_value = true)]
    tsa_url: String,

    /// Do not request a trusted timestamp (offline labs). Also set by RV_NO_TIMESTAMP=1.
    /// Default: off (a timestamp is requested)
    /// Example: --no-timestamp
    #[arg(long, env = "RV_NO_TIMESTAMP", value_parser = clap::builder::FalseyValueParser::new(), verbatim_doc_comment, hide_env = true)]
    no_timestamp: bool,

    /// Examiner's certificate (.p12/.pfx, RSA) used to sign every PDF report. The password is read from
    /// the environment variable named by --sign-pass-env, or asked for interactively.
    /// Default: none (PDFs are not signed)
    /// Example: --sign-cert C:\certs\ana-ruiz.p12
    #[arg(long, value_name = "P12", verbatim_doc_comment)]
    sign_cert: Option<PathBuf>,

    /// Environment variable holding the certificate password.
    /// Default: RV_SIGN_PASSWORD
    /// Example: --sign-pass-env CERT_PASS
    #[arg(
        long,
        value_name = "VAR",
        default_value = "RV_SIGN_PASSWORD",
        verbatim_doc_comment,
        hide_default_value = true
    )]
    sign_pass_env: String,

    /// Shared case folder that links the Rakvor tools of one case (created with --case if missing).
    /// Outputs go to <DIR>/runs/rv-rsmf/<RUN-ID>/; every hashed file is registered in
    /// <DIR>/evidence/registry.json; the run is recorded in <DIR>/case-log.jsonl.
    /// With no --input, every registered item is examined again.
    /// Default: none (standalone run in --out)
    /// Example: --case-dir E:\CASE-014 --case CASE-014
    #[arg(long, value_name = "DIR", verbatim_doc_comment)]
    case_dir: Option<PathBuf>,

    /// Registered evidence ids to examine (needs --case-dir), comma-separated or repeated.
    /// Default: none (the --input files, or every registered item)
    /// Example: --evidence E0001,E0007
    #[arg(
        short = 'E',
        long = "evidence",
        value_name = "IDS",
        requires = "case_dir",
        verbatim_doc_comment
    )]
    evidence: Vec<String>,

    #[command(flatten)]
    chat: ChatArgs,

    /// Register the evidence with the intake tool first: hashinv runs RV HashInv
    /// (`rv-hashinv scan --case-dir`) on the same --input/--input-list, so the case gets its hash
    /// inventory, hash reports and evidence registry from rv-hashinv itself; then every export is
    /// converted and verified against that registry. Needs --case-dir; not with --evidence.
    /// rv-hashinv is searched in: --hashinv-exe, RV_HASHINV, next to rv-rsmf, the RV HashInv
    /// install folder, PATH. Its path, version and SHA-256 go into this run's audit log.
    /// Default: none. Example: --case-dir E:\CASE-014 --case CASE-014 -i D:\export -r --intake hashinv
    #[arg(
        long,
        value_enum,
        default_value_t = Intake::None,
        requires = "case_dir",
        hide_default_value = true,
        hide_possible_values = true,
        verbatim_doc_comment
    )]
    intake: Intake,

    /// rv-hashinv executable for --intake hashinv (needs version 0.3.0 or later).
    /// Default: searched (see --intake). Example: --hashinv-exe "C:\Tools\RV HashInv\rv-hashinv.exe"
    #[arg(long, value_name = "EXE", verbatim_doc_comment)]
    hashinv_exe: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum Intake {
    /// The case registry already exists (rv-hashinv was run before), or no case folder.
    None,
    /// Run rv-hashinv on the same evidence first.
    Hashinv,
}

/// How the chat exports are read (shared by `convert` and `preview`).
#[derive(Args, Clone, Debug)]
struct ChatArgs {
    /// Export type: whatsapp, telegram, teams-msg or csv. Forces the reader for files that would
    /// otherwise not be recognised (e.g. a renamed WhatsApp .txt).
    /// Default: detected from each file's name and content. Example: --source whatsapp
    #[arg(long, value_name = "TYPE", value_parser = ["whatsapp", "telegram", "teams-msg", "csv"], hide_possible_values = true, verbatim_doc_comment)]
    source: Option<String>,

    /// Time zone the phone or tool used when the export was made (IANA name). Required for
    /// WhatsApp and CSV exports, whose times have no UTC offset; also used to cut the days.
    /// Default: none (UTC for slicing exports whose times carry an offset). Example: --timezone Europe/Paris
    #[arg(short = 'z', long, value_name = "TZ", verbatim_doc_comment)]
    timezone: Option<String>,

    /// Order of day, month and year in numeric dates: dmy, mdy or ymd. Needed only when the
    /// export does not decide it (every day and month <= 12); otherwise inferred and reported.
    /// Default: inferred from the file. Example: --date-order dmy
    #[arg(long, value_name = "ORDER", value_parser = ["dmy", "mdy", "ymd"], hide_possible_values = true, verbatim_doc_comment)]
    date_order: Option<String>,

    /// Device owner as named in the export (or their number). Becomes the RSMF custodian and From
    /// header; WhatsApp authors written as "You"/"Tú"/"Vous" are given this name.
    /// Default: none. Example: --custodian "Ana Ruiz"
    #[arg(long, value_name = "NAME", verbatim_doc_comment)]
    custodian: Option<String>,

    /// CSV column mapping (TOML: [columns] timestamp, timestamp_format, conversation, sender, body,
    /// …; [options] delimiter, encoding). See docs/usage.md.
    /// Default: none (the --csv-profile columns). Example: --csv-map xry.toml
    #[arg(long, value_name = "TOML", verbatim_doc_comment)]
    csv_map: Option<PathBuf>,

    /// Built-in CSV column profile: generic, xry-draft or cellebrite-draft. The two tool profiles
    /// are drafts (not yet checked against real exports) and are reported as such.
    /// Default: generic. Example: --csv-profile xry-draft
    #[arg(long, value_name = "PROFILE", default_value = "generic", value_parser = ["generic", "xry-draft", "cellebrite-draft"], hide_default_value = true, hide_possible_values = true, verbatim_doc_comment)]
    csv_profile: String,

    /// Name shown as the platform in Relativity (RSMF platform / X-RSMF-Application).
    /// Default: WhatsApp, Telegram, Microsoft Teams or Mobile export. Example: --platform-name "WhatsApp Business"
    #[arg(long, value_name = "NAME", verbatim_doc_comment)]
    platform_name: Option<String>,
}

#[derive(Args)]
struct PreviewArgs {
    /// Export file or folder to preview. Repeat for several.
    /// Default: none (required). Example: -i chat.txt
    #[arg(
        short,
        long = "input",
        value_name = "PATH",
        required = true,
        verbatim_doc_comment
    )]
    inputs: Vec<PathBuf>,

    /// Also look in subfolders.
    /// Default: off. Example: -i D:\export -r
    #[arg(short, long, verbatim_doc_comment)]
    recursive: bool,

    /// Messages to show per export.
    /// Default: 20. Example: --limit 50
    #[arg(
        long,
        value_name = "N",
        default_value_t = 20,
        hide_default_value = true,
        verbatim_doc_comment
    )]
    limit: usize,

    /// Print JSON (one object per export) instead of text.
    /// Default: off. Example: --json
    #[arg(long, verbatim_doc_comment)]
    json: bool,

    #[command(flatten)]
    chat: ChatArgs,
}

const PREVIEW_AFTER_HELP: &str = "\
EXAMPLES:
  rv-rsmf preview -i chat.txt --timezone Europe/Paris
  rv-rsmf preview -i D:\\telegram\\result.json --limit 5 --json

EXIT CODES:
  0  Every export can be converted with these options.
  1  An export could not be read (damaged or unsupported format).
  2  A decision is needed: --timezone, --date-order or a CSV mapping (the message says which).

FORENSIC NOTES:
  - Nothing is written and nothing is logged: preview is for choosing options before `convert`,
    which records every choice in its audit log.";

const VALIDATE_AFTER_HELP: &str = "\
EXAMPLES:
  rv-rsmf validate out\\rsmf\\<RUN-ID>
  rv-rsmf validate other-vendor.rsmf

EXIT CODES:
  0  Every file passes the RSMF 2.0 schema and the strict checks.
  1  A file could not be read (not an RSMF message, no rsmf.zip, damaged zip).
  3  At least one file fails validation (each problem is printed).

FORENSIC NOTES:
  - Checks: the official RSMF 2.0 JSON schema (bundled, from Relativity's rsmf-validator-samples);
    unknown keys; participant, conversation and parent references; UTC timestamps; every
    attachment, avatar and icon present in rsmf.zip; X-RSMF-Version/EventCount/AttachmentCount
    agree with the manifest; at most 10,000 events per file.
  - It does not check how a given Relativity version displays the file.";

/// Builds the conversion options from the shared chat arguments (usage errors as text).
fn convert_options(c: &ChatArgs) -> Result<ConvertOptions, String> {
    let timezone = match &c.timezone {
        None => None,
        Some(z) => Some(z.parse::<chrono_tz::Tz>().map_err(|_| {
            format!(
                "unknown time zone '{z}': use an IANA name such as Europe/Paris or America/Bogota"
            )
        })?),
    };
    Ok(ConvertOptions {
        parse: ParseOptions {
            source: c.source.as_deref().and_then(SourceKind::parse),
            timezone,
            date_order: c.date_order.as_deref().and_then(DateOrder::parse),
            csv_map: c.csv_map.clone(),
            csv_profile: CsvProfile::parse(&c.csv_profile).unwrap_or(CsvProfile::Generic),
            custodian: c.custodian.clone(),
        },
        platform_name: c.platform_name.clone(),
    })
}

/// Usage errors in the exports (time zone, date order, mapping) stop before any output (exit 2).
fn check_exports(
    inputs: &[PathBuf],
    list: Option<&std::path::Path>,
    recursive: bool,
    opts: &ConvertOptions,
) -> Result<(), u8> {
    let files: Vec<PathBuf> = match rv_rsmf_core::input::collect(inputs, list, recursive) {
        Ok(items) => items.into_iter().map(|i| i.path).collect(),
        Err(_) => return Ok(()), // the run reports missing inputs itself (exit 1, audited)
    };
    match convert::preflight(&files, opts) {
        Ok(_) => Ok(()),
        Err(e) => Err(usage_error(&e.to_string())),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    ExitCode::from(match cli.command {
        Command::Convert(a) => scan(*a),
        Command::Preview(a) => preview(*a),
        Command::Validate { files } => validate(files),
        Command::VerifyLog { log } => verify_log(log),
        Command::VerifyCase { case, rehash } => verify_case(case, rehash),
        Command::SelfTest => self_test(),
        Command::Timestamp {
            log,
            tsa_url,
            actor,
        } => timestamp(log, tsa_url, actor),
        Command::DocsCli => {
            print!("{}", docs_cli());
            EXIT_OK
        }
    })
}

fn usage_error(msg: &str) -> u8 {
    eprintln!(
        "error: {}\n\nFor more information, try 'rv-rsmf scan --help'.",
        msg
    );
    EXIT_USAGE
}

fn scan(a: ScanArgs) -> u8 {
    let mut algos = Vec::new();
    for h in &a.hashes {
        match HashAlgo::parse(h) {
            Some(x) => algos.push(x),
            None => {
                return usage_error(&format!(
                    "unknown hash algorithm '{}': use md5, sha1, sha256, sha512, sha3-256 or blake3",
                    h
                ));
            }
        }
    }
    if let Err(e) = hash::validate_selection(&algos) {
        return usage_error(&e);
    }
    let langs = match i18n::parse_langs(&a.lang) {
        Ok(l) => l,
        Err(e) => return usage_error(&e),
    };
    let evidence_ids = match case::parse_evidence_ids(&a.evidence) {
        Ok(ids) => ids,
        Err(e) => return usage_error(&e.message),
    };
    let convert_opts = match convert_options(&a.chat) {
        Ok(o) => o,
        Err(e) => return usage_error(&e),
    };
    let intake = a.intake == Intake::Hashinv;
    if intake && !a.evidence.is_empty() {
        return usage_error(
            "--intake hashinv registers new evidence: it cannot be combined with --evidence",
        );
    }
    if intake && a.inputs.is_empty() && a.input_list.is_none() {
        return usage_error(
            "--intake hashinv needs the evidence to register (--input or --input-list)",
        );
    }
    if let Err(code) = check_exports(
        &a.inputs,
        a.input_list.as_deref(),
        a.recursive,
        &convert_opts,
    ) {
        return code;
    }
    // With --intake the case may not exist yet: rv-hashinv creates it, and the core checks it afterwards.
    if let Some(dir) = a.case_dir.as_ref().filter(|_| !intake) {
        // Case problems found before the run map to their own exit codes (2 usage, 3 broken case).
        if let Err(e) = case::preflight(dir, a.case_id.as_deref(), &evidence_ids) {
            return match e.exit_code() {
                EXIT_USAGE => usage_error(&e.message),
                code => {
                    eprintln!("error: {}", e.message);
                    code
                }
            };
        }
    }

    let cancel = Arc::new(AtomicBool::new(false));
    let c = cancel.clone();
    let _ = ctrlc::set_handler(move || c.store(true, Ordering::SeqCst));

    let cfg = RunConfig {
        inputs: a.inputs,
        input_list: a.input_list,
        recursive: a.recursive,
        algorithms: algos,
        out_dir: a.out,
        langs,
        actor: a.actor.unwrap_or_else(rv_rsmf_core::env::os_user),
        case_id: a.case_id,
        frontend: "cli".into(),
        preamble: vec![],
        command_line: std::env::args().collect(),
        tool_name: TOOL_NAME.into(),
        tool_version: TOOL_VERSION.into(),
        jurisdiction: a.jurisdiction.clone(),
        tsa_url: if a.no_timestamp {
            None
        } else {
            Some(a.tsa_url.clone())
        },
        sign_password: a
            .sign_cert
            .as_ref()
            .and_then(|_| password(&a.sign_pass_env)),
        sign_cert: a.sign_cert.clone(),
        case_dir: a.case_dir.clone(),
        evidence_ids,
        case_role: CASE_ROLE,
        convert: convert_opts,
        intake_hashinv: intake,
        hashinv_exe: a.hashinv_exe.clone(),
    };

    let bar = match a.progress {
        ProgressMode::Bar => {
            let b = ProgressBar::with_draw_target(Some(0), ProgressDrawTarget::stderr_with_hz(8));
            b.set_style(
                ProgressStyle::with_template("{spinner} [{elapsed_precise}] {wide_bar} {binary_bytes}/{binary_total_bytes} {binary_bytes_per_sec} ETA {eta} {msg}")
                    .unwrap_or_else(|_| ProgressStyle::default_bar()),
            );
            Some(b)
        }
        _ => None,
    };
    let jsonl = matches!(a.progress, ProgressMode::Jsonl);
    // A closed stdout (e.g. `| head`) must never abort a forensic run half-way: stop printing, keep working.
    let mut stdout_open = true;
    let mut sink = |e: &Event| {
        if jsonl && stdout_open {
            let mut out = std::io::stdout().lock();
            stdout_open = writeln!(out, "{}", serde_json::to_string(e).unwrap_or_default())
                .and_then(|_| out.flush())
                .is_ok();
        }
        if let Some(b) = &bar {
            match e {
                Event::RunStarted {
                    total_bytes,
                    total_items,
                    ..
                } => {
                    b.set_length(*total_bytes);
                    b.set_message(format!("0/{} files", total_items));
                }
                Event::Progress {
                    done,
                    total,
                    bytes_done,
                    ..
                } => {
                    b.set_position(*bytes_done);
                    b.set_message(format!("{}/{} files", done, total));
                }
                Event::Error { path, message } => b.println(format!(
                    "warning: {}: {}",
                    path.as_deref().unwrap_or("-"),
                    message
                )),
                Event::IntakeStarted { version, path, .. } => {
                    b.println(format!("intake: rv-hashinv {version} ({path}) registers the evidence"))
                }
                Event::IntakeProgress { done, total, .. } => {
                    b.set_message(format!("intake {}/{} files", done, total))
                }
                Event::IntakeFinished { run_id, exit_code, items, report_dir, .. } => b.println(format!(
                    "intake: rv-hashinv run {run_id} finished (exit {exit_code}, {items} files); hash reports: {}",
                    report_dir.as_deref().unwrap_or("-")
                )),
                Event::ExportStarted { path, source, .. } => {
                    b.set_message(format!("converting {source}: {path}"))
                }
                Event::RsmfWritten { valid: false, path, .. } => {
                    b.println(format!("warning: {path} fails validation (see the report)"))
                }
                Event::ExportFinished { path, conversations, events, files, invalid, .. } => b.println(format!(
                    "converted {path}: {conversations} conversation(s), {events} event(s), {files} RSMF file(s){}",
                    if *invalid > 0 { format!(", {invalid} INVALID") } else { String::new() }
                )),
                Event::Cancelled { .. } => b.println("cancelled: writing partial reports..."),
                Event::Timestamp { status, reason, .. } if status != "ok" => b.println(format!(
                    "warning: timestamp pending ({}); run `rv-rsmf timestamp` later",
                    reason.as_deref().unwrap_or("-")
                )),
                _ => {}
            }
        }
    };
    let result = run(&cfg, &mut sink, &cancel);
    if let Some(b) = &bar {
        b.finish_and_clear();
    }
    match result {
        Err(e) => {
            eprintln!("error: {}", e);
            EXIT_ERROR
        }
        Ok(s) => {
            if !jsonl {
                let out = format!(
                    "{status}  run {id}\n  items: {items}  bytes: {bytes}  errors: {errors}  elapsed: {elapsed}\n  findings: high {h}, medium {m}, low {l}, info {i}\n  timestamp: {ts}\n  report: {report}\n  logs:   {logs}",
                    status = if s.cancelled {
                        "CANCELLED"
                    } else if s.high > 0 {
                        "DONE (high-severity findings)"
                    } else {
                        "DONE"
                    },
                    id = s.run_id,
                    items = s.items,
                    bytes = s.bytes,
                    errors = s.errors,
                    elapsed = rv_rsmf_core::time::human_duration(s.elapsed_ms),
                    h = s.high,
                    m = s.medium,
                    l = s.low,
                    i = s.info,
                    ts = format!(
                        "{} {}",
                        s.timestamp["status"].as_str().unwrap_or("off"),
                        s.timestamp["gen_time"].as_str().unwrap_or("")
                    )
                    .trim(),
                    report = s
                        .report_dir
                        .join(format!("statistics.{}.html", cfg.langs[0]))
                        .display(),
                    logs = s.log_dir.display()
                );
                let _ = writeln!(std::io::stdout(), "{}", out);
                let cv = &s.conversion;
                let _ = writeln!(
                    std::io::stdout(),
                    "  rsmf:   {} export(s){}, {} conversation(s), {} event(s), {} file(s){} -> {}",
                    cv.exports,
                    if cv.exports_failed > 0 {
                        format!(" ({} FAILED)", cv.exports_failed)
                    } else {
                        String::new()
                    },
                    cv.conversations,
                    cv.events,
                    cv.files,
                    if cv.files_invalid > 0 {
                        format!(" ({} INVALID)", cv.files_invalid)
                    } else {
                        String::new()
                    },
                    s.rsmf_dir.display()
                );
                if let Some(c) = &s.case {
                    let t = &c.tally;
                    let _ = writeln!(
                        std::io::stdout(),
                        "  case:   {} ({}) registered {} (new {}), verified {}, mismatched {}, not in registry {}",
                        c.case_id,
                        c.role,
                        t.registered,
                        t.added,
                        t.verified,
                        t.mismatched,
                        t.unregistered
                    );
                }
            }
            if s.cancelled {
                EXIT_CANCELLED
            } else if s.high > 0 {
                EXIT_FINDINGS
            } else {
                EXIT_OK
            }
        }
    }
}

fn password(env: &str) -> Option<String> {
    if let Ok(p) = std::env::var(env) {
        return Some(p);
    }
    use std::io::IsTerminal;
    if std::io::stdin().is_terminal() {
        // No echo: the password never appears on screen or in a terminal recording.
        return rpassword::prompt_password("Certificate password: ").ok();
    }
    None
}

fn timestamp(log: PathBuf, tsa_url: String, actor: Option<String>) -> u8 {
    let dir = if log.is_file() {
        log.parent().map(|p| p.to_path_buf()).unwrap_or_default()
    } else {
        log
    };
    if !dir.join("audit.jsonl").is_file() {
        eprintln!(
            "error: no audit log at {}: nothing to timestamp",
            dir.join("audit.jsonl").display()
        );
        return EXIT_FINDINGS;
    }
    let mut audit_log =
        match audit::AuditLog::resume(&dir, &actor.unwrap_or_else(rv_rsmf_core::env::os_user)) {
            Ok(l) => l,
            Err(e) => {
                eprintln!("error: {}", e);
                return EXIT_FINDINGS;
            }
        };
    match trust::timestamp_head(&dir, audit_log.entries(), audit_log.head(), &tsa_url) {
        Ok(ts) => {
            if let Err(e) = audit_log.record("timestamp_obtained", Some(&ts.file), ts.to_json()) {
                eprintln!("error: {}", e);
                return EXIT_ERROR;
            }
            println!(
                "OK  timestamp {} by {} covers entry #{}",
                ts.gen_time, ts.tsa, ts.covers_seq
            );
            EXIT_OK
        }
        Err(e) => {
            eprintln!("error: {}", e);
            EXIT_ERROR
        }
    }
}

fn verify_log(log: PathBuf) -> u8 {
    let path = if log.is_dir() {
        log.join("audit.jsonl")
    } else {
        log
    };
    let v = match verify::verify_log(&path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("error: {}", e);
            return EXIT_ERROR;
        }
    };
    if v.problems.is_empty() {
        println!(
            "OK  {} entries, chain intact: {}",
            v.entries,
            path.display()
        );
        for t in &v.timestamps {
            println!("    {}", t);
        }
        println!("    {}", v.truncation);
    } else {
        println!("BROKEN  {}", path.display());
        for p in &v.problems {
            println!("  - {}", p);
        }
    }
    for w in &v.warnings {
        println!("  warning: {}", w);
    }
    if v.problems.is_empty() {
        EXIT_OK
    } else {
        EXIT_FINDINGS
    }
}

fn verify_case(dir: PathBuf, rehash: bool) -> u8 {
    if !dir.join(case::CASE_FILE).is_file() {
        eprintln!(
            "error: {} is not a case folder (no {})",
            dir.display(),
            case::CASE_FILE
        );
        return EXIT_ERROR;
    }
    let mut v = case::verify_case(&dir);
    let rehashed = rehash.then(|| case::rehash_registry(&dir, &mut v));
    if v.problems.is_empty() {
        println!(
            "OK  case {}: {} case-log entries, {} registered items, {} runs verified: {}",
            v.case_id,
            v.entries,
            v.registry_items,
            v.runs,
            dir.display()
        );
        if let Some(r) = &rehashed {
            println!(
                "    re-hashed {} of {} registered items: {} match the registry",
                r.checked, r.registered, r.matched
            );
        }
        for w in &v.warnings {
            println!("  warning: {}", w);
        }
        EXIT_OK
    } else {
        println!("BROKEN  case {}: {}", v.case_id, dir.display());
        for p in &v.problems {
            println!("  - {}", p);
        }
        if let Some(r) = &rehashed {
            println!(
                "    re-hashed {} of {} registered items: {} match the registry",
                r.checked, r.registered, r.matched
            );
        }
        for w in &v.warnings {
            println!("  warning: {}", w);
        }
        EXIT_FINDINGS
    }
}

fn self_test() -> u8 {
    let failed = hash::self_test();
    for (algo, _) in hash::KNOWN_ANSWERS {
        println!(
            "{:<9} {}",
            algo.name(),
            if failed.contains(&algo) {
                "FAIL"
            } else {
                "PASS"
            }
        );
    }
    if failed.is_empty() {
        EXIT_OK
    } else {
        EXIT_ERROR
    }
}

/// Markdown of every command's long help, so docs/usage-cli.md can never drift from the binary.
fn docs_cli() -> String {
    let mut root = Cli::command().term_width(100);
    root.build();
    let mut out = String::from(
        "# Command-line reference\n\n<!-- Generated by `rv-rsmf docs-cli`. Do not edit by hand: CI fails if this file differs. -->\n\n",
    );
    out.push_str("```mermaid\nflowchart LR\n  A[rv-rsmf] --> B[scan]\n  A --> C[verify-log]\n  A --> D[self-test]\n  A --> T[timestamp]\n  A --> V[verify-case]\n  T --> E\n  B --> E[(logs/RUN-ID)]\n  B --> F[(reports/RUN-ID)]\n  B -. --case-dir .-> K[(CASE folder)]\n  V --> K\n  C --> E\n```\n\n");
    out.push_str(&format!(
        "## rv-rsmf\n\n```text\n{}\n```\n\n",
        root.render_long_help().to_string().trim_end()
    ));
    let names: Vec<String> = root
        .get_subcommands()
        .filter(|c| !c.is_hide_set() && c.get_name() != "help")
        .map(|c| c.get_name().to_string())
        .collect();
    for name in names {
        let sub = root.find_subcommand_mut(&name).expect("subcommand");
        let mut sub = sub.clone().bin_name(format!("rv-rsmf {}", name));
        out.push_str(&format!(
            "## rv-rsmf {}\n\n```text\n{}\n```\n\n",
            name,
            sub.render_long_help().to_string().trim_end()
        ));
    }
    out
}

fn preview(a: PreviewArgs) -> u8 {
    let opts = match convert_options(&a.chat) {
        Ok(o) => o,
        Err(e) => return usage_error(&e),
    };
    let files: Vec<PathBuf> = match rv_rsmf_core::input::collect(&a.inputs, None, a.recursive) {
        Ok(items) => items.into_iter().map(|i| i.path).collect(),
        Err(e) => {
            eprintln!("error: {e}");
            return EXIT_ERROR;
        }
    };
    let (exports, orphans) = sources::plan_exports(&files, opts.parse.source);
    if exports.is_empty() {
        eprintln!(
            "error: no chat export found among {} file(s); use --source to force the type",
            files.len()
        );
        return EXIT_ERROR;
    }
    let mut code = EXIT_OK;
    let mut out = std::io::stdout().lock();
    for e in &exports {
        match sources::parse(e, &opts.parse) {
            Ok(p) => {
                let events = sources::preview_events(&p, a.limit);
                if a.json {
                    let v = serde_json::json!({
                        "export": e.primary.display().to_string(), "source": e.kind.name(),
                        "conversations": p.conversations.iter().map(|c| &c.display).collect::<Vec<_>>(),
                        "participants": p.participants.iter().map(|x| &x.display).collect::<Vec<_>>(),
                        "events": p.events.len(), "facts": p.facts,
                        "findings": p.findings.iter().map(|f| &f.code).collect::<Vec<_>>(),
                        "first": events.iter().map(|ev| serde_json::json!({"source_time": ev.source_time,
                            "utc": ev.timestamp.as_ref().map(rsmf::rsmf_time), "participant": ev.participant.as_ref().and_then(|id| p.participant(id)).map(|x| &x.display),
                            "body": ev.body.chars().take(120).collect::<String>(), "line": ev.source.line})).collect::<Vec<_>>(),
                    });
                    let _ = writeln!(out, "{}", v);
                } else {
                    let _ = writeln!(out, "{} [{}]", e.primary.display(), e.kind.name());
                    for (k, v) in &p.facts {
                        let _ = writeln!(out, "  {k}: {v}");
                    }
                    let _ = writeln!(
                        out,
                        "  conversations: {}  participants: {}  events: {}",
                        p.conversations.len(),
                        p.participants.len(),
                        p.events.len()
                    );
                    for f in &p.findings {
                        let _ = writeln!(out, "  finding: {} ({})", f.code, f.severity.key());
                    }
                    let _ = writeln!(
                        out,
                        "  {:<22} {:<22} {:<20} text",
                        "as written", "UTC", "from"
                    );
                    for ev in events {
                        let who = ev
                            .participant
                            .as_ref()
                            .and_then(|id| p.participant(id))
                            .map(|x| x.display.as_str())
                            .unwrap_or("-");
                        let _ = writeln!(
                            out,
                            "  {:<22} {:<22} {:<20} {}",
                            ev.source_time.chars().take(22).collect::<String>(),
                            ev.timestamp
                                .as_ref()
                                .map(rsmf::rsmf_time)
                                .unwrap_or_else(|| "-".into()),
                            who.chars().take(20).collect::<String>(),
                            ev.body
                                .replace('\n', " ")
                                .chars()
                                .take(60)
                                .collect::<String>()
                        );
                    }
                }
            }
            Err(err) if err.is_usage() => {
                eprintln!("error: {}: {}", e.primary.display(), err);
                code = EXIT_USAGE;
            }
            Err(err) => {
                eprintln!("error: {}: {}", e.primary.display(), err);
                if code == EXIT_OK {
                    code = EXIT_ERROR;
                }
            }
        }
    }
    if !orphans.is_empty() && !a.json {
        let _ = writeln!(out, "not part of any export: {} file(s)", orphans.len());
    }
    code
}

fn validate(paths: Vec<PathBuf>) -> u8 {
    let mut files = vec![];
    for p in paths {
        if p.is_dir() {
            for e in walkdir::WalkDir::new(&p)
                .sort_by_file_name()
                .into_iter()
                .flatten()
            {
                if e.path()
                    .extension()
                    .is_some_and(|x| x.eq_ignore_ascii_case("rsmf"))
                {
                    files.push(e.path().to_path_buf());
                }
            }
        } else {
            files.push(p);
        }
    }
    if files.is_empty() {
        eprintln!("error: no .rsmf file found");
        return EXIT_ERROR;
    }
    let mut code = EXIT_OK;
    for f in &files {
        match rsmf::validate_file(f) {
            Ok((v, _)) if v.ok() => println!(
                "OK       {}  ({} events, {} attachments)",
                f.display(),
                v.events,
                v.attachments
            ),
            Ok((v, _)) => {
                println!("INVALID  {}", f.display());
                for e in v
                    .schema_errors
                    .iter()
                    .map(|e| format!("schema: {e}"))
                    .chain(v.strict_errors.iter().map(|e| format!("strict: {e}")))
                {
                    println!("    {e}");
                }
                code = EXIT_FINDINGS;
            }
            Err(e) => {
                println!("ERROR    {}: {e}", f.display());
                if code == EXIT_OK {
                    code = EXIT_ERROR;
                }
            }
        }
    }
    code
}
