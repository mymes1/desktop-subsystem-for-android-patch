#!/usr/bin/env bash
# Builds what the Linux host's VM runs, for CI: gfxstream's host renderer, then crosvm linked
# with it, and passt (dsa/host/platform/linux/crosvm/build.sh). Everything lands in one
# folder, which the KVM test and the packages take:
#
#   build-vm-programs.sh <out folder>
#
# A cache of that folder, keyed on the pinned revisions and the build scripts, skips the
# build when nothing changed.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
out="${1:?name the output folder}"
mkdir -p "$out"
out="$(cd "$out" && pwd)"

if [[ -x "$out/crosvm" && -x "$out/passt" && -f "$out/libgfxstream_backend.so" &&
      -f "$out/crosvm-crates.txt" && -f "$out/passt-source.tar.gz" ]]; then
    echo "the VM programs are already built"
    ls -l "$out"
    exit 0
fi

sudo apt-get update
sudo apt-get install -y --no-install-recommends \
    git make gcc g++ clang libclang-dev pkg-config libcap-dev libwayland-dev \
    wayland-protocols libdrm-dev protobuf-compiler ninja-build cmake libvulkan-dev \
    libx11-dev libxext-dev

bash "$root/dsa/graphics/host/fetch.sh" "$root/.scratch"
bash "$root/dsa/graphics/host/build.sh" "$root/.scratch" "$root/.scratch/build-host"
install -m 0755 "$root/.scratch/build-host/dist/bin/libgfxstream_backend.so" "$out/"

bash "$root/dsa/host/platform/linux/crosvm/build.sh" --out "$out" --gfxstream "$out"
