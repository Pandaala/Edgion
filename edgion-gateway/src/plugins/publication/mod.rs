//! Canonical publication of scope policy and compatible executable views.

pub(crate) mod inputs;
mod publisher;
mod views;

pub(crate) use publisher::{global_publisher, same_source, PluginPublisher};
pub(crate) use views::{
    ConnectionView, GatewayScope, HttpListenerPublication, PluginPublication, PluginScope, PublicationInventory,
    PublicationStats,
};

#[cfg(test)]
#[path = "../../../tests/unit/plugins/publication/mod_test.rs"]
mod tests;
