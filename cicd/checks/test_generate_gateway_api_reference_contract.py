#!/usr/bin/env python3
"""Reference extraction drift gates for selected source versions and field loss."""
import json
import unittest
import generate_gateway_api_reference_contract as generator


class ReferenceContractTests(unittest.TestCase):
    def test_exact_registered_sources_remain_independent(self):
        contracts = json.loads(generator.generate())["contracts"]
        self.assertEqual(len(contracts), 18)
        keys = {(entry["group"], entry["version"], entry["kind"]) for entry in contracts}
        self.assertEqual(len(keys), 18)
        grpc = {entry["version"]: entry for entry in contracts if entry["kind"] == "GRPCRoute"}
        self.assertEqual(grpc["v1alpha2"]["gatewayApiVersion"], "v1.1.1")
        self.assertEqual(grpc["v1"]["channel"], "standard")
        self.assertNotEqual(grpc["v1"]["selectedSha256"], grpc["v1alpha2"]["selectedSha256"])
        for entry in contracts:
            self.assertTrue(entry["scopeInventory"])

    def test_removed_reference_field_fails_generation(self):
        with self.assertRaisesRegex(RuntimeError, "missing selected property /spec/parametersRef"):
            generator.project_schema({"type":"object", "properties":{"spec":{"type":"object", "properties":{}}}}, generator.selected_tree("GatewayClass"))

    def test_mirror_does_not_invent_backend_weight(self):
        http = next(entry for entry in json.loads(generator.generate())["contracts"] if entry["kind"] == "HTTPRoute" and entry["version"] == "v1")
        filter_schema = http["schema"]["properties"]["spec"]["properties"]["rules"]["items"]["properties"]["filters"]["items"]
        mirror = filter_schema["properties"]["requestMirror"]["properties"]["backendRef"]
        self.assertNotIn("weight", mirror["properties"])
        self.assertEqual(mirror["properties"]["group"]["default"], "")
        self.assertIn("name", mirror["required"])


if __name__ == "__main__":
    unittest.main()
