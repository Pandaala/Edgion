---
name: redis-client-lookup-borrows-link-key
description: Use when reviewing DSL Redis alias lookup allocation or client-generation lifetime.
knowledge_type: regression-guards
---

# DSL Redis client lookup borrows the LinkSys key

## Conclusion and scope

The fourteen non-rate-limit `redis.*` builtins, including `get_bounded`, resolve
their live client through `resolve_redis_client`. A successful alias lookup borrows the stored
`namespace/name` string from `DslRuntime.redis_links` for the synchronous
LinkSys store lookup and performs no deep key clone. Only the returned
`Arc<RedisLinkClient>` survives into asynchronous command execution.

`redis.rate_limit` uses the same borrowed-key shape directly. Every Redis
builtin still resolves the current client on each call; no plugin generation
captures or pins a provider client.

The caller-owned alias is distinct from the stored `namespace/name` key. After
successful resolution, each non-rate-limit caller explicitly drops its temporary
alias String before awaiting the command. The rate-limit caller similarly drops
its alias and policy-name Arcs after lookup; the selected policy belongs to the
immutable runtime, and the business key remains available for evaluation. This
shortens input ownership without changing per-call resolution or eliminating the
original string conversion. Dropping an Arc does not reclaim a shared payload
until its last owner is gone.

## Rationale and evidence

Canonical authority is `skills/00-design-rules.md` sections D, U, and Z and
`skills/03-coding/dsl/redis-builtin.md`. Redis access must use the canonical
LinkSys client store, request-path transient allocation should be avoided, and
each DSL Redis call must re-resolve its client for hot reload.

`DslPlugin::resolve_parts` builds the immutable alias-to-key map once per plugin
generation. `resolve_redis_client_with` in
`edgion-gateway/src/plugins/http/dsl/lang/builtins.rs` borrows the mapped key,
passes it to a synchronous lookup, and returns the lookup result. The production
wrapper supplies `get_redis_client`, whose ArcSwap-backed store clones only the
selected client `Arc`.

The original implementation called `String::clone` after every successful alias
lookup in the thirteen basic operations. That allocated and copied the complete
key even though the copy was dropped immediately after the synchronous store
lookup. `redis.rate_limit` already demonstrated that this ownership was
unnecessary.

## Alternatives and limits

Changing `DslRuntime.redis_links` to another owned key representation is not
needed: the existing `HashMap<String, String>` owns stable keys for the plugin
generation. Retaining a borrowed key across a Redis command await would couple
the future to the plugin snapshot unnecessarily; the client `Arc`, not the key,
is the generation-safe asynchronous handle.

The allocation assertion covers the successful alias-to-store lookup boundary,
not allocations made by Redis commands, error-map construction, ArcSwap internals
under unrelated operations, or the DSL VM as a whole. Missing-alias and
provider-not-ready result semantics are unchanged and may allocate their error
strings.

## Verification and re-evaluation

Fresh focused verification after the correction:

- `cargo test -p edgion-gateway --lib warm_alias_lookup_borrows_the_stored_link_key_without_allocating --locked`:
  1 passed. The test confirms that the lookup receives the original stored string
  buffer and records zero allocation calls and zero requested bytes.
- `cargo test -p edgion-gateway --lib plugins::http::dsl::tests::redis_builtins --locked`:
  5 passed, covering the existing missing-alias, provider-not-ready, result-budget,
  and rate-limit error contracts.
- Independent read-only review: LGTM. It rechecked the client lifetime, ArcSwap
  generation behavior, error paths, allocation assertion, and knowledge routing.
- `cargo fmt --all`, workspace all-target check and Clippy, agent-doc consistency,
  SSA force, metrics inventory, and Pingora source policy passed. Clippy completed
  successfully with pre-existing warnings; none referred to the changed code.

Re-evaluate if the alias map representation, LinkSys store lookup API, Redis
client publication model, or DSL builtin async boundary changes. Full repository
tests were not requested and were not run.

## Resolution references

- `avoid-dsl-redis-link-key-clones` — confirmed hot-path allocation in
  `resolve_redis_client`; corrected by borrowing the existing LinkSys key while
  preserving per-call live-client resolution.
