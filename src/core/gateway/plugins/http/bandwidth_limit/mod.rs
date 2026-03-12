//! BandwidthLimit plugin module
//!
//! Limits downstream response bandwidth by throttling body chunk delivery.
//! Uses Pingora's upstream_response_body_filter return value (Option<Duration>)
//! to control the rate of body chunk transmission.

mod plugin;

pub use plugin::BandwidthLimit;
