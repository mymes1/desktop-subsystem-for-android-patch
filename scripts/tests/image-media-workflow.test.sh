#!/usr/bin/env bash
# The image CI builds the media probe, an ordinary app, and runs it on the booted image with
# SELinux enforcing: it must play sound and run codecs in its own process.
set -euo pipefail

workflow="$(cd "$(dirname "$0")/../.." && pwd)/.github/workflows/test-image.yml"
grep -Fq 'bash dsa/guest/media-probe/build.sh .scratch/media-probe' "$workflow"
grep -Fq 'name: guest-media-probe' "$workflow"
grep -Fq 'DSA_MEDIA_PROBE="$GITHUB_WORKSPACE/.scratch/media-probe/dsa-media-probe.apk"' "$workflow"
grep -Fq 'check: Play sound and run codecs in an app on the booted image' "$workflow"
grep -Fq 'test: media' "$workflow"
grep -Fq 'artifact: media-check-logs' "$workflow"
grep -Eq -- '--test media( --test [a-z_]+)* --no-run' "$workflow"
# The image waits for the job that builds the agent and the media probe, among others.
grep -Eq 'needs: \[guest-programs, agent(, [a-z-]+)*\]' "$workflow"
grep -Fq 'name: Build the guest agent and the media probe' "$workflow"
echo "image media workflow: ok"
