#!/usr/bin/env bash
# The image CI makes the booted image with a released default ARM translator and checks, with
# SELinux enforcing, that it and a user's Google apps are laid over the system: an image whose
# policy refused both mounts at every boot passed every other check.
set -euo pipefail

workflow="$(cd "$(dirname "$0")/../.." && pwd)/.github/workflows/test-image.yml"
grep -Fq 'name: Download the default ARM translator' "$workflow"
grep -Fq 'echo "args=--arm-translation .scratch/arm-translation" >> "$GITHUB_OUTPUT"' "$workflow"
grep -Fq 'args+=(${{ steps.translator.outputs.args }})' "$workflow"
grep -Fq -- '--test overlays --no-run' "$workflow"
grep -Fq 'check: Lay the ARM translator and Google Play over the booted system' "$workflow"
grep -Fq 'test: overlays' "$workflow"
grep -Fq 'artifact: overlay-check-logs' "$workflow"
grep -Fq '"dsa/host/platform/linux/tests/overlays.rs"' "$workflow"
# The check starts an ARM app through the translator: a translator whose files were all in
# place aborted every ARM app as it started, and only an app's start showed it.
grep -Fq 'run: bash dsa/guest/arm-probe/build.sh .scratch/arm-probe' "$workflow"
grep -Fq 'name: guest-arm-probe' "$workflow"
grep -Fq 'export DSA_ARM_PROBE="$GITHUB_WORKSPACE/.scratch/arm-probe/dsa-arm-probe.apk"' "$workflow"
grep -Fq '"dsa/guest/arm-probe/**"' "$workflow"
# The translator is downloaded before the image's inputs are named and the image is made, so
# the image carries it and its cache entry names it.
download=$(grep -n 'name: Download the default ARM translator' "$workflow" | head -1 | cut -d: -f1)
inputs=$(grep -n "name: Name the image's inputs" "$workflow" | head -1 | cut -d: -f1)
make=$(grep -n 'name: Make the image' "$workflow" | head -1 | cut -d: -f1)
test "$download" -lt "$inputs"
test "$inputs" -lt "$make"
grep -Fq '[[ -d .scratch/arm-translation ]] && programs+=(.scratch/arm-translation)' "$workflow"
# The idle reclaim with Google apps installed is measured on a runner of its own, and the
# overlay check leaves it out; its report goes to the job summary and its artifact.
grep -Fq 'filter: --skip nfr3_the_idle_reclaim_with_google_apps_is_measured' "$workflow"
grep -Fq 'filter: nfr3_the_idle_reclaim_with_google_apps_is_measured' "$workflow"
grep -Fq 'summary: /tmp/overlays-test-*/idle-reclaim.md' "$workflow"
grep -Fq 'if: always() && matrix.summary' "$workflow"
grep -Fq 'cat "$report" >> "$GITHUB_STEP_SUMMARY"' "$workflow"
grep -Fq '/tmp/overlays-test-*/idle-reclaim*' "$workflow"
grep -Fq '"dsa/host/platform/linux/tests/idle_reclaim/**"' "$workflow"
echo "image boot overlays workflow: ok"
