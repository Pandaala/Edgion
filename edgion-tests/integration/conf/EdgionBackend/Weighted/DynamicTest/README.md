# Weighted EdgionBackend Dynamic Test

Scenario: `AiBackendWeighted`.

This self-cleaning scenario creates two distinguishable EdgionBackend targets
with route weights 1:3. Deterministic UUID trace IDs make the weighted entropy
cover complete four-ticket cycles, so each 32-request observation must select
the targets exactly 8 and 24 times.

The route also contains a real Service with weight zero. It proves that a
zero-weight Service is protocol-neutral and does not make an otherwise all-AI
route invalid. Deleting the weight-three EdgionBackend, and then denying that
same cross-namespace reference by updating its ReferenceGrant, must each yield
24 failed selections and 8 successful selections of the weight-one backend.
This proves the invalid positive-weight slot is retained rather than silently
redistributed. Recreating or re-authorizing the backend restores the exact 1:3
distribution.

The same route then exercises native backend request-header modifiers through
initial, updated, restored, and removed filter generations. Both providers must
observe only the first case-equivalent configured set/add entry, all preexisting
same-name add values, and removal of every matching field line. The shared AI
fixture's `mock_mode=header_modifier` observes only these synthetic test fields.

Finally, the scenario replaces the direct parent rule with a delegation to a
child HTTPRoute carrying the same ordered backend slots. It repeats both the
healthy distribution and the denied-slot observation, proving that delegation
preserves weights, ordering, and invalid-slot identity.

Both mock providers use the shared local AI test server. Their distinct
endpoint prefixes make the selected EdgionBackend externally observable in
the provider response without adding test-only production behavior.
