//! Fed-sync gRPC type definitions shared by Controller and Center.
//!
//! This module only exposes generated proto types; runtime logic lives in
//! `core::controller::fed_sync` (client side) and `core::center::fed_sync`
//! (server side).

pub mod proto;
