# Documentation Versioning and Changelog

## When documentation must be updated

### Changes that require a documentation update

| Change type | Description | Update target |
|-------------|-------------|---------------|
| New feature | Add a new plugin, resource type, or configuration option | New documentation page |
| Behavior change | Change defaults, change matching logic, change error handling | Update affected pages |
| Configuration schema change | Add / remove / rename fields | Update parameter table |
| New annotation | Add an `edgion.io/*` annotation | Update the annotation reference |
| CLI change | New command, parameter change | Update edgion-cli docs |

### Changes that do not require a documentation update

| Change type | Description |
|-------------|-------------|
| Internal refactor | Code refactor that doesn't change external behavior |
| Performance optimization | Optimizations that don't change configuration or behavior (unless users need to know) |
| Bug fix | Fixes for behavior that "should have worked this way all along" |
| Test changes | Test-only code changes |

### Gray area

If a bug fix changes behavior that users could have observed (for example, a configuration that was previously ignored now takes effect), explain it in the relevant documentation.

## Breaking change callouts

When a feature has an incompatible change, add an alert to the affected documentation page:

```markdown
> ⚠️ **Breaking Change (v0.X)**
> 
> In v0.X, `old_field` was replaced by `new_field`. Update manifests before upgrading; the old field is not accepted.
```

### Callout rules

- Place at the top of the page (after the H1, before the overview)
- Note the version number
- Describe what changed and how to migrate
- State that the old behavior or field is unavailable and give the exact migration
- Keep the alert while the affected version remains relevant; move durable upgrade details to `migrations/`

## Pre-release compatibility policy

Edgion has not shipped to production. Renames and removals therefore land
atomically across code, CRDs, configuration examples, and documentation. Do not
document deprecated aliases, dual fields, migration shims, or a multi-release
compatibility window unless the user explicitly requests a released-version
compatibility policy.

For a pre-release rename, show only the current field in parameter tables:

```markdown
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
| `new_field` | String | — | Current field; `old_field` was removed in v0.X |
```

When an entire feature is removed, point readers to the replacement and an
explicit migration page. Do not imply the removed feature remains executable.

## Version-sensitive content

### Minimum-version callouts

When a feature has a minimum version requirement:

```markdown
## Session Persistence

> Requires Edgion ≥ v0.5.
```

### Behavior differences across versions

When behavior differs across versions, use a blockquote:

```markdown
> **Version differences**
> - v0.4 and earlier: `timeout` defaults to `30s`
> - v0.5 and later: `timeout` defaults to `60s`
```

## Relationship with Release Notes

| Content | Release Notes | Documentation |
|---------|---------------|---------------|
| "What changed" | ✅ Focus | Only Breaking Change callouts |
| "How to use" | Brief mention | ✅ Focus |
| "How to migrate" | Brief mention | ✅ Detailed steps |
| Full change list | ✅ | Not needed |

Release Notes link to documentation for detailed usage guides; documentation links to Release Notes for the full change list.

## Documentation updates in PRs

### Checklist

For code PRs that introduce user-visible changes, confirm before submitting:

- [ ] The corresponding documentation page is updated
- [ ] The parameter table matches the schema in code
- [ ] YAML examples still work
- [ ] zh-CN and en are both updated (or marked `[i18n-sync-needed]`)
- [ ] If there is a Breaking Change, the alert callout has been added
- [ ] Directory README and VitePress navigation are updated if documents were added / removed / moved
