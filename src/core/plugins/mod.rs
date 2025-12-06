//! Plugin store module for EdgionPlugins resources

mod plugin_store;
mod conf_handler_impl;

pub use plugin_store::{get_global_plugin_store, PluginStore};
pub use conf_handler_impl::create_plugin_handler;

