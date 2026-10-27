//! End-to-end checks of the Teams (.msg) and CSV parsers on the synthetic samples: every
//! examples/sample/expected/*.json whose source is teams-msg or csv is planned, parsed, sliced, written,
//! validated, read back and written again (byte-identical), and its counts and findings compared.

#[path = "support/make_msg.rs"]
mod make_msg;
#[path = "support/samples.rs"]
mod samples;

use rv_rsmf_core::model::SourceKind;
use rv_rsmf_core::rsmf::{self, WriteOptions};
use rv_rsmf_core::sources::{self, CsvProfile, ExportInput, ParseError, ParseOptions};
use serde_json::Value;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

fn sample_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/sample")
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .map(|e| e.path().to_path_buf())
        .collect()
}

fn write_all(
    p: &rv_rsmf_core::model::ParsedExport,
    tz: chrono_tz::Tz,
    kind: SourceKind,
    dir: &Path,
) -> Vec<(rsmf::WrittenSlice, Vec<u8>)> {
    let o = WriteOptions {
        tz,
        custodian: None,
        platform: rsmf::platform_for(kind, None),
        generator: "rv-rsmf test".into(),
        collection_seed: "TEST-CASE".into(),
    };
    rsmf::slice(p, tz)
        .iter()
        .map(|s| rsmf::write_slice(p, s, &o, dir).expect("write slice"))
        .collect()
}

