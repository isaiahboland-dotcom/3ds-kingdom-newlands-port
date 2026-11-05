#!/usr/bin/env bash
# One-command install: installs what's missing (system packages, Rust,
# devkitPro), converts your copy of Kingdom: New Lands, builds the game and
# lays out the SD card in dist/sdcard/. Works on Debian/Ubuntu, Fedora and
# Arch based distros.
#
#   cp .example.env .env    # optional: set your game folder, paths, target
#   ./build.sh
#
#   ./build.sh desktop|3ds|cia   override TARGET from .env for this run
#   ./build.sh --rerip           rip the game's assets again
#   ./build.sh --help
#
# With no TARGET on the command line, in your shell or in .env, it asks
# whether to build the .3dsx (Homebrew Launcher) or the .cia (HOME Menu);
# without a terminal to ask on, it builds the .3dsx.
#
# Settings come from .env (see .example.env); variables already exported in
# your shell take precedence over it. The real work is done by tools/setup.sh
# (README, "Manual install").
set -euo pipefail

ROOT=$(cd "$(dirname "$0")" && pwd)
cd "$ROOT"

say() { printf '\033[1;32m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[1;33m==> %s\033[0m\n' "$*" >&2; }
die() { printf '\033[1;31mbuild: %s\033[0m\n' "$*" >&2; exit 1; }

# --- Settings ---------------------------------------------------------------

# Reads KEY=VALUE lines without running the file as a script, so unquoted
# paths with spaces work. Surrounding quotes are dropped and a leading ~,
# $HOME and $USER are expanded. Empty values and variables already set in
# the environment are left alone.
load_env() {
    local line key value
    while IFS= read -r line || [ -n "$line" ]; do
        line=${line%$'\r'}
        case "$line" in ''|'#'*) continue ;; esac
        line=${line#export }
        key=${line%%=*}
        value=${line#*=}
        key=${key//[[:space:]]/}
        [[ "$key" =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] || continue
        value=${value#"${value%%[![:space:]]*}"}
        value=${value%"${value##*[![:space:]]}"}
        case "$value" in
            \"*\") value=${value#\"}; value=${value%\"} ;;
            \'*\') value=${value#\'}; value=${value%\'} ;;
        esac
        # shellcheck disable=SC2088 # matching a literal ~ on purpose
        case "$value" in '~'|'~/'*) value=$HOME${value#\~} ;; esac
        value=${value//\$\{HOME\}/$HOME}
        value=${value//\$HOME/$HOME}
        value=${value//\$\{USER\}/${USER:-}}
        value=${value//\$USER/${USER:-}}
        [ -n "$value" ] || continue
        [ -n "${!key:-}" ] && continue
        export "$key=$value"
    done < "$1"
}

case "${1:-}" in -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;; esac

if [ -f "$ROOT/.env" ]; then
    load_env "$ROOT/.env"
else
    say "No .env found, using defaults (cp .example.env .env to change them)"
fi

TARGET=${TARGET:-}
INSTALL_DEPS=${INSTALL_DEPS:-yes}
SETUP_ARGS=()

while [ $# -gt 0 ]; do
    case "$1" in
        desktop|3ds|cia) TARGET=$1 ;;
        --rerip) SETUP_ARGS+=(--rerip) ;;
        -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown option $1 (see --help)" ;;
    esac
    shift
done

# No target chosen anywhere: ask which 3DS build to make.
ask_target() {
    local answer
    printf '\nWhich 3DS build do you want?\n'
    printf '  1) .3dsx  run from the Homebrew Launcher (default)\n'
    printf '  2) .cia   install to the HOME Menu with FBI (the .3dsx is built too)\n'
    while true; do
        read -r -p 'Choose 1 or 2 [1]: ' answer || answer=
        case "${answer,,}" in
            ''|1|3dsx|.3dsx|3ds) TARGET=3ds; return ;;
            2|cia|.cia) TARGET=cia; return ;;
            *) echo "Please answer 1 or 2." ;;
        esac
    done
}
if [ -z "$TARGET" ]; then
    if [ -t 0 ]; then
        ask_target
    else
        TARGET=3ds
        say "No TARGET set and no terminal to ask on: building the .3dsx"
    fi
fi

case "$TARGET" in
    desktop|3ds|cia) ;;
    *) die "TARGET must be desktop, 3ds or cia (got '$TARGET')" ;;
esac
case "$INSTALL_DEPS" in
    yes|true|1) INSTALL_DEPS=1 ;;
    no|false|0) INSTALL_DEPS=0 ;;
    *) die "INSTALL_DEPS must be yes or no (got '$INSTALL_DEPS')" ;;
esac

[ "$(id -u)" -ne 0 ] || die "run this as your normal user, not root; it asks for sudo when it needs it"

