# shellcheck shell=bash
#
# lib.sh — shared helpers for the benchmark scripts. Sourced, never executed.
#
# Provides: repository paths, dataset resolution, tool discovery, building (or
# reusing) the PanGraphX binary, per-run result directories with environment
# metadata, size-scaled hyperfine sampling, and a cross-platform peak-RSS
# measurement with an optional memory/time watchdog.
#
# Kept compatible with bash 3.2 (the macOS system bash): no associative arrays,
# no mapfile, and no empty-array expansion under `set -u`.

# Force a C locale so awk/printf use '.' as the decimal separator (some locales
# print "0,00", which breaks number parsing).
export LC_ALL=C

BENCH_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$BENCH_DIR/.." && pwd)"
RESULTS_ROOT="$BENCH_DIR/results"

# Directories searched (in order) for `<name>.gfa` datasets. Large downloaded
# graphs live in benchmarks/data/ (see utils/download_gfa.sh); the small curated
# fixtures live with the tests.
DATA_DIRS=("$BENCH_DIR/data" "$REPO_ROOT/pangraphx-core/tests/test_files/gfa")

# Datasets used when a script is run without arguments, smallest first. Missing
# ones (e.g. the large downloads) are skipped with a warning.
DEFAULT_DATASETS=(
    phix
    escherichia_phage_lambda
    mycoplasma_genitalium
    merged
    random_medium
    ecoli50
    chr22.hprc-v1.1-mc-grch38
)

PANGRAPHX="$REPO_ROOT/target/dist/pangraphx-cli"

log()  { echo ">>> $*"; }
warn() { echo "WARNING: $*" >&2; }
die()  { echo "ERROR: $*" >&2; exit 1; }
have() { command -v "$1" >/dev/null 2>&1; }

# Print a high-resolution UNIX timestamp (seconds, 3 decimals). `date +%N` is
# not available on macOS, so perl is used instead.
now() { perl -MTime::HiRes=time -e 'printf "%.3f\n", time'; }

# file_size <path> -> size in bytes (GNU and BSD stat differ).
file_size() { stat -c %s "$1" 2>/dev/null || stat -f %z "$1"; }

# total_ram_mib -> physical memory in MiB.
total_ram_mib() {
    if [[ -r /proc/meminfo ]]; then
        awk '/^MemTotal:/ { printf "%d\n", $2 / 1024 }' /proc/meminfo
    else
        echo $(( $(sysctl -n hw.memsize) / 1024 / 1024 ))
    fi
}

# cpu_count -> number of logical CPUs.
cpu_count() { getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.ncpu; }

# --- Datasets ---------------------------------------------------------------

# resolve_dataset <name|path> -> absolute path of the GFA, or non-zero status.
# A name is looked up as `<name>.gfa` in each of DATA_DIRS in turn; an existing
# file path is accepted as-is.
resolve_dataset() {
    local ds="$1" dir
    if [[ -f "$ds" ]]; then
        echo "$(cd "$(dirname "$ds")" && pwd)/$(basename "$ds")"
        return 0
    fi
    for dir in "${DATA_DIRS[@]}"; do
        if [[ -f "$dir/$ds.gfa" ]]; then
            echo "$dir/$ds.gfa"
            return 0
        fi
    done
    # The synthetic graph is not committed; it is regenerated (deterministic
    # seed) into the gitignored data directory.
    if [[ "$ds" == random_medium ]] && have python3; then
        mkdir -p "$BENCH_DIR/data"
        log "Generating random_medium.gfa" >&2
        python3 "$REPO_ROOT/utils/generate_gfa.py" --file "$BENCH_DIR/data/random_medium.gfa" >&2 || return 1
        echo "$BENCH_DIR/data/random_medium.gfa"
        return 0
    fi
    return 1
}

# dataset_name <name|path> -> label used in result file names.
dataset_name() { basename "${1%.gfa}"; }

