"""Keep the initial-alpha descriptor and dynamic artifact identifiers aligned."""

import re
import subprocess
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class PackagingContracts(unittest.TestCase):
    """Reject artifacts that could silently use an incompatible descriptor layout."""

    def test_candidate_version_and_abi_identifiers_match(self):
        manifest = tomllib.loads((ROOT / "Cargo.toml").read_text())
        lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
        self.assertEqual(manifest["package"]["version"], "0.1.0-alpha.7")
        self.assertEqual(lock["package"][0]["version"], manifest["package"]["version"])
        self.assertEqual(manifest["lib"]["crate-type"], ["cdylib"])
        makefile = (ROOT / "Makefile").read_text()
        self.assertIn("PACKAGE_VERSION ?= 0.1.0-alpha.7", makefile)
        self.assertIn("SOVERSION := 4", makefile)
        self.assertIn("libstd-", makefile)
        header = (ROOT / "include/rptadvradio/rptadvradio.h").read_text()
        self.assertRegex(header, r"#define RPTADV_RADIO_ABI_VERSION 4U\b")
        self.assertIn("abi_version=@ABI_VERSION@", (ROOT / "rptadvradio.pc.in").read_text())

    def test_runtime_package_contains_only_the_current_soname(self):
        control = (ROOT / "debian/control").read_text()
        self.assertEqual(re.findall(r"^Package: (.+)$", control, re.MULTILINE),
                         ["librptadvradio4", "librptadvradio-dev"])
        self.assertIn("librptadvradio4 (= ${binary:Version})", control)
        self.assertIn("librptadvsquelch-dev (>= 0.1.0~alpha1-1)", control)
        self.assertIn("librptadvsquelch1 (>= 0.1.0~alpha1-1)", control)
        self.assertIn("librptadvctcss-dev (>= 0.1.0~alpha1-1)", control)
        self.assertIn("librptadvctcss1 (>= 0.1.0~alpha1-1)", control)
        self.assertIn("librptadvdcs-dev (>= 0.1.0~alpha1-1)", control)
        self.assertIn("librptadvdcs1 (>= 0.1.0~alpha1-1)", control)
        self.assertFalse((ROOT / "debian/librptadvradio1.install").exists())
        self.assertFalse((ROOT / "debian/librptadvradio2.install").exists())
        self.assertFalse((ROOT / "debian/librptadvradio3.install").exists())
        self.assertEqual((ROOT / "debian/librptadvradio4.install").read_text().strip(),
                         "usr/lib/*/librptadvradio.so.4*")
        self.assertTrue((ROOT / "debian/changelog").read_text().startswith(
            "librptadvradio (0.1.0~alpha7-1)"))

    def test_released_squelch_is_a_resolved_dynamic_dependency(self):
        subprocess.run(["make", "all"], cwd=ROOT, check=True,
                       capture_output=True, text=True)
        library = ROOT / "build" / "librptadvradio.so.4"
        dynamic = subprocess.check_output(["readelf", "-d", str(library)], text=True)
        self.assertIn("Shared library: [librptadvsquelch.so.1]", dynamic)
        self.assertNotRegex(dynamic, r"RPATH|RUNPATH")
        resolved = subprocess.check_output(["ldd", str(library)], text=True)
        self.assertRegex(resolved, r"librptadvsquelch\.so\.1 => /")
        self.assertNotIn("not found", resolved)
        self.assertNotIn(str(ROOT / "build"), resolved)

    def test_released_ctcss_is_a_resolved_dynamic_dependency(self):
        subprocess.run(["make", "all"], cwd=ROOT, check=True,
                       capture_output=True, text=True)
        library = ROOT / "build" / "librptadvradio.so.4"
        dynamic = subprocess.check_output(["readelf", "-d", str(library)], text=True)
        self.assertIn("Shared library: [librptadvctcss.so.1]", dynamic)
        self.assertNotRegex(dynamic, r"RPATH|RUNPATH")
        resolved = subprocess.check_output(["ldd", str(library)], text=True)
        self.assertRegex(resolved, r"librptadvctcss\.so\.1 => /")
        self.assertNotIn("not found", resolved)
        self.assertNotIn(str(ROOT / "build"), resolved)

    def test_released_dcs_is_a_resolved_dynamic_dependency(self):
        subprocess.run(["make", "all"], cwd=ROOT, check=True,
                       capture_output=True, text=True)
        library = ROOT / "build" / "librptadvradio.so.4"
        dynamic = subprocess.check_output(["readelf", "-d", str(library)], text=True)
        self.assertIn("Shared library: [librptadvdcs.so.1]", dynamic)
        self.assertNotRegex(dynamic, r"RPATH|RUNPATH")
        resolved = subprocess.check_output(["ldd", str(library)], text=True)
        self.assertRegex(resolved, r"librptadvdcs\.so\.1 => /")
        self.assertNotIn("not found", resolved)
        self.assertNotIn(str(ROOT / "build"), resolved)


if __name__ == "__main__":
    unittest.main()