export DEVKITPRO=${DEVKITPRO:-/opt/devkitpro}
export DEVKITARM=${DEVKITARM:-$DEVKITPRO/devkitARM}
NEEDS_3DS=0
[ "$TARGET" = desktop ] || NEEDS_3DS=1

# --- System packages --------------------------------------------------------

distro_family() {
    local id="" like=""
    if [ -r /etc/os-release ]; then
        # shellcheck disable=SC1091
        id=$(. /etc/os-release && echo "${ID:-}")
        # shellcheck disable=SC1091
        like=$(. /etc/os-release && echo "${ID_LIKE:-}")
    fi
    for word in $id $like; do
        case "$word" in
            debian|ubuntu) echo debian; return ;;
            fedora|rhel|centos) echo fedora; return ;;
            arch|archlinux) echo arch; return ;;
        esac
    done
    echo unknown
}

FAMILY=$(distro_family)

sudo_run() {
    if command -v sudo >/dev/null; then sudo "$@"; else die "sudo not found; install it or set INSTALL_DEPS=no"; fi
}

install_packages() {
    local missing=() p
    case "$FAMILY" in
        debian)
            for p in build-essential pkg-config libasound2-dev libx11-dev libxi-dev libgl1-mesa-dev \
                curl wget ca-certificates xz-utils git clang libclang-dev libicu-dev; do
                dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q 'install ok installed' || missing+=("$p")
            done
            [ ${#missing[@]} -eq 0 ] && return
            say "Installing system packages: ${missing[*]}"
            sudo_run apt-get update
            sudo_run apt-get install -y "${missing[@]}"
            ;;
        fedora)
            for p in gcc gcc-c++ make pkgconf-pkg-config alsa-lib-devel libX11-devel libXi-devel \
                mesa-libGL-devel curl tar xz git clang clang-devel libicu; do
                rpm -q "$p" >/dev/null 2>&1 || missing+=("$p")
            done
            [ ${#missing[@]} -eq 0 ] && return
            say "Installing system packages: ${missing[*]}"
            sudo_run dnf install -y "${missing[@]}"
            ;;
        arch)
            mapfile -t missing < <(pacman -T base-devel pkgconf alsa-lib libx11 libxi mesa curl xz git clang icu || true)
            [ ${#missing[@]} -eq 0 ] && return
            say "Installing system packages: ${missing[*]}"
            sudo_run pacman -S --needed --noconfirm "${missing[@]}"
            ;;
        *)
            die "unrecognised distro (not Debian, Fedora or Arch based); install the dependencies yourself (README, \"Manual install\") and set INSTALL_DEPS=no"
            ;;
    esac
}

# --- Rust -------------------------------------------------------------------

install_rust() {
    # shellcheck disable=SC1091
    [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
    if ! command -v cargo >/dev/null; then
        [ "$INSTALL_DEPS" = 1 ] || die "cargo not found: install Rust from https://rustup.rs"
        say "Installing Rust (rustup)"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
        # shellcheck disable=SC1091
        . "$HOME/.cargo/env"
    fi
    [ "$NEEDS_3DS" = 1 ] || return 0
    command -v rustup >/dev/null ||
        die "the 3DS build needs rustup (your Rust comes from elsewhere, e.g. the distro); see https://rustup.rs"
    if ! rustup +nightly component list --installed 2>/dev/null | grep -q '^rust-src'; then
        [ "$INSTALL_DEPS" = 1 ] || die "the 3DS build needs nightly Rust with rust-src (rustup toolchain install nightly --component rust-src)"
        say "Installing nightly Rust with rust-src"
        rustup toolchain install nightly --profile minimal --component rust-src
    fi
    if ! command -v cargo-3ds >/dev/null; then
        [ "$INSTALL_DEPS" = 1 ] || die "cargo-3ds not found (cargo install --locked cargo-3ds)"
        say "Installing cargo-3ds"
        cargo install --locked cargo-3ds
    fi
}

# --- devkitPro --------------------------------------------------------------

DKP_KEY=BC26F752D25B92CE272E0F44F7FD5492264BB9D0

# Arch (and Fedora, which packages pacman) use devkitPro's pacman repos.
add_dkp_repos() {
    if grep -q '^\[dkp-libs\]' /etc/pacman.conf 2>/dev/null; then
        return
    fi
    say "Adding devkitPro's package repositories to /etc/pacman.conf"
    sudo_run pacman-key --init
    sudo_run pacman-key --recv "$DKP_KEY" --keyserver keyserver.ubuntu.com
    sudo_run pacman-key --lsign "$DKP_KEY"
    sudo_run pacman -U --noconfirm https://pkg.devkitpro.org/devkitpro-keyring.pkg.tar.zst
    # shellcheck disable=SC2016 # $arch is pacman's, not ours
    printf '\n[dkp-libs]\nServer = https://pkg.devkitpro.org/packages\n\n[dkp-linux]\nServer = https://pkg.devkitpro.org/packages/linux/$arch/\n' |
        sudo_run tee -a /etc/pacman.conf >/dev/null
}

devkitpro_ok() {
    [ -x "$DEVKITPRO/tools/bin/tex3ds" ] && [ -x "$DEVKITARM/bin/arm-none-eabi-gcc" ] && [ -d "$DEVKITPRO/libctru" ]
}

install_devkitpro() {
    devkitpro_ok && return
    [ "$INSTALL_DEPS" = 1 ] ||
        die "devkitPro (3ds-dev) not found at $DEVKITPRO: install it (https://devkitpro.org/wiki/Getting_Started) or set DEVKITPRO in .env"
    [ "$DEVKITPRO" = /opt/devkitpro ] ||
        die "devkitPro not found at $DEVKITPRO; it can only be installed automatically into /opt/devkitpro"
    local dkp_pacman
    case "$FAMILY" in
        debian)
            if ! command -v dkp-pacman >/dev/null; then
                say "Installing devkitPro pacman"
                local tmp
                tmp=$(mktemp -d)
                # devkitPro's server turns away some download tools; wget is what its guide uses.
                wget -q -O "$tmp/install-devkitpro-pacman" https://apt.devkitpro.org/install-devkitpro-pacman ||
                    die "couldn't download devkitPro's installer; follow https://devkitpro.org/wiki/devkitPro_pacman"
                chmod +x "$tmp/install-devkitpro-pacman"
                sudo_run "$tmp/install-devkitpro-pacman"
                rm -rf "$tmp"
            fi
            dkp_pacman=dkp-pacman
            ;;
        fedora)
            command -v pacman >/dev/null || { say "Installing pacman (for devkitPro)"; sudo_run dnf install -y pacman; }
            add_dkp_repos
            dkp_pacman=pacman
            ;;
        arch)
            add_dkp_repos
            dkp_pacman=pacman
            ;;
        *)
            die "install devkitPro yourself: https://devkitpro.org/wiki/Getting_Started"
            ;;
    esac
    say "Installing devkitPro's 3ds-dev tools"
    sudo_run "$dkp_pacman" -Sy --needed --noconfirm 3ds-dev
    devkitpro_ok || die "devkitPro installed but tex3ds/devkitARM/libctru are still missing under $DEVKITPRO"
}