# select_datasets "$@" -> sets DATASETS to the arguments, or the defaults.
select_datasets() {
    if [[ $# -gt 0 ]]; then
        DATASETS=("$@")
    else
        DATASETS=("${DEFAULT_DATASETS[@]}")
    fi
}

# skip_banner <dataset> — loud notice for a dataset that cannot be found.
skip_banner() {
    echo >&2
    echo "!!! SKIP dataset '$1': no '$1.gfa' in any of:" >&2
    local dir
    for dir in "${DATA_DIRS[@]}"; do echo "!!!   $dir" >&2; done
    echo "!!! (large graphs: run utils/download_gfa.sh)" >&2
}

dataset_banner() {
    echo
    echo "############################################################"
    echo "# Dataset: $1"
    echo "############################################################"
}

# --- Tools ------------------------------------------------------------------

# discover_tools — sets HAVE_VG / HAVE_ODGI, warning about missing tools.
discover_tools() {
    HAVE_VG=false
    HAVE_ODGI=false
    if have vg; then HAVE_VG=true; else warn "'vg' not found — skipping vg comparisons."; fi
    if have odgi; then HAVE_ODGI=true; else warn "'odgi' not found — skipping odgi comparisons."; fi
}

# ensure_pangraphx — sets PANGRAPHX to an executable CLI binary.
#
# PANGRAPHX_BIN selects a pre-built executable; otherwise the CLI is built from
# source with the fully optimized `dist` profile. SKIP_BUILD=1 reuses
# target/dist/pangraphx-cli as-is.
ensure_pangraphx() {
    if [[ -n "${PANGRAPHX_BIN:-}" ]]; then
        PANGRAPHX="$PANGRAPHX_BIN"
        [[ -x "$PANGRAPHX" ]] || die "PANGRAPHX_BIN is set but not executable: $PANGRAPHX"
        log "Using pre-built PanGraphX: $PANGRAPHX"
        return
    fi
    if [[ "${SKIP_BUILD:-0}" != 1 ]]; then
        have cargo || die "cargo not found; set PANGRAPHX_BIN to a pre-built binary."
        log "Building PanGraphX (dist)..."
        cargo build --profile dist --manifest-path "$REPO_ROOT/Cargo.toml"
    fi
    [[ -x "$PANGRAPHX" ]] || die "PanGraphX binary not found at $PANGRAPHX"
}

tool_version() {
    case "$1" in
        vg)         vg version 2>/dev/null | head -1 ;;
        odgi)       odgi version 2>/dev/null | head -1 ;;
        hyperfine)  hyperfine --version 2>/dev/null ;;
        rustc)      rustc --version 2>/dev/null ;;
        pangraphx)  "$PANGRAPHX" --version 2>/dev/null ;;
    esac
}

# --- Result directories -----------------------------------------------------

# init_run_dir <suite> — creates results/<suite>/<timestamp>-<host>/, records
# the environment in env.txt, points results/<suite>/latest at it, and sets
# RUN_DIR. RESULTS_DIR overrides the location entirely.
init_run_dir() {
    local suite="$1" host stamp
    host="$(hostname -s 2>/dev/null || hostname)"
    stamp="$(date +%Y%m%d-%H%M%S)"
    RUN_DIR="${RESULTS_DIR:-$RESULTS_ROOT/$suite/$stamp-$host}"
    mkdir -p "$RUN_DIR"
    ln -sfn "$RUN_DIR" "$RESULTS_ROOT/$suite/latest" 2>/dev/null || true
    : >"$RUN_DIR/failures.txt"
    write_env "$RUN_DIR/env.txt"
    log "Results: $RUN_DIR"
}

write_env() {
    local commit dirty="" t
    commit="$(git -C "$REPO_ROOT" rev-parse --short HEAD 2>/dev/null || echo unknown)"
    if ! git -C "$REPO_ROOT" diff --quiet HEAD 2>/dev/null; then dirty=" (dirty)"; fi
    {
        echo "date:      $(date '+%Y-%m-%d %H:%M:%S %z')"
        echo "host:      $(hostname)"
        echo "os:        $(uname -srm)"
        if [[ -r /proc/cpuinfo ]]; then
            echo "cpu:       $(awk -F': ' '/model name/ { print $2; exit }' /proc/cpuinfo)"
        else
            echo "cpu:       $(sysctl -n machdep.cpu.brand_string 2>/dev/null)"
        fi
        echo "cpus:      $(cpu_count)"
        echo "ram_mib:   $(total_ram_mib)"
        echo "commit:    $commit$dirty"
        echo "binary:    $PANGRAPHX"
        for t in pangraphx rustc hyperfine vg odgi; do
            printf '%-10s %s\n' "$t:" "$(tool_version "$t" || true)"
        done
        echo "env:       WARMUP=${WARMUP:-} RUNS=${RUNS:-} RAYON_NUM_THREADS=${RAYON_NUM_THREADS:-} VG_THREADS=${VG_THREADS:-} ODGI_THREADS=${ODGI_THREADS:-}"
    } >"$1"
}

# record_failure <label> <detail> — notes a failed step and keeps going.
record_failure() {
    echo "$1: $2" >>"$RUN_DIR/failures.txt"
    warn "FAILED $1 ($2) — continuing."
}

# finish_run — prints the result location and any failures.
finish_run() {
    echo
    if [[ -s "$RUN_DIR/failures.txt" ]]; then
        warn "$(wc -l <"$RUN_DIR/failures.txt" | tr -d ' ') step(s) failed, see $RUN_DIR/failures.txt:"
        sed 's/^/    /' "$RUN_DIR/failures.txt" >&2
    else
        rm -f "$RUN_DIR/failures.txt"
    fi
    log "Done. Results in: $RUN_DIR"
}

# --- Sampling ---------------------------------------------------------------

