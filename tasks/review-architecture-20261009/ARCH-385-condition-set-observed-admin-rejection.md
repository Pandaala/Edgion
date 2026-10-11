---
id: ARCH-385
status: confirmed
kind: behavior-defect
priority: P2
partition: PART-505
analyst: /root/analyze_conversion
baseline: 93910657a31737c17726558f707ccce0e901fd11b669f0e948ef71b37a90f339
---

# Require an observed Admin rejection for malformed ConditionSet probes

`condition_set_structural_invalid` in `edgion-tests/src/client/suites/edgion_plugins/plugin_validation_status/plugin_validation_status.rs:286-343` passes its local-mode assertion whenever `TestContext::apply_yaml` returns an error for each of three malformed ConditionSet documents. The error is treated as proof that the Controller rejected both operators or forbidden nesting. That helper erases distinct outcomes into `Result<(), String>`: client construction, PUT/POST transport failure, authentication or authorization denial, retry failure, HTTP 500 and the intended HTTP 400 can all return `Err`. None of those outcomes is inspected by this case.

A concrete conditional witness is a transport outage affecting these three operations after the earlier static status checks completed: each apply returns an error and this case reports that all malformed shapes were rejected without observing any Controller response. Other cases can detect persistent infrastructure failure, but their independent success does not make this operation's failure schema evidence. No current run failure, admission bypass or whole-suite false pass is claimed.

The real decoder deliberately rejects these shapes before resource persistence. `ConditionSet` is a bounded, deny-unknown-fields union, and the actual `condition_set_schema` tests separately distinguish structural decode rejection from semantic rejection of an empty operator. The registered Kubernetes path already performs server dry-run and requires specific decoding/schema-rejection reasons before setting its verification flag. This finding is limited to the local Admin probe; the Kubernetes proof must remain intact.

Use the existing strict admission-observation policy in `status::helpers::expect_admin_admission_rejected`: authenticated precondition and non-persistence checks, an observed HTTP 400, and failed transport/auth/unexpected-status outcomes. Preserve these authored YAML documents and their YAML request boundary; the existing JSON helper needs only a narrow payload seam if reused. Do not route this negative assertion through a generic apply helper that has already discarded the response status. Keep malformed fixture identities isolated and handle unexpected persistence cleanup as failure, without weakening any of the other nine assertions.

Retaining broad errors plus a prior health check is an inadequate alternative because the operation can fail after that check. A display-string match on `apply_yaml` errors would couple evidence to formatting rather than HTTP outcomes. No production admission, ConditionSet semantics, status projection, transport implementation, logging or metrics changes are needed.

Future verification should make this predicate fail on connection refusal, deadline, 401/403, 500, unexpected success and failed non-persistence checks, while the three real malformed YAML documents produce observed 400s. Preserve existing schema unit tests and local/Kubernetes integration execution. This is distinct from ARCH266's Controller projection, ARCH355's structural-versus-reference evidence and ARCH265's cleanup-result ownership. All primary code and decisive real seams were read; review remained static and read-only.
