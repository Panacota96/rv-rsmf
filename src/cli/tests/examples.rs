//! The worked examples in each command's `--help` (EXAMPLES:) really work: they run, in a temporary folder,
//! against copies of the repository's sample chat exports. The sample paths of the examples (D:\evidence,
//! E:\CASE-014, the example run id, chat.txt, ...) are mapped to that folder. Every subcommand that `docs-cli`
//! documents, plus `docs-cli` itself, is run; the exceptions are explicit: the subcommands in PARSE_ONLY and the
//! example lines that use an option in NEEDS are only parsed (by the unit test of src/main.rs, which also fails
//! when a hidden subcommand is added that this list does not see), because they need the network or another
//! program.

use assert_cmd::Command;
use rv_rsmf_core::convert::ConvertOptions;
use rv_rsmf_core::sources::{DateOrder, ParseOptions};
use rv_rsmf_core::{CASE_ROLE, CaseRole, RunConfig, run};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

const TOOL: &str = "rv-rsmf";
/// Subcommands whose examples are not run here, with the reason. They are parsed by the unit test in src/main.rs.
const PARSE_ONLY: &[(&str, &str)] = &[("timestamp", "needs an RFC 3161 TSA on the network")];
/// Example lines that are only parsed when they use one of these options, with the reason.
const NEEDS: &[(&str, &str)] = &[(
    "--intake",
    "runs rv-hashinv, a separate program that the test machine may not have",
)];
/// The run id the examples use; replaced by a real run id of the same folder.
const EXAMPLE_RUN: &str = "20261019T101500Z-a1b2c3";
/// `convert` exits 3 when a high-severity finding is raised.
const EXIT_FINDINGS: i32 = 3;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn bin(cwd: &Path) -> Command {
    let mut c = Command::cargo_bin(TOOL).unwrap();
    c.current_dir(cwd)
        .env("RV_NO_TIMESTAMP", "1") // offline: the examples must not depend on a TSA
        .env("RV_NO_UPDATE_CHECK", "1"); // update-check answers "turned off" (exit 0) without the network
    c
}

fn help(sub: &str) -> String {
    let mut args: Vec<&str> = sub.split(' ').collect();
    args.push("--help");
    let out = Command::cargo_bin(TOOL)
        .unwrap()
        .args(args)
        .output()
        .unwrap();
    String::from_utf8(out.stdout).unwrap()
}

/// Command lines under EXAMPLES: (same rule as the unit test in src/main.rs).
fn examples(help: &str) -> Vec<String> {
    let Some(section) = help.split("EXAMPLES:\n").nth(1) else {
        return vec![];
    };
    // Commands are indented by 4. clap wraps a long one onto further lines with the same indent: those lines
    // continue the command.
    let prefix = format!("{} ", TOOL);
    let mut out: Vec<String> = vec![];
    let mut in_command = false;
    for l in section
        .lines()
        .take_while(|l| l.is_empty() || l.starts_with(' '))
    {
        let t = l.trim();
        if t.starts_with(&prefix) {
            out.push(t.to_string());
            in_command = true;
        } else if in_command && l.starts_with("    ") && !t.is_empty() {
            if let Some(last) = out.last_mut() {
                last.push(' ');
                last.push_str(t);
            }
        } else {
            in_command = false;
        }
    }
    out
}

fn split_args(line: &str) -> Vec<String> {
    let (mut args, mut cur, mut quoted, mut any) = (vec![], String::new(), false, false);
    for c in line.chars() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            c if c.is_whitespace() && !quoted => {
                if any {
                    args.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            c => {
                cur.push(c);
                any = true;
            }
        }
    }
    if any {
        args.push(cur);
    }
    if let Some(i) = args.iter().position(|a| a == ">") {
        args.truncate(i);
    }
    args
}

/// Copies a folder with its subfolders (the sample exports keep their media in subfolders).
fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &to.join(e.file_name()));
        } else {
            std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
        }
    }
}

/// The first run folder name under `dir` (a logs/, rsmf/ or runs/<tool>/ folder).
fn a_run(dir: &Path) -> String {
    let mut runs: Vec<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {}", dir.display(), e))
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    runs.sort();
    runs.into_iter()
        .next()
        .unwrap_or_else(|| panic!("no run in {}", dir.display()))
}

/// The first .rsmf file below `dir` (sorted walk).
fn an_rsmf(dir: &Path) -> Option<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .map(|e| e.unwrap().path())
        .collect();
    entries.sort();
    entries.into_iter().find_map(|p| {
        if p.is_dir() {
            an_rsmf(&p)
        } else {
            p.extension().is_some_and(|x| x == "rsmf").then_some(p)
        }
    })
}

