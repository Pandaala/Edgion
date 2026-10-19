# Prepared authentication challenge generation

Status: deferred preparation/retained-memory decision for the user.
Source baseline: `72fbf83b1`.

## Evidence

The canonical gateway `send_gateway_auth_error_response` in
`edgion-gateway/src/plugins/http/common/auth_common.rs` validates and escapes the
raw realm for every failure response, then constructs WWW-Authenticate. Recent
changes already borrow clean escaped fields, allocate the challenge at exact
capacity and transfer its buffer into HeaderValue. The remaining candidate is
moving stable work to construction, not removing the already eliminated copy.

BasicAuth, KeyAuth, JwtAuth, OpenidConnect, LdapAuth and HmacAuth retain configurable
realms in their runtime configuration or owner. HeaderCertAuth and JweDecrypt use
constant `edgion`. Scheme and realm are stable for each plugin generation, but
Bearer error attributes vary by outcome. JwtAuth distinguishes missing credentials
from invalid-token failures; OIDC distinguishes mixed 401 failures from 403
insufficient-scope failures, whose description uses the caller's body. A complete
request-bound challenge cannot be cached solely by realm.

Normal runtime validation rejects invalid realm configuration. Direct constructors
such as BasicAuth::new and HmacAuth::new return Self rather than Result, while the
shared response helper independently rejects invalid raw realms before any
response write. Removing that check without a verified preparation contract would
change the direct-constructor boundary. Existing raw realm serialization and
plugin-specific empty-realm rules remain separate from escaped wire storage.

## Decision alternatives

- Keep construction local to failures until measurements establish meaningful
  denial-path cost. Authentication delay and other response work remain separate
  costs; neither proves that challenge preparation is useful or useless.
- Retain an immutable prepared realm/base challenge per runtime generation,
  produced by one canonical gateway preparation API. A complete challenge can
  serve None-code paths; Some-code paths still need outcome-specific attributes
  and description handling unless a separately reviewed finite variant set exists.
- Evaluate the constant-realm consumers independently if that yields a narrower
  useful scope. Do not turn this into an implicit global cache or a cross-plugin
  mutable response template.

A selected design must define its preparation owner, validation result and exact
consumer allow-list. Do not duplicate escaping in individual plugins, silently
strip realm controls, substitute a default realm, or unwrap invalid preparation
in a nonfallible constructor. Preserve failure before response writes for direct
invalid construction unless a separately authorized API redesign changes it.
Do not add a Controller dependency on Gateway code or put prepared header bytes
into shared resource/conf-sync schema merely to reuse runtime preparation.

Prepared data adds retained state to every selected generation, including ones
that rarely reject requests and old generations retained by in-flight work.
HeaderValue cloning/sharing has a cost too; do not assume the first clone and
steady-state clones have identical allocation behavior on pinned http/bytes.
Compare representation sizes and clone/build costs before choosing a layout.
This is not permission to cache status/body, request-specific errors or secrets.

## Acceptance for a selected implementation

Preserve raw realm configuration, quotes/backslashes and UTF-8, ASCII-control/DEL
rejection, plugin-specific empty rules, exact scheme/code bytes and existing
error-description escaping. None-code paths must not add error attributes;
Some-code mappings retain the documented Bearer policy. Body text, status, failure
delays, diagnostics, header/body write ordering and shutdown behavior remain owned
by their current paths. A prepared header must not be mutated as a shared response.

Test normal validation, direct construction with invalid realms, exact challenge
bytes for every selected consumer, dynamic descriptions, repeated/concurrent
responses and header/body write failures. Reconstruct a generation after realm
updates: new requests use the new challenge, old in-flight owners remain coherent,
and retired prepared owners release when their generation drops. No process-global
cache may return a previous realm after an update or grow with historical realms.

Measure construction CPU, retained memory across overlapping generations, first
and repeated clone allocations, and complete denial-path CPU/allocation against
the current exact-capacity builder. Distinguish borrowed plain realms from escaped
realms and None-code from Some-code cases. No production logs or metrics are
needed. Any implementation requires a separate IMPL plan and independent review.

## Original knowledge and baseline

- `skills/04-review/security/regression-guards/auth-challenge-realm-quoted-strings.md`
- `skills/04-review/jwt-auth/regression-guards/bearer-www-authenticate-error-attributes.md`
- `skills/04-review/rate-limit/intentional-design/deny-response-shutdown-scope.md`

Fresh baseline verification passed 36 shared-auth, 14 KeyAuth, 47 JwtAuth,
19 BasicAuth and 56 HMAC-filtered tests (172 total), formatting check, workspace
check and Clippy with existing unrelated warnings. These establish the current
behavior, not the correctness or performance of a prepared representation.
This task changes no runtime code and makes no throughput claim.
