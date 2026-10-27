---
name: dsl-builtins-path
description: DSL builtin path/uri/query variable reference — views, behavior, and usage guidance for req.path / req.uri / req.request_path / req.request_uri / req.query / req.query_string.
---

# DSL builtins: path / uri / query variables

> Full architectural context: [../../01-architecture/02-gateway/uri-normalization/SKILL.md](../../01-architecture/02-gateway/uri-normalization/SKILL.md)

---

## Variable cheat sheet

| Variable | View | Meaning | Affected by rewrite |
|------|------|------|----------------|
| `req.path` | current (normalized) | Current path, post-rewrite | Yes |
| `req.uri` | current (normalized) | Current path + current query | Yes |
| `req.query_string` | current | Whole current query string, raw (not decoded, not normalized) | Yes |
| `req.query("name")` | current | Single query parameter, **percent-decoded** (`+` → space) | Yes |
| `req.request_path` | entry (frozen) | Raw client-entry path | No |
| `req.request_uri` | entry (frozen) | Raw client-entry path + raw query | No |

Allocation counts are implementation details, not a script contract: `Value::Str` uses
`Arc<str>`, and formatting/ownership depends on the selected view. Benchmark the actual
call path before making an optimization claim.

`req.query("name")` uses the logical current Query through
`PluginSession::get_query_param_ref`. Until an edit it reuses the original backing;
after replacement or removal it reads the current backing or selection. Tokens and
decoded values initialize on demand and are reused. A missing current parameter
never falls back to its entry value.

### Behavior changes (migration note)

- **`req.path` / `req.uri`** semantics switched from "raw current" to "normalized current":
  - Values are unchanged for already-normalized requests
  - For requests containing `%XX` encoding (except unreserved characters) / double slashes / dot-segments / path parameters, the normalized form is returned
- **`req.request_path` / `req.request_uri`** are newly added variables that reflect the raw entry form
- **`req.query("name")`** returns the **percent-decoded** value sourced from the logical current query string. Callers comparing literals must use the decoded form (e.g. compare against `"a b"`, not `"a%20b"`)

---

## Usage guidance

### Routine routing and ACL

```dsl
# Correct: use the current view for ACL (covers all encoding variants)
if req.path == "/admin" { deny }
if req.path starts_with "/internal/" { deny }
```

### Audit and logging

```dsl
# Record the URI actually sent by the client (for auditing)
set_header "X-Entry-Uri" req.request_uri

# Record the URI forwarded to upstream after rewrite
set_header "X-Upstream-Path" req.path
```

### Entry raw deny (use with care)

```dsl
# Scenario: deny by literal raw form (rare cases)
# Note: such rules may falsely reject legitimate clients
if req.request_path contains "%2F" { deny }
```

---

## RegularExpression — important callout

**When matching `req.path` / `req.uri` with regex in DSL, the request side has already been normalized**. The following literals **will never match** (silently fail):

- `%XX` literals (e.g., `%2F`, `%61`)
- `//`, `/./`, `/../`
- `..;`, `;param` forms

```dsl
# Wrong: the regex below never matches req.path (request side is already normalized)
if req.path matches ".*%2F.*" { deny }      # %2F has been decoded to /
if req.path matches ".*\/\/.*" { deny }     # // has been merged
if req.path matches ".*\/\.\..*" { deny }   # /.. has been eliminated

# Correct: write regex against the normalized space
if req.path matches "^/admin(/.*)?$" { deny }

# If raw-form regex is needed, use req.request_path
if req.request_path matches ".*%2[fF].*" { deny }
```

---

## Implementation locations (internal reference)

- DSL builtin registration: `edgion-dsl/src/compiler/builtins.rs`
- DSL builtin evaluation: `edgion-gateway/src/plugins/http/dsl/lang/builtins.rs`
- session trait: `edgion-gateway/src/plugins/runtime/traits/session.rs`
- ctx canonical API: `edgion-gateway/src/ctx.rs`
