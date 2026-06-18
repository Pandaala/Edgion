# Internationalization Rules

## Languages and directories

| Language | Directory | Status |
|----------|-----------|--------|
| English | `docs/en/` | Active |
| Chinese (Simplified) | `docs/zh-CN/` | Active |
| Japanese | `docs/ja/` | Reserved, not yet started |

## Source-language convention

**zh-CN is the source language; en is the translation target.**

Reason: the project's core team is primarily Chinese-speaking, and new documents are usually written in Chinese first.

Workflow:
1. Write the Chinese document under zh-CN/
2. Translate to en/ (in the same PR or a follow-up PR)
3. For subsequent edits, modify the source language (zh-CN) and then sync the translation

**Exception**: If a particular document is better written in English first (for example, a feature targeting the international community), it may use en as the source, but note this in the PR.

## Symmetry rules

### Directory structure symmetry

The directory structures of en/ and zh-CN/ must be fully consistent:

```
✅ Correct:
docs/en/user-guide/http-route/filters/edgion-plugins/cors.md
docs/zh-CN/user-guide/http-route/filters/edgion-plugins/cors.md

❌ Incorrect (zh-CN has it but en does not):
docs/zh-CN/user-guide/http-route/backends/circuit-breaker.md
(no corresponding file under en/)
```

### Filename consistency

Filenames are kept identical across all languages (English kebab-case); do not translate filenames:

```
✅ docs/zh-CN/getting-started/first-gateway.md
❌ docs/zh-CN/getting-started/<localized-name>.md
```

### Section structure symmetry

The section titles of the same document may be translated, but the section structure (count, order, level) must be identical.

### YAML examples consistent

YAML code blocks are fully identical across languages (including field values). Only the comment text is translated:

```yaml
# zh-CN
# <Chinese translation of "Allowed frontend origins">
allow_origins: "https://app.example.com"

# en
# Allowed frontend origins
allow_origins: "https://app.example.com"
```

## Glossary

The following terms remain in English (untranslated) in Chinese documentation:

### Standard Gateway API terms

| English | Form in Chinese docs | Do not write as |
|---------|----------------------|-----------------|
| Gateway | Gateway | A Chinese translation of "gateway" when used as a resource name |
| GatewayClass | GatewayClass | A Chinese translation of "gateway class" |
| HTTPRoute | HTTPRoute | A Chinese translation such as "HTTP route" when used as a resource name |
| GRPCRoute | GRPCRoute | — |
| TCPRoute | TCPRoute | — |
| UDPRoute | UDPRoute | — |
| TLSRoute | TLSRoute | — |
| Listener | Listener | A Chinese translation of "listener" when used as a field name |
| BackendRef | BackendRef | A Chinese translation of "backend reference" |
| ParentRef | ParentRef | A Chinese translation of "parent reference" |
| Filter | Filter (when used as a field name) | A Chinese translation of "filter" (acceptable as conceptual description only) |
| Secret | Secret | A Chinese translation of "secret/key" when referring to the K8s resource |
| Namespace | Namespace | A Chinese translation of "namespace" (acceptable as conceptual description only) |
| ReferenceGrant | ReferenceGrant | A Chinese translation of "reference grant" |
| Service | Service (when referring to the K8s resource) | A Chinese translation of "service" |

### Edgion-specific terms

| English | Form in Chinese docs |
|---------|----------------------|
| EdgionPlugins | EdgionPlugins |
| EdgionTls | EdgionTls |
| EdgionGatewayConfig | EdgionGatewayConfig |
| Controller | Chinese term for "control plane" / Controller (annotate on first appearance with the original English in parentheses) |
| Gateway (as the Edgion component) | Chinese term for "data plane" / Gateway (annotate on first appearance with the original English in parentheses) |
| edgion-cli | edgion-cli |
| PluginLog | PluginLog |
| access log | access log |

### General technical terms

| English | Chinese | Notes |
|---------|---------|-------|
| TLS termination | (Chinese translation of "TLS termination") | Translatable |
| mTLS | mTLS | Do not translate |
| load balancing | (Chinese translation of "load balancing") | Translatable |
| health check | (Chinese translation of "health check") | Translatable |
| circuit breaker | (Chinese translation of "circuit breaker") | Translatable |
| rate limiting | (Chinese translation of "rate limiting") | Translatable |
| session persistence | (Chinese translation of "session persistence") | Translatable |
| upstream | (Chinese translation of "upstream") | Translatable |
| downstream | (Chinese translation of "downstream") | Translatable |
| backend | (Chinese translation of "backend") | Translatable |

## Sync workflow

### Adding a document

1. Create the document under zh-CN/ (source language)
2. Translate to en/
3. Update the README.md in both directories (if any)
4. Update the VitePress sidebar/nav entry if the page should appear in site navigation

### Modifying an existing document

1. Modify the source document under zh-CN/
2. Sync the modification into the translation under en/
3. If you cannot sync the translation in this PR, mark the PR with `[i18n-sync-needed]`

### Sync-status tracking

When a translation lags behind, you may add a temporary callout at the top of the English document:

```markdown
> ⚠️ This page may be out of date with the zh-CN version. Last synced: 2026-03-15.
```

Remove the callout once the task is complete.

## Chinese typography rules

- Add a space between Chinese characters and English / digits (for example, between Chinese prose and a token like `Gateway` or `TLS`)
- Add a space between Chinese characters and inline code (for example, between Chinese prose and a token like `allow_origins`)
- Use full-width punctuation for full stop, comma, and colon (except in code / YAML)
- Use correct capitalization for proper nouns: `Kubernetes`, not `kubernetes`
