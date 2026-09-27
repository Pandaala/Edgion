# Prepare HTTPDNS clients before request-time resolution

## Status and evidence

Design decision required; found during Common quality batch B0083.
`edgion-common/src/dns/mod.rs::resolve` constructs `ReqwestHttpDnsFetcher` for
every HTTPDNS resolution. `http_fetcher.rs::new` invokes `build_outbound_client`
and the shared TLS resolver. Gateway DNS cache misses can invoke this chain
from a request callback. `outbound_tls.rs::resolve_outbound_tls` emits WARN for
skip-verification or a SAN override of that setting. The current implementation
therefore repeats client/TLS construction and can log on the Gateway hot path.
The existing TLS-loading review correctly states HTTPDNS has no prepared runtime
client; that statement is not permission for request-path tracing.

## Required decision

Choose ownership for a prepared immutable resolver generation: attach a prepared
resolver to the existing effective DNS snapshot, or introduce an explicit shared
prepared-resolver handle consumed by Controller and Gateway. Prefer attaching
owned handles to existing generations to avoid another global cache/store.
Do not merely suppress audit logs without moving the audit to configuration time.
Determine how global TLS defaults and Secret rotation invalidate prepared clients.

## Constraints and acceptance

Preserve ResolverSpec precedence, fallback, secret-redacted fingerprint semantics,
64 KiB HTTP body and 256-IP limits, timeout/redirect/proxy policy, and binary-owned
observability. No Gateway API route matching or conformance contract changes.
Verify create/update/delete, global-policy change, Secret rotation, empty full sync,
failed preparation with configured fallback, and cancellation. Tests must prove
no request-path tracing or TLS/client rebuilding; dynamic tests use the existing
shared topology and wait for observable generations. Preserve resource ownership
and cleanup; no compatibility shim is needed in this pre-release repository.
