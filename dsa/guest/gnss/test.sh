#!/usr/bin/env bash
# Builds and runs the GNSS HAL's tests on the build machine: its channel messages against the
# protocol vectors, the GnssLocation values a host position becomes, and when the host is
# asked for locations.
#
#   dsa/guest/gnss/test.sh
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
"$cxx" "${flags[@]}" -I"$here/tests" -I"$protocol/cpp/generated" "${protobuf[@]}" \
    "$here/src/location_messages.cpp" "$here/src/location_session.cpp" \
    "$here/tests/gnss_test.cpp" "$out/control.pb.o" \
    -lprotobuf-lite -pthread -o "$out/gnss_test"
"$out/gnss_test" "$protocol"
