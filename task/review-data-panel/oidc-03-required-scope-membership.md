# Evaluate OIDC required-scope membership storage

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `23b370661`.

## Evidence and decision

OpenidConnect::validate_required_scopes in
`edgion-gateway/src/plugins/http/openid_connect/config_validation.rs` builds a
request-local HashSet<&str> from the available claims before checking configured
requirements. It already borrows scope text: there is no per-scope String copy.
HashSet::new does not allocate until insertion; missing or empty available claims
can reject without table storage. Nonempty insertions allocate/grow the table.
Both introspection paths and enabled JWT scope verification call this helper.

Choose whether to retain this strategy, investigate repeated borrowed scans, or
prepare a generation-owned required-scope index with per-request match accounting.
Let A count available scope occurrences and R configured requirements. Current
work is expected O(A + R) hash-table operations, plus tokenization, hashing and
string-equality byte costs. Repeated scans can avoid table allocation but require
O(A * R) comparisons; actual work depends on match positions and early rejection.
A prepared index can avoid storing irrelevant available scopes but needs bounded,
duplicate-safe scratch state for each request. No required-scope cardinality cap
is imposed by the current field/validation code; do not assume a tiny fixed R.

There is no candidate implementation or workload benchmark. Fewer allocations
alone do not establish lower CPU cost, especially for many requirements or long
and duplicate-heavy claim values. Do not introduce a new scope-count limit,
configuration knob, shared mutable request state or another cache as an incidental
optimization.

## Semantic boundaries

Preserve the bearerOnly guard and the immediate success for absent/empty required
lists. String scope and string scp split with Rust split_whitespace, including
Unicode whitespace; their tokens form one union. An scp array contributes each
string element intact, including empty strings or strings containing spaces;
non-string elements are ignored. Other scp types and non-string scope are ignored.
Required strings are compared exactly, without trimming, case folding or splitting.
Duplicates in either source or required configuration must not change the result.

Missing any requirement returns the existing fixed InsufficientScope error.
Keep claim validation, introspection caching and authentication ordering intact;
ID-token nonce verification still disables scope validation. Preserve generation
isolation and cancellation cleanup if preparing indexes or request scratch.
No scope names, tokens or claims may be added to logs or metric labels.

## Acceptance for a later implementation

Use IMPL with an exact allow-list. Differential tests must cover both claim
sources together and independently, absent/empty/wrong JSON types, repeated
requirements, case differences, string whitespace versus whole array elements,
empty array strings, permitted Unicode whitespace, misses and mixed arrays.
Retain existing bearer-only and authentication integration boundaries.

Compare allocations, requested/retained bytes and CPU across small and large A/R,
long strings, duplicate-heavy inputs, early/late successes and failures, and
unused available scopes. Include preparation and per-request scratch costs in
the ownership assessment; do not benchmark only a prebuilt lookup operation.

Fresh unchanged-code verification passed three required-scope tests covering
union, non-string/missing-scope handling and bearer-only behavior. They do not
prove candidate equivalence or cover all proposed edge cases. No benchmark ran.
Authority: `skills/04-review/oidc/intentional-design/required-scopes-bearer-only-coupling.md`.
