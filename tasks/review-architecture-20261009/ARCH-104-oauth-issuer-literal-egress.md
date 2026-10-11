---
id: ARCH-104
status: candidate
kind: behavior-defect
priority: P2
partition: PART-434
analyst: /root
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-104: Apply CredentialSource egress policy to literal issuer hosts

## Current ownership and supported bypass

OAuth2Provider::new derives the token endpoint and egress policy from one actual CredentialSource owner. It installs Common EgressResolver in the canonical HTTP client, with block_private copied from source.egress. acquire posts the parsed endpoint with the selected sensitive bootstrap Authorization header. The provider validates HTTPS, host presence, length and absence of userinfo/query/fragment, but does not classify literal hosts.

The installed DNS resolver does not cover literal URL addresses. The pinned reqwest0.12.28 client creates hyper-util0.1.20 HttpConnector with the custom resolver. Its call_async parses IPv4/IPv6 hosts into socket addresses and connects directly, before the branch invoking that resolver. Common's existing HTTPDNS fetcher explicitly handles this same boundary by classifying the normalized URL host before sending that exact parsed URL.

A valid CredentialSource tokenEndpoint such as https://10.0.0.1/token with the default egress.blockPrivate=true therefore reaches the connection path without the configured private-address decision. HTTPS loopback, link-local and unspecified literals similarly bypass the canonical always-blocked categories in ordinary production mode. Bracketed/mapped IPv6 follows the connector's literal branch. A hostname resolving to the same address would pass through the installed resolver and be filtered.

This is an operator-configured policy gap, not evidence of unauthenticated arbitrary issuer selection, successful token acquisition, certificate acceptance, cloud metadata access or an incident. TLS may fail after dialing. The local controller owns the issuer call; this is not main Gateway response processing.

## Bounded improvement

At provider preparation, inspect the host of the already parsed immutable token_endpoint. For IPv4/IPv6 literals, use Common's existing egress filtering policy with the same source block_private value and shared testing-mode exception. Reject an empty admitted set through the existing fixed-vocabulary permanent provider error before issuer IO. Keep that exact parsed URL for later requests; do not classify one string and send another.

Reuse filter_resolved_addrs or an existing equivalent canonical policy entry rather than hand-maintaining a second list of ranges. Its classifier already normalizes mapped IPv4. This is cold provider preparation and needs no request-path state. Hostnames must retain the existing at-dial EgressResolver, which returns the actual admitted addresses to the connector; a one-time DNS lookup would introduce a rebinding gap.

Preserve private/ULA literals when blockPrivate=false, public literals, configured canonical DNS, disabled proxy environment, denied redirects, HTTPS/native versus active proved TLS policy, captured owner/defaults, source/Secret/Lease fences, request timeouts, response cap and typed retry/invalid_client behavior. Do not add an issuer loopback override, configurable deny-list, alternate resolver, client factory or raw endpoint diagnostic. This correction grants no new product scope or global egress-policy redesign.

## Original decisions and meaningful verification

The graded rules require canonical egress checks for config-influenced outbound destinations. CredentialSource typed config defaults blockPrivate to true and its feature example exposes that setting. The original HTTPDNS endpoint guard explicitly records literal hosts bypassing reqwest DNS, but its per-resolver loopback allowance and always-allowed private service policy are HTTPDNS-specific; they must not be copied into OAuth2.

The current OAuth2 provider tests cover bootstrap bounds/redaction, Basic encoding, response classification, RetryAfter and actual TLS owner/material/default captures. None exercise the literal issuer address policy. Add focused actual-provider preparation checks for public/private IPv4, bracketed IPv6, mapped loopback/private/link-local and unspecified addresses, explicit blockPrivate=false, and production-mode versus explicit shared integration-test exception. Verify blocked literals fail before issuer IO and hostname resolution still uses the configured canonical resolver. Preserve existing typed permanent/transient and previous-bootstrap semantics. The already existing HTTPDNS transport tests are supporting precedent, not fresh verification of this provider.

Current finding/task searches found no duplicate literal-issuer policy correction. Analysis used pinned local dependency source and actual current tests; no tests, builds, network calls or runtime reproduction were executed. No performance, frequency, heap bound, immediate revocation or successful credential-exfiltration claim is made.
