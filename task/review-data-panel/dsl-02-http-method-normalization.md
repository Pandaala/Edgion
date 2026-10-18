# DSL outbound method normalization

Status: deferred for user decision. No runtime change.
Source baseline: e4e1833de.

## Evidence and scope

`Vm::call_builtin_http` converts the method with Unicode `to_uppercase()` and
matches seven fixed methods: GET, POST, PUT, DELETE, PATCH, HEAD and OPTIONS.
This materializes a temporary String on every call. Pure ASCII-insensitive
comparisons would avoid that allocation, but are not an equivalent replacement:
Unicode uppercase mappings can turn non-ASCII input into ASCII letters (for
example the Unicode long-s mapping can affect a spelling of POST or OPTIONS).
This is source and standard-library semantic evidence, not an executed DSL or
wire reproduction. Existing acceptance is observed behavior, not an approved
requirement to preserve every unusual spelling indefinitely.

The existing request-method case contract explicitly excludes this outbound
builtin. General Method parsing is also not a substitute: it would admit
extension methods and preserve case rather than normalize only this allow-list.
Current method errors include the original input and occur after URL/scheme/SSRF,
Body snapshot/type and timeout validation, but before header-map validation.
The existing method-position test covers a Body type error; it is not a casing
or complete supported-method matrix.

## Decision requested

Choose whether to investigate a preserving fast path (ASCII comparisons with the
original Unicode path retained), explicitly define an ASCII-only outbound method
contract, or retain the current implementation. The preserving option avoids a
semantic change but adds branch/comparison work; no workload measurement yet
establishes that the tradeoff matters for a network-bound call. An ASCII-only
contract needs separate authorization and an explicit rejection/error policy.
Do not add a compatibility switch or expand the supported method set incidentally.

## Acceptance before implementation

Use IMPL with a bounded source allow-list. Compare allocations and normalization
cost for common uppercase, lowercase, mixed-case and unsupported inputs; do not
claim request throughput from a temporary-allocation count. Validate the chosen
Unicode policy without violating repository content-language rules.

Retain the seven methods unless a scope change is approved. Verify empty and
unsupported methods, exact ApiError category and original-input message, argument
pop/coercion order, and competing malformed URL, Body, timeout and header errors.
Keep SSRF, client selection, redirects, request snapshots, deadlines and bounded
response handling unchanged. Tests should exercise the actual builtin selection;
method-parser tests alone do not cover its argument-validation order.
