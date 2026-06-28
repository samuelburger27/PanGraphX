#!/usr/bin/env bash
#
# compare_tools.sh — wall-clock comparison of PanGraphX against `vg` and `odgi`
# for equivalent format-conversion operations, using hyperfine.
#
# Designed for Linux (odgi is effectively Linux-only). Any external tool that is
# not installed is skipped with a warning, so the script degrades gracefully.
#
# Results are written to benchmarks/results/ as both Markdown (for the thesis)
# and JSON (for further plotting).
#
# Usage:
#   benchmarks/compare_tools.sh [DATASET ...]
# With no arguments the default datasets below are used. A DATASET is the base
# filename (without extension) of a graph present under
# pangraphx-core/tests/test_files/{gfa,vg,og}/.

set -euo pipefail

# --- Locations ---------------------------------------------------------------
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
TESTDIR="$REPO_ROOT/pangraphx-core/tests/test_files"
RESULTS="$SCRIPT_DIR/results"
PANGRAPHX="$REPO_ROOT/target/release/pangraphx-cli"

WARMUP="${WARMUP:-3}"
RUNS="${RUNS:-10}"

# Default datasets (base names). Override by passing names as arguments.
if [[ $# -gt 0 ]]; then
    DATASETS=("$@")
else
    DATASETS=(escherichia_phage_lambda mycoplasma_genitalium merged)
fi

mkdir -p "$RESULTS"

# Scratch dir for conversion outputs; cleaned up on exit.
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

# --- Tool discovery ----------------------------------------------------------
have() { command -v "$1" >/dev/null 2>&1; }

if ! have hyperfine; then
    echo "ERROR: hyperfine is required but not found on PATH." >&2
    echo "       Install it (e.g. 'cargo install hyperfine' or via your package manager)." >&2
    exit 1
fi

HAVE_VG=false
HAVE_ODGI=false
have vg && HAVE_VG=true || echo "WARNING: 'vg' not found — skipping vg comparisons." >&2
have odgi && HAVE_ODGI=true || echo "WARNING: 'odgi' not found — skipping odgi comparisons." >&2

# --- Build PanGraphX (release, with odgi so .og conversions work) ------------
if $HAVE_ODGI; then
    echo ">>> Building PanGraphX (release, --features odgi)..."
    cargo build --release --features odgi --manifest-path "$REPO_ROOT/Cargo.toml"
else
    echo ">>> Building PanGraphX (release)..."
    cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml"
fi

if [[ ! -x "$PANGRAPHX" ]]; then
    echo "ERROR: PanGraphX binary not found at $PANGRAPHX" >&2
    exit 1
fi

# --- Comparison helper -------------------------------------------------------
# run_compare <title> <outbase> <name::cmd> [<name::cmd> ...]
# Each command is a single string executed by hyperfine via 'sh -c', so shell
# redirections inside the string work as written.
run_compare() {
    local title="$1"; shift
    local outbase="$1"; shift
    local hf_args=(
        --warmup "$WARMUP" --runs "$RUNS"
        --export-markdown "$RESULTS/$outbase.md"
        --export-json "$RESULTS/$outbase.json"
    )
    local spec name cmd
    for spec in "$@"; do
        name="${spec%%::*}"
        cmd="${spec#*::}"
        hf_args+=(--command-name "$name" "$cmd")
    done
    echo
    echo "=== $title ==="
    hyperfine "${hf_args[@]}"
}

# --- Per-dataset comparisons -------------------------------------------------
for ds in "${DATASETS[@]}"; do
    gfa="$TESTDIR/gfa/$ds.gfa"
    vg="$TESTDIR/vg/$ds.vg"
    og="$TESTDIR/og/$ds.og"

    echo
    echo "############################################################"
    echo "# Dataset: $ds"
    echo "############################################################"

    # GFA <-> VG  vs. vg convert
    if $HAVE_VG; then
        if [[ -f "$gfa" ]]; then
            run_compare "GFA -> VG ($ds)" "gfa_to_vg_$ds" \
                "pangraphx::$PANGRAPHX convert -i $gfa -o $WORK/${ds}_px.vg" \
                "vg::vg convert --gfa-in --vg-out $gfa > $WORK/${ds}_vg.vg"
        fi
        if [[ -f "$vg" ]]; then
            run_compare "VG -> GFA ($ds)" "vg_to_gfa_$ds" \
                "pangraphx::$PANGRAPHX convert -i $vg -o $WORK/${ds}_px.gfa" \
                "vg::vg convert --gfa-out $vg > $WORK/${ds}_vg.gfa"
        fi
    fi

    # GFA <-> OG  vs. odgi build / odgi view
    if $HAVE_ODGI; then
        if [[ -f "$gfa" ]]; then
            run_compare "GFA -> OG ($ds)" "gfa_to_og_$ds" \
                "pangraphx::$PANGRAPHX convert -i $gfa -o $WORK/${ds}_px.og" \
                "odgi::odgi build -g $gfa -o $WORK/${ds}_odgi.og"
        fi
        if [[ -f "$og" ]]; then
            run_compare "OG -> GFA ($ds)" "og_to_gfa_$ds" \
                "pangraphx::$PANGRAPHX convert -i $og -o $WORK/${ds}_px.gfa" \
                "odgi::odgi view -i $og -g > $WORK/${ds}_odgi.gfa"
        fi
    fi
done

echo
echo ">>> Done. Markdown + JSON results in: $RESULTS"