# sampling_for_size <bytes> — sets HF_WARMUP / HF_RUNS for hyperfine, fewer for
# larger inputs. WARMUP / RUNS env vars override the scaled values.
sampling_for_size() {
    local size="$1" mib=$(( $1 / 1024 / 1024 ))
    if   (( mib >= 256 )); then HF_WARMUP=0; HF_RUNS=3
    elif (( mib >= 64 ));  then HF_WARMUP=1; HF_RUNS=5
    elif (( mib >= 1 ));   then HF_WARMUP=2; HF_RUNS=10
    else                        HF_WARMUP=3; HF_RUNS=20
    fi
    : "$size"
    HF_WARMUP="${WARMUP:-$HF_WARMUP}"
    HF_RUNS="${RUNS:-$HF_RUNS}"
}

# --- Peak memory ------------------------------------------------------------

# Pick a `time` that reports peak RSS: GNU time (`-v`, KiB) or BSD time (`-l`,
# bytes, the macOS /usr/bin/time).
detect_time() {
    TIME_BIN=""; TIME_KIND=""
    local t
    for t in /usr/bin/time gtime; do
        if "$t" -v true >/dev/null 2>&1; then TIME_BIN="$t"; TIME_KIND=gnu; return; fi
    done
    if /usr/bin/time -l true >/dev/null 2>&1; then TIME_BIN=/usr/bin/time; TIME_KIND=bsd; return; fi
    die "no 'time' reporting peak RSS found (Linux: apt install time; macOS: built-in /usr/bin/time -l)."
}

# rss_tree_kib <pid> -> summed RSS (KiB) of pid's descendants.
rss_tree_kib() {
    local total=0 child rss
    for child in $(pgrep -P "$1" 2>/dev/null); do
        rss="$(ps -o rss= -p "$child" 2>/dev/null | tr -d ' ')"
        total=$(( total + ${rss:-0} + $(rss_tree_kib "$child") ))
    done
    echo "$total"
}

# measure <cmd> — runs <cmd> via `sh -c` and sets:
#   M_RSS_MIB  peak RSS in MiB (one decimal), or "NA"
#   M_WALL_S   wall-clock seconds (three decimals)
#   M_STATUS   ok | fail | oom-guard | timeout
#
# Optional guards (unset = unlimited):
#   GUARD_MEM_MIB  kill the command once its RSS exceeds this many MiB
#   GUARD_TIME_S   kill the command after this many seconds
# The watchdog polls every 0.2 s, so a very fast allocation burst can overshoot
# the memory limit briefly; keep the limit well below physical RAM.
measure() {
    local cmd="$1" err start end pid wd="" status=ok
    [[ -n "${TIME_BIN:-}" ]] || detect_time
    err="$(mktemp)"
    rm -f "$err.guard"

    start="$(now)"
    if [[ "$TIME_KIND" == gnu ]]; then
        "$TIME_BIN" -v sh -c "exec $cmd" >/dev/null 2>"$err" &
    else
        "$TIME_BIN" -l sh -c "exec $cmd" >/dev/null 2>"$err" &
    fi
    pid=$!

    if [[ -n "${GUARD_MEM_MIB:-}" || -n "${GUARD_TIME_S:-}" ]]; then
        (
            local limit_kib=$(( ${GUARD_MEM_MIB:-0} * 1024 )) t0 rss
            t0="$(now)"
            while kill -0 "$pid" 2>/dev/null; do
                rss="$(rss_tree_kib "$pid")"
                if (( limit_kib > 0 && rss > limit_kib )); then
                    echo oom-guard >"$err.guard"; pkill -KILL -P "$pid" || true; exit 0
                fi
                if [[ -n "${GUARD_TIME_S:-}" ]] &&
                    awk -v a="$(now)" -v b="$t0" -v l="$GUARD_TIME_S" 'BEGIN { exit !(a - b > l) }'; then
                    echo timeout >"$err.guard"; pkill -KILL -P "$pid" || true; exit 0
                fi
                sleep 0.2
            done
        ) &
        wd=$!
    fi

    if ! wait "$pid"; then status=fail; fi
    end="$(now)"
    if [[ -n "$wd" ]]; then kill "$wd" 2>/dev/null || true; wait "$wd" 2>/dev/null || true; fi
    if [[ -s "$err.guard" ]]; then status="$(cat "$err.guard")"; fi

    local kb=""
    if [[ "$TIME_KIND" == gnu ]]; then
        kb="$(grep -i 'Maximum resident set size' "$err" | grep -oE '[0-9]+' | tail -1 || true)"
    else
        kb="$(awk '/maximum resident set size/ { printf "%d", $1 / 1024 }' "$err")"
    fi
    M_RSS_MIB="NA"
    if [[ -n "$kb" ]]; then M_RSS_MIB="$(awk -v k="$kb" 'BEGIN { printf "%.1f", k / 1024 }')"; fi
    M_WALL_S="$(awk -v a="$end" -v b="$start" 'BEGIN { printf "%.3f", a - b }')"
    M_STATUS="$status"

    if [[ "$status" == fail ]]; then
        # Show the command's own error output (time's report is at the end).
        grep -v -iE '^\s*[0-9]+\s+[a-z]|^\s+[A-Z].*:|^Command (exited|terminated)' "$err" | tail -20 >&2 || true
    fi
    rm -f "$err" "$err.guard"
}

