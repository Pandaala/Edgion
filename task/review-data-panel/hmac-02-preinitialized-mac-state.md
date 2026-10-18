# Preinitialized HMAC key state

Status: deferred CPU/retained-memory decision for the user.
Source baseline: `2fcc91654`.

## Evidence

The standalone plugin and compiled conditions already retain immutable
HmacVerifier generations. That avoids rebuilding configuration and credential
maps per request, but `HmacAuth::verify_signature` still calls new_from_slice
with the selected credential secret for each verification, for SHA-256, SHA-384
or SHA-512, before processing the signing string and calling verify_slice.

The locked `hmac` 0.12.1 implementation (`src/optim.rs`, HmacCore KeyInit) derives
a block-sized key, initializes the inner pad digest and initializes the outer pad
digest. `src/lib.rs::get_der_key` hashes keys longer than the hash block size.
HmacCore implements Clone for its digest states. These sources establish repeated
key-dependent work and a possible immutable-template strategy; they do not prove
an application CPU or throughput improvement. This is not evidence that current
key initialization allocates heap storage.

The current verifier owns an Arc credential map cloned at construction. Default
configuration permits all three algorithms; each compiled verifier and retained
old generation has its own ownership. Preparing every allowed algorithm for every
credential would retain derived keyed state proportional to those dimensions,
even for rarely used credentials. Request-local cloning also has a cost. Concrete
sizes depend on the selected layout, dependency features and target; no byte-size
or workload claim has been measured here.

## Decision alternatives

- Keep request-local initialization until bounded offline measurements establish
  its contribution to the complete header verification cost.
- Prepare immutable keyed states at verifier construction for the allowed
  algorithms, then clone the selected initial state for each request. Compare
  startup/reload work, resident memory and request CPU against the current form.
  Never update/finalize the shared template, introduce a locked mutable MAC pool,
  or lazily publish request-created keyed states as an unreviewed shortcut.

A selected design must define the prepared credential owner and its relationship
to raw credential bytes and metadata. The source resource/conf-sync owner remains
independent; replacing one runtime copy does not eliminate all secret owners.
Derived keyed states are sensitive too: preserve redacted Debug and diagnostics,
keep them out of schema/admin transport, and do not claim drop proves zeroization.
Retain only the selected generation's state and preserve existing in-flight
snapshot ownership during rotation. This record does not authorize a new
cross-generation credential cache or a broader secret-lifecycle redesign.

## Acceptance for a selected implementation

Preserve the exact SHA-256/384/512 results and verify_slice length/constant-time
comparison behavior. Keep algorithm admission, credential/date/required-header
checks, signed byte ordering and signature-before-body-capture behavior. Retain
first-wins resolved credentials, success-only group attribution, sensitive
forwarded metadata and existing failure text/status policies. Do not add secret
or raw-identity logs, request metrics, algorithm restrictions or key-length limits.

Test empty direct keys and short/block-sized/long keys for each algorithm,
repeated and concurrent requests without message-state contamination, wrong
keys/tags/lengths, valid signed-body tampering, and all compiled condition paths.
Verify reconstruction/rotation binds the new generation while old in-flight
owners remain usable until dropped; establish owner release without claiming
cryptographic memory erasure. Preserve the existing fixed signing vectors.

Measure construction cost, request clone/init CPU and retained memory separately,
across credential/algorithm counts and overlapping generations. No production
instrumentation is required. Any selected implementation needs its own IMPL plan,
independent review and verification; this task makes no runtime change.

Relevant original knowledge:
- `skills/04-review/cpu-memory/regression-guards/header-hmac-condition-verifier-prebuilt.md`
- `skills/04-review/security/review-evidence/credential-copy-ownership.md`
- `skills/04-review/hmac-auth/regression-guards/wire-format-is-edgion-contract.md`
- `skills/04-review/hmac-auth/regression-guards/request-body-hash-after-cheap-auth.md`
- `skills/04-review/hmac-auth/mandatory-design/dup-key-first-wins.md`

Evidence in this record is fresh source inspection, not a performance benchmark
or a new acceptance of broader retained credential state.
