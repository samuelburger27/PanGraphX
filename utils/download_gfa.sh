#!/usr/bin/env bash
set -euo pipefail

# Download the large benchmark GFA graphs into benchmarks/data/, where the
# benchmark scripts and `PANGRAPHX_BENCH_DATA_DIR=auto` look for them.
# Sources: pangraphx-core/tests/test_files/source.md

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GFA_DIR="${SCRIPT_DIR}/../benchmarks/data"
mkdir -p "${GFA_DIR}"
GFA_DIR="$(cd "${GFA_DIR}" && pwd)"

TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

# E. coli pangenome graph (50 haplotypes)
ECOLI_GFA="${GFA_DIR}/ecoli50.gfa"
if [ -f "${ECOLI_GFA}" ]; then
    echo "Already present, skipping: ${ECOLI_GFA}"
else
    echo "Downloading ecoli50.gfa.zst..."
    curl -L --fail "https://zenodo.org/api/records/7937947/files/ecoli50.gfa.zst/content" \
        -o "${TMP_DIR}/ecoli50.gfa.zst"
    zstd -d "${TMP_DIR}/ecoli50.gfa.zst" -o "${TMP_DIR}/ecoli50.gfa"
    mv "${TMP_DIR}/ecoli50.gfa" "${ECOLI_GFA}"
fi

# HPRC chr22 human pangenome graph
CHR22_GFA="${GFA_DIR}/chr22.hprc-v1.1-mc-grch38.gfa"
if [ -f "${CHR22_GFA}" ]; then
    echo "Already present, skipping: ${CHR22_GFA}"
else
    echo "Downloading chr22.zip..."
    curl -L --fail "https://zenodo.org/api/records/17173731/files/chr22.zip/content" \
        -o "${TMP_DIR}/chr22.zip"
    unzip -q "${TMP_DIR}/chr22.zip" -d "${TMP_DIR}/chr22"
    gunzip -c "${TMP_DIR}/chr22/chr22/chr22.hprc-v1.1-mc-grch38.gfa.gz" \
        > "${TMP_DIR}/chr22.hprc-v1.1-mc-grch38.gfa"
    mv "${TMP_DIR}/chr22.hprc-v1.1-mc-grch38.gfa" "${CHR22_GFA}"
fi

# HPRC v1.0 whole-genome minigraph graph (rGFA: segments only, no paths).
# Segments are named `s1`, `s2`, ...; odgi only accepts integer names, so the
# `s` prefix is stripped from S and L lines and every tool reads the same
# numeric copy. An unmodified download already in GFA_DIR is reused.
MINIGRAPH_NAME="hprc-v1.0-minigraph-chm13"
MINIGRAPH_GFA="${GFA_DIR}/${MINIGRAPH_NAME}.num.gfa"
if [ -f "${MINIGRAPH_GFA}" ]; then
    echo "Already present, skipping: ${MINIGRAPH_GFA}"
else
    T=$'\t'
    numeric_ids() {
        LC_ALL=C sed -E \
            -e "s/^(S${T})s([0-9]+)${T}/\1\2${T}/" \
            -e "s/^(L${T})s([0-9]+)(${T}[+-]${T})s([0-9]+)${T}/\1\2\3\4${T}/"
    }
    if [ -f "${GFA_DIR}/${MINIGRAPH_NAME}.gfa" ]; then
        echo "Renumbering existing ${MINIGRAPH_NAME}.gfa..."
        numeric_ids < "${GFA_DIR}/${MINIGRAPH_NAME}.gfa" > "${TMP_DIR}/${MINIGRAPH_NAME}.num.gfa"
    else
        echo "Downloading ${MINIGRAPH_NAME}.gfa.gz..."
        curl -L --fail "https://human-pangenomics.s3.amazonaws.com/pangenomes/freeze/freeze1/minigraph/${MINIGRAPH_NAME}.gfa.gz" \
            -o "${TMP_DIR}/${MINIGRAPH_NAME}.gfa.gz"
        gunzip -c "${TMP_DIR}/${MINIGRAPH_NAME}.gfa.gz" | numeric_ids > "${TMP_DIR}/${MINIGRAPH_NAME}.num.gfa"
    fi
    mv "${TMP_DIR}/${MINIGRAPH_NAME}.num.gfa" "${MINIGRAPH_GFA}"
fi

echo "Done. GFA files are in ${GFA_DIR}"
