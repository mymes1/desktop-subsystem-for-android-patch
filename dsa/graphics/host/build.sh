#!/usr/bin/env bash
# Builds gfxstream's host renderer (libgfxstream_backend.so) on Linux, as build.ps1 does on
# Windows. The Linux compositor loads it to run the guest's Vulkan on the host GPU.
#
# Usage: build.sh [scratch dir] [out dir] [config]
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
scratch="${1:-$root/.scratch}"
mkdir -p "$scratch"
scratch="$(cd "$scratch" && pwd)"
out="${2:-$scratch/build-host-gfxstream}"
config="${3:-Release}"

if [[ ! -f "$scratch/gfxstream/CMakeLists.txt" || ! -f "$scratch/aemu/CMakeLists.txt" ||
      ! -d "$scratch/angle/include" ]]; then
    bash "$here/fetch.sh" "$scratch"
fi

generator=()
if command -v ninja >/dev/null; then
    generator=(-G Ninja)
fi

cmake -B "$out" -S "$here" "${generator[@]}" \
    -DCMAKE_BUILD_TYPE="$config" \
    -DSCRATCH_DIR="$scratch" \
    -DDIST_DIR="$out/dist"
cmake --build "$out" --config "$config" --target dist_host_gfxstream

library="$out/dist/bin/libgfxstream_backend.so"
if [[ ! -f "$library" ]]; then
    found="$(find "$out" -name 'libgfxstream_backend.so*' -type f | head -n1)"
    if [[ -z "$found" ]]; then
        echo "the build finished but libgfxstream_backend.so was not found" >&2
        exit 1
    fi
    mkdir -p "$out/dist/bin"
    cp "$found" "$library"
fi
echo "Built host graphics renderer: $library"
