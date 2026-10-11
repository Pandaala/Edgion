---
id: ARCH-108
status: candidate
kind: architecture-debt
priority: P3
partition: PART-407
analyst: /root/verify_ip_union
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-108: Validate request-template Secret status without collecting discarded values

## Current ownership and concrete cost

request_secrets.rs shares resolve_request_secrets_core between material acquisition and readonly status. request_secret_errors_readonly returns only the errors, but invokes the core that builds the full BTreeMap<String, String> and discards it.

For every successful reference, read_secret_key clones Secret.data bytes into String::from_utf8, or clones the string_data value. The core also copies the exact runtime map key and inserts the owned value into a BTreeMap. These values are retained together until the status walk finishes even though status never consumes them.

This runs through the actual EdgionPlugins status path: EdgionPluginsHandler::update_status invokes bound_credential_status_errors, whose ForwardAuth/Guardrail/AiGuard branch calls request_secret_errors_readonly with actual owning template loans. A template referencing a valid 64 KiB Secret value therefore copies that complete selected value and constructs a map entry solely to discard it. Several referenced values extend the same unnecessary material projection.

The source establishes these copies and the discarded map, not measured latency, RSS or a data-plane throughput regression. SecretStore::get already returns an owned Secret clone; changing that store API is outside this proposal.

## Bounded improvement

Keep one private extraction rule and one owning-token acquisition/error loop. Make successful key extraction borrow &str from the fetched Secret: validate data bytes with str::from_utf8, otherwise borrow string_data using the same precedence.

Allow the existing core to collect values only for callers that need the map. The status-only entry validates the same selected values and accumulates the same errors without copying successful values or inserting map entries. Material acquisition converts a successful borrowed value to an owned String when inserting into its required runtime map.

Dependency registration and value collection are independent decisions. resolve_request_secrets_readonly is used by route_auth::resolve to retain real runtime values after root recovery receipts were installed separately; it must continue collecting values despite disabling registration. Do not use the registration flag as a substitute for the output projection.

No public decoder framework, new Secret store, cached status material, extra request-path allocation, or alternate template parser is required.

## Required semantics and risks

Preserve actual TemplateSecretReference ownership, exact map/data keys, ordered traversal and error order, malformed empty-key handling, identity-before-acquisition, canonical Grant checks, and readonly no-registration behavior.

Within a fetched Secret, data has priority. A present invalid UTF-8 data value remains an error even when string_data contains a valid fallback for that key. A missing data key may fall back to string_data; present empty values remain successful. Missing Secret, denied reference, missing key and invalid UTF-8 classifications and diagnostics remain unchanged.

The installed Rust source shows String::from_utf8 calling str::from_utf8 and FromUtf8Error::Display forwarding its Utf8Error. Borrowed validation can retain the existing UTF-8 diagnostic format without retaining a duplicate byte buffer. This is source evidence, not execution of a specific compiler build.

Keep parse-time values for managed HTTP plugins, LinkSys Webhook and internal HTTPRoute authentication. Their conf_sync/runtime ownership, readiness, sensitive-header provenance, TLS behavior and independent credential copies remain unchanged. Do not log successful values or add metrics.

## Original knowledge and deduplication

request-template-policy-unification.md requires readonly parity for present-empty values, string_data fallback, non-UTF-8 and missing-key errors, with no dependency registration. It does not require collecting successful material during an errors-only projection.

The pending resource-reference-contract template slice requires actual owning tokens and the same read_secret_key behavior; this proposal preserves that behavior while narrowing its output ownership. The credential-copy-retention decision accepts necessary independent owners, not an unused status map.

ARCH-104 concerns OAuth2 dial-time egress enforcement and ARCH-105 concerns source-qualified manager results. Neither owns this local template-status projection. Existing projection findings in other mechanisms require no shared decoder or allocation framework here.

## Future verification

Retain the existing request_secrets tests for real bound templates, missing and rotated Secrets, malformed/stale owners, Grant recovery/revocation, lexical keys, empty values and string_data fallback. Retain actual ForwardAuth status tests and managed material acquisition controls.

Add focused parity coverage for invalid UTF-8 data shadowing valid string_data and for present-empty data shadowing a nonempty fallback. Compare errors and order between material and errors-only projections. Verify that readonly material acquisition still returns its map and registers no dependencies.

Source inspection must show that the errors-only path validates borrowed successful values without building a populated value map. No benchmark or test execution is claimed by this review.
