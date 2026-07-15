---
name: doc-standards
description: User-documentation writing standards. Use this skill when writing or reviewing docs under docs/. Covers directory structure, writing style, page templates, internationalization, version management, and review checklist.
---

# User Documentation Standards

> **About this directory**: `skills/11-doc/` is the **writing standard and style guide** for authoring and reviewing the public `docs/` directory; it is itself not user-facing documentation and is not part of `docs/`. To browse user documentation, go to `docs/`; read this directory only to learn how to write good user documentation.

> Applies to all human-facing documentation under `docs/`. `skills/` targets AI and developers and is not bound by these rules.

## Core principles

1. **User first** — Documentation targets users (operators, developers, integrators), not internal implementers
2. **Example-driven** — Every feature page must include a complete, working YAML example; "read it and use it"
3. **Progressive disclosure** — Quick start → detailed configuration → advanced scenarios → FAQ; go deeper as needed
4. **Multilingual parity** — new and changed user-facing pages update `en/` and `zh-CN/` together; any temporary gap is explicitly tracked
5. **Do not duplicate skills/** — `docs/` does not cover internal implementation; link to dev-guide or external docs when needed

## File list

| # | File | Topic |
|---|------|-------|
| 00 | [00-doc-structure.md](00-doc-structure.md) | Documentation directory structure: four-layer system, directory naming, README.md conventions, VitePress navigation sync, placement decisions for new documents |
| 01 | [01-writing-style.md](01-writing-style.md) | Writing style + YAML examples + cross-references: heading rules, person and tone, Edgion-extension callouts, YAML completeness and placeholders, link conventions |
| 02 | [02-page-template.md](02-page-template.md) | Page templates + parameter tables + FAQ conventions: section structure for plugin pages, Route pages, ops pages, Getting Started pages; four-column parameter-table format; FAQ Q/A format |
| 03 | [03-i18n-rules.md](03-i18n-rules.md) | Internationalization: en/zh-CN symmetry rules, source-language convention, glossary, sync workflow, Chinese typography |
| 04 | [04-versioning-and-changelog.md](04-versioning-and-changelog.md) | Document version management: change-trigger rules, Breaking Change callouts, deprecation callouts, relationship with Release Notes |
| 05 | [05-review-checklist.md](05-review-checklist.md) | Documentation review checklist: technical accuracy, completeness, multilingual sync, link validity, security review |

## Quick decisions

| You want to… | Entry point |
|--------------|-------------|
| Write a documentation page for a new plugin | [02-page-template.md](02-page-template.md) §Plugin page template |
| Decide which directory a new document goes in | [00-doc-structure.md](00-doc-structure.md) §Placement decision tree |
| Write YAML examples | [01-writing-style.md](01-writing-style.md) §YAML example conventions |
| Handle bilingual content when adding a document | [03-i18n-rules.md](03-i18n-rules.md) §Sync workflow |
| Review documentation written by others | [05-review-checklist.md](05-review-checklist.md) |
| Write a configuration parameter table | [02-page-template.md](02-page-template.md) §Configuration parameter table conventions |
