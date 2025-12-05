//! Local file writer implementation of DataSender

use anyhow::Result;
use async_trait::async_trait;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use tokio::sync::mpsc::{self, Sender};

use super::DataSender;
use crate::core::observe::global_metrics;
use crate::types::link_sys::LocalFileWriterConfig;
use crate::types::prefix_dir;

/// Local file writer that implements DataSender
/// 
/// Uses a background task to write log entries asynchronously
pub struct LocalFileWriter {
    /// Relative path (will be joined with prefix_dir)
    relative_path: String,
    sender: Option<Sender<String>>,
    healthy: bool,
}

impl LocalFileWriter {
    /// Create a new LocalFileWriter with the given configuration
    pub fn new(config: LocalFileWriterConfig) -> Self {
        Self {
            relative_path: config.path,
            sender: None,
            healthy: false,
        }
    }
    
    /// Create with simple relative path
    pub fn with_path(path: impl Into<String>) -> Self {
        Self {
            relative_path: path.into(),
            sender: None,
            healthy: false,
        }
    }
    
    /// Get full path by joining prefix_dir with relative path
    fn full_path(&self) -> PathBuf {
        prefix_dir().join(&self.relative_path)
    }
}

#[async_trait]
impl DataSender for LocalFileWriter {
    async fn init(&mut self) -> Result<()> {
        let full_path = self.full_path();
        
        // Create parent directory if it doesn't exist
        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        
        // Create channel for async writes
        let (tx, mut rx) = mpsc::channel::<String>(10_000);
        
        // Spawn background writer task
        let path_for_task = full_path.clone();
        tokio::spawn(async move {
            // Open file in append mode
            let file = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path_for_task);
            
            let mut file = match file {
                Ok(f) => f,
                Err(e) => {
                    tracing::error!(error = %e, path = %path_for_task.display(), "Failed to open log file");
                    return;
                }
            };
            
            while let Some(line) = rx.recv().await {
                if let Err(e) = writeln!(file, "{}", line) {
                    tracing::error!(error = %e, "Failed to write to log file");
                }
            }
        });
        
        self.sender = Some(tx);
        self.healthy = true;
        
        tracing::info!(path = %full_path.display(), "LocalFileWriter initialized");
        
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

