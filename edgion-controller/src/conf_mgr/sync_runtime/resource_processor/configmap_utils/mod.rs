//! ConfigMap utilities for BackendTLSPolicy CA certificate resolution
//!
//! Provides a minimal ConfigMap store for resolving `caCertificateRefs`
//! with `kind: ConfigMap` in BackendTLSPolicy resources.

mod configmap_store;

pub use configmap_store::ConfigMapStore;
