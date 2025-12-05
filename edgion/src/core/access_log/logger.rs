//! Access logger with pluggable sinks
//!
//! Provides a queue-based logger that dispatches log entries to registered sinks.

use std::sync::Arc;
use tokio::sync::mpsc::{self, Sender, Receiver};
use crate::core::misc::LogSink;

/// Default queue capacity (10,000 entries)
pub const DEFAULT_QUEUE_CAPACITY: usize = 10_000;

/// Access logger that dispatches log entries to registered sinks
pub struct AccessLogger {
    /// Sender for the log queue
    sender: Sender<String>,
    /// Registered sinks
    sinks: Vec<Arc<dyn LogSink>>,
}

impl AccessLogger {
    /// Create a new AccessLogger with default queue capacity
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_QUEUE_CAPACITY)
    }

    /// Create a new AccessLogger with specified queue capacity
    pub fn with_capacity(capacity: usize) -> Self {
        let (sender, _receiver) = mpsc::channel(capacity);
        Self {
            sender,
            sinks: Vec::new(),
        }
    }

    /// Register a log sink
    pub fn register_sink(&mut self, sink: Arc<dyn LogSink>) {
        self.sinks.push(sink);
    }

    /// Get the sender for sending log entries
    pub fn sender(&self) -> Sender<String> {
        self.sender.clone()
    }

    /// Log a formatted string (non-blocking)
    pub fn log(&self, line: String) {
        // Use try_send to avoid blocking
        if let Err(e) = self.sender.try_send(line) {
            tracing::warn!("Access log queue full, dropping entry: {}", e);
        }
    }

    /// Start the logger background task
    /// 
    /// Returns a handle that can be used to stop the logger
    pub fn start(self) -> AccessLoggerHandle {
        let (sender, receiver) = mpsc::channel(DEFAULT_QUEUE_CAPACITY);
        let sinks = self.sinks;
        
        let handle = tokio::spawn(async move {
            Self::run_loop(receiver, sinks).await;
        });

        AccessLoggerHandle {
            sender,
            _handle: handle,
        }
    }

    /// Background loop that processes log entries
    async fn run_loop(mut receiver: Receiver<String>, sinks: Vec<Arc<dyn LogSink>>) {
        while let Some(line) = receiver.recv().await {
            for sink in &sinks {
                if let Err(e) = sink.write(&line).await {
                    tracing::error!(sink = sink.name(), error = %e, "Failed to write access log");
                }
            }
        }
        
        // Flush all sinks on shutdown
        for sink in &sinks {
            if let Err(e) = sink.flush().await {
                tracing::error!(sink = sink.name(), error = %e, "Failed to flush access log sink");
            }
        }
    }
}

impl Default for AccessLogger {
    fn default() -> Self {
        Self::new()
    }
}

/// Handle for a running AccessLogger
pub struct AccessLoggerHandle {
    /// Sender for log entries
    pub sender: Sender<String>,
    /// Background task handle
    _handle: tokio::task::JoinHandle<()>,
}

impl AccessLoggerHandle {
    /// Log a formatted string (non-blocking)
    pub fn log(&self, line: String) {
        if let Err(e) = self.sender.try_send(line) {
            tracing::warn!("Access log queue full, dropping entry: {}", e);
        }
    }

    /// Log a formatted string (async, waits if queue is full)
    pub async fn log_async(&self, line: String) {
        if let Err(e) = self.sender.send(line).await {
            tracing::error!("Access log channel closed: {}", e);
        }
    }
}

