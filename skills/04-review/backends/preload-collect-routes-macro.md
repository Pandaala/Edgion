---
name: preload-collect-routes-macro
description: Read when reviewing LB-preload collect_http_routes / grpc / tcp / udp / tls in preload.rs having five byte-for-byte identical bodies / extract a generic helper or trait to dedup the namespace-fallback + key-format logic (backends-F009).
---

# backends-F009 — preload collect_*_routes five identical bodies

**Status / verdict**: fixed (backends-F009, 2026-05-24)

When reviewing LB-preload findings of the form "`collect_http_routes` / `grpc` / `tcp` / `udp` / `tls` in `preload.rs` have five byte-for-byte identical bodies / extract a generic helper or trait to dedup the namespace-fallback + key-format logic (backends-F009)": fixed (backends-F009, 2026-05-24).

`preload_load_balancers` (`edgion-gateway/src/backends/preload.rs`) now generates the five collectors from a local `macro_rules! collect_routes` parameterized only by the `list_*_routes` method name; the five standalone `fn collect_*_routes` are gone. A macro (not a trait/generic) is the idiomatic choice because the five route types share the `metadata.namespace` / `spec.rules` / `rule.backend_refs` / `br.{namespace,name}` shape but are distinct concrete types with no shared trait — a trait abstraction would need ~15 impls to replace ~90 lines (net negative), and the sibling `config_client.rs::apply_change!` already establishes the local-macro pattern for this exact route-type fan-out. Pure no-behavior-change DRY refactor (macro expansion is byte-for-byte equivalent to the old bodies; startup-only preload path, not hot path). The trait/generic-helper suggested fix is not accepted. Same class as backends-F007 / backends-F008. Re-reports of the five-identical-copies form are not accepted; verify the current `preload_load_balancers` uses `collect_routes!` before re-reporting.
