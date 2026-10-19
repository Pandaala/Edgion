//! Parsed load-balancing policy types.
//!
//! These are the runtime representation of a load-balancing algorithm and (for
//! `ConsistentHash`) its hash source. They have no `extensionRef`-parsing origin
//! anymore — the route-level `extensionRef` LB mechanism was removed. Today the
//! sole producer is [`crate::resources::edgion_backend_traffic_policy::LoadBalancerConfig::to_parsed`]
//! (`EdgionBackendTrafficPolicy`). Consumers on the gateway side: the
//! `EdgionBackendTrafficPolicy` policy store that resolves and caches the
//! per-service winner (`edgion-gateway/src/backends/policy/backend_traffic/store.rs`),
//! the backend selector that maps a policy onto a selection algorithm
//! (`edgion-gateway/src/backends/selector.rs`), backend selection itself
//! (`edgion-gateway/src/backends/mod.rs`), and per-attempt LB accounting
//! (`edgion-gateway/src/lb/attempt.rs`).
//!
//! `Serialize` is required because the gateway `UpstreamInfo` context struct
//! (`edgion-gateway/src/ctx.rs`) derives `Serialize` and holds an
//! `Option<ParsedLBPolicy>`. `Deserialize` is not needed (no consumer parses
//! these back from a wire format).

use serde::Serialize;
use std::sync::Arc;

/// Hash source type for consistent hash LB policy
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ConsistentHashOn {
    /// Hash based on request header
    Header(Arc<str>),
    /// Hash based on cookie
    Cookie(Arc<str>),
    /// Hash based on query argument
    Arg(Arc<str>),
    /// Hash based on the (RealIp-normalized) client source IP. No value name.
    SourceIp,
}

/// Parsed LB policy
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum ParsedLBPolicy {
    /// Consistent hash with specified source
    ConsistentHash(ConsistentHashOn),
    /// Least connection
    LeastConn,
    /// EWMA (Exponentially Weighted Moving Average) based on response time
    Ewma,
}
