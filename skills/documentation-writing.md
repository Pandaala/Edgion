# Edgion Documentation Writing Guide

> Skill for writing and maintaining Edgion documentation under `docs/zh-CN/`.
> Focus: user-guide (for app developers) and ops-guide (for operators).
>
> **TODO (2026-02-25): Small Improvement**
> - [ ] Add CRD schema update workflow (how to update `config/crd/edgion-crd/` when adding new plugin/resource)
> - [ ] Add versioning and changelog guidelines (whether docs should annotate "since vX.Y.Z")

## Scope & Language

- **Language**: zh-CN (Chinese) only for now. Code comments and YAML remain in English.
- **Location**: All documentation lives under `docs/zh-CN/`.

## Document Classification

Correctly classify every document into the right guide. **Do NOT mix audiences.**

| Guide | Audience | Content | Path |
|-------|----------|---------|------|
| **user-guide** | App developers, API consumers | Route config (HTTPRoute/GRPCRoute/TCPRoute/UDPRoute), filters, plugins (as applied to routes), backends, resilience, LB, advanced traffic patterns | `docs/zh-CN/user-guide/` |
| **ops-guide** | Platform operators, SREs | Gateway/GatewayClass setup, listeners, TLS/mTLS/ACME, infrastructure (Secrets, ReferenceGrant), observability (access-log, metrics), edgion-ctl CLI, deployment | `docs/zh-CN/ops-guide/` |
| **dev-guide** | Edgion contributors | Architecture, plugin development, resource system internals, annotation implementation details, logging internals | `docs/zh-CN/dev-guide/` |

### Classification Decision Rules

1. **If it configures what happens to a request** (route matching, plugin behavior, backend selection) → **user-guide**.
2. **If it configures the gateway infrastructure itself** (listeners, certs, TLS policies, monitoring, CLI) → **ops-guide**.
3. **If it explains source code internals or contribution steps** → **dev-guide**.
4. A topic that spans audiences should have a **short doc in each relevant guide**, each tailored to that audience, with cross-references.

### Where Plugins Go

- **EdgionPlugins used as HTTPRoute filters** → `user-guide/http-route/filters/edgion-plugins/<plugin>.md`
- **EdgionPlugins used as standalone route-level resources** (e.g., cluster-scope rate-limit, ctx-setter, real-ip, forward-auth) → `user-guide/edgion-plugins/<plugin>.md`
- **Stream plugins for TCPRoute/UDPRoute** → `user-guide/tcp-route/stream-plugins.md`
- **Gateway-level EdgionPlugins** (preflight, http-to-https-redirect) → `ops-guide/gateway/`

If a plugin exists in two locations (like RateLimit for single-instance vs. cluster), each doc must **clearly state which scope it covers** and link to the other.

---

## Directory Structure — AI-Friendly Principle

> **Core Idea**: A flat, predictable, self-describing directory layout so AI agents can navigate and generate docs without guessing paths.

### Rules

1. **Mirror the resource hierarchy**. The directory tree should map 1:1 to the Gateway API / Edgion resource model:
   ```
   user-guide/
   ├── http-route/
   │   ├── overview.md
   │   ├── matches/
   │   │   ├── path.md
   │   │   ├── headers.md
   │   │   ├── query-params.md
   │   │   └── method.md
   │   ├── filters/
   │   │   ├── overview.md
   │   │   ├── plugin-composition.md
   │   │   ├── gateway-api/          # standard Gateway API filters
   │   │   └── edgion-plugins/       # Edgion extension plugins
   │   ├── backends/
   │   └── resilience/
   ├── grpc-route/
   ├── tcp-route/
   ├── udp-route/
   ├── edgion-plugins/               # standalone route-level plugins
   └── advanced/
   ```

2. **One topic = one file**. Never put two independent features into one file.

3. **File naming**: kebab-case, descriptive, match the resource/feature name. E.g., `basic-auth.md`, `tls-termination.md`, `session-persistence.md`.

4. **Every directory has a README.md** that serves as a table of contents with brief descriptions. Keep the README index **in sync** with actual files — never link to files that don't exist.

