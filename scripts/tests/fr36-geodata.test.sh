#!/usr/bin/env bash
# The address data script's own tests: how Korea's dongs become address parts, how boundaries
# are simplified, and the file format the host reads. The data itself is made by the packaging
# scripts; these run on every change and download nothing.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

if command -v uv >/dev/null 2>&1; then
    uv run dsa/host/core/geodata/test_build_geodata.py
else
    python3 dsa/host/core/geodata/test_build_geodata.py
fi
