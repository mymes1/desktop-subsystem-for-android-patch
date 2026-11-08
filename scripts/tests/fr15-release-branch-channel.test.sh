#!/usr/bin/env bash
# fr15_the_release_branch_decides_the_channel
#
# The release workflows decide what they may publish from the branch they run on: main below
# 1.0 publishes pre-releases, main from 1.0 stable or pre-release as chosen, develop
# pre-releases only, and any other branch or a tag fails before anything is built or
# published. A release's tag names the commit the run built. main has no planning documents,
# so nothing the release runs may need them.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
decide="$root/.github/scripts/release/release-branch-channel.sh"
tagcheck="$root/.github/scripts/release/check-release-tag.sh"
image="$root/.github/workflows/release-image.yml"
site="$root/.github/workflows/release-site.yml"
sync="$root/.github/scripts/release/sync-release.sh"

fail() {
    echo "fr15 release branch: $*" >&2
    exit 1
}
refused() { ! bash "$decide" "$@" >/dev/null 2>&1; }
reason() { bash "$decide" "$@" 2>&1 >/dev/null || true; }

# main: below 1.0 a pre-release, from 1.0 as chosen.
[[ "$(bash "$decide" refs/heads/main 0.1.10 prerelease)" == prerelease ]] \
    || fail "main does not publish 0.1.10 as a pre-release"
refused refs/heads/main 0.9.0 stable || fail "main published 0.9.0 as stable"
[[ "$(bash "$decide" refs/heads/main 1.0.0 stable)" == stable ]] \
    || fail "main does not publish 1.0.0 as stable"
[[ "$(bash "$decide" refs/heads/main 1.2.0 prerelease)" == prerelease ]] \
    || fail "main does not publish 1.2.0 as a pre-release"

# develop: pre-releases only, at any version.
[[ "$(bash "$decide" refs/heads/develop 0.1.10 prerelease)" == prerelease ]] \
    || fail "develop does not publish 0.1.10 as a pre-release"
[[ "$(bash "$decide" refs/heads/develop 1.3.0 prerelease)" == prerelease ]] \
    || fail "develop does not publish 1.3.0 as a pre-release"
refused refs/heads/develop 1.3.0 stable || fail "develop published 1.3.0 as stable"
[[ "$(reason refs/heads/develop 1.3.0 stable)" == *"develop publishes pre-releases only"* ]] \
    || fail "refusing stable on develop does not say why"
refused refs/heads/develop v1 prerelease || fail "develop accepted a version that is not numbers"

# Any other branch, a tag or a pull request ref: refused with the reason, with or without a
# version.
for ref in refs/heads/feat/release-branch-channel refs/heads/release refs/tags/v0.1.10 refs/pull/12/merge; do
    refused "$ref" 0.1.10 prerelease || fail "$ref published a release"
    refused "$ref" || fail "$ref was accepted for a release without a channel"
    [[ -n "$(reason "$ref")" ]] || fail "refusing $ref gives no reason"
done
[[ "$(reason refs/heads/release)" == *"branch release"* ]] || fail "refusing a branch does not name it"
[[ "$(reason refs/tags/v0.1.10)" == *"tag v0.1.10"* ]] || fail "refusing a tag does not name it"
[[ "$(bash "$decide" refs/heads/main)" == main ]] || fail "main is refused for the images"
[[ "$(bash "$decide" refs/heads/develop)" == develop ]] || fail "develop is refused for the images"

# The image release decides first, and every job waits for that decision.
grep -Fq '.github/scripts/release/release-branch-channel.sh "$REF" "$VERSION" "$REQUESTED"' "$image" \
    || fail "the image release does not decide the installer's channel from the branch"
grep -Fq '.github/scripts/release/release-branch-channel.sh "$REF")"' "$image" \
    || fail "the image release does not check the branch when it publishes images only"
grep -Fq 'REF: ${{ github.ref }}' "$image" || fail "the image release does not pass the ref it runs on"
jobs="$(sed -n 's/^  \([a-z0-9-]*\):\r\{0,1\}$/\1/p' "$image")"
[[ "$(head -n 1 <<<"$jobs")" == channel ]] || fail "the branch check is not the first job"
for job in $jobs; do
    [[ "$job" == channel ]] && continue
    # The job's own lines, up to the next job.
    body="$(awk -v job="$job" '
        $0 ~ "^  " job ":\r?$" { on = 1; next }
        on && /^  [a-z0-9-]+:\r?$/ { exit }
        on { print }' "$image")"
    needs="$(grep -m 1 '^    needs:' <<<"$body" || true)"
    [[ -n "$needs" ]] || fail "the job $job does not wait for the branch check"
    # A job that needs only other jobs waits for the check through them.
    if [[ "$needs" != *channel* ]]; then
        for dep in $(sed 's/^    needs: *//; s/[][,]/ /g' <<<"$needs"); do
            grep -qx "$dep" <<<"$jobs" || fail "the job $job needs $dep, which is not a job"
        done
    fi
