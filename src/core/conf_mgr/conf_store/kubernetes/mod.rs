//! Kubernetes-based configuration store
//!
//! Provides a ConfStore implementation that watches and caches Kubernetes resources

mod store_impl;
pub mod controller;

pub use store_impl::KubernetesStore;