5. **Relative links only**. Use `./` or `../` relative paths. Never use absolute paths.

6. **Cross-references**: When referencing another doc, use the full relative path from the current file. Add a brief note about what the linked doc covers.

---

## Document Structure Template

Every feature document MUST follow this structure. Sections can be omitted only if truly not applicable, but **never omit silently** — add a brief note if a section is intentionally skipped.

```markdown
# <Feature Name>

> **🔌 Edgion 扩展**                          ← Only for non-standard-Gateway-API features
> 
> <One-line note: this is an EdgionPlugins CRD / Annotation / CRD extension>

## 概述 (Overview)

What this feature does in 2-5 sentences. Include:
- The problem it solves
- Which CRD / Annotation / API it belongs to
- How it compares to equivalent features in other gateways (if applicable)

## 快速开始 (Quick Start)

Minimal working example. The user should be able to copy-paste and get something running.
Include both the YAML resource AND any prerequisite resources (Secrets, Services, etc.).

## 配置参数 (Configuration Reference)

Full parameter table(s) in this format:

| 参数 | 类型 | 必填 | 默认值 | 说明 |
|------|------|------|--------|------|
| `fieldName` | Type | ✅/❌ | value | Clear description |

Rules for this section:
- **Every field** in the config struct must appear. No hidden fields.
- Mark required vs optional explicitly.
- Show default values. If the default is computed/dynamic, explain how.
- For nested objects, use sub-tables with their own headers.
- For enum fields, list ALL valid values.
- For fields with special behavior (e.g., `null` vs `[]` vs absent), document each case.

## 场景示例 (Usage Scenarios)

Multiple real-world scenarios, each with:
1. A descriptive title (e.g., "### 场景 1：多用户认证")
2. Brief context explaining when you'd use this
3. Complete YAML configuration
4. Expected behavior description
5. Test command (curl, etc.) where applicable

Aim for 3-5 scenarios covering common, advanced, and edge cases.

## 行为细节与特殊处理 (Behavior Details & Special Handling)

**This section enforces the No-Hidden-Logic principle.** Document:

- **Annotations**: If the feature uses annotations, list every annotation key,
  its exact format, and its effect. Clarify which resource it applies to.
- **Implicit behaviors**: Processing order, default injection of headers,
  automatic wrapping (like ConditionalFilter), config preprocessing by controller
- **null vs empty vs absent**: If a field behaves differently when `null`, `[]`,
  or omitted, spell it out (e.g., `requestHeaders: null` = forward all,
  `requestHeaders: []` = forward all, `requestHeaders: ["X-Custom"]` = only those)
- **Interaction with other plugins**: Execution order, data dependencies
  (e.g., "RealIp must run before RateLimit if using client IP key")
- **Controller-side processing**: If the controller resolves Secrets, enriches
  config, or performs preprocessing, describe what happens between user YAML and
  runtime config. Mention `resolved_*` / `runtime_*` / `#[schemars(skip)]` fields
  and their purpose.
- **Status reporting**: How errors or warnings surface in CRD status conditions.

## 注意事项 (Important Notes)

Numbered list of caveats, gotchas, and operational considerations:
1. Security implications (e.g., "必须使用 HTTPS")
2. Performance impact (e.g., memory, CPU, connection pooling)
3. Scope limitations (e.g., "全局生效，无法针对单个监听器配置")
4. Update/reload behavior (e.g., "修改 Secret 后自动重新加载，可能有短暂延迟")
5. Cross-namespace requirements

## 当前限制 (Current Limitations)

Explicit list of what is NOT supported yet. Use this format:

1. **<Limitation title>**
   - What: brief description
   - Workaround: if any, describe it; otherwise state "暂无"
   - Tracking: link to issue or TODO if available

This section MUST exist even if empty (write "暂无已知限制" in that case).

## 故障排除 (Troubleshooting)

Common problems in Q&A or symptom-cause-fix format:

### 问题 N：<Symptom>

**原因**：...
**解决方案**：...（include concrete commands/YAML）

## 完整示例 (Complete Example)

