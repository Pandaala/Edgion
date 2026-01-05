//! DataSender trait implementation for LocalFileWriter

use anyhow::Result;
use async_trait::async_trait;
use std::fs;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

use super::rotation::{
    cleanup_old_files, find_next_size_index, get_rotated_path, get_rotation_key, get_size_rotated_path, open_log_file,
};
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
            let strategy = rotation.strategy.clone();
            let max_files = rotation.max_files;

            // For time-based rotation
            let mut current_key = get_rotation_key(&strategy);
            let rotation_check_interval = Duration::from_secs(rotation.check_interval_secs);
            let mut last_rotation_check = Instant::now();

            // For size-based rotation
            let mut current_size: u64 = 0;
            let mut size_index: u32 = 0;

            // Determine initial file path
            let mut current_path = match &strategy {
                RotationStrategy::Size(_) => {
                    // For size strategy, always start with base file (index 0)
                    // Get existing file size if resuming
                    if base_path.exists() {
                        current_size = fs::metadata(&base_path).map(|m| m.len()).unwrap_or(0);
                    }
                    base_path.clone()
                }
                _ => get_rotated_path(&base_path, &strategy),
            };

            // Open initial file with buffered writer for better performance
            let mut file = match open_log_file(&current_path) {
                Ok(f) => BufWriter::new(f),
                Err(e) => {
                    tracing::error!(error = %e, path = %current_path.display(), "Failed to open log file");
                    return;
                }
            };

            tracing::info!(path = %current_path.display(), "Log file opened");

            // Helper closure to rotate file for size strategy
            let rotate_for_size = |file: &mut BufWriter<std::fs::File>,
                                   current_path: &mut std::path::PathBuf,
                                   current_size: &mut u64,
                                   size_index: &mut u32| {
                let _ = file.flush();
                *size_index = find_next_size_index(&base_path);
                *current_path = get_size_rotated_path(&base_path, *size_index);

                match open_log_file(current_path) {
                    Ok(f) => {
                        *file = BufWriter::new(f);
                        *current_size = 0;
                        tracing::info!(path = %current_path.display(), index = *size_index, "Log file rotated (size)");
                        cleanup_old_files(&base_path, max_files);
                        true
                    }
                    Err(e) => {
                        tracing::error!(error = %e, path = %current_path.display(), "Failed to rotate log file");
                        false
                    }
                }
            };

            // Block on receiving first message, then batch process remaining
            while let Ok(first_line) = rx.recv() {
                // Check rotation at configured interval (most cases skip this block)
                if last_rotation_check.elapsed() >= rotation_check_interval {
                    last_rotation_check = Instant::now();

                    match &strategy {
                        RotationStrategy::Never => {}
                        RotationStrategy::Size(max_size) => {
                            if current_size >= *max_size {
                                rotate_for_size(&mut file, &mut current_path, &mut current_size, &mut size_index);
                            }
                        }
                        RotationStrategy::Daily | RotationStrategy::Hourly => {
                            let new_key = get_rotation_key(&strategy);
                            if new_key != current_key {
                                let _ = file.flush();
                                current_key = new_key;
                                current_path = get_rotated_path(&base_path, &strategy);

                                match open_log_file(&current_path) {
                                    Ok(f) => {
                                        file = BufWriter::new(f);
                                        tracing::info!(path = %current_path.display(), "Log file rotated");
                                        cleanup_old_files(&base_path, max_files);
                                    }
                                    Err(e) => {
                                        tracing::error!(error = %e, path = %current_path.display(), "Failed to rotate log file");
                                    }
                                }
                            }
                        }
                    }
                }

                // Write first line and track size
                let bytes_written = first_line.len() as u64 + 1; // +1 for newline
                let _ = writeln!(file, "{}", first_line);
                current_size += bytes_written;

                // Batch: drain available messages (max 999, total 1000 with first_line)
                for _ in 0..999 {
                    match rx.try_recv() {
                        Ok(line) => {
                            let bytes = line.len() as u64 + 1;
                            let _ = writeln!(file, "{}", line);
                            current_size += bytes;
                        }
                        Err(_) => break,
                    }
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
