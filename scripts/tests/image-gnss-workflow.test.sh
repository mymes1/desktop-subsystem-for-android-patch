#!/usr/bin/env bash
# The image CI builds DSA's GNSS HAL, checks it starts on the image, and installs it.
set -euo pipefail

workflow="$(cd "$(dirname "$0")/../.." && pwd)/.github/workflows/test-image.yml"
grep -Fq 'dsa/guest/gnss/build.sh' "$workflow"
grep -Fq 'guest-gnss-hal' "$workflow"
grep -Fq -- '--gnss-hal .scratch/gnss-hal/dsa-gnss' "$workflow"

# A released image is made the same way.
release="$(dirname "$workflow")/release-image.yml"
grep -Fq 'dsa/guest/gnss/build.sh' "$release"
grep -Fq 'guest-gnss-hal' "$release"
grep -Fq -- '--gnss-hal .scratch/gnss-hal/dsa-gnss' "$release"
