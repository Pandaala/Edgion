---
name: fed-proxy-request-sanitisation
description: Use when reviewing Controller federation HttpProxy findings about the Center-supplied method/path/headers/body being forwarded into the admin router; covers the defence-in-depth sanitisation applied inside the HttpProxy arm.
status: fixed
finding_id: controller-06
closed: 2026-06-14
knowledge_type: regression-guards
---

# Federation HttpProxy: Center-Supplied Request Is Sanitised Before the Admin Router — FIXED

## Conclusion

**FIXED (controller-06, 2026-06-14).** The Controller federation client's
`CenterPayload::HttpProxy` arm
(`edgion-controller/src/fed_sync/fed_client/mod.rs`, with execution in
`fed_client/proxy.rs`) sanitises the
Center-supplied request envelope before building the `http::Request` it feeds to
the admin router via `oneshot`. This is defence-in-depth: the mTLS channel +
injected `CenterRestricted` RBAC role already authenticate and authorize the peer
(see [`fed-proxy-mtls-fail-close`](fed-proxy-mtls-fail-close.md)), but the request
envelope is still attacker-controlled if the channel is ever compromised or
misconfigured. Four narrow checks now run inside the spawned task:

1. **Body cap** — `proxy_req.body.len() > MAX_PROXY_REQUEST_BODY` (1 MB) → 413,
   so a multi-GB body cannot be buffered before any handler runs (mirrors the
   existing 10 MB response cap with a tighter inbound limit).
2. **Method** — an unparseable `proxy_req.method` → 400 instead of silently
   falling back to `http::Method::GET` (a malformed payload must not become a
   side-effecting GET).
3. **Path** — `proxy_req.path` (query/fragment stripped) must start with one of
   `ALLOWED_PROXY_PATH_PREFIXES` (`/api/v1/`, `/health`,
   `/ready`, `/metrics`) → otherwise 404 before the router is touched.
4. **Headers** — only `ALLOWED_PROXY_HEADERS` (`accept`, `content-type`,
   `if-match`, `user-agent`, case-insensitive) are forwarded; everything else
   (`Authorization`, `Cookie`, `X-Forwarded-*`, hop-by-hop, ...) is dropped.

Fix suggestions of the form "centralise the allowlist as a tower middleware on
the cloned fed router", "forward an `x-edgion-federation-client-id` header so the
handler can identify the Center", or "drop the local checks because mTLS + RBAC
already cover this" are **not accepted** (see below).

The source-browsing migration removed `/configserver/` from federation reachability.
Local diagnostics keep their existing embedded-credential redaction; their
exclusion is distinct from the old unredacted generic Watch leak. `if-match`
preserves source CAS preconditions. Bounded proxy envelopes, response serialization
and execution deadlines additionally constrain the request task; these are not
permission to expose processed resource bodies.

### Processed resource boundary (ISSUE-009)

`ISSUE-009` from `important_issue/run-e6cbd068` identified an unredacted generic
federation Watch of processed EdgionTls resources, including resolved TLS private
keys. Excluding Secret from RBAC did not exclude credentials embedded in another
resource. The correction removes generic processed List/Watch from federation:
`fed_client/center_watch.rs` accepts only the exact synthetic `RegionRoute` view,
including under a wildcard policy, and reads only allowlisted lightweight
`CenterWatchStore` projections. Adding a future view requires separate projection
and authorization review; granting a generic kind must never expose ServerCache.

This is distinct from local `/configserver` redaction, which already existed.
Federation proxy rejection of `/configserver/` establishes the source-resource
boundary; it is not the fix for the historical unredacted Watch itself. Gateway
conf_sync still carries real credentials through its trusted channel.

Evidence includes `generic_kinds_are_denied_even_with_wildcard_policy_and_faulted_store`,
dedicated List/update/delete and revocation tests, strict projection fixtures,
`configserver_redacts_resolved_secrets`, and
`sync_cached_path_ships_real_secrets_and_stays_unpoisoned`. Cross-repository live
acceptance exercised directory deltas and the processed proxy denial; real Gateway
`CredentialInjectorDynamic` exercised initial credentials, rotation, deletion,
and recovery. Re-evaluate if a projection gains payload fields, a generic Watch
kind is accepted, or a processed diagnostic endpoint becomes federation-reachable.

