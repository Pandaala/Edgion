//! Canonical publication of scope policy and compatible executable views.

pub(crate) mod inputs;
mod publisher;
mod views;

pub(crate) use publisher::{global_publisher, PluginPublisher};
pub(crate) use views::{
    ConnectionView, GatewayScope, PluginPublication, PluginScope, PublicationInventory, PublicationStats,
};

#[cfg(test)]
mod tests;
