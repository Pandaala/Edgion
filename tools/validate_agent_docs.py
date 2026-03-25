#!/usr/bin/env python3
"""Validate AGENTS/skills/dev-guide knowledge docs.

This script intentionally focuses on the repository's agent-facing entry files
and the developer-guide documents that describe the same architecture, so the
AI collaboration layer does not silently drift over time.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent

ENTRY_FILES = [
    REPO_ROOT / "README.md",
    REPO_ROOT / "CONTRIBUTING.md",
    REPO_ROOT / "AGENTS.md",
    REPO_ROOT / "CLAUDE.md",
    REPO_ROOT / "examples/README.md",
    REPO_ROOT / ".cursor/rules/00-edgion-entry.mdc",
    REPO_ROOT / "docs/DIRECTORY.md",
    REPO_ROOT / "skills/01-architecture/00-common/03-resource-system.md",
    REPO_ROOT / "skills/01-architecture/01-controller/06-requeue-mechanism.md",
    REPO_ROOT / "skills/02-development/00-add-new-resource.md",
    REPO_ROOT / "skills/02-development/01-edgion-plugin-dev.md",
    REPO_ROOT / "skills/02-development/02-stream-plugin-dev.md",
    REPO_ROOT / "skills/02-development/03-link-sys-dev.md",
    REPO_ROOT / "skills/02-development/04-config-reference.md",
    REPO_ROOT / "skills/02-development/05-annotations-reference.md",
    REPO_ROOT / "skills/02-development/06-feature-flags.md",
    REPO_ROOT / "skills/02-development/07-documentation-writing.md",
    REPO_ROOT / "skills/05-testing/01-integration-testing.md",
    REPO_ROOT / "skills/06-tracing/00-debugging.md",
    REPO_ROOT / "skills/09-misc/00-local-build.md",
    REPO_ROOT / "skills/09-misc/01-docker-build.md",
    REPO_ROOT / "skills/09-misc/02-github-workflow.md",
    REPO_ROOT / "skills/09-misc/debugging-tls-gateway.md",
    REPO_ROOT / "docs/en/dev-guide/README.md",
    REPO_ROOT / "docs/en/dev-guide/ai-agent-collaboration.md",
    REPO_ROOT / "docs/en/dev-guide/knowledge-source-map.md",
    REPO_ROOT / "docs/en/dev-guide/architecture-overview.md",
    REPO_ROOT / "docs/en/dev-guide/resource-architecture-overview.md",
    REPO_ROOT / "docs/en/dev-guide/resource-registry-guide.md",
    REPO_ROOT / "docs/en/dev-guide/add-new-resource-guide.md",
    REPO_ROOT / "docs/en/dev-guide/annotations-guide.md",
    REPO_ROOT / "docs/en/dev-guide/http-plugin-development.md",
    REPO_ROOT / "docs/en/dev-guide/stream-plugin-development.md",
    REPO_ROOT / "docs/en/dev-guide/ci-release-workflow.md",
    REPO_ROOT / "docs/zh-CN/dev-guide/README.md",
    REPO_ROOT / "docs/zh-CN/dev-guide/ai-agent-collaboration.md",
    REPO_ROOT / "docs/zh-CN/dev-guide/knowledge-source-map.md",
    REPO_ROOT / "docs/zh-CN/dev-guide/architecture-overview.md",
    REPO_ROOT / "docs/zh-CN/dev-guide/resource-architecture-overview.md",
    REPO_ROOT / "docs/zh-CN/dev-guide/resource-registry-guide.md",
    REPO_ROOT / "docs/zh-CN/dev-guide/add-new-resource-guide.md",
    REPO_ROOT / "docs/zh-CN/dev-guide/annotations-guide.md",
    REPO_ROOT / "docs/zh-CN/dev-guide/http-plugin-development.md",
    REPO_ROOT / "docs/zh-CN/dev-guide/stream-plugin-development.md",
    REPO_ROOT / "docs/zh-CN/dev-guide/ci-release-workflow.md",
]

SKILL_REFERENCE_GLOBS = [
    "docs/**/*.md",
    "skills/**/SKILL.md",
    "skills/**/*.md",
    "skills/**/references/*.md",
    "skills/**/references/**/*.md",
    ".cursor/**/*.mdc",
]

FORBIDDEN_PATTERNS = {
    "README.md": [
        "cd examples/testing",
        "./run_integration_test.sh",
        "gRPC :5810",
    ],
    "CONTRIBUTING.md": [
        "cd examples/testing",
        "./run_integration_test.sh",
        "Integration tests go in `examples/testing/`",
    ],
    "examples/README.md": [
        "cd examples/testing",
        "./run_integration_test.sh",
    ],
    "skills/01-architecture/00-common/00-project-overview.md": [
        "ConfigSyncServer (gRPC :5810)",
    ],
    "skills/01-architecture/SKILL.md": [
        "ConfigSyncServer (gRPC :5810)",
    ],
    "examples/test/conf/EdgionPlugins/ResponseRewrite/README.md": [
        "./run_integration_test.sh ResponseRewrite",
    ],
    "docs/en/dev-guide/architecture-overview.md": [
        "src/types/resource_kind.rs",
        "src/types/resource_meta_traits/",
        "src/core/conf_mgr/",
        "src/core/conf_sync/",
    ],
    "docs/zh-CN/dev-guide/architecture-overview.md": [
        "src/types/resource_kind.rs",
        "src/types/resource_meta_traits/",
        "src/core/conf_mgr/",
        "src/core/conf_sync/",
    ],
    "docs/en/dev-guide/resource-architecture-overview.md": [
        "src/types/resource_registry.rs",
        "src/types/resource_meta_traits/",
        "src/core/conf_sync/conf_client",
        "src/core/conf_sync/conf_server",
    ],
    "docs/zh-CN/dev-guide/resource-architecture-overview.md": [
        "src/types/resource_registry.rs",
        "src/types/resource_meta_traits/",
        "src/core/conf_sync/conf_client",
        "src/core/conf_sync/conf_server",
    ],
    "docs/en/dev-guide/resource-registry-guide.md": [
        "src/types/resource_registry.rs",
        "src/types/resource_meta_traits/",
    ],
    "docs/zh-CN/dev-guide/resource-registry-guide.md": [
        "src/types/resource_registry.rs",
        "src/types/resource_meta_traits/",
    ],
    "docs/en/dev-guide/annotations-guide.md": [
        "edgion.com/enable-http2",
    ],
    "docs/zh-CN/dev-guide/annotations-guide.md": [
        "edgion.com/enable-http2",
    ],
    "docs/en/dev-guide/stream-plugin-development.md": [
        ".github/workflows/actions/setup-rust",
    ],
    "docs/zh-CN/dev-guide/stream-plugin-development.md": [
        ".github/workflows/actions/setup-rust",
    ],
    "docs/en/dev-guide/ci-release-workflow.md": [
        ".github/workflows/actions/setup-rust",
    ],
    "docs/zh-CN/dev-guide/ci-release-workflow.md": [
        ".github/workflows/actions/setup-rust",
    ],
    "docs/README.md": [
        "[English Documentation](./en/README.md) *(Coming soon)*",
        "[日本語ドキュメント (Japanese)](./ja/README.md) *(Coming soon)*",
    ],
    "docs/en/getting-started/README.md": [
        "Installation and Deployment (coming soon)",
        "Your First Gateway (coming soon)",
        "Core Concepts (coming soon)",
    ],
    "docs/zh-CN/getting-started/README.md": [
        "安装部署（即将推出）",
        "第一个 Gateway（即将推出）",
        "核心概念（即将推出）",
    ],
    "docs/en/user-guide/http-route/filters/overview.md": [
        "| RequestMirror | Request mirroring | Coming soon |",
    ],
    "docs/zh-CN/user-guide/http-route/filters/overview.md": [
        "| RequestMirror | 请求镜像 | 即将推出 |",
    ],
    "docs/en/user-guide/README.md": [
        "Canary Release (coming soon)",
        "Blue-Green Deployment (coming soon)",
    ],
    "docs/zh-CN/user-guide/README.md": [
        "灰度发布（即将推出）",
        "蓝绿部署（即将推出）",
    ],
    "docs/en/user-guide/http-route/backends/weight.md": [
        "Canary Release (coming soon)",
    ],
    "docs/zh-CN/user-guide/http-route/backends/weight.md": [
        "灰度发布（即将推出）",
    ],
    "examples/test/scripts/utils/start_all_with_conf.sh": [
        'EDGION_TEST_ACCESS_LOG_PATH="${LOG_DIR}/access.log"',
        "  - ${LOG_DIR}/access.log",
    ],
    "examples/test/scripts/utils/start_all.sh": [
        'EDGION_TEST_ACCESS_LOG_PATH="${LOG_DIR}/access.log"',
        "  - ${LOG_DIR}/access.log",
    ],
    "examples/test/scripts/integration/run_redis_test.sh": [
        'EDGION_TEST_ACCESS_LOG_PATH="${LOG_DIR}/access.log"',
    ],
    "examples/test/scripts/integration/run_etcd_test.sh": [
        'EDGION_TEST_ACCESS_LOG_PATH="${LOG_DIR}/access.log"',
    ],
    "examples/test/scripts/integration/run_es_test.sh": [
        'EDGION_TEST_ACCESS_LOG_PATH="${LOG_DIR}/access.log"',
    ],
    "docs/en/ops-guide/observability/access-log.md": [
        "Default path**: `logs/access.log`",
    ],
    "docs/zh-CN/ops-guide/observability/access-log.md": [
        "默认路径**: `logs/access.log`",
    ],
    "docs/en/dev-guide/work-directory.md": [
        'work_dir().resolve("logs/access.log")',
        'path = "logs/access.log"',
    ],
    "docs/zh-CN/dev-guide/work-directory.md": [
        'work_dir().resolve("logs/access.log")',
        'path = "logs/access.log"',
    ],
    "skills/02-development/01-edgion-plugin-dev.md": [
        "src/core/gateway/plugins/runtime/runtime.rs",
    ],
    "skills/02-development/07-documentation-writing.md": [
        "TODO (2026-02-25): Small Improvement",
        "**Language**: zh-CN (Chinese) only for now.",
        "All documentation lives under `docs/zh-CN/`.",
    ],
    "skills/02-development/03-link-sys-dev.md": [
        "TODO (2026-02-25): Small Improvement",
        "currently listed in directory but not described",
    ],
    "skills/05-testing/01-integration-testing.md": [
        "TODO (2026-02-25): Small Improvement",
        "examples/test/code/",
        "scripts/certs/",
    ],
    "skills/05-testing/SKILL.md": [
        "examples/test/code/",
    ],
    "docs/en/dev-guide/README.md": [
        'Only link to existing documents; mark planned content as "(coming soon)".',
    ],
    "docs/zh-CN/dev-guide/README.md": [
        "只链接已存在文档；规划中内容标注为“（即将推出）”。",
    ],
}

LINK_RE = re.compile(r"\[[^\]]+\]\(([^)]+)\)")
NAME_RE = re.compile(r"(?m)^name:\s*.+$")
DESC_RE = re.compile(r"(?m)^description:\s*.+$")

DEV_GUIDE_INDEXES = {
    "docs/zh-CN/dev-guide/README.md": [
        "architecture-overview.md",
        "resource-architecture-overview.md",
        "resource-registry-guide.md",
        "add-new-resource-guide.md",
        "annotations-guide.md",
        "http-plugin-development.md",
        "stream-plugin-development.md",
        "work-directory.md",
        "logging-system.md",
        "ci-release-workflow.md",
        "ai-agent-collaboration.md",
        "knowledge-source-map.md",
    ],
    "docs/en/dev-guide/README.md": [
        "architecture-overview.md",
        "resource-architecture-overview.md",
        "resource-registry-guide.md",
        "add-new-resource-guide.md",
        "annotations-guide.md",
        "http-plugin-development.md",
        "stream-plugin-development.md",
        "work-directory.md",
        "logging-system.md",
        "ci-release-workflow.md",
        "ai-agent-collaboration.md",
        "knowledge-source-map.md",
    ],
}

DIRECTORY_REQUIRED_SNIPPETS = [
    "ai-agent-collaboration.md",
    "knowledge-source-map.md",
    "http-plugin-development.md",
    "stream-plugin-development.md",
    "ci-release-workflow.md",
]


def iter_target_files() -> list[Path]:
    files = set(ENTRY_FILES)
    for pattern in SKILL_REFERENCE_GLOBS:
        files.update(REPO_ROOT.glob(pattern))
    return sorted(files)


def validate_exists(files: list[Path]) -> list[str]:
    errors: list[str] = []
    for file in files:
        if not file.exists():
            errors.append(f"missing file: {file.relative_to(REPO_ROOT)}")
    return errors


def validate_markdown_links(files: list[Path]) -> list[str]:
    errors: list[str] = []
    for file in files:
        if not file.exists() or file.suffix not in {".md", ".mdc"}:
            continue
        text = file.read_text(encoding="utf-8")
        for raw_target in LINK_RE.findall(text):
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
        if file.name != "SKILL.md" or not file.exists():
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


def validate_forbidden_patterns() -> list[str]:
    errors: list[str] = []
    for relative, patterns in FORBIDDEN_PATTERNS.items():
        file = REPO_ROOT / relative
        if not file.exists():
            errors.append(f"guarded file missing: {relative}")
            continue
        text = file.read_text(encoding="utf-8")
        for pattern in patterns:
            if pattern in text:
                errors.append(f"stale pattern '{pattern}' found in {relative}")
    return errors


def validate_wrappers() -> list[str]:
    errors: list[str] = []

    claude = (REPO_ROOT / "CLAUDE.md").read_text(encoding="utf-8")
    if "AGENTS.md" not in claude or "skills/SKILL.md" not in claude:
        errors.append("CLAUDE.md should point back to AGENTS.md and skills/SKILL.md")

    cursor = (REPO_ROOT / ".cursor/rules/00-edgion-entry.mdc").read_text(
        encoding="utf-8"
    )
    if "AGENTS.md" not in cursor or "skills/SKILL.md" not in cursor:
        errors.append(
            ".cursor/rules/00-edgion-entry.mdc should point back to "
            "AGENTS.md and skills/SKILL.md"
        )

    return errors


def validate_indexes() -> list[str]:
    errors: list[str] = []

    for relative, required_names in DEV_GUIDE_INDEXES.items():
        file = REPO_ROOT / relative
        if not file.exists():
            errors.append(f"index file missing: {relative}")
            continue
        text = file.read_text(encoding="utf-8")
        for name in required_names:
            if name not in text:
                errors.append(f"dev-guide README missing '{name}': {relative}")

    directory = REPO_ROOT / "docs/DIRECTORY.md"
    if not directory.exists():
        errors.append("index file missing: docs/DIRECTORY.md")
    else:
        text = directory.read_text(encoding="utf-8")
        for snippet in DIRECTORY_REQUIRED_SNIPPETS:
            if snippet not in text:
                errors.append(f"docs/DIRECTORY.md missing '{snippet}'")

    return errors


def validate_local_actions() -> list[str]:
    errors: list[str] = []

    setup_rust = REPO_ROOT / ".github/actions/setup-rust/action.yml"
    if not setup_rust.exists():
        errors.append("missing local action: .github/actions/setup-rust/action.yml")

    stale_placeholder = REPO_ROOT / ".github/workflows/actions/setup-rust"
    if stale_placeholder.exists():
        errors.append(
            "stale placeholder should be removed: .github/workflows/actions/setup-rust"
        )

    return errors


def main() -> int:
    files = iter_target_files()
    errors: list[str] = []
    errors.extend(validate_exists(files))
    errors.extend(validate_markdown_links(files))
    errors.extend(validate_skill_frontmatter(files))
    errors.extend(validate_forbidden_patterns())
    errors.extend(validate_wrappers())
    errors.extend(validate_indexes())
    errors.extend(validate_local_actions())

    if errors:
        print("Agent-doc validation failed:")
        for err in errors:
            print(f"- {err}")
        return 1

    print(
        f"Agent-doc validation passed: checked {len(files)} files, "
        f"{sum(1 for f in files if f.name == 'SKILL.md')} skill entry files."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
