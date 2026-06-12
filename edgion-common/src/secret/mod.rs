//! Global Secret store, shared by the control plane (writers) and the data
//! plane (readers).
//!
//! Lives in `common` so both the controller (which populates the store from
//! Kubernetes Secret resources) and the gateway (TLS callbacks + plugins that
//! read secret material) can depend on it without a cross-plane edge.

mod secret_store;

pub use secret_store::{get_global_secret_store, get_secret, get_secret_by_name, SecretStore};
