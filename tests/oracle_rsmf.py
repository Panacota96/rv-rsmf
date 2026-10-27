#!/usr/bin/env python3
"""Independent check of rv-rsmf output (test oracle; not shipped).

For every .rsmf under a run's rsmf folder:
  1. Python's own `email` package parses the message (not rv-rsmf's reader);
  2. it has X-RSMF-Version, exactly one rsmf.zip attachment and a text/plain body;
  3. rsmf_manifest.json validates against Relativity's official RSMF 2.0 schema, given as a separate file
     (CI downloads it from relativitydev/rsmf-validator-samples at a pinned commit, so the check does not
     trust the copy bundled in this repository) — with format checking on;
  4. X-RSMF-EventCount / X-RSMF-AttachmentCount agree with the manifest, and every attachment id is in the zip;
  5. the SHA-256 of the file equals the one rv-rsmf logged (rsmf_written in audit.jsonl);
  6. trace.csv has one row per event and points only at files that exist.

Usage: python tests/oracle_rsmf.py <run-rsmf-dir> <audit.jsonl> <rsmf_schema_2_0_0.json>
Exit 0 = every check passed.
"""

import csv
import email
import email.policy
import hashlib
import io
import json
import sys
import zipfile
from pathlib import Path

import jsonschema


def main() -> int:
    rsmf_dir, audit, schema_path = (Path(a) for a in sys.argv[1:4])
    schema = json.loads(schema_path.read_text(encoding="utf-8"))
    validator = jsonschema.Draft7Validator(schema, format_checker=jsonschema.FormatChecker())
    logged = {}
    for line in audit.read_text(encoding="utf-8").splitlines():
        e = json.loads(line)
        if e.get("action") == "rsmf_written":
            logged[e["item"].replace("\\", "/")] = e["details"]["sha256"]
    problems = []
    files = sorted(rsmf_dir.rglob("*.rsmf"))
    events_total = 0
    for f in files:
        rel = f.relative_to(rsmf_dir).as_posix()
        raw = f.read_bytes()
        if hashlib.sha256(raw).hexdigest() != logged.get(rel):
            problems.append(f"{rel}: SHA-256 differs from the audit log ({logged.get(rel)})")
        msg = email.message_from_bytes(raw, policy=email.policy.default)
        if msg["X-RSMF-Version"] != "2.0.0":
            problems.append(f"{rel}: X-RSMF-Version is {msg['X-RSMF-Version']!r}")
        zips = [p for p in msg.iter_attachments() if p.get_filename() == "rsmf.zip"]
        if len(zips) != 1:
            problems.append(f"{rel}: {len(zips)} rsmf.zip attachments")
            continue
        body = msg.get_body(preferencelist=("plain",))
        if body is None:
            problems.append(f"{rel}: no text/plain body")
        z = zipfile.ZipFile(io.BytesIO(zips[0].get_payload(decode=True)))
        names = set(z.namelist())
        manifest = json.loads(z.read("rsmf_manifest.json"))
        for err in validator.iter_errors(manifest):
            problems.append(f"{rel}: schema: {err.message} at {list(err.absolute_path)}")
        n_events = len(manifest["events"])
        n_att = sum(len(e.get("attachments", [])) for e in manifest["events"])
        events_total += n_events
        if str(n_events) != str(msg["X-RSMF-EventCount"]):
            problems.append(f"{rel}: X-RSMF-EventCount {msg['X-RSMF-EventCount']} != {n_events}")
        if str(n_att) != str(msg["X-RSMF-AttachmentCount"]):
            problems.append(f"{rel}: X-RSMF-AttachmentCount {msg['X-RSMF-AttachmentCount']} != {n_att}")
        for e in manifest["events"]:
            for a in e.get("attachments", []):
                if a["id"] not in names:
                    problems.append(f"{rel}: attachment {a['id']} not in rsmf.zip")
    with open(rsmf_dir / "trace.csv", encoding="utf-8", newline="") as fh:
        rows = list(csv.DictReader(fh))
    if len(rows) != events_total:
        problems.append(f"trace.csv has {len(rows)} rows, the RSMF files hold {events_total} events")
    for r in rows:
        if not (rsmf_dir / r["rsmf_file"]).is_file():
            problems.append(f"trace.csv points at missing file {r['rsmf_file']}")
            break
    for p in problems:
        print("FAIL", p)
    print(f"{len(files)} RSMF file(s), {events_total} event(s), {len(rows)} trace row(s): "
          f"{'OK' if not problems else str(len(problems)) + ' problem(s)'}")
    return 0 if files and not problems else 1


if __name__ == "__main__":
    sys.exit(main())
