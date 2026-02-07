//! RateLimit plugin module
//!
//! Rate limiting using Pingora's Count-Min Sketch (CMS) algorithm.

mod plugin;

pub use plugin::RateLimit;