A full, end-to-end example showing the feature in a realistic setup with all
related resources (Gateway, HTTPRoute, EdgionPlugins, Secrets, Services).
Should be directly `kubectl apply`-able.

## 相关文档 (Related Docs)

Bullet list linking to related pages with brief descriptions.
```

---

## No-Hidden-Logic Principle

> **Every behavior that affects request processing must be explicitly documented.
> If the user would be surprised by a behavior, it MUST be in the docs.**

### Checklist — Apply to Every Document

- [ ] **Annotations**: All `edgion.io/*` annotation keys are listed with exact syntax, target resource, and effect.
- [ ] **Default values**: Every field's default is documented. Dynamic defaults are explained.
- [ ] **Processing order**: If multiple plugins/filters interact, the execution order is stated.
- [ ] **Controller enrichment**: Any field that gets auto-populated (e.g., `resolved_secret` from `secretRef`) is explained from the user's perspective ("controller automatically resolves Secret references").
- [ ] **Conditional behavior**: Fields that change meaning based on other fields are documented (e.g., `key.name` required only when `source` is Header/Cookie/Query).
- [ ] **Error behavior**: What happens on invalid config, missing references, timeout, and unreachable dependencies.
- [ ] **Ambiguous terms**: If something could be misunderstood (e.g., "interval" could mean sliding window or fixed window), add a clarification.
- [ ] **Non-obvious interactions**: Cross-plugin dependencies, ordering requirements, and mutual exclusions.

### Annotation Documentation Template

When documenting a feature that relies on annotations, always include this block:

```markdown
## Annotation 参考

> **🔌 Edgion 扩展 Annotation**
>
> 以下 Annotation 使用 `edgion.io/` 前缀，为 Edgion Gateway 的扩展配置。

| Annotation | 适用资源 | 类型 | 默认值 | 说明 |
|------------|----------|------|--------|------|
| `edgion.io/xxx` | Gateway / TCPRoute / ... | string | `"..."` | Exact behavior |

**行为细节**：
- 设置为 `"true"` 时：<exact effect>
- 不设置或设置为 `"false"` 时：<exact effect>
- 与 <other feature> 的交互：<description>
```

---

## AI-Friendly Writing Principles

These rules make docs easier for AI to parse, reference, and generate:

1. **Consistent heading levels**: H1 = page title, H2 = major sections (follow the template), H3 = subsections, H4 = sub-subsections. Never skip levels.

2. **Parameter tables over prose**: Always use tables for configuration parameters. Prose explanations go below the table.

3. **Explicit over implicit**: State the obvious rather than assuming knowledge. E.g., write "该字段的值为字符串类型，不是布尔类型（使用 `\"true\"` 而非 `true`）".

4. **YAML as the primary config format**: All examples in YAML. Include complete resource definitions with `apiVersion`, `kind`, `metadata`.

5. **Searchable keywords**: Use exact field names from CRD in backticks (`fieldName`). Include both Chinese and English terms for key concepts on first use.

6. **Self-contained sections**: Each H2 section should be understandable on its own. AI tools often extract individual sections.

7. **No dangling references**: Never reference a file that doesn't exist. If a feature is planned but not documented, use "（即将推出）" or remove the link entirely.

8. **Mark Edgion extensions clearly**: Every non-standard-Gateway-API feature must have the `🔌 Edgion 扩展` callout at the top.

---

## Quality Checklist — Run Before Finishing Any Document

### Content Quality

- [ ] 概述 clearly states what the feature does and who it's for
- [ ] 快速开始 is copy-pasteable and actually works
- [ ] Every config field is documented in a table
- [ ] At least 3 usage scenarios with complete YAML
- [ ] Annotations section present (if applicable) with exact keys and behavior
- [ ] Behavior details cover null/empty/absent differences
- [ ] 当前限制 section exists and is honest about gaps
- [ ] 故障排除 covers the top 3 most likely issues
- [ ] 完整示例 includes all necessary resources

### Structural Quality

- [ ] Follows the document structure template (section order matters)
- [ ] Heading levels are correct and consistent
- [ ] All internal links are valid (no broken links)
- [ ] File is in the correct guide (user/ops/dev) per classification rules
- [ ] README.md index is updated to include the new document
- [ ] 🔌 Edgion 扩展 callout present for non-standard features

### No-Hidden-Logic Compliance

- [ ] No annotation goes undocumented
- [ ] No implicit/automatic behavior is hidden
- [ ] Ambiguous field behaviors are clarified
- [ ] Controller preprocessing steps are explained from user perspective
- [ ] Plugin interaction/ordering is documented
- [ ] Error/fallback behaviors are specified

---

## Handling Duplicate or Overlapping Topics

When the same feature has docs in multiple locations (e.g., RateLimit appearing in both `user-guide/edgion-plugins/` and `user-guide/http-route/filters/edgion-plugins/`):

1. **Each doc must clearly state its scope** at the top. E.g.:
   - "本文档描述 RateLimit 插件在 **单实例模式** 下作为 HTTPRoute Filter 使用的配置"
   - "本文档描述 RateLimit 插件的 **集群级分布式限流** 功能"
2. **Cross-link** the other document in both the overview and related docs sections.
3. **Avoid content duplication** — shared concepts should be in one place, referenced from the other.

---

## README Index Maintenance

Every `README.md` directory index must:

1. **List only existing files**. Never link to planned-but-not-written docs without marking them as `（即将推出）`.
2. **Include a one-line description** for each link.
3. **Mark Edgion extensions** with 🔌 emoji.
4. **Group logically** (matches, filters, backends, etc.).
5. **Stay in sync** — when adding/removing a doc file, update the parent README.

---

## Style Conventions

1. **Code blocks**: Always specify language (`yaml`, `bash`, `json`, etc.).
2. **CRD field names**: Use backticks and camelCase as they appear in YAML (`secretRefs`, not `secret_refs`).
3. **Status badges in tables**: Use ✅ for required, ❌ for optional.
4. **Security warnings**: Use blockquotes with ⚠️ prefix for security-critical notes.
5. **Edgion extension callout**: Use the exact format:
   ```markdown
   > **🔌 Edgion 扩展**
   > 
   > <description of what CRD/Annotation this is>
   ```
6. **Mixed-language text**: Feature names, resource names, and API terms stay in English. Explanations and descriptions in Chinese.
7. **Line length**: No hard wrap. Let the renderer handle it.
8. **Emoji usage**: Only use designated emojis (🔌 for extensions, ⚠️ for warnings). Do not add decorative emojis.

---

## Existing Document Quality Issues to Be Aware Of

When writing new docs or editing existing ones, watch for these known issues in the current `zh-CN/` docs:

1. **Broken links in README indexes**: `ops-guide/README.md` and `user-guide/README.md` link to many non-existent files (tcp.md, grpc.md, gateway-api filter pages, grpc-route/, udp-route/, advanced/). Fix or mark as `（即将推出）` when touching these files.

2. **Inconsistent RateLimit docs**: Two separate rate-limit docs exist with overlapping content. Ensure each clearly states its scope.

3. **Plugin-composition.md language mismatch**: Parts are in English, should be zh-CN.

4. **Cross-reference errors**: `dev-guide/annotations-guide.md` links to `../user-guide/http-to-https-redirect-guide.md` which doesn't exist (correct path is `../ops-guide/gateway/http-to-https-redirect.md`).

5. **Missing getting-started docs**: `installation.md`, `first-gateway.md`, `concepts.md` are linked but don't exist.

6. **Rust code in user docs**: `lb-algorithms.md` contains implementation-level Rust code that may not belong in user-facing documentation. Evaluate if it helps users or just adds noise.

---

## Workflow Summary

When asked to write or update documentation:

1. **Classify** → Which guide does it belong to?
2. **Locate** → Check if docs already exist for this topic; read them.
3. **Structure** → Follow the document structure template.
4. **Write** → Apply No-Hidden-Logic principle and AI-Friendly writing rules.
5. **Cross-reference** → Add links from/to related docs.
6. **Update index** → Update the parent README.md.
7. **Validate** → Run the Quality Checklist.
