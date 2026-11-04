#!/usr/bin/env bash
# Fetches what DSA's camera provider HAL builds against, at pinned revisions, into .scratch/:
#
# - the frozen AIDL definitions of the camera provider HAL it serves (android.hardware.camera
#   provider version 1, the oldest the compatibility matrix of every supported image accepts)
#   and the packages it imports;
# - the platform's camera metadata code (system/media/camera), which builds and reads the
#   metadata buffers the camera framework carries;
# - libbinder_ndk's C++ headers, which the NDK does not ship;
# - the protobuf-lite runtime matching the protoc that generated dsa/protocol/cpp/generated.
#
#   dsa/guest/camera/fetch.sh [scratch dir]
#
# None of it is kept in Git; a build always starts here so it cannot pick up another
# upstream revision by accident.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
scratch_arg="${1:-$root/.scratch}"
mkdir -p "$scratch_arg"
scratch="$(cd "$scratch_arg" && pwd)"

# fetch <url> <dir> <revision> <path>...: a sparse checkout of the paths at the revision.
fetch() {
    local url="$1" dir="$2" revision="$3"
    shift 3
    if [[ ! -d "$dir/.git" ]]; then
        git clone --filter=blob:none --no-checkout "$url" "$dir"
    fi
    git -C "$dir" sparse-checkout set --no-cone "$@"
    git -C "$dir" fetch --depth=1 --no-tags origin "$revision"
    git -C "$dir" checkout --detach "$revision"
    git -C "$dir" reset --hard "$revision"
}

# android-16.0.0_r1. Frozen AIDL versions never change, so any later tag gives the same files.
fetch https://android.googlesource.com/platform/hardware/interfaces "$scratch/camera_interfaces" \
    5688f7eb1e117ed26e642a695de300b7683acb87 \
    'camera/provider/aidl/aidl_api/android.hardware.camera.provider/1/' \
    'camera/device/aidl/aidl_api/android.hardware.camera.device/1/' \
    'camera/common/aidl/aidl_api/android.hardware.camera.common/1/' \
    'camera/metadata/aidl/aidl_api/android.hardware.camera.metadata/1/' \
    'graphics/common/aidl/aidl_api/android.hardware.graphics.common/6/' \
    'common/aidl/aidl_api/android.hardware.common/2/' \
    'common/fmq/aidl/aidl_api/android.hardware.common.fmq/1/'

# android-16.0.0_r1: the camera metadata buffer code and its tag tables.
fetch https://android.googlesource.com/platform/system/media "$scratch/camera_media" \
    f01e84b958fb6a887dc0e74e4b5ebd159f03860a \
    'camera/src/' 'camera/include/' 'private/camera/include/'

# android-16.0.0_r1: the binder NDK headers.
fetch https://android.googlesource.com/platform/frameworks/native "$scratch/camera_native" \
    2827a4a16b0340ecd07c2d5a6c89991799b362bb \
    'libs/binder/ndk/include_cpp/' 'libs/binder/ndk/include_ndk/' 'libs/binder/ndk/include_platform/'

# protobuf v21.12 (3.21.12), the runtime dsa/protocol/cpp/generated was generated for.
protobuf_dir="$scratch/protobuf-21.12"
protobuf_revision="f0dc78d7e6e331b8c6bb2d5283e06aa26883ca7c"
if [[ ! -d "$protobuf_dir/.git" ]]; then
    git clone --filter=blob:none --no-checkout https://github.com/protocolbuffers/protobuf "$protobuf_dir"
fi
git -C "$protobuf_dir" fetch --depth=1 --no-tags origin "$protobuf_revision"
git -C "$protobuf_dir" checkout --detach "$protobuf_revision"
git -C "$protobuf_dir" reset --hard "$protobuf_revision"
