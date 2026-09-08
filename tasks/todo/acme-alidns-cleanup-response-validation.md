# Validate AliDNS Cleanup Responses

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-09-08 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |
| Source | ACME DNS provider error-redaction follow-up |

## AI guide summary

Make AliDNS TXT-record cleanup reject provider failures and malformed success responses instead of treating them as no records or successful deletion, while preserving bounded diagnostics and the Controller's best-effort cleanup behavior.

## Requirements

- Route both `DescribeDomainRecords` and `DeleteDomainRecord` responses through the canonical bounded response reader and AliDNS response classifier, including HTTP status, streaming body-size, JSON, business-rejection, and operation-specific success-shape validation.
- Require the describe response to contain a valid `DomainRecords.Record` array before record selection; do not collapse a missing or malformed result into an empty record set.
- Require the delete response to contain the minimal documented acknowledgement shape, including non-empty string `RequestId` and `RecordId` fields, before accepting deletion as successful.
- Return only fixed, safe error classifications. Do not expose provider `Code` or `Message`, record IDs, signed URLs, response bodies, credentials, challenge values, or dependency error chains through errors, status, or logs.
- Preserve the shared outbound HTTP client, redirect denial, connect and request deadlines, AliDNS signing, domain/RR construction, existing record-selection behavior, and request ordering.
- Preserve Controller-owned cleanup semantics: a removal failure is non-fatal, emits the existing single bounded warning at the service boundary, and does not prevent cleanup attempts for later records.
- Do not add provider-local warning logs, parallel observability, or new metrics.
- Add deterministic focused unit tests for valid responses, HTTP rejection, business rejection, malformed JSON and success shapes, declared and streamed body limits, body-read failure, and cleanup warning/continuation behavior.
- Do not modify existing integration-test files; record a separate integration gap only if focused unit coverage cannot exercise a required cross-process contract.

## Scope

### In scope

- AliDNS describe/delete response consumption and validation in `edgion-controller/src/services/acme/dns_provider.rs`.
- Existing ACME cleanup service tests only where needed to prove non-fatal warning ownership and continuation.
- Focused in-crate test support required to inject deterministic provider responses without weakening production transport behavior.

### Out of scope

- AliDNS request signing, TXT-record matching or deletion-selection policy, credential loading, propagation polling, and issuance scheduling.
- Cloudflare, Pebble challtestsrv, or other ACME provider behavior.
- New logs, metrics, public API fields, configuration, or integration-test scenarios.
- Reworking cleanup from best-effort to fatal or retrying failed provider operations in place.

## Acceptance criteria

- Describe HTTP failures, AliDNS business rejections, invalid JSON, and missing or malformed `DomainRecords.Record` fail with bounded static classifications rather than appearing as no matching records.
- Delete responses are bounded and validated for HTTP success, valid JSON, no business rejection, and the required non-empty acknowledgement fields before the deletion is accepted.
- Error and warning output contains none of the provider-controlled or sensitive values excluded above, including through alternate `anyhow` display or source chains.
- One failed record removal produces only the Controller service's existing safe non-fatal warning and cleanup continues with subsequent records.
- Valid describe/delete responses retain current record selection, signing, deadlines, and cleanup ordering.
- Deterministic focused unit tests cover the response and continuation boundaries; existing integration tests remain unchanged.
- Controller format, targeted tests, workspace check/Clippy, documentation validation, and repository static guards pass.
