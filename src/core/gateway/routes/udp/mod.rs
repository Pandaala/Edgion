//! UDP Routes and Proxy Implementation
//!
//! This module contains all UDP-related functionality:
//! - Per-port route management ([`routes_mgr`])
//! - UDP proxy implementation ([`edgion_udp`])
//! - Per-port route table ([`udp_route_table`])

mod conf_handler_impl;
mod routes_mgr;
pub(crate) mod udp_route_table;

pub mod edgion_udp;

pub use routes_mgr::{
    get_global_udp_route_managers, GlobalUdpRouteManagers, UdpPortRouteManager, UdpRouteManagerStats,
};

pub use conf_handler_impl::create_udp_route_handler;

pub use udp_route_table::UdpRouteTable;

pub use edgion_udp::EdgionUdpProxy;
