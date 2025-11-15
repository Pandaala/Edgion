use anyhow::Result;
use std::path::PathBuf;
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

/// Async log writer that sends log messages to a channel
#[derive(Clone)]
pub struct AsyncLogWriter {
    tx: mpsc::Sender<String>,
}

impl AsyncLogWriter {
    pub fn new(tx: mpsc::Sender<String>) -> Self {
        Self { tx }
    }

    /// Write a log line asynchronously
    pub async fn write(&self, line: String) {
        // Ignore send errors (channel closed)
        let _ = self.tx.send(line).await;
    }
}

/// Background worker that handles actual file writing with rotation
///
/// Features:
/// - Daily log rotation
/// - Non-blocking async writes
/// - Automatic file creation and directory management
pub async fn log_worker(
    mut rx: mpsc::Receiver<String>,
    log_dir: PathBuf,
    file_prefix: String,
) -> Result<()> {
    // Ensure log directory exists
    tokio::fs::create_dir_all(&log_dir).await?;

    // Build log file path
    let log_file_name = format!("{}.log", file_prefix);
    let log_path = log_dir.join(&log_file_name);

    // Open log file for appending
    let mut file = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .await?;

    // Track current date for rotation detection
    let mut current_date = chrono::Local::now().date_naive();

    while let Some(line) = rx.recv().await {
        // Check if we need to rotate (date changed)
        let now_date = chrono::Local::now().date_naive();
        if now_date != current_date {
            current_date = now_date;
            
            // Archive old log file with date suffix
            let old_log_path = log_dir.join(format!(
                "{}.{}.log",
                file_prefix,
                current_date.format("%Y-%m-%d")
            ));
            
            // Rename current log to dated log
            if let Err(e) = tokio::fs::rename(&log_path, &old_log_path).await {
                eprintln!("Failed to rotate log file: {}", e);
            }
            
            // Open new log file
            file = tokio::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .await?;
        }

        // Write log line
        if let Err(e) = file.write_all(line.as_bytes()).await {
            eprintln!("Failed to write log: {}", e);
            continue;
        }

        // Write newline
        if let Err(e) = file.write_all(b"\n").await {
            eprintln!("Failed to write newline: {}", e);
            continue;
        }

        // Flush periodically for important logs
        if line.contains("ERROR") || line.contains("WARN") {
            let _ = file.flush().await;
        }
    }

    // Final flush before shutdown
    let _ = file.flush().await;

    Ok(())
}
