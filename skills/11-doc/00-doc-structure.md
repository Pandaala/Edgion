# Documentation Directory Structure

## Top-level structure

```
docs/
├── index.md               # VitePress landing page and language entry point
├── .vitepress/            # VitePress site config, nav, and sidebars
├── en/                    # English documentation
├── zh-CN/                 # Chinese documentation
└── superpowers/           # Internal design proposals and plans (not user-facing)
```

The intended steady state is matching relative paths under `en/` and `zh-CN/`.
Track and close any temporary translation gap explicitly.

## Four-layer system

Each language directory has four audience layers, plus three auxiliary
directories (`migrations/`, `promotion/`, and `release-notes/`):

| Layer | Directory | Audience | Positioning |
|-------|-----------|----------|-------------|
| 1 | `getting-started/` | New users | From zero to first successful run; runnable in 5 minutes |
| 2 | `user-guide/` | Daily users | Usage guide organized by feature; Route types, plugins, match rules, backend configuration |
| 3 | `ops-guide/` | Operators | Gateway operations: Gateway configuration, TLS, listeners, observability, infrastructure |
| 4 | `dev-guide/` | Developers / contributors | Architecture overview, plugin development, resource registration, logging, CI/CD |
| aux | `migrations/` | Upgraders | Version-to-version migration guides (breaking changes, config/API renames) |
| aux | `promotion/` | Evaluators | Promotional material (white papers, intro pieces); exists in both `en/` and `zh-CN/` |
| aux | `release-notes/` | Upgraders | Version-specific change summaries and links to detailed guides |

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
| Use README.md when a directory needs a local index | `filters/README.md` | `filters/index.md` |

**Exception**: getting-started may need to imply reading order, but still uses feature names rather than numeric prefixes.

## README.md conventions

When a directory has a `README.md`, it acts as that directory's local navigation
page. Not every current leaf directory has one; do not claim an index exists
without checking the tree.

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

## Navigation sync rules

Directory-level `README.md` files are local indexes where present.
`docs/.vitepress/config.ts` is the authoritative source for the rendered site
navigation bars and sidebars.

**Triggers an update**:
- Adding a documentation file
- Removing a documentation file
- Moving or renaming a documentation file
- Adding or removing a subdirectory

**Does not trigger an update**:
- Modifying only file content (paths unchanged)

**How to update**: PRs that change documentation structure must update the
containing directory's `README.md` when that index exists and, when the page is
user-facing navigation, the matching VitePress sidebar/nav entry. Create a new
README only when the directory needs a browsable local index.

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
├── Version migration / upgrade guide (breaking changes, config/API renames)?
│   └── migrations/
│
├── Promotional material (white papers, intro pieces)?
    └── promotion/ (en + zh-CN)

└── Version-specific change summary?
    └── release-notes/ (en + zh-CN)
```

## Relationship with skills/

- `docs/` is the human-facing documentation layer
- `skills/` is the knowledge layer for AI and developers
- Do not duplicate detailed content between them
- When `docs/` needs to touch architectural details, link to the corresponding doc under dev-guide/; do not inline content from skills/
