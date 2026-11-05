#!/usr/bin/env bash
# Builds gfxstream's host renderer (libgfxstream_backend.dylib) on macOS, as build.sh does on
# Linux and build.ps1 on Windows. The renderer runs on Vulkan, which macOS provides through
# MoltenVK over Metal; the Vulkan loader, its headers and MoltenVK come from Homebrew
# (vulkan-loader, vulkan-headers, molten-vk). Each task's frames reach the window manager by
# read-back (dsa_gfx_set_display_pixels_callback), as on Linux.
#
# Usage: build-macos.sh [scratch dir] [out dir] [config]
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
scratch="${1:-$root/.scratch}"
mkdir -p "$scratch"
scratch="$(cd "$scratch" && pwd)"
out="${2:-$scratch/build-host-gfxstream-macos}"
config="${3:-Release}"

if [[ "$(uname -s)" != Darwin ]]; then
    echo "build-macos.sh builds the macOS renderer and runs on macOS only" >&2
    exit 1
fi

if [[ ! -f "$scratch/gfxstream/CMakeLists.txt" || ! -f "$scratch/aemu/CMakeLists.txt" ||
      ! -d "$scratch/angle/include" ]]; then
    bash "$here/fetch.sh" "$scratch"
fi

brew_prefix="$(brew --prefix 2>/dev/null || echo /opt/homebrew)"
generator=()
if command -v ninja >/dev/null; then
    generator=(-G Ninja)
fi

cmake -B "$out" -S "$here" ${generator[@]+"${generator[@]}"} \
    -DCMAKE_BUILD_TYPE="$config" \
    -DCMAKE_OSX_DEPLOYMENT_TARGET=13.0 \
    -DCMAKE_PREFIX_PATH="$brew_prefix" \
    -DSCRATCH_DIR="$scratch" \
    -DDIST_DIR="$out/dist"
cmake --build "$out" --config "$config" --target dist_host_gfxstream

library="$out/dist/bin/libgfxstream_backend.dylib"
if [[ ! -f "$library" ]]; then
    found="$(find "$out" -name 'libgfxstream_backend*.dylib' -type f | head -n1)"
    if [[ -z "$found" ]]; then
        echo "the build finished but libgfxstream_backend.dylib was not found" >&2
        exit 1
    fi
    mkdir -p "$out/dist/bin"
    cp "$found" "$library"
fi
echo "Built host graphics renderer: $library"
