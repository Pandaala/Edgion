---
name: conf-sync-plaintext-default-observable
description: Use when reviewing conf_sync (Controller→Gateway) plaintext / secret-in-transit findings; records the 2026-07-14 decision to make an omitted conf_sync_security fail-close (refuse to start), what the old observable-but-not-fail-closed posture was, and why skip_tls remains the only plaintext path.
knowledge_type: regression-guards
---

# conf_sync Plaintext — Fail-Close (decided 2026-07-14)

## Status: CLOSED — plaintext-by-omission no longer exists

The conf_sync channel carries **resolved Secrets** (TLS private keys via
`TlsCerts::secret`, backend credentials via `SecretSlot.secret`) from the Controller
to the data plane. As of **2026-07-14** an omitted `conf_sync_security` section makes
**both binaries refuse to start**. Plaintext is now reachable only through the
explicit `skip_tls: true` opt-in.

The file name is kept because other docs link to it; the content is the decision
record, not an argument for the old posture.

## What the posture used to be (historical)

- 2026-06-21: the previously-*silent* omitted path was made loud — ERROR on both
  ends plus a `plaintext_default` system-error reason — and the shipped K8s manifests
  and the default integration stack were flipped to mTLS. The channel still **ran** in
  plaintext when the section was omitted ("observable, not fail-closed"), justified by
  "conf_sync is data-plane critical and there is no cert bootstrap yet".
- 2026-07-xx: auto-CA provisioning (Phase 1 + rotation Phase 2) landed, which removed
  the "no cert bootstrap" premise, and the gateway began failing closed for
  `auto_ca: true` + omitted certs. Only the legacy no-`auto_ca` omitted path stayed
  plaintext.

## The decision (2026-07-14)

The operator-responsibility model in this project is "do not hard-reject a configuration
that is *still safe*; validate, WARN, and expose an alertable signal instead". A
plaintext channel shipping TLS private keys is **not a still-safe configuration** — it is
an exposure — so the model never applied to it. A metric only helps a cluster that is
actually alerting on it; a forgotten config block should not silently downgrade the
transport that carries private keys.

Therefore: **plaintext must be an explicit, audited choice, never the consequence of an
omission.** The explicit escape hatch (`skip_tls: true`) already existed, so the
migration cost for an affected deployment is one config block.

## Behavior now

| Configuration | Behavior |
|---|---|
| `auto_ca: true` (shipped K8s default, both configmaps) | mTLS. Certs auto-provisioned + rotated by the Controller's internal CA. Gateway refuses to connect in plaintext under `auto_ca`. |
| File-mode selected `certs` paths | mTLS. Unchanged. |
| `skip_tls: true` | **Plaintext, still supported** — the only plaintext path. ERROR log on both ends + `edgion_system_error_state{reason="skip_tls"}`. A `skip_tls` that overrides `auto_ca` additionally raises a higher-severity WARN. |
| **Section omitted entirely** | **Refuses to start** (non-zero exit) on both the Controller and the Gateway. |

Since 2026-07-14 the **checked-in `config/edgion-{controller,gateway,controller-etcd}.yaml` carry
file-mode mTLS**, not `skip_tls` — a bare `cargo run` speaks mTLS once the operator bootstraps
the cert set. Certs are generated into `<work_dir>/certs/grpc/primary/` — integration scripts
generate them inside their own per-run work directory, and a bare `cargo run` requires an explicit
`edgion-cli gen-certs` into the repo root (no test script populates the repo root); nothing
injects or rewrites the transport block any more. `skip_tls: true` survives only in
`run_conf_sync_skip_tls_metric_test.sh` (its subject) and the conformance stack (deliberate, and
commented). A finding of the shape "the dev/test configs default to plaintext" is therefore
**stale** — verify against the current configs before re-filing.

Implementation:

- Shared guard: `ConfSyncSecurityConfig::ensure_transport_configured()`
  (`edgion-common/src/conf_sync/config.rs`). It is deliberately **not** part of
  `validate()`, which must stay lenient for consumers that merely parse the block.
  It also treats a bare `auto_ca: true` (no cert paths — the K8s default) as a
  *configured* transport, not the omitted state.
- Controller: `edgion-controller/src/cli/mod.rs` (startup, before any listener is bound)
  and `ConfigSyncGrpcServer::accept_skip_tls_or_refuse`
  (`edgion-controller/src/conf_sync/conf_server/grpc_server.rs`).
