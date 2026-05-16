---
name: schema-validator-non-k8s-asymmetry
description: Decide on "non-K8s mode watcher path bypasses schema validation / SchemaValidator only covers admin API / should inject validator into ingest path" findings. Architectural asymmetry, not a defect.
---

# SchemaValidator Non-K8s Asymmetry Is Architectural, Not a Gap

**Audit date**: 2026-05-16
**Linked finding**: c3a-013 (P2, architecture, confirmed → closed, accepted tradeoff)
**Conclusion**: **Current SchemaValidator placement is correct. Adding it to watcher paths
or removing it entirely both regress UX with no positive on correctness.**

## Why the asymmetry is fundamental

Three write paths in non-K8s mode, with very different control surfaces:

| Path | Can controller gate the write? | Feedback channel |
|---|---|---|
| Admin API (HTTP) | ✅ — request rejected with 400 before persist | Synchronous error |
| `conf_dir/*.yaml` direct write | ❌ — file is on disk before watcher sees event | Status conditions (async) |
| etcd direct put | ❌ — KV in etcd before watcher sees event | Status conditions (async) |

For the two direct-write paths, **whatever validator runs in the watcher cannot prevent
the bad resource from entering the store**. It can only enrich the status conditions
written back. The downstream handler chain (`parse_resource_content` → `handler.validate`
→ `handler.preparse` → `handler.parse` → `handler.update_status`) **already** writes
serde errors, semantic errors, and parse errors to `*.status.yaml` via
`status_handler.write_error_status` / `update_status`. Adding OpenAPI on top would
duplicate most of that, costing CPU at init for negligible marginal coverage.

K8s mode is correctly skipped (`load_schemas` returns `SchemaValidator::empty()`) because
the K8s API server already gates writes via CRD OpenAPI validation at admission time.

## What SchemaValidator actually buys

Only one thing, and it is real: **synchronous 400 on the admin API path** for programmatic
clients (CI pipelines, `edgion-cli`, custom tooling). Without it, those clients would need
a "write → poll status → translate conditions → derive error" round-trip. Hence keep.

## Fix suggestions not accepted

- "Inject `SchemaValidator::validate` into `parse_resource_content` / watcher init loop
  for FileSystem and Etcd ingest paths" — cannot prevent the on-disk/in-etcd bad value;
  status reporting via the handler chain already covers it; mostly redundant work.
- "Remove `SchemaValidator` entirely; rely on serde + handler.validate" — regresses admin
  API UX for programmatic callers (sync 400 → async status fetch).
- "Also run schema validation in K8s mode as a defense against CRD/server skew" — K8s
  mode does not load schemas (`empty()`); loading them only for defense is unjustified
  maintenance overhead.
- "`edgion-cli config lint <file>` as the canonical solution to this finding" — orthogonal
  feature; whether to add a CLI lint subcommand is a separate product decision.

## The one real adjacent problem (out of scope here)

Fields where OpenAPI catches but `serde` and `handler.validate` both miss — `format:
hostname`, numeric `minimum`/`maximum`, `pattern`, certain `enum` strings not modeled as
Rust enums — can pass through and become **silent misconfiguration at runtime** (SNI
never matches, defaults silently applied, etc.). The fix is **handler-side validation
completeness**, not validator relocation. Findings of that shape should be routed to the
relevant handler, not back to `SchemaValidator`.

## When to re-evaluate

- A new ingest path appears that **is** gateable (e.g., a write-through admin API for the
  etcd backend) — re-examine whether validation belongs there.
- Programmatic admin API usage drops to zero (e.g., all writes go through K8s or
  edgion-cli stops calling admin API) — removing `SchemaValidator` (option b) becomes
  cheaper.
- Telemetry shows the "silent misconfiguration" class is biting users at scale —
  prioritize the handler-completeness audit, still not validator relocation.
