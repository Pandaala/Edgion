//! Kubernetes-based configuration store
//!
//! Provides a ConfStore implementation that watches and caches Kubernetes resources

pub mod controller;
mod store_impl;

pub use store_impl::KubernetesStore;
