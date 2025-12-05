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

mod counter;
mod selection;

pub use counter::{decrement, get_count, increment};
pub use selection::LeastConnection;

