"""Tests for the translator build's lock (digitalis-lock.json) and its reader."""

import copy
import importlib.util
import json
import re
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import digitalis_lock  # noqa: E402

REPO = HERE.parents[2]
DEVICE = REPO / ".github/scripts/translator/device"

SPEC = importlib.util.spec_from_file_location("assemble", HERE / "assemble-digitalis.py")
assemble = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(assemble)


class LockTests(unittest.TestCase):
    def setUp(self):
        self.lock = digitalis_lock.load()

    def test_fr25_the_lock_pins_android16_and_android17_to_exact_commits(self):
        self.assertEqual(sorted(v["android"] for v in self.lock["versions"]), ["android16", "android17"])
        for version in self.lock["versions"]:
            self.assertEqual(version["manifest"]["url"], "https://github.com/DigitalisX64/manifest")
            self.assertEqual(version["manifest"]["branch"], version["android"])
            for project in version["projects"]:
                self.assertRegex(project["commit"], r"^[0-9a-f]{40}$", project["path"])

    def test_fr25_the_translator_comes_from_digitalis_and_names_the_package_version(self):
        for version in self.lock["versions"]:
            translator = digitalis_lock.project(version, digitalis_lock.TRANSLATOR_PATH)
            self.assertTrue(translator["url"].startswith("https://github.com/DigitalisX64/"))
            self.assertEqual(digitalis_lock.translator_version(version),
                             f"{version['android_major']}-{translator['commit'][:12]}")
            self.assertLessEqual(len(digitalis_lock.translator_version(version)), 64)

    def test_fr25_the_build_fetches_what_it_cannot_do_without(self):
        # The build system, the toolchain, the translator, bionic and the native bridge
        # support; the AOSP projects come from Google's servers over https.
        needed = ["build/make", "build/soong", "build/blueprint", "build/release",
                  "prebuilts/clang/host/linux-x86", "prebuilts/build-tools", "prebuilts/go/linux-x86",
                  "bionic", "frameworks/libs/native_bridge_support", digitalis_lock.TRANSLATOR_PATH]
        for version in self.lock["versions"]:
            paths = [p["path"] for p in version["projects"]]
            for path in needed:
                self.assertIn(path, paths, f"{version['android']} lacks {path}")
            for project in version["projects"]:
                if project["path"] not in (digitalis_lock.TRANSLATOR_PATH, "digitalis"):
                    self.assertTrue(project["url"].startswith("https://android.googlesource.com/"),
                                    project["path"])

    def test_fr25_the_large_prebuilts_are_fetched_in_part(self):
        for version in self.lock["versions"]:
            clang = digitalis_lock.project(version, "prebuilts/clang/host/linux-x86")
            self.assertIn(digitalis_lock.CLANG_TOKEN, clang["cone"])
            # A header-only project leaves its build files out, so the build sees none of its
            # modules (the framework libraries come from the image instead).
            for path in ("frameworks/native", "frameworks/base", "frameworks/av"):
                patterns = digitalis_lock.project(version, path)["patterns"]
                self.assertIn("!Android.bp", patterns)
                self.assertTrue(all(p.startswith(("/", "!")) for p in patterns))

    def test_fr25_a_lock_that_does_not_pin_or_escapes_the_tree_is_refused(self):
        version = self.lock["versions"][0]
        broken = []
        for change in [
            lambda v: v["projects"][0].update(commit="main"),
            lambda v: v["projects"][0].update(path="../outside"),
            lambda v: v["projects"][0].update(url="git@github.com:x/y"),
            lambda v: v["projects"].append(copy.deepcopy(v["projects"][0])),
            lambda v: v["projects"][0].update(cone=["a"], patterns=["/b/"]),
            lambda v: v["projects"][0].update(linkfiles=[{"src": "a", "dest": "/etc/x"}]),
            lambda v: v.update(android_major=v["android_major"] + 1),
            lambda v: v["manifest"].update(commit=""),
            lambda v: v.update(projects=[p for p in v["projects"]
                                         if p["path"] != digitalis_lock.TRANSLATOR_PATH]),
        ]:
            lock = {"versions": [copy.deepcopy(version)]}
            change(lock["versions"][0])
            with self.assertRaises(digitalis_lock.LockError):
                digitalis_lock.check(lock)
            broken.append(lock)
        self.assertEqual(len(broken), 9)

    def test_fr25_clang_entries_take_the_release_configuration_s_clang(self):
        with tempfile.TemporaryDirectory() as tmp:
            top = Path(tmp)
            (top / "build/soong/cc/config").mkdir(parents=True)
            (top / "build/soong/cc/config/global.go").write_text('\tClangDefaultVersion = "clang-r584948"\n')
            self.assertEqual(digitalis_lock.clang_versions(top), ["clang-r584948"])
            flag = top / "build/release/flag_values/trunk_staging"
            flag.mkdir(parents=True)
            (flag / "RELEASE_BUILD_CLANG_VERSION.textproto").write_text(
                'name: "RELEASE_BUILD_CLANG_VERSION"\nvalue: {\n  string_value: "clang-r584948b"\n}\n')
            self.assertEqual(digitalis_lock.clang_versions(top), ["clang-r584948b"])
        project = {"path": "prebuilts/clang/host/linux-x86", "cone": ["soong", "{clang}"]}
        self.assertEqual(digitalis_lock.sparse_entries(project, ["clang-r1"]), ("cone", ["soong", "clang-r1"]))
        with self.assertRaises(digitalis_lock.LockError):
            digitalis_lock.sparse_entries(project, [])
        self.assertEqual(digitalis_lock.sparse_entries({"path": "bionic"}), (None, []))

    def test_fr25_rust_entries_take_soong_s_rust_toolchain(self):
        # Bionic's libc and linker link Rust (icu4x, the no_std allocator and panic handler),
        # so the build needs the one Rust toolchain Soong names, and nothing more.
        with tempfile.TemporaryDirectory() as tmp:
            top = Path(tmp)
            self.assertEqual(digitalis_lock.rust_versions(top), [])
            (top / "build/soong/rust/config").mkdir(parents=True)
            (top / "build/soong/rust/config/global.go").write_text('\tRustDefaultVersion = "1.93.1"\n')
            self.assertEqual(digitalis_lock.rust_versions(top), ["1.93.1"])
        project = {"path": "prebuilts/rust", "cone": ["soong", "linux-x86/{rust}"]}
        self.assertEqual(digitalis_lock.sparse_entries(project, ["clang-r1"], ["1.88.0"]),
                         ("cone", ["soong", "linux-x86/1.88.0"]))
        with self.assertRaises(digitalis_lock.LockError):
            digitalis_lock.sparse_entries(project, ["clang-r1"], [])
        for version in self.lock["versions"]:
            rust = [p for p in version["projects"] if digitalis_lock.RUST_TOKEN in "".join(p.get("cone", []))]
            self.assertEqual(len(rust), 1, version["android"])

    def test_fr25_soong_ui_s_go_packages_are_fetched(self):
        # soong_ui is built from these before anything else; one missing stops the build at
        # its first step (external/pogreb did in the first run).
        for version in self.lock["versions"]:
            paths = [p["path"] for p in version["projects"]]
            for path in ("external/golang-protobuf", "external/starlark-go", "external/pogreb"):
                self.assertIn(path, paths, f"{version['android']} lacks {path}")

    def test_fr25_the_prebuilt_framework_libraries_are_the_ones_taken_from_the_image(self):
        text = (DEVICE / "prebuilts/Android.bp").read_text()
        names = re.findall(r'cc_prebuilt_library_shared \{\s*name: "([^"]+)"', text)
        self.assertEqual(sorted(f"{n}.so" for n in names), sorted(assemble.FRAMEWORK_LIBS))
        for name in names:
            self.assertIn(f'srcs: ["x86_64/{name}.so"]', text)

    def test_fr25_the_build_product_is_x86_64_only_with_an_arm64_bridge(self):
        board = (DEVICE / "BoardConfig.mk").read_text()
        self.assertIn("TARGET_ARCH := x86_64", board)
        self.assertNotIn("TARGET_2ND_ARCH", board)
        self.assertIn("TARGET_NATIVE_BRIDGE_ARCH := arm64", board)
        self.assertIn("TARGET_NATIVE_BRIDGE_ABI := arm64-v8a", board)
        product = (DEVICE / "dsa_translator.mk").read_text()
        self.assertIn("soong_config_set,berberis,translation_arch,arm64_to_x86_64", product)


if __name__ == "__main__":
    unittest.main()
