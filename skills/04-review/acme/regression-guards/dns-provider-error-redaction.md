---
name: dns-provider-error-redaction
description: Review ACME DNS provider errors reaching Controller logs and EdgionAcme failure status.
knowledge_type: regression-guards
---

# DNS provider error redaction

## Conclusion and scope

The Controller's `services/acme/dns_provider.rs` normalizes Cloudflare, AliDNS,
and Pebble challtestsrv diagnostic errors before returning or logging them.
Provider operation names and error categories are static. HTTP status is numeric;
Cloudflare business rejection diagnostics retain at most the first error's code,
only when it is a JSON integer fitting `u16`. Arbitrary `errors`, `Code`, `Message`,
response bodies, record IDs, signed URLs, and dependency error chains are not
diagnostic fields. Configured certificate domains remain locating context.

`CloudflareDnsProvider.api_token` and `AlidnsDnsProvider.access_key_id` /
`access_key_secret` still authenticate the same calls. Their storage, request
signing, DNS propagation polling, and issuance/retry scheduling are unchanged.
`EdgionAcmeStatus.last_failure_reason` still receives alternate Display of the
flow error; normalization drops the unsafe source chain at the provider boundary.
This correction does not claim redaction of every unrelated ACME-client error.

## Rationale and evidence

Authority: [log safety](../../../03-coding/01-log-safety.md), iron rule 1, and
[graded design rules](../../../00-design-rules.md), section A.

The `acme-dns-provider-error-redaction` task originated in the D0036 adjacent
error-surface audit. The confirmed defect was publication of arbitrary external
text, not an observed real credential echo. The existing 256 KiB streaming cap
bounded memory but did not sanitize data.

- Cloudflare list/create failures previously formatted the complete `errors`
  array; delete business failures logged it directly.
- AliDNS create failures formatted both `Code` and `Message`. Its signed query
  also made preserving a `reqwest::Error` URL unsafe.
- Pebble failures formatted raw response bodies. Provider record IDs were also
  logged in success/cleanup branches.
- `execute_dns01_flow` adds creation context; the failure handler in `service.rs`
  logs `%e` and persists `format!("{:#}", e)` through `update_acme_status`.
  `cleanup_txt_records` logs removal errors and continues with later records.

`dns_transport_error` deliberately constructs a new error without a source;
wrapping an unsafe source in fixed anyhow context would still leak through status
or Debug. JSON and streaming-read errors are also replaced, not wrapped. HTTP
rejections are diagnosed from status alone without polling their response body.

## Alternatives and limits

Do not sanitize by removing only known credentials: provider text can contain
unknown identity data, and credentials may be transformed. Do not print unknown
textual provider codes just because they are short. A finite numeric code and
fixed category retain diagnostics without forwarding arbitrary strings.

Successful response payloads are still consumed for zone/record selection. This
is a diagnostic boundary, not a new provider response-schema validator. Cloudflare
delete business failures keep their local warning behavior; service-level cleanup
remains best-effort and non-fatal. AliDNS's existing incomplete cleanup-response
handling is deferred in [the cleanup response task](../../../../tasks/todo/acme-alidns-cleanup-response-validation.md).

## Verification and re-evaluation

Fresh verification for this correction:

- `cargo test -p edgion-controller --lib services::acme::`: 29 passed.
- `cargo fmt --all` and `git diff --check`: passed.
- `cargo check --workspace --all-targets`: passed.
- `cargo clippy --workspace --all-targets`: passed with existing workspace warnings.
  A targeted Controller JSON-diagnostic check also passed; the changed DNS file
  and added cleanup test emitted no warnings. The three `manual_let_else`
  suggestions in unchanged `service.rs` production code predate this correction.
- `validate_agent_docs.py`, `check_ssa_force.py`, `check_metrics_inventory.py`,
  and `check_pingora_source_policy.py` under `cicd/checks/`: passed.
- Independent read-only code and knowledge review: LGTM.

Targeted tests cover provider rejections, numeric-code bounds,
successful/malformed JSON, HTTP rejection without
body reads, exact/excess body limits, unsafe transport/source chains, status
serialization, and real cleanup warning formatting/continuation. No live
Cloudflare/AliDNS call or Kubernetes deployment is claimed by these tests.

Re-evaluate whenever a provider, output field, error wrapper, or diagnostic sink
changes. Audit all create, lookup, and removal branches, including success-path
logging of response-controlled fields. Historical service-shape acceptance is not
an exemption from this security boundary.

## Resolution references

- `acme-dns-provider-error-redaction` — fixed; confirmed security defect corrected
  at the DNS provider diagnostic boundary.
- `D0036` — [original ownership audit and limits](../../security/review-evidence/credential-copy-ownership.md).
