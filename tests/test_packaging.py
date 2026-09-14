"""Keep the initial-alpha descriptor and dynamic artifact identifiers aligned."""

import re
import tomllib
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]


class PackagingContracts(unittest.TestCase):
    """Reject artifacts that could silently use an incompatible descriptor layout."""

    def test_candidate_version_and_abi_identifiers_match(self):
        manifest = tomllib.loads((ROOT / "Cargo.toml").read_text())
        lock = tomllib.loads((ROOT / "Cargo.lock").read_text())
        self.assertEqual(manifest["package"]["version"], "0.1.0-alpha.3")
        self.assertEqual(lock["package"][0]["version"], manifest["package"]["version"])
        self.assertEqual(manifest["lib"]["crate-type"], ["cdylib"])
        makefile = (ROOT / "Makefile").read_text()
        self.assertIn("PACKAGE_VERSION ?= 0.1.0-alpha.3", makefile)
        self.assertIn("SOVERSION := 3", makefile)
        self.assertIn("libstd-", makefile)
        header = (ROOT / "include/rptadvradio/rptadvradio.h").read_text()
        self.assertRegex(header, r"#define RPTADV_RADIO_ABI_VERSION 3U\b")
        self.assertIn("abi_version=@ABI_VERSION@", (ROOT / "rptadvradio.pc.in").read_text())

    def test_runtime_package_contains_only_the_current_soname(self):
        control = (ROOT / "debian/control").read_text()
        self.assertEqual(re.findall(r"^Package: (.+)$", control, re.MULTILINE),
                         ["librptadvradio3", "librptadvradio-dev"])
        self.assertIn("librptadvradio3 (= ${binary:Version})", control)
        self.assertFalse((ROOT / "debian/librptadvradio1.install").exists())
        self.assertFalse((ROOT / "debian/librptadvradio2.install").exists())
        self.assertEqual((ROOT / "debian/librptadvradio3.install").read_text().strip(),
                         "usr/lib/*/librptadvradio.so.3*")
        self.assertTrue((ROOT / "debian/changelog").read_text().startswith(
            "librptadvradio (0.1.0~alpha3-1)"))


if __name__ == "__main__":
    unittest.main()
