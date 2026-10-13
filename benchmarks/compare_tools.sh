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
# filename (without extension) of a GFA graph present under
# pangraphx-core/tests/test_files/gfa/.
#
# By default PanGraphX is built from source with `cargo build --release`. To
# benchmark a pre-built executable instead, set PANGRAPHX_BIN to its path:
#   PANGRAPHX_BIN=/path/to/pangraphx-cli benchmarks/compare_tools.sh
#
# Only GFA files are used as the input dataset. For each target format the
# forward conversion (GFA -> target) is benchmarked first; the target file
# produced by the *original* tool (vg/odgi, not PanGraphX) is then reused as the
# input for the reverse benchmark (target -> GFA).

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
    # DATASETS=(escherichia_phage_lambda mycoplasma_genitalium merged ecoli50 chr22.hprc-v1.1-mc-grch38)
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

# --- Build (or reuse) PanGraphX -----------------------------------------------
# PANGRAPHX_BIN lets you benchmark a pre-built executable instead of building
# from source (which requires `cargo`). It must support the `convert` subcommand.
if [[ -n "${PANGRAPHX_BIN:-}" ]]; then
    PANGRAPHX="$PANGRAPHX_BIN"
    if [[ ! -x "$PANGRAPHX" ]]; then
        echo "ERROR: PANGRAPHX_BIN is set but not executable: $PANGRAPHX" >&2
        exit 1
    fi
    echo ">>> Using pre-built PanGraphX: $PANGRAPHX"
else
    # Build PanGraphX (release, with odgi so .og conversions work)
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

    echo
    echo "############################################################"
    echo "# Dataset: $ds"
    echo "############################################################"

    if [[ ! -f "$gfa" ]]; then
        echo "SKIP: no GFA input at $gfa" >&2
        continue
    fi

    # GFA <-> VG  vs. vg convert
    if $HAVE_VG; then
        # Forward: GFA -> VG. The VG produced by vg is reused below as input.
        vg_input="$WORK/${ds}_vg.vg"
        run_compare "GFA -> VG ($ds)" "gfa_to_vg_$ds" \
            "pangraphx::$PANGRAPHX convert -i $gfa -o $WORK/${ds}_px.vg" \
            "vg::vg convert --gfa-in --vg-out $gfa > $vg_input"

        # Reverse: VG -> GFA, reusing the vg-produced VG file.
        run_compare "VG -> GFA ($ds)" "vg_to_gfa_$ds" \
            "pangraphx::$PANGRAPHX convert -i $vg_input -o $WORK/${ds}_px.gfa" \
            "vg::vg convert --gfa-out $vg_input > $WORK/${ds}_vg.gfa"
    fi

    # GFA <-> OG  vs. odgi build / odgi view
    if $HAVE_ODGI; then
        # Forward: GFA -> OG. The OG produced by odgi is reused below as input.
        og_input="$WORK/${ds}_odgi.og"
        run_compare "GFA -> OG ($ds)" "gfa_to_og_$ds" \
            "pangraphx::$PANGRAPHX convert -i $gfa -o $WORK/${ds}_px.og" \
            "odgi::odgi build -g $gfa -o $og_input"

        # Reverse: OG -> GFA, reusing the odgi-produced OG file.
        run_compare "OG -> GFA ($ds)" "og_to_gfa_$ds" \
            "pangraphx::$PANGRAPHX convert -i $og_input -o $WORK/${ds}_px.gfa" \
            "odgi::odgi view -i $og_input -g > $WORK/${ds}_odgi.gfa"
    fi
done

echo
echo ">>> Done. Markdown + JSON results in: $RESULTS"
