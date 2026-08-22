---
name: deserialization-review
description: Use when reviewing JSON/YAML/protobuf/binary deserialization paths for panic-on-unknown, recursion depth, size limits, untrusted-input boundaries, and serde flatten footguns.
---

# Deserialization Safety Review

> Decision notes and checklists for auditing deserialization paths in Edgion.

## Threat Model

Untrusted or semi-trusted input can cross a deserialization boundary in four places:

1. **Admin API request bodies** — JSON posted to Controller or Gateway admin endpoints. Attacker-controlled if the admin endpoint is exposed without proper auth.
2. **CRD `spec` from Kubernetes** — Any user with `kubectl apply` rights can craft arbitrary YAML field values. The K8s API server validates CRD schema, but serde deserialization happens again in the Controller.
3. **gRPC streams from Controller / Center** — Config sync messages carrying serialized resource payloads. Attacker-controlled if the control plane is compromised or if a malicious controller is substituted.
4. **DSL bytecode load** — `CompiledScript::deserialize_base64` reads base64-encoded serde-json bytecode delivered via K8s CR or gRPC. This is the highest-risk path: a crafted payload can trigger serde-json panics, OOM, or `Constant::Regex` recompilation (ReDoS). See [dsl-bytecode-deserialize-safety.md](dsl-bytecode-deserialize-safety.md).

## General Principles

- **Size limits before deserialization**: reject oversized payloads at the framing layer (HTTP body length, gRPC message size, base64 string length) before calling `from_slice` / `from_str`. Do not rely on serde to bound depth or size.
- **Recursion depth**: serde-json does not enforce stack depth by default. Deeply nested JSON objects can overflow the stack. For untrusted JSON, prefer `serde_json::from_reader` with a custom depth-limited deserializer or pre-validate nesting depth.
- **`#[serde(deny_unknown_fields)]`**: use on internal structs that should be stable and closed. Omit on user-facing extension points. Missing this on an internal struct causes silent field drops that may mask attacks.
- **`deny_unknown_fields` is not a general-purpose typo defense — it cannot reach every shape, and where the project wants a warning instead of a hard reject, it is the wrong tool anyway.** It cannot be combined with `#[serde(flatten)]` (a serde limitation: a flattened field is deserialized through a buffering `Content`/`Value` seam that `deny_unknown_fields` cannot see past), and it hard-rejects the whole document, which is unwanted on an operator-authored CRD subtree the project has decided to warn-not-block on (`x-kubernetes-preserve-unknown-fields` plugin `config`; see `skills/00-design-rules.md`). For that shape use the schema-walk unknown-key detector instead — `edgion-resources::schema_check::unknown_key_paths` — which walks the `schemars`-generated JSON Schema against the raw input and sees through both `#[serde(flatten)]` and adjacently-tagged enums (measured comparison against the `serde_ignored` crate, which is blind through both: `skills/04-review/architecture/serde-ignored-blind-spots-schema-walk-instead.md`). Keep `deny_unknown_fields` for closed, stable, internal structs where a hard reject is exactly the desired outcome (credential sub-structs — see `deny-unknown-fields-credential-structs-forward-compat-tradeoff.md`); reach for the schema walk when the field is user-facing, needs to stay open for forward-compat, or the policy is warn-only.
- **`#[serde(flatten)]` footguns**: flattening a `HashMap<String, Value>` absorbs unknown fields silently. Review every flatten to confirm the absorb-all behavior is intentional.
- **Regex recompilation on deserialize**: any struct that contains a compiled regex and derives `serde::Deserialize` will recompile on every deserialization. Add `size_limit` / `dfa_size_limit` via `regex::RegexBuilder` to bound ReDoS risk.
- **Error handling**: deserialization errors must return a structured error to the caller, not panic. Verify call sites use `?` or match the `Result`; bare `.unwrap()` on a deserialization result is a P0 finding.

Coding rule references: see `skills/03-coding/02-rust-coding-rules.md` Rule 2 (no bare unwrap) and Rule 5 (graceful degrade in long-running paths).

## Known Rules in This Directory

| File | Topic |
|------|-------|
| [dsl-bytecode-deserialize-safety.md](dsl-bytecode-deserialize-safety.md) | DSL bytecode (`CompiledScript::deserialize_base64`) — size limits, depth, ReDoS via `Constant::Regex`; fuzz infrastructure gap |
| [linksys-auth-resolved-secret-clear-before-resolve.md](linksys-auth-resolved-secret-clear-before-resolve.md) | LinkSys auth — clear Controller-owned resolved Secret state before every resolution attempt |
| [plugin-auth-resolved-credentials-clear-before-resolve.md](plugin-auth-resolved-credentials-clear-before-resolve.md) | KeyAuth/JwtAuth — rebuild resolved credential maps only from Controller Secret resolution |
| [status-seed-must-preserve-ingest-sidecars.md](status-seed-must-preserve-ingest-sidecars.md) | FileSystem/etcd status seeding — preserve populated `#[serde(skip)]` ingest diagnostics across generic serde merging |
| [configdata-typed-policy-unknown-fields-are-advisory.md](configdata-typed-policy-unknown-fields-are-advisory.md) | EdgionConfigData policy data — typed non-credential fields warn through the schema walk; credential/auth sub-structures remain strict |
| [runtime-ignore-does-not-mean-no-unknown-field-diagnostic.md](runtime-ignore-does-not-mean-no-unknown-field-diagnostic.md) | Documentation — distinguish ignored runtime fields from advisory raw-input diagnostics and apply from validate verdicts |
| [unknown-field-tests-need-clean-and-typo-coverage.md](unknown-field-tests-need-clean-and-typo-coverage.md) | Unknown-field regression tests — cover clean and exact-typo cases for every tagged variant plus the public validation discriminator |

## Usage

When a code review surfaces a deserialization call:

1. Identify which threat-model bucket the call belongs to (see above).
2. Check that size limits exist at the framing layer.
3. Check that the error path does not panic.
4. Check for regex fields in the deserialized struct.
5. Consult the rule file for the specific subsystem if one exists.
