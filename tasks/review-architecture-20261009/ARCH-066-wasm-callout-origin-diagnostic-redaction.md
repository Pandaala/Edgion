---
id: ARCH-066
status: candidate
kind: behavior-defect
priority: P2
partition: PART-214
analyst: /root
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Keep raw Wasm callout origins out of configuration diagnostics

## Current ownership and evidence

`normalize_callout_origin` in
`edgion-resources/src/resources/edgion_plugins/plugin_configs/wasm.rs:437-444`
formats the entire authored `raw` value into every rejection message. Its
private canonical parser already distinguishes invalid URL, scheme, components,
credentials, missing host and missing port. The quiet runtime wrapper uses the
same parser and discards the error without formatting raw input.

An otherwise valid file-sourced Wasm config with
`calloutAllowlist: ["https://user:private-password@auth.example"]` is correctly
rejected for credentials, but its returned error contains `private-password`.
A query such as `https://auth.example?token=private-token` is rejected by the
components check and similarly copied verbatim into that error. URL parsing
failure is not necessary for either static witness. No real secret or live
incident was supplied or observed.

`WasmConfig::validate` prefixes this returned error with `calloutAllowlist` field context.
`EdgionPlugin::validation_error` and the shared enabled-entry validator preserve
it with the stage/index/plugin context. Controller EdgionPlugins preparse
collects those diagnostics, and `update_status` joins them into an
`Accepted=False`, `Invalid` condition message. The actual `Condition.message`
is an ordinary serialized String without a redaction adapter. Therefore the
rejected field's contents are copied to status, rather than remaining only in
the operator's original configuration. The serial resource processor also
WARNs each returned preparse diagnostic before status construction, so the
normal Controller path is a logging sink for the same raw-bearing String.

Gateway's enabled-entry validation likewise retains the returned detail and
rejects construction. Separately, the public `WasmPlugin::create_request`,
`create_response_filter` and `create_body_filter` wrappers call private
`WasmPlugin::build` without their own validation. That cold builder warns with
`%e` when dropping an invalid allowlist entry, so a caller of those public
wrappers can reach another diagnostic sink. Normal validated publication
rejects the invalid entry before reaching that builder, including Gateway
publication without Controller admission. No request-path tracing, allowlist
bypass, credential forwarding or network call is claimed.

## Bounded improvement and alternative

Make the public diagnostic wrapper render only fixed safe rejection categories,
using the existing private error enum. Do not interpolate raw input or render
parser errors with input-dependent details. Omit query values, userinfo and
path fragments from every rejection category.
The current owning validator already supplies field/stage/index context. Keep
that context and useful category text such as credentials being forbidden.
Controller preparse logging, status construction and the cold constructor
inherit safe details from this one origin; no downstream regex scrubber or
blanket status masking is needed.

Removing only the constructor WARN leaves Controller status and other callers
unsafe. Applying the admin Secret serde adapter at one sink cannot remove the
secret already embedded in a String and would miss other diagnostic surfaces.
Sanitizing every configured URI locally introduces redundant URL/redaction
policy. Fixed categories are sufficient for a rejected exact-origin field.
The original configuration and accepted normalization output stay unchanged.

## Required invariants, knowledge and future verification

Preserve the canonical accepted origin language, case/default-port handling,
rejection of credentials, query, fragment and non-root paths, and acceptance
of a bare `/`. Preserve quiet dispatch behavior and empty-allowlist denial.
Preserve configured plugin-instance runtime isolation,
shared grants, independent integrity pins, bounded artifact loading and existing
egress/client/deadline ownership. Keep current diagnostic precedence and
Controller/Gateway fail-closed behavior. This requires no new tracing, metrics,
response buffer, configuration cache or PluginSession capability.

Original graded redaction rules and `skills/03-coding/01-log-safety.md` prohibit
password logging. The Wasm URL source-identity guard separately removes userinfo,
query and clear-text path from metrics and module-fetch diagnostics; it
explicitly leaves guest-callout URLs as a separate boundary. The original
Wasm callout egress/header tradeoffs permit exact operator-authored destinations
and certain guest headers; they do not authorize echoing credentials in rejected
configuration. The original Wasm tracing guard explicitly permits the cold
builder WARN location; it does not permit secret-bearing content there.
The current source normalizer is already canonical. ARCH038/039
concern live ABI retention and pooled host state, not diagnostic construction.

Existing Wasm schema tests check credential/query rejection and diagnostic/quiet
normalization equivalence, but only examine acceptance, not error redaction.
Other tests cover source URL redaction. Future focused checks should preserve
all existing normalization results while verifying password/token/path markers
are absent from public normalizer, Wasm config and enabled-entry diagnostics
and Controller condition text, including invalid syntax and unsupported schemes.
No Wasmtime guest or outbound network is needed for those pure checks. A narrow
cold-constructor wiring assertion may verify the safe returned error reaches its
existing logger without adding hot-path mock allocations.

Independent verification inspected the original governing rules, canonical
normalizer, current validation/logging/status and constructor call chains, and
the relevant existing test bodies. These supporting reads add no primary file
coverage. This is static source evidence; no tests, builds, network calls or
live logging reproduction were executed.
