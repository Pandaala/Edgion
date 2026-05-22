# Page Template

This document defines the standard section structure for each kind of documentation page. Each template lists **required** and **optional** sections.

## Plugin page template

Applies to: `user-guide/http-route/filters/edgion-plugins/*.md`, `user-guide/edgion-plugins/*.md`

```markdown
# <Plugin name>

> **🔌 Edgion extension**
> 
> <Plugin name> is a <one-sentence functional description> provided by the `EdgionPlugins` CRD; it is not part of standard Gateway API.

## What is <concept>?                  [Optional, add if the concept is not self-evident]

Explain the concept in 2–3 sentences using non-technical language. Pair with a simple analogy or example.

## Quick Start                         [Required]

### Simplest configuration

A minimal viable YAML snippet that works.

### Default-value notes                [Optional]

Key default behaviors, especially security-related defaults.

---

## Configuration parameters            [Required]

(Parameter table format: see §Configuration parameter table conventions below)

### Basic parameters
### Security parameters                [Optional, when there are security-related parameters]
### Advanced parameters                [Optional, when there are advanced / less-common parameters]
### Special-value notes                [Optional, when there are wildcards or other special semantics]

---

## Common configuration scenarios      [Required, at least 3 scenarios]

### 1. Scenario name

Description + complete YAML example. Order scenarios from simple to complex.

---

## FAQ                                 [Recommended]

(FAQ format: see §FAQ and troubleshooting conventions below)

---

## Security best practices             [Optional, required for security-related plugins]

### ✅ Recommended
### ❌ To avoid

---

## Performance tuning                  [Optional]

---

## Complete configuration example      [Recommended]

A production-grade complete YAML, including HTTPRoute + plugin configuration.

---

## Related links                       [Required]

- External references (RFC, MDN, etc.)
- Internal links to related doc pages
```

## Gateway API Filter page template

Applies to: `user-guide/http-route/filters/gateway-api/*.md`

```markdown
# <Filter name>

<No Edgion-extension callout needed>

## Overview                            [Required]

1–2 paragraphs describing what the Filter does.

## Configuration                       [Required]

YAML example + parameter description.

## Examples                            [Required]

### Example 1: <scenario>
### Example 2: <scenario>

## Notes                               [Optional]

## Related documents                   [Required]
```

## Route-type overview page template

Applies to: `user-guide/http-route/overview.md`, `user-guide/grpc-route/overview.md`, etc.

```markdown
# <Route type> Overview

<One sentence describing the purpose of this Route type>

## Basic structure                     [Required]

Minimum YAML example + key field notes.

## Core concepts                       [Required]

### ParentRefs
### Rules
### Matches
### Filters / Plugins
### BackendRefs

For each concept, give 2–3 sentences plus a link to the detailed doc.

## Complete example                    [Required]

A complete YAML covering common features.

## Subdocument navigation              [Required]

| Topic | Document | Description |
|-------|----------|-------------|
| Match rules | matches/ | ... |
| Filters | filters/ | ... |
| Backend configuration | backends/ | ... |
```

## Ops configuration page template

Applies to: feature-configuration pages under `ops-guide/`.

```markdown
# <Feature name>

<One sentence summary>

## Overview                            [Required]

What the feature is and when you need it. May use ASCII diagrams to illustrate the data flow.

## Configuration                       [Required]

### Method 1: <standard approach>

YAML example.

### Method 2: <extension approach>     [Optional, when there is an Edgion extension]

> **🔌 Edgion extension**

YAML example.

## <Detail sections>                   [As needed]

E.g. certificate management, parameter tuning.

## Examples                            [Required]

### Example 1: <scenario>
### Example 2: <scenario>

## Related documents                   [Required]
```

## Getting Started page template

Applies to: onboarding pages under `getting-started/`.

```markdown
# <Title>

<Opening sentence describing the goal of this page>

## Step 1: <action>                    [Required]

The minimum knowledge needed + a YAML example.
End each step by stating "what this step has established".
Link to deeper documents: "See more: …"

## Step 2: <action>

...

## Step N: Understanding the overall flow [Required]

Tie the previous steps together and describe the end-to-end flow.

## Next steps                          [Required]

Branch by reader intent:
- "If you want to keep doing X, read …"
- "If you want to learn about Y, read …"
```

## General section ordering

Regardless of page type, section order follows the **progressive complexity** principle:

```
Overview / Quick Start (what is it, simplest usage)
  ↓
Configuration parameters (how to configure)
  ↓
Common scenarios (real-world usage examples)
  ↓
Advanced topics (security, performance, edge cases)
  ↓
FAQ / Troubleshooting
  ↓
Related links
```

---

## Configuration parameter table conventions

### Standard header

