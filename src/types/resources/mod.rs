//! Kubernetes resource type definitions
//!
//! This module contains all custom resource definitions and Gateway API types

pub mod edgion_gateway_config;
pub mod edgion_tls;
pub mod gateway;
pub mod gateway_class;
pub mod http_route;

// Re-export all resource types
pub use self::edgion_gateway_config::*;
pub use self::edgion_tls::*;
pub use self::gateway::*;
pub use self::gateway_class::*;
pub use self::http_route::*;

