#!/usr/bin/env bash
#
# install_external_tools.sh — installs the pinned reference tools vg and odgi,
# used by the cross-validation tests (CI) and the benchmarks
# (benchmarks/setup_ubuntu.sh sources this file).
#
# Usage:
#   utils/install_external_tools.sh [PREFIX]
#
#   PREFIX   install prefix; binaries go to PREFIX/bin (default /usr/local).
#            sudo is used only when PREFIX/bin is not writable.
#
# vg is the static x86_64 release binary; odgi is built from source (needs
# cmake, a C++ toolchain, zlib and jemalloc). A tool already at the pinned
# version in PREFIX/bin is left alone.
#
# Environment:
#   VG_VERSION     vg release to install (default v1.73.0, x86_64 only)
#   ODGI_REF       odgi git tag or commit to build (default fb43f08, v0.9.4-92)
#   SRC_DIR        where odgi is cloned and built (default ~/.cache/pangraphx-setup)
#   JOBS           parallel compile jobs for odgi (default: cores, capped by RAM)

# shellcheck source=../benchmarks/lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/benchmarks/lib.sh"

# The versions the thesis results were measured with.
VG_VERSION="${VG_VERSION:-v1.73.0}"
ODGI_REF="${ODGI_REF:-fb43f0817d8f3b81ce854f111d6daed8c4c8cbf4}"
PREFIX="${PREFIX:-/usr/local}"
SRC_DIR="${SRC_DIR:-$HOME/.cache/pangraphx-setup}"

# install_bin <file> <name> — installs an executable into PREFIX/bin.
install_bin() {
    local sudo=""
    mkdir -p "$PREFIX/bin" 2>/dev/null || true
    if [[ ! -w "$PREFIX/bin" ]]; then
        sudo="sudo"
        $sudo mkdir -p "$PREFIX/bin"
    fi
    $sudo install -m 755 "$1" "$PREFIX/bin/$2"
}

install_vg() {
    local vg="$PREFIX/bin/vg"
    if [[ -x "$vg" ]] && "$vg" version 2>/dev/null | head -1 | grep -qF "$VG_VERSION"; then
        log "vg $VG_VERSION already installed"
        return
    fi
    if [[ "$(uname -m)" != x86_64 ]]; then
        warn "vg releases are x86_64 only; skipping vg (its comparisons will be skipped)."
        return
    fi
    log "Installing vg $VG_VERSION to $PREFIX/bin..."
    local tmp
    tmp="$(mktemp)"
    curl -L --fail -o "$tmp" "https://github.com/vgteam/vg/releases/download/$VG_VERSION/vg"
    install_bin "$tmp" vg
    rm -f "$tmp"
    log "$("$vg" version | head -1)"
}

install_odgi() {
    local odgi="$PREFIX/bin/odgi" short="${ODGI_REF:0:7}"
    if [[ -x "$odgi" ]] && "$odgi" version 2>/dev/null | grep -qF "$short"; then
        log "odgi $short already installed"
        return
    fi
    local src="$SRC_DIR/odgi" jobs
    # odgi's heaviest translation units need ~2 GiB each.
    jobs="${JOBS:-$(cpu_count)}"
    if [[ -z "${JOBS:-}" ]] && (( jobs > $(total_ram_mib) / 2048 )); then
        jobs=$(( $(total_ram_mib) / 2048 ))
        (( jobs >= 1 )) || jobs=1
    fi

    log "Building odgi $ODGI_REF in $src ($jobs jobs)..."
    mkdir -p "$SRC_DIR"
    if [[ ! -d "$src/.git" ]]; then
        git clone https://github.com/pangenome/odgi.git "$src"
    fi
    git -C "$src" fetch --tags origin
    git -C "$src" checkout --quiet "$ODGI_REF"
    git -C "$src" submodule update --init --recursive
    cmake -S "$src" -B "$src/build" -DCMAKE_BUILD_TYPE=Release
    cmake --build "$src/build" -- -j "$jobs"
    # odgi's CMakeLists puts the executable in <source>/bin, not the build dir.
    install_bin "$src/bin/odgi" odgi
    log "$("$odgi" version | head -1)"
}

# Run directly (not sourced): install both.
if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
    set -euo pipefail
    case "${1:-}" in
        -h|--help) sed -n '3,/^$/s/^# \{0,1\}//p' "$0"; exit 0 ;;
        "") ;;
        *) PREFIX="$1" ;;
    esac
    install_vg
    install_odgi
fi
