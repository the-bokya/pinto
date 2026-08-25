#!/usr/bin/env bash
# For each test case, generate pinto + Frappe-reference PDFs and pixel-diff them.
# Usage: run_matrix.sh <cases_dir> [dpi]
set -u
CASES="$1"; DPI="${2:-300}"
CHROME=/home/qwerty/pilot/benches/frappe-bench2/chromium/chrome-linux/headless_shell
PINTO=/home/qwerty/pinto/target/debug/pinto
REF=/home/qwerty/pinto/tools/reference.py
BENCH=/home/qwerty/pilot/benches/frappe-bench2
OUT="$CASES/out"; mkdir -p "$OUT"

fail=0
for html in "$CASES"/*.html; do
  name=$(basename "$html" .html)
  json="$CASES/$name.json"
  [ -f "$json" ] || json="$CASES/default.json"
  pd=""
  grep -q '"is_print_designer": *true' "$json" 2>/dev/null && pd="--print-designer"

  "$PINTO" --html "$html" --options "$json" --out "$OUT/$name.pinto.pdf" --chrome-path "$CHROME" 2>"$OUT/$name.pinto.err"
  ( cd "$BENCH" && timeout 120 env/bin/python "$REF" "$html" "$json" "$OUT/$name.ref.pdf" $pd >"$OUT/$name.ref.log" 2>&1 )

  if [ ! -s "$OUT/$name.pinto.pdf" ] || [ ! -s "$OUT/$name.ref.pdf" ]; then
    echo "CASE $name: GENERATION FAILED (pinto=$(wc -c <"$OUT/$name.pinto.pdf" 2>/dev/null) ref=$(wc -c <"$OUT/$name.ref.pdf" 2>/dev/null))"
    tail -2 "$OUT/$name.ref.log" 2>/dev/null | sed 's/^/    /'
    fail=1; continue
  fi
  res=$(bash /home/qwerty/pinto/tools/pdfdiff.sh "$OUT/$name.pinto.pdf" "$OUT/$name.ref.pdf" "$DPI" | tail -1)
  worst=$(echo "$res" | grep -oE "worst page: [0-9]+" | grep -oE "[0-9]+")
  pages=$(pdfinfo "$OUT/$name.pinto.pdf" 2>/dev/null | awk '/Pages/{print $2}')
  status="OK"
  [ "${worst:-0}" -gt 0 ] && status="DIFF"
  echo "CASE $name (${pages}pg): $status  [$res]"
  [ "$status" = "DIFF" ] && fail=1
done
exit $fail
