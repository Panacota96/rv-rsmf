//! WhatsApp "Export chat" parser: a `.txt` (Android or iOS) or a `.zip` (the `.txt` plus media).
//!
//! ```text
//!  line ──► header? ──yes──► [date, time] author: text      → message (attachment / media omitted?)
//!    │                  └──► [date, time] text without author → disclaimer / join / leave / history
//!    └──no──► before the first header → UNPARSED_LINES; after → continuation of the previous message
//!  all headers ──► date order (a component > 12 decides; otherwise the examiner's --date-order)
//!  local time + --timezone ──► rsmf::local_to_utc (DST gaps/overlaps counted)
//! ```
//!
//! WhatsApp has no published export format. The header shapes follow the test corpus of
//! whatsapp-chat-parser (MIT, © Loris Bettazza / Pustur, https://github.com/Pustur/whatsapp-chat-parser):
//! its idea of one shared date/time prefix for messages and system lines was adapted here; the regexes
//! below are our own and stricter (anchored ` - ` / `[...]` separators, explicit AM/PM forms).

use super::{DateOrder, ExportInput, ParseError, ParseOptions};
use crate::analyzer::{Finding, Severity};
use crate::model::{
    Attachment, AttachmentData, Conversation, ConversationType, Direction, Event, EventType,
    ParsedExport, Participant, SourceLoc, stable_id,
};
use crate::report::capped_list;
use crate::rsmf::local_to_utc;
use chrono::NaiveDate;
use regex::Regex;
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Day, month and year in any order, separated by `/ . -` (optionally followed by a space, as in
/// `13. 6. 2018.`); 2- or 4-digit years.
const DATE: &str = r"(?P<d1>\d{1,4})[-/.]\s?(?P<d2>\d{1,2})[-/.]\s?(?P<d3>\d{2,4})\.?";
/// `9:05`, `21.25.15`, `9:15:02 p. m.`, `1:30 PM`, `9:10 a.m.` — `\s` is Unicode-aware, so U+00A0 and
/// U+202F (iOS 17+) count as spaces.
const TIME: &str =
    r"(?P<h>\d{1,2})[:.](?P<mi>\d{2})(?:[:.](?P<s>\d{2}))?(?:\s?(?P<ap>[aApP])\.?\s?[mM]\.?)?";

/// Android: `02/03/2026, 10:00 - Ana: text`.
static ANDROID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^(?P<stamp>{DATE},?\s+{TIME})\s[-\u{{2013}}]\s(?P<rest>.*)$"
    ))
    .expect("android header regex")
});
/// iOS: `[4/3/26, 9:15:02 p. m.] Ana: text`.
static IOS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^\[(?P<stamp>{DATE},?\s+{TIME})\]\s?(?P<rest>.*)$"
    ))
    .expect("ios header regex")
});
/// iOS attachment: `<attached: 00000042-PHOTO-2020-06-07-15-13-20.jpg>` and localised forms.
static IOS_ATTACHMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^<(?:attached|adjunto|pi\x{E8}ce jointe|anhang|anexo|allegato)\s?:\s*(?P<n>[^<>\n]+?)\s*>",
    )
    .expect("ios attachment regex")
});
/// Android attachment: `IMG-20210428-WA0001.jpg (file attached)` and localised forms.
static ANDROID_ATTACHMENT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(?P<n>[^\n]+?)\s\((?:file attached|archivo adjunto|fichier joint|datei angeh\x{E4}ngt|arquivo anexado|file allegato)\)",
    )
    .expect("android attachment regex")
});
/// A display name that is only a phone number.
static NUMBER_ONLY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\+?[\d\s\-()]+$").expect("number regex"));

/// Whole-message texts that mean "the media was left out of the export" (Android `<Media omitted>`,
/// iOS "image omitted"…). The Android strings are the commonly reported ones; the iOS ones are unverified.
const MEDIA_OMITTED: &[&str] = &[
    "<media omitted>",
    "<multimedia omitido>",
    "<archivo omitido>",
    "<médias omis>",
    "<medien ausgeschlossen>",
    "<mídia oculta>",
    "<arquivo de mídia oculto>",
    "<media omessi>",
    "image omitted",
    "video omitted",
    "audio omitted",
    "sticker omitted",
    "gif omitted",
    "document omitted",
    "imagen omitida",
    "video omitido",
    "audio omitido",
    "sticker omitido",
    "gif omitido",
    "documento omitido",
    "image absente",
    "vidéo absente",
    "audio omis",
    "autocollant omis",
    "bild weggelassen",
    "video weggelassen",
    "audio weggelassen",
    "sticker weggelassen",
    "imagem ocultada",
    "vídeo ocultado",
    "áudio ocultado",
    "figurinha omitida",
    "immagine omessa",
    "video omesso",
    "audio omesso",
    "sticker omesso",
];

/// How the exporting phone names its owner as an author.
const YOU: &[&str] = &["You", "Tú", "Vous", "Du", "Você", "Tu"];

/// File-name prefixes WhatsApp gives exports, by language: the rest is the chat name.
const NAME_PREFIXES: &[&str] = &[
    "WhatsApp Chat - ",
    "WhatsApp Chat with ",
    "Chat de WhatsApp con ",
    "Chat de WhatsApp - ",
    "Discussion WhatsApp avec ",
    "WhatsApp-Chat mit ",
    "Conversa do WhatsApp com ",
    "Chat WhatsApp con ",
];

fn is_mark(c: char) -> bool {
    matches!(c, '\u{200E}' | '\u{200F}' | '\u{FEFF}')
}

