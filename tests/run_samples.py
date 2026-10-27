#!/usr/bin/env python3
"""Converts every synthetic sample with the real CLI and checks the result end to end.

For each examples/sample/expected/<name>.json:
  - runs `rv-rsmf convert` on the sample with the options in the file;
  - checks conversations, events, packed/missing attachments and that every expected finding code is
    reported for the export (pipeline-level info findings may be added on top);
  - runs `rv-rsmf verify-log` on the run and tests/oracle_rsmf.py (independent schema + MIME check);
  - converts a second time and checks the .rsmf files are byte-identical (reproducibility).
Also checks that the ambiguous WhatsApp sample without --date-order stops with exit code 2.

Usage: python tests/run_samples.py <rv-rsmf binary> <official rsmf_schema_2_0_0.json> [work dir]
"""

import hashlib
import json
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SAMPLES = ROOT / "examples" / "sample"


def sample_path(rel: str) -> Path:
    for base in (SAMPLES, SAMPLES / "evidence"):
        if (base / rel).exists():
            return base / rel
    raise SystemExit(f"sample not found: {rel}")


def args_for(exp: dict) -> list:
    o = exp.get("options", {})
    a = []
    if o.get("timezone"):
        a += ["--timezone", o["timezone"]]
    if o.get("date_order"):
        a += ["--date-order", o["date_order"]]
    if o.get("csv_profile") and o["csv_profile"] != "generic":
        a += ["--csv-profile", o["csv_profile"]]
    if o.get("csv_map"):
        a += ["--csv-map", str(sample_path(o["csv_map"]))]
    return a


def inputs_for(exp: dict) -> list:
    p = sample_path(exp["path"])
    # Exports with their own folder (media, Teams .msg files, mapping) are given as that folder.
    if p.parent != SAMPLES / "evidence" and exp["source"] != "csv":
        return ["-i", str(p.parent), "-r"]
    return ["-i", str(p)]


def run(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", **kw)


def digests(rsmf_dir: Path) -> dict:
    return {p.relative_to(rsmf_dir).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(rsmf_dir.rglob("*.rsmf"))}


def convert(binary, exp, out: Path):
    if out.exists():
        shutil.rmtree(out)
    r = run([binary, "convert", *inputs_for(exp), *args_for(exp), "--out", str(out),
             "--progress", "none", "--no-timestamp", "--lang", "en", "--actor", "sample-check"])
    if r.returncode not in (0, 3):
        raise SystemExit(f"{exp['path']}: convert exited {r.returncode}\n{r.stdout}\n{r.stderr}")
    run_id = sorted(p.name for p in (out / "rsmf").iterdir())[-1]
    return out / "rsmf" / run_id, out / "logs" / run_id, out / "reports" / run_id


def main() -> int:
    binary, schema = sys.argv[1], sys.argv[2]
    work = Path(sys.argv[3] if len(sys.argv) > 3 else "sample-check")
    failures = []
    for f in sorted((SAMPLES / "expected").glob("*.json")):
        exp = json.loads(f.read_text(encoding="utf-8"))
        name = f.stem
        rsmf_dir, log_dir, report_dir = convert(binary, exp, work / name / "a")
        conv = json.loads((rsmf_dir / "conversions.json").read_text(encoding="utf-8"))
        ex = [e for e in conv["exports"] if e["source"] == exp["source"]]
        if len(ex) != 1:
            failures.append(f"{name}: expected one {exp['source']} export, got {len(ex)}")
            continue
        e = ex[0]
        files = e["files"]
        got = {"conversations": e["conversations"], "events": e["events"],
               "attachments_packed": sum(x["attachments"] for x in files),
               "attachments_missing": sum(x["missing_attachments"] for x in files)}
        for k, v in got.items():
            if v != exp[k]:
                failures.append(f"{name}: {k} = {v}, expected {exp[k]}")
        if not all(x["valid"] for x in files):
            failures.append(f"{name}: invalid RSMF file(s)")
        stats = json.loads((report_dir / "statistics.json").read_text(encoding="utf-8"))
        item = next(i for i in stats["items"] if i["index"] == e["item_index"])
        missing = sorted(set(exp["findings"]) - set(item["findings"]))
        if missing:
            failures.append(f"{name}: findings not reported: {missing} (got {item['findings']})")
        r = run([binary, "verify-log", str(log_dir)])
        if r.returncode != 0:
            failures.append(f"{name}: verify-log exit {r.returncode}: {r.stdout.strip()[-300:]}")
        r = run([sys.executable, str(ROOT / "tests" / "oracle_rsmf.py"), str(rsmf_dir), str(log_dir / "audit.jsonl"), schema])
        print(f"{name}: {r.stdout.strip().splitlines()[-1] if r.stdout.strip() else r.stderr.strip()}")
        if r.returncode != 0:
            failures.append(f"{name}: oracle failed\n{r.stdout}")
        rsmf_b, _, _ = convert(binary, exp, work / name / "b")
        if digests(rsmf_dir) != digests(rsmf_b):
            failures.append(f"{name}: a second conversion did not give byte-identical .rsmf files")
    amb = sample_path("whatsapp-ambiguous.txt")
    r = run([binary, "convert", "-i", str(amb), "--timezone", "Europe/Madrid", "--out", str(work / "amb"), "--no-timestamp", "--progress", "none"])
    if r.returncode != 2 or "--date-order" not in r.stderr:
        failures.append(f"ambiguous sample without --date-order: exit {r.returncode}, stderr {r.stderr.strip()[:200]}")
    if (work / "amb").exists():
        failures.append("ambiguous sample without --date-order wrote output")
    for x in failures:
        print("FAIL", x)
    print("all samples OK" if not failures else f"{len(failures)} failure(s)")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
