---
name: multi-source-key-composition-injective
description: Use when reviewing rate-limit key composition findings; covers why multi-source bucket keys are length-prefixed and slot-positional instead of a plain "_" join.
status: fixed
finding_id: gw-plugin-acl-rl-04
closed: 2026-08-03
---

# Multi-source rate-limit key composition must be injective — FIXED

## Conclusion

`resolve_rate_limit_key` (`edgion-gateway/src/plugins/http/common/mod.rs`, shared by
`RateLimit` and `RateLimitRedis`) used to `parts.join("_")` over the values that resolved,
silently dropping the sources that did not. Both properties broke injectivity of the bucket
key, so two different value tuples could land in one bucket. The composition now runs through
`compose_rate_limit_key`: a single configured source is used verbatim, and multiple sources are
encoded as one slot per configured `KeyGet` in configuration order — `<byte-len>:<value>` for a
resolved source, `-` for an unresolved one — joined with `_`.

Re-reports of the "`_` separator is unescaped / values containing `_` collide" or "a skipped
missing key shifts the remaining parts" forms are **not accepted**; verify `compose_rate_limit_key`
first.

## Core Rationale

**1. The composed string IS the bucket identity, end to end**

Neither consumer re-splits or re-hashes it: `RateLimit` passes it to `Rate::observe(&limit_key, 1)`
(CMS counter), and `RateLimitRedis` passes it to `build_redis_key(prefix, algo, policy_idx, limit_key, suffix)`,
which interpolates it as one opaque segment. Any ambiguity introduced at composition time
propagates unchanged to the actual quota counter.

The one exception bounds the guarantee: `RateLimitRedis` runs `truncate_limit_key(limit_key, max_key_len)`
before building the Redis key, and prefix truncation is not injective for any encoding. Injectivity
therefore holds for composed keys within `maxKeyLen` (default 4096, so unreachable in practice; the
configurable minimum of 64 is reachable). This is not a regression — truncation was already
non-injective, and before this fix the key was ambiguous even without it.

**2. At least one source is normally client-controlled**

`KeyGet::Header` / `Cookie` / `Query` read client-supplied values (an accepted, documented
tradeoff — see [ip-restriction/ip-source-client-ip-false-positive.md](../ip-restriction/ip-source-client-ip-false-positive.md)),
and `KeyGet::Ctx` carries values written by earlier plugins (including DSL `ctx.set`), so no
byte value can be assumed absent. With `key: [Header("X-Tenant"), Header("X-User")]`, the tuples
`("t1", "u_2")` and `("t1_u", "2")` both rendered to `t1_u_2`. A client that can guess a target's
part boundary could therefore merge its own traffic into that target's bucket and exhaust its
quota — the precise isolation the multi-source key feature exists to provide.

**3. The skipped-slot variant needed no separator at all**

The old loop pushed a part only on `Some`, so with the same two-header configuration
`(None, "x")` and `("x", None)` both rendered to `x`. This second collision class was not in the
original finding; it is fixed by the positional `-` marker. The key-miss fallback still triggers
only when a configured source failed to resolve — see rationale 5 for the empty-`keys` carve-out
that the same predicate now expresses.

**4. Length prefixing is the cheapest injective encoding**

