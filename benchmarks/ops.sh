# shellcheck shell=bash
#
# ops.sh — the conversion operations compared against vg / odgi. Sourced after
# lib.sh by compare_tools.sh, which measures time and peak memory of exactly the
# same commands on exactly the same inputs.
#
# Every operation is either a forward conversion (GFA -> X) reading the dataset
# GFA, or a reverse conversion (X -> GFA) reading the X file produced by the
# *reference* tool (vg/odgi, not PanGraphX) in prepare_inputs. Outputs go to
# $WORK/<dataset>/ and are overwritten on each run.
#
# odgi gets `-t $ODGI_THREADS` (all cores; without -t it runs on one thread).
# `vg convert` gets no -t: it already defaults to all cores, and GFA <-> VG
# conversion is single-threaded anyway. GBZ construction uses all cores via
# `vg gbwt --num-jobs` (VG_THREADS overrides; vg gbwt has no -t/--threads). VG files
# are written as Protobuf (--vg-out), the encoding PanGraphX reads and writes,
# so both tools produce the same format. GBZ construction passes --max-node 0:
# vg otherwise chops segments to 1024 bp, which PanGraphX does not do. GFA
# import passes -r -1: vg otherwise turns rank-0 rGFA tags (SN/SO/SR) into
# paths, which PanGraphX and odgi ignore. Inputs without rGFA tags are unaffected.

VG_THREADS="${VG_THREADS:-$(cpu_count)}"
ODGI_THREADS="${ODGI_THREADS:-$(cpu_count)}"

ALL_OPS=(gfa_to_vg vg_to_gfa gfa_to_gbz gbz_to_gfa gfa_to_og og_to_gfa)

# op_tool <op> -> name of the reference tool.
op_tool() {
    case "$1" in
        *og*) echo odgi ;;
        *)    echo vg ;;
    esac
}

# op_title <op> -> human-readable title, e.g. "GFA -> VG".
op_title() {
    local from="${1%%_to_*}" to="${1##*_to_}"
    echo "$(echo "$from" | tr '[:lower:]' '[:upper:]') -> $(echo "$to" | tr '[:lower:]' '[:upper:]')"
}

# op_available <op> -> success when the reference tool is installed.
op_available() {
    case "$(op_tool "$1")" in
        vg)   $HAVE_VG ;;
        odgi) $HAVE_ODGI ;;
    esac
}

# op_input <op> <dataset-workdir> <gfa> -> input file of the operation.
op_input() {
    local op="$1" dir="$2" gfa="$3" from="${1%%_to_*}"
    if [[ "$from" == gfa ]]; then echo "$gfa"; else echo "$dir/ref.$from"; fi
    : "$op"
}

# op_px_cmd <op> <dataset-workdir> <gfa> -> PanGraphX command line.
op_px_cmd() {
    local op="$1" dir="$2" in to="${1##*_to_}"
    in="$(op_input "$op" "$dir" "$3")"
    echo "$PANGRAPHX convert $in -o $dir/px_out.$to"
}

# op_tool_cmd <op> <dataset-workdir> <gfa> -> reference tool command line.
op_tool_cmd() {
    local op="$1" dir="$2" in
    in="$(op_input "$op" "$dir" "$3")"
    case "$op" in
        gfa_to_vg)  echo "vg convert --gfa-in -r -1 --vg-out $in > $dir/tool_out.vg" ;;
        vg_to_gfa)  echo "vg convert --gfa-out $in > $dir/tool_out.gfa" ;;
        gfa_to_gbz) echo "vg gbwt -G $in -g $dir/tool_out.gbz --max-node 0 --num-jobs $VG_THREADS" ;;
        gbz_to_gfa) echo "vg convert --gfa-out $in > $dir/tool_out.gfa" ;;
        gfa_to_og)  echo "odgi build -t $ODGI_THREADS -g $in -o $dir/tool_out.og" ;;
        og_to_gfa)  echo "odgi view -t $ODGI_THREADS -i $in -g > $dir/tool_out.gfa" ;;
    esac
}

# prepare_inputs <dataset-workdir> <gfa> — builds the reverse-conversion inputs
# (ref.vg, ref.gbz, ref.og) once with the reference tools, untimed. Existing
# files are reused, so timing and memory runs sharing a WORK dir see the same
# inputs. A failed build is recorded and its reverse op is skipped later.
prepare_inputs() {
    local dir="$1" gfa="$2" fmt cmd
    mkdir -p "$dir"
    for fmt in vg gbz og; do
        [[ -s "$dir/ref.$fmt" ]] && continue
        case "$fmt" in
            vg)  $HAVE_VG   || continue; cmd="vg convert --gfa-in -r -1 --vg-out $gfa > $dir/ref.vg" ;;
            gbz) $HAVE_VG   || continue; cmd="vg gbwt -G $gfa -g $dir/ref.gbz --max-node 0 --num-jobs $VG_THREADS" ;;
            og)  $HAVE_ODGI || continue; cmd="odgi build -t $ODGI_THREADS -g $gfa -o $dir/ref.og" ;;
        esac
        log "Preparing reference input ref.$fmt"
        if ! sh -c "$cmd" >/dev/null; then
            rm -f "$dir/ref.$fmt"
            record_failure "prepare $(basename "$dir") ref.$fmt" "$cmd"
        fi
    done
}

# op_ready <op> <dataset-workdir> <gfa> -> success when the op can run (tool
# installed and input present).
op_ready() {
    op_available "$1" && [[ -s "$(op_input "$1" "$2" "$3")" ]]
}
