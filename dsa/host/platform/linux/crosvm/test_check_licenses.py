# /// script
# requires-python = ">=3.11"
# ///
"""Tests for the licence check of the shipped crosvm (check-licenses.py), on made-up crates,
dependency graphs and shared libraries."""

import importlib.util
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("check_licenses", HERE / "check-licenses.py")
cl = importlib.util.module_from_spec(spec)
spec.loader.exec_module(cl)


def metadata():
    """crosvm needs `base` and `cc` (a build dependency); `base` needs `gplthing` only in tests."""
    def pkg(name, source="registry", license="MIT"):
        return {"id": name, "name": name, "version": "1.0.0", "source": source,
                "license": license, "manifest_path": f"/src/{name}/Cargo.toml"}
    packages = [pkg("crosvm", None, None), pkg("base", None, None), pkg("cc"),
                pkg("gplthing", license="GPL-2.0-only")]
    normal, build, dev = [{"kind": None}], [{"kind": "build"}], [{"kind": "dev"}]
    nodes = [
        {"id": "crosvm", "deps": [{"pkg": "base", "dep_kinds": normal},
                                  {"pkg": "cc", "dep_kinds": build}]},
        {"id": "base", "deps": [{"pkg": "gplthing", "dep_kinds": dev}]},
        {"id": "cc", "deps": []},
        {"id": "gplthing", "deps": []},
    ]
    return {"packages": packages, "resolve": {"nodes": nodes}}


class CrosvmLicences(unittest.TestCase):
    def test_nfr7_permissive_and_dual_licences_ship(self):
        self.assertTrue(cl.free_of_copyleft("MIT OR Apache-2.0"))
        self.assertTrue(cl.free_of_copyleft("MIT OR GPL-2.0-only"))
        self.assertTrue(cl.free_of_copyleft("Apache-2.0 WITH LLVM-exception"))
        self.assertTrue(cl.free_of_copyleft("BSD-3-Clause"))

    def test_nfr7_gpl_lgpl_and_agpl_crates_fail_the_check(self):
        for expr in ["GPL-2.0-only", "LGPL-2.1-or-later", "AGPL-3.0-only",
                     "MIT AND GPL-3.0-or-later", "GPL-2.0-only WITH Classpath-exception-2.0"]:
            self.assertFalse(cl.free_of_copyleft(expr), expr)
        problems = cl.crate_problems([("thing", "1.0.0", "LGPL-2.1-only"), ("ok", "1", "MIT")])
        self.assertEqual(len(problems), 1)
        self.assertIn("thing 1.0.0", problems[0])

    def test_nfr7_a_crate_without_a_licence_fails_the_check(self):
        self.assertIn("names no licence", cl.crate_problems([("mystery", "0.1.0", "")])[0])

    def test_nfr7_only_linked_dependencies_are_counted(self):
        names = [p["name"] for p in cl.linked_packages(metadata())]
        self.assertEqual(names, ["base", "crosvm"])

    def test_nfr7_crosvms_own_crates_carry_its_licence(self):
        crate = {"name": "base", "license": None, "source": None,
                 "manifest_path": "/build/crosvm/base/Cargo.toml"}
        self.assertEqual(cl.licence_of(crate, Path("/build/crosvm")), "BSD-3-Clause")
        outside = dict(crate, manifest_path="/elsewhere/base/Cargo.toml")
        self.assertEqual(cl.licence_of(outside, Path("/build/crosvm")), "")

    def test_nfr7_unknown_shared_libraries_fail_the_check(self):
        self.assertEqual(cl.library_problems(
            ["libc.so.6", "ld-linux-x86-64.so.2", "libgfxstream_backend.so"]), [])
        problems = cl.library_problems(["libreadline.so.8"])
        self.assertEqual(len(problems), 1)
        self.assertIn("libreadline.so.8", problems[0])

    def test_nfr7_the_notice_lists_every_crate_and_library(self):
        text = cl.notice([("base", "0.1.0", "BSD-3-Clause")],
                         ["libc.so.6", "libgfxstream_backend.so"], "abc")
        self.assertIn("crosvm abc (BSD-3-Clause)", text)
        self.assertIn("base 0.1.0: BSD-3-Clause", text)
        self.assertIn("libgfxstream_backend.so: Apache-2.0, shipped beside crosvm", text)
        self.assertIn("libc.so.6: provided by the system, not shipped", text)


if __name__ == "__main__":
    unittest.main()
