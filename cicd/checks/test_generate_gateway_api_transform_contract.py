#!/usr/bin/env python3
"""Focused tests for Gateway API transform-contract drift detection."""

from __future__ import annotations

import copy
from pathlib import Path
import tempfile
import unittest

import generate_gateway_api_transform_contract as generator
import yaml


class TransformContractGeneratorTests(unittest.TestCase):
    def write_mutated_standard(self, mutate) -> Path:
        documents = list(yaml.safe_load_all(generator.STANDARD.read_text(encoding="utf-8")))
        route = next(
            document
            for document in documents
            if isinstance(document, dict)
            and document.get("kind") == "CustomResourceDefinition"
            and document.get("spec", {}).get("names", {}).get("kind") == "HTTPRoute"
        )
        rule = route["spec"]["versions"][0]["schema"]["openAPIV3Schema"]["properties"]["spec"][
            "properties"
        ]["rules"]["items"]
        mutate(rule)
        temporary = tempfile.NamedTemporaryFile(mode="w", suffix=".yaml", delete=False, encoding="utf-8")
        with temporary:
            yaml.safe_dump_all(documents, temporary)
        self.addCleanup(Path(temporary.name).unlink, missing_ok=True)
        return Path(temporary.name)

    def test_new_transform_rule_is_selected_without_a_message_allowlist(self) -> None:
        node = {
            "x-kubernetes-validations": [
                {"rule": "!has(self.requestRedirect) || self.type == 'RequestRedirect'"},
                {"rule": 'self.type != "URLRewrite"'},
                {"rule": "!has(self.extensionRef) || self.type == 'ExtensionRef'"},
            ]
        }

        selected = generator.select_transform_rules(node, "test")

        self.assertEqual(selected, node["x-kubernetes-validations"][:2])

    def test_unselected_scope_rule_still_changes_the_inventory(self) -> None:
        node = {
            "x-kubernetes-validations": [
                {"rule": "!has(self.requestRedirect) || self.type == 'RequestRedirect'"}
            ]
        }
        changed = copy.deepcopy(node)
        changed["x-kubernetes-validations"].append(
            {"rule": "!has(self.extensionRef) || self.type == 'ExtensionRef'"}
        )

        self.assertEqual(
            generator.select_transform_rules(node, "before"),
            generator.select_transform_rules(changed, "after"),
        )
        self.assertNotEqual(
            generator.validation_inventory(node),
            generator.validation_inventory(changed),
        )

    def test_descriptions_do_not_create_semantic_drift(self) -> None:
        node = {
            "x-kubernetes-validations": [
                {
                    "rule": "!has(self.urlRewrite) || self.type == 'URLRewrite'",
                    "message": "invalid URLRewrite",
                    "description": "documentation only",
                }
            ]
        }

        self.assertNotIn("description", generator.validation_inventory(node)[0])

    def test_rule_and_backend_list_bound_drift_is_rejected(self) -> None:
        def mutate(rule: dict) -> None:
            backend_filters = rule["properties"]["backendRefs"]["items"]["properties"]["filters"]
            backend_filters["minItems"] = 1

        path = self.write_mutated_standard(mutate)

        with self.assertRaisesRegex(RuntimeError, "filter-list bounds differ"):
            generator.selected_contract(path)

    def test_new_shared_list_bound_changes_the_generated_contract(self) -> None:
        baseline, _ = generator.selected_contract(generator.STANDARD)

        def mutate(rule: dict) -> None:
            rule["properties"]["filters"]["minItems"] = 1
            rule["properties"]["backendRefs"]["items"]["properties"]["filters"]["minItems"] = 1

        path = self.write_mutated_standard(mutate)
        changed, _ = generator.selected_contract(path)

        self.assertNotEqual(baseline, changed)
        self.assertEqual(changed["fragments"]["filterList"]["minItems"], 1)


if __name__ == "__main__":
    unittest.main()
