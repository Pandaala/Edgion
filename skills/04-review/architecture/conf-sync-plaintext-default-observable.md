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

## Why not fail-close / not secure-by-default in code

conf_sync is **data-plane critical** (the Gateway cannot serve traffic without it)
and Edgion has **no auto cert bootstrap yet** (`tasks/pending/13-conf-sync-spiffe-cert-bootstrap.md`).
Fail-close on omission would brick fresh deploys lacking certs; code-level
secure-by-default needs certs that do not exist without #13. This is the deliberate
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
