#!/usr/bin/env bash
# The supported Android versions are written once, in dsa/android-versions.json. The image
# inputs, both image workflows and the README rows that name the versions must say the same,
# so a new major is added in one place and every other place is caught if it is left behind.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

uv run --no-project python - <<'PY'
import json
import re
import sys

failed = False


def fail(message):
    global failed
    print(message)
    failed = True


supported = sorted(set(json.load(open("dsa/android-versions.json"))["supported_majors"]))
if len(supported) != 2:
    fail(f"dsa/android-versions.json names {supported}; DSA supports exactly the latest two majors")


def check(where, found):
    if sorted(set(found)) != supported:
        fail(f"{where} names Android {sorted(set(found))}, but dsa/android-versions.json says {supported}")


inputs = json.load(open("dsa/guest/image/inputs.json"))["images"]
check("dsa/guest/image/inputs.json", [image["android_major"] for image in inputs])

release = open(".github/workflows/release-image.yml").read()
matrix = re.findall(r"^\s+android: \[([0-9, ]+)\]", release, re.M)
if not matrix:
    fail(".github/workflows/release-image.yml has no android matrix")
for line in matrix:
    check(".github/workflows/release-image.yml (matrix)", [int(m) for m in line.split(",")])
if "dsa/android-versions.json" not in release:
    fail(".github/workflows/release-image.yml does not take the supported versions from dsa/android-versions.json")

test = open(".github/workflows/test-image.yml").read()
options = re.search(r"\n      android:\n(?:        .*\n)*?        options:\n((?:          - .*\n)+)", test)
if not options:
    fail(".github/workflows/test-image.yml has no android options")
else:
    check(".github/workflows/test-image.yml (options)",
          [int(m) for m in re.findall(r"- \"?([0-9]+)\"?", options.group(1))])
for line in re.findall(r"'\[(\"[0-9]+\"(?:, \"[0-9]+\")*)\]'", test):
    check(".github/workflows/test-image.yml (all)", [int(m) for m in re.findall(r"[0-9]+", line)])

for path, row in (("README.md", "| Versions |"), ("docs/locales/README_ko.md", "| 버전 |")):
    lines = [line for line in open(path, encoding="utf-8") if line.startswith(row)]
    if len(lines) != 1:
        fail(f"{path} has no single row starting {row!r}")
        continue
    # The row names the versions as two-digit numbers and no other number.
    check(path, [int(n) for n in re.findall(r"\b([0-9]{2})\b", lines[0])])

if failed:
    sys.exit(1)
print(f"the supported Android versions agree everywhere: {supported}")
PY
