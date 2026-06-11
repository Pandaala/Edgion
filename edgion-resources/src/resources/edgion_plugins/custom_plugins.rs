//! Custom Edgion plugin configurations
//!
//! This module is reserved for future custom Edgion edgion_plugins that extend beyond
//! the Gateway API standard plugins.
//!
//! Future custom edgion_plugins will be added here, such as:
//! - EdgionRateLimit(RateLimitConfig) - Rate limiting plugin
//! - EdgionCircuitBreaker(CircuitBreakerConfig) - Circuit breaker plugin
//! - EdgionAuth(AuthConfig) - Authentication plugin
//! - EdgionWaf(WafConfig) - Web application firewall plugin
//! - EdgionCache(CacheConfig) - Caching plugin
//! - EdgionTransform(TransformConfig) - Request/response transformation plugin
//! - EdgionObservability(ObservabilityConfig) - Observability plugin
//! - EdgionCors(CorsConfig) - CORS configuration plugin
//! - EdgionCompression(CompressionConfig) - Compression plugin
//! - etc.
//!
//! # Example placeholder structure
//!
//! ```rust,ignore
//! use schemars::JsonSchema;
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
//! #[serde(rename_all = "camelCase")]
//! pub struct RateLimitConfig {
//!     /// Maximum requests per second
//!     pub requests_per_second: u32,
//!     /// Burst size for rate limiting
//!     pub burst_size: u32,
//!     /// Optional key for rate limit buckets (e.g., IP address, user ID)
//!     #[serde(default, skip_serializing_if = "Option::is_none")]
//!     pub key: Option<String>,
//! }
//! ```
