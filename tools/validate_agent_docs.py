#!/usr/bin/env python3
"""Validate the agent-facing markdown layer.

Two checks, run over every markdown file at the repo root, under ``docs/``,
``skills/``, and ``examples/README.md``:

1. Relative markdown links resolve to a real file on disk.
2. Every file named ``SKILL.md`` has YAML frontmatter with ``name:`` and
   ``description:`` fields (so the agent skill index stays usable).

Everything else (migration-era forbidden-pattern guards, hardcoded entry-file
and dev-guide-index lists, GitHub Actions path checks, etc.) used to live in
this script and was trimmed — those checks went stale faster than they caught
real defects.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent

TARGET_GLOBS = [
    "*.md",
    "examples/README.md",
    "docs/**/*.md",
    "skills/**/*.md",
]

LINK_RE = re.compile(r"\[[^\]]+\]\(([^)]+)\)")
FENCED_CODE_RE = re.compile(r"(?ms)^([ \t]*)(`{3,}|~{3,}).*?\n.*?^\1\2\s*$")
INLINE_CODE_RE = re.compile(r"`[^`\n]*`")
NAME_RE = re.compile(r"(?m)^name:\s*.+$")
DESC_RE = re.compile(r"(?m)^description:\s*.+$")


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
    errors: list[str] = []
    errors.extend(validate_markdown_links(files))
    errors.extend(validate_skill_frontmatter(files))

    if errors:
        print("Agent-doc validation failed:")
        for err in errors:
            print(f"- {err}")
        return 1

    skill_count = sum(1 for f in files if f.name == "SKILL.md")
    print(
        f"Agent-doc validation passed: checked {len(files)} files, "
        f"{skill_count} skill entry files."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