# --- Run --------------------------------------------------------------------

say "Target: $TARGET ($FAMILY)"
[ "$INSTALL_DEPS" = 1 ] && install_packages
install_rust
[ "$NEEDS_3DS" = 1 ] && install_devkitpro

case "$TARGET" in
    3ds) SETUP_ARGS+=(--3ds) ;;
    cia) SETUP_ARGS+=(--cia) ;;
esac
if [ -n "${KINGDOM_GAME:-}" ]; then
    [ -d "$KINGDOM_GAME" ] || die "KINGDOM_GAME=$KINGDOM_GAME isn't a folder"
    SETUP_ARGS+=(--game "$KINGDOM_GAME")
fi

"$ROOT/tools/setup.sh" "${SETUP_ARGS[@]}"

if [ "$TARGET" = desktop ]; then
    say "Building the desktop version"
    cargo build --release --locked -p desktop
    say "Done. Play with: cargo run --release -p desktop (or target/release/desktop)"
    exit 0
fi

# Lay out the SD card: everything in dist/sdcard goes onto the card's root.
OUT="$ROOT/crates/n3ds/target/armv6k-nintendo-3ds/release"
DIST="$ROOT/dist/sdcard"
say "Preparing the SD card files in dist/sdcard/"
rm -rf "$DIST"
mkdir -p "$DIST/3ds"
cp -r "$ROOT/crates/n3ds/sdcard/3ds/." "$DIST/3ds/"
cp "$OUT/kingdom.3dsx" "$DIST/3ds/"
if [ "$TARGET" = cia ]; then
    mkdir -p "$DIST/cias"
    cp "$OUT/kingdom.cia" "$DIST/cias/"
fi

if [ -n "${SDCARD:-}" ]; then
    [ -d "$SDCARD" ] && [ -w "$SDCARD" ] || die "SDCARD=$SDCARD isn't a writable folder (is the card mounted?)"
    say "Copying to the SD card at $SDCARD"
    # Merges into the card: existing saves in /3ds/kingdom-3ds/ are kept.
    cp -r "$DIST/." "$SDCARD/"
    sync
    [ -f "$SDCARD/3ds/dspfirm.cdc" ] ||
        warn "No /3ds/dspfirm.cdc on the card: run DSP1 on the console once, or the game has no sound"
    say "Done. Eject the card, then launch Kingdom from the Homebrew Launcher"
else
    say "Done. Copy everything inside dist/sdcard/ onto the SD card's root (merge folders)"
fi
[ "$TARGET" = cia ] && echo "  then install /cias/kingdom.cia with FBI"
echo "  Sound also needs /3ds/dspfirm.cdc on the card (dump it once with DSP1)."
