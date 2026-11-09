"""Makes an ARM translation package that `dsa arm-translation install` takes.

A translator's binary bundle (such as the one build-digitalis.sh leaves) has a `system/`
tree: the native bridge library and its proxies in system/lib64, the ARM64 guest libraries in
system/lib64/arm64, the ARM64 linker and app_process in system/bin/arm64, the program runners
in system/bin, and the linker configuration and binfmt_misc rules in system/etc. The package is
a folder holding

    arm-translation.json   what the package is: name, version, bridge library, ABIs, licence
    system.img             that tree as an erofs image

The guest mounts the image at /mnt/dsa-arm-translation and binds parts of it onto empty places
of its own (system/lib64 onto /system_ext/lib64, system/lib64/arm64 and system/bin/arm64 onto
the same paths under /system, system/bin onto /system/bin/dsa-arm-translation, and
system/etc/ld.config.arm64.txt onto the same path), so a package needs each of them. It also
binds a Berberis bundle's system/lib64/libberberis_exec_region.so onto that same path: Berberis
opens it by that absolute path, so without it every ARM app aborts as it starts. The
guest's properties name the bridge `libdsa_native_bridge.so`, whatever the translator calls it,
so the image adds that name as a link to the bridge library. Each binfmt_misc rule names its
runner where the guest binds it, under /system/bin/dsa-arm-translation.

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
# The folders of a bundle the image takes; anything else in a bundle is left out.
TAKEN = ["lib64", "bin", "etc"]
# What the guest binds onto its own empty places, so every package has it.
BOUND = ["lib64/arm64", "bin/arm64", "etc/ld.config.arm64.txt"]
# The binfmt_misc rules the guest registers, and where it binds the bundle's system/bin.
RULES = ["arm64_exe", "arm64_dyn"]
# What the guest binds onto its own empty place for a Berberis bundle (one whose bridge is
# libberberis_*.so): the library Berberis runs its translated code in, which it opens by the
# absolute path /system/lib64/libberberis_exec_region.so.
BERBERIS_BOUND = ["lib64/libberberis_exec_region.so"]
RUNNERS = "/system/bin/dsa-arm-translation/"
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


def bound_rule(rule):
    """The binfmt_misc rule `rule` (`:name:type:offset:magic:mask:interpreter:flags`) with its
    interpreter where the guest binds the bundle's system/bin. A rule already naming that place
    is kept; one naming a runner anywhere but system/bin is refused, since the guest never
    reaches it."""
    fields = rule.rstrip("\n").split(":")
    if len(fields) < 8 or fields[0] != "":
        raise SystemExit(f"{rule!r} is not a binfmt_misc rule")
    interpreter = fields[6]
    if interpreter.startswith(RUNNERS):
        name = interpreter[len(RUNNERS):]
    elif interpreter.startswith("/system/bin/"):
        name = interpreter[len("/system/bin/"):]
    else:
        raise SystemExit(f"the binfmt_misc rule {fields[1]} starts {interpreter}, outside system/bin")
    if not name or "/" in name:
        raise SystemExit(f"the binfmt_misc rule {fields[1]} starts {interpreter}, outside system/bin")
    fields[6] = RUNNERS + name
    return ":".join(fields) + ("\n" if rule.endswith("\n") else "")


def stage(bundle, staging, library):
    """Copies the parts of `bundle`'s system tree the guest uses into `staging`/system, adds the
    bridge's DSA name, and points the binfmt_misc rules at the runners where the guest binds
    them."""
    system = Path(bundle) / "system"
    if not (system / "lib64" / library).is_file():
        raise SystemExit(f"{system / 'lib64' / library} is missing; is {bundle} a translator's bundle?")
    bound = BOUND + (BERBERIS_BOUND if library.startswith("libberberis_") else [])
    for path in bound:
        if not (system / path).exists():
            raise SystemExit(f"{system / path} is missing; the guest binds it, so a package needs it")
    target = Path(staging) / "system"
    for directory in TAKEN:
        source = system / directory
        if source.is_dir():
            shutil.copytree(source, target / directory, symlinks=True)
    link = target / "lib64" / BRIDGE_LINK
    if link.exists() or link.is_symlink():
        link.unlink()
    os.symlink(library, link)
    for name in RULES:
        rule = target / "etc" / "binfmt_misc" / name
        if not rule.is_file():
            raise SystemExit(f"{system / 'etc/binfmt_misc' / name} is missing; the guest registers it")
        text = bound_rule(rule.read_text())
        runner = text.split(":")[6][len(RUNNERS):]
        if not (target / "bin" / runner).is_file():
            raise SystemExit(f"the binfmt_misc rule {name} starts {runner}, which is not in "
                             f"{system / 'bin'}")
        rule.write_text(text)
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
