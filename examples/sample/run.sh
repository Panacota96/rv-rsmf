#!/usr/bin/env sh
# Converts every synthetic sample export in one run and checks the result. Usage: examples/sample/run.sh [path-to-rv-rsmf]
# Output folder: examples/sample/out (or $RV_SAMPLE_OUT).
# Fails (non-zero exit) if the self-test, the conversion, verify-log or validate fails, or if the tool's hash
# register (logs/<RUN-ID>/hashes.csv) differs from the independently computed expected_hashes.csv.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
bin=${1:-rv-rsmf}
out=${RV_SAMPLE_OUT:-"$here/out"}
"$bin" self-test
# Times in the WhatsApp and CSV samples have no UTC offset: they are read as Paris time. The ambiguous WhatsApp
# sample (every day and month <= 12) is read as day/month/year. Expected exit code 0 (no high-severity finding).
"$bin" convert --input "$here/evidence" --recursive --timezone Europe/Paris --date-order dmy \
  --hash sha256,md5 --lang all --case DEMO-001 --actor "Sample" --out "$out" --progress none
run=$(ls "$out/logs" | tail -n 1)
"$bin" verify-log "$out/logs/$run"
"$bin" validate "$out/rsmf/$run"
# Compare the tool's hash register with expected_hashes.csv: same files (path below evidence/), sizes, SHA-256, MD5.
awk -F, '
  FNR == 1 { if (FILENAME ~ /expected_hashes\.csv$/) { for (i = 1; i <= NF; i++) e[$i] = i } else { for (i = 1; i <= NF; i++) h[$i] = i; hn = NF }; next }
  FILENAME ~ /expected_hashes\.csv$/ { want[$e["file"]] = $e["size"] "," $e["sha256"] "," $e["md5"]; n++; next }
  {
    path = $0; sub(/^[0-9]+,/, "", path); sub(/,[0-9]+,[^,]*,[^,]*,[^,]*,[^,]*$/, "", path)
    gsub(/"/, "", path); gsub(/\\/, "/", path); sub(/.*\/evidence\//, "", path)
    got[path] = $(NF - hn + h["size"]) "," $(NF - hn + h["sha256"]) "," $(NF - hn + h["md5"])
    m++
  }
  END {
    for (f in want) if (got[f] != want[f]) { print "MISMATCH " f ": tool " got[f] ", expected " want[f]; bad = 1 }
    if (m != n) { print "MISMATCH: the tool hashed " m " files, expected_hashes.csv lists " n; bad = 1 }
    if (bad) exit 3
    print "hashes.csv matches expected_hashes.csv (" n " files: size, SHA-256, MD5)"
  }' "$here/expected_hashes.csv" "$out/logs/$run/hashes.csv"
echo "RSMF files: $out/rsmf/$run"
echo "Reports:    $out/reports/$run/statistics.en.html"
