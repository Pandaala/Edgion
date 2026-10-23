# Prepare native CORS origin patterns

Status: deferred for user decision. No runtime change.
Source baseline: 7cd42d4f6.

## Evidence and scope

GapiCorsFilter retains validated origin strings. Its is_origin_allowed loop
checks the global wildcard and exact equality before origin_matches_pattern
checks for a wildcard, splits the fixed pattern's port, splits the incoming
Origin's port, and locates the wildcard prefix/suffix. Multiple wildcard
candidates can therefore repeat parsing of both fixed patterns and the same
live Origin. This is bounded string scanning, not per-match heap allocation.

HTTPCORSFilter::validate limits the list to 64 origins and each origin to 253
bytes. The global '*' is exclusive: a valid list cannot mix it with other
origins. The performance opportunity is primarily repeated fixed wildcard work
and repeated live-Origin splitting, not a large mixed global-wildcard list.

Both request and upstream-response callbacks call the matcher against the
current session Origin. Header reflection and credential presence remain live
phase decisions. The existing response-value preparation and preflight value
transfer do not prepare the origin matcher.

## Decision requested

Select a measured, bounded preparation design: for example, pattern-kind and
substring offsets beside each owned string, with one live-Origin split for
wildcard candidates. Avoid duplicating a full raw configuration or separately
owned copies of every pattern fragment. No representation is selected here.
Exact-only and short-list requests may gain little while paying metadata and
construction costs. Compare that tradeoff before choosing the implementation.

Design rule U directs reusable preparation to configuration time. Leaving the
current repeated pattern work is not declared an accepted exception by this
task; the user decision concerns representation, value and implementation scope.
No request-time cache or new schema field is authorized.

The EdgionPlugins CORS matcher has different shorthand/regex and credential
contracts. Its HTTPS-only '*.domain' shorthand is not the native filter's
explicit 'http://*.domain' / 'https://*.domain' model. Reuse any existing matcher
only after proving equivalent semantics; do not silently substitute DNS hostname,
URL-normalization or regex policy for native raw origin matching.

## Acceptance before implementation

Use IMPL with an exact source allow-list. Measure cold construction, per-rule
retained bytes and warmed matching CPU for exact-only, wildcard-only and mixed
valid lists, early/late hits and misses, short/maximal lists and patterns. Include
singleton '*' separately. Separate matching from header reflection/insertion;
no throughput claim follows from eliminating string scans alone.

Canonical validation must precede preparation, with one immutable runtime
generation and no duplicate raw configuration. Preserve rule-scoped ownership,
Controller/Gateway validation parity and equivalent non-Kubernetes ingest paths.
Any shared helper must remain with the correct canonical owner.

Use a differential matcher reference over valid configured patterns and diverse
incoming strings. Preserve raw exact equality before wildcard interpretation,
case-sensitive scheme/host text, explicit port suffix equality, absent versus
explicit default port, and leading-zero port spelling. For example ':0443' and
':443' are not equal under the current string comparison. Preserve multi-label
wildcard matching, prefix/suffix boundaries and complete-host wildcard behavior.
Include empty, null-like, malformed, non-ASCII and delimiter-heavy incoming values
without treating them as newly approved wire inputs. A semantic/security change
found during comparison needs separate review, not an optimization side effect.

Do not cache the allowed-origin result across phases: rewrites can change the
live request headers. Keep credential-dependent reflection, preflight handling,
actual-response header policy, ignored insertion failures and logging unchanged.
No extra per-request metrics or tracing is authorized. Existing cors-01 Max-Age
preparation and cors-02 session value transfer remain separate decisions.

Fresh baseline verification passed 47 Gateway CORS tests. Existing exact,
wildcard, port, validation and callback tests are starting evidence, not proof
of a new prepared representation. No matching benchmark was run.
Original sources: `plugins/runtime/gateway_api_filters/cors_filter.rs` under
Gateway, `edgion-resources/src/resources/http_route.rs`, design rule U, and
`skills/04-review/cors/regression-guards/native-v15-validation-parity.md`,
`skills/04-review/cors/regression-guards/native-prepared-response-values.md`,
`skills/04-review/cors/intentional-design/subdomain-wildcard-https-only.md`.
