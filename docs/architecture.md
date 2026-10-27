# Architecture

One engine, two front-ends. The engine (`src/core`, crate `rv_rsmf_core`) does all the work and never prints; the
CLI and the GUI only turn user input into a `RunConfig` and render the `Event`s the engine emits. Both interfaces
therefore produce identical audit logs, RSMF files and reports.

```mermaid
flowchart TB
    subgraph Front-ends
        CLI["rv-rsmf (src/cli)<br/>clap · indicatif"]
        GUI["rv-rsmf-gui (src/gui)<br/>egui; state.rs = testable rules"]
    end
    subgraph Core["rv_rsmf_core (src/core)"]
        RUN["run.rs<br/>pipeline"]
        IN["input.rs"]
        H["hash.rs"]
        AN["analyzer.rs<br/>type check"]
        CA["case.rs · intake.rs<br/>case folder, rv-hashinv"]
        CV["convert.rs<br/>conversion stage, trace"]
        SRC["sources/<br/>whatsapp · telegram · teams · csvmap"]
        MD["model.rs<br/>neutral chat model"]
        RS["rsmf/<br/>slice · write · read · validate · shortcodes"]
        AU["audit.rs · verify.rs · trust.rs"]
        REP["report/<br/>JSON · HTML · PDF"]
    end
    CLI -- "RunConfig" --> RUN
    GUI -- "RunConfig (+ UI actions)" --> RUN
    RUN -- "Event stream" --> CLI
    RUN -- "Event stream (channel)" --> GUI
    RUN --> IN & H & AN & CA & CV & REP & AU
    CV --> SRC --> MD
    CV --> RS
    MD --> RS
```

## Modules

| Module | Responsibility |
| --- | --- |
| `input.rs` | Files, folders, list files → sorted, de-duplicated, absolute paths. Symlinks not followed. |
| `hash.rs` | One read loop (1 MiB chunks) feeding 1–2 digests; known-answer self-test. |
| `analyzer.rs` | Magic-byte type check of every input file (`EMPTY_FILE`, `EXTENSION_MISMATCH`, `EXECUTABLE_CONTENT`); the `Finding` type. |
| `model.rs` | The neutral chat model (participants, conversations, events, attachments, reactions, source locations) every parser produces. Frozen: fields are only added with a default. |
| `sources/mod.rs` | Detection of each file's export type, grouping files into exports (`plan_exports`), parser dispatch with panic containment, `ParseOptions`, the examiner-decision errors (`NeedsTimezone`, `NeedsDateOrder`, `NeedsMapping`). |
| `sources/whatsapp.rs`, `telegram.rs`, `teams.rs`, `csvmap.rs` | One parser per source → `ParsedExport` ([theory.md B5](theory.md#b5-how-each-source-is-read)). |
| `rsmf/mod.rs` | Slicing per conversation per day, manifest building, deterministic zip and EML writing, the RSMF reader, schema + strict validation, read-back, `local_to_utc`. |
| `rsmf/shortcodes.rs` | Emoji → Relativity shortcode table. |
| `convert.rs` | The conversion stage: plan, parse, slice, write, validate, trace (`trace.csv`, `trace.json`, `conversions.json`), pipeline-level findings, `preflight` for examiner decisions. |
| `run.rs` | Orchestration: case link, hashing, registry verification, conversion, duplicates, `hashes.csv`, reports, timestamp, cancellation. |
| `case.rs`, `intake.rs` | Shared case folder (`rakvor.case/1`); one-step intake by running rv-hashinv. |
| `audit.rs`, `verify.rs` | Append-only hash-chained `audit.jsonl` + `audit.log`; log verification and truncation anchors. |
| `trust.rs` | RFC 3161 client; CMS signing of PDF reports. |
| `report/` | View models → minijinja HTML, built-in PDF writer, schema-versioned JSON. |
| `events.rs`, `env.rs`, `i18n.rs`, `time.rs` | Typed events and ETA; `environment.json`; EN/ES/FR wording; UTC formatting. |

## One run, step by step

```mermaid
sequenceDiagram
    autonumber
    participant UI as CLI / GUI
    participant R as run.rs
    participant V as convert.rs
    participant P as sources/*
    participant W as rsmf/
    participant A as audit.jsonl
    UI->>V: preflight(files, options) — CLI: exit 2 if a decision is missing
    UI->>R: run(config, sink, cancel)
    R->>A: run_started, case_linked, intake_delegated / ui_*, environment_recorded, inputs_resolved
    loop each input file
        R->>A: item_started, item_hashed, (registry check), item_analyzed
        R-->>UI: Progress{elapsed_ms, eta_ms}, ItemFinished
    end
    R->>V: convert_items(results, options)
    V->>A: conversion_started
    loop each export
        V->>P: parse(export, ParseOptions) → ParsedExport
        V->>A: export_planned, export_parsed / export_failed
        loop each conversation × day
            V->>W: build manifest, zip, EML → write .rsmf
            V->>A: rsmf_written, attachment_packed
            V->>W: read back + validate
            V->>A: rsmf_validated
            V-->>UI: RsmfWritten{valid}
        end
        V-->>UI: ExportFinished
    end
    V->>A: trace_written ×3
    R->>A: conversion_finished, duplicate_group, hash_register_written, item_reports_written ×N
    R->>A: timestamp_obtained / pending, statistics_report_written, run_finished
    R-->>UI: RunFinished
```

## Data flow of one export

```mermaid
flowchart LR
    F["files of the export<br/>(main file + members)"] --> D["detect + plan_exports"]
    D --> PA["parser<br/>(ParseOptions: zone, date order,<br/>mapping, custodian)"]
    PA --> M["ParsedExport<br/>participants · conversations ·<br/>events (UTC, source loc) · facts · findings"]
    M --> MF["model_findings<br/>(time order, duplicates,<br/>zone applied, deleted msgs)"]
    M --> SL["slice(tz)<br/>conversation × day ≤ 10,000"]
    SL --> BS["build_slice → manifest + files"]
    BS --> ZP["zip_bytes (sorted, fixed time)"] --> EM["eml_bytes (headers, body, base64)"]
    EM --> OUT[".rsmf"] --> RT["validate_file + roundtrip"]
    SL --> TR["trace rows"]
```

The parsers never write anything; only `convert.rs` writes (into the run's `rsmf/` folder) and only through
`rsmf::write_slice` and `report::write_hashed`, which hash what they write.

## Run states (as the GUI sees them)

```mermaid
stateDiagram-v2
    [*] --> Idle
    Idle --> Preview: Preview (parse only, nothing written)
    Preview --> Idle: decisions made
    Idle --> Running: Start (inputs ≥ 1, algorithms 1–2, decisions complete)
    Running --> Finished: RunFinished
    Running --> Cancelled: Cancel → partial reports
    Running --> Failed: error (e.g. output not writable)
    Finished --> Running: Start again (new run ID)
    Cancelled --> Running
    Failed --> Running
```

## Concurrency and cancellation

- The GUI runs the engine on a worker thread and receives events through a channel, so the window stays responsive.
- Cancellation is an `AtomicBool` checked after every 1 MiB chunk while hashing, and before every export and every
  RSMF file while converting. The engine still writes the trace files, `hashes.csv`, partial reports and
  `run_cancelled`.
- Files and exports are processed sequentially on purpose: the order of audit entries equals the order of work.
- A panic inside a parser is caught and reported as `PARSER_FAILED` for that export; the Teams reader also contains
  a panic per `.msg` file.

## Why Rust

See [ADR 0001](adr/0001-language.md).
