#!/usr/bin/env bash
#
# peak_memory.sh — peak resident-set-size (RSS) comparison of PanGraphX against
# `vg` and `odgi`, for the same operations and inputs as compare_tools.sh
# (hyperfine measures time only).
#
# Usage:
#   benchmarks/peak_memory.sh [DATASET ...]
#
# Peak RSS comes from GNU time (`-v`) on Linux or the built-in BSD time (`-l`)
# on macOS. Each command is run MEM_REPS times and the median is reported.
# Commands run one at a time, so measurements never compete for memory.
#
# Environment:
#   PANGRAPHX_BIN, SKIP_BUILD, OPS, VG_THREADS, WORK, RESULTS_DIR — as in compare_tools.sh
#   MEM_REPS       repetitions per command (default 3)
#
# Output (in results/peak_memory/<run>/): peak_memory.md (table, MiB),
# peak_memory.csv (one row per tool and operation), env.txt.

set -euo pipefail
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"
# shellcheck source=ops.sh
source "$BENCH_DIR/ops.sh"

MEM_REPS="${MEM_REPS:-3}"

detect_time
discover_tools
ensure_pangraphx
select_datasets "$@"
init_run_dir peak_memory

if [[ -z "${WORK:-}" ]]; then
    WORK="$(mktemp -d)"
    trap 'rm -rf "$WORK"' EXIT
fi
read -r -a OPS_SELECTED <<<"${OPS:-${ALL_OPS[*]}}"

MD="$RUN_DIR/peak_memory.md"
CSV="$RUN_DIR/peak_memory.csv"
{
    echo "# Peak memory (RSS, MiB, median of $MEM_REPS runs)"
    echo
    echo "| Dataset | Operation | PanGraphX | Tool | Tool RSS |"
    echo "|---------|-----------|----------:|------|---------:|"
} >"$MD"
echo "dataset,operation,tool,rss_mib,wall_s,status" >"$CSV"

# median_of <cmd> <label> — runs <cmd> MEM_REPS times and sets MED_RSS / MED_WALL
# to the medians, or MED_RSS / MED_STATUS to the failure status on error.
median_of() {
    local cmd="$1" label="$2" i rss_samples=() wall_samples=()
    for (( i = 0; i < MEM_REPS; i++ )); do
        measure "$cmd"
        if [[ "$M_STATUS" != ok ]]; then
            MED_RSS="$M_STATUS"; MED_STATUS="$M_STATUS"
            record_failure "$label" "$M_STATUS: $cmd"
            return
        fi
        rss_samples+=("$M_RSS_MIB")
        wall_samples+=("$M_WALL_S")
    done
    MED_STATUS=ok
    MED_RSS="$(median "${rss_samples[@]}")"
    MED_WALL="$(median "${wall_samples[@]}")"
}

for ds in "${DATASETS[@]}"; do
    if ! gfa="$(resolve_dataset "$ds")"; then skip_banner "$ds"; continue; fi
    name="$(dataset_name "$ds")"
    dir="$WORK/$name"
    dataset_banner "$name ($gfa)"
    prepare_inputs "$dir" "$gfa"

    for op in "${OPS_SELECTED[@]}"; do
        if ! op_available "$op"; then continue; fi
        if ! op_ready "$op" "$dir" "$gfa"; then
            record_failure "$op $name" "missing input $(op_input "$op" "$dir" "$gfa")"
            continue
        fi
        tool="$(op_tool "$op")"

        MED_WALL=NA
        median_of "$(op_px_cmd "$op" "$dir" "$gfa")" "$op $name pangraphx"
        px_rss="$MED_RSS"; px_wall="$MED_WALL"; px_status="$MED_STATUS"
        MED_WALL=NA
        median_of "$(op_tool_cmd "$op" "$dir" "$gfa")" "$op $name $tool"
        tool_rss="$MED_RSS"; tool_wall="$MED_WALL"; tool_status="$MED_STATUS"

        echo "| $name | $(op_title "$op") | $px_rss | $tool | $tool_rss |" >>"$MD"
        echo "$name,$op,pangraphx,$px_rss,$px_wall,$px_status" >>"$CSV"
        echo "$name,$op,$tool,$tool_rss,$tool_wall,$tool_status" >>"$CSV"
        echo "  $(op_title "$op"): pangraphx=${px_rss} MiB, ${tool}=${tool_rss} MiB"
    done
done

finish_run
