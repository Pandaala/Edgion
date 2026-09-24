#!/usr/bin/env python3
"""Focused regression checks for the pilot unit-test layout guard."""

import importlib.util
from pathlib import Path
import tempfile
import unittest

SPEC = importlib.util.spec_from_file_location("layout", Path(__file__).with_name("check_unit_test_layout.py"))
layout = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(layout)


class LayoutTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.crate = Path(self.temp.name) / "edgion-common"
        self.original_crate = layout.CRATE
        layout.CRATE = self.crate
        self.addCleanup(setattr, layout, "CRATE", self.original_crate)

    def write(self, name, content):
        path = self.crate / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)

    def test_mounted_mod_and_named_modules(self):
        self.write("src/dns/mod.rs", '''// #[cfg(test)] mod false_positive { }
const SAMPLE: &str = r#"#[test]"#;
#[cfg(test)]
#[path = "../../tests/unit/dns/mod_test.rs"]
mod tests;
#[cfg(test)]
#[path = "../../tests/unit/dns/mod_test/extra.rs"]
mod extra;
''')
        self.write("tests/unit/dns/mod_test.rs", "use super::*;\n")
        self.write("tests/unit/dns/mod_test/extra.rs", "use super::*;\n")
        self.assertEqual(layout.check(), [])

    def test_inline_missing_and_orphan_are_rejected(self):
        self.write("src/foo.rs", "#[cfg(test)]\nmod tests { #[test] fn case() {} }\n")
        self.write("src/bar.rs", '#[cfg(test)]\n#[path = "../tests/unit/bar_test.rs"]\nmod tests;\n')
        self.write("tests/unit/orphan_test.rs", "use super::*;\n")
        messages = "\n".join(layout.check())
        self.assertIn("inline test module", messages)
        self.assertIn("missing", messages)
        self.assertIn("orphan", messages)

    def test_test_attribute_in_source_is_rejected(self):
        self.write("src/foo.rs", "#[test]\nfn escaped_case() {}\n")
        self.assertIn("test attribute remains in src", "\n".join(layout.check()))


if __name__ == "__main__":
    unittest.main()
