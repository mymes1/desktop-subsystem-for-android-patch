#!/usr/bin/env bash
# The image CI builds DSA's sensors HAL, checks it starts on the image, and installs it.
set -euo pipefail

workflow="$(cd "$(dirname "$0")/../.." && pwd)/.github/workflows/test-image.yml"
grep -Fq 'dsa/guest/sensors/build.sh' "$workflow"
grep -Fq 'guest-sensors' "$workflow"
grep -Fq -- '--sensors .scratch/sensors/dsa-sensors' "$workflow"

# A released image is made the same way.
release="$(dirname "$workflow")/release-image.yml"
grep -Fq 'dsa/guest/sensors/build.sh' "$release"
grep -Fq 'guest-sensors' "$release"
grep -Fq -- '--sensors .scratch/sensors/dsa-sensors' "$release"
