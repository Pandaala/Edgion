---
id: ARCH-178
status: candidate
kind: behavior-defect
priority: P2
partition: PART-234
analyst: /root/analyze_conversion
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-178 — Give CLI connection diagnostics one safe URL projection

## Current ownership and evidence

`EdgionClient` in `edgion-cli/src/cli/client.rs:65-120` retains the operator-supplied server address as an unchanged String. `send_request` passes the original constructed URL to `format_request_error` (`207-217`). That formatter prints it in both `Request to ... failed` and the suggested `curl -v` command (`366-406`). The authentication-status probe separately copies the original base address into its network-error context (`139-145`).

These are real command paths. `commands/get.rs:50-63` repeats the original base address in connection hints. `commands/login.rs:36-58` prints the original status URL on a failed request or non-successful response. The CLI accepts `--server` as a String; there is no admission step rejecting URL userinfo.

An accepted URL such as `http://operator:PRIVATE_PASSWORD_SENTINEL@127.0.0.1:1` therefore causes the password to be copied into diagnostics if the connection fails. An explicit static token bypasses the authentication probe, but the first request still reaches `send_request`; without a token, the probe has its own raw-address error path. Login's best-effort probe similarly prints the address on failure. These are static source witnesses, not executed requests or evidence of a production incident.

The pinned reqwest 0.12.28 implementation does not prevent this exposure. `RequestBuilder::new` calls `extract_authority`, which decodes userinfo, removes it from reqwest's parsed URL and installs Basic authentication (`src/async_impl/request.rs:168-183,579-605`). It does not mutate the CLI's original String. Consequently, even a correctly cleaned SDK request URL leaves the CLI's explicit diagnostic copies unsafe. Independently, reqwest `Error::Display` includes any attached URL (`src/error.rs:228-270`); `Error::without_url` removes that attachment while retaining its kind and source (`86-91`).

## Root cause and bounded improvement

The CLI's HTTP owner exposes its transport address directly to diagnostic consumers without defining a safe display identity. Correct this boundary in `client.rs` with one small CLI-owned diagnostic URL projection, reused by its request formatter, authentication-probe context and the supporting get/login hints.

Render a readable endpoint origin containing scheme, host and port, without userinfo, path, query or fragment. Use a fixed safe label when the input cannot produce an appropriate parsed HTTP(S) origin; do not echo malformed input as a fallback. Keep the actual stored address and constructed request URL unchanged. Make connection suggestions use the safe endpoint identity and avoid presenting a credential-bearing curl command.

Before SDK errors enter the same printable request messages or authentication-probe error chain, remove their attached URL with the existing `without_url` mechanism. Keep useful request/timeout/connect classification, target-specific hints and numeric HTTP status. The login probe already discards its transport error; it needs the common safe address projection for its explicit warning text.

Sanitizing only reqwest errors leaves the demonstrated original-String copies exposed. Fixing each command independently would duplicate URL policy and leave the client error producer unsafe. Rejecting userinfo or rewriting the transport URL would change accepted request/authentication behavior and is unnecessary for this diagnostic correction. A generic logging or redaction framework is unnecessary.

## Required invariants and verification

Preserve endpoint selection, API route construction, pagination and namespace behavior, bearer-token precedence, reqwest's existing authentication behavior, canonical outbound-client construction, corporate proxy support, redirect policy, timeouts and the existing per-host bound of two idle connections in EdgionClient. Preserve login's best-effort probe and token persistence, and preserve intended resource output and explicitly requested token-generation output. This finding does not attempt to eliminate credentials already supplied in process arguments.

Existing `ensure_auth_tests.rs` bodies cover authentication/readiness behavior, static bearer storage and sensitive Authorization-header Debug handling. Existing get and login tests cover their current arguments and namespace helper behavior. None of those inspected bodies establishes safe URL diagnostics.

Future focused tests should exercise the pure diagnostic projection with userinfo, percent-encoded userinfo, query/fragment markers, IPv6 with an explicit port and malformed input. Verify ordinary origins remain useful and secret markers never appear. A locally constructed reqwest error with an attached distinctive URL can verify URL-free error formatting without a network request. Narrow command/probe wiring checks should establish that all named consumers use the projection while preserving status, failure outcome and login token-save semantics. Do not introduce secret-bearing mock state, new logging, transport policy or a general sanitization framework.

## Original decisions and scope limits

The graded redaction rules and `skills/03-coding/01-log-safety.md` forbid password disclosure and explicitly identify reqwest errors as a full-URL risk. Their origin-level correction principle preserves useful diagnostics rather than masking entire errors. The original CLI overview deliberately provides readable endpoint and failure hints; it does not authorize printing embedded credentials.

The Wasm URL-source guard separately removes userinfo, path, query and fragment from observability identities while preserving the actual fetch URL. The TLS diagnostic guard preserves ordinary useful address/error information and requires evidence before masking additional fragments. These decisions support a narrow diagnostic projection, not blanket suppression of CLI output.

ARCH-066 concerns rejected Wasm callout configuration; ARCH-076 concerns HTTP DNS SDK-error stringification. Neither owns the CLI's explicit raw-address strings. ARCH-176 owns cache namespace pagination, and ARCH-177 owns directory apply outcomes. Their command locations are supporting seams here, not additional findings for those files. No existing finding or original decision was found accepting credential-bearing CLI connection diagnostics.

All evidence is static. No requests, builds or tests were executed; deployment frequency and impact were not measured. The current userinfo witness does not depend on signed-query endpoint support, an authentication bypass or disclosure on the wire.