/// Bidi controls WhatsApp puts around names and numbers.
fn is_bidi(c: char) -> bool {
    matches!(c, '\u{200E}' | '\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

fn strip_marks(s: &str) -> &str {
    s.trim_start_matches(is_mark)
}

/// One header line, before the date order is known.
#[derive(Clone, Debug)]
struct Header {
    line: u64,
    stamp: String,
    d: [u32; 3],
    d1_len: usize,
    hour: u32,
    minute: u32,
    second: u32,
    /// Some(true) = PM, Some(false) = AM, None = 24-hour clock.
    pm: Option<bool>,
    ios: bool,
    rest: String,
    raw: String,
}

fn match_header(line: &str, line_no: u64) -> Option<Header> {
    let l = strip_marks(line);
    let (c, ios) = match IOS.captures(l) {
        Some(c) => (c, true),
        None => (ANDROID.captures(l)?, false),
    };
    let n = |k: &str| c.name(k).and_then(|m| m.as_str().parse::<u32>().ok());
    Some(Header {
        line: line_no,
        stamp: c["stamp"].to_string(),
        d: [n("d1")?, n("d2")?, n("d3")?],
        d1_len: c["d1"].len(),
        hour: n("h")?,
        minute: n("mi")?,
        second: n("s").unwrap_or(0),
        pm: c.name("ap").map(|m| m.as_str().eq_ignore_ascii_case("p")),
        ios,
        rest: c["rest"].to_string(),
        raw: l.to_string(),
    })
}

/// Whether the first bytes of a .txt look like a WhatsApp export: one of the first ten non-empty lines
/// is a WhatsApp header.
pub fn sniff(head: &[u8]) -> bool {
    let text = String::from_utf8_lossy(head);
    text.lines()
        .map(|l| l.trim_end_matches('\r'))
        .filter(|l| !strip_marks(l).trim().is_empty())
        .take(10)
        .any(|l| match_header(l, 0).is_some())
}

fn is_txt(name: &str) -> bool {
    name.to_ascii_lowercase().ends_with(".txt") && !name.starts_with("__MACOSX")
}

/// Whether a zip holds a WhatsApp chat (a .txt whose first lines look like WhatsApp lines).
pub fn zip_has_chat(zip: &Path) -> bool {
    let Ok(f) = std::fs::File::open(zip) else {
        return false;
    };
    let Ok(mut z) = zip::ZipArchive::new(f) else {
        return false;
    };
    chat_entry(&mut z).is_some()
}

fn base_name(name: &str) -> &str {
    name.rsplit(['/', '\\']).next().unwrap_or(name)
}

/// Entry names of a zip (names that are not valid text are skipped).
fn zip_names<R: std::io::Read + std::io::Seek>(z: &zip::ZipArchive<R>) -> Vec<String> {
    z.file_names()
        .filter_map(|n| n.ok().map(|n| n.to_string()))
        .collect()
}

/// The chat inside a zip: `_chat.txt` (iOS) first, else the first .txt (sorted) that sniffs as WhatsApp.
fn chat_entry<R: std::io::Read + std::io::Seek>(z: &mut zip::ZipArchive<R>) -> Option<String> {
    let mut names: Vec<String> = zip_names(z)
        .into_iter()
        .filter(|n| is_txt(n) && !n.ends_with('/'))
        .collect();
    names.sort_by_key(|n| (base_name(n) != "_chat.txt", n.clone()));
    for n in names.into_iter().take(50) {
        let Ok(f) = z.by_name(&n) else { continue };
        let mut head = vec![];
        if f.take(64 * 1024).read_to_end(&mut head).is_ok() && sniff(&head) {
            return Some(n);
        }
    }
    None
}

/// Where referenced media can be found.
enum Media {
    Zip {
        zip: PathBuf,
        /// base name → entry name
        entries: HashMap<String, String>,
    },
    Folder {
        root: PathBuf,
        /// base name → member path
        members: HashMap<String, PathBuf>,
    },
}

impl Media {
    fn find(&self, name: &str) -> Option<AttachmentData> {
        if name.is_empty() || name.contains(['/', '\\']) || name == "." || name == ".." {
            return None;
        }
        match self {
            Media::Zip { zip, entries } => entries.get(name).map(|e| AttachmentData::ZipMember {
                zip: zip.clone(),
                name: e.clone(),
            }),
            Media::Folder { root, members } => members
                .get(name)
                .cloned()
                .or_else(|| Some(root.join(name)).filter(|p| p.is_file()))
                .map(|path| AttachmentData::File { path }),
        }
    }
}

fn chat_name(primary: &Path, inner: Option<&str>) -> String {
    let stem = |s: &str| -> String {
        Path::new(s)
            .file_stem()
            .map(|x| x.to_string_lossy().to_string())
            .unwrap_or_default()
    };
    let from = |s: &str| -> Option<String> {
        NAME_PREFIXES
            .iter()
            .find_map(|p| s.strip_prefix(p))
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
    };
    let own = stem(&primary.file_name().unwrap_or_default().to_string_lossy());
    from(&own)
        .or_else(|| inner.and_then(|i| from(&stem(base_name(i)))))
        .unwrap_or(own)
}

/// Kind of an author-less line, by its wording in EN/ES/FR/DE/PT/IT. None = not a known system text.
fn system_kind(text: &str) -> Option<EventType> {
    let t = text.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| t.contains(w));
    if has(&[
        "end-to-end",
        "extremo a extremo",
        "bout en bout",
        "ende-zu-ende",
        "ponta a ponta",
        "ponta-a-ponta",
    ]) {
        return Some(EventType::Disclaimer);
    }
    if has(&[
        "created group",
        "created this group",
        "creó el grupo",
        "creaste el grupo",
        "a créé le groupe",
        "avez créé le groupe",
        "criou o grupo",
        "ha creato il gruppo",
        "hai creato il gruppo",
    ]) || (t.contains("gruppe") && t.contains("erstellt"))
    {
        return Some(EventType::History);
    }
    if has(&[
        " left",
        "salió",
        "saliste",
        "est parti",
        "êtes parti",
        "verlassen",
        " saiu",
        "è uscito",
        "sei uscito",
        "removed",
        "eliminó",
        "a retiré",
        "entfernt",
        "removeu",
        "ha rimosso",
    ]) {
        return Some(EventType::Leave);
    }
    if has(&[
        " added",
        "añadió",
        "agregó",
        "a ajouté",
        "hinzugefügt",
        "adicionou",
        "ha aggiunto",
        " joined",
        "se unió",
        "a rejoint",
        "beigetreten",
        " entrou",
        "si è unito",
        "è entrato",
    ]) {
        return Some(EventType::Join);
    }
    None
}

/// `Author: text` on the header line. Authors are short and never quoted; anything else is a system line.
fn split_author(rest: &str) -> Option<(&str, &str)> {
    let (a, b) = match rest.find(": ") {
        Some(i) => (&rest[..i], &rest[i + 2..]),
        None => (rest.strip_suffix(':')?, ""),
    };
    let ok = !a.trim().is_empty()
        && a.chars().count() <= 80
        && !a.contains([
            '"', '\u{201C}', '\u{201D}', '\u{00AB}', '\u{00BB}', '\u{201E}',
        ]);
    ok.then_some((a, b))
}

fn clean_author(a: &str) -> String {
    a.chars()
        .filter(|c| !is_bidi(*c))
        .collect::<String>()
        .trim()
        .to_string()
}

/// The decided date order: (order, basis, finding, whether --date-order was used).
type OrderDecision = (DateOrder, String, Finding, bool);

/// Decides the day/month order for the whole file. The examiner's `--date-order` is used only when the
/// file itself cannot decide (every day and month ≤ 12); when the file decides, its order wins, so one run
/// can convert several exports made with different phone settings (the unused option is recorded in facts).
fn decide_order(
    headers: &[Header],
    chosen: Option<DateOrder>,
) -> Result<OrderDecision, ParseError> {
    let inferred = |order: DateOrder, h: &Header, basis: String| {
        (
            order,
            basis,
            Finding::new("DATE_ORDER_INFERRED", Severity::Info)
                .with("order", order.name())
                .with("line", h.line.to_string())
                .with("example", h.stamp.clone()),
            false,
        )
    };
    if let Some(h) = headers.iter().find(|h| h.d1_len >= 3) {
        if let Some(other) = headers.iter().find(|x| x.d1_len < 3) {
            return Err(ParseError::Invalid(format!(
                "mixed date formats: line {} (\"{}\") starts with a year, line {} (\"{}\") does not",
                h.line, h.stamp, other.line, other.stamp
            )));
        }
        return Ok(inferred(
            DateOrder::Ymd,
            h,
            format!("line {} starts with a 4-digit year", h.line),
        ));
    }
    let first = headers.iter().find(|h| h.d[0] > 12);
    let second = headers.iter().find(|h| h.d[1] > 12);
    match (first, second, chosen) {
        (Some(a), Some(b), _) => Err(ParseError::Invalid(format!(
            "inconsistent dates: line {} (\"{}\") can only be day-first, line {} (\"{}\") only month-first",
            a.line, a.stamp, b.line, b.stamp
        ))),
        (Some(a), None, _) => Ok(inferred(
            DateOrder::Dmy,
            a,
            format!("line {}: first component {} > 12", a.line, a.d[0]),
        )),
        (None, Some(b), _) => Ok(inferred(
            DateOrder::Mdy,
            b,
            format!("line {}: second component {} > 12", b.line, b.d[1]),
        )),
        // Year-first cannot apply to dates whose first component has one or two digits.
        (None, None, Some(c)) if c != DateOrder::Ymd => Ok((
            c,
            "examiner choice (--date-order): every day and month is 12 or less".into(),
            Finding::new("DATE_ORDER_CHOSEN", Severity::Info).with("order", c.name()),
            true,
        )),
        (None, None, _) => {
            let mut evidence: Vec<String> = vec![];
            let mut seen = std::collections::BTreeSet::new();
            for h in headers {
                if seen.insert(h.d) {
                    let line: String = h.raw.chars().take(160).collect();
                    evidence.push(format!("line {}: {line}", h.line));
                }
                if evidence.len() == 3 {
                    break;
                }
            }
            Err(ParseError::NeedsDateOrder {
                candidates: vec![DateOrder::Dmy, DateOrder::Mdy],
                evidence,
            })
        }
    }
}

fn local_time(h: &Header, order: DateOrder) -> Result<chrono::NaiveDateTime, ParseError> {
    let [a, b, c] = h.d;
    let (y, m, d) = match order {
        DateOrder::Dmy => (c, b, a),
        DateOrder::Mdy => (c, a, b),
        DateOrder::Ymd => (a, b, c),
    };
    // Two-digit years: WhatsApp exists since 2009, so 26 = 2026.
    let y = if y < 100 { 2000 + y } else { y };
    let bad = |what: &str| {
        ParseError::Invalid(format!(
            "line {} (\"{}\"): {what} as {}",
            h.line,
            h.stamp,
            order.name()
        ))
    };
    let date = NaiveDate::from_ymd_opt(y as i32, m, d).ok_or_else(|| bad("not a valid date"))?;
    let hour = match h.pm {
        None => h.hour,
        Some(pm) => {
            if !(1..=12).contains(&h.hour) {
                return Err(bad("hour not valid on a 12-hour clock"));
            }
            h.hour % 12 + if pm { 12 } else { 0 }
        }
    };
    date.and_hms_opt(hour, h.minute, h.second)
        .ok_or_else(|| bad("not a valid time"))
}

/// One message or system line with its continuation lines.
struct Block {
    header: Header,
    line_end: u64,
    /// Text after the header (author included), continuation lines appended with '\n'.
    tail: Vec<String>,
}

struct Text {
    text: String,
    file_label: String,
    inner: Option<String>,
    media: Media,
}

fn load(export: &ExportInput) -> Result<Text, ParseError> {
    let primary = &export.primary;
    let file_name = primary
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let is_zip = primary
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    let (bytes, file_label, inner, media) = if is_zip {
        let f = std::fs::File::open(primary)
            .map_err(|e| ParseError::Invalid(format!("{}: {e}", primary.display())))?;
        let mut z = zip::ZipArchive::new(f)
            .map_err(|e| ParseError::Invalid(format!("{}: {e}", primary.display())))?;
        let entry = chat_entry(&mut z).ok_or_else(|| {
            ParseError::Invalid(format!(
                "{}: no WhatsApp chat (.txt) inside the zip",
                primary.display()
            ))
        })?;
        let mut bytes = vec![];
        z.by_name(&entry)
            .and_then(|mut f| f.read_to_end(&mut bytes).map_err(Into::into))
            .map_err(|e| ParseError::Invalid(format!("{}!{entry}: {e}", primary.display())))?;
        let mut entries = HashMap::new();
        for n in zip_names(&z) {
            if !n.ends_with('/') && n != entry {
                entries
                    .entry(base_name(&n).to_string())
                    .or_insert_with(|| n.clone());
            }
        }
        (
            bytes,
            entry.clone(),
            Some(entry),
            Media::Zip {
                zip: primary.clone(),
                entries,
            },
        )
    } else {
        let bytes = export
            .read(primary)
            .map_err(|e| ParseError::Invalid(format!("{}: {e}", primary.display())))?;
        let mut members = HashMap::new();
        for m in export.members.iter().filter(|m| *m != primary) {
            if let Some(n) = m.file_name() {
                members
                    .entry(n.to_string_lossy().to_string())
                    .or_insert_with(|| m.clone());
            }
        }
        (
            bytes,
            file_name,
            None,
            Media::Folder {
                root: export.root.clone(),
                members,
            },
        )
    };
    let text = match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).to_string(),
    };
    Ok(Text {
        text,
        file_label,
        inner,
        media,
    })
}

