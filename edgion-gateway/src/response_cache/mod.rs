//! Bounded public-origin cache ownership; request plugins nominate and the proxy tail binds eligibility.

mod key;
pub(crate) mod lifecycle;
mod policy;
mod storage;

pub(crate) use key::{discovery_stamp, tls_result, traffic_stamp, DiscoveryStamp, TlsStamp};
pub(crate) use lifecycle::{record_discovery, record_tls, record_traffic};
pub(crate) use policy::GraphqlProof;
pub(crate) use storage::RequestState;
pub(crate) use storage::{advance_epoch, current_epoch, epoch_control_bytes, PublicationEpoch};
pub(crate) use storage::{initialize, nominate, process_store, snapshot_stats, Stats, Store};

#[cfg(test)]
#[path = "../../tests/unit/response_cache/mod_test.rs"]
pub(crate) mod tests;
