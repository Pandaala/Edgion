#!/usr/bin/env python3
"""Validate the agent-facing markdown layer.

Three checks, run over every markdown file at the repo root, under ``docs/``,
``skills/``, and ``edgion-tests/README.md``:

1. Relative markdown links resolve to a real file on disk.
2. Every file named ``SKILL.md`` has YAML frontmatter with ``name:`` and
   ``description:`` fields (so the agent skill index stays usable).
3. Every ``src/<path>.rs`` reference inside ``skills/`` (``.md`` + ``.skill``)
   resolves on disk. Catches skill text drifting out of sync with the source
   tree when files are renamed or removed.

Everything else (migration-era forbidden-pattern guards, hardcoded entry-file
and dev-guide-index lists, GitHub Actions path checks, etc.) used to live in
this script and was trimmed — those checks went stale faster than they caught
real defects.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent.parent

TARGET_GLOBS = [
    "*.md",
    "edgion-tests/README.md",
    "docs/**/*.md",
    "skills/**/*.md",
]

LINK_RE = re.compile(r"\[[^\]]+\]\(([^)]+)\)")
FENCED_CODE_RE = re.compile(r"(?ms)^([ \t]*)(`{3,}|~{3,}).*?\n.*?^\1\2\s*$")
INLINE_CODE_RE = re.compile(r"`[^`\n]*`")
NAME_RE = re.compile(r"(?m)^name:\s*.+$")
DESC_RE = re.compile(r"(?m)^description:\s*.+$")
# Matches a Rust source path, optionally prefixed by a workspace-crate dir
# (e.g. ``edgion-cli/src/bin/edgion_ctl.rs``). The optional ``crate`` group lets
# the validator resolve crate-prefixed paths instead of mis-parsing the inner
# ``src/...`` portion (whose first segment may collide with a root ``src/`` dir).
SOURCE_PATH_RE = re.compile(r"\b(?:(?P<crate>[a-zA-Z0-9_-]+)/)?(?P<src>src/[a-zA-Z0-9_/]+\.rs)\b")


def iter_target_files() -> list[Path]:
    files: set[Path] = set()
    for pattern in TARGET_GLOBS:
        files.update(REPO_ROOT.glob(pattern))
    return sorted(f for f in files if f.is_file())


def validate_markdown_links(files: list[Path]) -> list[str]:
    errors: list[str] = []
    for file in files:
        text = file.read_text(encoding="utf-8")
        # Strip fenced and inline code so rustdoc intra-doc links
        # (e.g. ``[Foo](crate::path::Foo)``) are not flagged as broken
        # filesystem links.
        scan_text = FENCED_CODE_RE.sub("", text)
        scan_text = INLINE_CODE_RE.sub("", scan_text)
        for raw_target in LINK_RE.findall(scan_text):
            target = raw_target.strip()
            if (
                target.startswith(("http://", "https://", "mailto:"))
                or target.startswith("#")
                or target.startswith("app://")
                or target.startswith("plugin://")
            ):
                continue
            path_part = target.split("#", 1)[0].split("?", 1)[0].strip()
            if not path_part:
                continue
            resolved = (file.parent / path_part).resolve()
            if not resolved.exists():
                errors.append(
                    f"broken link in {file.relative_to(REPO_ROOT)} -> {path_part}"
                )
    return errors


def iter_skill_content_files() -> list[Path]:
    """All skill content under ``skills/`` — both ``*.md`` and ``*.skill``."""
    skills_root = REPO_ROOT / "skills"
    files: set[Path] = set()
    files.update(skills_root.rglob("*.md"))
    files.update(skills_root.rglob("*.skill"))
    return sorted(f for f in files if f.is_file())


def _edgion_src_tops() -> set[str]:
    """Top-level entries under ``src/`` — used to distinguish Edgion source
    paths from external-crate paths (e.g. ``src/string.rs`` mentioned when
    citing the ``kstring`` crate's internals)."""
    src_root = REPO_ROOT / "src"
    if not src_root.is_dir():
        return set()
    return {entry.name for entry in src_root.iterdir()}


def validate_source_paths(files: list[Path]) -> list[str]:
    """Check that every ``src/<path>.rs`` reference resolves on disk.

    Filters out paths whose first segment after ``src/`` is not a real
    Edgion top-level entry, so external-crate citations like
    ``src/string.rs`` (kstring internals) don't false-positive.
    """
    errors: list[str] = []
    seen: set[tuple[Path, str]] = set()
    valid_tops = _edgion_src_tops()
    for file in files:
        for line_no, line in enumerate(
            file.read_text(encoding="utf-8").splitlines(), 1
        ):
            for match in SOURCE_PATH_RE.finditer(line):
                crate = match.group("crate")
                src_part = match.group("src")
                if crate is not None and (REPO_ROOT / crate).is_dir():
                    # Crate-prefixed workspace path (e.g. ``edgion-cli/src/...``):
                    # resolve the full path directly.
                    path_part = f"{crate}/{src_part}"
                else:
                    # Bare ``src/...`` reference into the root crate. Filter out
                    # paths whose first segment is not a real top-level entry so
                    # external-crate citations don't false-positive.
                    first_seg = src_part[len("src/"):].split("/", 1)[0]
                    if first_seg not in valid_tops:
                        continue
                    path_part = src_part
                key = (file, path_part)
                if key in seen:
                    continue
                seen.add(key)
                if not (REPO_ROOT / path_part).is_file():
                    errors.append(
                        f"stale src/ ref in {file.relative_to(REPO_ROOT)}:{line_no} -> {path_part}"
                    )
    return errors


def validate_skill_frontmatter(files: list[Path]) -> list[str]:
    errors: list[str] = []
    for file in files:
        if file.name != "SKILL.md":
            continue
        text = file.read_text(encoding="utf-8")
        if not text.startswith("---\n"):
            errors.append(f"missing frontmatter: {file.relative_to(REPO_ROOT)}")
            continue
        closing = text.find("\n---\n", 4)
        if closing == -1:
            errors.append(f"unterminated frontmatter: {file.relative_to(REPO_ROOT)}")
            continue
        frontmatter = text[4:closing]
        if not NAME_RE.search(frontmatter):
            errors.append(f"frontmatter missing name: {file.relative_to(REPO_ROOT)}")
        if not DESC_RE.search(frontmatter):
            errors.append(
                f"frontmatter missing description: {file.relative_to(REPO_ROOT)}"
            )
    return errors


def main() -> int:
    files = iter_target_files()
    skill_content_files = iter_skill_content_files()
    errors: list[str] = []
    errors.extend(validate_markdown_links(files))
    errors.extend(validate_skill_frontmatter(files))
    errors.extend(validate_source_paths(skill_content_files))

    if errors:
        print("Agent-doc validation failed:")
        for err in errors:
            print(f"- {err}")
        return 1

    skill_count = sum(1 for f in files if f.name == "SKILL.md")
    print(
        f"Agent-doc validation passed: checked {len(files)} files, "
        f"{skill_count} skill entry files, "
        f"{len(skill_content_files)} skill source-path scopes."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
