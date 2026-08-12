# AI endpoint failure skips the service

Predispatch failures are classified by the configuration they depend on.
Credential admission failures skip only that credential. DNS resolution and
resolved-address safety validation depend solely on the EdgionBackend
endpoint, so either failure must skip the whole service for the current
scheduler round.

Peer construction returns a typed predispatch failure whose variant owns this
scope decision. Its `finish()` method releases the active reservation and
advances the scheduler. Future credential-dependent fallible work must use a
separate variant instead of being folded into the endpoint branch.

The active credential reservation is still released before advancing. A
service-scoped predispatch skip consumes no dispatch slot, failure-ledger slot,
or retry-budget sample, and the route picker may immediately try another AI
service.

Tests for this boundary need at least two credentials on the failed service and
a second service. They must prove the failed service cannot be selected again,
the second service remains selectable, and dispatch count stays zero.
