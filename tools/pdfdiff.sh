#!/usr/bin/env bash
# Rasterize two PDFs and report per-page pixel differences (AE = differing pixel count).
# Usage: pdfdiff.sh <a.pdf> <b.pdf> [dpi]
set -u
A="$1"; B="$2"; DPI="${3:-200}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

pdftoppm -png -r "$DPI" "$A" "$TMP/a" >/dev/null 2>&1
pdftoppm -png -r "$DPI" "$B" "$TMP/b" >/dev/null 2>&1

na=$(ls "$TMP"/a-*.png 2>/dev/null | wc -l)
nb=$(ls "$TMP"/b-*.png 2>/dev/null | wc -l)
echo "pages: A=$na B=$nb (dpi=$DPI)"
if [ "$na" != "$nb" ]; then
  echo "PAGE COUNT MISMATCH"
  exit 1
fi

total=0
worst=0
for a in "$TMP"/a-*.png; do
  page=$(basename "$a" | sed 's/a-//; s/.png//')
  b="$TMP/b-$page.png"
  dims_a=$(magick identify -format "%wx%h" "$a")
  dims_b=$(magick identify -format "%wx%h" "$b")
  if [ "$dims_a" != "$dims_b" ]; then
    echo "page $page: DIMENSION MISMATCH a=$dims_a b=$dims_b"
    worst=999999999
    continue
  fi
  npix=$(( ${dims_a%x*} * ${dims_a#*x} ))
  ae=$(compare -metric AE "$a" "$b" null: 2>&1)
  ae=${ae%% *}   # ImageMagick prints "AE (normalized)"; keep the count
  ae=${ae%%.*}
  case "$ae" in ''|*[!0-9]*) ae=0 ;; esac
  pct=$(awk "BEGIN{printf \"%.4f\", ($ae/$npix)*100}")
  echo "page $page: diff_pixels=$ae / $npix (${pct}%)  dims=$dims_a"
  total=$((total + ae))
  [ "$ae" -gt "$worst" ] && worst=$ae
done
echo "TOTAL diff pixels: $total   worst page: $worst"
