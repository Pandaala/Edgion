//! FileSystem based configuration center
//!
//! Provides:
//! - FileSystemCenter: unified configuration center implementing ConfCenter trait
//! - FileSystemWriter: reading/writing local YAML files (used by Admin API)
//! - FileSystemController: unified controller that spawns independent ResourceControllers
//! - FileSystemWatcher: centralized file watching with event dispatch by kind
//!
//! ## Architecture
//!
//! ```text
//! FileSystemCenter (implements ConfCenter = CenterApi + CenterLifeCycle)
//!     │
//!     ├── writer: FileSystemWriter (CenterApi delegate)
//!     │
//!     └── lifecycle (CenterLifeCycle impl)
//!             │
//!             └── FileSystemController
//!                     │
//!                     ├── spawn::<HTTPRoute, _>(HttpRouteHandler)
//!                     │       └── ResourceProcessor + ResourceController
//!                     │
//!                     ├── spawn::<Gateway, _>(GatewayHandler)  
//!                     │       └── ResourceProcessor + ResourceController
//!                     │
//!                     └── FileSystemWatcher (shared)
//!                             │
//!                             ├── Init phase: scan dir -> dispatch Init/InitApply/InitDone by kind
//!                             │
//!                             └── Runtime phase: inotify -> dispatch Apply/Delete by kind
//! ```
//!
//! ## File naming convention
//!
//! - With namespace: `{Kind}_{namespace}_{name}.yaml`
//! - Cluster-scoped: `{Kind}__{name}.yaml` (double underscore)

mod center;
pub mod config;
mod controller;
mod event;
mod file_watcher;
mod resource_controller;
mod writer;

pub use center::FileSystemCenter;
pub use config::FileSystemConfig;
pub use controller::FileSystemController;
pub use event::{FileSystemEvent, ResourceEvent};
pub use file_watcher::FileSystemWatcher;
pub use resource_controller::FileSystemResourceController;
pub use writer::FileSystemWriter;