done
grep -Fq "needs.channel.result == 'success'" "$image" \
    || fail "the installer does not wait for the channel decision"

# The site, which carries the package repositories, refuses other branches before building.
grep -Fq '.github/scripts/release/release-branch-channel.sh "$REF"' "$site" \
    || fail "the site release does not check the branch"

# Every release's tag names the commit this run built.
grep -Fq 'check-release-tag.sh "$tag" "$GITHUB_SHA"' "$image" || fail "the image tag is not checked"
grep -Fq 'check-release-tag.sh "v$version" $env:GITHUB_SHA' "$image" \
    || fail "the installer tag is not checked"
grep -Fq 'gh release create "$tag" "${files[@]}" --target "$GITHUB_SHA"' "$image" \
    || fail "the image release is not created at the commit built"
grep -Fq -- '--target $env:GITHUB_SHA' "$image" || fail "the installer release is not created at the commit built"

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin"
# A stand-in gh that serves tags from files: $TAGS/<tag> holds "<type> <sha>", and
# $TAGS/objects/<sha> the commit an annotated tag object names.
cat > "$work/bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == api ]] || { echo "unexpected gh $*" >&2; exit 1; }
path="$2"
case "$path" in
    repos/*/git/ref/tags/*)
        tag="${path##*/git/ref/tags/}"
        [[ -f "$TAGS/$tag" ]] || { echo "gh: Not Found (HTTP 404)" >&2; exit 1; }
        cat "$TAGS/$tag"
        ;;
    repos/*/git/tags/*)
        cat "$TAGS/objects/${path##*/git/tags/}"
        ;;
    *) echo "unexpected gh $*" >&2; exit 1 ;;
esac
EOF
chmod +x "$work/bin/gh"
export TAGS="$work/tags" GITHUB_REPOSITORY=thisisthepy/desktop-subsystem-for-android
mkdir -p "$TAGS/objects"
built=1111111111111111111111111111111111111111
other=2222222222222222222222222222222222222222
tagcheck() { PATH="$work/bin:$PATH" bash "$tagcheck" "$@" >/dev/null 2>&1; }

tagcheck v0.1.11 "$built" || fail "a new tag was refused"
echo "commit $built" > "$TAGS/v0.1.11"
tagcheck v0.1.11 "$built" || fail "a tag at the commit built was refused"
echo "commit $other" > "$TAGS/v0.1.11"
! tagcheck v0.1.11 "$built" || fail "a tag at another commit was accepted"
echo "tag 3333333333333333333333333333333333333333" > "$TAGS/v0.1.12"
echo "$other" > "$TAGS/objects/3333333333333333333333333333333333333333"
! tagcheck v0.1.12 "$built" || fail "an annotated tag at another commit was accepted"
echo "$built" > "$TAGS/objects/3333333333333333333333333333333333333333"
tagcheck v0.1.12 "$built" || fail "an annotated tag at the commit built was refused"

# main has no planning documents. The release sync keeps .github/ and dsa/, and what the
# release workflows run does not name the documents it strips.
for file in "$decide" "$tagcheck" "$root/.github/scripts/release/release-channel.sh" \
    "$root/.github/scripts/release/publish-program-list.sh" \
    "$root/.github/scripts/release/build-linux-repo.sh" "$image" "$site" \
    "$root/.github/workflows/release-linux-repo.yml"; do
    if grep -nE '(^|[^a-z/])(PROJECT|AGENTS|CLAUDE)\.md|docs/[A-Za-z0-9_-]+\.md' "$file" \
        | grep -v '^[0-9]*: *#' >/dev/null; then
        fail "$(basename "$file") needs a document that main does not have"
    fi
done
grep -Fq 'for path in PROJECT.md AGENTS.md CLAUDE.md; do' "$sync" \
    || fail "the release sync strips other root files than expected; check what main keeps"
if grep -Eq 'private_paths\+=\("(\.github|dsa|scripts)' "$sync"; then
    fail "the release sync strips what the release workflows run"
fi

echo "fr15 release branch: ok"
