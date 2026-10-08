# Implementation and verification

## Changes

- Schema-hidden invalid unit markers at the two extension discriminators; shared
  opaque-config discard helper also used by existing plugin sentinels.
- Existing Controller dependency clearing, status and conf_sync remain the path for
  invalid replacements. Gateway provider retirement and atomic ConfigData indexes
  process unsupported generations without retaining stale typed state.
- Open-string CRD contract guard replaces enum parity. Known config and credential
  substructures keep their existing validation and redaction.
- Dynamic `LinkSysProviderReplacement` now includes unknown replacement/recovery for
  Webhook and Redis. `RegionRouteValidation` includes unknown overlay fallback and
  recovery without rewriting consumers.

## Coverage plan

| Boundary | Verification |
| --- | --- |
| Admission schema | Python and Rust open-string/config preservation guards; live API server not exercised |
| Typed ingest and transport | Mixed list/watch JSON/YAML, strict gate on/off, opaque payload discard, invalid sentinel round-trip, malformed known types |
| Controller | Current-generation status, dependency cleanup/requeue, credential-source desired/results cleanup, strict trust/WAF consumers |
| Gateway full/partial | Old provider retirement/recovery and ConfigData typed matcher/proof removal/recovery |
| Cross-process hot reload | Two existing dynamic scenarios through the shared topology |

## Execution record

| Command / scope | Result |
| --- | --- |
| `cargo fmt --all` | Passed; final `-- --check` also passed |
| `cargo check --workspace --all-targets` | Passed after final serializer correction |
| `cargo clippy --workspace --all-targets` | Passed after final serializer correction |
| `cargo test -p edgion-resources --test open_extension_types --test crd_coverage --test plugin_crd_type_coverage --test stream_plugin_crd_type_coverage` | Passed 29 tests after final correction |
| `cargo test -p edgion-controller --lib unsupported_extensions_create_and_update` | Passed 1 regression using actual committed CRDs and real Admin create/update handlers |
| `cargo test -p edgion-resources -p edgion-common -p edgion-controller -p edgion-gateway --lib unsupported` | Passed 76 tests after final serializer correction |
| `cargo test -p edgion-controller --lib dependency_requeue_tests` | Passed 5 tests |
| `cargo test -p edgion-tests --lib current_projection` | Passed 2 tests |
| Agent docs / SSA force / metrics / Pingora source / Gateway API transform / unit layout guards | Passed, including final correction state |
| Open extension / overlay signal / no legacy data refs / Gateway no-tracing guards | Passed |
| Overlay guard temporary-tree mutation checks | Positive source and 7 rejection cases passed |
| Canonical integration `prepare.sh` | Passed after final correction (357 seconds) |
| Dynamic LinkSysProviderReplacement | Passed after correction; both Webhook and Redis unknown/recovery transitions |
| Dynamic RegionRouteValidation | Passed; unknown overlay falls back to base and recovers without rewriting consumers |

The open-extension target covers all 17 known enum variants, schema tag parity,
wire round-trips, mixed JSON/YAML list/watch decoding, opaque-secret discard and
concrete WAF content redaction. The API regression also removes `config` from the
normalized object to prove the actual schema rejects an incomplete envelope.

Full `cargo test --all` was not run. Live Kubernetes API-server admission was not
exercised; committed CRD validation, typed watch decoding, projection tests and the
native shared topology cover distinct boundaries. See the review decisions for
findings discovered during execution and the corrections they required.

Both dynamic scenarios ran sequentially against the canonical shared native
topology (`EdgionPlugins/base,HTTPRoute/Basic`) with the repository Redis fixture.
The wrapper restored the pre-existing Redis state and local CLI token, and stopped
only native processes launched from this workspace. No commit or push was made.
