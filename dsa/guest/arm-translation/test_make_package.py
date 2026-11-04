"""Tests for the ARM translation package maker."""

import importlib.util
import os
import sys
import tempfile
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location("make_package", Path(__file__).with_name("make-package.py"))
make_package = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(make_package)


def bundle(root):
    system = Path(root) / "bundle" / "system"
    for name in ["lib64/libberberis_arm64.so", "lib64/libberberis_exec_region.so",
                 "lib64/libberberis_proxy_libEGL.so", "lib64/arm64/libc.so",
                 "bin/berberis_program_runner_arm64", "etc/binfmt_misc/arm64_exe",
                 "etc/ld.config.arm64.txt", "etc/init/berberis.rc", "framework/unused.jar"]:
        (system / name).parent.mkdir(parents=True, exist_ok=True)
        (system / name).write_text(name)
    return system.parent


class MakePackageTests(unittest.TestCase):
    def test_fr25_the_bridge_library_is_found_in_a_berberis_bundle(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(make_package.find_library(bundle(tmp) / "system"), "libberberis_arm64.so")

    @unittest.skipIf(sys.platform == "win32", "symbolic links need privileges on Windows")
    def test_fr25_the_image_tree_has_what_the_guest_lays_over_and_the_bridge_under_dsa_name(self):
        with tempfile.TemporaryDirectory() as tmp:
            staged = make_package.stage(bundle(tmp), Path(tmp) / "staging", "libberberis_arm64.so")
            self.assertTrue((staged / "lib64/arm64/libc.so").is_file())
            self.assertTrue((staged / "bin/berberis_program_runner_arm64").is_file())
            self.assertTrue((staged / "etc/binfmt_misc/arm64_exe").is_file())
            self.assertFalse((staged / "framework").exists(), "only lib64, bin and etc are laid over")
            link = staged / "lib64/libdsa_native_bridge.so"
            self.assertTrue(link.is_symlink())
            self.assertEqual(os.readlink(link), "libberberis_arm64.so")

    def test_fr25_a_bundle_without_the_library_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(SystemExit):
                make_package.stage(bundle(tmp), Path(tmp) / "staging", "libhoudini.so")

    def test_fr25_the_description_says_arm64_only_and_is_checked_as_the_host_checks_it(self):
        self.assertEqual(
            make_package.manifest("Digitalis", "17.1", "libberberis_arm64.so", "Apache-2.0"),
            {"name": "Digitalis", "version": "17.1", "library": "libberberis_arm64.so",
             "abis": ["arm64-v8a"], "license": "Apache-2.0"},
        )
        make_package.check("Digitalis", "17.1", "libberberis_arm64.so", "Apache-2.0")
        for bad in [("", "1", "l.so", ""), ("D", " 1", "l.so", ""), ("D", "1", "../l.so", ""),
                    ("D", "1", "libl", "")]:
            with self.assertRaises(SystemExit):
                make_package.check(*bad)


if __name__ == "__main__":
    unittest.main()
