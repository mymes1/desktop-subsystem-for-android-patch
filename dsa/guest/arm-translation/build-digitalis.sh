#!/usr/bin/env bash
# Builds Digitalis, the open source (Apache-2.0) ARM64 to x86_64 translator built on AOSP's
# Berberis, at a pinned revision, and leaves its binary bundle where make-package.py takes it.
#
#   dsa/guest/arm-translation/build-digitalis.sh [<work dir>]
#
# The work dir defaults to .scratch/digitalis in this repository. Nothing is vendored: the
# Digitalis manifest pulls an AOSP tree of its own (several hundred GB once built), and the
# build runs in Digitalis' own Docker container. It needs repo, git, Docker, and a machine
# with the disk, memory and hours a full AOSP build takes, which a hosted CI runner does not
# have. Then:
#
#   uv run dsa/guest/arm-translation/make-package.py \
#       --bundle .scratch/digitalis/digitalis/dist/digitalis-prebuilts \
#       --name Digitalis --version <revision> --out .scratch/arm-translation-package
#   dsa arm-translation install .scratch/arm-translation-package
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/../../.." && pwd)"
WORK="${1:-$REPO/.scratch/digitalis}"

# The guest is Android 17, so the translator and its ARM64 guest libraries come from
# Digitalis' Android 17 branch. Each is a commit, so a rebuild is the same build.
MANIFEST_URL="https://github.com/DigitalisX64/manifest.git"
MANIFEST_REVISION="9cd175075230ef2cd775ceae412ed36505083a16"   # android17
DIGITALIS_REVISION="65e242e3898be362f6160a353297a50d5430b6dc"  # android17

for tool in repo git docker; do
    command -v "$tool" >/dev/null || { echo "build-digitalis.sh needs $tool" >&2; exit 1; }
done

mkdir -p "$WORK"
cd "$WORK"
if [ ! -d .repo ]; then
    repo init -u "$MANIFEST_URL" -b "$MANIFEST_REVISION" --depth=1
fi
repo sync -c -d --no-tags --force-sync -j"$(nproc)"

# The manifest names Digitalis' own repository by branch; it is moved to the pinned commit.
git -C digitalis fetch --depth=1 origin "$DIGITALIS_REVISION"
git -C digitalis checkout --detach "$DIGITALIS_REVISION"

# A full build the first time: the bundle takes the guest libraries from the assembled image.
if [ -d out/target/product ]; then
    digitalis/docker/build-digitalis.sh
else
    digitalis/docker/build-digitalis.sh --full
fi
digitalis/scripts/verify-digitalis-prebuilts.sh

echo "the bundle is in $WORK/digitalis/dist/digitalis-prebuilts"
