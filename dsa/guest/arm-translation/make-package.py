"""Makes an ARM translation package that `dsa arm-translation install` takes.

A translator's binary bundle (such as the one build-digitalis.sh leaves) has a `system/`
tree: the native bridge library and its proxies in system/lib64, the ARM64 guest libraries in
system/lib64/arm64, the program runners in system/bin, and the linker configuration and
binfmt_misc rules in system/etc. The package is a folder holding

    arm-translation.json   what the package is: name, version, bridge library, ABIs, licence
    system.img             that tree as an erofs image, which the guest lays over its /system

The guest's properties name the bridge `libdsa_native_bridge.so`, whatever the translator
calls it, so the image adds that name as a link to the bridge library.

    uv run dsa/guest/arm-translation/make-package.py --bundle <bundle dir> \\
        --name Digitalis --version <version> [--library libberberis_arm64.so] \\
        [--license Apache-2.0] --out <package dir> [--mkfs-erofs mkfs.erofs]

It needs mkfs.erofs (erofs-utils).
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

MANIFEST = "arm-translation.json"
IMAGE = "system.img"
BRIDGE_LINK = "libdsa_native_bridge.so"
ABIS = ["arm64-v8a"]
# The directories the guest lays the image over; anything else in a bundle is left out.
LAID_OVER = ["lib64", "bin", "etc"]
LIBRARY_NAME = re.compile(r"^[A-Za-z0-9._-]+\.so$")


def find_library(system):
    """The bridge library in a bundle's system tree: the one name Berberis builds use."""
    for candidate in sorted((system / "lib64").glob("libberberis_*.so")):
        if candidate.name not in ("libberberis_exec_region.so",) and not candidate.name.startswith(
                "libberberis_proxy_"):
            return candidate.name
    return None


def check(name, version, library, license_id):
    """Refuses what the host would refuse: the same rules as its package check."""
    for what, value, longest in [("name", name, 64), ("version", version, 64)]:
        if not value or len(value) > longest or value.strip() != value:
            raise SystemExit(f"the package's {what} must have from 1 to {longest} characters, "
                             "without spaces at either end")
    if license_id and len(license_id) > 128:
        raise SystemExit("the licence is longer than 128 characters")
    if not LIBRARY_NAME.match(library) or len(library) > 128:
        raise SystemExit(f"{library!r} is not a library file name")


def stage(bundle, staging, library):
    """Copies the parts of `bundle`'s system tree the guest uses into `staging`/system, and adds
    the bridge's DSA name."""
    system = Path(bundle) / "system"
    if not (system / "lib64" / library).is_file():
        raise SystemExit(f"{system / 'lib64' / library} is missing; is {bundle} a translator's bundle?")
    target = Path(staging) / "system"
    for directory in LAID_OVER:
        source = system / directory
        if source.is_dir():
            shutil.copytree(source, target / directory, symlinks=True)
    link = target / "lib64" / BRIDGE_LINK
    if link.exists() or link.is_symlink():
        link.unlink()
    os.symlink(library, link)
    return target


def manifest(name, version, library, license_id):
    return {"name": name, "version": version, "library": library, "abis": ABIS, "license": license_id}


def make_image(mkfs, staging, image):
    """The staged tree as an erofs image, with every file owned by root and readable by all."""
    subprocess.run([mkfs, "--all-root", "-zlz4hc", str(image), str(staging)], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--bundle", type=Path, required=True)
    parser.add_argument("--name", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--library", help="the bridge library in system/lib64 (found when not given)")
    parser.add_argument("--license", default="Apache-2.0")
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--mkfs-erofs", default="mkfs.erofs")
    args = parser.parse_args()

    library = args.library or find_library(args.bundle / "system")
    if library is None:
        raise SystemExit(f"no libberberis_*.so in {args.bundle / 'system/lib64'}; name it with --library")
    check(args.name, args.version, library, args.license)
    if shutil.which(args.mkfs_erofs) is None and not Path(args.mkfs_erofs).is_file():
        raise SystemExit(f"{args.mkfs_erofs} is not found; install erofs-utils")
    args.out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=args.out) as tmp:
        stage(args.bundle, tmp, library)
        make_image(args.mkfs_erofs, tmp, args.out / IMAGE)
    (args.out / MANIFEST).write_text(
        json.dumps(manifest(args.name, args.version, library, args.license), indent=2) + "\n")
    print(f"{args.out}: {args.name} {args.version}, bridge {library}, "
          f"{(args.out / IMAGE).stat().st_size >> 20} MiB image")


if __name__ == "__main__":
    sys.exit(main())
