#!/usr/bin/env python3
"""Guard open extension tags without weakening preserved configs or required fields.

Rust decoding, invalidation, and schema-hidden sentinel behavior are tested by
edgion-resources/tests/open_extension_types.rs. Nested enums remain independent.
"""
from pathlib import Path
import sys
import yaml

ROOT = Path(__file__).resolve().parents[2]


def check(path: Path, nested: bool) -> None:
    document = yaml.safe_load(path.read_text(encoding="utf-8"))
    versions = document["spec"]["versions"]
    if not versions:
        raise ValueError(f"{path.name}: at least one CRD version is required")
    for version in versions:
        spec = version["schema"]["openAPIV3Schema"]["properties"]["spec"]
        parent = spec["properties"]["data"] if nested else spec
        tag = dict(parent["properties"]["type"])
        tag.pop("description", None)
        if tag != {"type": "string"}:
            raise ValueError(f"{path.name}/{version['name']}: extension type must be an open string")
        if not {"type", "config"}.issubset(parent.get("required", [])):
            raise ValueError(f"{path.name}: type and config must remain required")
        config = parent["properties"]["config"]
        if config.get("type") != "object" or config.get("x-kubernetes-preserve-unknown-fields") is not True:
            raise ValueError(f"{path.name}: config must remain a preserved object")
        if nested and "data" not in spec.get("required", []):
            raise ValueError(f"{path.name}: spec.data must remain required")


def main() -> int:
    try:
        for filename, nested in [("link_sys_crd.yaml", False), ("edgion_config_data_crd.yaml", True)]:
            check(ROOT / "config/crd/edgion-crd" / filename, nested)
    except (KeyError, TypeError, ValueError, OSError) as error:
        print(f"ERROR: {error}", file=sys.stderr)
        return 1
    print("OK: LinkSys and EdgionConfigData extension types are open; config boundaries remain intact")
    return 0


if __name__ == "__main__":
    sys.exit(main())
