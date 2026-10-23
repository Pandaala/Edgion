# HTTP route parallel verification failures

Status: deferred investigation; no runtime or fixture change authorized here.
Observed baseline: `36074f033`, before the empty hostname-index optimization.

## Observation

`cargo test -p edgion-gateway --lib routes::http::tests` passed 37 tests and
failed two in `tests/unit/routes/http/mod_test.rs`:

- `full_set_and_gateway_rebuild_reuse_unchanged_hostname_tree`: the replacement
  table did not share the previous hostname tree at line 252.
- `hostname_delta_preserves_untouched_paths_units_and_old_snapshots`: the updated
  route lookup returned None instead of the expected owner at line 326.

Each test passed in isolation. The unchanged full selection then passed all
39 tests with `-- --test-threads=1`. This establishes a failing parallel baseline,
not a proven production defect or an identified test race. Both fixtures create
local route managers but exercise publication/rebuild machinery. Investigate
shared state and admission dependencies before assigning a cause.

## Follow-up acceptance

Reproduce the parallel failures and identify the interacting owner, state change
or fixture. Preserve topology-sharing, old-snapshot and route-selection assertions.
If fixture isolation is responsible, fix it at the owning boundary; do not use
serialization or weaker assertions as an unexplained permanent repair. Verify
the selected correction with parallel and isolated runs. This task does not
authorize changes to runtime admission or hostname matching semantics.
