#!/usr/bin/env bash
# Checks the release channels of the installer release: a version below 1.0 is always a
# pre-release, published as a GitHub pre-release and on the pre-release program list only;
# from 1.0 the person releasing chooses. The list that services from before the channels read
# keeps naming the highest version published.
set -euo pipefail

root="$(cd "$(dirname "$0")/../.." && pwd)"
channel="$root/.github/scripts/release/release-channel.sh"
publish="$root/.github/scripts/release/publish-program-list.sh"
workflow="$root/.github/workflows/release-image.yml"

fail() {
    echo "fr15 release channels: $*" >&2
    exit 1
}

# fr15_a_version_below_1_0_is_released_as_a_prerelease
[[ "$(bash "$channel" 0.1.10 prerelease)" == prerelease ]] || fail "0.1.10 is not a pre-release"
[[ "$(bash "$channel" 0.99.0 prerelease)" == prerelease ]] || fail "0.99.0 is not a pre-release"
if bash "$channel" 0.2.0 stable >/dev/null 2>&1; then
    fail "a stable release below 1.0 was accepted"
fi
# fr15_from_1_0_the_person_releasing_chooses_the_channel
[[ "$(bash "$channel" 1.0.0 stable)" == stable ]] || fail "1.0.0 cannot be stable"
[[ "$(bash "$channel" 1.2.3 prerelease)" == prerelease ]] || fail "1.2.3 cannot be a pre-release"
if bash "$channel" 1.0.0 beta >/dev/null 2>&1; then
    fail "an unknown channel was accepted"
fi
if bash "$channel" v1.0 stable >/dev/null 2>&1; then
    fail "a version that is not numbers was accepted"
fi

# fr15_the_workflow_publishes_a_prerelease_as_a_github_prerelease_with_its_channel_lists
grep -Fq '.github/scripts/release/release-channel.sh "$VERSION" "$REQUESTED"' "$workflow" \
    || fail "the workflow does not decide the channel with release-channel.sh"
grep -Fq "if (\$env:CHANNEL -eq 'prerelease') { \$channelFlags += '--prerelease' }" "$workflow" \
    || fail "the workflow does not publish a pre-release as a GitHub pre-release"
grep -Fq -- '--generate-notes @channelFlags' "$workflow" \
    || fail "the release is created without the channel's flags"
grep -Fq 'publish-program-list.sh "$CHANNEL" x64 .scratch/program-list' "$workflow" \
    || fail "the x64 program list is not published on its channel"
grep -Fq 'publish-program-list.sh "$CHANNEL" arm64 .scratch/program-list-arm64' "$workflow" \
    || fail "the arm64 program list is not published on its channel"
for list in program-stable.json program-prerelease.json program-stable-arm64.json program-prerelease-arm64.json; do
    grep -Fq "releases/download/images/$list" "$workflow" || fail "the installers do not read $list"
done
grep -Fq -- '-Channel $env:CHANNEL' "$workflow" || fail "the installer does not record its channel"
if grep -Eq 'gh release upload images program(-arm64)?\.json' "$workflow"; then
    fail "the workflow uploads a program list outside publish-program-list.sh"
fi

# fr15_a_prerelease_goes_on_the_prerelease_list_only, with a stand-in gh that serves a
# published "images" release from a folder and records the uploads.
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/bin" "$work/images"
cat > "$work/bin/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "$1 $2" in
    "release download")
        shift 3
        pattern="" dir=""
        while [[ $# -gt 0 ]]; do
            case "$1" in
                --pattern) pattern="$2"; shift 2 ;;
                --dir) dir="$2"; shift 2 ;;
                *) shift ;;
            esac
        done
        [[ -f "$IMAGES/$pattern" ]] || { echo "no asset matches $pattern" >&2; exit 1; }
        cp "$IMAGES/$pattern" "$dir/"
        ;;
    "release upload")
        shift 3
        for file in "$@"; do
            [[ "$file" == --clobber ]] && continue
            cp "$file" "$IMAGES/"
            basename "$file" >> "$IMAGES/uploads.log"
        done
        ;;
    *) echo "unexpected gh $*" >&2; exit 1 ;;
esac
EOF
chmod +x "$work/bin/gh"
export IMAGES="$work/images"
list() {
    mkdir -p "$work/$1"
    printf '{\n  "format": 1,\n  "version": "%s",\n  "setup": {}\n}\n' "$2" > "$work/$1/program.json"
    echo "signature of $2" > "$work/$1/program.json.sig"
}
uploaded() { grep -qx "$1" "$IMAGES/uploads.log"; }
version_in() { sed -n 's/.*"version": "\(.*\)".*/\1/p' "$IMAGES/$1"; }

# Today's published list, which every service from before the channels reads.
list old 0.1.9
cp "$work/old/program.json" "$IMAGES/program.json"

list pre 0.1.10
PATH="$work/bin:$PATH" bash "$publish" prerelease x64 "$work/pre" >/dev/null
uploaded program-prerelease.json && uploaded program-prerelease.json.sig \
    || fail "the pre-release list was not published"
! uploaded program-stable.json || fail "a pre-release went on the stable list"
[[ "$(version_in program.json)" == 0.1.10 ]] \
    || fail "the services from before the channels are not offered 0.1.10"
[[ "$(cat "$IMAGES/program.json.sig")" == "signature of 0.1.10" ]] \
    || fail "the old list's signature is not the new list's"

# A stable 1.0 goes on the stable list only, and the old list follows the higher version.
: > "$IMAGES/uploads.log"
list stable 1.0.0
PATH="$work/bin:$PATH" bash "$publish" stable arm64 "$work/stable" >/dev/null
uploaded program-stable-arm64.json || fail "the stable arm64 list was not published"
! uploaded program-prerelease-arm64.json || fail "a stable release went on the pre-release list"
[[ "$(version_in program-arm64.json)" == 1.0.0 ]] || fail "the old arm64 list does not name 1.0.0"

# A lower version published later leaves the old list on the higher one.
: > "$IMAGES/uploads.log"
list lower 0.1.11
cp "$work/old/program.json" "$IMAGES/program.json"
list high 0.2.0
cp "$work/high/program.json" "$IMAGES/program.json"
PATH="$work/bin:$PATH" bash "$publish" prerelease x64 "$work/lower" >/dev/null
uploaded program-prerelease.json || fail "the pre-release list was not published"
! uploaded program.json || fail "the old list was moved down to 0.1.11"
[[ "$(version_in program.json)" == 0.2.0 ]] || fail "the old list no longer names 0.2.0"

echo "fr15 release channels: ok"
