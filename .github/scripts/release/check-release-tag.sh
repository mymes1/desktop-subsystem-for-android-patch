#!/usr/bin/env bash
# Usage: ./.github/scripts/release/check-release-tag.sh <tag> <commit>
#
# Fails when the tag <tag> already exists in this repository ($GITHUB_REPOSITORY) at a
# commit other than <commit>. A release's tag must name the commit its files were built from,
# and `gh release create --target` reuses a tag that already exists, wherever it points,
# without moving it. Run it just before creating the release; a missing tag is fine, as
# `gh release create` then makes it at the target.

set -euo pipefail

die() {
    echo "check-release-tag: $*" >&2
    exit 1
}

[[ $# -eq 2 ]] || die "usage: check-release-tag.sh <tag> <commit>"
tag="$1"
commit="$2"
repo="${GITHUB_REPOSITORY:?the repository is not set (GITHUB_REPOSITORY)}"

if ! ref="$(gh api "repos/$repo/git/ref/tags/$tag" --jq '.object.type + " " + .object.sha' 2>&1)"; then
    if [[ "$ref" == *"Not Found"* || "$ref" == *"HTTP 404"* ]]; then
        echo "the tag $tag does not exist yet; the release creates it at $commit"
        exit 0
    fi
    die "looking up the tag $tag failed: $ref"
fi

type="${ref%% *}"
sha="${ref#* }"
# An annotated tag names a tag object, which names the commit.
if [[ "$type" == tag ]]; then
    sha="$(gh api "repos/$repo/git/tags/$sha" --jq '.object.sha')" \
        || die "reading the annotated tag $tag failed"
fi

if [[ "$sha" != "$commit" ]]; then
    die "the tag $tag already exists at $sha, not at $commit, the commit this run built; delete the tag (or replace the release) and run again"
fi
echo "the tag $tag is already at $commit"
