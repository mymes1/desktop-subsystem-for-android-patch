#!/usr/bin/env bash
# Usage: ./.github/scripts/release/release-branch-channel.sh <ref> [<version> <requested channel>]
#
# Decides what a release workflow may publish from the ref it runs on ($GITHUB_REF).
#
#   refs/heads/main      a reviewed release: below 1.0 a pre-release, from 1.0 stable or
#                        pre-release as chosen (release-channel.sh decides that part).
#   refs/heads/develop   a development build: always a pre-release; choosing stable fails.
#   anything else        another branch, a tag or a pull request ref: fails, so nothing is
#                        built or published from code that has not reached main or develop.
#
# With only <ref> it checks the branch and prints its name (main or develop), for a workflow
# that publishes nothing on a channel. With <version> and <requested channel> it prints the
# channel the installer release of <version> goes on: stable or prerelease.
#
# Binaries come from the same code as the published source, and only reviewed code becomes
# stable: main moves only through a reviewed pull request, develop takes work as it lands.

set -euo pipefail

die() {
    echo "release-branch-channel: $*" >&2
    exit 1
}

[[ $# -eq 1 || $# -eq 3 ]] \
    || die "usage: release-branch-channel.sh <ref> [<version> <stable|prerelease>]"
ref="$1"

case "$ref" in
    refs/heads/main) branch=main ;;
    refs/heads/develop) branch=develop ;;
    refs/tags/*)
        die "this run is on the tag ${ref#refs/tags/}; releases are published only from main or develop, so run the workflow on one of those branches"
        ;;
    refs/heads/*)
        die "this run is on the branch ${ref#refs/heads/}; releases are published only from main (reviewed releases) or develop (pre-releases), so nothing is published from it"
        ;;
    *)
        die "this run is on $ref, which is not a branch; releases are published only from main or develop"
        ;;
esac

if [[ $# -eq 1 ]]; then
    echo "$branch"
    exit 0
fi

version="$2"
requested="$3"
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [[ "$branch" == develop ]]; then
    if [[ "$requested" == stable ]]; then
        die "develop publishes pre-releases only; release $version from main to publish it on the stable channel, or choose prerelease"
    fi
    # The version and the channel name are still checked.
    bash "$here/release-channel.sh" "$version" "$requested" >/dev/null
    echo prerelease
else
    bash "$here/release-channel.sh" "$version" "$requested"
fi
