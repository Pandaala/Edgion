#!/usr/bin/env python3
"""Focused regression checks for the migrated unit-test layout guard."""

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
const QUOTE: u8 = b'"';
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

    def test_controller_support_mount_and_scoped_branch(self):
        layout.CRATE = Path(self.temp.name) / "edgion-controller"
        self.crate = layout.CRATE
        self.write(
            "src/conf_mgr/conf_center/common.rs",
            "fn f() { #[cfg(test)]\nlet crd_dir = fallback; }\n",
        )
        self.write(
            "src/api/mod.rs",
            '#[cfg(test)]\n#[path = "../../tests/support/api/mod.rs"]\nmod test_support;\n',
        )
        self.write("tests/support/api/mod.rs", "pub(super) fn reset() {}\n")
        self.assertEqual(layout.check(), [])

        self.write("src/api/other.rs", "fn f() { #[cfg(test)]\nlet crd_dir = fallback; }\n")
        self.assertIn("test-only implementation remains in src", "\n".join(layout.check()))

    def test_waf_support_mount_and_exact_exception(self):
        layout.CRATE = Path(self.temp.name) / "edgion-waf"
        self.crate = layout.CRATE
        self.write(
            "src/aho_corasick/match_types.rs",
            '#[cfg(test)]\n#[path = "../../tests/support/aho_corasick/match_types.rs"]\nmod test_support;\n',
        )
        self.write("tests/support/aho_corasick/match_types.rs", "use super::*;\n")
        self.write(
            "src/seclang/semantic.rs",
            "struct Projection { #[cfg(test)] inspected_folds: usize }\n",
        )
        self.assertEqual(layout.check(), [])

        self.write("src/seclang/other.rs", "struct Projection { #[cfg(test)] inspected_folds: usize }\n")
        self.assertIn("test-only implementation remains in src", "\n".join(layout.check()))

    def test_gateway_gated_mount_support_include_and_associated_helper(self):
        layout.CRATE = Path(self.temp.name) / "edgion-gateway"
        self.crate = layout.CRATE
        self.write(
            "src/ai/routing/adaptive.rs",
            '#[cfg(all(test, unix))]\n// Keep the test gate.\n#[allow(dead_code)]\n'
            '#[path = "../../../tests/unit/ai/routing/adaptive_test.rs"]\n'
            'mod tests;\n#[cfg(test)]\ninclude!("../../../tests/support/ai/routing/adaptive_test/helper.rs");\n'
            'impl AdaptiveTelemetryStore { #[cfg(test)] fn get_or_create(&self) {} }\n',
        )
        self.write("tests/unit/ai/routing/adaptive_test.rs", "use super::*;\n")
        self.write("tests/support/ai/routing/adaptive_test/helper.rs", "fn helper() {}\n")
        self.assertEqual(layout.check(), [])

        self.write("src/ai/new.rs", "impl New { #[cfg(test)] fn unlisted_helper(&self) {} }\n")
        self.assertIn("test-only implementation remains in src", "\n".join(layout.check()))


if __name__ == "__main__":
    unittest.main()
