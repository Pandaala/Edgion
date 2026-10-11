---
id: ARCH-025
status: candidate
kind: behavior-defect
priority: P2
primary_partition: PART-273
independent_verification: confirmed
---

# Reject malformed Elasticsearch date formats before worker publication

## Current ownership and concrete trigger

Elasticsearch config_mapping.rs:191 copies index.datePattern into EsBulkConfig
without validation. current_index_name at line 229 invokes
Utc::now().format(&self.date_pattern).to_string(). Shared LinkSys validation checks
nonempty endpoints but does not validate this pattern; actual-resource runtime
construction starts the worker without checking it either.

A valid endpoint configuration with datePattern "%" can publish a client. On
the first nonempty flush, flush_batch calls current_index_name. Locked Chrono
0.4.45 parses the trailing percent as Item::Error and its formatter returns
fmt::Error. ToString converts that formatting failure into a panic, terminating
the spawned bulk worker. This conclusion comes from the local locked dependency
source, not a runtime reproduction.

The retained client sender then observes the closed receiver. The independently
running endpoint probe can still report healthy, so first-healthy access-log
routing continues choosing ES and dropping entries instead of taking its
fallback. This is a configuration-boundary failure, not a claimed incident.

## Bounded improvement and alternative

Validate the date format once through shared resource validation, and defend
actual-resource Gateway construction before spawning tasks. Keep a canonical
validator for Controller/Gateway parity. Precompiled owned format items are an
optional cold optimization; plain validation suffices. Silent default replacement
would conceal the invalid configuration. Panic catching is not the normal policy
for a predictable invalid field.

The improvement puts invalidity at the cold owner rather than the long-running
worker. Preserve configured UTC index naming, escaped NDJSON headers, effective
fingerprint identity, equal-generation reuse, health publication and fallback
policy. No request allocation, tracing or additional registry is needed.

## Authority, migration and future verification

Original endpoint admission, bulk partition/header escaping, item retries,
shutdown, provider lifecycle and healthy/fallback decisions were inspected.
None accepts invalid date-format worker termination. The resources/Gateway
boundary and construction result are migration seams; no historical aliases are
needed. Existing worker failure and delivery policies remain intact.

Future checks: malformed and trailing-percent patterns, valid escaped percent,
defaults, Controller/Gateway parity and rejection before tasks start. Merely
testing normal index formatting does not distinguish the failure. Static only;
no builds, tests or measurements. Source/knowledge hash receipts are in the
partition report and subsequent independent verification.
