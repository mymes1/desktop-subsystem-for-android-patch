#!/usr/bin/env bash
# Usage: ./.github/scripts/release/publish-program-list.sh <channel> <arch> <folder>
#
# Publishes the signed program list of one installer release to the GitHub release "images",
# which the installed services read:
#
#   <folder>/program.json, program.json.sig   the list and signature written for this release
#   <channel>                                  stable or prerelease (release-channel.sh)
#   <arch>                                     x64 or arm64
#
# The list goes up as the list of its channel only:
#
#   program-stable.json, program-prerelease.json               x64
#   program-stable-arm64.json, program-prerelease-arm64.json   arm64
#
# with its .sig beside it. A computer on the stable channel reads the stable list alone; one
# that takes pre-releases reads both and installs the higher version.
#
# Services installed before the channels existed read program.json (program-arm64.json on
# arm64) and nothing else. Every one of them came from a pre-release installer, so that list
# keeps the version a pre-release computer would take: it is replaced with this release's
# list when this version is higher than the one it names. Those services then update to a
# version that knows the channels, and from then on read the channel lists.
#
# gh must be signed in; GH_REPO names the repository when the folder is not its checkout.

set -euo pipefail

die() {
    echo "publish-program-list: $*" >&2
    exit 1
}

[[ $# -eq 3 ]] || die "usage: publish-program-list.sh <stable|prerelease> <x64|arm64> <folder>"
channel="$1"
arch="$2"
dir="$3"

case "$channel" in
    stable | prerelease) ;;
    *) die "the channel is stable or prerelease, not $channel" ;;
esac
case "$arch" in
    x64) suffix="" ;;
    arm64) suffix="-arm64" ;;
    *) die "the architecture is x64 or arm64, not $arch" ;;
esac
[[ -f "$dir/program.json" && -f "$dir/program.json.sig" ]] \
    || die "$dir has no program.json and program.json.sig"

# The version a list names, from its "version" key.
version_of() {
    sed -n 's/.*"version"[[:space:]]*:[[:space:]]*"\([0-9.]*\)".*/\1/p' "$1" | head -n 1
}

# Whether version $1 is higher than version $2, comparing the numbers.
higher() {
    [[ "$1" != "$2" && "$(printf '%s\n%s\n' "$1" "$2" | sort -t. -k1,1n -k2,2n -k3,3n -k4,4n | tail -n 1)" == "$1" ]]
}

version="$(version_of "$dir/program.json")"
[[ -n "$version" ]] || die "$dir/program.json names no version"

out="$(mktemp -d)"
trap 'rm -rf "$out"' EXIT

named="program-$channel$suffix.json"
cp "$dir/program.json" "$out/$named"
cp "$dir/program.json.sig" "$out/$named.sig"
uploads=("$out/$named" "$out/$named.sig")

legacy="program$suffix.json"
mkdir -p "$out/published"
if gh release download images --pattern "$legacy" --dir "$out/published" >/dev/null 2>&1 \
    && [[ -f "$out/published/$legacy" ]]; then
    published="$(version_of "$out/published/$legacy")"
else
    published=""
fi
if [[ -z "$published" ]] || higher "$version" "$published"; then
    cp "$dir/program.json" "$out/$legacy"
    cp "$dir/program.json.sig" "$out/$legacy.sig"
    uploads+=("$out/$legacy" "$out/$legacy.sig")
    echo "$legacy, which services from before the channels read, now names $version (it named ${published:-nothing})"
else
    echo "$legacy keeps $published, which is not lower than $version"
fi

gh release upload images "${uploads[@]}" --clobber
echo "published $named for DSA $version on the $channel channel"
