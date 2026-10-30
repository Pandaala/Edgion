//! Configuration Center module
//!
//! Provides configuration center implementations for different backends.
//!
//! ## Architecture
//!
//! ```text
//! ConfCenter = CenterApi + CenterLifeCycle (super trait)
//!     │
//!     ├── FileSystemCenter (implements ConfCenter)
//!     │   ├── CenterApi delegate -> FileSystemStorage
//!     │   └── CenterLifeCycle impl (lifecycle logic)
//!     │
//!     └── KubernetesCenter (implements ConfCenter)
//!         ├── CenterApi delegate -> KubernetesStorage
//!         └── CenterLifeCycle impl (lifecycle logic)
//! ```
//!
//! ## Traits
//!
//! - `CenterApi`: CRUD operations for configuration storage
//! - `CenterLifeCycle`: Lifecycle management (start, leadership, shutdown)
//! - `ConfCenter`: Super trait combining CenterApi + CenterLifeCycle
//!
//! ## Configuration
//!
//! - `common`: Common types shared across backends
//! - `config`: Top-level ConfCenterConfig enum
//! - `kubernetes::config`: Kubernetes-specific config (LeaderElectionConfig, MetadataFilterConfig)
//!
//! ## Implementations
//!
//! - `FileSystemCenter`: FileSystem-based implementation
//! - `KubernetesCenter`: Kubernetes API-based implementation

pub mod common;
mod config;
pub mod traits;

// These macros are defined here (rather than in non_kube_common.rs) so that
// child modules (non_kube_common, etcd, file_system) inherit them via textual
// scoping without needing explicit `use` imports.

/// Invoke `$cb!(TypeIdent, handler_expr)` for each provider resource.
/// Excludes conditional `EndpointSlice`; callers handle it.
macro_rules! for_each_provider_core_resource {
    ($cb:ident, $controller_name:expr) => {
        $cb!(
            GatewayClass,
            GatewayClassHandler::new(GatewayClassOwnership::new($controller_name.to_string()))
        );
        $cb!(Secret, SecretHandler::new());
        $cb!(ConfigMap, ConfigMapHandler::new());
        $cb!(ReferenceGrant, ReferenceGrantHandler::new());
        $cb!(Service, ServiceHandler::new());
        $cb!(EdgionConfigData, EdgionConfigDataHandler::new());
    };
}

/// Invoke `$cb!(TypeIdent, handler_expr)` for each dependent resource.
macro_rules! for_each_dependent_resource {
    ($cb:ident, $controller_name:expr) => {
        // Routes
        $cb!(HTTPRoute, HttpRouteHandler::new($controller_name.to_string()));
        $cb!(GRPCRoute, GrpcRouteHandler::new($controller_name.to_string()));
        $cb!(TCPRoute, TcpRouteHandler::new($controller_name.to_string()));
        $cb!(UDPRoute, UdpRouteHandler::new($controller_name.to_string()));
        $cb!(TLSRoute, TlsRouteHandler::new($controller_name.to_string()));
        // TLS
        $cb!(EdgionTls, EdgionTlsHandler::new($controller_name.to_string()));
        $cb!(
            BackendTLSPolicy,
            BackendTlsPolicyHandler::new($controller_name.to_string())
        );
        // Plugins
        $cb!(EdgionPlugins, EdgionPluginsHandler::new());
        $cb!(EdgionStreamPlugins, EdgionStreamPluginsHandler::new());
        $cb!(LinkSys, LinkSysHandler::new());
        $cb!(EdgionBackend, EdgionBackendHandler::new());
        // ACME
        $cb!(EdgionAcme, EdgionAcmeHandler::new());
        // Backend traffic policy
        $cb!(
            EdgionBackendTrafficPolicy,
            EdgionBackendTrafficPolicyHandler::new($controller_name.to_string())
        );
        // Cluster-scoped dependent
        $cb!(EdgionGatewayConfig, EdgionGatewayConfigHandler::new());
    };
}

pub mod non_kube_common;

pub mod etcd;
pub mod file_system;
pub mod kubernetes;

// Re-export traits
pub use traits::{
    CenterApi, CenterLifeCycle, ConfCenter, ConfEntry, ConfWriterError, ListOptions, ListResult, ResourceKey,
    ResourceKeyMeta,
};

// Re-export FileSystem types
pub use file_system::{FileSystemCenter, FileSystemConfig, FileSystemController, FileSystemStorage};

// Re-export Kubernetes types
pub use kubernetes::{
    ControllerExitReason, KubernetesCenter, KubernetesConfig, KubernetesController, KubernetesStorage, LeaderElection,
    LeaderElectionConfig, LeaderHandle, MetadataFilterConfig, NamespaceWatchMode,
};

// Re-export Etcd types
pub use etcd::{EtcdCenter, EtcdConfig};

// Re-export configuration types
pub use config::ConfCenterConfig;