- Gateway: `edgion-gateway/src/cli/mod.rs` (`create_config_sync_client`) and
  `ConfigSyncClient::new` (`edgion-gateway/src/conf_sync/conf_client/grpc_client.rs`).

## Controller startup precedence (ARCH-080)

The Controller CLI prepares one private effective transport after installing metrics and
before constructing/spawning ConfMgr. Shared selection validation precedes all CA side
effects. A selected `skip_tls` overrides `auto_ca`, including its rotation schedule;
an inactive bypass never overrides the active set. Bare `auto_ca` still provisions mTLS.
Invalid selectors and invalid active paths fail closed rather than becoming plaintext.

The prepared mode controls rotation initialization and the identity builder used by
`start_services`. Plaintext creates no Kubernetes client, reads no certificate files,
publishes no identity and starts no certificate watchers. Effective auto-CA initializes
before leadership and publishes before per-replica identity synchronization. The existing
leadership callback remains unconditional: it records leadership even with no rotation
parameters. The CLI owns the override WARN; the serving guard retains ERROR, the existing
`skip_tls` metric and the non-loopback WARN. Federation is outside this override.

Regression coverage lives in `edgion-controller/tests/unit/cli/mod_test.rs`: canonical
`certs` fixtures exercise validation and the real initialization gate with a recording
callback, and invoke the production identity builder for bypass and missing file material.
These network-free tests do not claim live Kubernetes provisioning or transport coverage;
existing identity/revocation tests retain those narrower identity invariants. Re-evaluate
this boundary if startup wiring, selection priority, or CA lifecycle ownership changes.

## Retired: the `plaintext_default` metric reason

`edgion_system_error_state{reason="plaintext_default"}` meant "a secret-carrying channel
is *running* in plaintext because the config was omitted". That state is now unreachable,
so the reason constant and all its references were deleted. Alerts should key on
`reason="skip_tls"` (the explicit opt-in) instead. `no_tls_configured` remains
federation-only ("refused to start").

## Reviewing a new finding in this area

- "conf_sync ships Secrets over plaintext by default" → **already fixed**; the only
  plaintext path is an explicit operator opt-in. Not a defect.
- "Make `skip_tls` fail-close too" → **rejected**: it is the deliberate emergency escape
  hatch, is loud (ERROR + metric + WARN when it overrides `auto_ca`), and removing it
  leaves an operator with a broken cert chain no way to recover.
- Re-open only if conf_sync stops carrying resolved Secrets (the fail-close could then be
  relaxed), or if an in-pod loopback hop is introduced so the cross-network hop is
  separately securable.

## Reference

- Original observability spec (historical):
  `docs/superpowers/specs/2026-06-21-conf-sync-plaintext-default-observability-design.md`
- Cert rotation / auto-CA design:
  `docs/superpowers/specs/2026-07-12-conf-sync-cert-rotation-design.md`
- Industry baseline: Istio secure xDS (15012/mTLS) with plaintext as a separate,
  explicitly-selected port — the same "plaintext only when you ask for it" shape.

## Canonical certificate selection (ARCH-074)

`ConfSyncSecurityConfig` now carries only `certs` and `active` for file-mode
selection. The singular `tls` member and fallback are removed. An explicit selector
must match a named entry even when the list is empty; unknown old material cannot
satisfy it. A sole entry can still be selected without a name or selector. Duplicate
names and multiple entries without a selector remain invalid.

Legacy-only inputs no longer supply TLS material and ordinary file-mode transport
refuses them. Existing consumer-specific unknown-field handling is unchanged:
removal does not mean every old key becomes a deserialization error. Bare Controller
auto-CA, Gateway mounted-path requirements, selected pathless skip-TLS, and federation
mTLS refusal keep their existing owners and behavior. This migration does not change
the Controller auto-CA-first startup ordering or resolve the separately identified
auto-CA/skip-TLS precedence discrepancy in existing documentation.

Regression coverage lives in Common `conf_sync/config_test.rs`, both binary config
unit modules (real shipped/generated YAML through actual loaders), existing server
and federation refusal tests, Gateway client tests and `conf_sync_cert_expiry`.
These configuration checks do not prove remote handshakes or filesystem notification
delivery. Re-evaluate if a second material representation or selector bypass is
introduced. Fresh execution outcomes belong to the ARCH-074 implementation record;
historical results above are not fresh verification of this change.
