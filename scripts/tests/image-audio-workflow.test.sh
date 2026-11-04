#!/usr/bin/env bash
# The image CI builds DSA's audio module, checks it starts on the image, and installs it.
set -euo pipefail

workflow="$(cd "$(dirname "$0")/../.." && pwd)/.github/workflows/test-image.yml"
grep -Fq 'dsa/guest/audio/build.sh' "$workflow"
grep -Fq 'guest-audio' "$workflow"
grep -Fq 'check-guest-driver.py --executable .scratch/audio/dsa-audio' "$workflow"
grep -Fq -- '--audio .scratch/audio/dsa-audio' "$workflow"

# A released image is made the same way.
release="$(dirname "$workflow")/release-image.yml"
grep -Fq 'dsa/guest/audio/build.sh' "$release"
grep -Fq 'check-guest-driver.py --executable .scratch/audio/dsa-audio' "$release"
grep -Fq -- '--audio .scratch/audio/dsa-audio' "$release"
