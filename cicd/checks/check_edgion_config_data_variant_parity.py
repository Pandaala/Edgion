#!/usr/bin/env python3
"""Verify the EdgionConfigData CRD `type` enum matches the Rust `ConfigEntry` variant set.

EdgionConfigData is a tagged-enum CRD (`x-kubernetes-preserve-unknown-fields` on the
config payload), so the project's general `crd_coverage.rs` field guard does NOT
cover it. The one thing the CRD does still constrain is the `type:` discriminator,
which must enumerate exactly the variants the gateway / controller will accept.

History: during the EdgionConfigData overlay refactor, S3 deleted two Rust variants
(ClusterRegionRoute / ServiceRegionRoute) but the CRD `type` enum kept listing
them. S7 fixed the drift later, but only after the orchestrator noticed. This
gate exists so the same drift can never silently land again -- the next time the
Rust enum changes, this script fails the build until the CRD is updated.

Policy source:
  tasks/todo/edgion-config-data-overlay-refactor/06-decisions.md  -> D2, D6
  tasks/todo/edgion-config-data-overlay-refactor/05-issues.md     -> D-S7-A

Exit 0 when the two sets match; exit 1 with the drift report otherwise.
"""

from __future__ import annotations

import os
import re
import sys
from typing import Set

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
RUST_FILE = os.path.join(REPO_ROOT, "edgion-resources/src/resources/edgion_config_data/mod.rs")
CRD_FILE = os.path.join(REPO_ROOT, "config/crd/edgion-crd/edgion_config_data_crd.yaml")


def extract_rust_variants(path: str) -> Set[str]:
    """Pull the variant identifier names from `pub enum ConfigEntry { ... }`.

    Tolerates inline doc comments and arbitrary inner-type whitespace. Matches
    `Variant(...)`, `Variant { ... }`, and bare `Variant,`.
    """
    with open(path, encoding="utf-8") as f:
        src = f.read()
    m = re.search(r"pub\s+enum\s+ConfigEntry\s*\{(.*?)\}\s*", src, re.S)
    if not m:
        sys.exit(f"ERROR: could not locate `pub enum ConfigEntry` in {path}")
    body = m.group(1)
    # Strip line comments to avoid false matches inside doc comments.
    body = re.sub(r"//.*?$", "", body, flags=re.M)
    # Strip block comments.
    body = re.sub(r"/\*.*?\*/", "", body, flags=re.S)
    # Each variant starts with an UpperCamel ident at the start of a logical
    # entry. We look for `<Name>(`, `<Name> {`, or `<Name>,`.
    variants = set(re.findall(r"\b([A-Z][A-Za-z0-9_]*)\s*[\(\{,]", body))
    if not variants:
        sys.exit(f"ERROR: parsed `ConfigEntry` body but found no variants in {path}")
    return variants


def extract_crd_type_enum(path: str) -> Set[str]:
    """Pull the `type:` discriminator `enum:` list from the EdgionConfigData CRD YAML.

    The CRD nests several `enum:` blocks (e.g. for `matchMode`); we want the one
    that immediately follows the `type:` property definition under `data:`.
    Rather than parse YAML (heavy dep), we grep for a block whose enum members
    are the well-known variant identifiers. To avoid ambiguity we anchor on the
    sequence: a `type:` line, then `description:`, then `enum:`, then a run of
    `- <Name>` lines.
    """
    with open(path, encoding="utf-8") as f:
        src = f.read()
    # Find the FIRST `type:` property whose enum body lists CamelCase tags. The
    # block is: `<indent>type:\n<indent2>description: ...\n<indent2>enum:\n<list>`.
    m = re.search(
        r"\n[ \t]+type:\s*\n[ \t]+(?:type:\s*string\s*\n[ \t]+)?description:[^\n]*\n[ \t]+enum:\s*\n((?:[ \t]+-\s*[A-Z][A-Za-z0-9_]*\s*\n)+)",
        src,
    )
    if not m:
        sys.exit(f"ERROR: could not locate `data.type.enum:` block in {path}")
    members = set(re.findall(r"-\s*([A-Z][A-Za-z0-9_]*)", m.group(1)))
    if not members:
        sys.exit(f"ERROR: parsed enum block but found no members in {path}")
    return members


def main() -> int:
    rust = extract_rust_variants(RUST_FILE)
    crd = extract_crd_type_enum(CRD_FILE)

    only_in_rust = rust - crd
    only_in_crd = crd - rust

    if not only_in_rust and not only_in_crd:
        print(f"OK: EdgionConfigData CRD `type` enum matches Rust `ConfigEntry` variants ({len(rust)} variants)")
        return 0

    print("ERROR: EdgionConfigData CRD `type` enum drifted from Rust `ConfigEntry`:", file=sys.stderr)
    if only_in_rust:
        print(
            "  in Rust but NOT in CRD (CRD rejects valid resources): "
            + ", ".join(sorted(only_in_rust)),
            file=sys.stderr,
        )
        print(f"  -> add to {os.path.relpath(CRD_FILE, REPO_ROOT)} `data.type.enum`", file=sys.stderr)
    if only_in_crd:
        print(
            "  in CRD but NOT in Rust (CRD allows resources the code can't decode): "
            + ", ".join(sorted(only_in_crd)),
            file=sys.stderr,
        )
        print(f"  -> remove from {os.path.relpath(CRD_FILE, REPO_ROOT)} `data.type.enum`", file=sys.stderr)
    return 1


if __name__ == "__main__":
    sys.exit(main())
