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
# The boot checks, the graphics one among them, wait for the booted image, the checks'
# build and the test apps; the list may name more jobs (the kernel they boot with).
boot_check_needs="$(awk '/^  boot-check:$/ { job = 1; next } job && /^  [a-z-]+:$/ { exit }
  job && /^    needs: \[/ { print; exit }' "$workflow")"
for need in boot-image boot-checks graphics-test-apps; do
  grep -Eq "[[ ]$need[],]" <<<"$boot_check_needs"
done
grep -Fq 'verify.py fetch-cts' "$workflow"
grep -Fq 'test: graphics' "$workflow"
grep -Eq -- '--test graphics( --test [a-z_]+)* --no-run' "$workflow"
grep -Fq 'DSA_GRAPHICS_TEST_APPS=' "$workflow"
grep -Fq 'artifact: graphics-check-results' "$workflow"
grep -Fq '/tmp/graphics-test-*/cts/' "$workflow"
