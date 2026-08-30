# Conf-Sync Rotation Transactional Address Selection

## Meta

| Key | Value |
|-----|-------|
| Created | 2026-08-30 |
| Status | pending |
| Type | bugfix |
| Priority | P1 |
| Issue | N/A |

## AI guide summary

`ConfigSyncClient::rotate` advances `EndpointRotator` before the replacement
channel is built and installed. If installation fails, the active channel and
`current_addr` remain on the old endpoint while the rotator cursor and lap-failure
count describe the uninstalled candidate. Fix this as one transactional selection
state machine; do not add an isolated cursor decrement or adjust only one caller.

## Confirmed findings

- `EndpointRotator::advance` changes both the cursor and lap-failure count before
  `install_channel` validates TLS material and builds the lazy channel.
- `install_channel` deliberately leaves the active channel untouched on failure.
- After such a failure, the next rotation advances from the uninstalled cursor,
  so a candidate can be skipped until a later lap.
- Full-lap accounting can include an address that was selected but never installed
  or dialed.
- `reload_client_tls` snapshots `current_addr` before `install_channel` acquires
  `swap_lock`. A concurrent rotation can therefore install address B first and a
  delayed reload can then reinstall stale address A while the rotator remains at B.
- Bootstrap rotation and the long-lived leader-session coordinator share this
  mechanism and need one consistent failure definition.

## Requirements

- Separate candidate selection from committed cursor/lap state, or provide an
  equivalent transaction whose failure leaves rotator and installed-channel
  state consistent.
- Decide explicitly whether TLS/file/URI channel-construction failures count as
  endpoint failures; do not silently mix local configuration failures with remote
  connection failures.
- Preserve shuffled first selection, single-address stability, leader reload
  dwell, generation cancellation ordering, and capped jittered full-lap backoff.
- Keep validate/build before publication and cache-client replacement before the
  old generation is cancelled.
- Serialize certificate reload and endpoint rotation through the existing channel
  swap boundary, including selection of the address that a certificate reload
  rebuilds. Do not snapshot that address before entering the transaction.

## Acceptance tests

- Failed installation leaves the active address and next rotation candidate
  consistent; no address is skipped.
- A local TLS validation failure follows the documented lap-counting policy.
- A successful installation commits the candidate exactly once and wakes all
  watch loops only after every cache client and meta handle are updated.
- Two complete failed remote laps produce the intended capped backoff progression.
- Single-address rotation remains a channel and generation no-op.
- Concurrent certificate reload and endpoint rotation cannot commit mismatched
  address, TLS identity, handle, or generation state.
- A reload racing after a successful A-to-B rotation rebuilds B, never stale A.
- Bootstrap and leader-session focused tests plus normal workspace guards pass.

## Scope

### In scope

- `edgion-gateway/src/conf_sync/conf_client/rotation.rs`
- `edgion-gateway/src/conf_sync/conf_client/grpc_client.rs`
- Narrow bootstrap call-site adjustments when required by the new transaction API
- Focused rotation/channel-publication tests

### Out of scope

- Controller leader-election changes
- New transport protocols or wire fields
- Changing the configured address order before its one-time shuffle
- Existing integration-test files

## Related context

- `skills/00-design-rules.md`
- `skills/01-architecture/03-controller-gateway-link/02-gateway-side.md`
- `AGENTS.md` concurrency and conf-sync transport rules
