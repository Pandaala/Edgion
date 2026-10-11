---
id: ARCH-080
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-392
analyst: /root
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

# ARCH-080: Resolve the Controller's explicit plaintext override before auto-CA lifecycle setup

## Current ownership and concrete witness

The current original Controller-Gateway link overview, skills/01-architecture/03-controller-gateway-link/00-overview.md:304-305, specifies that an explicitly active skip_tls set overrides auto_ca. The fail-close decision record preserves this deliberate emergency escape hatch and its loud diagnostics. Gateway ConfigSyncClient::new follows that contract: an active skip_tls entry selects an HTTP channel, reads no certificate files, warns when overriding auto_ca and marks the existing SKIP_TLS state. The actual Gateway unit body auto_ca_with_skip_tls_is_allowed_escape_hatch asserts this combination remains usable.

Controller startup diverges. In edgion-controller/src/cli/mod.rs, run_after_logging at586 tests only conf_sync_security.auto_ca before rotation::init. start_services at246 validates rotation under the same raw flag, then at274 chooses auto-CA provisioning before inspecting resolve_active_tls().skip_tls. It requests a Kubernetes client, provisions/adopts the root and Gateway leaf, installs an in-memory serving identity and starts per-replica identity reconciliation. ConfigSyncGrpcServer::serve_with_shutdown selects the TLS accept loop whenever that identity handle is Some; its existing plaintext guard is never reached.

A canonical authored configuration with auto_ca:true and one selected certs entry with skip_tls:true and empty certificate paths passes shared validate and ensure_transport_configured. If auto-CA provisioning succeeds, Controller serves TLS while Gateway's same explicit choice constructs plaintext; their transports disagree. If Kubernetes access or provisioning fails, Controller cannot use the promised escape hatch. An invalid Controller rotation schedule can also reject this supposedly overridden path before serving. These are static source witnesses, not an executed incident or an assertion that current shipped defaults are plaintext.

## Structural cause and change scenario

The active transport choice and the raw provisioning flag independently control several startup phases. Selecting plaintext at the final serving call would leave early rotation initialization and possible Secret-changing leader work active. In Kubernetes center.start_leader_jobs, start_conf_sync_rotation is always called and starts a reconciler whenever the earlier PARAMS initialization exists. That same function also sets the leadership flag used by manual rotation admission, so simply skipping the entire leadership callback is an incorrect repair.

This is avoidable policy scattering inside the Controller startup owner. The shared authored selector already identifies the active entry. One validated effective decision can govern auto-CA initialization, schedule validation, provisioning, identity reconciliation and serving together. The source file's size or number of startup arguments is not the finding.

## Bounded improvement and alternative

Resolve the validated selected transport before auto-CA setup and derive effective auto-CA only when auto_ca is enabled and the selected entry does not explicitly bypass TLS. Carry or reuse that same decision across run_after_logging and start_services. Explicit plaintext must leave rotation PARAMS uninitialized, skip auto-CA provisioning and identity/expiry watchers, pass no serving TLS handle, and retain the existing loud plaintext gate. Preserve the higher-severity override warning required by the original decision without adding another metric family or request-path logging.

Keep center.start_leader_jobs invoking the existing rotation lifecycle signal: without initialized PARAMS it retains the existing no-worker behavior while preserving leadership state. Bare auto_ca with no file selection must still provision the Controller's in-memory identity; it must not be misclassified as omitted plaintext. Gateway's requirement for mounted material in its bare auto_ca case remains distinct. An inactive skip_tls entry must never override the active TLS set. Resolve invalid selectors and path errors through the existing validation rather than treating unresolved selection as permission to downgrade.

A private effective-mode value or a narrowly shared selector helper is sufficient. A generalized transport framework, auto-CA service rewrite, removal of the explicit escape hatch, or requirement to disable auto_ca manually would either add unnecessary machinery or change the accepted contract. If the product wishes to withdraw the combined escape-hatch configuration instead, that requires an explicit policy change across the current source, original decision and Gateway tests; current Controller behavior alone does not establish that authorization.

## Preserved boundaries and future verification

Preserve omitted-security fail-close, ordinary file mTLS, Controller bare auto-CA, immutable selected-path behavior, rotation init-before-leadership on effective auto-CA, independent leader rotation versus per-replica identity sync, live identity validation/expiry, cancellation and trust-root session revocation. Federation still requires real mTLS and must never inherit this emergency conf_sync choice. Native Gateway BoringSSL policy, request BodyStorage, streaming response bounds and serial resource workers are unrelated and unchanged.

Existing Controller plaintext tests exercise accept_skip_tls_or_refuse directly or call serve_with_shutdown with no handle; they do not cover the CLI's raw auto_ca branch. Existing Gateway tests explicitly exercise the combined override. Future focused coverage must therefore verify startup selection and absence of auto-CA initialization/provisioning work for the selected override, including invalid rotation inputs and inactive bypass entries, and retain bare auto-CA, real TLS rotation and omitted-security cases. Test the production decision seam rather than adding a generic client abstraction for mocks. No tests or live services were run here.

## Original knowledge, deduplication and limits

Read the original startup/shutdown and link-overview architecture, fail-close decision, shared security configuration, actual startup/provision/rotation/leadership/serving consumers and relevant test bodies. Searches of current review knowledge and pending tasks found no separate fix for this precedence mismatch. ARCH-074 explicitly excluded this Controller-owned seam from removal of the legacy tls representation; the witness here uses canonical certs and survives that migration. ARCH-078 concerns metric lifetime after an authorized state is selected, not which transport is selected.

The current startup overview's detached-lifecycle error description is stale relative to the observed deadman shutdown code and is recorded separately as documentation follow-up. It cannot authorize this transport mismatch. Supporting docs and code remain read-only. This finding concerns an explicitly configured emergency path, not implicit exposure, remote attack reachability, measured recovery time or a general auto-CA correctness verdict.
