---
name: gateway-optional-subsystem-features
description: Use when reviewing Gateway Cargo features that compile optional subsystems (Kafka, OTLP, GeoIP, Wasm) in or out, or a finding that the Gateway links every integration unconditionally.
knowledge_type: intentional-design
---

# Gateway Optional Subsystems Are Cargo Features With Fail-Closed Placeholders

## Conclusion and scope

`edgion-gateway` exposes four additive subsystem features — `kafka`, `otlp`,
`geoip`, `wasm` — aggregated by `full`, which is in `default`. Each
feature owns its third-party dependencies (`rskafka`; `opentelemetry*`;
`maxminddb` + `memmap2`; `wasmtime` + `edgion-wasm`). The former `ldap` feature
was removed on 2026-11-09 when `ldap3` was replaced by the in-tree `edgion-ldap`
leaf: LDAP adds no third-party dependency beyond tokio's I/O traits,
`thiserror` and `zeroize`, which the workspace already uses, so it compiles unconditionally (see `skills/04-review/ldap-auth/regression-guards/bounded-response-parsing.md`). The canonical reference is
`skills/02-features/01-binary-and-deployment/02-feature-flags.md`.

When a feature is off, the module swaps in a placeholder with the same API:

- Kafka/OTLP: `providers/{kafka,otlp}/client_disabled.rs` define an uninhabited
  client whose construction fails, so the LinkSys runner never publishes a client;
  `KafkaLogSender::new` / `OtlpLogSender::new` fail after validating the reference.
  A primary Kafka/OTLP log output therefore fails logger creation and Gateway
  startup. A Kafka/OTLP fallback output is dropped with a WARN by the existing
  fallback policy, and an OTLP SSL-log output fails startup (the full build only
  warns that OTLP is unsupported for SSL logs).
- Wasm: `wasm/plugin_disabled.rs` fails `validate_tls_entry`, so the plugin
  runtime records the entry in `invalid_plugins` and the route fails closed. `wasm/request_disabled.rs` keeps the
  request-context slot type; `wasm/admission.rs` and `wasm/limits.rs` stay compiled
  so `wasm_limits` configuration still parses and validates.
- GeoIP: `GeoIpProvider::load_from` and `updater::validate_structural` return an
  error; `geoip::init` logs an error when `db_path`/`update_url` is configured, and
  lookups report `Unavailable`, so each `GeoIpLocation` plugin applies its own
  `failOpen` (identical to the existing missing-database semantics).

No request-path check is added: the default build's request path is unchanged,
and placeholders reuse existing failure paths. Two cases accept configuration that
then has no effect, both signalled only by logs: a compiled-out fallback log
output, and a `GeoIpLocation` entry whose `failOpen`/parse-only setting passes
traffic while lookups are `Unavailable` (identical to a missing database).

## Rationale and evidence

Finding `arch-03` (P3) reported that every integration was linked into the Gateway,
so its CVE surface applied to operators who never used it, and no minimal build was
possible. The user approved implementing the subsystems with isolated, heavy
dependencies on 2026-11-06. Selection criteria: the dependency is used only by
that subsystem, and the subsystem has a natural fail-closed seam.

Placeholders were chosen over `cfg` at every call site: types stay identical, so
`store.rs`, `plugin_runtime.rs`, `ctx.rs`, and the session traits needed no gating,
and an uninhabited client (`match self.never {}`) proves at compile time that no
disabled client instance can exist.

## Alternatives and limits

- Not optional: `edgion-waf` (required by `edgion-resources` rule validation, which
  the Controller also runs, and by AI usage parsing through its bounded JSON reader),
  Redis (rate limiting, AI quota, DSL builtins, admin API), webhook and local-file
  outputs (core outbound and default logging), etcd (owned by `edgion-common`).
  Elasticsearch has no dedicated dependency, and the AI gateway is cross-cutting.
  These are follow-up candidates in `tasks/todo/arch-03-build-time-extension-selection.md`.
- Accepted gap: the Controller does not know which subsystems a Gateway binary
  contains, so it can report `Accepted=True` for a `Wasm` plugin or a
  Kafka/OTLP LinkSys that a core Gateway rejects. The Gateway still fails closed;
  only the status is optimistic. Capability reporting is tracked in the task above.
- Diagnostics: a compiled-out `Wasm` entry is reported with the existing generic
  "plugin construction failed" text, because the runtime's Wasm preflight branch
  discards the validator error. Fail-closed still holds; surfacing the reason would change the
  default build's error text and its Controller/Gateway parity tests, so it is left
  to the follow-up task. `--print-build-capabilities` shows the missing subsystem.
- An explicit `--no-default-features` list must name `full` (or individual
  subsystems). `cicd/build-image.sh` passes `FEATURES` verbatim, so the FIPS recipe
  is `FEATURES="allocator-jemalloc,fips,full"`.

## Verification and re-evaluation

Verified on 2026-11-06 with `cargo check -p edgion-gateway --all-targets` for the
default build, the core build (`--no-default-features --features
allocator-jemalloc,boringssl`), and each single-subsystem-off build, all warning
free; and `cargo test -p edgion-gateway --lib` on the core build (all pass).
Core-only regression tests cover the placeholders:
`compiled_out_kafka_rejects_a_valid_reference`,
`compiled_out_wasm_request_and_attached_entries_fail_closed` (request-only and
attached response entries), and
`compiled_out_geoip_never_loads_a_configured_database`. The CI `gateway-core` job
runs the core check and core library tests with `-D warnings`, and the
`gateway-build` job asserts `--print-build-capabilities` reports
`subsystem-<name>=enabled` for every subsystem in the default build. Tests that
need a real subsystem are gated with `#[cfg(feature = "...")]` or
`required-features`. `cicd/build-image.sh` warns when an explicit `FEATURES` list
names no subsystem.

Re-evaluate when: a gated dependency gains a second consumer outside its
subsystem; `edgion-resources` stops depending on `edgion-waf`; Gateway capability
reporting to the Controller is implemented (the accepted status gap closes); or a
new heavy optional integration is added (it should follow this pattern).

## Resolution references

- `arch-03` — build-time extension selection; dependency hygiene (`instant-acme`
  removal, `rcgen` to dev-dependencies, single `tonic`) landed earlier on
  `feature-10-28`; subsystem features implemented 2026-11-06; remainder deferred to
  `tasks/todo/arch-03-build-time-extension-selection.md`.
