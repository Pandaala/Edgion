---
name: basic-auth-no-bcrypt
description: Use when reviewing basic_auth plugin password-format findings; explains why bcrypt is deliberately unsupported and which formats ({SHA}, $apr1$, $1$) are accepted.
knowledge_type: intentional-design
---

# Basic Auth: bcrypt is not supported (by design)

The Edgion `basic_auth` gateway plugin **does not support bcrypt** (`$2a$` / `$2b$` / `$2y$`). This is a deliberate design choice, aligning with Envoy and Kong.

## Supported password formats

`htpasswd.rs::verify` recognises only:

- `{SHA}<base64>` — SHA-1
- `$apr1$salt$hash` — Apache APR-MD5 (htpasswd `-m`)
- `$1$salt$hash` — MD5-crypt

Any other prefix returns `false`. Plaintext passwords in Secrets are auto-hashed as `{SHA}<base64(sha1(password))>` on load.

## Why bcrypt was removed

1. **Bcrypt forces a result cache.** Cost-10 bcrypt verify is ~100 ms per request; without a cache, every request burns CPU and is trivially DoS-amplifiable. The cache then brings revocation lag and memory cost — the very issue this design choice avoids.
2. **Aligns with industry data-plane peers.** Envoy's `basic_auth` filter only supports `{SHA}` (htpasswd `-s`); Kong stores `SHA1(password || consumer_id)`. Neither caches verify results, neither supports bcrypt. Edgion now matches that layering.
3. **Heavy auth belongs off the data plane.** When operators need bcrypt-grade or stronger key-stretching, the project recommendation is to delegate to an external auth service via `forward_auth` or external authorization, not to embed it in the gateway hot path.

## Scope: gateway data-plane only

This finding is about the gateway `basic_auth` data-plane plugin and is unaffected by the controller auth refactor. The Controller's Admin API auth is now **token-only** (a pre-shared Bearer token, no password), so there is no controller-side password hashing — the former `local_auth` admin-password subsystem was removed. The only password-format discussion that remains is the gateway `basic_auth` htpasswd one above.

## Review-finding handling

- "Basic auth should support bcrypt for compliance" → **closed**, see this file. Use `forward_auth` / ext authz for compliance-grade key stretching.
- "Plaintext password handling on load is unsafe — should bcrypt them" → **closed**, the `{SHA}` upgrade matches the verify-side capability and avoids reintroducing the cache.
- "Auth result cache should be added back to amortize verify cost" → **closed**. SHA-1 / APR-MD5 / MD5-crypt verify cost is sub-millisecond; cache adds revocation lag with no measurable benefit.
- "Move htpasswd verify to `spawn_blocking`" → **closed**. Verify runs inline on the request worker (`BasicAuth::verify_user`), like other auth crypto (JWT, HMAC, JWE). Measured 2026-11-05 (release build, one core): `{SHA}` about 0.05 us for a short password (linear in length, comparable to the Authorization base64 decode); `$apr1$`/`$1$` grows linearly with the presented password (about 125 us at 16 bytes, 670 us at 256 bytes, 152 ms at 64 KiB), so `htpasswd::MAX_MD5_CRYPT_PASSWORD_BYTES` (256, the longest password Apache `htpasswd` can hash; `-b`/`-i` accept 255) rejects longer passwords before hashing. With the default work-stealing runtime, peers pick up queued tasks; with `work_stealing: false` (one current-thread runtime per worker) the cost is per-thread head-of-line delay. The worst case is roughly an order of magnitude above an inline JWT RS*/ES* verify, accepted because it is bounded by the cap and reachable only with a known username (`users.get` runs before hashing). A blocking-pool hop does not reduce CPU cost; it adds a thread handoff and admits up to Tokio's 512 blocking threads per runtime (Pingora builds one runtime per listener service, and one per worker thread with `work_stealing: false`), each with an unbounded queue and no admission permit. The former `spawn_blocking` was a leftover from the bcrypt era (introduced in the initial commit for bcrypt, kept when bcrypt was removed) and was removed. Reconsider only if a supported format costs milliseconds per verify — which bcrypt-class hashes would, and is why they stay unsupported.
- "`is_pre_hashed` still accepts `$5$` / `$6$` prefixes but `verify` rejects them" → known acceptable degradation: such an entry simply fails authentication at runtime rather than being re-hashed; it is not a security defect.

## Anchors

- `edgion-gateway/src/plugins/http/basic_auth/htpasswd.rs` — the `verify()` switch and module-level docstring
- `edgion-gateway/src/plugins/http/basic_auth/plugin.rs::load_users` — plaintext → `{SHA}` upgrade path
- `edgion-gateway/src/plugins/http/basic_auth/plugin.rs::verify_user` — inline verify, no blocking-pool hop
- `edgion-gateway/src/plugins/http/basic_auth/htpasswd.rs::MAX_MD5_CRYPT_PASSWORD_BYTES` — MD5-crypt presented-password length cap
- `docs/{en,zh-CN}/user-guide/http-route/filters/edgion-plugins/basic-auth.md` — operator-facing description
