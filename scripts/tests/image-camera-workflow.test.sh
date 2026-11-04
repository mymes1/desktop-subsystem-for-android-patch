#!/usr/bin/env bash
# The image CI builds DSA's camera HAL, checks it starts on the image, and installs it.
set -euo pipefail

workflow="$(cd "$(dirname "$0")/../.." && pwd)/.github/workflows/test-image.yml"
grep -Fq 'dsa/guest/camera/build.sh' "$workflow"
grep -Fq 'guest-camera' "$workflow"
grep -Fq -- '--camera .scratch/camera/dsa-camera' "$workflow"

# A released image is made the same way.
release="$(dirname "$workflow")/release-image.yml"
grep -Fq 'dsa/guest/camera/build.sh' "$release"
grep -Fq 'guest-camera' "$release"
grep -Fq -- '--camera .scratch/camera/dsa-camera' "$release"
