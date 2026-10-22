#!/usr/bin/env bash
#
# dbg.sh — de Bruijn graph construction benchmark (time + peak RSS) of the
# PanGraphX CLI (`pangraphx-cli dbg`), swept over mode, k and thread count.
#
# Usage:
#   benchmarks/dbg.sh [DATASET ...]
#
# DBG memory scales with the number of distinct k-mers (about the total node
# sequence length of the input); colored output adds one step per path base.
# The uncompacted output graph (one node per k-mer) dominates, `--compact`
# output is far smaller. Thread count barely matters. So this script:
#   * runs exactly one construction at a time — never run it alongside other
#     benchmarks, and never run several instances in parallel;
#   * estimates each configuration's memory up front from the dataset's node
#     and path lengths and skips configurations over the budget;
#   * watches every run and kills it once its RSS exceeds the budget (recorded
#     as `oom-guard`), or once it exceeds the time limit (`timeout`);
#   * after a memory-guard hit, skips the larger thread counts of that mode and
#     k, since they would not use less memory (a timeout does not, because
#     more threads finish sooner).
#
# Environment:
#   K_VALUES            k-mer sizes (default "21 31 63"; max 63)
#   MODES               subset of "standard colored full_topology compact
#                       colored_compact" (default all); the *compact modes add
#                       `--compact` (unitig output)
#   THREADS             RAYON_NUM_THREADS values; "max" = all cores
#                       (default "1 2 4 max")
#   DBG_MEM_BUDGET_MIB  memory limit per run (default 60% of physical RAM)
#   DBG_TIMEOUT_S       time limit per run (default 1800)
#   DBG_BYTES_PER_NODE_BP, DBG_COMPACT_BYTES_PER_NODE_BP, DBG_BYTES_PER_STEP_BP
#                       cost model for the pre-flight estimate (defaults 240,
#                       60 and 32; calibrate against dbg.csv or the dbg_memory
#                       bench)
#   DBG_FULL_TOPOLOGY_MAX_BP
#                       skip full_topology above this many path bases, since its
#                       walk enumeration can blow up (default 5000000);
#                       DBG_ALLOW_FULL_TOPOLOGY=1 disables the cap
#   PANGRAPHX_BIN, SKIP_BUILD, WORK, RESULTS_DIR — as in compare_tools.sh
#
# Output (in results/dbg/<run>/): dbg.csv (one row per configuration,
# including skipped ones), dbg.md (table), env.txt.

set -euo pipefail
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

read -r -a K_LIST <<<"${K_VALUES:-21 31 63}"
read -r -a MODE_LIST <<<"${MODES:-standard colored full_topology compact colored_compact}"
read -r -a THREAD_LIST <<<"${THREADS:-1 2 4 max}"
RAM_MIB="$(total_ram_mib)"
BUDGET_MIB="${DBG_MEM_BUDGET_MIB:-$(( RAM_MIB * 60 / 100 ))}"
TIMEOUT_S="${DBG_TIMEOUT_S:-1800}"
BYTES_PER_NODE_BP="${DBG_BYTES_PER_NODE_BP:-240}"
COMPACT_BYTES_PER_NODE_BP="${DBG_COMPACT_BYTES_PER_NODE_BP:-60}"
BYTES_PER_STEP_BP="${DBG_BYTES_PER_STEP_BP:-32}"
FULL_TOPOLOGY_MAX_BP="${DBG_FULL_TOPOLOGY_MAX_BP:-5000000}"
CPUS="$(cpu_count)"

detect_time
ensure_pangraphx
select_datasets "$@"
init_run_dir dbg
echo "dbg:       K_VALUES=${K_LIST[*]} MODES=${MODE_LIST[*]} THREADS=${THREAD_LIST[*]} budget=${BUDGET_MIB}MiB timeout=${TIMEOUT_S}s" \
    >>"$RUN_DIR/env.txt"
log "Memory budget per run: $BUDGET_MIB MiB of $RAM_MIB MiB; one run at a time."

if [[ -z "${WORK:-}" ]]; then
    WORK="$(mktemp -d)"
    trap 'rm -rf "$WORK"' EXIT
fi

CSV="$RUN_DIR/dbg.csv"
MD="$RUN_DIR/dbg.md"
echo "dataset,path_bp,mode,k,threads,est_mib,rss_mib,wall_s,status" >"$CSV"
{
    echo "# de Bruijn graph construction (pangraphx-cli dbg)"
    echo
    echo "Budget $BUDGET_MIB MiB, timeout ${TIMEOUT_S}s. Output written as GFA (included in time)."
    echo
    echo "| Dataset | Mode | k | Threads | Peak RSS [MiB] | Wall [s] | Status |"
    echo "|---------|------|--:|--------:|---------------:|---------:|--------|"
} >"$MD"

# path_bp <gfa> -> total bases over all paths (via `pangraphx-cli stats`).
path_bp() {
    NO_COLOR=1 "$PANGRAPHX" stats "$1" 2>/dev/null |
        sed -n 's/.*(total \([0-9,]*\) bp).*/\1/p' | tr -d ',' | head -1
}

