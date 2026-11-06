#!/usr/bin/env bash
# The ARM translation package maker's own tests: what goes into a package's system image and
# what it refuses. Building a translator needs a full AOSP tree and is not run here.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

uv run dsa/guest/arm-translation/test_make_package.py
