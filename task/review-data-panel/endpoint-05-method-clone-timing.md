# Decide endpoint probe method ownership and clone timing

Status: deferred for user decision. No runtime change.
Source baseline: 74cae4730.

## Evidence

`edgion-gateway/src/plugins/http/all_endpoint_status/plugin.rs::selected_method`
clones the prepared override, or parses the inherited method once. The caller
performs this selection after the empty-endpoint return. `fan_out_requests`
then clones the selected Method for every endpoint while collecting the local
FuturesUnordered, before polling, permit acquisition, latency start, transport
preparation and creation of the wall timer in `fan_out_with_wall_timeout`.

Locked `http` 1.5.0 stores standard methods and extension tokens of at most 15
bytes inline. Longer extensions own a `Box<[u8]>`, which derived Clone copies.
Thus ordinary GET/POST probes do not allocate method storage at these clones;
the allocation opportunity concerns long extension tokens. Locked reqwest
0.12.28 `Client::request` takes an owned Method for each outbound request.

## Decision

Consider these separately:

- Borrowing the selected prepared override could avoid one long-token clone
  per invocation. Inherited methods still require parsing and an owned result.
  Assess a borrowed/owned selection representation against its common-method
  overhead; this does not inherently move per-endpoint clone work.
- Borrowing the selected method in pending endpoint futures and cloning only
  after permit acquisition and successful transport preparation could avoid
  copies for cancelled or preparation-rejected probes. Successful requests still
  need their own methods. This moves clone work inside the wall budget, permit
  interval and endpoint latency measurement; near-deadline completions may
  change. Keeping current eager clones remains an option.

There is no selected representation or measured throughput/RSS benefit. The
operation is a bounded diagnostic fan-out, so establish value for realistic
method lengths and endpoint counts before implementation. Do not broaden this
into a shared method cache or dependency change.

## Required acceptance

- Preserve case-sensitive method tokens, override precedence, cold rejection of
  invalid overrides, invalid inherited-method failure, and the ability of a valid
  override to ignore an invalid original method. Keep the empty-endpoint return
  before inherited-method parsing and retain fixed error text/status.
- Compare scoped allocation counts for standard methods, 15-byte and longer
  extensions, inherited and overridden methods, successful preparation, failed
  preparation, cancellation and endpoints never admitted by the semaphore.
- Preserve partial-result ordinals/order, ready-at-deadline selection, permit
  cleanup and cancellation. Explicitly approve any change to deadline/latency
  boundaries; helper tests alone do not establish real network equivalence.
- Preserve generation checks, endpoint pinning, per-endpoint client isolation,
  TLS/SSRF policy, bounded response reads and credential-safe diagnostics. Add
  no request-path tracing or raw method/header logging.
- Use IMPL with an exact source allow-list and independent review. Existing
  method-selection tests cover token/error behavior, not long-token allocation
  savings or a delayed-clone timing change.

This is distinct from [result address ownership](endpoint-01-result-address-ownership.md)
and [forwarded-header preparation](endpoint-04-forwarded-header-preparation.md).
It adds no runtime code, tests or benchmark results.