# node_bp <gfa> -> total node sequence length (via `pangraphx-cli stats`).
node_bp() {
    NO_COLOR=1 "$PANGRAPHX" stats "$1" 2>/dev/null |
        sed -n 's/^ *Total length: *\([0-9,]*\) bp.*/\1/p' | tr -d ',' | head -1
}

# estimate_mib <mode> <node_bp> <path_bp> <gfa_bytes> -> rough peak MiB: a fixed
# 64 MiB, the parsed input (~3x its file size), plus per node base (≈ per
# distinct k-mer) the k-mer tables and the output graph, plus for uncompacted
# colored output one path step per path base. Uncompacted, the output dominates:
# ~90 B per k-mer for the DTO node + edge and ~140 B for their GFA lines, which
# the serializer formats all at once. The dbg_memory bench puts construction
# alone at ~36 B per k-mer and compaction at ~26 B more.
estimate_mib() {
    local per_node="$BYTES_PER_NODE_BP" per_step=0
    case "$1" in
        compact | colored_compact) per_node="$COMPACT_BYTES_PER_NODE_BP" ;;
    esac
    [[ "$1" == colored ]] && per_step="$BYTES_PER_STEP_BP"
    awk -v n="$2" -v p="$3" -v f="$4" -v c="$per_node" -v s="$per_step" \
        'BEGIN { printf "%d", 64 + (3 * f + n * c + p * s) / 1048576 }'
}

mode_flag() {
    case "$1" in
        standard)      echo "" ;;
        colored)       echo "-c" ;;
        full_topology) echo "-f" ;;
        compact)       echo "--compact" ;;
        colored_compact) echo "-c --compact" ;;
        *)             die "unknown mode '$1' (expected standard, colored, full_topology, compact, colored_compact)" ;;
    esac
}

# row <fields...> — appends a CSV row and a Markdown row.
row() {
    local ds="$1" bp="$2" mode="$3" k="$4" t="$5" est="$6" rss="$7" wall="$8" status="$9"
    echo "$ds,$bp,$mode,$k,$t,$est,$rss,$wall,$status" >>"$CSV"
    echo "| $ds | $mode | $k | $t | $rss | $wall | $status |" >>"$MD"
    printf '  %-14s k=%-3s threads=%-3s est=%6s MiB  rss=%8s MiB  wall=%8s s  %s\n' \
        "$mode" "$k" "$t" "$est" "$rss" "$wall" "$status"
}

for ds in "${DATASETS[@]}"; do
    if ! gfa="$(resolve_dataset "$ds")"; then skip_banner "$ds"; continue; fi
    name="$(dataset_name "$ds")"
    dataset_banner "$name ($gfa)"

    bp="$(path_bp "$gfa")"
    if [[ -z "$bp" || "$bp" == 0 ]]; then
        record_failure "dbg $name" "no path bases reported by 'stats' (DBG needs paths)"
        continue
    fi
    gfa_bytes="$(file_size "$gfa")"
    nbp="$(node_bp "$gfa")"
    log "Total path length: $bp bp, node length: ${nbp:-?} bp"

    for mode in "${MODE_LIST[@]}"; do
        flag="$(mode_flag "$mode")"
        if [[ "$mode" == full_topology && "${DBG_ALLOW_FULL_TOPOLOGY:-0}" != 1 ]] &&
            (( bp > FULL_TOPOLOGY_MAX_BP )); then
            for k in "${K_LIST[@]}"; do
                row "$name" "$bp" "$mode" "$k" "-" "-" NA NA "skipped-full-topology-cap"
            done
            continue
        fi
        for k in "${K_LIST[@]}"; do
            guard_hit=false
            for t in "${THREAD_LIST[@]}"; do
                [[ "$t" == max ]] && t="$CPUS"
                est="$(estimate_mib "$mode" "${nbp:-$bp}" "$bp" "$gfa_bytes")"
                if $guard_hit; then
                    row "$name" "$bp" "$mode" "$k" "$t" "$est" NA NA "skipped-after-guard"
                    continue
                fi
                if (( est > BUDGET_MIB )); then
                    row "$name" "$bp" "$mode" "$k" "$t" "$est" NA NA "skipped-estimate"
                    continue
                fi

                out="$WORK/${name}_dbg.gfa"
                RAYON_NUM_THREADS="$t" GUARD_MEM_MIB="$BUDGET_MIB" GUARD_TIME_S="$TIMEOUT_S" \
                    measure "$PANGRAPHX dbg -k $k $flag $gfa -o $out"
                rm -f "$out"
                row "$name" "$bp" "$mode" "$k" "$t" "$est" "$M_RSS_MIB" "$M_WALL_S" "$M_STATUS"
                case "$M_STATUS" in
                    ok) ;;
                    oom-guard) guard_hit=true
                        record_failure "dbg $name $mode k=$k t=$t" "$M_STATUS" ;;
                    *) record_failure "dbg $name $mode k=$k t=$t" "$M_STATUS" ;;
                esac
            done
        done
    done
done

finish_run
