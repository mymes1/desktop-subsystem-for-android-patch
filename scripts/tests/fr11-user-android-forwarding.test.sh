#!/usr/bin/env bash
# Every wrapper the service puts around a user's Android forwards every request the local API
# can make of it. A request a wrapper leaves out falls back to the trait's refusal, so the
# feature is switched off in the installed service while every session test still passes:
# printing, widgets, APK installs and app management were lost that way once.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

uv run --no-project python - <<'PY'
import re
import sys
from pathlib import Path

FN = re.compile(r"(?m)^    fn (\w+)")


def block(text, start):
    # The body runs to the first closing brace at the start of a line: format strings hold
    # braces too, so counting them would end it in the wrong place.
    begin = text.index("{\n", start) + 2
    return text[begin:text.index("\n}\n", begin)]


trait_text = Path("dsa/host/core/src/host_api.rs").read_text(encoding="utf-8")
requests = FN.findall(block(trait_text, trait_text.index("pub trait UserAndroid")))
wrappers = {
    "dsa/host/core/src/update/service.rs": "UpdatingAndroid",
    "dsa/host/core/src/update/program_service.rs": "WithProgramStatus",
}
failed = False
for path, name in wrappers.items():
    text = Path(path).read_text(encoding="utf-8")
    have = set(FN.findall(block(text, text.index(f"impl UserAndroid for {name}"))))
    for request in requests:
        if request not in have:
            print(f"{name} ({path}) does not forward {request}, so the service refuses it")
            failed = True
if failed:
    sys.exit(1)
print(f"ok   - {len(wrappers)} wrappers forward all {len(requests)} requests")
PY
