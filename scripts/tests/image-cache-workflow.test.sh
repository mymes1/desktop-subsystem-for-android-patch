#!/usr/bin/env bash
# The image CI keeps the Android 17 x86_64 image in the Actions cache, keyed on everything
# that decides it, and makes it again only when one of those changed. The check runs on every
# image, the cached one too, against the key set the image was signed with.
set -euo pipefail

workflow="$(cd "$(dirname "$0")/../.." && pwd)/.github/workflows/test-image.yml"
steps="$(sed -n '/steps: &image-steps/,/^  [a-z-]*:$/p' "$workflow")"
step() { sed -n "/- name: $1\$/,/^      - /p" <<<"$steps"; }

key="$(step "Name the image's inputs")"
for input in "'dsa/guest/image/**'" "'dsa/guest/framework/**'" "'dsa/guest/*/*.rc'" "'dsa/guest/*/*.xml'" \
    "'dsa/android-versions.json'" "'!dsa/guest/**/__pycache__/**'" '.scratch/guest-graphics' \
    '.scratch/channeld' '.scratch/hwcomposer' '.scratch/codec2' '.scratch/audio' '.scratch/sensors' \
    '.scratch/camera' '.scratch/power-hal' '.scratch/gnss-hal' '.scratch/agent' \
    '.scratch/build-tools/aapt2' '.scratch/arm-translation' "sed 's/^/argument: /' .scratch/image-args" \
    'sgdisk --version' 'qemu-img --version' '$ImageOS'; do
  grep -Fq -- "$input" <<<"$key" || { echo "the image's cache key does not name $input"; exit 1; }
done
# The key keeps its prefix once the runner has put the expressions in. The file hash is hex,
# so "$prefix<hash>" would read as one unset variable and leave only "-<inputs>".
rendered="$(grep -E '^ +(prefix|key)=' <<<"$key" \
  | sed -e 's/\${{ matrix.android }}/17/g' -e 's/\${{ matrix.arch }}/x86_64/g' \
        -e 's/\${{ hashFiles([^}]*) }}/40455b6b65898e49/g')"
expanded="$(inputs=aa551940637083dd bash -c "$rendered"$'\n''printf %s "$key"')"
[[ "$expanded" == "guest-image-android-17-x86_64-40455b6b65898e49-aa551940637083dd" ]] \
  || { echo "the image's cache key expands to '$expanded'"; exit 1; }
# The image is made with exactly the arguments the key names.
grep -Fq 'mapfile -t args < .scratch/image-args' <<<"$(step 'Make the image')"

# A hit skips making the image and the keys, and restores the keys the image was signed with.
grep -Fq "if: steps.image-cache.outputs.cache-hit != 'true'" <<<"$(step 'Make the image')"
grep -Fq "if: steps.image-cache.outputs.cache-hit != 'true'" <<<"$(step 'Make a throwaway key set')"
for name in 'Take the image made from the same inputs' 'Keep the image for the next run with the same inputs'; do
  block="$(step "$name")"
  { grep -Fq 'dsa/target/image' <<<"$block" && grep -Fq '.scratch/image-keys' <<<"$block"; } \
    || { echo "$name does not hold the image and its key set"; exit 1; }
  grep -Fq 'key: ${{ steps.image-key.outputs.key }}' <<<"$block"
done
if grep -Fq 'restore-keys' <<<"$(step 'Take the image made from the same inputs')"; then
  echo "an image made from other inputs would be restored"
  exit 1
fi
if grep -Eq '^ +if:' <<<"$(step 'Check the image')"; then
  echo "the check does not run on every image"
  exit 1
fi
# The check comes after the restore and before the save.
restore=$(grep -n 'name: Take the image made from the same inputs' "$workflow" | head -1 | cut -d: -f1)
checked=$(grep -n 'name: Check the image' "$workflow" | head -1 | cut -d: -f1)
save=$(grep -n 'name: Keep the image for the next run with the same inputs' "$workflow" | head -1 | cut -d: -f1)
test "$restore" -lt "$checked" && test "$checked" -lt "$save"

# The cache stays under GitHub's 10 GB: only develop saves, without the partition images the
# system disk already holds, and the entry a new one replaces is deleted.
grep -Fq "zip -d dsa/target/image/*/signed-target_files.zip 'IMAGES/*'" <<<"$steps"
grep -Fq "github.ref == 'refs/heads/develop'" <<<"$(step 'Keep the image for the next run with the same inputs')"
grep -Fq '.github/scripts/cache/prune.sh superseded "${{ steps.image-key.outputs.prefix }}" "${{ steps.image-key.outputs.key }}"' <<<"$steps"
grep -Fq 'CACHE_IMAGE: "true"' "$workflow"
echo "image cache workflow: ok"
