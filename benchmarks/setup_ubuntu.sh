#!/usr/bin/env bash
#
# setup_ubuntu.sh — prepares a fresh Ubuntu machine for the shell benchmarks
# (compare_tools.sh, dbg.sh): system packages, Rust, hyperfine >= 2, vg, odgi,
# the `dist` PanGraphX binary and the large benchmark graphs.
#
# Usage:
#   benchmarks/setup_ubuntu.sh [--skip-data] [--skip-build]
#
#   --skip-data   do not download the large GFA graphs (utils/download_gfa.sh)
#   --skip-build  do not build target/dist/pangraphx-cli
#
# Safe to re-run: every step is skipped when its result is already present.
# vg and odgi are pinned to the versions the thesis results were measured with.
#
# Environment:
#   VG_VERSION     vg release to install (default v1.73.0, x86_64 only)
#   ODGI_REF       odgi git tag or commit to build (default fb43f08, v0.9.4-92)
#   PREFIX         install prefix for vg / odgi (default /usr/local)
#   SRC_DIR        where odgi is cloned and built (default ~/.cache/pangraphx-setup)
#   JOBS           parallel compile jobs for odgi (default: cores, capped by RAM)

set -euo pipefail
# shellcheck source=lib.sh
source "$(dirname "${BASH_SOURCE[0]}")/lib.sh"

VG_VERSION="${VG_VERSION:-v1.73.0}"
ODGI_REF="${ODGI_REF:-fb43f0817d8f3b81ce854f111d6daed8c4c8cbf4}"
PREFIX="${PREFIX:-/usr/local}"
SRC_DIR="${SRC_DIR:-$HOME/.cache/pangraphx-setup}"
MIN_RUST="1.85" # edition 2024
MIN_FREE_GIB=20 # graphs (~1 GiB) + vg/odgi intermediates + target/

SKIP_DATA=false
SKIP_BUILD=false
for arg in "$@"; do
    case "$arg" in
        --skip-data)  SKIP_DATA=true ;;
        --skip-build) SKIP_BUILD=true ;;
        -h|--help)    sed -n '3,/^$/s/^# \{0,1\}//p' "$0"; exit 0 ;;
        *)            die "unknown option: $arg (see --help)" ;;
    esac
done

SUDO=""
if [[ "$(id -u)" -ne 0 ]]; then
    have sudo || die "run as root or install sudo."
    SUDO="sudo"
fi

# version_ge <a> <b> — true when version a >= b.
version_ge() { [[ "$(printf '%s\n%s\n' "$2" "$1" | sort -V | head -1)" == "$2" ]]; }

check_system() {
    if [[ -r /etc/os-release ]] && grep -q '^ID=ubuntu' /etc/os-release; then
        log "$(. /etc/os-release && echo "$PRETTY_NAME"), $(uname -m)"
    else
        warn "this script targets Ubuntu; continuing anyway."
    fi
    local free_gib
    free_gib=$(( $(df -Pk "$REPO_ROOT" | awk 'NR == 2 { print $4 }') / 1024 / 1024 ))
    if (( free_gib < MIN_FREE_GIB )); then
        warn "only ${free_gib} GiB free in $REPO_ROOT; the benchmarks need about ${MIN_FREE_GIB} GiB."
    fi
}

install_packages() {
    log "Installing system packages..."
    # build-essential/cmake/python3-*/libjemalloc-dev/zlib1g-dev: odgi;
    # protobuf-compiler: prost-build (VG format); time: peak RSS in dbg.sh;
    # zstd/unzip: utils/download_gfa.sh; perl: timestamps in lib.sh.
    $SUDO apt-get update
    DEBIAN_FRONTEND=noninteractive $SUDO apt-get install -y --no-install-recommends \
        build-essential cmake git curl ca-certificates pkg-config \
        python3 python3-dev python3-setuptools libjemalloc-dev zlib1g-dev protobuf-compiler \
        time perl zstd unzip gzip
}

install_rust() {
    if [[ -f "$HOME/.cargo/env" ]]; then
        # shellcheck source=/dev/null
        source "$HOME/.cargo/env"
    fi
    if ! have cargo; then
        log "Installing Rust (rustup, stable)..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
            sh -s -- -y --profile minimal --default-toolchain stable
        # shellcheck source=/dev/null
        source "$HOME/.cargo/env"
    fi
    local version
    version="$(rustc --version | awk '{ print $2 }')"
    if ! version_ge "$version" "$MIN_RUST"; then
        have rustup || die "rustc $version is older than $MIN_RUST and rustup is missing."
        log "Updating Rust ($version < $MIN_RUST)..."
        rustup update stable
        rustup default stable
    fi
    log "$(rustc --version)"
}

install_hyperfine() {
    # Ubuntu's apt package is hyperfine 1.x, which does not record peak RSS.
    local version=""
    if have hyperfine; then version="$(hyperfine --version | awk '{ print $2 }')"; fi
    if [[ -n "$version" ]] && version_ge "$version" 2; then
        log "hyperfine $version already installed"
        return
    fi
    log "Installing hyperfine >= 2 (cargo)..."
    cargo install --locked hyperfine
    log "$(hyperfine --version)"
}

install_vg() {
    if have vg && vg version 2>/dev/null | head -1 | grep -qF "$VG_VERSION"; then
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
    $SUDO install -m 755 "$tmp" "$PREFIX/bin/vg"
    rm -f "$tmp"
    log "$(vg version | head -1)"
}

install_odgi() {
    local short="${ODGI_REF:0:7}"
    if have odgi && odgi version 2>/dev/null | grep -qF "$short"; then
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
    $SUDO install -m 755 "$src/bin/odgi" "$PREFIX/bin/odgi"
    log "$(odgi version | head -1)"
}

build_pangraphx() {
    log "Building PanGraphX (dist)..."
    cargo build --profile dist --manifest-path "$REPO_ROOT/Cargo.toml"
    log "$("$PANGRAPHX" --version)"
}

check_system
install_packages
install_rust
install_hyperfine
install_vg
install_odgi
if ! $SKIP_BUILD; then build_pangraphx; fi
if ! $SKIP_DATA; then "$REPO_ROOT/utils/download_gfa.sh"; fi

echo
log "Setup complete. Open a new shell (or 'source ~/.cargo/env'), then e.g.:"
echo "    SKIP_BUILD=1 benchmarks/compare_tools.sh"
echo "    SKIP_BUILD=1 benchmarks/dbg.sh ecoli50"
