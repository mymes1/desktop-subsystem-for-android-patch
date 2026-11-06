# /// script
# requires-python = ">=3.11"
# ///
"""Lists the licence of everything the shipped crosvm links, and fails on copyleft.

crosvm's crates come from `cargo metadata` for the features DSA builds, following only the
dependencies that are linked into the program (build scripts and dev dependencies are not
shipped). A crate inside crosvm's own tree that names no licence is under crosvm's LICENSE
(BSD-3-Clause), as minijail's crates are under minijail's. The shared libraries the program
needs come from its dynamic section: each must be one the system provides, or gfxstream's
renderer, which DSA ships under Apache-2.0.

Anything GPL, LGPL or AGPL that cannot be chosen away (an `OR` with a permissive licence
can) fails the check. The list is written for the package's licence notices.

  check-licenses.py --source <crosvm checkout> --features <list> --binary <crosvm> --out <file>
"""

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

# Shared libraries the system provides and the package does not ship. glibc is LGPL, but it
# is the system's C library, which every program on the computer links, not part of DSA.
SYSTEM_LIBRARIES = {
    "libc.so.6", "libm.so.6", "libdl.so.2", "libpthread.so.0", "librt.so.1",
    "libgcc_s.so.1", "libstdc++.so.6", "libcap.so.2", "libwayland-client.so.0",
    "libdrm.so.2", "libgbm.so.1", "libepoxy.so.0",
}
SHIPPED_LIBRARIES = {"libgfxstream_backend.so": "Apache-2.0"}
LOADERS = re.compile(r"^ld-linux[-\w.]*\.so\.\d+$")

COPYLEFT = re.compile(r"^(A|L)?GPL-")


def is_copyleft(token: str) -> bool:
    return bool(COPYLEFT.match(token))


def tokens(expr: str):
    normalized = expr.replace("/", " OR ")
    return [t.strip() for t in re.split(r"(\(|\)|\bAND\b|\bOR\b)", normalized) if t.strip()]


def free_of_copyleft(expr: str) -> bool:
    """Whether `expr` can be met without a GPL-family licence."""
    py = ""
    for t in tokens(expr):
        if t in ("AND", "OR", "(", ")"):
            py += f" {t.lower()} "
        else:
            py += f" {not is_copyleft(t.split(' WITH ')[0].strip())} "
    try:
        return bool(eval(py, {"__builtins__": {}}))
    except Exception as exc:
        raise ValueError(f"cannot read the licence expression '{expr}': {exc}")


def linked_packages(metadata: dict) -> list:
    """The packages linked into the `crosvm` program: its normal dependencies, transitively."""
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    packages = {p["id"]: p for p in metadata["packages"]}
    root = next(p["id"] for p in metadata["packages"]
                if p["name"] == "crosvm" and p.get("source") is None)
    seen, todo = set(), [root]
    while todo:
        pid = todo.pop()
        if pid in seen:
            continue
        seen.add(pid)
        for dep in nodes[pid].get("deps", []):
            if any(k.get("kind") is None for k in dep.get("dep_kinds", [])):
                todo.append(dep["pkg"])
    return [packages[pid] for pid in sorted(seen)]


def licence_of(package: dict, source_dir: Path) -> str:
    if package.get("license"):
        return package["license"]
    manifest = Path(package["manifest_path"]).resolve()
    if package.get("source") is None and source_dir.resolve() in manifest.parents:
        # minijail's crates carry minijail's licence, every other one crosvm's.
        return "BSD-3-Clause"
    return ""


def needed_libraries(binary: Path) -> list:
    out = subprocess.run(["readelf", "-d", str(binary)], capture_output=True, text=True,
                         check=True).stdout
    return re.findall(r"\(NEEDED\)\s+Shared library: \[([^\]]+)\]", out)


def library_problems(libraries: list) -> list:
    problems = []
    for lib in libraries:
        if lib in SYSTEM_LIBRARIES or lib in SHIPPED_LIBRARIES or LOADERS.match(lib):
            continue
        problems.append(f"crosvm needs {lib}, which is neither a known system library nor one "
                        "DSA ships with a checked licence; add it with its licence")
    return problems


def crate_problems(rows: list) -> list:
    problems = []
    for name, version, expr in rows:
        if not expr:
            problems.append(f"{name} {version} names no licence")
        elif not free_of_copyleft(expr):
            problems.append(f"{name} {version} is under '{expr}', which is GPL, LGPL or AGPL; "
                            "crosvm links it, so it cannot ship")
    return problems


def notice(rows: list, libraries: list, revision: str) -> str:
    lines = [
        "crosvm and what it links",
        "========================",
        "",
        f"crosvm {revision} (BSD-3-Clause), https://chromium.googlesource.com/crosvm/crosvm",
        "",
        "Rust crates linked into crosvm:",
    ]
    lines += [f"  {name} {version}: {expr}" for name, version, expr in rows]
    lines += ["", "Shared libraries crosvm loads:"]
    for lib in libraries:
        if lib in SHIPPED_LIBRARIES:
            lines.append(f"  {lib}: {SHIPPED_LIBRARIES[lib]}, shipped beside crosvm")
        else:
            lines.append(f"  {lib}: provided by the system, not shipped")
    return "\n".join(lines) + "\n"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--features", required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()

    host = re.search(r"^host: (\S+)$", subprocess.run(
        ["rustc", "-vV"], cwd=args.source, capture_output=True, text=True, check=True).stdout,
        re.M).group(1)
    metadata = json.loads(subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--no-default-features",
         "--features", args.features, "--filter-platform", host],
        cwd=args.source, capture_output=True, text=True, check=True).stdout)
    rows = sorted((p["name"], p["version"], licence_of(p, args.source))
                  for p in linked_packages(metadata))
    libraries = needed_libraries(args.binary)
    problems = crate_problems(rows) + library_problems(libraries)
    if problems:
        print("The shipped crosvm cannot be distributed with DSA:", file=sys.stderr)
        for problem in problems:
            print(f"  {problem}", file=sys.stderr)
        return 1
    revision = subprocess.run(["git", "-C", str(args.source), "rev-parse", "HEAD"],
                              capture_output=True, text=True, check=True).stdout.strip()
    args.out.write_text(notice(rows, libraries, revision), encoding="utf-8")
    print(f"crosvm links {len(rows)} crates and {len(libraries)} shared libraries, none copyleft")
    return 0


if __name__ == "__main__":
    sys.exit(main())
