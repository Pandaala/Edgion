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

## What `local_auth` is — and isn't

`src/core/common/local_auth/` (Controller admin auth, management plane) **still uses bcrypt** for the admin password. That is correct and unrelated: it computes the hash once at startup and verifies on rare admin-login requests, not on every data-plane request. The `bcrypt = "0.18"` Cargo dep is retained for `local_auth` only.

## Review-finding handling

- "Basic auth should support bcrypt for compliance" → **closed**, see this file. Use `forward_auth` / ext authz for compliance-grade key stretching.
- "Plaintext password handling on load is unsafe — should bcrypt them" → **closed**, the `{SHA}` upgrade matches the verify-side capability and avoids reintroducing the cache.
- "Auth result cache should be added back to amortize verify cost" → **closed**. SHA-1 / APR-MD5 / MD5-crypt verify cost is sub-millisecond; cache adds revocation lag with no measurable benefit.
- "`is_pre_hashed` still accepts `$5$` / `$6$` prefixes but `verify` rejects them" → known acceptable degradation: such an entry simply fails authentication at runtime rather than being re-hashed; it is not a security defect.

## Anchors

- `src/core/gateway/plugins/http/basic_auth/htpasswd.rs` — the `verify()` switch and module-level docstring
- `src/core/gateway/plugins/http/basic_auth/plugin.rs::load_users` — plaintext → `{SHA}` upgrade path
- `docs/{en,zh-CN}/user-guide/http-route/filters/edgion-plugins/basic-auth.md` — operator-facing description
