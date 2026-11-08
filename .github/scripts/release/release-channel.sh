#!/usr/bin/env bash
# Usage: ./.github/scripts/release/release-channel.sh <version> <requested channel>
#
# Prints the release channel an installer release of <version> is published on: stable or
# prerelease. Every version below 1.0 is a pre-release, whatever was asked for, and asking
# for stable below 1.0 is an error rather than a silent change. From 1.0 the person releasing
# chooses, and the request is the answer.
#
# The release workflow publishes a pre-release as a GitHub pre-release and on the
# pre-release program list only; a stable release goes on the stable program list.

set -euo pipefail

die() {
    echo "release-channel: $*" >&2
    exit 1
}

[[ $# -eq 2 ]] || die "usage: release-channel.sh <version> <stable|prerelease>"
version="$1"
requested="$2"

[[ "$version" =~ ^[0-9]+(\.[0-9]+)*$ ]] || die "the version $version is not dot-separated numbers"
case "$requested" in
    stable | prerelease) ;;
    *) die "the channel is stable or prerelease, not $requested" ;;
esac

major="${version%%.*}"
if (( 10#$major < 1 )); then
    if [[ "$requested" == stable ]]; then
        die "$version is below 1.0, and every release below 1.0 is a pre-release; release it on the prerelease channel"
    fi
    echo prerelease
else
    echo "$requested"
fi
