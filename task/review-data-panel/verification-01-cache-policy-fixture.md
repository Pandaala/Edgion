# Repair the cache reload test's policy admission fixture

Status: deferred. Type: test fixture issue discovered during optimization checks.

`response_cache_real_tail_to_peer_store_reload_discards_fill_and_preserves_ordinary_capture`
in `edgion-gateway/tests/unit/routes/http/proxy_http/pg_request_filter_test/response_cache_tests.rs`
fails at the expected successful upstream selection with `backend policy ancestor limit`.
The failure reproduces in an isolated run and a serial run of the 45 request-filter tests
(44 pass, one fails).

The traffic-policy branch directly publishes a policy without
`preparedAncestorAdmission`. The same fixture omission is present in baseline
`81291f899`. The traffic store preserves None, and
`pg_upstream_peer::prepare_selected_policy_admission` calls `ancestor_admitted`,
whose canonical `policy_ancestor_permitted` rejects absent preparation. This is
the intended fail-closed production contract, not a consequence of borrowing
the temporary raw request path. Independent review confirmed this source chain;
the baseline binary was not rebuilt or executed.

Decide which configuration mode this fixture represents, then populate the
matching Controller-prepared admission (scoped to `cache-c0/gateway` for the
scoped case). Do not relax production admission or silently remove the test.
Re-run the isolated case and complete request-filter group after correction.
