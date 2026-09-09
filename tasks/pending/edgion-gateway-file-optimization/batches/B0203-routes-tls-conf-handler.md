# B0203 — TLS Configuration Handler

## Scope

- Pipeline state: analysis-ready; queued, not the active mutation batch.
- Active anchor crate: `edgion-gateway`.
- Target: `edgion-gateway/src/routes/tls/conf_handler_impl.rs`.
- Baseline: `9853a685`.
- Analyst: `b0203_tls_handler_analysis`.
- Investigate full/delta schema and compiled-unit ownership, initialization,
  shared rebuild boundaries, locking and per-listener publication.
- Existing integration files and user-owned paths remain protected. No
  implementation is authorized yet.

## Analysis Outcome

- Decision: change; simple. Target blob `8a82db31ebf7f586c0b22a353fd86a7a8a2c8e82`; fresh handler baseline 7/7 passed.
- Full/delta paths deep-copy TLSRoute then immediately wrap it in another Arc. TlsRouteUnit and both proxy consumers only borrow the schema; preserve the input Arc directly through initialization instead.
- Keep Result adapter, shared delta helper, rebuild lock, fixed logs, resolved listener/SNI fields, denied weights, first-rule selection and per-listener atomic snapshots unchanged. This removes cold-path copy/allocation work, not measured request latency.
- Exact proposed scope: target; new TLS schema-Arc sharing regression page; routing index; `skills/01-architecture/05-resources/07-tls-route.md` only to correct its claim that initialization validates rather than compiles.
- Preserve all seven tests and verify full/add/update pointer identity, multi-SNI unit sharing, listener migration, retained old schema/hostname/backend generation and sibling selector state. Run all TLS route tests; Arc identity does not need integration additions.
- Stale manager-cache commentary belongs to existing architecture-documentation follow-up; TLSRoute version fallback is D0111, not this runtime batch.
