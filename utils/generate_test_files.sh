#!/usr/bin/env bash
#
# generate_test_files.sh — derive VG / GBZ / FASTG (and optionally OG) fixtures
# from the committed GFA files under pangraphx-core/tests/test_files/gfa/.
#
# The repository commits only GFA files; every derived format is produced here
# and gitignored, so no binary fixtures are checked in by hand.
#
# Modes:
#   (default)      PanGraphX `convert` (self-contained; used for benchmarks and
#                  local development). Note that this is PanGraphX output, so it
#                  must NOT be used to validate PanGraphX against itself — for
#                  that, use --external.
#   --external     Use the reference tools `vg` and `odgi` where available to
#                  produce files that PanGraphX can later be cross-validated
#                  against (the CI path). Missing tools are skipped with a
#                  warning and fall back to PanGraphX.
#
# Options:
#   --force        Regenerate even when the target already exists.
#   --with-odgi    Also produce .og files (requires the `odgi` cargo feature,
#                  which builds on Linux only). In --external mode, `odgi build`
#                  is used instead when `odgi` is on PATH.
#   --help         Show this help.
#
# Environment:
#   PANGRAPHX_BIN  Path to a pre-built `pangraphx-cli` to use instead of building.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
GFA_DIR="$REPO_ROOT/pangraphx-core/tests/test_files/gfa"
TEST_FILES_DIR="$(cd "$GFA_DIR/.." && pwd)"
BIN="$REPO_ROOT/target/release/pangraphx-cli"

MODE=pangraphx
FORCE=false
WITH_ODGI=false

usage() {
    sed -n '2,24p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --external) MODE=external ;;
        --force) FORCE=true ;;
        --with-odgi) WITH_ODGI=true ;;
        -h|--help) usage ;;
        *) echo "Unknown argument: $1" >&2; exit 2 ;;
    esac
    shift
done

have() { command -v "$1" >/dev/null 2>&1; }

# --- Build (or reuse) PanGraphX -----------------------------------------------
if [[ -n "${PANGRAPHX_BIN:-}" ]]; then
    BIN="$PANGRAPHX_BIN"
    if [[ ! -x "$BIN" ]]; then
        echo "ERROR: PANGRAPHX_BIN is set but not executable: $BIN" >&2
        exit 1
    fi
    echo ">>> Using pre-built PanGraphX: $BIN"
else
    if $WITH_ODGI; then
        echo ">>> Building PanGraphX (release, --features pangraphx-core/odgi)..."
        cargo build --release --features pangraphx-core/odgi \
            --manifest-path "$REPO_ROOT/Cargo.toml"
    else
        echo ">>> Building PanGraphX (release)..."
        cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml"
    fi
fi

# --- Helpers ------------------------------------------------------------------
already_done() {
    [[ -f "$1" ]] && ! $FORCE
}

# convert_px <src> <dst> <format> — convert via PanGraphX.
convert_px() {
    local src="$1" dst="$2" fmt="$3"
    "$BIN" convert "$src" -o "$dst" --from gfa --to "$fmt" >/dev/null
    echo "  wrote: $dst"
}

# --- Discovery ----------------------------------------------------------------
HAVE_VG=false; have vg && HAVE_VG=true
HAVE_ODGI=false; have odgi && HAVE_ODGI=true

if [[ "$MODE" == "external" ]]; then
    if ! $HAVE_VG && ! $HAVE_ODGI; then
        echo "WARNING: --external given but neither 'vg' nor 'odgi' found; " \
             "falling back to PanGraphX for everything." >&2
    fi
fi

shopt -s nullglob
GFA_FILES=("$GFA_DIR"/*.gfa)
if [[ ${#GFA_FILES[@]} -eq 0 ]]; then
    echo "ERROR: no GFA files found under $GFA_DIR" >&2
    exit 1
fi

mkdir -p "$TEST_FILES_DIR/vg" "$TEST_FILES_DIR/gbz" "$TEST_FILES_DIR/fastg"
if $WITH_ODGI; then
    mkdir -p "$TEST_FILES_DIR/og"
fi

# --- Generate -----------------------------------------------------------------
for gfa in "${GFA_FILES[@]}"; do
    base="$(basename "$gfa" .gfa)"
    echo ">>> $base"

    # VG
    vg_out="$TEST_FILES_DIR/vg/$base.vg"
    if already_done "$vg_out"; then
        echo "  skip (exists): $vg_out"
    elif [[ "$MODE" == "external" ]] && $HAVE_VG; then
        vg convert --gfa-in --vg-out "$gfa" > "$vg_out"
        echo "  wrote (vg): $vg_out"
    else
        convert_px "$gfa" "$vg_out" vg
    fi

    # GBZ (vg `gbwt` builds it in --external mode)
    gbz_out="$TEST_FILES_DIR/gbz/$base.gbz"
    if already_done "$gbz_out"; then
        echo "  skip (exists): $gbz_out"
    elif [[ "$MODE" == "external" ]] && $HAVE_VG; then
        vg gbwt -G "$gfa" -g "$gbz_out"
        echo "  wrote (vg): $gbz_out"
    else
        convert_px "$gfa" "$gbz_out" gbz
    fi

    # FASTG (no standalone external tool; PanGraphX only)
    fastg_out="$TEST_FILES_DIR/fastg/$base.fastg"
    if already_done "$fastg_out"; then
        echo "  skip (exists): $fastg_out"
    else
        convert_px "$gfa" "$fastg_out" fastg
    fi

    # OG (optional)
    if $WITH_ODGI; then
        og_out="$TEST_FILES_DIR/og/$base.og"
        if already_done "$og_out"; then
            echo "  skip (exists): $og_out"
        elif [[ "$MODE" == "external" ]] && $HAVE_ODGI; then
            odgi build -g "$gfa" -o "$og_out"
            echo "  wrote (odgi): $og_out"
        else
            convert_px "$gfa" "$og_out" og
        fi
    fi
done

echo
echo "Done. Derived files are under $TEST_FILES_DIR/{vg,gbz,fastg,og}/"
