//! TCP Routes and Proxy Implementation
//!
//! This module contains all TCP-related functionality:
//! - Per-port route management ([`routes_mgr`])
//! - TCP proxy implementation ([`edgion_tcp`])
//! - Per-port route table ([`tcp_route_table`])

mod conf_handler_impl;
mod routes_mgr;
pub(crate) mod tcp_route_table;

pub mod edgion_tcp;

pub use routes_mgr::{
    get_global_tcp_route_managers, GlobalTcpRouteManagers, TcpPortRouteManager, TcpRouteManagerStats,
};

pub use conf_handler_impl::create_tcp_route_handler;

pub use tcp_route_table::TcpRouteTable;

pub use edgion_tcp::EdgionTcpProxy;
