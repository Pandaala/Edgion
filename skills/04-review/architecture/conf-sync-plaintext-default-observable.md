---
name: conf-sync-plaintext-default-observable
description: Use when reviewing conf_sync (Controller→Gateway) plaintext / secret-in-transit findings; covers why omitted conf_sync_security is observable-but-not-fail-closed, why it differs from fed_sync, and that this is an interim posture pending cert bootstrap (#13).
---

# conf_sync Plaintext Default — Observable, Not Fail-Closed (interim)

## Status: risk reduced (interim), NOT closed

The conf_sync channel carries resolved Secrets (TLS keys via `TlsCerts::secret`,
backend creds via `SecretSlot.secret`). When `conf_sync_security` is omitted it
runs plaintext. As of 2026-06-21 this is **not eliminated** for deployments that
stay plaintext — it is made loud and the shipped defaults are flipped to mTLS:

1. The previously-silent omitted path now logs ERROR on both ends and raises
   `edgion_system_error_state{component=conf_sync_client|conf_sync_server,
   reason="plaintext_default"}` (gateway emits the client series, controller the
   server series).
2. The shipped K8s manifests (`cicd/deploy/kubernetes/`) and the default
   integration stack now default to active mTLS (cert Secret `edgion-conf-sync-certs`
   is a documented prerequisite).

## Update (2026-06-21+): partial fail-close under `auto_ca`

Auto-CA provisioning v0 (Phase 1) has since landed: the Controller's internal CA
auto-provisions the conf_sync certs when `conf_sync_security.auto_ca: true`. For that
path the re-evaluation trigger below ("cert bootstrap lands") has fired, and the
**data plane now fails closed under `auto_ca`** (see
`edgion-gateway/src/conf_sync/conf_client/grpc_client.rs` — the `auto_ca` + omitted-TLS
fail-close branch — and
`docs/superpowers/specs/2026-07-12-conf-sync-cert-rotation-design.md` for the hot-reload
half of this behavior):

- `auto_ca: true` + omitted `tls`/`certs` → gateway fails fast at startup (misconfig;
  no plaintext, no retry).
- `auto_ca: true` + certs configured but Secret not yet provisioned → gateway retries
  client init within the startup-probe budget; it never downgrades to plaintext.
- `skip_tls: true` stays the only explicit plaintext escape hatch (overrides `auto_ca`
  with a high-visibility WARN).

The **legacy omitted path (no `auto_ca`) is unchanged** — still observable-but-not-fail-closed
(`plaintext_default`), for backward compatibility. The global fail-close flip (reject
plaintext even without `auto_ca`) remains deferred as a fast-follow with a migration note
(`tasks/pending/14-mesh-identity-plane-scale-taxes.md` §2.5).

## Why not fail-close / not secure-by-default in code

conf_sync is **data-plane critical** (the Gateway cannot serve traffic without it). Auto
cert bootstrap now exists — the Controller's internal CA provisions conf_sync certs
(`conf_sync_security.auto_ca: true`, Phase 1) and rotates them on a policy
(Phase 2; see `docs/superpowers/specs/2026-07-12-conf-sync-cert-rotation-design.md`) — and
under `auto_ca` the data plane already fails closed, per the Update above. The remaining
gap is narrower than "no bootstrap": the **legacy omitted path (no `auto_ca` configured at
all)** still runs plaintext-by-default rather than defaulting `auto_ca` on, for backward
compatibility with existing deployments that have not opted in yet. This is the deliberate
opposite of fed_sync, which **does** fail-close because it is auxiliary, not
data-plane critical (see [fed-proxy-mtls-fail-close.md](fed-proxy-mtls-fail-close.md)).

## Fix suggestions NOT accepted (here)

- "Add a fail-close / `allow_plaintext_conf_sync` opt-in" — a non-fail-close flag is
  decorative (plaintext runs either way); a meaningful one collapses into fail-close,
  rejected above for data-plane safety + missing cert bootstrap.
- "Reuse `no_tls_configured` for the metric" — that reason means *refused to start*
  (federation fail-close); conf_sync is *running degraded*, hence a distinct
  `plaintext_default`.

## Re-evaluation triggers (DO re-open when any holds)

- Cert bootstrap (#13) lands → revisit code-level secure-by-default / fail-close.
- conf_sync stops carrying resolved Secrets.
- An in-pod loopback hop is introduced (Istio-agent-style) so the cross-network hop
  is separately securable.

## Reference

- Spec: `docs/superpowers/specs/2026-06-21-conf-sync-plaintext-default-observability-design.md`
- Industry baseline: Istio secure xDS (15012/mTLS) default + plaintext as a separate
  discouraged port; Envoy SDS requires a secured channel. Neither fail-closes the data
  plane, but both achieve secure-by-default via auto-provisioned certs (Edgion lacks
  this until #13).
