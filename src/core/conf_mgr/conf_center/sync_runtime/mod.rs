//! Gateway API 资源同步运行时
//!
//! 提供通用的同步组件，可被各种 conf_center 后端复用：
//! - Workqueue: 去重 + 重试的工作队列
//! - Shutdown: 优雅关闭信号
//! - Metrics: 同步指标
//! - ResourceProcessor: 资源处理器 trait 和实现

pub mod metrics;
pub mod resource_processor;
pub mod shutdown;
pub mod workqueue;

pub use metrics::{controller_metrics, ControllerMetrics, InitSyncTimer, ResourceMetrics};
pub use resource_processor::{
    make_resource_key, BackendTlsPolicyProcessor, EdgionGatewayConfigProcessor, EdgionPluginsProcessor,
    EdgionStreamPluginsProcessor, EdgionTlsProcessor, EndpointSliceProcessor, EndpointsProcessor,
    GatewayClassProcessor, GatewayProcessor, GrpcRouteProcessor, HttpRouteProcessor, LinkSysProcessor,
    PluginMetadataProcessor, ProcessConfig, ProcessContext, ProcessResult, ReferenceGrantProcessor,
    RequeueRegistry, ResourceProcessor, SecretProcessor, ServiceProcessor, TcpRouteProcessor, TlsRouteProcessor,
    UdpRouteProcessor,
};
pub use shutdown::{ShutdownController, ShutdownHandle, ShutdownSignal};
pub use workqueue::{WorkItem, Workqueue, WorkqueueConfig, WorkqueueMetrics};