Reading `<byte-len>` fixes exactly how many bytes the part occupies, so uniqueness holds for any
content — no escaping loop and no per-part hashing. Single-source configurations (the common case
in the docs' own examples) stay verbatim, which keeps Redis keys human-readable for the
overwhelming majority of deployments and makes the change a no-op for them.

**5. Two sibling defects of the same class were fixed in the same change**

Both were found while reviewing the composition fix and share its root question — "which bucket does
this request belong to?":

- **Empty `key` list was fail-open.** `#[serde(default)]` on `key` yielded `Vec::default()` (the
  `Default` impl's `vec![RemoteIp]` never runs on deserialization), so a config that omits `key`
  resolved no key, fell through to `onMissingKey` and — with its `Allow` default — silently
  disabled the limiter. `key` now uses `#[serde(default = "default_key_sources")]` on both
  `RateLimitConfig` and `RateLimitRedisConfig` so an omitted list matches the documented
  `[{type: remoteIp}]`, and `resolve_rate_limit_key` treats an explicitly empty list as "one global
  bucket" (`keys.is_empty()` composes to `""`) rather than as a key miss. Nothing configured cannot
  be "missing", so `onMissingKey` / `defaultKey` no longer apply to it.
- **Cross-plugin bucket namespace.** Every `RateLimitRedis` instance defaults to the same
  `keyPrefix` (`edgion:rl:`) and `policy_idx`, so bucket identity came down to the composed key
  alone. Two unrelated EdgionPlugins resources therefore indexed the same buckets — merging quotas
  outright when their key sources matched (two default `key: [remoteIp]` plugins share one set of
  per-IP buckets cluster-wide), and letting a client that controls one plugin's source craft a value
  matching another plugin's composed key when they did not. The Redis key now carries the owning
  entry's identity, `<namespace>/<name>/<alias or #index>`, built by `plugins::common::entry_scope`
  and folded into `RateLimitRedis`'s `scoped_key_prefix` at build time.

  Identity, not a config digest, is the right discriminator: the compiled runtime is built once per
  EdgionPlugins resource and Arc-shared by every route referencing it, so `ns/name` already *is* the
  sharing unit — it is exactly what the in-memory `RateLimit` gets for free from the plugin store
  being keyed by `ns/name`, and what the `rate-limit-shared` integration scenarios assert. A digest
  of the key sources was implemented first and rejected on review: it separates plugins that declare
  *different* sources but still merges the far more common case of two unrelated resources left on
  the default configuration. `RateLimit` needs no equivalent (each instance owns its `Rate`). The stream
  `ConnectionRateLimit` turned out to have the same defect and was fixed the same way — see
  [connection-rate-limit-entry-scoped-buckets.md](connection-rate-limit-entry-scoped-buckets.md).

## Fix Suggestions Not Accepted

- Join with a control character (`'\u{1}'` / `'\0'`) — not airtight: `KeyGet::Ctx` values come from
  other plugins and are not constrained to printable bytes, and it puts invisible bytes into Redis
  keys and monitoring output.
- Hash each part before joining — destroys the operational readability of the Redis key and the
  `kl=` access-log signal for no additional safety over length prefixing.
- Escape `_` inside each part only — fixes the separator collision but leaves the skipped-slot
  collision (rationale 3) intact.
- Length-prefix the single-source case too — pure churn for the common configuration; a lone slot
  is already unambiguous within its own key configuration, and the entry-identity segment
  (rationale 5) is what separates it from other plugins' keys.
- Give each plugin instance a random or process-local bucket namespace — it would break the
  cluster-wide sharing `RateLimitRedis` exists to provide; the namespace must be derived from the
  synced resource so every Gateway instance computes the same value.
- Namespace the Redis key by a digest of the `key` source list instead of by resource identity —
  implemented and then replaced: it leaves two unrelated resources on the default
  `key: [remoteIp]` sharing one bucket set, which is the more likely accident.
- Use the resource key without the entry discriminator — one resource may declare several
  `RateLimitRedis` entries; `alias` (validated `^[a-zA-Z0-9._-]{1,32}$`) or `#<index>` separates
  them, and `#` cannot appear in an alias so the two forms never collide. Alias uniqueness within a
  stage array is enforced in both validation layers (`duplicate_alias_error`), and the gateway
  additionally *drops* the duplicate — reporting alone would not protect the data plane, because
  lenient admission keeps an `Accepted=False` resource synced and serving, so the two entries would
  still share one scope. Dropping falls the entry back to its `#index` form. The same alias in two
  different stage arrays stays legal (the accumulator is per stage array), which is sound only while
  the request stage is the sole consumer of `entry_scope`.
- Re-derive `<namespace>/<name>` inside the runtime instead of taking the store key — a missing
  namespace normalizes to `default` there, which would alias two distinct store entries.
- Reject an empty `key` list at validation instead of defining it — an empty list is a meaningful
  "one global bucket" policy, and rejecting it would not have covered the *omitted*-field path,
  which was the actual fail-open.

## Re-evaluation Triggers

Re-open this decision only if:

- A new `KeyGet` variant is added whose value is composed from multiple sub-values internally with
  a recoverable boundary. `RemoteIpAndPath` is the existing case and is not affected: it joins
  `<ip>:<path>` and every reachable path either starts with `/` or is exactly `*`
  (asterisk-form `OPTIONS`), neither of which an IP literal can contain or be a prefix collision
  with, so the boundary survives even though the path is client-controlled.
- A consumer starts parsing the composed key back into its parts (today nobody does), which would
  make the encoding a decode contract rather than an identity.

## Reference Cases

- Finding `gw-plugin-acl-rl-04` (2026-08-03), `tasks/fable-review/_candidates/G14/`.
- Source: `edgion-gateway/src/plugins/http/common/mod.rs` (`compose_rate_limit_key` + its four
  regression tests).
- Sibling shared-helper decision: [key-extraction-shared-helper.md](key-extraction-shared-helper.md)
  (plugins-http-F003) — the reason this composition lives in exactly one place.
- User docs updated in the same change: `docs/{en,zh-CN}/user-guide/edgion-plugins/rate-limit.md`,
  `docs/{en,zh-CN}/user-guide/http-route/filters/edgion-plugins/rate-limit-redis.md`.
