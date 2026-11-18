#!/usr/bin/env bash
#
# generate_test_files.sh — writes the committed VG / GBZ / OG test files with
# vg and odgi from the GFA fixtures in pangraphx-core/tests/test_files/gfa/,
# plus MANIFEST.tsv (source GFA, checksum, tool version and command per file).
# One VG file is BGZF compressed, as libvgio writes by default.
#
# tests/test_reference_files.rs reads them with our codecs and compares them
# with their source GFA. Never PanGraphX output: a file we wrote ourselves
# cannot catch a bug that our reader and writer share. Needs vg and odgi on
# PATH (pinned versions: utils/install_external_tools.sh).
#
# Usage:
#   utils/generate_test_files.sh [--out DIR]
#
#   --out DIR   output directory (default pangraphx-core/tests/test_files);
#               CI writes elsewhere and points PANGRAPHX_REFERENCE_DIR at it
#               to detect tool drift.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEST_FILES_DIR="$(cd "$SCRIPT_DIR/../pangraphx-core/tests/test_files" && pwd)"
GFA_DIR="$TEST_FILES_DIR/gfa"
OUT_DIR="$TEST_FILES_DIR"

while [[ $# -gt 0 ]]; do
    case "$1" in
        --out) OUT_DIR="${2:?--out needs a directory}"; shift ;;
        -h|--help) sed -n '3,/^$/s/^# \{0,1\}//p' "$0"; exit 0 ;;
        *) echo "Unknown argument: $1" >&2; exit 2 ;;
    esac
    shift
done

have() { command -v "$1" >/dev/null 2>&1; }

# Fixtures small enough to commit, and the formats each tool cannot write:
# `vg convert` rejects overlaps (tiny), `odgi build` needs integer names
# (names_mixed) and ignores W lines (walks), and `vg gbwt` keeps only the
# start of a walk's `[start-end]` subrange (walks). `vg gbwt --max-node 0`
# keeps nodes unchopped.
FIXTURES=(tiny names_mixed random phix escherichia_phage_lambda walks)
skipped() {
    case "$1:$2" in
        tiny:vg|names_mixed:og|walks:gbz|walks:og) return 0 ;;
        *) return 1 ;;
    esac
}

# `vg convert` writes uncompressed VG; the deprecated `vg view -Fv` writes it
# BGZF compressed. Lambda spans several BGZF blocks.
BGZF_FIXTURES=(escherichia_phage_lambda)

sha256() {
    if have sha256sum; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

for tool in vg odgi; do
    if ! have "$tool"; then
        echo "ERROR: '$tool' is not on PATH" >&2
        exit 1
    fi
done
vg_version="$(vg version | head -1)"
odgi_version="$(odgi version | head -1)"

mkdir -p "$OUT_DIR/vg" "$OUT_DIR/gbz" "$OUT_DIR/og"
manifest="$OUT_DIR/MANIFEST.tsv"
printf 'file\tsource_gfa\tsha256\ttool\ttool_version\tcommand\n' > "$manifest"

for base in "${FIXTURES[@]}"; do
    gfa="$GFA_DIR/$base.gfa"
    for fmt in vg gbz og; do
        skipped "$base" "$fmt" && continue
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
for base in "${BGZF_FIXTURES[@]}"; do
    out="vg/$base.bgzf.vg"
    cmd="vg view -Fv gfa/$base.gfa > $out"
    vg view -Fv "$GFA_DIR/$base.gfa" 2>/dev/null > "$OUT_DIR/$out"
    printf '%s\t%s\t%s\t%s\t%s\t%s\n' "$out" "gfa/$base.gfa" \
        "$(sha256 "$OUT_DIR/$out")" vg "$vg_version" "$cmd" >> "$manifest"
    echo "  wrote (vg): $OUT_DIR/$out"
done
echo "Done. Manifest: $manifest"
