#!/usr/bin/env bash
# Builds dsa-gnss, the guest's GNSS HAL, for x86_64 Android with the NDK.
#
#   dsa/guest/gnss/build.sh [scratch dir] [output dir]
#
# It fetches the pinned sources (fetch.sh), generates the binder code for the HAL and the
# control channel with the Android SDK's aidl, and builds with CMake. The binary lands at
# <output dir>/dsa-gnss (default .scratch/build-gnss).
#
# ANDROID_NDK_HOME (or ANDROID_NDK_ROOT) names the NDK, ANDROID_HOME (or ANDROID_SDK_ROOT)
# the SDK whose newest build-tools provide aidl. Both are found in their usual places when
# unset.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/../../.." && pwd)"
scratch_arg="${1:-$root/.scratch}"
mkdir -p "$scratch_arg"
scratch="$(cd "$scratch_arg" && pwd)"
out_arg="${2:-$scratch/build-gnss}"
mkdir -p "$out_arg"
out="$(cd "$out_arg" && pwd)"

"$here/fetch.sh" "$scratch"

windows=false
case "$(uname -s)" in
    MINGW*|MSYS*|CYGWIN*) windows=true ;;
esac
# aidl on Windows compares import paths with backslashes.
native_path() {
    if $windows; then cygpath -w "$1"; else printf '%s\n' "$1"; fi
}

sdk="${ANDROID_HOME:-${ANDROID_SDK_ROOT:-}}"
for candidate in "$sdk" /usr/local/lib/android/sdk "$HOME/AppData/Local/Android/Sdk" "$HOME/Android/Sdk"; do
    if [[ -n "$candidate" && -d "$candidate/build-tools" ]]; then
        sdk="$candidate"
        break
    fi
done
aidl=""
if [[ -d "$sdk/build-tools" ]]; then
    for tools in $(find "$sdk/build-tools" -mindepth 1 -maxdepth 1 -type d | sort -V -r); do
        if [[ -x "$tools/aidl" || -x "$tools/aidl.exe" ]]; then
            aidl="$tools/aidl"
            [[ -x "$aidl" ]] || aidl="$tools/aidl.exe"
            break
        fi
    done
fi
if [[ -z "$aidl" ]]; then
    echo "aidl not found: install Android SDK build-tools and set ANDROID_HOME" >&2
    exit 1
fi
echo "aidl: $aidl"

# The binder code for the GNSS HAL (frozen version 4) and the control channel's service.
gen="$out/aidl"
rm -rf "$gen"
mkdir -p "$gen/include" "$gen/src"
gnss="$scratch/gnss_interfaces/gnss/aidl/aidl_api/android.hardware.gnss/4"
channel="$root/dsa/guest/aidl"

# generate <frozen dir> <version> [<import dir>...]: every type in the package.
generate() {
    local dir="$1" version="$2"
    shift 2
    local args=(--lang=ndk --structured --stability=vintf --version="$version")
    if [[ -f "$dir/.hash" ]]; then
        args+=(--hash="$(tail -n 1 "$dir/.hash")")
    fi
    for import in "$@"; do
        args+=(-I "$(native_path "$import")")
    done
    local files=()
    while IFS= read -r file; do
        files+=("$(native_path "$file")")
    done < <(find "$dir" -name '*.aidl' | sort)
    "$aidl" "${args[@]}" -I "$(native_path "$dir")" \
        -h "$(native_path "$gen/include")" -o "$(native_path "$gen/src")" "${files[@]}"
}

generate "$gnss" 4
generate "$channel" 1

ndk_dir="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-}}"
for candidate in /usr/local/lib/android/sdk/ndk "$HOME/AppData/Local/Android/Sdk/ndk"; do
    if [[ -z "$ndk_dir" && -d "$candidate" ]]; then
        ndk_dir="$(find "$candidate" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -n 1)"
    fi
done
if [[ -z "$ndk_dir" || ! -f "$ndk_dir/build/cmake/android.toolchain.cmake" ]]; then
    echo "Android NDK not found. Set ANDROID_NDK_HOME or ANDROID_NDK_ROOT." >&2
    exit 1
fi

cmake_bin="${CMAKE_BIN:-cmake}"
if ! command -v "$cmake_bin" >/dev/null 2>&1; then
    for candidate in "$HOME/AppData/Local/Android/Sdk/cmake"/*/bin/cmake.exe \
        "/usr/local/lib/android/sdk/cmake"/*/bin/cmake; do
        if [[ -f "$candidate" ]]; then
            cmake_bin="$candidate"
            break
        fi
    done
fi
generator=()
if command -v ninja >/dev/null 2>&1; then
    generator=(-G Ninja)
elif [[ -f "${cmake_bin%/*}/ninja" ]]; then
    generator=(-G Ninja -DCMAKE_MAKE_PROGRAM="${cmake_bin%/*}/ninja")
elif [[ -f "${cmake_bin%/*}/ninja.exe" ]]; then
    generator=(-G Ninja -DCMAKE_MAKE_PROGRAM="${cmake_bin%/*}/ninja.exe")
fi

"$cmake_bin" -B "$out/cmake" -S "$here" "${generator[@]}" \
    -DCMAKE_BUILD_TYPE=Release \
    -DCMAKE_TOOLCHAIN_FILE="$ndk_dir/build/cmake/android.toolchain.cmake" \
    -DANDROID_ABI=x86_64 \
    -DANDROID_PLATFORM=android-34 \
    -DSCRATCH_DIR="$scratch" \
    -DAIDL_GEN_DIR="$gen"
"$cmake_bin" --build "$out/cmake" --target dsa-gnss

cp "$out/cmake/dsa-gnss" "$out/dsa-gnss"
echo "Built: $out/dsa-gnss"
