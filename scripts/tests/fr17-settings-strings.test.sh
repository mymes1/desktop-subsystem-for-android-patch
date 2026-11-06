#!/usr/bin/env bash
# Every string table of the settings app is well-formed XML, names each key once, and has the
# same keys in every language. A merge that drops one closing tag breaks the build of the app
# and every installer, so it is caught here, in the fast checks.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

uv run --no-project python - dsa/host/platform/windows/settings/DsaSettings/Strings*.resx <<'PY'
import sys
import xml.etree.ElementTree as ET

tables = {}
failed = False
for path in sys.argv[1:]:
    try:
        root = ET.parse(path).getroot()
    except ET.ParseError as error:
        print(f"{path} is not well-formed XML: {error}")
        failed = True
        continue
    names = [data.get("name") for data in root.iter("data")]
    seen = set()
    for name in names:
        if name in seen:
            print(f"{path} names {name} twice")
            failed = True
        seen.add(name)
    tables[path] = seen
if tables:
    first, keys = next(iter(tables.items()))
    for path, other in tables.items():
        for name in sorted(keys - other):
            print(f"{path} has no {name}, which {first} has")
            failed = True
        for name in sorted(other - keys):
            print(f"{path} has {name}, which {first} does not")
            failed = True
if failed:
    sys.exit(1)
print(f"ok   - {len(tables)} string tables, {len(keys) if tables else 0} keys each")
PY
