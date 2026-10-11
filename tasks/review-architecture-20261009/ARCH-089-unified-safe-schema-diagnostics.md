---
id: ARCH-089
status: candidate
kind: architecture-debt
priority: P2
partition: PART-393
analyst: /root
baseline: 22f0fb8b6f4353f689ca4ddc172a4dfd98a2d2929dd1709240cbe4a1b3796471
---

Title: Give all schema-validation diagnostics the existing bounded value-free formatter

SchemaValidator owns two error-formatting contracts for the same JSON Schema mechanism. validate_gateway_source uses bounded schema/path diagnostics and masks user map keys, while validate for Edgion custom resources interpolates jsonschema ValidationError Display and its raw instance path. Adding a custom schema constraint therefore introduces a value-printing observability path even though the file already contains the safer canonical mechanism.

The live custom-resource branch is schema_validator.rs:308-330. It logs each raw error at debug and joins all errors into ValidationError::SchemaViolation. api/common.rs:38-46 logs that returned string at WARN during non-Kubernetes create/update. namespaced_handlers.rs:225-227 and569-571 actually parse EdgionTls then call that helper. EdgionTlsSpec.hosts is a plain Vec<String>; the CRD rejects invalid hostname patterns. A typed document with hosts containing tenant-internal_host is therefore rejected by the loaded schema, and the pinned jsonschema0.40.2 Pattern formatter prints that submitted value. The warning carries authored configuration, although the value is unnecessary to locate /spec/hosts/0 and the failing pattern. This witness does not assert that every malformed document survives typed parsing or that a private key is presently printed by this specific hostname constraint.

The same unsafe formatting is returned by the validation endpoint; response recipients are authorized callers who supplied their document, but that does not make the separate persistent WARN safe. Type, enum, pattern and array-limit Display branches may print larger instances; the custom branch also has no error-count limit. The demonstrated violation is configuration leakage in ordinary warning logs. Future schema changes should not need a second review of raw-value formatting when the existing Gateway API path has already established the diagnostic boundary.

Use one private bounded formatter for errors from both loaded registries, based on safe schema locations, masked instance paths and fixed error classes when useful. Generalize the existing safe_gateway_error_path helper locally rather than adding a redaction registry, inspecting arbitrary raw exception strings, or introducing typed resource taxonomy. Retain useful property names and true array indexes, mask authored map keys, cap both count and diagnostic lengths, and do not interpolate instance values. Avoid a second raw debug emission that would bypass that owner. A short fixed summary at the existing warning site is sufficient; no new logging or metric family is needed.

Preserve the current validation placement: local custom-resource schemas on non-Kubernetes Admin writes, empty validators under Kubernetes, source-GVK/profile selection and shared reference normalization for Gateway API, existing synchronous400 rejection before persistence, and direct-watcher asynchronous handler/status behavior. Do not expand this into live CRD discovery, new admission rules, schema relocation or altered accepted/rejected documents. Validator::new errors from trusted schema construction are a separate cold surface, outside this finding.

The full existing schema_validator_test bodies prove that Gateway API errors hide values/map keys and bound reference-path diagnostics; custom-resource tests assert acceptance/rejection without asserting returned/loggable strings are safe. Add the actual EdgionTls malformed-host case and a custom-registry schema with an authored map-key error, verifying value-free bounded diagnostics while retaining useful locations. Preserve existing profile and unknown-plugin tests. No test or application was executed.

Original schema-validator-non-k8s-asymmetry, the review Resources/Observability indexes and log-safety rules were read. The accepted validation-placement decision does not authorize unsafe formatting. No earlier candidate or matching pending task was found. Analysis is source-based, with a pinned formatter inspected; no production log incident or credential exploit is claimed.
