//! Controller DNS resolver for `controller`-mode `ExternalName` Services.
//!
//! [`service`] resolves the opted-in Services and publishes in-memory
//! synthetic EndpointSlices ([`synthetic`]) through the EndpointSlice
//! pipeline; [`targets`] is the Service-event-driven work set.

pub mod service;
pub(crate) mod synthetic;
pub(crate) mod targets;

pub(crate) use service::{ensure_dns_resolver_running, DnsResolverTask};
pub(crate) use synthetic::DnsSyntheticSlices;
