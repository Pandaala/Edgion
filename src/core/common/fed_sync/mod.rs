//! Fed-sync gRPC type definitions shared by Controller and Center.
//!
//! This module only exposes generated proto types; runtime logic lives in
//! `core::controller::fed_sync` (client side). The federation server side now
//! lives in the separate EdgionCenter repository.

pub mod proto;
pub mod spiffe;