All configuration parameter tables use a unified four-column format:

```markdown
| Parameter | Type | Default | Description |
|-----------|------|---------|-------------|
```

Chinese version: same four columns, with header cells translated to the Chinese equivalents of "Parameter", "Type", "Default", and "Description".

### Parameter column

- Wrap with backticks: `` `allow_origins` ``
- Use dot notation for nested fields: `` `tls.mode` ``
- Match the field name in YAML exactly (snake_case or camelCase, depending on the actual schema)

### Type column

Use a unified set of type labels:

| Type | Notation | Maps to |
|------|----------|---------|
| String | String | Rust `String` / YAML string |
| Integer | Integer | Rust `i32`/`u32`/`i64`/`u64` |
| Boolean | Boolean | Rust `bool` |
| Array of strings | Array\<String\> | Rust `Vec<String>` |
| Array of objects | Array\<Object\> | Rust `Vec<T>` |
| Enum | Enum | Rust `enum`; list possible values in the description |
| Object | Object | Rust `struct`; describe with a separate sub-table |
| Duration | Duration | E.g. `"30s"`, `"5m"` |

Do not use Rust type names (such as `Option<String>`) or JSON Schema type names.

### Default column

| Case | Notation |
|------|----------|
| Has a default | Backticks: `` `"GET,HEAD,OPTIONS"` `` |
| No default (required) | `—` (required) or **Required** |
| No default (optional) | `none` |
| Empty string | `` `""` (empty) `` |
| Boolean default | `` `false` `` or `` `true` `` |

### Description column

- One sentence stating the purpose of the parameter
- Mark required fields as **Required** (or note this in the default column)
- If the value space is bounded, enumerate the values: `Allowed values: \`Terminate\` | \`Passthrough\``
- Provide an example of a typical value: `Example: \`"https://app.com,https://admin.com"\``
- Describe multi-value separators: `Use commas to separate multiple values`

### Parameter grouping

When parameters exceed 6, group them by function and use H3 headings. Common group names:
- **Basic parameters** — required and most common parameters
- **Security parameters** — authentication, authorization, encryption
- **Advanced parameters** — less common, tuning, edge cases
- **Performance parameters** — caching, timeouts, concurrency

### Nested object handling

**Method 1: Sub-table** (for deeper nesting):

```markdown
| `healthCheck` | Object | none | Health check configuration; see the table below |

#### `healthCheck` sub-fields

| `interval` | Duration | `"10s"` | Check interval |
| `timeout` | Duration | `"5s"` | Timeout |
```

**Method 2: Dot path** (for shallow nesting):

```markdown
| `healthCheck.interval` | Duration | `"10s"` | Health check interval |
| `healthCheck.timeout` | Duration | `"5s"` | Health check timeout |
```

### Enum handling

- **Inline list** (≤ 3 values): `Allowed values: \`Terminate\` | \`Passthrough\``
- **Separate table** (> 3 values): use a sub-table to list each value with a description

### Special-value notes

When a parameter has wildcards or magic values, add a "Special values" table after the parameter table:

```markdown
| Value | Meaning | Use case |
|-------|---------|----------|
| `*` | Wildcard, allow all | **Not recommended in production** |
| `""` (empty string) | Reject all | Default value, safe |
```

---

## FAQ and troubleshooting conventions

### When a FAQ section is needed

| Document type | FAQ | Notes |
|---------------|-----|-------|
| Plugin page | Recommended | Configuration mistakes are the most common issue |
| Route overview page | Optional | Conceptual questions |
| Ops configuration page | Recommended | Deployment and debugging issues |
| Getting Started | Not needed | Onboarding pages don't suit FAQs |

### Q/A format

```markdown
### Q1: Question title (use a question form)

**A**: The answer. Lead with the conclusion, then expand.
```

Rules:
- Use `### Q1:` numbering + an interrogative form
- Begin the answer with a bold `**A**:`
- The first sentence gives the answer
- Leave a blank line between Q/A pairs

### Right / wrong example markers

Use ✅/❌ to mark correct vs. incorrect approaches. You can compare inline within code blocks, or use `### ✅ Recommended` / `### ❌ To avoid` section titles.

### Troubleshooting steps

- Short troubleshooting (≤ 3 steps): list the steps directly in the answer
- Detailed troubleshooting (> 3 steps): use a separate section with commands + checkpoints + key terms

### Question priority order

1. Configuration mistakes → 2. Conceptual understanding → 3. Environment differences → 4. Debugging methods → 5. Performance tuning → 6. Compatibility

### "Get help" section

Add to the end of every page that has a FAQ:

```markdown
## Get help

If you run into problems:

1. Check the FAQ section of this document
2. Check the gateway logs
3. Open an issue on GitHub
```
