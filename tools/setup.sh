#!/usr/bin/env bash
# Sets everything up from your own copy of Kingdom: New Lands: finds the
# game, rips its assets with AssetRipper (downloaded once, pinned and
# checksummed), runs the extract tool, and optionally builds the 3DS game.
# Nothing it makes is ever committed (reference/, assets/ and the romfs are
# gitignored).
#
#   tools/setup.sh                      desktop: rip + extract into assets/
#   tools/setup.sh --3ds                ...then pack the romfs and build kingdom.3dsx
#   tools/setup.sh --cia                ...and kingdom.cia as well
#   tools/setup.sh --game "/path/to/Kingdom New Lands"
#
# Options:
#   --game DIR    the game's folder (the one with Kingdom_Data in it); found
#                 in the usual Steam libraries when left out ($KINGDOM_GAME works too)
#   --3ds         also run pack3ds and build the .3dsx
#   --cia         also build the .cia (implies --3ds)
#   --rerip       rip again even if reference/ripped already exists
#   --decomp      also decompile the game's scripts into reference/decomp
#                 (for working on the port; needs ilspycmd)
#
# Environment: ASSETRIPPER (an AssetRipper.GUI.Free you already have, used
# instead of downloading one), DEVKITPRO (default /opt/devkitpro).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
RIPPED="$ROOT/reference/ripped"
ASSETS="$ROOT/assets"

# The AssetRipper release the extract tool is written against, with each
# download's SHA-256 (from the release page). (A case, not an associative
# array: macOS still ships bash 3.)
AR_VERSION=2.0.0
ar_sha256() {
    case "$1" in
        linux_x64.tar.xz) echo 6f5d4352d36795b944243eb3ec790ec37dbca6d6756e52605bc8616e042d384e ;;
        linux_arm64.tar.xz) echo 8031752621d7fb27addab6108bad8bfbd43546bbb07b36912a987dfb4029c914 ;;
        mac_x64.tar.xz) echo 1b3836f135d16845cb57595fadff5739cbb86d28caaf621005b4301dba9b5a56 ;;
        mac_arm64.tar.xz) echo 237a6da404bf089c5e5c71a5a9a698c2ea580872292dfa5ce21e25fd1dbe9045 ;;
        win_x64.zip) echo 9a7ef0e7c5c3ea5b90b4e6d855e2d98d5f7ec8c3f9e26fccbc194c6a7b01baf7 ;;
        win_arm64.zip) echo c2c3a335948aa0ecaf26baf22ac77e36993d1a1695b5db291cb63f66872627f1 ;;
    esac
}
# The world.bin the port is developed against (Steam, Linux build). Another
# build of the game may differ a little; it still works, but saves made
# with one don't load with the other.
KNOWN_WORLD_SHA1=4f551dcf938c0e54ab39cb0b21a03db3e93f22cc

GAME=${KINGDOM_GAME:-}
DO_3DS=0
DO_CIA=0
RERIP=0
DECOMP=0

say() { printf '\033[1m==> %s\033[0m\n' "$*"; }
die() { printf 'setup: %s\n' "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --game) GAME=${2:?--game needs a folder}; shift ;;
        --3ds) DO_3DS=1 ;;
        --cia) DO_3DS=1; DO_CIA=1 ;;
        --rerip) RERIP=1 ;;
        --decomp) DECOMP=1 ;;
        -h|--help) sed -n '2,/^set -euo/p' "$0" | sed '$d; s/^# \{0,1\}//'; exit 0 ;;
        *) die "unknown option $1 (see --help)" ;;
    esac
    shift
done

command -v cargo >/dev/null || die "cargo not found: install Rust from https://rustup.rs"
command -v curl >/dev/null || die "curl not found"

# --- The game ---------------------------------------------------------------

