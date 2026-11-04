#!/usr/bin/env bash
# Fetches what the DSA audio module builds against, at pinned revisions, into .scratch/:
#
# - the frozen AIDL definitions of the audio core HAL it serves (version 4, which the
#   images' audio service declares) and the packages it imports;
# - libbinder_ndk's C++ headers, which the NDK does not ship;
# - the protobuf-lite runtime matching the protoc that generated dsa/protocol/cpp/generated.
#
#   dsa/guest/audio/fetch.sh [scratch dir]
#
# None of it is kept in Git; a build always starts here so it cannot pick up another
# upstream revision by accident.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
scratch_arg="${1:-$root/.scratch}"
mkdir -p "$scratch_arg"
scratch="$(cd "$scratch_arg" && pwd)"

# fetch <url> <dir> <revision> [<sparse path>...]
fetch() {
    local url="$1" dir="$2" revision="$3"
    shift 3
    if [[ ! -d "$dir/.git" ]]; then
        git clone --filter=blob:none --no-checkout "$url" "$dir"
    fi
    if [[ $# -gt 0 ]]; then
        git -C "$dir" sparse-checkout set --no-cone "$@"
    fi
    git -C "$dir" fetch --depth=1 --no-tags origin "$revision"
    git -C "$dir" checkout --detach "$revision"
    git -C "$dir" reset --hard "$revision"
}

# android-17.0.0_r1, the first tag where audio core version 4 is frozen. Frozen AIDL
# versions never change, so any later tag gives the same files.
fetch https://android.googlesource.com/platform/hardware/interfaces \
    "$scratch/audio_hardware_interfaces" 0162af698935100a590b7359581ac8b1b80693e5 \
    'audio/aidl/aidl_api/android.hardware.audio.core/4/' \
    'audio/aidl/aidl_api/android.hardware.audio.core.sounddose/4/' \
    'audio/aidl/aidl_api/android.hardware.audio.common/5/' \
    'audio/aidl/aidl_api/android.hardware.audio.effect/4/' \
    'common/aidl/aidl_api/android.hardware.common/2/' \
    'common/fmq/aidl/aidl_api/android.hardware.common.fmq/1/'

# android-17.0.0_r1: the media types the audio HAL's definitions use.
fetch https://android.googlesource.com/platform/system/hardware/interfaces \
    "$scratch/audio_system_interfaces" 9c3469aa3d4aeec5b2bc8e8ae8fdb4437dab4edb \
    'media/aidl_api/android.media.audio.common.types/5/' \
    'media/aidl_api/android.media.audio.eraser.types/2/'

# android-16.0.0_r1: the binder NDK headers the generated code and the service use.
fetch https://android.googlesource.com/platform/frameworks/native \
    "$scratch/binder_ndk" 2827a4a16b0340ecd07c2d5a6c89991799b362bb \
    'libs/binder/ndk/include_cpp/' 'libs/binder/ndk/include_ndk/' 'libs/binder/ndk/include_platform/'

# protobuf v21.12 (3.21.12), the runtime dsa/protocol/cpp/generated was generated for.
fetch https://github.com/protocolbuffers/protobuf \
    "$scratch/protobuf-21.12" f0dc78d7e6e331b8c6bb2d5283e06aa26883ca7c
