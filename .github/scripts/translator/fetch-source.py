"""Fetches the reduced AOSP tree one Digitalis version is built from, as
dsa/guest/arm-translation/digitalis-lock.json pins it.

    fetch-source.py --android android17 --top <tree> [--jobs 8]

Instead of repo, each project is fetched at its pinned commit alone: one commit deep, and for
a project the lock fetches in part, with blobs left on the server until git's sparse checkout
asks for the ones it keeps. That is what keeps the tree to a few gigabytes. The manifest's
linkfile and copyfile entries are then made as repo makes them, and each project's .git is
dropped, since the build reads none of them; <tree>/.dsa-sources.json says what was fetched.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

REPO = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO / "dsa/guest/arm-translation"))
import digitalis_lock  # noqa: E402

# What the {clang} entry needs: the build's own word on its Clang.
FIRST = ["build/soong", "build/release"]


def git(*args, cwd):
    subprocess.run(["git", "-c", "advice.detachedHead=false", *args], cwd=cwd, check=True)


def fetch(top, project, clang):
    path = top / project["path"]
    if path.exists():
        shutil.rmtree(path)
    path.mkdir(parents=True)
    mode, entries = digitalis_lock.sparse_entries(project, clang)
    for attempt in range(4):
        try:
            if (path / ".git").exists():
                shutil.rmtree(path / ".git")
            git("init", "-q", cwd=path)
            git("remote", "add", "origin", project["url"], cwd=path)
            fetch_args = ["fetch", "-q", "--depth=1", "--no-tags"]
            if mode:
                git("config", "core.sparseCheckout", "true", cwd=path)
                git("sparse-checkout", "set", "--cone" if mode == "cone" else "--no-cone",
                    *entries, cwd=path)
                fetch_args.append("--filter=blob:none")
            git(*fetch_args, "origin", project["commit"], cwd=path)
            git("checkout", "-q", "--detach", "FETCH_HEAD", cwd=path)
            break
        except subprocess.CalledProcessError:
            if attempt == 3:
                raise
            wait = 15 * (attempt + 1)
            print(f"{project['path']}: fetch failed, again in {wait} s", flush=True)
            time.sleep(wait)
    shutil.rmtree(path / ".git")
    print(f"{project['path']} {project['commit'][:12]}"
          + (f" ({mode}: {' '.join(entries)})" if mode else ""), flush=True)


def link_files(top, project):
    """The manifest's linkfile (a relative symbolic link) and copyfile entries."""
    for entry in project.get("linkfiles", []):
        source = top / project["path"] / entry["src"]
        dest = top / entry["dest"]
        if not source.exists():
            print(f"{project['path']}: skipping the link to {entry['src']}, which is not fetched")
            continue
        dest.parent.mkdir(parents=True, exist_ok=True)
        if dest.is_symlink() or dest.exists():
            dest.unlink()
        dest.symlink_to(os.path.relpath(source, dest.parent))
    for entry in project.get("copyfiles", []):
        source = top / project["path"] / entry["src"]
        if source.is_file():
            (top / entry["dest"]).parent.mkdir(parents=True, exist_ok=True)
            shutil.copy(source, top / entry["dest"])


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--android", required=True)
    parser.add_argument("--top", type=Path, required=True)
    parser.add_argument("--jobs", type=int, default=8)
    parser.add_argument("--lock", type=Path, default=digitalis_lock.LOCK)
    args = parser.parse_args()
    version = digitalis_lock.version_entry(digitalis_lock.load(args.lock), args.android)
    top = args.top.resolve()
    top.mkdir(parents=True, exist_ok=True)
    projects = version["projects"]

    first = [p for p in projects if p["path"] in FIRST]
    for project in first:
        fetch(top, project, ())
    clang = digitalis_lock.clang_versions(top)
    print(f"the build uses {', '.join(clang)}", flush=True)
    rest = [p for p in projects if p["path"] not in FIRST]
    # The large prebuilts first, so they do not finish last on their own.
    rest.sort(key=lambda p: not p["path"].startswith("prebuilts/"))
    with ThreadPoolExecutor(args.jobs) as pool:
        for future in [pool.submit(fetch, top, p, clang) for p in rest]:
            future.result()
    for project in projects:
        link_files(top, project)
    sources = {
        "android": version["android"],
        "manifest": version["manifest"],
        "superproject": version["superproject"],
        "clang": clang,
        "projects": {p["path"]: {"url": p["url"], "commit": p["commit"]} for p in projects},
    }
    (top / ".dsa-sources.json").write_text(json.dumps(sources, indent=2) + "\n")


if __name__ == "__main__":
    sys.exit(main())
