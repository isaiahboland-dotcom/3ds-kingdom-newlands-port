#!/usr/bin/env bash
# Builds kingdom.cia, an installable HOME Menu title, from the same build as
# kingdom.3dsx. Run `extract` and `pack3ds` first (README, "Building for 3DS").
#
#   tools/cia/build.sh
#
# Needs devkitPro (DEVKITPRO, default /opt/devkitpro) and cargo-3ds, plus
# makerom and bannertool: taken from PATH, else built once from source into
# tools/cia/bin (needs git, make and a C/C++ compiler). The HOME Menu icon,
# banner and jingle come from your own game files (RIPPED, default
# reference/ripped/ExportedProject) and, like the romfs, are never committed.
#
# Output: crates/n3ds/target/armv6k-nintendo-3ds/release/kingdom.cia. Install
# it with FBI (or another title manager); sound still comes from
# sdmc:/3ds/kingdom-3ds/audio, as for the .3dsx.
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/../.." && pwd)
HERE="$ROOT/tools/cia"
RIPPED=${RIPPED:-$ROOT/reference/ripped/ExportedProject}
export DEVKITPRO=${DEVKITPRO:-/opt/devkitpro}
export DEVKITARM=${DEVKITARM:-$DEVKITPRO/devkitARM}
export PATH="$HERE/bin:$DEVKITARM/bin:$DEVKITPRO/tools/bin:$PATH"
OUT="$ROOT/crates/n3ds/target/armv6k-nintendo-3ds/release"
WORK="$OUT/cia"

# Pinned sources for the packaging tools.
MAKEROM_REPO=https://github.com/3DSGuy/Project_CTR.git
MAKEROM_REV=e8f5f529c54ff9b22a2491a480ffa69206bf7b19
BANNERTOOL_REPO=https://github.com/diasurgical/bannertool.git
BANNERTOOL_REV=16d8c5a0ce02a5e06e64ab42275132fca57c04a2

build_makerom() {
    echo "building makerom ($MAKEROM_REV)..."
    rm -rf "$HERE/src/Project_CTR"
    git clone --quiet "$MAKEROM_REPO" "$HERE/src/Project_CTR"
    git -C "$HERE/src/Project_CTR" checkout --quiet "$MAKEROM_REV"
    # The dependencies first: a parallel build can link before they exist.
    make -C "$HERE/src/Project_CTR/makerom" deps
    make -C "$HERE/src/Project_CTR/makerom" program >/dev/null
    cp "$HERE/src/Project_CTR/makerom/bin/makerom" "$HERE/bin/"
}

build_bannertool() {
    echo "building bannertool..."
    rm -rf "$HERE/src/bannertool"
    git clone --quiet "$BANNERTOOL_REPO" "$HERE/src/bannertool"
    git -C "$HERE/src/bannertool" checkout --quiet "$BANNERTOOL_REV"
    git -C "$HERE/src/bannertool" submodule update --quiet --init --depth 1
    make -C "$HERE/src/bannertool" >/dev/null
    local bt
    bt="$(find "$HERE/src/bannertool/output" -type f -name bannertool | head -n 1)"
    if [ -z "$bt" ]; then
        echo "bannertool build produced no binary" >&2
        return 1
    fi
    cp "$bt" "$HERE/bin/"
}

mkdir -p "$HERE/bin" "$HERE/src" "$WORK"
command -v makerom >/dev/null || build_makerom
command -v bannertool >/dev/null || build_bannertool

if [ ! -f "$ROOT/crates/n3ds/romfs/world.bin" ]; then
    echo "no romfs: run extract and pack3ds first (README, \"Building for 3DS\")" >&2
    exit 1
fi

echo "building the game..."
(cd "$ROOT/crates/n3ds" && cargo +nightly 3ds build --release)

echo "making the HOME Menu art..."
(cd "$ROOT" && cargo run --quiet --release -p extract --bin ciaart -- "$RIPPED" "$ROOT/assets" "$WORK")
bannertool makebanner -i "$WORK/banner.png" -a "$WORK/banner.wav" -o "$WORK/banner.bnr" >/dev/null
bannertool makesmdh -s "Kingdom 3DS" -l "Kingdom: New Lands, fan port" -p "Fan port (your own game files)" \
    -i "$WORK/icon.png" -o "$WORK/icon.icn" >/dev/null

echo "packing kingdom.cia..."
makerom -f cia -target t -o "$OUT/kingdom.cia" \
    -elf "$OUT/kingdom.elf" -rsf "$HERE/kingdom.rsf" \
    -icon "$WORK/icon.icn" -banner "$WORK/banner.bnr" \
    -DAPP_ROMFS="$ROOT/crates/n3ds/romfs"
ls -l "$OUT/kingdom.cia"
