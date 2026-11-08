#!/usr/bin/env bash
# The image CI builds the media probe, an ordinary app, and runs it on the booted image with
# SELinux enforcing: it must play sound and run codecs in its own process.
set -euo pipefail

workflow="$(cd "$(dirname "$0")/../.." && pwd)/.github/workflows/test-image.yml"
grep -Fq 'bash dsa/guest/media-probe/build.sh .scratch/media-probe' "$workflow"
grep -Fq 'name: guest-media-probe' "$workflow"
grep -Fq 'DSA_MEDIA_PROBE="$GITHUB_WORKSPACE/.scratch/media-probe/dsa-media-probe.apk"' "$workflow"
grep -Fq 'cargo test -p dsa-host-linux --test media -- --nocapture --test-threads 1' "$workflow"
grep -Fq -- '--test media --no-run' "$workflow"
grep -Fq 'needs: [guest-graphics, channeld, hwcomposer, power-hal, gnss-hal, codec2, audio, sensors, camera, agent, media-probe]' "$workflow"
echo "image media workflow: ok"
