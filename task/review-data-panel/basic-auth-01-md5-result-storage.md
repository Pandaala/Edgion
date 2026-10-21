# BasicAuth MD5 result storage

Status: deferred for user decision. No runtime change.
Source baseline: `6e9e51949`.

## Evidence and scope

Gateway `plugins/http/basic_auth/htpasswd.rs` computes MD5-crypt/APR1 during
password verification. After 1,000 stretching rounds, compute_md5_crypt creates
one Vec containing magic, at most eight salt bytes, a separator and 22 encoded
digest bytes. Its only production caller supplies `$apr1$` or `$1$`; maximum
result lengths and requested capacities are therefore 37 and 34 bytes.
String::from_utf8 consumes this Vec without a second result-buffer allocation.
Requested capacity is not a measurement of allocator heap usage.

A bounded stack result or an encoded-digest scratch buffer could remove that
final allocation. No allocation benchmark or throughput measurement establishes
its value relative to the 1,000 rounds. Existing SHA-1 scratch storage and pinned
user-generation ownership are separate, already implemented optimizations.

## Decision requested

Choose bounded byte storage, a separate 22-byte encoded result, or retain the
current representation after measuring the real verifier. Avoid a second hash
implementation or unnecessary abstractions for this small output.

Resolve malformed-salt handling explicitly before implementation: current code
truncates raw salt bytes to eight, which can split a valid UTF-8 code point. The
final String::from_utf8(...).expect(...) can then panic even though the encoded
digest alphabet is ASCII. prepare_users preserves supported hash strings without
validating the salt. authenticate_request runs verification in spawn_blocking
and maps a join error to BadCredentials. This is source-inspected behavior, not
a live panic-test result or a requirement to preserve a panic. Do not silently
change acceptance or bundle an unspecified failure-policy change into a storage
optimization; separately decide and test safe malformed-salt rejection.

## Acceptance before implementation

Use IMPL with independent plan and final reviews. Preserve supported algorithm
selection, custom alphabet and byte interleaving, round inputs/order, valid-salt
semantics, wrong-password rejection and whole-result comparison. The existing
length check plus full XOR fold is a source-level property, not a machine-code
constant-time proof. Do not replace it with an early-exit byte comparison.

Cover both variants with independent fixed vectors: the current APR1 test has
one, while the MD5 test is a round trip. Exercise empty/eight-byte/long salts,
non-ASCII salts and truncation boundaries, empty/long passwords, malformed stored
hashes and wrong passwords. Verify the agreed malformed-salt behavior through
the plugin failure path, including shared delay and uniform denial metadata.
Measure allocation and CPU effects through actual verification, keeping digest
work separate from output construction when reporting the result.

Keep blocking execution, generation pinning, credential lookup, successful group
attribution and sensitive-data redaction unchanged. Add no bcrypt support,
credential cache, new logs or authentication policy. No runtime/test changes or
performance improvement are claimed by this deferred record.

## Sources

- `edgion-gateway/src/plugins/http/basic_auth/htpasswd.rs`: verify,
  verify_md5_variant, compute_md5_crypt and to64.
- `edgion-gateway/src/plugins/http/basic_auth/plugin.rs`: prepare_users,
  classify_password, verify_pinned_user and authenticate_request.
- `edgion-gateway/tests/unit/plugins/http/basic_auth/htpasswd_test.rs`.
- `skills/04-review/basic-auth/SKILL.md` and its mandatory-design pages.
- `skills/04-review/security/mandatory-design/iron-rules.md`.
