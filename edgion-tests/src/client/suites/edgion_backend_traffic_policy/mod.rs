// EdgionBackendTrafficPolicy scenario group.
//
// End-to-end coverage for the EdgionBackendTrafficPolicy CRD, focused on the
// control-plane / config-sync contract that is unique to the policy resource
// (and NOT already exercised by the data-plane suites that were migrated onto
// the CRD). It drives policies through the Controller Admin API at runtime and
// asserts the resulting state on the Gateway admin surface:
//
//   * GET /api/v1/debug/store-stats -> data.lb_policies (service_key -> algorithm)
//   * GET /configclient/edgionbackendtrafficpolicy/list  (synced policy list)
//
// Coverage map to the spec's seven integration items:
//   (1) active health check   -> config round-trip asserted here; data-plane
//                                ejection covered by HealthCheck/GrpcHealthCheck
//                                suites (now CRD-driven).
//   (2) outlier detection     -> config round-trip asserted here; data-plane
//                                ejection+restore covered by CircuitBreaker suite.
//   (3) LB algorithm select   -> lb_store_mapping (LeastConn/Ewma/ConsistentHash
//                                + no-policy -> RoundRobin default = absent entry).
//   (4) degrade threshold     -> config round-trip asserted here; data-plane degrade
//                                behavior covered by DegradeThreshold suite.
//   (5) hot reload            -> hot_reload_create_edit_delete.
//   (6) conflict oldest-wins  -> conflict_oldest_wins_and_promotion.
//   (7) resync survival       -> resync_service_churn (Service delete+recreate with
//                                the policy intact; see note in that case about the
//                                gateway-reconnect variant).

mod connect_timeout;
mod control_plane;
mod retry_budget;
mod upstream_authority;

pub use connect_timeout::ConnectTimeoutTestSuite;
pub use control_plane::EdgionBackendTrafficPolicyTestSuite;
pub use retry_budget::RetryBudgetTestSuite;
pub use upstream_authority::UpstreamAuthorityTestSuite;
