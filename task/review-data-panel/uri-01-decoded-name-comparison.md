# Evaluate canonical decoded query-name comparison

Status: deferred performance decision for the user. Baseline: `a35a4490b`.
No runtime change or selected replacement.

## Evidence

`backends/selector.rs::extract_hash_key` borrows the selected raw query value.
Its Arg branch first compares the raw name; only a nonmatching name containing
`%` or `+` calls `edgion_resources::uri_utils::url_decode` to build an owned
String solely for equality with the configured name. The returned decoded
String is discarded immediately. Plain names already avoid decoding storage.

`plugins/http/common/auth_common.rs::strip_query_param` also materializes encoded
names only to compare them. Its policy differs: encoded keys are compared after
decoding, without the load balancer's raw-equality exemption. It collects raw
surviving pairs separately for mutation. This task concerns decoded-name storage,
not that survivor vector or final rewritten query.

The canonical decoder's plain-input shortcut does not remove encoded-name
allocation. A comparison-only interface might avoid complete output storage,
but its CPU, complexity and maintenance costs are unmeasured. Compare retaining
the decoder with a canonical comparison operation or explicitly scoped reusable
scratch. Scratch amortizes allocation while potentially retaining peak capacity;
it is not inherently allocation-free. No shared mutable buffer, pool, second
decoder or cache is approved here.

## Canonical decoding gate

Read the [canonical query token contract](../../skills/04-review/uri-normalization/mandatory-design/normalization-contract.md#canonical-query-percent-token-contract).
The decoder consumes up to two Unicode scalars after `%`, checks token byte
length and uses radix parsing, including accepted `%+A` and `%+0` tokens.
Malformed tokens retain consumed scalars literally; plus outside a token becomes
space. Successful adjacent tokens form one byte run. If its UTF-8 is invalid,
the entire run becomes bytewise Latin-1 characters, including a valid prefix.

A streaming comparator therefore cannot emit a decoded UTF-8 prefix before
establishing the run's validity. Consider validated replay or scratch only within
the canonical owner, preserving full output equivalence. Replacing this behavior
with a standard form decoder, lossy UTF-8 replacement or bytewise token comparison
is a semantic change. Do not introduce a plugin-specific decoding algorithm or
alter the separately bounded DSL/strict transcoder parsers.

## Consumer gates

- LB keeps raw-name equality OR decoded-name equality, first matching pair
  containing `=`, and borrowed raw values. A first matching empty value still
  terminates lookup and yields the existing empty-key fallback; do not continue
  to a later duplicate. Bare pairs remain ignored. Read the
  [LB name contract](../../skills/04-review/backends/regression-guards/consistent-hash-arg-percent-encoded-name.md).
- Auth removes every decoded-equivalent key, including bare pairs, while
  preserving retained pair bytes and order. Encoded raw equality is not a match
  exemption. Empty pairs disappear only when an actual removal triggers a
  rewrite; otherwise the setter is not called. Preserve ignored setter errors.
- Both consumers read their current query view. The frozen decoded query cache
  uses last-decoded-key wins and owned decoded values, so substituting it would
  change ordering, raw bytes and rewrite visibility. Keep the
  [auth no-match contract](../../skills/04-review/key-auth/regression-guards/strip-query-param-no-match-preserves-query.md).
- Preserve existing API ownership and immutable configuration publication.
  Add no request tracing, metrics, broad decoding cache or new dependency.

## Acceptance and verification limits

Before implementation, compare encoded-name frequency, lengths, hit/miss
positions, duplicates and long UTF-8 runs against current decoding. Measure CPU,
allocation calls/requested bytes and retained scratch memory; include plain names
already served by allocation-free comparison. Keep current code if savings do
not justify a more complex canonical interface.

Differentially compare proposed equality with `url_decode(input) == expected`
across valid/malformed tokens, leading-plus radix forms, empty strings, plain
UTF-8, valid-prefix/invalid-tail runs and mismatches at every output boundary.
Separately test each consumer's raw-equality, duplicate, empty-value, bare-pair
and live-query behavior; helper parity does not prove the caller policy.
Require an exact implementation scope, independent IMPL reviews and project gates.

Fresh baseline selections passed 14 resource URI tests, 44 Gateway query tests
and one ordinary-selection borrowed-input test (59 executions). The last covers
SourceIp ownership, not encoded Arg parity. No candidate implementation, allocation
measurement, benchmark, full suite or live traffic test ran.
