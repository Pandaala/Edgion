# Investigate backend TLS verification failures

Status: deferred. Type: test verification issues discovered during optimization checks.

During the borrowed TLS metadata projection batch, `cargo test -p edgion-gateway
--lib backends::` reported 334 passing tests and three failures. Repeating with
`-- --test-threads=1` reported 336 passing tests and one failure.

The persistent failure is
`native_service_dispatch_tests::actual_dns_peer_stage_preserves_ssrf_tls_app_protocol_and_captured_absence`
in `tests/unit/backends/mod_test/native_service_dispatch_tests.rs`: its public DNS
answer reaches `peer_from_backend_tls_lookup` and returns `BackendTlsPolicyInvalid`.
The fixture creates an effective TLS policy without `preparedAncestorAdmission`.
The unchanged `ancestor_admitted` delegates to `policy_ancestor_permitted`, which
rejects absent preparation. Decide the fixture's intended admission mode and
populate its Controller-prepared admission; do not weaken production fail-closed
behavior or remove the test.

A detached worktree at the pre-change commit `7abcce8af` ran the same backend
suite serially: 335 passed and the identical DNS fixture failed at the same
unwrap with `BackendTlsPolicyInvalid`. The current run adds one passing test.
This confirms that deterministic failure predates this batch. The baseline
parallel suite was not run, so it does not establish the origin of the two
parallel-only failures.

The parallel run also failed these `peer_builder::pool_tests` cases:

- `captured_policy_uses_live_prepared_gateway_replacement_and_recovers_after_bad_material`:
  the final reuse-hash equality in `assert_isolated` failed.
- `prepared_identity_errors_precede_custom_ca_and_policy_overrides_failed_gateway`:
  observed `BackendTlsCaInvalid` where `Unknown` was expected.

Both passed in the serial run. The cause of their parallel failures remains
unproven; investigate shared Gateway identity state and test isolation before
changing expectations or production code. The new projection regression passed
in both runs. No existing test was changed or disabled.

## Additional focused peer-builder observation

During the RequestMirror address-borrow batch based on `54458581f`, the focused
`backends::peer_builder` filter passed 22 tests and failed
`pool_isolation_composes_gateway_rotation_and_policy_precedence` at the repeated
fallback reuse-hash equality in `tests/unit/backends/peer_builder_test/pool_tests.rs`.
The same 23 tests passed unchanged with `-- --test-threads=1`. This is another
parallel-failure/serial-success observation, not proof of a shared-state cause
or a fix. The focused filter did not exercise the earlier DNS fixture above.

The runtime diff only borrows the selected address in RequestMirror's synchronous
`resolve_target`; no peer-builder, Gateway TLS store or test source changed.
No pre-change parallel reproduction was run for this additional case. Preserve
this limit when investigating the existing isolation question; serial success
must not be reported as a clean parallel-suite result.
