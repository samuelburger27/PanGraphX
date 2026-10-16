#!/usr/bin/env bash
#
# compare_tools.sh — wall-clock comparison of PanGraphX against `vg` and `odgi`
# for equivalent format conversions, using hyperfine.
#
# Usage:
#   benchmarks/compare_tools.sh [DATASET ...]
#
# A DATASET is a GFA base name looked up in benchmarks/data/ and then
# pangraphx-core/tests/test_files/gfa/, or a path to a .gfa file. Without
# arguments DEFAULT_DATASETS from lib.sh is used; missing ones are skipped.
#
# The operations are defined once in ops.sh. Reverse conversions (X -> GFA)
# read the X file produced by the reference tool, built once per dataset before
# timing starts. Missing tools are skipped with a warning; a failing comparison
# is recorded in failures.txt and the run continues.
#
# Environment:
#   PANGRAPHX_BIN  benchmark this pre-built binary instead of building
#   SKIP_BUILD=1   reuse target/release/pangraphx-cli without rebuilding
#   WARMUP, RUNS   fixed hyperfine sampling (default: scaled by input size)
#   OPS            space-separated subset of operations (default: all)
#   WORK           directory for intermediate files (default: temporary)
#   RESULTS_DIR    output directory (default: results/compare_tools/<run>/)
#
# Output (in the run directory): <op>_<dataset>.{md,json} per comparison,
# summary.md with every table, and env.txt describing the machine and versions.

set -euo pipefail
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
# shellcheck source=ops.sh
source "$BENCH_DIR/ops.sh"

have hyperfine || die "hyperfine is required (e.g. 'cargo install hyperfine')."
discover_tools
ensure_pangraphx
select_datasets "$@"
init_run_dir compare_tools

if [[ -z "${WORK:-}" ]]; then
    WORK="$(mktemp -d)"
    trap 'rm -rf "$WORK"' EXIT
fi
read -r -a OPS_SELECTED <<<"${OPS:-${ALL_OPS[*]}}"

SUMMARY="$RUN_DIR/summary.md"
echo "# PanGraphX vs. vg / odgi — wall-clock time" >"$SUMMARY"

for ds in "${DATASETS[@]}"; do
    if ! gfa="$(resolve_dataset "$ds")"; then skip_banner "$ds"; continue; fi
    name="$(dataset_name "$ds")"
    dir="$WORK/$name"
    dataset_banner "$name ($gfa)"

    sampling_for_size "$(file_size "$gfa")"
    prepare_inputs "$dir" "$gfa"
    printf '\n## %s (warmup %s, runs %s)\n' "$name" "$HF_WARMUP" "$HF_RUNS" >>"$SUMMARY"

    for op in "${OPS_SELECTED[@]}"; do
        if ! op_available "$op"; then continue; fi
        if ! op_ready "$op" "$dir" "$gfa"; then
            record_failure "$op $name" "missing input $(op_input "$op" "$dir" "$gfa")"
            continue
        fi
        tool="$(op_tool "$op")"
        out="$RUN_DIR/${op}_$name"

        echo
        echo "=== $(op_title "$op") ($name) ==="
        # An explicit --shell is required by hyperfine >= 2 for the '>' in
        # the reference-tool commands.
        if hyperfine --shell sh --warmup "$HF_WARMUP" --runs "$HF_RUNS" \
            --export-markdown "$out.md" --export-json "$out.json" \
            --command-name pangraphx "$(op_px_cmd "$op" "$dir" "$gfa")" \
            --command-name "$tool" "$(op_tool_cmd "$op" "$dir" "$gfa")"; then
            printf '\n### %s\n\n' "$(op_title "$op")" >>"$SUMMARY"
            cat "$out.md" >>"$SUMMARY"
        else
            rm -f "$out.md" "$out.json"
            record_failure "$op $name" "hyperfine exited non-zero"
        fi
    done
done

finish_run
