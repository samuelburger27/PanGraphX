#!/usr/bin/env bash
#
# peak_memory.sh — peak resident-set-size (RSS) comparison of PanGraphX against
# `vg` and `odgi`. hyperfine measures time only, so this companion uses GNU
# `/usr/bin/time -v` to capture max memory per conversion.
#
# Linux only (relies on GNU time's "Maximum resident set size" line). Missing
# external tools are skipped.
#
# Output: benchmarks/results/peak_memory.md (Markdown table, RSS in MiB).
#
# Usage: benchmarks/peak_memory.sh [DATASET ...]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TESTDIR="$REPO_ROOT/pangraphx-core/tests/test_files"
RESULTS="$SCRIPT_DIR/results"
PANGRAPHX="$REPO_ROOT/target/release/pangraphx-cli"
OUT="$RESULTS/peak_memory.md"

if [[ $# -gt 0 ]]; then
    DATASETS=("$@")
else
    DATASETS=(escherichia_phage_lambda mycoplasma_genitalium merged)
fi

mkdir -p "$RESULTS"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

have() { command -v "$1" >/dev/null 2>&1; }

# Pick a GNU-time binary (named 'time' on most Linux distros; 'gtime' via brew).
TIME_BIN=""
for t in /usr/bin/time gtime; do
    if "$t" -v true >/dev/null 2>&1; then TIME_BIN="$t"; break; fi
done
if [[ -z "$TIME_BIN" ]]; then
    echo "ERROR: GNU 'time' (supporting -v) not found. On Debian/Ubuntu: apt install time." >&2
    exit 1
fi

HAVE_VG=false; have vg && HAVE_VG=true || echo "WARNING: 'vg' not found — skipping." >&2
HAVE_ODGI=false; have odgi && HAVE_ODGI=true || echo "WARNING: 'odgi' not found — skipping." >&2

if [[ ! -x "$PANGRAPHX" ]]; then
    echo "ERROR: PanGraphX binary not found at $PANGRAPHX (build it first)." >&2
    exit 1
fi

# measure_rss <cmd> -> peak RSS in MiB (one decimal), or "ERR".
measure_rss() {
    local cmd="$1" tmp kb
    tmp="$(mktemp)"
    if ! "$TIME_BIN" -v sh -c "$cmd" >/dev/null 2>"$tmp"; then
        cat "$tmp" >&2; rm -f "$tmp"; echo "ERR"; return
    fi
    kb="$(grep -i 'Maximum resident set size' "$tmp" | grep -oE '[0-9]+' | tail -1)"
    rm -f "$tmp"
    if [[ -z "$kb" ]]; then echo "ERR"; else awk "BEGIN{printf \"%.1f\", $kb/1024}"; fi
}

# row <dataset> <operation> <pangraphx-cmd> <other-tool> <other-cmd>
row() {
    local ds="$1" op="$2" pxcmd="$3" tool="$4" toolcmd="$5"
    local px other
    px="$(measure_rss "$pxcmd")"
    other="$(measure_rss "$toolcmd")"
    echo "| $ds | $op | $px | $tool | $other |" >>"$OUT"
    echo "  $ds $op: pangraphx=${px} MiB, ${tool}=${other} MiB"
}

{
    echo "# Peak memory comparison (RSS, MiB)"
    echo
    echo "| Dataset | Operation | PanGraphX | Tool | Tool RSS |"
    echo "|---------|-----------|-----------|------|----------|"
} >"$OUT"

for ds in "${DATASETS[@]}"; do
    gfa="$TESTDIR/gfa/$ds.gfa"; vg="$TESTDIR/vg/$ds.vg"; og="$TESTDIR/og/$ds.og"
    echo ">>> Dataset: $ds"

    if $HAVE_VG; then
        [[ -f "$gfa" ]] && row "$ds" "GFA->VG" \
            "$PANGRAPHX convert -i $gfa -o $WORK/px.vg" \
            "vg" "vg convert --gfa-in --vg-out $gfa > $WORK/vg.vg"
        [[ -f "$vg" ]] && row "$ds" "VG->GFA" \
            "$PANGRAPHX convert -i $vg -o $WORK/px.gfa" \
            "vg" "vg convert --gfa-out $vg > $WORK/vg.gfa"
    fi
    if $HAVE_ODGI; then
        [[ -f "$gfa" ]] && row "$ds" "GFA->OG" \
            "$PANGRAPHX convert -i $gfa -o $WORK/px.og" \
            "odgi" "odgi build -g $gfa -o $WORK/odgi.og"
        [[ -f "$og" ]] && row "$ds" "OG->GFA" \
            "$PANGRAPHX convert -i $og -o $WORK/px.gfa" \
            "odgi" "odgi view -i $og -g > $WORK/odgi.gfa"
    fi
done

echo
echo ">>> Wrote $OUT"
