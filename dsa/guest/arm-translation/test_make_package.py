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


# Berberis' own rules (frameworks/libs/binary_translation/prebuilt/system/etc/binfmt_misc).
RULE_EXE = (":arm64_exe:M::\\x7fELF\\x02\\x01\\x01\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x02\\x00\\xb7::"
            "/system/bin/berberis_program_runner_binfmt_misc_arm64:P\n")
RULE_DYN = (":arm64_dyn:M::\\x7fELF\\x02\\x01\\x01\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x00\\x03\\x00\\xb7::"
            "/system/bin/berberis_program_runner_binfmt_misc_arm64:P")


def bundle(root, leave_out=()):
    system = Path(root) / "bundle" / "system"
    for name in ["lib64/libberberis_arm64.so", "lib64/libberberis_exec_region.so",
                 "lib64/libberberis_proxy_libEGL.so", "lib64/arm64/libc.so",
                 "bin/berberis_program_runner_arm64", "bin/berberis_program_runner_binfmt_misc_arm64",
                 "bin/arm64/linker64", "etc/ld.config.arm64.txt", "etc/init/berberis.rc",
                 "framework/unused.jar"]:
        if name in leave_out:
            continue
        (system / name).parent.mkdir(parents=True, exist_ok=True)
        (system / name).write_text(name)
    for name, text in [("arm64_exe", RULE_EXE), ("arm64_dyn", RULE_DYN)]:
        (system / "etc/binfmt_misc").mkdir(parents=True, exist_ok=True)
        (system / "etc/binfmt_misc" / name).write_text(text)
    return system.parent


class MakePackageTests(unittest.TestCase):
    def test_fr25_the_bridge_library_is_found_in_a_berberis_bundle(self):
        with tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(make_package.find_library(bundle(tmp) / "system"), "libberberis_arm64.so")

    @unittest.skipIf(sys.platform == "win32", "symbolic links need privileges on Windows")
    def test_fr25_the_image_tree_has_what_the_guest_binds_and_the_bridge_under_dsa_name(self):
        with tempfile.TemporaryDirectory() as tmp:
            staged = make_package.stage(bundle(tmp), Path(tmp) / "staging", "libberberis_arm64.so")
            self.assertTrue((staged / "lib64/arm64/libc.so").is_file())
            self.assertTrue((staged / "bin/arm64/linker64").is_file())
            self.assertTrue((staged / "bin/berberis_program_runner_arm64").is_file())
            self.assertTrue((staged / "etc/ld.config.arm64.txt").is_file())
            self.assertFalse((staged / "framework").exists(), "only lib64, bin and etc are taken")
            link = staged / "lib64/libdsa_native_bridge.so"
            self.assertTrue(link.is_symlink())
            self.assertEqual(os.readlink(link), "libberberis_arm64.so")

    RUNNER = "/system/bin/dsa-arm-translation/berberis_program_runner_binfmt_misc_arm64"

    @unittest.skipIf(sys.platform == "win32", "symbolic links need privileges on Windows")
    def test_fr25_the_binfmt_misc_rules_start_the_runner_where_the_guest_binds_it(self):
        # The guest binds the package's system/bin onto /system/bin/dsa-arm-translation, so a
        # rule naming /system/bin/<runner> would start nothing.
        with tempfile.TemporaryDirectory() as tmp:
            staged = make_package.stage(bundle(tmp), Path(tmp) / "staging", "libberberis_arm64.so")
            exe = (staged / "etc/binfmt_misc/arm64_exe").read_text()
            dyn = (staged / "etc/binfmt_misc/arm64_dyn").read_text()
        self.assertEqual(exe.split(":")[6], self.RUNNER)
        self.assertEqual(dyn.split(":")[6], self.RUNNER)

    def test_fr25_a_rule_is_pointed_at_the_bound_runners_and_keeps_the_rest(self):
        exe = make_package.bound_rule(RULE_EXE)
        self.assertEqual(exe, RULE_EXE.replace("/system/bin/", "/system/bin/dsa-arm-translation/"))
        self.assertEqual(exe.split(":")[6], self.RUNNER)
        dyn = make_package.bound_rule(RULE_DYN)
        self.assertEqual(dyn.split(":")[6], self.RUNNER)
        self.assertFalse(dyn.endswith("\n"), "the rule keeps its own ending")
        # A rule already pointing there is kept, and one outside system/bin is refused.
        self.assertEqual(make_package.bound_rule(exe), exe)
        for bad in [":a:M::\\x7f::/vendor/bin/runner:P\n", ":a:M::\\x7f::/system/bin/x/runner:P\n", "a:b"]:
            with self.assertRaises(SystemExit, msg=bad):
                make_package.bound_rule(bad)

    def test_fr25_a_bundle_without_the_library_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            with self.assertRaises(SystemExit):
                make_package.stage(bundle(tmp), Path(tmp) / "staging", "libhoudini.so")

    def test_fr25_a_bundle_without_what_the_guest_binds_is_refused(self):
        missing_parts = ["lib64/arm64/libc.so", "bin/arm64/linker64", "etc/ld.config.arm64.txt"]
        if sys.platform != "win32":
            # Found once the bridge's link is made, which needs privileges on Windows.
            missing_parts.append("bin/berberis_program_runner_binfmt_misc_arm64")
        for missing in missing_parts:
            with tempfile.TemporaryDirectory() as tmp:
                with self.assertRaises(SystemExit, msg=missing):
                    make_package.stage(bundle(tmp, leave_out=[missing]), Path(tmp) / "staging",
                                       "libberberis_arm64.so")

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
