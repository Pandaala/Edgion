//! Controller-owned acquisition and publication of dynamic credentials.

pub mod providers;
pub mod scheduler;
pub mod secret_io;
mod service;

pub use service::{start_credential_manager_service, stop_credential_manager_service};
