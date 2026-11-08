#!/usr/bin/env bash
# The image CI consumes the exact guest transport artifact that the graphics CI built.
set -euo pipefail

workflow="$(cd "$(dirname "$0")/../.." && pwd)/.github/workflows/test-image.yml"
grep -Fq 'graphics_run:' "$workflow"
grep -Fq 'guest-gfxstream-hvsocket' "$workflow"
grep -Fq 'gh run download' "$workflow"
grep -Fq -- '--graphics-library' "$workflow"
# The booted image runs the Vulkan CTS subset and opens the OpenGL ES and Vulkan test apps in
# task windows, with the renderer on Lavapipe, and keeps each case's result.
grep -Fq 'name: Build the graphics test apps' "$workflow"
grep -Fq 'dsa/graphics/verify/build-test-apps.sh' "$workflow"
grep -Fq 'needs: [guest-programs, agent, graphics-test-apps]' "$workflow"
grep -Fq 'verify.py fetch-cts' "$workflow"
grep -Fq 'cargo test -p dsa-host-linux --test graphics' "$workflow"
grep -Fq -- '--test media --test graphics --no-run' "$workflow"
grep -Fq 'DSA_GRAPHICS_TEST_APPS=' "$workflow"
grep -Fq 'name: graphics-check-results' "$workflow"
grep -Fq '/tmp/graphics-test-*/cts/' "$workflow"
