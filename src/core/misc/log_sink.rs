//! Log sink trait definition
//!
//! Provides a generic trait for log destinations.

use async_trait::async_trait;

/// Log sink trait for pluggable log destinations
///
/// Implement this trait to create custom log sinks (file, network, etc.)
///
/// # Example
///
/// ```ignore
/// struct MyLogSink { ... }
///
/// #[async_trait]
/// impl LogSink for MyLogSink {
///     fn name(&self) -> &str { "my-sink" }
///     async fn connect(&mut self) -> Result<(), String> { Ok(()) }
///     async fn write(&self, line: &str) -> Result<(), String> { ... }
///     async fn flush(&self) -> Result<(), String> { Ok(()) }
///     async fn close(&self) -> Result<(), String> { Ok(()) }
/// }
/// ```
#[async_trait]
pub trait LogSink: Send + Sync {
    /// Sink name for identification and logging
    fn name(&self) -> &str;
    
    /// Connect/initialize the sink
    ///
    /// Called once before any write operations.
    /// Use this to open files, establish connections, etc.
    async fn connect(&mut self) -> Result<(), String>;
    
    /// Write a single log line
    ///
    /// The line does not include a trailing newline.
    async fn write(&self, line: &str) -> Result<(), String>;
    
    /// Flush any buffered data
    ///
    /// Called periodically or before shutdown.
    async fn flush(&self) -> Result<(), String>;
    
    /// Close the sink and release resources
    ///
    /// Called on shutdown. After this, no more writes will occur.
    async fn close(&self) -> Result<(), String>;
}

