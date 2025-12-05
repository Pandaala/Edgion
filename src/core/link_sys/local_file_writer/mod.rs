//! Local file writer implementation of DataSender

use anyhow::Result;
use async_trait::async_trait;
use tokio::sync::mpsc::{self, Sender};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use std::io::Write;

use super::DataSender;
use crate::types::link_sys::{LocalFileWriterConfig, RotationStrategy};
use crate::core::observe::global_metrics;

/// Local file writer that implements DataSender
/// 
/// Uses a background task to write log entries asynchronously
pub struct LocalFileWriter {
    config: LocalFileWriterConfig,
    sender: Option<Sender<String>>,
    healthy: bool,
}

impl LocalFileWriter {
    /// Create a new LocalFileWriter with the given configuration
    pub fn new(config: LocalFileWriterConfig) -> Self {
        Self {
            config,
            sender: None,
            healthy: false,
        }
    }
    
    /// Create with simple path and prefix
    pub fn with_path(path: impl Into<std::path::PathBuf>, prefix: impl Into<String>) -> Self {
        Self::new(LocalFileWriterConfig::new(path, prefix))
    }
}

#[async_trait]
impl DataSender for LocalFileWriter {
    async fn init(&mut self) -> Result<()> {
        // Create log directory if it doesn't exist
        tokio::fs::create_dir_all(&self.config.path).await?;
        
        // Convert rotation strategy to tracing_appender Rotation
        let rotation = match &self.config.rotation.strategy {
            RotationStrategy::Daily => Rotation::DAILY,
            RotationStrategy::Hourly => Rotation::HOURLY,
            RotationStrategy::Never => Rotation::NEVER,
            RotationStrategy::Size(_) => Rotation::DAILY, // tracing_appender doesn't support size-based, fallback to daily
        };
        
        // Create rolling file appender
        let file_appender = RollingFileAppender::new(
            rotation,
            &self.config.path,
            &self.config.file_prefix,
        );
        
        // Create channel for async writes
        let (tx, mut rx) = mpsc::channel::<String>(10_000);
        
        // Spawn background writer task
        tokio::spawn(async move {
            let mut appender = file_appender;
            while let Some(line) = rx.recv().await {
                if let Err(e) = writeln!(appender, "{}", line) {
                    tracing::error!(error = %e, "Failed to write to log file");
                }
            }
        });
        
        self.sender = Some(tx);
        self.healthy = true;
        
        tracing::info!(
            path = %self.config.path.display(),
            prefix = %self.config.file_prefix,
            "LocalFileWriter initialized"
        );
        
        Ok(())
    }
    
    fn healthy(&self) -> bool {
        self.healthy && self.sender.is_some()
    }
    
    async fn send(&self, data: String) -> Result<()> {
        if let Some(sender) = &self.sender {
            // Non-blocking send, drop if channel is full
            if sender.try_send(data).is_err() {
                global_metrics().access_log_dropped();
            }
        }
        Ok(())
    }
    
    fn name(&self) -> &str {
        "local_file_writer"
    }
}

