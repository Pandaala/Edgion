# Documentation Directory Structure

## Top-level structure

```
docs/
├── README.md              # Project-level README (language entry point)
├── DIRECTORY.md           # Full directory-tree index (shared by AI and humans)
├── en/                    # English documentation
├── zh-CN/                 # Chinese documentation
├── ja/                    # Japanese documentation (reserved)
├── refactoring/           # Internal refactoring design docs (not user-facing)
├── review/                # Code review records (not user-facing)
└── superpowers/           # Design proposals and plans (not user-facing)
```

The internal structure of `en/` and `zh-CN/` must be fully symmetric.

## Four-layer system

Each language directory has four layers, targeting different audiences and stages:

| Layer | Directory | Audience | Positioning |
|-------|-----------|----------|-------------|
| 1 | `getting-started/` | New users | From zero to first successful run; runnable in 5 minutes |
| 2 | `user-guide/` | Daily users | Usage guide organized by feature; Route types, plugins, match rules, backend configuration |
| 3 | `ops-guide/` | Operators | Gateway operations: Gateway configuration, TLS, listeners, observability, infrastructure |
| 4 | `dev-guide/` | Developers / contributors | Architecture overview, plugin development, resource registration, logging, CI/CD |

### Layer boundaries

- **getting-started** explains only "the minimum knowledge needed to get going"; do not enumerate all options
- **user-guide** is grouped by feature; each Route type gets its own subdirectory
- **ops-guide** focuses on "deployment and operations"; does not cover business routing rules
- **dev-guide** targets people contributing code to Edgion, not users who only consume Edgion

## Directory naming rules

| Rule | Example | Counter-example |
|------|---------|-----------------|
| kebab-case | `http-route/`, `backend-tls.md` | `httpRoute/`, `backend_tls.md` |
| Directory names are plural nouns or feature names | `filters/`, `backends/`, `matches/` | `filter/` |
| File names are feature names without numeric prefixes | `cors.md`, `session-persistence.md` | `01-cors.md` |
| README.md serves as the directory index | `filters/README.md` | `filters/index.md` |

**Exception**: getting-started may need to imply reading order, but still uses feature names rather than numeric prefixes.

## README.md conventions

The README.md of each subdirectory acts as a navigation page for that directory:

```markdown
# <Directory topic>

<One sentence describing what this directory covers>

## Contents

| Document | Description |
|----------|-------------|
| xxx.md | One-sentence description |
| yyy.md | One-sentence description |
```

Requirements:
- The H1 is the directory topic, not "Contents" or "Index"
- The table lists every document in the directory with a one-sentence description
- If subdirectories exist, list them too and mark them as "directory"

## DIRECTORY.md sync rules

`docs/DIRECTORY.md` is the full index of the entire documentation tree.

**Triggers an update**:
- Adding a documentation file
- Removing a documentation file
- Moving or renaming a documentation file
- Adding or removing a subdirectory

**Does not trigger an update**:
- Modifying only file content (paths unchanged)

**How to update**: Maintained manually. PRs that change documentation structure must update DIRECTORY.md in sync.

## Placement decision tree for new documents

```
The document you're writing is about…

├── First-time user contact, quick onboarding?
│   └── getting-started/
│
├── How to use a particular route type (HTTP/gRPC/TCP/UDP)?
│   └── user-guide/<route-type>/
│   ├── Match rules → matches/
│   ├── Filters / plugins → filters/
│   ├── Backend configuration → backends/
│   └── Resilience (timeout / retry / session persistence) → resilience/
│
├── Standalone plugin (not bound to a specific Route type)?
│   └── user-guide/edgion-plugins/
│
├── Gateway / Listener / TLS configuration?
│   └── ops-guide/gateway/
│
├── Observability (logs, metrics)?
│   └── ops-guide/observability/
│
├── Infrastructure (Secret, ReferenceGrant, mTLS)?
│   └── ops-guide/infrastructure/
│
├── Development / contribution guide (architecture, plugin development, CI)?
│   └── dev-guide/
│
└── Promotional material (white papers, intro pieces)?
    └── promotion/ (zh-CN only)
```

## Relationship with skills/

- `docs/` is the human-facing documentation layer
- `skills/` is the knowledge layer for AI and developers
- Do not duplicate detailed content between them
- When `docs/` needs to touch architectural details, link to the corresponding doc under dev-guide/; do not inline content from skills/
