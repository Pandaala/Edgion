//! LeastConnection load balancing algorithm
//!
//! Implements a least-connection load balancing strategy that directs traffic
//! to the backend with the fewest active connections.
//!
//! ## Usage
//!
//! The connection counter must be used in the proxy layer:
//! - Call `increment(&addr)` when a connection is established
//! - Call `decrement(&addr)` when a connection is closed
//!
//! ## Example
//!
//! ```ignore
//! use crate::core::lb::leastconn::{LeastConnection, increment, decrement};
//!
//! // In upstream_peer success path:
//! increment(&peer_addr);
//!
//! // In logging/cleanup phase:
//! decrement(&peer_addr);
//! ```

pub mod backend_state;
mod cleaner;
mod counter;
mod selection;

pub use backend_state::{get_state, is_active, mark_draining, reactivate, BackendState};
pub use cleaner::BackendCleaner;
pub use counter::{decrement, get_count, increment, remove};
pub use selection::LeastConnection;
