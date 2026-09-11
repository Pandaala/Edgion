# Define OTLP Egress Enforcement and Safe Endpoint Identity

## Meta
- Status: pending
- Severity: high
- Category: security

## Evidence
The OTLP SDK exporter and its independent TCP health probe directly parse and dial an operator-configured endpoint without dial-time `EgressResolver` or IP classification. The complete free-form endpoint is also used as health metric link identity and appears in logs and errors; a URI containing user information, a path, or a query token can therefore expose sensitive material and create high-cardinality labels.

## Why separate
The correction spans exporter construction, health probing, endpoint identity/redaction, DNS and egress policy, and the supported OTLP transport contract. Kafka, Redis, and OTLP request-path masking are separate concerns.

## Candidate next step
Define an OTLP destination model with safe bounded identity, apply dial-time egress checks to the exporter and probe, prevent credentials/path/query material from entering logs or metric labels, and separately confirm the product contract for system DNS and SDK-managed HTTPS before implementation.
