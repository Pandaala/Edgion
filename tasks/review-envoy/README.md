# Envoy-Comparison Design Review — Edgion

A design review comparing Edgion's architecture against Envoy's (and Istio's / Gateway API's)
proven, production-hardened designs. Each finding is a place where the Envoy/Istio design is
well-established and good, but Edgion's current design has a concrete, triggerable defect.

Findings went through **three passes**: (1) generation (one Sonnet subagent per module),
(2) adversarial verification (a skeptic re-checked each claim against source), (3) an
independent strict second-opinion pass that deleted anything that was a pure feature-parity
gap, a latent issue with no current trigger, an accepted design tradeoff, or factually wrong.
Everything here is grounded in actual source code (`file:line`) with a named real-world trigger.
One problem per file.

Format: frontmatter (`module`, `title`, `severity`, `envoy_reference`) + Envoy baseline /
Edgion's design / the gap / impact / suggested direction + `## Verification note`.

## Scope (8 modules)

| Prefix | Module | Baseline |
|--------|--------|----------|
| `sync-` | Controller↔Gateway config sync | Envoy xDS / ADS |
| `resource-` | Resource & config model, schema, validation | xDS resource model |
| `controller-` | Controller translation, status, leader election | Istio Pilot / Gateway API status |
| `filter-` | Plugin / filter model | Envoy HTTP filter chain |
| `route-` | Routing, retries, timeouts, traffic split | Envoy virtual hosts / routes |
| `lb-` | Load balancing, health, outlier, circuit breaking | Envoy upstream resilience |
| `listener-` | Listeners, sockets, upstream conn pooling | Envoy listeners / clusters |
| `tls-` | TLS / mTLS / certificate & secret mgmt | Envoy TLS transport socket + SDS |

## Verification funnel

`50` generated → `48` after pass-2 → **`36` after pass-3 (strict)**. 14 deleted total.

**Deleted in pass-3 (12)** — confirmed not real problems under the strict bar:

| File | Why deleted |
|------|-------------|
| `resource-06` (proto enum stability) | Wire uses `string kind`; no `ResourceKind` proto enum field exists. No current trigger. |
| `controller-02` (no K8s Events) | Pure observability parity; status conditions already surface errors. |
| `controller-03` (on_change before cache write) | Latent; no current handler reads ServerCache in on_change. |
| `controller-08` (preparse side-effects on Skip) | No handler returns `Skip` after preparse side-effects; no functional impact. |
| `filter-06` (stream plugin context gap) | Parity wish; mTLS identity (the real L4→L7 need) is already bridged via `PluginSession.client_cert_info`. |
| `route-03` (regex beats prefix) | **Behavior is Gateway-API-spec-compliant** (Exact > Prefix > Regex by type). Only a SKILL.md doc inversion remains — a docs bug, not a routing defect. |
| `route-05` (no route recompute after header mutation) | No shipped plugin causes wrong-backend; routing-changing plugins correctly use `ctx.jump`. |
| `lb-04` (no success-rate outlier detection) | P2C (EWMA/LeastConn) already deprioritizes degraded backends; no distinct failure mode. |
| `lb-05` (no concurrency circuit breaker) | Accepted gateway design tradeoff (nginx/HAProxy/Traefik don't either); Pingora pool + fd limits bound concurrency. |
| `listener-02` (global pool not per-protocol) | "H1 evicts H2" was false (separate v1/v2 connectors); residual is a config no-op + duplicate of lb-05. |
| `tls-01` (no cert-expiry metric) | Observability parity; the availability consequence is already covered by tls-06. |
| `tls-02` (non-atomic store/matcher swap) | Latent; no data-plane path cross-reads both stores. Off the hot path. |

**Deleted in pass-2 (2)**: `resource-03` (duplicate of sync-05), `route-06` (factually wrong — uses `ArcSwapOption`, rebuilt every update).

## Critical (1)

- `tls-06-invalid-cert-update-drops-listener.md` — **re-verified with a full path trace.** The
  Controller's `EdgionTlsHandler::validate()` only checks parent-refs, secret existence, and
  key↔cert match — it does NOT run `validate_cert()` (expiry/SAN/PEM). A malformed cert (classic
  cert-manager mid-renewal Secret race) is synced to the Gateway, where `rebuild_matcher_from_data`
  rebuilds the matcher from scratch and silently drops the host with **no last-known-good carry-forward**.
  New handshakes for that host get `cert not found` → hostname blackholed until a valid cert arrives.

## High (19)

- **sync**: `01` no NACK/error feedback; `02` cross-kind ordering (route 502s before backend arrives); `03` slow gateway stalls controller watcher; `05` cold-start blackout (no local snapshot).
- **resource**: `01` unknown plugin type fails the whole list batch; `02` cross-kind dangling plugin ref → 500s.
- **controller**: `01` status writes dropped on transient 429/503; `06` deleting `EdgionGatewayConfig` silently blackholes the Gateway while status says `Accepted=True`; `07` typo in `parametersRef` → same silent blackhole, no `ResolvedRefs`.
- **filter**: `01` response body immutable (no WAF/transform); `02` no async pause → `block_in_place` parks worker threads (already in shipped DslPlugin); `04` `ErrResponse` body never reaches client in response phase; `05` `validateRequestBody:true` makes HmacAuth return 500 for the whole route (**upgraded to high**).
- **route**: `01` POST re-sent on stale keepalive (no method guard); `02` no per-try timeout.
- **lb**: `01` no panic threshold → all-unhealthy = 100% failure; `03` outlier detection HTTP-only (TCP/UDP/TLS never eject).
- **listener**: `01` ports frozen at startup (silent unbound + misleading log); `03` no downstream conn limit → fd exhaustion.
- **tls**: `03` OCSP/CRL are no-ops → revoked client cert passes mTLS.

## Medium (11) / Low (5)

Medium: route-04 (mirror bypasses BackendTLSPolicy/LB + pool churn), sync-04 (reload polling), resource-04/05 (untyped errors / forward-compat), controller-04 (cross-kind TLS-rotation window), controller-05 (no worker drain on leader loss — **downgraded**), listener-04 (no max-conn-duration → stale post-scale-out), lb-02 (fixed ejection, no backoff — **downgraded**), filter-03 (no per-route plugin config variant), tls-05 (no `h2` ALPN to gRPC-over-TLS backends).
Low: sync-06, lb-06, listener-05/06, tls-04.

## Totals

**36 findings — 1 critical, 19 high, 11 medium, 5 low.**
Per module: sync 6 · resource 4 · controller 5 · filter 5 · route 3 · lb 4 · listener 5 · tls 4.

> Multi-agent review: per-module Sonnet generation → adversarial verification → independent strict
> second opinion. No source code was modified. Note: `route-03`'s deletion leaves a real **docs bug**
> in `skills/01-architecture/02-gateway/03-routes/SKILL.md` (claims "regex runs before radix" — the
> opposite of the code); worth fixing separately.
