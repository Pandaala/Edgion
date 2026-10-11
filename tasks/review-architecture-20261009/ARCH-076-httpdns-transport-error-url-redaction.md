---
id: ARCH-076
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-224
analyst: /root/analyze_conversion
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-076 — Strip configured HTTP DNS URLs before transport errors become diagnostics

## Current ownership and evidence

`ReqwestHttpDnsFetcher::get` in `edgion-common/src/dns/http_fetcher.rs:123-138` owns the bounded HTTP DNS exchange. Both `.send()` and `.chunk()` errors are converted directly with `DnsError::Http(e.to_string())`. The configured URL is passed to the canonical prepared reqwest client; `HttpDnsResolverConfig.url_template` intentionally supports account-specific and signed endpoints (`edgion-resources/src/resources/link_sys/httpdns.rs:101-114,187-201`).

The pinned reqwest 0.12.28 `PendingRequest::poll` attaches the request URL to total/read timeout errors and connection errors (`src/async_impl/client.rs:3044-3076`). Its `Error::Display` prints the complete attached URL, including query values (`src/error.rs:227-271`). Thus an allowed template such as `https://resolver.example/d?host={domain}&signature=PRIVATE_SIGNATURE_SENTINEL`, with `fallback: none`, can produce a DNS error string containing the signing value when sending fails. `httpdns::resolve_http_dns_with` preserves the primary error when fallback also fails (`httpdns.rs:198-203`), and `DnsError::Display` preserves that text (`dns/mod.rs:248`). Controller background resolution logs it with `error = %e` (`edgion-controller/src/services/dns_resolver/service.rs:332-343`); Gateway ExternalName resolution stores it in the current upstream's error record (`edgion-gateway/src/backends/mod.rs:297-304`). This is a source-proven diagnostic exposure, not a claim of a production incident or an unauthenticated arbitrary-URL attack.

The demonstrated URL attachment is in the send path. The pinned `Response::chunk` currently constructs a decode error without attaching its response URL (`src/async_impl/response.rs:315-330`), so current body-chunk failures are not claimed as a separately proven URL leak.

## Root cause and bounded improvement

The transport-to-domain-error boundary stringifies a URL-bearing SDK error before removing configuration-sensitive context. Apply reqwest's existing `Error::without_url()` before stringification at both HTTP DNS error mappings. The send mapping closes the demonstrated exposure; the chunk mapping uses the same safe projection if a URL-bearing error reaches it after dependency changes. This does not require a new sanitizer, a shared error framework, or changes to the consumers' logging.

`without_url` only clears the error's attached URL (`reqwest/src/error.rs:86-91`); it retains the SDK error kind and source. The resulting display retains the existing request/body/decode class, rather than adding timeout details that current `to_string()` does not provide. Keep HTTP status failures' existing numeric status text and the existing fixed URL-parse, endpoint-policy and body-limit errors. Gateway GeoIP transport already uses this SDK mechanism before formatting send and chunk errors (`edgion-gateway/src/geoip/updater.rs:557-565,599-606`).

The concrete change scenario is enabling a signed HTTP DNS endpoint and then encountering a transport failure: the sent request stays identical, but its signing URL no longer enters durable error strings. The benefit is origin-level redaction with no request-time URL parsing beyond existing work. Query-key filtering is weaker because operator credentials may appear in arbitrary key names or paths; replacing every DNS error with a generic label would lose useful existing failure classes and unnecessarily expand scope.

## Required invariants and verification

Preserve signed URLs exactly as sent, preset/explicit-template precedence, the actual-owner prepared TLS client and source/inheritance proof, System bootstrap resolution, admitted dial addresses, no environment proxy, denied redirects, explicit loopback exception and link-local/unspecified denial. Preserve deadlines, cancellation, native pool limits, streaming enforcement of the configured response cap no greater than 64 KiB, the configured fallback and primary-error precedence. Do not add tracing to Common's query path or duplicate Gateway request observability. This separate bounded exchange creates no response BodyStorage or general full-response API.

Future tests should exercise a transport failure with a distinctive signing query, assert that the URL and signing value do not appear in the returned DNS error or its formatted consumer wrapper, and verify that the existing error class and fallback behavior remain. Cover the body-error projection as well, without claiming the pinned chunk path currently attaches a URL. Existing transport tests cover endpoint policy, redirects, timeout and byte caps, but the inspected test scopes do not establish signed-URL error redaction. No tests or network exchanges were run during this review.

## Original decisions and scope limits

Original log-safety rules explicitly identify reqwest errors as a full-URL risk and require redacting proven sensitive fragments at their origin (`skills/03-coding/01-log-safety.md`). The HTTP DNS egress and prepared-runtime guards require canonical dialing/client ownership, configured fallback and reusable generations; none accepts credential-bearing diagnostics (`skills/04-review/ssrf/regression-guards/httpdns-endpoint-egress.md`; `skills/04-review/link-sys/regression-guards/httpdns-prepared-runtime.md`). This cause is distinct from ARCH-066's Wasm cold configuration diagnostic projection and ARCH-077's immutable response preparation. It does not retire signed endpoint support, redesign DNS cache/TTL policy, introduce DNS-01, or alter the supported Gateway TLS backend. Static evidence establishes the exposure path and the SDK projection's feasibility; deployment frequency and impact were not measured.
