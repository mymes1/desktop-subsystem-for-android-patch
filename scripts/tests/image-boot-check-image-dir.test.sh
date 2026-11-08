#!/usr/bin/env bash
# The image the boot checks boot is never kept in Cargo's target folder on a runner that
# restores the Rust build. On a cache that was not an exact match (any changed Cargo.toml),
# rust-cache cleans the target folder and deletes every file in it that Cargo did not make:
# the image folder was left empty, and every boot check failed with "has neither system.vhdx
# nor disk.vhdx".
set -euo pipefail

workflow="$(cd "$(dirname "$0")/../.." && pwd)/.github/workflows/test-image.yml"
job() { sed -n "/^  $1:\$/,/^  [a-z-]*:\$/p" "$workflow"; }
boot="$(job boot-check)"
build="$(job boot-checks)"
test -n "$boot" && test -n "$build" && test -n "$(job image)"

# The boot checks take the image outside the target folder and read it from there.
grep -Fq 'path: .scratch/boot-image' <<<"$boot"
grep -Fq 'image="$(ls -d "$PWD"/.scratch/boot-image/*/ | head -1)"' <<<"$boot"
if grep -q 'DSA_WIDGET_TEST_IMAGE=.*dsa/target' "$workflow" \
  || grep -Fq 'ls -d "$PWD"/dsa/target/image' "$workflow"; then
  echo "the boot checks read the image from Cargo's target folder, which rust-cache cleans"
  exit 1
fi
# The jobs that hold an image do not restore the Rust build, and the job that restores it
# holds no image.
for name in boot-check image; do
  if grep -Fq 'uses: Swatinem/rust-cache' <<<"$(job "$name")"; then
    echo "the $name job restores the Rust build beside an image"
    exit 1
  fi
done
grep -Fq 'uses: Swatinem/rust-cache' <<<"$build"
if grep -v '^ *#' <<<"$build" | grep -Eq 'boot-image|dsa/target/image'; then
  echo "the job that restores the Rust build also holds an image"
  exit 1
fi
echo "image boot check image folder: ok"
