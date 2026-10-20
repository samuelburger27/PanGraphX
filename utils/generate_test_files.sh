#!/usr/bin/env bash
#
# generate_test_files.sh — derive VG / GBZ / FASTG / OG fixtures
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
#                  against. Missing tools are skipped with a warning and fall
#                  back to PanGraphX.
#   --reference    Write the committed golden files that
#                  tests/test_reference_files.rs reads: vg/odgi output for the
#                  small fixtures plus MANIFEST.tsv, into
#                  tests/test_files/reference/ (or --out DIR). Fails when a
#                  tool is missing; never falls back to PanGraphX.
#
# Options:
#   --out DIR      Output directory for --reference.
#   --force        Regenerate even when the target already exists.
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
OUT_DIR="$TEST_FILES_DIR/reference"

usage() {
    sed -n '2,28p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --external) MODE=external ;;
        --reference) MODE=reference ;;
        --out) OUT_DIR="${2:?--out needs a directory}"; shift ;;
        --force) FORCE=true ;;
        -h|--help) usage ;;
        *) echo "Unknown argument: $1" >&2; exit 2 ;;
    esac
    shift
done

have() { command -v "$1" >/dev/null 2>&1; }

# --- Golden reference files (--reference) ---------------------------------------
# Fixtures small enough to commit, and the formats each tool cannot write:
# `vg convert` rejects overlaps (tiny), `odgi build` needs integer names
# (names_mixed). `vg gbwt --max-node 0` keeps nodes unchopped.
REFERENCE_FIXTURES=(tiny names_mixed random phix escherichia_phage_lambda)
reference_skips() {
    case "$1:$2" in
        tiny:vg|names_mixed:og) return 0 ;;
        *) return 1 ;;
    esac
}

sha256() {
    if have sha256sum; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

generate_reference() {
    local tool
    for tool in vg odgi; do
        if ! have "$tool"; then
            echo "ERROR: --reference needs '$tool' on PATH" >&2
            exit 1
        fi
    done
    local vg_version odgi_version
    vg_version="$(vg version | head -1)"
    odgi_version="$(odgi version | head -1)"

    mkdir -p "$OUT_DIR/vg" "$OUT_DIR/gbz" "$OUT_DIR/og"
    local manifest="$OUT_DIR/MANIFEST.tsv"
    printf 'file\tsource_gfa\tsha256\ttool\ttool_version\tcommand\n' > "$manifest"

    local base fmt gfa out cmd tool version
    for base in "${REFERENCE_FIXTURES[@]}"; do
        gfa="$GFA_DIR/$base.gfa"
        for fmt in vg gbz og; do
            reference_skips "$base" "$fmt" && continue
            out="$fmt/$base.$fmt"
            case "$fmt" in
                vg)  tool=vg;   version="$vg_version"
                     cmd="vg convert --gfa-in --vg-out gfa/$base.gfa > $out"
                     vg convert --gfa-in --vg-out "$gfa" 2>/dev/null > "$OUT_DIR/$out" ;;
                gbz) tool=vg;   version="$vg_version"
                     cmd="vg gbwt -G gfa/$base.gfa --max-node 0 --gbz-format -g $out"
                     vg gbwt -G "$gfa" --max-node 0 --gbz-format -g "$OUT_DIR/$out" ;;
                og)  tool=odgi; version="$odgi_version"
                     cmd="odgi build -g gfa/$base.gfa -o $out"
                     odgi build -g "$gfa" -o "$OUT_DIR/$out" ;;
            esac
            printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$out" "gfa/$base.gfa" \
                "$(sha256 "$OUT_DIR/$out")" "$tool" "$version" "$cmd" >> "$manifest"
            echo "  wrote ($tool): $OUT_DIR/$out"
        done
    done
    echo "Done. Manifest: $manifest"
}

if [[ "$MODE" == "reference" ]]; then
    generate_reference
    exit 0
fi

# --- Build (or reuse) PanGraphX -----------------------------------------------
if [[ -n "${PANGRAPHX_BIN:-}" ]]; then
    BIN="$PANGRAPHX_BIN"
    if [[ ! -x "$BIN" ]]; then
        echo "ERROR: PANGRAPHX_BIN is set but not executable: $BIN" >&2
        exit 1
    fi
    echo ">>> Using pre-built PanGraphX: $BIN"
else
    echo ">>> Building PanGraphX (release)..."
    cargo build --release --manifest-path "$REPO_ROOT/Cargo.toml"
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

mkdir -p "$TEST_FILES_DIR/vg" "$TEST_FILES_DIR/gbz" "$TEST_FILES_DIR/fastg" "$TEST_FILES_DIR/og"

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
        vg gbwt -G "$gfa" --max-node 0 -g "$gbz_out"
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

    # OG (`odgi build` only accepts integer segment names; others fall back)
    og_out="$TEST_FILES_DIR/og/$base.og"
    if already_done "$og_out"; then
        echo "  skip (exists): $og_out"
    elif [[ "$MODE" == "external" ]] && $HAVE_ODGI \
            && odgi build -g "$gfa" -o "$og_out" 2>/dev/null; then
        echo "  wrote (odgi): $og_out"
    else
        convert_px "$gfa" "$og_out" og
    fi
done

echo
echo "Done. Derived files are under $TEST_FILES_DIR/{vg,gbz,fastg,og}/"
