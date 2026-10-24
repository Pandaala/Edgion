# BasicAuth immutable verifier preparation

Status: deferred for user decision. No runtime change.
Source baseline: `ead0254c0`.

## Evidence and scope

The request-filter path reaches authenticate_request, pins the immutable user-map
generation for a known username, and runs verify_pinned_user in spawn_blocking.
PreparedUser currently owns a password_hash String and group String. Despite the
name, prepare_users only classifies the password and converts plaintext to a
SHA-1 digest encoded as a `{SHA}` string; supported and recognized unsupported
hash strings are moved into the map unchanged.

Each verification redispatches on the stored prefix. SHA-1 decodes the unchanged
configured Base64 digest into a 20-byte stack buffer before computing the
presented password digest. This decoding already uses no heap scratch. MD5-crypt
and APR1 locate the salt separator again before computing the password-dependent
1,000 rounds and comparing the complete formatted result.

Preparing the immutable digest or format metadata once could remove repeated
decoding/parsing. Plaintext SHA-1 construction could also avoid the intermediate
Base64 representation. This is not a request-result cache: the presented password
would still be hashed on each verification. The value relative to password hashing
and blocking-pool dispatch has not been measured.

## Decision requested

Choose SHA-only digest preparation, a common prepared representation for all
supported formats, or retain strings pending measurement. Compare mixed-format
map entry size, construction cost and retained allocations as well as request CPU;
an enum's layout and total memory savings cannot be inferred from digest length.
Keep one canonical verifier rather than a parallel optimized implementation.

Malformed SHA strings and recognized unsupported formats currently remain known
user entries and deny at runtime. A prepared invalid sentinel could preserve this
behavior; dropping entries would change their dispatch path, while rejecting the
whole plugin would change admission behavior. Do not silently choose either.
Preserve exact case-sensitive prefix classification: unrecognized prefixes are
treated as plaintext, whereas recognized unsupported hashes are not rehashed.

MD5 preparation must retain whole-result comparison semantics, not accept a
different salt or suffix grammar through normalization. Its output scratch and
malformed UTF-8 salt truncation are the separate
[basic-auth-01 decision](basic-auth-01-md5-result-storage.md). This task neither
accepts that panic nor authorizes resolving it as an incidental optimization.

## Acceptance before implementation

- Use IMPL with independent plan and final reviews. Keep runtime preparation
  local to the Gateway; do not change Secret resolution or schema/transport.
- Compare SHA preparation against current verification for valid, malformed,
  incorrectly padded, noncanonical and wrong-length Base64, including empty and
  non-ASCII passwords. Retain the full digest XOR comparison and length checks;
  source structure alone is not a machine-code constant-time proof.
- Cover plaintext and all prefix-classification branches, successful group
  attribution, uniform bad-credential denial, and known/unknown dispatch.
  Preserve pinned old/new generations across replacement and blocking work.
- Preserve the shared failure delay, anonymous-access rules, sensitive headers
  and credential-free failure logs. Add no dummy verification, bcrypt, result
  cache, tracing, metrics or new credential exposure surface.
- Replace representation-specific pointer/string assertions only with explicit
  ownership and behavior coverage for the selected design. Preserve existing
  security, wire-format and generation tests.
- Measure actual verifier CPU/allocation effects separately from construction
  and blocking dispatch. Do not claim request allocation savings from removing
  SHA decoding, which already uses stack scratch, or claim physical erasure.

## Sources and current verification

- `edgion-gateway/src/plugins/http/basic_auth/plugin.rs`: PreparedUser,
  prepare_users, classify_password, pin_known_user_generation,
  authenticate_request and verify_pinned_user.
- `edgion-gateway/src/plugins/http/basic_auth/htpasswd.rs`: verify,
  verify_sha1 and verify_md5_variant.
- Mirrored unit tests under `edgion-gateway/tests/unit/plugins/http/basic_auth/`.
- `skills/04-review/basic-auth/` and `skills/04-review/auth-timing/`.

Fresh existing tests passed all 19 BasicAuth cases (the basic_auth filter also
selected one passing Elasticsearch configuration case). These verify current
behavior, not a future prepared representation. Formatting and workspace
all-target check/Clippy passed with existing warnings. No full suite, live reload
exchange or performance benchmark ran.
