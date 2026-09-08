---
name: dns-cleanup-response-validation
description: Review AliDNS bounded pagination, exact challenge selection, and cleanup response validation.
knowledge_type: regression-guards
---

# DNS cleanup selection and response validation

## Conclusion and scope

The Controller's `AlidnsDnsProvider::remove_txt_record` delegates response handling
to `cleanup_alidns_records` in `services/acme/dns_provider.rs`. Lookup and each
delete response pass through the existing HTTP status, 256 KiB streaming body
limit, JSON parser, and AliDNS `Code` rejection boundary. An HTTP rejection is
reported without reading its body. Errors use the existing
[normalized diagnostic boundary](dns-provider-error-redaction.md).

The lookup must contain a `DomainRecords.Record` array and integer `PageNumber`,
`PageSize`, and `TotalCount` metadata. All entries must have nonempty string
`RecordId` values and string `RR`, `Type`, and `Value` fields. Selection compares
RR and TXT type case-insensitively and the supplied challenge digest byte-for-byte.
An unrelated RR or another value at the same name is never a deletion candidate.
A valid zero-total empty page or a complete lookup with no matches is a successful
no-op. A delete response must return the requested
`RecordId`. Provider text and unexpected IDs never enter errors or logs.
The provider's credential fields, signing, endpoint, and public trait are unchanged.

The helper requests pages serially and validates every page before any deletion.
It checks stable total/page-size metadata, expected page numbers and lengths, and
unique IDs across all candidates (including nonmatches). Page arithmetic is
checked; malformed, empty nonterminal, repeated, or inconsistent pages fail before
deletion. Requests specify 100 entries per page; a smaller positive returned page
size is supported consistently. Hard bounds are 100 pages, 4,096 candidates, and
256 KiB of retained ID strings, counting both the seen-ID and selected-ID copies.
Excessive totals or retained data are rejected, never silently truncated.

Individual delete failures do not prevent attempts for later selected records.
The helper retains only the first normalized error and returns it after the loop.
The service's existing 120-second `DNS_OP_TIMEOUT` still bounds each provider
cleanup operation, including lookup and deletions. The outer
`cleanup_txt_records` logs a non-fatal warning and proceeds to later domain/value
pairs; `execute_dns01_flow` still returns the original certificate result.

## Rationale and evidence

The source task `acme-alidns-cleanup-response-validation` identified two unchecked
branches: lookup business rejections fell through a missing-array `if let` to
`Ok(())`, and deletion discarded the entire HTTP response. The former bounded
JSON reader already rejected HTTP failures and oversized lookup responses but
could not catch a business error without its consumer checking `Code`.

The AliDNS API documents the lookup's `DomainRecords.Record` array and the delete
response's record ID:

- [DescribeDomainRecords](https://www.alibabacloud.com/help/en/dns/api-alidns-2015-01-09-describedomainrecords)
- [DeleteDomainRecord](https://help.aliyun.com/en/dns/api-alidns-2015-01-09-deletedomainrecord)

This is a corrected provider response boundary, not a new global failure policy.
The canonical [log safety rules](../../../03-coding/01-log-safety.md) still govern
diagnostics; the [service shape tradeoff](../intentional-design/acme-service-shape.md)
never accepted swallowed external errors.

The source task `acme-alidns-cleanup-record-selection` identified the remaining
ownership and completeness defects: `remove_txt_record` ignored `value`, while
the helper deleted every returned ID from the first keyword-search page. The
service already retains each `PendingDnsChallenge.domain` / `digest` as a
`created_records` pair and forwards it to cleanup. The correction consumes that
digest at the provider boundary without changing the trait or creation flow.
AliDNS's default RR filter is fuzzy; the type filter is exact and case-insensitive.
The query remains a candidate filter, never proof of ownership.

## Alternatives and limits

Stopping at the first delete error would leave later returned records unattempted
even when they could be removed. Continuing serially within the existing deadline
improves best-effort cleanup without adding retries, tasks, or an error list.
Only the first failure is diagnosed; a later operation timeout can instead yield
the service's timeout warning. Cleanup is not guaranteed to finish every record.

Listing before deletion avoids offsets shifting because of this cleanup's own
deletions. It does not provide an AliDNS snapshot or an atomic compare-and-delete:
external inserts, deletes, or record updates may remain undetected when totals
and IDs stay consistent. Concurrently edited records can change after selection.
Identical RR/type/digest records are indistinguishable by the existing trait and
are all selected; this does not establish which actor created each copy. Retaining
creation IDs would require a separate lifecycle/state design and is outside this
local correction. Existing root-domain/RR derivation and Cloudflare/Pebble cleanup
semantics are unchanged. Large fuzzy result sets exceeding a bound fail safely
and may require operator cleanup; no new retry or provider-local log is added.

## Verification and re-evaluation

Historical response-validation verification (not a fresh selection-fix run):
`cargo test -p edgion-controller --lib services::acme::` passed: 34 tests,
including five new inline `alidns_cleanup_*` tests. The first run exposed two
incorrect oversized-response fixtures; using an actually oversized in-memory
body corrected the fixtures, and the rerun passed. Tests exercise the production
response-processing loop using in-memory responses and do not contact AliDNS.
Coverage includes empty/multiple records, HTTP/business rejections, malformed
JSON/list/IDs, exact/excess byte limits, body/transport failures, continued
deletions, first-error retention, and error-chain redaction. Existing service
cleanup tests cover non-fatal warning formatting and continuation across
domain/value pairs.

Historical closure checks for the response-validation correction:

- `cargo fmt --all` and `git diff --check`: passed.
- `cargo check --workspace --all-targets`: passed.
- `cargo clippy --workspace --all-targets`: passed with warnings in unchanged
  workspace code; no diagnostics referenced `dns_provider.rs`.
- `validate_agent_docs.py`, `check_ssa_force.py`, `check_metrics_inventory.py`,
  and `check_pingora_source_policy.py` under `cicd/checks/`: passed.
- Independent read-only code and knowledge review: LGTM. The follow-up task's
  keyword-matching description was narrowed to distinguish fuzzy RR matching
  from exact type matching using the official parameter table.

Re-evaluate if AliDNS response fields, selection/pagination, diagnostic wrappers,
per-request or operation deadlines, or the service cleanup policy change. No live
provider failure or Kubernetes issuance run is claimed.

The selection correction adds deterministic in-memory tests for mixed ownership,
all-pages-before-delete behavior with a mutable listing, malformed metadata and
fields, changing/repeated/nonprogressing pages, later-page response failures,
and inclusive/excess page, record, and retained-ID bounds. Existing response and
delete-continuation tests now pass through the paginated production helper.
Current selection-fix verification:

- `cargo test -p edgion-controller --lib services::acme::`: 38 passed.
- `cargo fmt --all` and `git diff --check`: passed.
- `cargo check --workspace --all-targets`: passed.
- `cargo clippy --workspace --all-targets`: passed with warnings in unchanged
  workspace code; none referenced `dns_provider.rs`.
- `validate_agent_docs.py`, `check_ssa_force.py`, `check_metrics_inventory.py`,
  and `check_pingora_source_policy.py` under `cicd/checks/`: passed.
- Independent read-only implementation and knowledge review: LGTM.

No live AliDNS deletion or full integration-test run was performed.

## Resolution references

- `acme-alidns-cleanup-response-validation` — fixed; confirmed correctness defect
  corrected at the AliDNS cleanup response boundary.
- `acme-alidns-cleanup-record-selection` — fixed; confirmed correctness defect; correction
  limits deletion to matching RR/type/digest after a complete bounded lookup.
