//! DataSender trait implementation for LocalFileWriter

use anyhow::Result;
use async_trait::async_trait;
use std::fs;
use std::io::{BufWriter, Write};

use super::rotation::{cleanup_old_files, get_rotated_path, get_rotation_key, open_log_file};
use super::LocalFileWriter;
use crate::core::link_sys::DataSender;
use crate::core::observe::global_metrics;
use crate::types::link_sys::RotationStrategy;

#[async_trait]
impl DataSender<String> for LocalFileWriter {
    async fn init(&mut self) -> Result<()> {
        let base_path = self.full_path();
        let rotation = self.rotation.clone();
        
        // Create parent directory if it doesn't exist
        if let Some(parent) = base_path.parent() {
            fs::create_dir_all(parent)?;
        }
        
        // Create bounded sync channel for writes
        let queue_size = self.get_queue_size();
        let (tx, rx) = std::sync::mpsc::sync_channel::<String>(queue_size);
        
        // Spawn background thread for file writes (avoids blocking tokio runtime)
        std::thread::spawn(move || {
            let strategy = &rotation.strategy;
            let max_files = rotation.max_files;
            let mut current_key = get_rotation_key(strategy);
            let mut current_path = get_rotated_path(&base_path, strategy);
            
            // Open initial file with buffered writer for better performance
            let mut file = match open_log_file(&current_path) {
                Ok(f) => BufWriter::new(f),
                Err(e) => {
                    tracing::error!(error = %e, path = %current_path.display(), "Failed to open log file");
                    return;
                }
            };
            
            tracing::info!(path = %current_path.display(), "Log file opened");
            
            // Block on receiving first message, then batch process remaining
            while let Ok(first_line) = rx.recv() {
                // Check if rotation needed before batch write (skip for Never strategy)
                if *strategy != RotationStrategy::Never {
                    let new_key = get_rotation_key(strategy);
                    if new_key != current_key {
                        // Flush before rotation to ensure data is written to old file
                        let _ = file.flush();
                        
                        current_key = new_key;
                        current_path = get_rotated_path(&base_path, strategy);
                        
                        match open_log_file(&current_path) {
                            Ok(f) => {
                                file = BufWriter::new(f);
                                tracing::info!(path = %current_path.display(), "Log file rotated");
                                
                                // Cleanup old files after rotation
                                cleanup_old_files(&base_path, max_files);
                            }
                            Err(e) => {
                                tracing::error!(error = %e, path = %current_path.display(), "Failed to rotate log file");
                                // Continue using old file
                            }
                        }
                    }
                }
                
                // Write first line
                let _ = writeln!(file, "{}", first_line);
                
                // Batch: drain all available messages without blocking
                while let Ok(line) = rx.try_recv() {
                    let _ = writeln!(file, "{}", line);
                }
                
                // Flush once after batch write
                if let Err(e) = file.flush() {
                    tracing::error!(error = %e, "Failed to flush log file");
                }
            }
        });
        
        self.sender = Some(tx);
        self.healthy = true;
        
        tracing::info!(
            path = %self.full_path().display(), 
            rotation = ?self.rotation.strategy,
            max_files = self.rotation.max_files,
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
        "local_file"
    }
}
