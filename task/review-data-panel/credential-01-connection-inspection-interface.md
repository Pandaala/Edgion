# Evaluate borrowed Connection inspection for credential injection

Status: deferred for user decision. Priority: P3. Type: performance investigation.
Inspected baseline: `420ba6445`.

## Evidence and decision

CredentialInjector::run_request is the sole production caller of
PluginSession::request_header_values. PingoraSessionAdapter delegates to
utf8_header_values, which collects every matching HeaderValue into Vec<String>.
The plugin only inspects borrowed Connection tokens for a case-insensitive match
against its already compiled target name. The existing borrowed tokenizer avoids
token copies, but not this earlier per-field copy and result vector. An absent
Connection header already produces an empty vector without payload allocation.

Target parsing, prefix concatenation, sensitive HeaderValue construction and
expiry conversion already occur at plugin construction. Credential Arc ownership
and per-attempt HeaderValue clones serve different lifetimes and are outside
this candidate. This task concerns only temporary Connection field text.

Removing the copies requires a shared PluginSession interface choice: an
object-safe borrowed inspection callback, a narrowly scoped nomination query,
or another reviewed view. Decide whether the expected workload benefit justifies
that API change. A boxed iterator or replacement per-field allocation must not be
presented as allocation-free. No candidate or benchmark has been implemented.

## Required boundaries

Preserve every repeated field and the canonical connection_option_tokens parser,
including trimming, empty tokens and case-insensitive comparison. Validate live
post-plugin request headers, not an entry snapshot or cached nomination set.

The current collector completes HeaderValue::to_str validation before the plugin
checks tokens. Therefore an invalid later field takes precedence over an earlier
target nomination: the plugin emits invalid Connection header, not credential
target is connection-scoped. A fused scan must preserve that diagnostic priority.
Keep absent and invalid distinct. Preserve the exact to_str acceptance predicate;
it is stricter than general UTF-8 validity. Do not replace it with from_utf8.

Expiry and reserved-target checks still precede inspection. Successful inspection
must precede removal of the client value and deferred credential registration.
Keep the final upstream hook's independent checks of both the session request
and attempt-local header views: Pingora sanitization can remove nominations from
one view. Keep all sensitive-value flags and canonical logs; no credential names,
values or raw Connection data may be added to diagnostics.

Review all real adapters and mock call sites when choosing the interface. The
current default uses request_headers and cannot recover invalid raw values that
an adapter already omitted; this investigation does not establish a new fix for
that separate limitation. Prefer the direct production read even if mock wiring
needs adjustment. Do not add a compatibility shim solely to retain an unused API,
a second token parser, or a request-header cache.

## Acceptance for a later implementation

Measure allocations and CPU for absent, one-field, repeated-field and long-token
inputs, including nominated and invalid values. Test invalid fields before and
after a nomination, mixed case, empty tokens, unrelated Upgrade, header mutation
between plugins, reserved targets, expiry, removal failure, deferral failure and
multiple upstream attempts. Verify exact error precedence and no mutation on
rejection. Keep the final dual-view guard and sensitive-value tests.

Use IMPL with an exact adapter/trait/plugin/test allow-list and independent review.
Fresh unchanged-code verification passed seven CredentialInjector, 17 deferred
credential and one raw header-value helper tests. These do not validate a proposed
API or establish end-to-end performance. Relevant authority:
`skills/04-review/security/regression-guards/deferred-credential-connection-nomination.md`.
