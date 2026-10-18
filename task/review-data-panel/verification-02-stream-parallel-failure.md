# Stream-suite parallel verification failure

Status: deferred for investigation; no behavior change proposed.

## Observation

During the static rejection-reason ownership batch based on `6cb12274f`,
`cargo test -p edgion-gateway --lib plugins::stream::` returned 111 passed,
one failed and one ignored. The failure was
`connection_filter_bridge::tests::listener_annotation_can_be_added_and_removed_after_startup`
at `edgion-gateway/tests/unit/plugins/stream/connection_filter_bridge_test.rs:247`:
the connection was allowed after publishing the referenced deny-list plugin.

The exact test passed in isolation. The complete Stream suite then passed with
`-- --test-threads=1` (112 passed, one ignored). The 14 TLS consumer tests also
passed. The ignored test was already ignored before this batch.

## Evidence and limits

The fixture uses a local PluginPublisher but inserts its IpList into the global
EdgionConfigData store. This makes shared-state interactions worth investigating;
it does not establish a race or identify the interfering test. The insertion
helper already uses atomic RCU and unique keys, so ordinary concurrent inserts
must not be blamed without additional evidence.

The optimization changes denial reason storage and fixed formatting only, not
policy matching, snapshot selection or Allow/Deny branch selection. The source
comparison supports unchanged decisions. No baseline binary comparison was run,
and neither the failure's preexistence nor its root cause is established.

## Follow-up acceptance

Reproduce the parallel failure and identify the interacting state transition or
fixture. Compare against the pre-change baseline before attributing causation.
Preserve the hot-reload assertions and production fail-closed behavior. If shared
test state is responsible, repair isolation using the owning test conventions;
do not weaken the denial assertion or use serialization as the permanent fix
without identifying the shared owner. Re-run parallel and isolated cases after
any correction.
