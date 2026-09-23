---
name: gateway-process-config-unknown-fields-advisory
description: Use when reviewing unknown-field handling in the local edgion-gateway process YAML or changing its Serde shape.
knowledge_type: intentional-design
---

# Gateway process-config unknown fields are advisory outside strict subtrees

## Conclusion and scope

The local Gateway process YAML reports ordinary unknown fields through bounded
startup ERROR diagnostics and continues with the recognized configuration. Each
diagnostic names the config file and full field path, sets `action="ignored"`,
and never includes the field value or raw YAML.

This advisory policy applies only to otherwise-lenient fields reachable from
`edgion_gateway::cli::config::EdgionGatewayConfig`. Existing strict subtrees
remain strict and still abort startup on unknown fields. These include
`GatewayConfig.geoip`, `path_normalization`, `request_mirror`,
`access_log_options`, strict output variants, and `LinkSysRef`. The detector is
not a replacement for `deny_unknown_fields` on credential or security
boundaries.

## Rationale and evidence

Gateway process configuration is local, restart-bound YAML loaded by
`EdgionGatewayConfig::load` before logging initialization. Historically, Serde
silently discarded unknown fields on the root and several nested objects. A
misplaced root `geoip` block therefore left `GatewayConfig.geoip` at its empty
default while the process started without any diagnostic.

D0005 resolved that silent no-op by wrapping the production YAML deserializer
with `serde_ignored` and retaining the existing
`singleton_map_recursive` adapter. The loaded result carries sorted, deduplicated
paths to `EdgionGatewayCli::run`, which emits ERROR logs immediately after binary-owned
logging is initialized. At most 32 paths are retained; a separate ERROR log reports
that additional findings were omitted.

The current reachable process-config tree has no Serde `flatten`, internally or
adjacently tagged enums, or untagged object enums. Clap `command(flatten)` does
not affect Serde. The externally tagged log-output enums remain covered by the
production deserializer and focused tests.

## Alternatives and limits

Failing startup for every unknown field would provide the strongest typo
protection, but the operator decision for this boundary is to report ordinary
unknown fields at ERROR level while continuing startup. The accepted cost is
that an operator can ignore the diagnostic and still run with a defaulted
setting. Documentation therefore describes both the ignored action and the
resulting fallback behavior.

Adding `deny_unknown_fields` to every shared Common or Resources type was also
rejected: it would change Controller and other consumers merely to implement a
Gateway-local policy. A second schema hierarchy or hand-maintained key list was
rejected because it could drift from the actual deserializer.

`serde_ignored` is not generally sufficient for arbitrary Serde models. It is
known to be blind through `#[serde(flatten)]` and adjacently tagged enums; see
[`serde_ignored` blind spots](../../resources/compatibility-constraints/serde-ignored-blind-spots-schema-walk-instead.md).
If either construct enters this process-config tree, re-evaluate this mechanism
and prefer the canonical schema walker rather than assuming the diagnostic remains
complete.

## Verification and re-evaluation

Focused Gateway config tests cover clean input, root and nested typos, externally
tagged local-file outputs and fallbacks, certificate arrays, legal aliases,
strict-subtree failures, no-file defaults, deterministic paths, and the 32-path
diagnostic cap. Repository-level verification is recorded in the closing report
for D0005 rather than treated as permanently current evidence here.

Re-evaluate this decision if:

- unknown fields should become fatal startup errors for the whole process file;
- a Serde flatten, internally/adjacently tagged enum, or untagged object enum is
  added to the reachable configuration tree;
- process configuration becomes remotely supplied or hot-reloaded;
- diagnostics gain a durable deployment-status or validation API surface.

## Resolution references

- `D0005` / `strict-gateway-process-config-unknown-fields` — fixed by advisory
  startup diagnostics while preserving existing strict subtrees.