pub fn parse(export: &ExportInput, opts: &ParseOptions) -> Result<ParsedExport, ParseError> {
    let Text {
        text,
        file_label,
        inner,
        media,
    } = load(export)?;
    let valid_utf8 = !text.contains('\u{FFFD}');

    // 1. Lines → blocks.
    let mut blocks: Vec<Block> = vec![];
    let mut unparsed: Vec<String> = vec![];
    let mut lines_read = 0u64;
    let all: Vec<&str> = text
        .strip_suffix('\n')
        .unwrap_or(&text)
        .split('\n')
        .collect();
    for (i, raw) in all.into_iter().enumerate() {
        let line_no = i as u64 + 1;
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        lines_read += 1;
        if let Some(h) = match_header(line, line_no) {
            blocks.push(Block {
                tail: vec![h.rest.clone()],
                line_end: line_no,
                header: h,
            });
        } else if let Some(b) = blocks.last_mut() {
            b.tail.push(strip_marks(line).to_string());
            b.line_end = line_no;
        } else if !strip_marks(line).trim().is_empty() {
            unparsed.push(format!(
                "line {line_no}: {}",
                line.chars().take(120).collect::<String>()
            ));
        }
    }
    if blocks.is_empty() {
        return Err(ParseError::Invalid(format!(
            "{file_label}: no WhatsApp message lines found"
        )));
    }
    let headers: Vec<Header> = blocks.iter().map(|b| b.header.clone()).collect();

    // 2. Date order, then time zone (both are the examiner's decision when the file does not settle them).
    let (order, basis, order_finding, option_used) = decide_order(&headers, opts.date_order)?;
    let tz = opts.timezone.ok_or(ParseError::NeedsTimezone)?;

    let mut out = ParsedExport::default();
    let conv_id = stable_id(
        "c",
        &export
            .primary
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
    );
    let ios = headers.iter().filter(|h| h.ios).count() * 2 > headers.len();
    let custodian = opts
        .custodian
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty());
    let owner_known = custodian.is_some()
        || blocks.iter().any(|b| {
            split_author(strip_marks(&b.header.rest))
                .is_some_and(|(a, _)| YOU.contains(&clean_author(a).as_str()))
        });

    let mut index: HashMap<String, usize> = HashMap::new();
    let mut authors: Vec<String> = vec![];
    let mut numbers: Vec<String> = vec![];
    let mut dst: Vec<String> = vec![];
    let mut omitted = 0usize;
    let mut not_found: Vec<String> = vec![];
    let mut messages = 0usize;

    for (n, b) in blocks.iter().enumerate() {
        let h = &b.header;
        let local = local_time(h, order)?;
        let (utc, problem) = local_to_utc(local, tz);
        let mut e = Event::message(
            format!("e-{:06}", n + 1),
            conv_id.clone(),
            SourceLoc {
                file: file_label.clone(),
                line: h.line,
                line_end: b.line_end,
                native_id: None,
            },
        );
        e.timestamp = Some(utc);
        e.source_time = h.stamp.clone();
        if let Some(p) = problem {
            dst.push(h.stamp.clone());
            e.custom.insert("local_time_issue".into(), p.into());
        }
        let first = strip_marks(&b.tail[0]);
        let rest_lines = &b.tail[1..];
        let join = |head: &str| -> String {
            std::iter::once(head)
                .chain(rest_lines.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join("\n")
        };
        match split_author(first) {
            Some((author_raw, text)) => {
                let body_raw = text;
                let body = strip_marks(body_raw);
                let author = clean_author(author_raw);
                // iOS writes system texts under the chat's name, after a U+200E mark.
                let system = body_raw
                    .starts_with('\u{200E}')
                    .then(|| system_kind(body))
                    .flatten();
                if let Some(kind) = system {
                    e.kind = kind;
                    e.body = join(body);
                    e.custom.insert("attributed_to".into(), author);
                } else {
                    messages += 1;
                    let is_you = YOU.contains(&author.as_str());
                    let display = match (is_you, custodian) {
                        (true, Some(c)) => c.to_string(),
                        _ => author.clone(),
                    };
                    if is_you && custodian.is_some() {
                        e.custom.insert("author_in_export".into(), author.clone());
                    }
                    let pid = stable_id("p", &display);
                    if !index.contains_key(&pid) {
                        let digits = display.chars().filter(char::is_ascii_digit).count();
                        let number_only = NUMBER_ONLY.is_match(&display) && digits >= 5;
                        if number_only {
                            numbers.push(display.clone());
                        }
                        index.insert(pid.clone(), out.participants.len());
                        out.participants.push(Participant {
                            id: pid.clone(),
                            display: display.clone(),
                            email: None,
                            account_id: number_only.then(|| display.clone()),
                            custom: BTreeMap::new(),
                        });
                        authors.push(pid.clone());
                    }
                    if owner_known {
                        let outgoing =
                            is_you || custodian.is_some_and(|c| c == display || c == author);
                        e.direction = Some(if outgoing {
                            Direction::Outgoing
                        } else {
                            Direction::Incoming
                        });
                    }
                    e.participant = Some(pid);
                    e.body = join(body);
                    // Attachments are named on the first line of the message.
                    let trimmed = body.trim();
                    if MEDIA_OMITTED
                        .iter()
                        .any(|m| trimmed.eq_ignore_ascii_case(m) || trimmed.to_lowercase() == *m)
                    {
                        omitted += 1;
                        e.attachments.push(Attachment {
                            display: trimmed.trim_matches(['<', '>']).to_string(),
                            data: AttachmentData::Missing {
                                reason:
                                    "media omitted when the chat was exported (\"without media\")"
                                        .into(),
                            },
                        });
                    } else if let Some(c) = IOS_ATTACHMENT
                        .captures(body)
                        .or_else(|| ANDROID_ATTACHMENT.captures(body))
                    {
                        let name = strip_marks(c["n"].trim()).to_string();
                        let data = media.find(&name).unwrap_or_else(|| {
                            not_found.push(name.clone());
                            AttachmentData::Missing {
                                reason: "named in the export but the file is not in it".into(),
                            }
                        });
                        e.attachments.push(Attachment {
                            display: name,
                            data,
                        });
                    }
                }
            }
            None => {
                e.kind = system_kind(first).unwrap_or(EventType::History);
                e.body = join(first);
            }
        }
        out.events.push(e);
    }

    // 3. Findings.
    out.findings.push(order_finding);
    if !unparsed.is_empty() {
        out.findings.push(
            Finding::new("UNPARSED_LINES", Severity::Medium)
                .with("count", unparsed.len().to_string())
                .with("examples", capped_list(&unparsed, 3)),
        );
    }
    if !dst.is_empty() {
        out.findings.push(
            Finding::new("DST_AMBIGUOUS_OR_MISSING_TIME", Severity::Medium)
                .with("count", dst.len().to_string())
                .with("timezone", tz.name())
                .with("examples", capped_list(&dst, 3)),
        );
    }
    if omitted > 0 {
        out.findings.push(
            Finding::new("MEDIA_OMITTED_IN_EXPORT", Severity::Low)
                .with("count", omitted.to_string()),
        );
    }
    if !not_found.is_empty() {
        out.findings.push(
            Finding::new("ATTACHMENT_REFERENCED_NOT_FOUND", Severity::Medium)
                .with("count", not_found.len().to_string())
                .with("examples", capped_list(&not_found, 3)),
        );
    }
    if !numbers.is_empty() {
        out.findings.push(
            Finding::new("PARTICIPANT_IDENTIFIED_BY_NUMBER_ONLY", Severity::Info)
                .with("count", numbers.len().to_string())
                .with("examples", capped_list(&numbers, 3)),
        );
    }
    if messages >= 39_000 {
        out.findings.push(
            Finding::new("POSSIBLE_EXPORT_LIMIT", Severity::Medium)
                .with("count", messages.to_string()),
        );
    }

    // 4. The conversation.
    let mut custom = BTreeMap::new();
    custom.insert("export_file".into(), file_label.clone());
    out.conversations.push(Conversation {
        id: conv_id,
        display: chat_name(&export.primary, inner.as_deref()),
        kind: if authors.len() <= 2 {
            ConversationType::Direct
        } else {
            ConversationType::Channel
        },
        participants: authors,
        custom,
    });

    let f = &mut out.facts;
    f.insert("variant".into(), if ios { "ios" } else { "android" }.into());
    f.insert("date_order".into(), order.name().into());
    f.insert("date_order_basis".into(), basis);
    if let Some(o) = opts.date_order {
        f.insert("date_order_option".into(), o.name().into());
        f.insert(
            "date_order_option_used".into(),
            if option_used { "yes" } else { "no" }.into(),
        );
    }
    f.insert("lines_read".into(), lines_read.to_string());
    f.insert("timezone".into(), tz.name().into());
    f.insert("chat_file".into(), file_label);
    if !valid_utf8 {
        f.insert(
            "encoding".into(),
            "not valid UTF-8: invalid bytes replaced by U+FFFD".into(),
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SourceKind;

    fn tz(s: &str) -> chrono_tz::Tz {
        s.parse().unwrap()
    }

    fn run(name: &str, text: &str, opts: &ParseOptions) -> Result<ParsedExport, ParseError> {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join(name);
        std::fs::write(&p, text).unwrap();
        let e = ExportInput {
            kind: SourceKind::Whatsapp,
            primary: p.clone(),
            root: d.path().to_path_buf(),
            members: vec![p],
        };
        parse(&e, opts)
    }

    fn opts(zone: &str, order: Option<DateOrder>) -> ParseOptions {
        ParseOptions {
            timezone: Some(tz(zone)),
            date_order: order,
            ..ParseOptions::default()
        }
    }

    fn codes(p: &ParsedExport) -> Vec<&str> {
        p.findings.iter().map(|f| f.code.as_str()).collect()
    }

    #[test]
    fn header_variants() {
        let cases = [
            ("02/03/2026, 10:00 - Ana: hi", [2, 3, 2026], 10, None, false),
            (
                "2/3/26, 1:55 PM - Ana: hi",
                [2, 3, 26],
                1,
                Some(true),
                false,
            ),
            (
                "23/06/2018, 01:55 p.m. - Ana: hi",
                [23, 6, 2018],
                1,
                Some(true),
                false,
            ),
            (
                "03-06-2018, 01.55 AM - Ana: hi",
                [3, 6, 2018],
                1,
                Some(false),
                false,
            ),
            ("[06.13.18 21:25:15] Ana: hi", [6, 13, 18], 21, None, true),
            (
                "[3/6/18 1:55:00 p. m.] Ana: hi",
                [3, 6, 18],
                1,
                Some(true),
                true,
            ),
            (
                "[4/3/26, 9:15:02\u{202F}p.\u{A0}m.] Ana: hi",
                [4, 3, 26],
                9,
                Some(true),
                true,
            ),
            (
                "\u{200E}[13. 6. 2018. 21:25:15] Ana: hi",
                [13, 6, 2018],
                21,
                None,
                true,
            ),
            ("2026-03-02, 10:00 - Ana: hi", [2026, 3, 2], 10, None, false),
            ("29.03.26, 02:30 - Ana: hi", [29, 3, 26], 2, None, false),
        ];
        for (line, d, h, pm, ios) in cases {
            let x = match_header(line, 1).unwrap_or_else(|| panic!("no match: {line}"));
            assert_eq!((x.d, x.hour, x.pm, x.ios), (d, h, pm, ios), "{line}");
            assert_eq!(x.rest, "Ana: hi", "{line}");
        }
        for not in [
            "1. Kick-off at 10:00",
            "Meeting 02/03/2026 at 10:00",
            "2026-03-02 10:00:00 INFO started",
        ] {
            assert!(match_header(not, 1).is_none(), "{not}");
        }
    }

    #[test]
    fn am_pm_conversion() {
        let o = opts("UTC", Some(DateOrder::Mdy));
        let p = run(
            "c.txt",
            "1/2/26, 12:05 AM - A: midnight\n1/2/26, 12:05 PM - A: noon\n1/2/26, 1:05 p. m. - A: one\n",
            &o,
        )
        .unwrap();
        let t: Vec<String> = p
            .events
            .iter()
            .map(|e| e.timestamp.unwrap().format("%H:%M").to_string())
            .collect();
        assert_eq!(t, ["00:05", "12:05", "13:05"]);
        assert_eq!(codes(&p), ["DATE_ORDER_CHOSEN"]);
    }

    #[test]
    fn continuation_and_author_with_colon() {
        let p = run(
            "c.txt",
            "13/03/2026, 18:46 - Ana Ruiz: Note: deadline\nline two: still Ana\n\nline four\n13/03/2026, 18:47 - Bob: ok\n",
            &opts("Europe/Paris", None),
        )
        .unwrap();
        assert_eq!(p.events.len(), 2);
        let e = &p.events[0];
        assert_eq!(e.body, "Note: deadline\nline two: still Ana\n\nline four");
        assert_eq!((e.source.line, e.source.line_end), (1, 4));
        assert_eq!(
            p.participant(e.participant.as_ref().unwrap())
                .unwrap()
                .display,
            "Ana Ruiz"
        );
        assert_eq!(e.source_time, "13/03/2026, 18:46");
        assert_eq!(
            e.timestamp.unwrap().to_rfc3339(),
            "2026-03-13T17:46:00+00:00"
        );
        assert_eq!(p.facts["date_order"], "dmy");
        assert_eq!(p.conversations[0].kind, ConversationType::Direct);
    }

    #[test]
    fn date_order_rules() {
        let amb = "03/04/26, 9:05 AM - A: x\n05/04/26, 1:30 PM - B: y\n";
        match run("c.txt", amb, &opts("UTC", None)) {
            Err(ParseError::NeedsDateOrder {
                candidates,
                evidence,
            }) => {
                assert_eq!(candidates, [DateOrder::Dmy, DateOrder::Mdy]);
                assert_eq!(evidence.len(), 2);
            }
            other => panic!("{other:?}"),
        }
        // Ambiguous file without a time zone: the date order is asked first, the zone next.
        assert!(matches!(
            run("c.txt", amb, &ParseOptions::default()),
            Err(ParseError::NeedsDateOrder { .. })
        ));
        assert_eq!(
            run(
                "c.txt",
                amb,
                &ParseOptions {
                    date_order: Some(DateOrder::Dmy),
                    ..ParseOptions::default()
                }
            ),
            Err(ParseError::NeedsTimezone)
        );
        let p = run("c.txt", amb, &opts("UTC", Some(DateOrder::Mdy))).unwrap();
        assert_eq!(
            p.events[0].timestamp.unwrap().format("%m-%d").to_string(),
            "03-04"
        );
        // The file decides: a contradicting --date-order is not used, only recorded.
        let dmy = "25/04/26, 9:05 - A: x\n";
        let p = run("c.txt", dmy, &opts("UTC", Some(DateOrder::Mdy))).unwrap();
        assert_eq!(codes(&p), ["DATE_ORDER_INFERRED"]);
        assert_eq!(p.facts["date_order"], "dmy");
        assert_eq!(p.facts["date_order_option"], "mdy");
        assert_eq!(p.facts["date_order_option_used"], "no");
        assert_eq!(
            p.events[0].timestamp.unwrap().format("%m-%d").to_string(),
            "04-25"
        );
        let p = run("c.txt", amb, &opts("UTC", Some(DateOrder::Dmy))).unwrap();
        assert_eq!(codes(&p), ["DATE_ORDER_CHOSEN"]);
        assert_eq!(p.facts["date_order_option_used"], "yes");
        // Year-first cannot be chosen for an ambiguous day/month file: still a question for the examiner.
        assert!(matches!(
            run("c.txt", amb, &opts("UTC", Some(DateOrder::Ymd))),
            Err(ParseError::NeedsDateOrder { .. })
        ));
        // Month-first inferred from the second component.
        let p = run("c.txt", "04/25/26, 9:05 - A: x\n", &opts("UTC", None)).unwrap();
        assert_eq!(p.facts["date_order"], "mdy");
        assert_eq!(p.findings[0].params["line"], "1");
        // Year first.
        let p = run("c.txt", "2026-04-05, 9:05 - A: x\n", &opts("UTC", None)).unwrap();
        assert_eq!(p.facts["date_order"], "ymd");
        let p = run(
            "c.txt",
            "2026-04-05, 9:05 - A: x\n",
            &opts("UTC", Some(DateOrder::Dmy)),
        )
        .unwrap();
        assert_eq!(p.facts["date_order"], "ymd");
        assert_eq!(p.facts["date_order_option_used"], "no");
        // Both day-first and month-first evidence: refuse.
        assert!(matches!(
            run(
                "c.txt",
                "25/04/26, 9:05 - A: x\n04/25/26, 9:05 - A: y\n",
                &opts("UTC", None)
            ),
            Err(ParseError::Invalid(_))
        ));
        // Impossible date in the inferred order.
        assert!(matches!(
            run("c.txt", "31/02/26, 9:05 - A: x\n", &opts("UTC", None)),
            Err(ParseError::Invalid(_))
        ));
    }

    #[test]
    fn dst_problems_are_counted_with_the_original_text() {
        let p = run(
            "c.txt",
            "29.03.26, 02:30 - A: gap\n25.10.26, 02:30 - B: overlap\n25.10.26, 04:00 - B: fine\n",
            &opts("Europe/Paris", None),
        )
        .unwrap();
        let f = p
            .findings
            .iter()
            .find(|f| f.code == "DST_AMBIGUOUS_OR_MISSING_TIME")
            .unwrap();
        assert_eq!(f.severity, Severity::Medium);
        assert_eq!(f.params["count"], "2");
        assert_eq!(f.params["timezone"], "Europe/Paris");
        assert_eq!(f.params["examples"], "29.03.26, 02:30, 25.10.26, 02:30");
        assert_eq!(p.events[1].custom["local_time_issue"], "ambiguous");
        assert_eq!(
            crate::rsmf::rsmf_time(&p.events[1].timestamp.unwrap()),
            "2026-10-25T00:30:00Z"
        );
    }

    #[test]
    fn system_lines_and_unparsed() {
        let text = "header junk\n\n\
02/03/2026, 09:58 - Messages and calls are end-to-end encrypted. Tap to learn more.\n\
02/03/2026, 09:59 - Ana created group \"Faro: 2026\"\n\
02/03/2026, 09:59 - Ana añadió a Bob\n\
02/03/2026, 10:00 - Bob a rejoint le groupe\n\
02/03/2026, 10:01 - Bob est parti\n\
02/03/2026, 10:02 - Ana changed this group's icon\n\
13/03/2026, 10:03 - Ana: hi\n";
        let p = run("c.txt", text, &opts("UTC", None)).unwrap();
        let kinds: Vec<EventType> = p.events.iter().map(|e| e.kind).collect();
        use EventType::*;
        assert_eq!(
            kinds,
            [Disclaimer, History, Join, Join, Leave, History, Message]
        );
        assert!(p.events[..6].iter().all(|e| e.participant.is_none()));
        let u = p
            .findings
            .iter()
            .find(|f| f.code == "UNPARSED_LINES")
            .unwrap();
        assert_eq!(u.params["count"], "1");
        assert_eq!(u.params["examples"], "line 1: header junk");
        // Localised disclaimers.
        for d in [
            "Los mensajes y las llamadas están cifrados de extremo a extremo.",
            "Les messages et les appels sont chiffrés de bout en bout.",
            "Nachrichten und Anrufe sind Ende-zu-Ende-verschlüsselt.",
            "As mensagens e as chamadas são protegidas com a criptografia de ponta a ponta.",
            "I messaggi e le chiamate sono crittografati end-to-end.",
        ] {
            assert_eq!(system_kind(d), Some(Disclaimer), "{d}");
        }
    }

    #[test]
    fn ios_system_text_under_the_chat_name_is_not_a_message() {
        let text = "[4/3/26, 9:10:00 p. m.] Lucía: \u{200E}Los mensajes y las llamadas están cifrados de extremo a extremo.\n\
[15/3/26, 9:12:30 p. m.] Lucía: Hola\n";
        let p = run("_chat.txt", text, &opts("America/Bogota", None)).unwrap();
        assert_eq!(p.events[0].kind, EventType::Disclaimer);
        assert_eq!(p.events[0].participant, None);
        assert_eq!(p.events[0].custom["attributed_to"], "Lucía");
        assert_eq!(p.events[1].kind, EventType::Message);
        assert_eq!(p.facts["variant"], "ios");
    }

    #[test]
    fn localised_media_strings() {
        for (line, omitted, name) in [
            ("<Media omitted>", true, ""),
            ("<Multimedia omitido>", true, ""),
            ("<Médias omis>", true, ""),
            ("<Medien ausgeschlossen>", true, ""),
            ("<Mídia oculta>", true, ""),
            ("<Media omessi>", true, ""),
            ("\u{200E}image omitted", true, ""),
            (
                "\u{200E}<attached: 00000042-PHOTO-2020-06-07-15-13-20.jpg>",
                false,
                "00000042-PHOTO-2020-06-07-15-13-20.jpg",
            ),
            (
                "\u{200E}<adjunto: 00000003-PHOTO-2026-03-04-21-15-02.jpg>",
                false,
                "00000003-PHOTO-2026-03-04-21-15-02.jpg",
            ),
            ("<pièce jointe : doc 1.pdf>", false, "doc 1.pdf"),
            (
                "IMG-20210428-WA0001.jpg (file attached)",
                false,
                "IMG-20210428-WA0001.jpg",
            ),
            (
                "IMG-20210428-WA0001.jpg (archivo adjunto)",
                false,
                "IMG-20210428-WA0001.jpg",
            ),
            (
                "Rapport 2026.pdf (fichier joint)",
                false,
                "Rapport 2026.pdf",
            ),
            (
                "PTT-20210428-WA0002.opus (Datei angehängt)",
                false,
                "PTT-20210428-WA0002.opus",
            ),
        ] {
            let p = run(
                "c.txt",
                &format!("13/03/2026, 10:00 - Ana: {line}\n"),
                &opts("UTC", None),
            )
            .unwrap();
            let e = &p.events[0];
            assert_eq!(e.attachments.len(), 1, "{line}");
            let has = |c: &str| p.findings.iter().any(|f| f.code == c);
            if omitted {
                assert!(has("MEDIA_OMITTED_IN_EXPORT"), "{line}");
            } else {
                assert_eq!(e.attachments[0].display, name, "{line}");
                assert!(has("ATTACHMENT_REFERENCED_NOT_FOUND"), "{line}");
            }
            assert!(matches!(
                e.attachments[0].data,
                AttachmentData::Missing { .. }
            ));
        }
    }

    #[test]
    fn media_next_to_the_txt_and_path_tricks() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("WhatsApp Chat with Ana Ruiz.txt");
        std::fs::write(
            &p,
            "13/03/2026, 10:00 - Ana: IMG-1.jpg (file attached)\n13/03/2026, 10:01 - Ana: ../secret.jpg (file attached)\n",
        )
        .unwrap();
        std::fs::write(d.path().join("IMG-1.jpg"), b"\xFF\xD8\xFF").unwrap();
        let e = ExportInput {
            kind: SourceKind::Whatsapp,
            primary: p.clone(),
            root: d.path().to_path_buf(),
            members: vec![p, d.path().join("IMG-1.jpg")],
        };
        let out = parse(&e, &opts("UTC", None)).unwrap();
        assert_eq!(
            out.events[0].attachments[0].data,
            AttachmentData::File {
                path: d.path().join("IMG-1.jpg")
            }
        );
        assert!(matches!(
            out.events[1].attachments[0].data,
            AttachmentData::Missing { .. }
        ));
        assert_eq!(out.conversations[0].display, "Ana Ruiz");
    }

    #[test]
    fn participants_numbers_and_custodian() {
        let text = "13/03/2026, 10:00 - \u{202A}+33 6 00 00 00 01\u{202C}: hola\n13/03/2026, 10:01 - Tú: hi\n13/03/2026, 10:02 - Bob: hey\n";
        let p = run("c.txt", text, &opts("UTC", None)).unwrap();
        assert_eq!(p.participants[0].display, "+33 6 00 00 00 01");
        assert_eq!(
            p.participants[0].account_id.as_deref(),
            Some("+33 6 00 00 00 01")
        );
        assert_eq!(p.participants[0].id, stable_id("p", "+33 6 00 00 00 01"));
        assert_eq!(p.participants[1].display, "Tú");
        let f = p
            .findings
            .iter()
            .find(|f| f.code == "PARTICIPANT_IDENTIFIED_BY_NUMBER_ONLY")
            .unwrap();
        assert_eq!(
            (f.params["count"].as_str(), f.severity),
            ("1", Severity::Info)
        );
        assert_eq!(p.conversations[0].kind, ConversationType::Channel);
        assert_eq!(p.events[1].direction, Some(Direction::Outgoing));
        assert_eq!(p.events[0].direction, Some(Direction::Incoming));
        let mut o = opts("UTC", None);
        o.custodian = Some("Carla Méndez".into());
        let p = run("c.txt", text, &o).unwrap();
        assert_eq!(p.participants[1].display, "Carla Méndez");
        assert_eq!(p.events[1].custom["author_in_export"], "Tú");
    }

    #[test]
    fn needs_timezone_and_garbage() {
        assert_eq!(
            run(
                "c.txt",
                "13/03/2026, 10:00 - A: x\n",
                &ParseOptions::default()
            ),
            Err(ParseError::NeedsTimezone)
        );
        assert!(matches!(
            run("c.txt", "not a chat\n", &opts("UTC", None)),
            Err(ParseError::Invalid(_))
        ));
        assert!(sniff("\u{FEFF}13/03/2026, 10:00 - A: x\n".as_bytes()));
        assert!(sniff(b"[06.13.18 21:25:15] a: m"));
        assert!(!sniff(b"Acta de entrega de equipos\nFirmado: Ana Ruiz\n"));
        assert!(!sniff(b"a,b,c\n1,2,3\n"));
    }

    #[test]
    fn zip_export() {
        let d = tempfile::tempdir().unwrap();
        let zp = d.path().join("WhatsApp Chat - Lucía Gómez.zip");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
            let o = zip::write::SimpleFileOptions::default();
            z.start_file("_chat.txt", o).unwrap();
            std::io::Write::write_all(
                &mut z,
                "[15/3/26, 9:15:02 p. m.] Lucía: \u{200E}<attached: 00000003-PHOTO.jpg>\n[15/3/26, 9:16:02 p. m.] Lucía: \u{200E}<attached: 00000004-PHOTO.jpg>\n".as_bytes(),
            )
            .unwrap();
            z.start_file("00000003-PHOTO.jpg", o).unwrap();
            std::io::Write::write_all(&mut z, b"\xFF\xD8\xFF").unwrap();
            z.finish().unwrap();
        }
        assert!(zip_has_chat(&zp));
        let e = ExportInput {
            kind: SourceKind::Whatsapp,
            primary: zp.clone(),
            root: d.path().to_path_buf(),
            members: vec![zp.clone()],
        };
        let p = parse(&e, &opts("UTC", None)).unwrap();
        assert_eq!(p.conversations[0].display, "Lucía Gómez");
        assert_eq!(p.events[0].source.file, "_chat.txt");
        assert_eq!(
            p.events[0].attachments[0].data,
            AttachmentData::ZipMember {
                zip: zp,
                name: "00000003-PHOTO.jpg".into()
            }
        );
        assert!(matches!(
            p.events[1].attachments[0].data,
            AttachmentData::Missing { .. }
        ));
        assert_eq!(
            p.events[0]
                .timestamp
                .unwrap()
                .format("%H:%M:%S")
                .to_string(),
            "21:15:02"
        );
    }

    #[test]
    fn export_limit() {
        let mut s = String::new();
        for i in 0..39_000 {
            s.push_str(&format!(
                "13/03/2026, {:02}:{:02} - A: m\n",
                (i / 60) % 24,
                i % 60
            ));
        }
        let p = run("c.txt", &s, &opts("UTC", None)).unwrap();
        assert!(
            p.findings
                .iter()
                .any(|f| f.code == "POSSIBLE_EXPORT_LIMIT" && f.params["count"] == "39000")
        );
    }
}
