#!/usr/bin/env bash
# Builds and runs the camera HAL's tests on the build machine: its camera port messages
# against the protocol vectors, what a host camera offers Android, the copy of a picture into
# a buffer, and the JPEG still's trailer.
#
#   dsa/guest/camera/test.sh
#
# Needs a C++17 compiler and the protobuf-lite headers and library of the same version as
# the protoc that generated dsa/protocol/cpp/generated (Debian: libprotobuf-dev).
# PROTOBUF_PREFIX points at another installation root (with include/ and lib/). Output goes
# to the ignored build/ folder beside this script.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
protocol="$(cd "$here/../../protocol" && pwd)"
out="$here/build/test"
cxx="${CXX:-c++}"
flags=(-std=c++17 -Wall -Wextra -Werror -I"$here/src")
protobuf=()
if [[ -n "${PROTOBUF_PREFIX:-}" ]]; then
    protobuf+=(-I"$PROTOBUF_PREFIX/include" -L"$PROTOBUF_PREFIX/lib" -Wl,-rpath,"$PROTOBUF_PREFIX/lib")
fi

rm -rf "$out"
mkdir -p "$out"

# The generated codec is not ours to warn about.
"$cxx" -std=c++17 -I"$protocol/cpp/generated" "${protobuf[@]}" -c \
    "$protocol/cpp/generated/control.pb.cc" -o "$out/control.pb.o"
"$cxx" "${flags[@]}" -I"$here/../hwcomposer/tests" -I"$protocol/cpp/generated" "${protobuf[@]}" \
    "$here/src/camera_messages.cpp" "$here/src/frame_copy.cpp" "$here/src/stream_config.cpp" \
    "$here/tests/camera_test.cpp" "$out/control.pb.o" \
    -lprotobuf-lite -pthread -o "$out/camera_test"
"$out/camera_test" "$protocol"