## Core Rationale

**1. mTLS + RBAC authenticate the peer; they do not bound the envelope.**
`inject_center_identity` stamps the request with the `CenterRestricted` role from
the `Authorizer` state — it reads **no** request header, so the Center identity
never depends on a Center-supplied header. RBAC gates `(role, verb, kind)`, not
body size, header set, or path shape. A huge body still exhausts memory, a
forged `X-Forwarded-For` still spoofs plugin IP allowlists / audit trails, and a
silent-GET fallback still side-effects — all *inside* an authorized session.
These are exactly the gaps the four checks close.

**2. The checks live in the `HttpProxy` arm, not in a router middleware.**
A tower layer on the cloned fed router was considered and rejected as a narrow
fix: it widens the blast radius to the router-assembly path in
`cli/mod.rs` (which already carries `authz_layer` + `inject_center_identity` in a
documented LIFO order — "do NOT add `.route()`/`.merge()` after this point"). The
arm-local form is a single-file, cold-path change that cannot perturb the
authn/authz onion.

**3. The header allowlist is positive, not a denylist.**
Center forwards `accept` / `content-type` / `if-match` / `user-agent` for the
admin REST calls it drives. `content-length` / `host` are recomputed by the
in-process `oneshot` and must not be forwarded. A denylist of "bad" headers would
silently let future smuggling vectors through; the positive allowlist fails safe.

## Fix Suggestions Not Accepted

- **"Centralise as a tower middleware on the fed router."** Rejected — touches the
  router-assembly LIFO order in `cli/mod.rs`; the arm-local check is narrower and
  cannot disturb `inject_center_identity` / `authz_layer`.
- **"Forward an `x-edgion-federation-client-id` header for identity."** Rejected —
  identity comes from mTLS + `inject_center_identity` (which reads no header);
  adding a header-carried identity would be a protocol change and a new trust in
  Center-supplied input.
- **"mTLS + RBAC already cover this, the checks are redundant."** Rejected — RBAC
  authorizes verb/kind, not envelope shape; the body-DoS, header-smuggling, and
  silent-GET vectors all survive authorization. The checks are defence-in-depth,
  not a duplicate of authn.
- **"Make the body cap / header set operator-configurable."** Rejected as a narrow
  review fix — the federation proxy is an internal channel with no operator-tunable
  surface; promote to `tasks/todo/` via brainstorming if a real need appears.

## Re-evaluation Triggers

Re-open only if:

- An admin endpoint legitimately reachable over federation moves outside the four
  allowlisted path prefixes (extend `ALLOWED_PROXY_PATH_PREFIXES`).
- A federation admin call legitimately needs a request header outside
  `accept`/`content-type`/`if-match`/`user-agent` (extend `ALLOWED_PROXY_HEADERS`, never
  re-add `Authorization`/`Cookie`/`X-Forwarded-*`).
- The 1 MB inbound body cap proves too small for a legitimate admin payload
  (raise `MAX_PROXY_REQUEST_BODY`, keep it ≤ the 10 MB response cap).

## Reference Cases

- `controller-06` (P2, Security), fixed 2026-06-14: body cap + method-reject +
  path allowlist + header allowlist in the `CenterPayload::HttpProxy` arm of
  `fed_client/mod.rs`; unit tests `http_proxy_oversized_body_rejected_413`,
  `http_proxy_invalid_method_rejected_400`,
  `http_proxy_disallowed_path_rejected_404`.
- Sibling `controller-01` (mTLS fail-close + RBAC layer):
  [fed-proxy-mtls-fail-close.md](fed-proxy-mtls-fail-close.md). This finding is the
  envelope-sanitisation layer *inside* the session that controller-01 authenticates.
