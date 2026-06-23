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
//! - `CenterLifeCycle`: Lifecycle management (start, reload, shutdown)
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

/// Invoke `$cb!(TypeIdent, handler_expr)` for each Phase-1 core resource.
/// Excludes conditional `Endpoints`/`EndpointSlice`; callers handle those.
macro_rules! for_each_phase1_core_resource {
    ($cb:ident) => {
        $cb!(
            GatewayClass,
            GatewayClassHandler::new(crate::conf_mgr::conf_center::common::DEFAULT_CONTROLLER_NAME.to_string())
        );
        $cb!(Gateway, GatewayHandler::new(None, None));
        $cb!(Secret, SecretHandler::new());
        $cb!(ConfigMap, ConfigMapHandler::new());
        $cb!(ReferenceGrant, ReferenceGrantHandler::new());
        $cb!(Service, ServiceHandler::new());
    };
}

/// Invoke `$cb!(TypeIdent, handler_expr)` for each Phase-2 dependent resource.
macro_rules! for_each_phase2_resource {
    ($cb:ident) => {
        // Routes
        $cb!(
            HTTPRoute,
            HttpRouteHandler::new(crate::conf_mgr::conf_center::common::DEFAULT_CONTROLLER_NAME.to_string())
        );
        $cb!(
            GRPCRoute,
            GrpcRouteHandler::new(crate::conf_mgr::conf_center::common::DEFAULT_CONTROLLER_NAME.to_string())
        );
        $cb!(
            TCPRoute,
            TcpRouteHandler::new(crate::conf_mgr::conf_center::common::DEFAULT_CONTROLLER_NAME.to_string())
        );
        $cb!(
            UDPRoute,
            UdpRouteHandler::new(crate::conf_mgr::conf_center::common::DEFAULT_CONTROLLER_NAME.to_string())
        );
        $cb!(
            TLSRoute,
            TlsRouteHandler::new(crate::conf_mgr::conf_center::common::DEFAULT_CONTROLLER_NAME.to_string())
        );
        // TLS
        $cb!(
            EdgionTls,
            EdgionTlsHandler::new(crate::conf_mgr::conf_center::common::DEFAULT_CONTROLLER_NAME.to_string())
        );
        $cb!(
            BackendTLSPolicy,
            BackendTlsPolicyHandler::new(crate::conf_mgr::conf_center::common::DEFAULT_CONTROLLER_NAME.to_string())
        );
        // Plugins
        $cb!(EdgionPlugins, EdgionPluginsHandler::new());
        $cb!(EdgionStreamPlugins, EdgionStreamPluginsHandler::new());
        $cb!(EdgionConfigData, EdgionConfigDataHandler::new());
        $cb!(LinkSys, LinkSysHandler::new());
        // ACME
        $cb!(EdgionAcme, EdgionAcmeHandler::new());
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
    LeaderElectionConfig, LeaderHandle, MetadataFilterConfig, NamespaceWatchMode, RelinkReason,
};

// Re-export Etcd types
pub use etcd::{EtcdCenter, EtcdConfig};

// Re-export configuration types
pub use config::ConfCenterConfig;
