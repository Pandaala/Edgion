# Incremental HMAC signing input

Status: deferred allocation/CPU decision for the user.
Source baseline: `145bab36b`.

## Evidence

In `edgion-gateway/src/plugins/http/hmac_auth/plugin.rs`,
`HmacVerifier::verify_headers_with` calls `HmacAuth::build_signing_string`
after algorithm, credential, signed-date, timestamp and required-header checks.
The helper builds one owned String containing every selected header name/value
and request-target component. Its initial capacity estimate includes names and
separators, but not values, method, path or query. The completed buffer is passed
to `verify_signature` and discarded. This is source-visible temporary storage;
the allocation count, requested bytes and request CPU have not been profiled.

There is one production signing-string caller. The canonicalizer borrows current
raw query and header values but copies them into the aggregate String. It returns
`Missing signed header` on the first unavailable ordinary header in signing order,
before any MAC initialization or message hashing occurs. Already appended data
is discarded on that failure. It does not expose the missing header's name.

The locked `hmac` 0.12.1 uses digest 0.10.7 CoreWrapper, whose Update implementation
passes each update to block-buffer 0.10.4 digest_blocks. Ordered fragment updates
can produce the same message digest without the aggregate buffer. However, each
fragment enters buffering logic; signed-token count has no local cap, and method
ASCII folding also needs a deliberate fragment/buffer strategy. Transport bounds
still apply, so this is not a claim of unbounded network input. A one-pass emitter
may initialize the MAC and hash earlier components before a late missing header
is discovered. Whether saved allocation/copy work outweighs these costs depends
on input shape and has not been measured.

The CSRF signature-input-fragments precedent demonstrates exact-byte incremental
hashing for a fixed small message shape. It explicitly makes no CPU benefit claim;
it does not establish the cost of variable HMAC header lists. This decision is
separate from hmac-01 membership ownership and hmac-02 prepared keyed states.

## Decision alternatives

- Retain the current complete-message buffer until offline measurements identify
  the request shapes and cost contribution that justify changing it.
- Use one canonical fallible fragment emitter, consumed by the selected generic
  MAC implementation. A test sink may collect the same fragments to verify bytes;
  do not maintain separate production and test canonicalization algorithms.
- If measurements justify it, coalesce small fragments in bounded stack storage.
  Evaluate complexity and copy cost before adding that mechanism. Do not replace
  one temporary String with a fragment Vec or owned copies of borrowed headers.

Prevalidating all headers before streaming would avoid hashing failed requests
but can repeat lookups or require retaining values. Treat that as an explicit
tradeoff, preserving access order and snapshot behavior. No new input rejection
limit, signature format, global cache, production metric or request log is
licensed by this task. A choice requires its own IMPL plan and review.

## Acceptance for a selected implementation

Preserve exact signed-header order and original header-value bytes, colon/space
separators, inter-component newlines without a trailing newline, ASCII-only method
lowercasing, both request-target spellings, current normalized path and raw current
query. Absent and empty queries omit the question mark. Percent encodings, plus
signs and repeated query keys remain raw; do not use the decoded entry-query cache.

Retain parser duplicate rejection, all preceding authentication checks, first
missing-header lookup order and generic failure outcome, algorithm selection and
constant-time verify_slice comparison. Missing-header failure must still precede
bad-signature failure, including malformed tag lengths. Signature validation must
still finish before digest-signed enforcement and asynchronous body capture.
Standalone plugin and compiled condition outcomes retain their different policies.
No sensitive input or derived keyed state may enter new diagnostics.

Keep fixed SHA-256/384/512 vectors and compare a proposed implementation with the
existing canonical bytes and verification outcomes. Include empty/long values,
both targets, mixed-case methods, raw query boundaries, first/late missing fields,
wrong signatures and lengths, short/block-sized/long keys, and messages crossing
SHA block boundaries. Cover standalone and compiled header/body consumers,
including signed-body tampering and failure ordering.

Measure allocation calls/requested bytes and CPU separately for small/large
header counts and values, long targets/queries, and late-missing-header failures
across all three algorithms. Construct fixtures outside measurement and compare
complete verification as well as canonicalization. Fewer allocations alone do
not establish throughput gains; repeated update calls alone do not prove a loss.

## Original knowledge and verification

- `skills/04-review/hmac-auth/regression-guards/wire-format-is-edgion-contract.md`
- `skills/04-review/hmac-auth/regression-guards/request-body-hash-after-cheap-auth.md`
- `skills/04-review/hmac-auth/review-evidence/signed-headers-scope.md`
- `skills/04-review/cpu-memory/regression-guards/header-hmac-condition-verifier-prebuilt.md`
- `skills/04-review/csrf/regression-guards/signature-input-fragments.md`

Fresh baseline verification passed 56 HMAC-filtered and 68 condition-system unit
tests (the filters overlap), formatting check, workspace check and Clippy with
existing unrelated warnings. These establish the current baseline, not a future
fragment implementation's correctness or performance. This task changes no runtime.
