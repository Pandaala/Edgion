---
id: ARCH-141
status: candidate
kind: behavior-defect
priority: P2
partition: PART-431
analyst: /root/analyze_outbound
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# Omit the ACME contact email from account-registration logs

AcmeClient::new and AcmeClient::new_with_ca in edgion-controller/src/services/acme/acme_client.rs:94-99 and 126-131 emit successful account-registration INFO events with email=email. The regular service's eligible, leader-admitted account-creation path in services/acme/service.rs:1521-1619 passes EdgionAcme.spec.email to AcmeClient::new when account credentials are absent or empty. The custom-CA constructor is also an actual integration helper used by the Pebble suite. Both constructors use the email to build the CA's mailto contact and then retain it in the log event.

Controller binary-owned logger initialization in cli/mod.rs:79-80 installs the common formatter before execution. edgion-controller/src/config/mod.rs:422-424 and 655-688 selects INFO by default, and edgion-common/src/observe/logs.rs:66-127 installs ordinary JSON/text formatting. Target-level filtering does not redact email fields. With INFO enabled, a successful account creation publishes the configured complete contact email into a normal system-log event. This is a source-level exposure of email PII, not an executed incident, credential/private-key disclosure or authentication bypass.

The original skills/03-coding/01-log-safety.md:5-16 explicitly requires omission or masking of email PII. Remove the optional email field from both success events. Preserve the mailto account contact, account persistence/restoration, registration outcome and existing CA/service/resource context. A partial masked contact should be introduced only if a concrete operational requirement establishes its need; current success events do not require that identity value. No formatter framework, logging filter, new metric or request-path allocation is needed.

The boundary already has one owner: account registration owns the contact protocol value, while binary logging owns output. Omitting the value at these two event sites avoids making ordinary logger output dependent on a field-specific scrubber. Keeping the events while dropping the optional field is smaller than a centralized redaction subsystem; leaving the fields unchanged would retain the explicit policy violation.

Preserve HTTP-01 scope, actor scheduling, reliable notification backpressure, leader/source/generation fences, cancellation, checkpoint reservation, retained-certificate retries, account conflict adoption and malformed/replaced-Secret protection. Preserve canonical accountCredentials restoration, existing EAB owners, forced SSA for dedicated creation and UID/resourceVersion merge-patch for an existing target. None of those accepted complexity or cold credential-copy contracts authorizes logging the contact value. Do not broaden this into DNS-01, a service-phase framework or removal of binary-owned logging.

Original ACME service-shape, scheduler-fencing-recovery, notification gating/backpressure, HTTP-01 serving ownership, canonical account-key, Secret-target identity, leader-job consolidation and credential-copy-retention pages were examined. Their accepted lifecycle and ownership decisions remain intact. This finding is limited to optional event fields and does not repeat a scheduler or Secret-store architecture candidate.

Actual service_test.rs (1373 lines), secret_io_test.rs and their service support bodies were read. They cover scheduling, fencing, cancellation, account conflicts and restoration. The Pebble account-create/restore bodies in edgion-tests/src/client/suites/services/acme/acme.rs verify account identity rather than event privacy. Future verification should inspect both actual success-event field sets and verify that the mailto contact construction remains unchanged. If an event capture is used, assert that a distinctive contact value is absent from structured and formatted output; do not add a network-dependent unit test or a new logging framework solely for this check. The existing Pebble integration remains the protocol wiring check. Keep existing lifecycle and account tests. No builds, tests, CA/API calls or runtime reproduction were executed.
