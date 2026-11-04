#!/usr/bin/env python3
"""Guard the Edgion Pingora branch/lockfile source policy.

Development manifests track the shared `edgion_v0.9.1` branch so advancing the fork
does not require synchronized SHA edits. A release milestone may select one
protected immutable tag for the whole family. Reproducibility comes from the
committed Cargo.lock and `--locked` CI/release commands. This guard prevents a
partial source-family edit, a return to per-package revisions, or a lockfile
containing more than one fork commit.
"""

import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
FORK_URL = "https://github.com/Pandaala/pingora.git"
FORK_BRANCH = "edgion_v0.9.1"
FORK_SOURCE = re.compile(
    r"^git\+https://github\.com/Pandaala/pingora\.git"
    r"\?(branch|tag)=([^#]+)#([0-9a-f]{40})$"
)
DIRECT_DEPENDENCIES = {
    "pingora-core": "pingora-core",
    "pingora-http": "pingora-http",
    "pingora-proxy": "pingora-proxy",
    "pingora-limits": "pingora-limits",
    "pingora-load-balancing": "pingora-load-balancing",
    "pingora-ketama": "pingora-ketama",
    "tinyufo": "TinyUFO",
}


def main() -> int:
    errors = []
    manifest = (REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8")
    selectors = set()

    for name, package in DIRECT_DEPENDENCIES.items():
        match = re.search(rf"^{re.escape(name)}\s*=\s*\{{([^}}]+)\}}$", manifest, re.MULTILINE)
        if match is None:
            errors.append(f"Cargo.toml: missing inline-table dependency {name}")
            continue
        spec = match.group(1)
        if re.search(rf'\bgit\s*=\s*"{re.escape(FORK_URL)}"', spec) is None:
            errors.append(f"Cargo.toml: {name} must use {FORK_URL}")
        branch_match = re.search(r'\bbranch\s*=\s*"([^"]+)"', spec)
        tag_match = re.search(r'\btag\s*=\s*"([^"]+)"', spec)
        if (branch_match is None) == (tag_match is None):
            errors.append(f"Cargo.toml: {name} must set exactly one of branch or tag")
        elif branch_match is not None:
            branch = branch_match.group(1)
            selectors.add(("branch", branch))
            if branch != FORK_BRANCH:
                errors.append(f"Cargo.toml: {name} development branch must be {FORK_BRANCH}")
        else:
            tag = tag_match.group(1)
            selectors.add(("tag", tag))
            if re.fullmatch(r"[A-Za-z0-9._/-]+", tag) is None:
                errors.append(f"Cargo.toml: {name} has an invalid release tag {tag!r}")
        if re.search(r"\brev\s*=", spec):
            errors.append(f"Cargo.toml: {name} must not set rev")
        package_match = re.search(r'\bpackage\s*=\s*"([^"]+)"', spec)
        if package != name and (
            package_match is None or package_match.group(1) != package
        ):
            errors.append(
                f"Cargo.toml: {name} must set package = {package!r}"
            )

    if len(selectors) != 1:
        errors.append(
            "Cargo.toml: all Edgion Pingora dependencies must use one selector, "
            f"found {sorted(selectors)}"
        )
    expected_selector = next(iter(selectors), None)

    lock = (REPO_ROOT / "Cargo.lock").read_text(encoding="utf-8")
    fork_packages = []
    commits = set()
    lock_selectors = set()
    for block in re.split(r"(?=^\[\[package\]\]$)", lock, flags=re.MULTILINE):
        name_match = re.search(r'^name = "([^"]+)"$', block, re.MULTILINE)
        source_match = re.search(r'^source = "([^"]+)"$', block, re.MULTILINE)
        if name_match is None or source_match is None:
            continue
        name = name_match.group(1)
        source = source_match.group(1)
        if not source.startswith(f"git+{FORK_URL}"):
            continue
        fork_packages.append(name)
        match = FORK_SOURCE.fullmatch(source)
        if match is None:
            errors.append(f"Cargo.lock: {name} has a non-policy fork source: {source}")
        else:
            lock_selectors.add((match.group(1), match.group(2)))
            commits.add(match.group(3))

    if not fork_packages:
        errors.append("Cargo.lock: no Edgion Pingora fork packages found")
    if len(commits) != 1:
        errors.append(
            "Cargo.lock: Edgion Pingora fork packages must resolve to exactly "
            f"one commit, found {sorted(commits)}"
        )
    if len(lock_selectors) != 1 or (
        expected_selector is not None and lock_selectors != {expected_selector}
    ):
        errors.append(
            "Cargo.lock: fork selector must match Cargo.toml, "
            f"manifest={expected_selector}, lock={sorted(lock_selectors)}"
        )

    locked_names = set(fork_packages)
    for package in DIRECT_DEPENDENCIES.values():
        if package not in locked_names:
            errors.append(f"Cargo.lock: fork package {package} is missing")

    if errors:
        print("Pingora source policy guard failed:")
        for error in errors:
            print(f"  - {error}")
        return 1

    commit = next(iter(commits))
    selector = next(iter(lock_selectors))
    print(
        "Pingora source policy guard passed: "
        f"{len(fork_packages)} fork packages use {selector[0]}={selector[1]} "
        f"and are locked to {commit}."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