/// Maps the sample paths of an example argument to the temporary folder.
fn localize(arg: &str, t: &Path) -> String {
    let unix = arg.replace('\\', "/");
    if let Some(rest) = unix.strip_prefix("D:/evidence") {
        return format!("{}{}", t.join("D-evidence").display(), rest);
    }
    if let Some(rest) = unix.strip_prefix("E:/CASE-014") {
        let rest = if rest.contains(EXAMPLE_RUN) {
            let run = a_run(&t.join("CASE-014/runs").join(TOOL));
            rest.replace(EXAMPLE_RUN, &run)
        } else {
            rest.to_string()
        };
        return format!("{}{}", t.join("CASE-014").display(), rest);
    }
    for kind in ["logs", "rsmf"] {
        let prefix = format!("rakvor-out/{}/", kind);
        if unix.starts_with(&prefix) && unix.contains(EXAMPLE_RUN) {
            return unix.replace(EXAMPLE_RUN, &a_run(&t.join("rakvor-out").join(kind)));
        }
    }
    if unix == "other-vendor.rsmf" {
        // Any RSMF file will do: one the conversion examples above wrote.
        let f = an_rsmf(&t.join("rakvor-out/rsmf")).expect("an .rsmf file written by convert");
        std::fs::copy(f, t.join("other-vendor.rsmf")).unwrap();
        return arg.to_string();
    }
    if unix == "digicert-tsa-roots.pem" {
        // Any CA certificate will do: the offline runs have no timestamp token to chain to it.
        return repo()
            .join("packaging/signing/rakvor-code-signing-root.cer")
            .display()
            .to_string();
    }
    arg.to_string()
}

#[test]
fn help_examples_run_against_the_sample_exports() {
    let tmp = tempfile::tempdir().unwrap();
    let t = tmp.path();
    let sample = repo().join("examples/sample/evidence");
    copy_dir(&sample, &t.join("D-evidence"));
    // A WhatsApp export saved under another name (the --source example).
    std::fs::copy(sample.join("whatsapp-dst.txt"), t.join("chat.txt")).unwrap();
    if CASE_ROLE != CaseRole::Intake {
        // An analysis tool does not register evidence: register the sample in the example case first, through
        // the core with the intake role (what rv-hashinv does), so `--evidence E0001,E0002` exists.
        let cfg = RunConfig {
            inputs: vec![t.join("D-evidence")],
            recursive: true,
            actor: "seed".into(),
            case_id: Some("CASE-014".into()),
            jurisdiction: "fr".into(),
            case_dir: Some(t.join("CASE-014")),
            case_role: CaseRole::Intake,
            convert: ConvertOptions {
                parse: ParseOptions {
                    timezone: Some("Europe/Paris".parse().unwrap()),
                    date_order: Some(DateOrder::Dmy),
                    ..ParseOptions::default()
                },
                ..ConvertOptions::default()
            },
            ..Default::default()
        };
        run(&cfg, &mut |_| {}, &AtomicBool::new(false)).unwrap();
    }

    // Every documented subcommand (docs-cli lists them; convert comes first, so its runs exist before the
    // validate and verify examples), then docs-cli itself (hidden from the list, its example only prints).
    let (top, nested) = (format!("## {} ", TOOL), format!("### {} ", TOOL));
    let docs = bin(t).arg("docs-cli").output().unwrap();
    let mut order: Vec<String> = String::from_utf8_lossy(&docs.stdout)
        .lines()
        .filter_map(|l| {
            l.strip_prefix(&top)
                .or_else(|| l.strip_prefix(&nested))
                .map(str::to_string)
        })
        .collect();
    assert_eq!(
        order.first().map(String::as_str),
        Some("convert"),
        "{:?}",
        order
    );
    order.push("docs-cli".into());
    let (mut ran, mut parsed_only) = (0, 0);
    for sub in &order {
        if PARSE_ONLY.iter().any(|(p, _)| p == sub) {
            continue;
        }
        let words: Vec<&str> = sub.split(' ').collect();
        let lines = examples(&help(sub));
        assert!(!lines.is_empty(), "`{} {}` has no EXAMPLES", TOOL, sub);
        for line in lines {
            let args = split_args(&line);
            if args.get(1..=words.len()).is_none_or(|w| *w != words[..]) {
                // convert's help also shows how to preview and verify; those commands run with their own help.
                continue;
            }
            if NEEDS.iter().any(|(o, _)| args.iter().any(|a| a == o)) {
                parsed_only += 1;
                continue;
            }
            let local: Vec<String> = args[1..].iter().map(|a| localize(a, t)).collect();
            let out = bin(t).args(&local).output().unwrap();
            let ok = match out.status.code() {
                Some(0) => true,
                Some(EXIT_FINDINGS) => sub == "convert",
                _ => false,
            };
            assert!(
                ok,
                "example `{}` (run as {:?}) failed with {:?}:\n{}{}",
                line,
                local,
                out.status.code(),
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            ran += 1;
        }
    }
    assert!(ran >= 25, "{} examples ran", ran);
    assert_eq!(parsed_only, 1, "examples only parsed (NEEDS)");
    // The case examples converted the registered sample and recorded their runs in the case log.
    let case_log = std::fs::read_to_string(t.join("CASE-014/case-log.jsonl")).unwrap();
    assert!(case_log.contains("\"rv-rsmf\""), "{}", case_log);
    // Every timestamp example names a log folder the examples above created (it is only parsed: no TSA).
    for line in examples(&help("timestamp")) {
        let args = split_args(&line);
        assert!(
            t.join(localize(&args[2], t)).join("audit.jsonl").is_file(),
            "{}",
            line
        );
    }
}