# The Unity data folder inside a game folder (Kingdom_Data on Linux and
# Windows, Contents/Resources/Data in the macOS app).
data_dir() {
    local d
    for d in "$1"/*_Data "$1"/*.app/Contents/Resources/Data "$1/Contents/Resources/Data" "$1"; do
        if [ -f "$d/globalgamemanagers" ]; then
            printf '%s\n' "$d"
            return 0
        fi
    done
    return 1
}

steam_libraries() {
    local roots=(
        "$HOME/.steam/steam"
        "$HOME/.local/share/Steam"
        "$HOME/.var/app/com.valvesoftware.Steam/.local/share/Steam"
        "$HOME/Library/Application Support/Steam"
        "/c/Program Files (x86)/Steam"
        "/mnt/c/Program Files (x86)/Steam"
    )
    local r
    for r in "${roots[@]}"; do
        [ -d "$r/steamapps" ] || continue
        printf '%s\n' "$r"
        # Other libraries the user added.
        if [ -f "$r/steamapps/libraryfolders.vdf" ]; then
            sed -n 's/^[[:space:]]*"path"[[:space:]]*"\(.*\)"/\1/p' "$r/steamapps/libraryfolders.vdf"
        fi
    done
}

find_game() {
    local lib
    while IFS= read -r lib; do
        if [ -d "$lib/steamapps/common/Kingdom New Lands" ]; then
            printf '%s\n' "$lib/steamapps/common/Kingdom New Lands"
            return 0
        fi
    done < <(steam_libraries | awk '!seen[$0]++')
    local d
    for d in "$HOME/GOG Games/Kingdom New Lands" "$HOME/Games/kingdom-new-lands" "$ROOT/gamefiles/Kingdom New Lands"; do
        if [ -d "$d" ]; then
            printf '%s\n' "$d"
            return 0
        fi
    done
    return 1
}

need_game() {
    if [ -z "$GAME" ]; then
        GAME=$(find_game) || die "couldn't find Kingdom: New Lands; pass its folder with --game"
    fi
    DATA=$(data_dir "$GAME") || die "$GAME doesn't look like Kingdom: New Lands (no Kingdom_Data/globalgamemanagers)"
    say "Game: $DATA"
}

# --- AssetRipper ------------------------------------------------------------

platform() {
    local os arch
    case "$(uname -s)" in
        Linux) os=linux ;;
        Darwin) os=mac ;;
        MINGW*|MSYS*|CYGWIN*) os=win ;;
        *) die "unsupported system $(uname -s); set ASSETRIPPER to an AssetRipper.GUI.Free you have" ;;
    esac
    case "$(uname -m)" in
        x86_64|amd64) arch=x64 ;;
        aarch64|arm64) arch=arm64 ;;
        *) die "unsupported CPU $(uname -m); set ASSETRIPPER to an AssetRipper.GUI.Free you have" ;;
    esac
    printf '%s_%s\n' "$os" "$arch"
}

sha256() {
    if command -v sha256sum >/dev/null; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi
}

need_assetripper() {
    if [ -n "${ASSETRIPPER:-}" ]; then
        [ -x "$ASSETRIPPER" ] || die "ASSETRIPPER=$ASSETRIPPER isn't an executable"
        AR=$ASSETRIPPER
        return
    fi
    local p file dir
    p=$(platform)
    case "$p" in win_*) file="$p.zip" ;; *) file="$p.tar.xz" ;; esac
    dir="$ROOT/tools/assetripper/$AR_VERSION/$p"
    AR="$dir/AssetRipper.GUI.Free"
    [ "${p%%_*}" = win ] && AR="$AR.exe"
    [ -x "$AR" ] && return
    say "Downloading AssetRipper $AR_VERSION ($p)"
    mkdir -p "$dir"
    local archive="$dir/AssetRipper_$file"
    curl -fL --progress-bar -o "$archive" \
        "https://github.com/AssetRipper/AssetRipper/releases/download/$AR_VERSION/AssetRipper_$file"
    local got
    got=$(sha256 "$archive")
    if [ "$got" != "$(ar_sha256 "$file")" ]; then
        rm -f "$archive"
        die "AssetRipper download failed its checksum (got $got)"
    fi
    case "$file" in
        *.zip) unzip -q -o "$archive" -d "$dir" ;;
        *) tar -xJf "$archive" -C "$dir" ;;
    esac
    rm -f "$archive"
    chmod +x "$AR"
}

AR_PID=
stop_assetripper() {
    if [ -n "$AR_PID" ]; then
        kill "$AR_PID" 2>/dev/null || true
        wait "$AR_PID" 2>/dev/null || true
        AR_PID=
    fi
}
trap stop_assetripper EXIT

# AssetRipper runs as a local web app; this drives it headless through the
# same forms its pages post.
rip() {
    need_game
    need_assetripper
    local port url tries
    port=$((20000 + RANDOM % 20000))
    url="http://127.0.0.1:$port"
    mkdir -p "$ROOT/reference"
    say "Ripping the game's assets with AssetRipper (a minute or so)"
    "$AR" --headless --port "$port" --log-path "$ROOT/reference/assetripper.log" >"$ROOT/reference/assetripper.out" 2>&1 &
    AR_PID=$!
    tries=0
    until curl -fs -o /dev/null "$url/"; do
        kill -0 "$AR_PID" 2>/dev/null || die "AssetRipper didn't start (see reference/assetripper.out)"
        tries=$((tries + 1))
        [ "$tries" -lt 120 ] || die "AssetRipper didn't answer on $url"
        sleep 0.5
    done
    # The settings the extract tool expects (AssetRipper's defaults, set in
    # case a saved configuration says otherwise). An unchecked box is left out.
    curl -fs -o /dev/null "$url/Settings/Update" \
        -d BundledAssetsExportMode=DirectExport -d ScriptContentLevel=Level2 \
        -d DefaultVersion=0.0.0a0 -d TargetVersion=0.0.0a0 \
        -d AudioExportFormat=Default -d ImageExportFormat=Png \
        -d LightmapTextureExportFormat=Yaml -d SpriteExportMode=Yaml \
        -d ShaderExportMode=Dummy -d TextExportMode=Parse \
        -d ScriptLanguageVersion=AutoSafe -d ScriptExportMode=Hybrid \
        -d PreferOriginalTextureExtension= ||
        die "AssetRipper refused its settings"
    curl -fs -o /dev/null "$url/LoadFolder" --data-urlencode "Path=$DATA" ||
        die "AssetRipper couldn't load $DATA"
    local tmp="$ROOT/reference/ripped.partial"
    rm -rf "$tmp"
    curl -fs -o /dev/null --max-time 3600 "$url/Export/UnityProject" --data-urlencode "Path=$tmp" ||
        die "AssetRipper's export failed (see reference/assetripper.log)"
    stop_assetripper
    [ -d "$tmp/ExportedProject/Assets" ] || die "AssetRipper exported nothing (see reference/assetripper.log)"
    rm -rf "$RIPPED"
    mv "$tmp" "$RIPPED"
}

if [ "$RERIP" = 1 ] || [ ! -d "$RIPPED/ExportedProject/Assets" ]; then
    rip
else
    say "Using the rip in reference/ripped (--rerip to redo it)"
fi

if [ "$DECOMP" = 1 ]; then
    command -v ilspycmd >/dev/null || die "--decomp needs ilspycmd (dotnet tool install -g ilspycmd)"
    need_game
    say "Decompiling the game's scripts into reference/decomp"
    ilspycmd -p -o "$ROOT/reference/decomp" "$DATA/Managed/Assembly-CSharp.dll"
fi

# --- Extract ----------------------------------------------------------------

say "Extracting assets into assets/"
(cd "$ROOT" && cargo run --release -p extract -- --ripped "$RIPPED/ExportedProject" --out "$ASSETS")

world_sha1=$( (sha1sum "$ASSETS/world.bin" 2>/dev/null || shasum -a 1 "$ASSETS/world.bin") | cut -d' ' -f1)
if [ "$world_sha1" = "$KNOWN_WORLD_SHA1" ]; then
    say "world.bin matches the build the port is made against"
else
    say "world.bin differs from the build the port is made against ($world_sha1); it should still play, but report anything odd"
fi

# --- 3DS --------------------------------------------------------------------

if [ "$DO_3DS" = 1 ]; then
    export DEVKITPRO=${DEVKITPRO:-/opt/devkitpro}
    export DEVKITARM=${DEVKITARM:-$DEVKITPRO/devkitARM}
    [ -x "$DEVKITPRO/tools/bin/tex3ds" ] || die "devkitPro not found at $DEVKITPRO (install it with the 3ds-dev group: https://devkitpro.org/wiki/Getting_Started)"
    command -v cargo-3ds >/dev/null || die "cargo-3ds not found (cargo install cargo-3ds)"
    rustup toolchain list | grep -q nightly || die "the 3DS build needs nightly Rust (rustup toolchain install nightly --component rust-src)"
    export PATH="$DEVKITARM/bin:$DEVKITPRO/tools/bin:$PATH"
    say "Packing the 3DS romfs"
    (cd "$ROOT" && cargo run --release -p extract --bin pack3ds -- "$ASSETS" crates/n3ds/romfs)
    if [ "$DO_CIA" = 1 ]; then
        say "Building kingdom.3dsx and kingdom.cia"
        "$ROOT/tools/cia/build.sh"
    else
        say "Building kingdom.3dsx"
        (cd "$ROOT/crates/n3ds" && cargo +nightly 3ds build --release)
    fi
    out="$ROOT/crates/n3ds/target/armv6k-nintendo-3ds/release"
    say "Done. On the SD card:"
    echo "  - copy crates/n3ds/sdcard/3ds onto the card's root (sound)"
    echo "  - then $out/kingdom.3dsx into /3ds/ (Homebrew Launcher)"
    if [ "$DO_CIA" = 1 ]; then
        echo "    or install $out/kingdom.cia with FBI"
    fi
else
    say "Done. Play on desktop with: cargo run --release -p desktop"
fi
