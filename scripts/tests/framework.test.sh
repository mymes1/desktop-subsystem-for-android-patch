#!/usr/bin/env bash
# The recovery policy SurfaceFlinger's RenderEngine and HWUI's RenderThread share after a GPU
# device loss (a C++ test, built with the host's compiler).
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT
c++ -std=c++17 -Wall -Wextra -Werror -I dsa/guest/framework/recovery \
  dsa/guest/framework/recovery/test_device_loss_recovery.cpp -o "$out/test_device_loss_recovery"
"$out/test_device_loss_recovery"
echo "framework recovery policy: ok"
