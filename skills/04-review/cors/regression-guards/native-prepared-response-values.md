---
name: native-prepared-cors-response-values
description: Native CORS prepares fixed wire values after validation and borrows them during header preparation.
knowledge_type: regression-guards
---

# Native CORS prepared response values

GapiCorsFilter consumes HTTPCORSFilter only after its canonical validation passes.
It prepares origin pattern structure and method/header policies, exposed-header
text and Max-Age once. It does not retain a duplicate raw configuration. Fixed
lists preserve order and the comma-space delimiter; missing or empty lists omit
the header. Credentialed lists containing an exact wildcard reflect the
corresponding request header, preserving missing versus empty values.

Header pairs borrow fixed values and the caller-provided origin. Reflections own
their values before session mutation; the returned lifetime must not borrow the
session. Cookie/Authorization presence remains a live decision for wildcard
origins without configured credentials. Configured credentials always echo the
origin. Origin matching, validation, Vary append, header ordering and errors are
unchanged. Native methods/allow-headers remain emitted on actual responses as
well as preflight; Max-Age remains preflight-only. This optimization does not
establish or change that native response policy.

The native filter unit tests cover fixed/empty/missing/wildcard policies,
credential presence, reflection values and both response phases. Warmed
header-pair construction allocates zero for fixed values and two for two
nonempty reflected values. This excludes origin acquisition and final header
insertion, and makes no end-to-end throughput claim.

The request callback borrows the first Origin value when textual directly from the
live request map. Missing or non-text first values remain absent, even if a later
duplicate is valid. Both private `is_preflight_with_origin` calls reuse that
textual-Origin premise with no intervening request-header mutation or await.
Classification still checks OPTIONS before textual Access-Control-Request-Method
presence; empty text remains present and non-text remains absent. This removes
one redundant Origin lookup for OPTIONS requests reaching classification.
Matching and response preparation use shared session
references; an owned local ResponseHeader is finished before asynchronous
session writes begin. The upstream-response callback still owns its Origin
while mutating session response headers. Reflection values retain ownership.
Callback tests forbid the owned getter for Origin and cover allowed/denied,
missing/empty/non-text/duplicate origins and actual/preflight dispatch.
This removes the Origin copy, not every allocation in the async callback.

The preflight builder consumes its header pairs. Owned method/header reflections
move into Pingora insertion; borrowed fixed values and Origin keep the borrowed
insertion path. Invalid header insertions are still ignored individually, so
later headers and Vary survive. The pinned String conversion validates the same
bytes and sensitivity state, but can retain spare capacity or allocate Bytes
metadata. This removes a payload copy, not necessarily every allocation.
The differential native preflight test compares status, headers and HTTP/1 wire
bytes against borrowed insertion for fixed/reflected, absent/empty, non-ASCII and
synthetic invalid values. Actual-response session mutation remains unchanged.

Re-evaluate when adding configuration fields, changing canonical validation,
reflection ownership, origin credential policy or header-pair capacity.


## Prepared origin structure

`cors-03-native-origin-preparation` moves fixed wildcard parsing into the validated
`GapiCorsFilter` generation. Each private origin entry is Any, an owned exact
string, or the original wildcard string plus byte offsets for its star and port
boundary. Existing source String buffers move into the runtime; pattern fragments
and a second raw configuration are not copied. Clones own corresponding strings
and offsets, and old instances retain their own patterns during replacement.

Matching keeps exact raw equality first, case-sensitive scheme/host prefix and
suffix comparisons, multi-label/empty wildcard behavior and exact port suffixes.
An omitted port differs from an explicit default; leading zeroes are significant.
Live origins are not normalized or newly validated. A local lazy split is shared
among wildcard candidates during one call, and exact-only matches do not need it.
The request and upstream-response phases independently read and match the current
Origin, so header mutation cannot reuse a stale allow decision. Credential-based
reflection, header order, preflight policy, ignored insertion errors and logging
are unchanged. EdgionPlugins CORS shorthand and regex contracts remain separate.

A differential reference preserves the prior matching algorithm for valid configured
patterns and diverse incoming values, including unusual values that are not new
wire-admission promises. Coverage includes the 64-pattern and 253-byte bounds,
full-host wildcards, literal-pattern equality, ports, UTF-8, empty/null-like and
malformed inputs, instance replacement and cross-phase header changes. Matching
continues to allocate zero; this optimization removes scans rather than allocations.
The offset enum adds per-entry metadata (40 versus 24 bytes on the measured 64-bit
build), and conversion costs cold work and a larger vector. Source-bound isolated
CPU samples demonstrate reduced wildcard matching work, not a full gateway
throughput or latency guarantee. Exact early-hit cases may see little benefit.
