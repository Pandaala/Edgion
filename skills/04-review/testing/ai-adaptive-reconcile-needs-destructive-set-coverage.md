# Adaptive telemetry reconciliation needs destructive-set coverage

## Decision

Adaptive telemetry reconciliation publishes exactly the retained key set. A
missing key therefore deletes discoverability for a live route, target, or
logical model and silently leaves adaptive selection without telemetry.

Keep the route-shape walk testable without mutating the process-global plugin
or AI-service stores. The production wrapper supplies two lookups: whether the
referenced plugin contains an enabled `AiProxy` using `LowestLatency` or
`LowestUsage`, and the catalogued logical models for one typed AI-service
target. The pure collector owns all route, namespace, identity, backend-kind,
and key-construction rules.

## Required coverage

- Only an enabled adaptive `AiProxy` activates retention; disabled entries and
  non-adaptive selection modes do not.
- Non-`ExtensionRef` filters, wrong plugin group or kind, unresolved terminal
  route identity, non-AI backends, missing catalog models, and a missing plugin
  match retain no keys.
- A valid rule retains one exact key per catalogued model. Backend namespace
  defaults to the route namespace, while a delegated plugin reference uses its
  resolved namespace.
- `AdaptiveTelemetryKey::new` rejects every empty identity component and a
  non-`EdgionBackend` target.
- A state `Arc` must contain recorded telemetry before target removal is
  tested. After removal hides the key, the captured state remains readable and
  accepts later records, proving request plans survive cold-path cleanup.

An assertion against a never-populated default snapshot is not evidence that
cleanup preserves captured state: it passes even when no lifecycle behavior is
exercised.
