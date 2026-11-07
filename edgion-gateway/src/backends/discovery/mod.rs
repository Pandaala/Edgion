mod coordinator;
pub mod dns_cache;
pub mod endpoint_slice;
pub mod services;

pub use coordinator::{
    get_service_discovery_coordinator, AvailableServiceProjection, LegacyRawProjection, ResolvedBackendSlot,
    ResolvedServiceSection, ServiceDiscoveryCoordinator, ServiceDiscoveryStatsSnapshot, ServicePublishedState,
    ServiceRuntimeCell, ServiceRuntimeSnapshot, ServiceSectionInvalid, ServiceSectionKey, ServiceSectionOutcome,
    ServiceSelectorGeneration, ServiceUnavailableReason,
};
pub(crate) use coordinator::{HealthPolicyIdentity, OutlierPolicyIdentity};
pub use dns_cache::{
    decide_external_name_path, upstream_dns_cache, DnsResolutionMode, ExternalNamePath, DEFAULT_DNS_CACHE_TTL,
};
pub use endpoint_slice::{create_ep_slice_handler, get_roundrobin_store, EndpointSliceExt, EpSliceStore};
pub use services::{create_service_handler, get_global_service_store, ServiceStore};
