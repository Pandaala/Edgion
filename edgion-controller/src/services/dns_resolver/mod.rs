//! Control-plane DNS resolver service.
//!
//! Background loop that resolves `mode=controller` ExternalName Services and
//! publishes real EndpointSlices to the apiserver (K8s conf_center mode only).
//! See [`service`] for the loop and [`slice_builder`] for the pure EndpointSlice
//! construction + reconcile decision.

pub mod service;
pub mod slice_builder;
pub(crate) mod targets;

pub use service::{start_dns_resolver_service, stop_dns_resolver_service};