#[test]
fn committed_samples_match_the_generator() {
    let ev = sample_dir().join("evidence");
    for (rel, bytes) in samples::files() {
        let on_disk = std::fs::read(ev.join(&rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        assert!(
            on_disk == bytes,
            "{rel} differs from the generator; run `cargo test -p rv-rsmf-core --test make_samples -- --ignored`"
        );
    }
}

#[test]
fn samples_convert_to_valid_rsmf_matching_expected() {
    let mut checked = 0;
    let mut expected: Vec<PathBuf> = files_under(&sample_dir().join("expected"))
        .into_iter()
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    expected.sort();
    for exp_path in expected {
        let exp: Value = serde_json::from_slice(&std::fs::read(&exp_path).unwrap()).unwrap();
        let kind = SourceKind::parse(exp["source"].as_str().unwrap()).unwrap();
        if !matches!(kind, SourceKind::TeamsMsg | SourceKind::Csv) {
            continue;
        }
        let name = exp_path.file_name().unwrap().to_string_lossy().to_string();
        let primary = sample_dir().join(exp["path"].as_str().unwrap());
        let o = &exp["options"];
        let opts = ParseOptions {
            timezone: o["timezone"].as_str().map(|t| t.parse().unwrap()),
            csv_profile: CsvProfile::parse(o["csv_profile"].as_str().unwrap_or("generic")).unwrap(),
            csv_map: o["csv_map"].as_str().map(|m| sample_dir().join(m)),
            ..Default::default()
        };
        let (exports, _) = sources::plan_exports(&files_under(primary.parent().unwrap()), None);
        let export = exports
            .into_iter()
            .find(|e| e.primary == primary)
            .unwrap_or_else(|| panic!("{name}: no export with primary {}", primary.display()));
        assert_eq!(export.kind, kind, "{name}");
        let p = sources::parse(&export, &opts).unwrap_or_else(|e| panic!("{name}: {e}"));

        let tz = opts.timezone.unwrap_or(chrono_tz::UTC);
        let d1 = tempfile::tempdir().unwrap();
        let d2 = tempfile::tempdir().unwrap();
        let w1 = write_all(&p, tz, kind, d1.path());
        let w2 = write_all(&p, tz, kind, d2.path());
        assert!(!w1.is_empty(), "{name}: nothing written");
        let (mut packed, mut missing) = (0, 0);
        for ((ws, b1), (_, b2)) in w1.iter().zip(&w2) {
            let (v, _) = rsmf::validate_file(&ws.path).unwrap();
            assert!(
                v.ok(),
                "{name} {}: {:?} {:?}",
                ws.path.display(),
                v.schema_errors,
                v.strict_errors
            );
            let rt = rsmf::roundtrip(ws).unwrap();
            assert!(rt.is_empty(), "{name}: {rt:?}");
            assert!(b1 == b2, "{name}: writing twice gave different bytes");
            packed += ws.attachments.len();
            missing += ws.missing_attachments;
        }
        let codes: Vec<String> = p
            .findings
            .iter()
            .map(|f| f.code.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let want: Vec<String> = exp["findings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap().to_string())
            .collect();
        assert_eq!(codes, want, "{name}: findings {:#?}", p.findings);
        assert_eq!(
            p.conversations.len() as u64,
            exp["conversations"].as_u64().unwrap(),
            "{name}"
        );
        assert_eq!(
            p.events.len() as u64,
            exp["events"].as_u64().unwrap(),
            "{name}"
        );
        assert_eq!(
            packed as u64,
            exp["attachments_packed"].as_u64().unwrap(),
            "{name}"
        );
        assert_eq!(
            missing as u64,
            exp["attachments_missing"].as_u64().unwrap(),
            "{name}"
        );
        checked += 1;
    }
    assert!(checked >= 3, "only {checked} Teams/CSV samples checked");
}

#[test]
fn teams_sample_details() {
    let dir = sample_dir().join("evidence/teams-msg");
    let (exports, _) = sources::plan_exports(&files_under(&dir), None);
    assert_eq!(exports.len(), 1);
    assert_eq!(exports[0].members.len(), 4);
    // No time zone needed: FILETIMEs are UTC.
    let p = sources::parse(&exports[0], &ParseOptions::default()).unwrap();
    assert_eq!(p.participants.len(), 3);
    let c = &p.conversations[0];
    assert_eq!(c.display, "Proyecto Faro / General");
    assert_eq!(c.kind, rv_rsmf_core::model::ConversationType::Channel);
    let e = &p.events;
    assert_eq!(
        e[0].timestamp.unwrap().to_rfc3339(),
        "2026-03-02T09:15:00+00:00"
    );
    assert_eq!(e[0].source.file, "0001.msg");
    assert_eq!(
        e[0].source.native_id.as_deref(),
        Some("<msg-0001@teams.example.com>")
    );
    assert_eq!(e[1].parent.as_deref(), Some(e[0].id.as_str()));
    assert_eq!(
        e[1].body,
        "Sí, adjunto la agenda.\nHora propuesta: 15:00 & sala 2"
    );
    assert_eq!(e[1].custom["body_source"], "PR_HTML (tags removed)");
    assert_eq!(e[1].attachments[0].display, "agenda-piloto.txt");
    assert_eq!(e[2].kind, rv_rsmf_core::model::EventType::History);
    assert!(e[2].source_time.contains("PR_MESSAGE_DELIVERY_TIME"));
    let u = p
        .findings
        .iter()
        .find(|f| f.code == "UNPARSED_LINES")
        .unwrap();
    assert!(u.params["examples"].contains("0004.msg: message class IPM.Note"));
}

#[test]
fn corrupt_msg_files_are_reported_not_fatal() {
    let d = tempfile::tempdir().unwrap();
    let (_, good) = samples::teams_messages().into_iter().next().unwrap();
    std::fs::write(d.path().join("a.msg"), make_msg::msg_bytes(&good)).unwrap();
    std::fs::write(
        d.path().join("b.msg"),
        b"\xD0\xCF\x11\xE0\xA1\xB1\x1A\xE1broken",
    )
    .unwrap();
    let export = |members: Vec<PathBuf>| ExportInput {
        kind: SourceKind::TeamsMsg,
        primary: members[0].clone(),
        root: d.path().to_path_buf(),
        members,
    };
    let p = sources::parse(
        &export(vec![d.path().join("a.msg"), d.path().join("b.msg")]),
        &ParseOptions::default(),
    )
    .unwrap();
    assert_eq!(p.events.len(), 1);
    let u = p
        .findings
        .iter()
        .find(|f| f.code == "UNPARSED_LINES")
        .unwrap();
    assert!(
        u.params["examples"].starts_with("b.msg:"),
        "{}",
        u.params["examples"]
    );
    // Every file corrupt → Invalid (PARSER_FAILED upstream), never an empty output.
    let r = sources::parse(
        &export(vec![d.path().join("b.msg")]),
        &ParseOptions::default(),
    );
    assert!(matches!(r, Err(ParseError::Invalid(_))), "{r:?}");
}

#[test]
fn csv_without_timezone_or_mapping_stops() {
    let dir = sample_dir().join("evidence/csv");
    let (exports, _) = sources::plan_exports(&files_under(&dir), None);
    let xry = exports
        .iter()
        .find(|e| e.primary.ends_with("xry-like.csv"))
        .unwrap();
    let generic = exports
        .iter()
        .find(|e| e.primary.ends_with("generic.csv"))
        .unwrap();
    let draft = ParseOptions {
        csv_profile: CsvProfile::XryDraft,
        ..Default::default()
    };
    assert_eq!(sources::parse(xry, &draft), Err(ParseError::NeedsTimezone));
    assert_eq!(
        sources::parse(generic, &ParseOptions::default()),
        Err(ParseError::NeedsTimezone)
    );
    // The xry-like header has none of the generic profile's column names.
    let generic_profile = ParseOptions {
        timezone: Some(chrono_tz::America::Bogota),
        ..Default::default()
    };
    assert!(
        matches!(sources::parse(xry, &generic_profile), Err(ParseError::NeedsMapping(m)) if m.contains("header has: Index, Time"))
    );
    // The sibling generic.map.toml is used and recorded.
    let p = sources::parse(
        generic,
        &ParseOptions {
            timezone: Some(chrono_tz::Europe::Madrid),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(p.facts["mapping"], "generic.map.toml");
    assert_eq!(p.facts["mapping_source"], "sibling");
    assert_eq!(p.participants.len(), 3);
    assert_eq!(
        p.events[1].timestamp.unwrap().to_rfc3339(),
        "2026-10-25T00:30:00+00:00"
    );
}
