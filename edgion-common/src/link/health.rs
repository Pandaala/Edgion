//! Health status DTO shared by all LinkSys providers.
//!
//! This is the canonical type — providers must `use crate::link::LinkSysHealth;`
//! rather than re-defining it locally or importing from a sibling provider.

use serde::Serialize;

/// Health status of a LinkSys client, exposed via admin API.
#[derive(Debug, Clone, Serialize)]
pub struct LinkSysHealth {
    pub name: String,
    pub system_type: String,
    pub connected: bool,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}
