# D0155 — Complete RequestMirror Standard Validation and Service Status

- Status: pending after B0304 independent review.
- Canonical standalone task: [complete-request-mirror-standard-validation-and-service-status.md](../../../todo/complete-request-mirror-standard-validation-and-service-status.md).
- RequestMirror sampling and core Service reference fields are not validated consistently across preserved EdgionPlugins configuration and non-Kubernetes HTTPRoute input. Missing Services also lack complete status and dependency-requeue lifecycle handling.
- The correction spans resources and Controller publication/status paths; B0304 keeps the Gateway runtime unchanged and terminalizes its five audited files as examined-no-change.
- Existing `edgion-tests/**` remains frozen and unchanged.
