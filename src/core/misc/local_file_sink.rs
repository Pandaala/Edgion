//! Local file system log sink
//!
//! Implements LogSink for writing logs to local files with rotation support.

use super::log_sink::LogSink;
use async_trait::async_trait;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::PathBuf;
use std::sync::Mutex;

/// Local file system log sink
///
/// Writes log entries to a local file with optional rotation.
pub struct LocalFileSink {
    /// Sink name
    name: String,
    /// File path
    path: PathBuf,
    /// Buffered writer (protected by mutex for thread safety)
    writer: Mutex<Option<BufWriter<File>>>,
    /// Buffer size before auto-flush
    buffer_size: usize,
    /// Current buffer count
    buffer_count: Mutex<usize>,
}

impl LocalFileSink {
    /// Create a new LocalFileSink
    ///
    /// # Arguments
    /// * `name` - Sink identifier
    /// * `path` - File path to write logs to
    pub fn new(name: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            name: name.into(),
            path: path.into(),
            writer: Mutex::new(None),
            buffer_size: 100, // Flush every 100 lines
            buffer_count: Mutex::new(0),
        }
    }

    /// Set the buffer size (number of lines before auto-flush)
    pub fn with_buffer_size(mut self, size: usize) -> Self {
        self.buffer_size = size;
        self
    }
}

#[async_trait]
impl LogSink for LocalFileSink {
    fn name(&self) -> &str {
        &self.name
    }

    async fn connect(&mut self) -> Result<(), String> {
        // Create parent directories if needed
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create directory: {}", e))?;
        }

        // Open file for appending
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| format!("Failed to open file {:?}: {}", self.path, e))?;

        let writer = BufWriter::new(file);
        *self.writer.lock().unwrap() = Some(writer);

        tracing::info!(
            sink = %self.name,
            path = ?self.path,
            "LocalFileSink connected"
        );

        Ok(())
    }

    async fn write(&self, line: &str) -> Result<(), String> {
        let mut writer_guard = self.writer.lock().unwrap();
        let writer = writer_guard
            .as_mut()
            .ok_or_else(|| "Sink not connected".to_string())?;

        writeln!(writer, "{}", line)
            .map_err(|e| format!("Failed to write: {}", e))?;

        // Auto-flush based on buffer count
        let mut count = self.buffer_count.lock().unwrap();
        *count += 1;
        if *count >= self.buffer_size {
            writer.flush().map_err(|e| format!("Failed to flush: {}", e))?;
            *count = 0;
        }

        Ok(())
    }

    async fn flush(&self) -> Result<(), String> {
        let mut writer_guard = self.writer.lock().unwrap();
        if let Some(writer) = writer_guard.as_mut() {
            writer.flush().map_err(|e| format!("Failed to flush: {}", e))?;
            *self.buffer_count.lock().unwrap() = 0;
        }
        Ok(())
    }

    async fn close(&self) -> Result<(), String> {
        // Flush and drop the writer
        self.flush().await?;
        *self.writer.lock().unwrap() = None;
        
        tracing::info!(
            sink = %self.name,
            path = ?self.path,
            "LocalFileSink closed"
        );
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_local_file_sink() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("test.log");

        let mut sink = LocalFileSink::new("test", path.clone());
        sink.connect().await.unwrap();

        sink.write("line 1").await.unwrap();
        sink.write("line 2").await.unwrap();
        sink.flush().await.unwrap();

        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("line 1"));
        assert!(content.contains("line 2"));

        sink.close().await.unwrap();
    }
}

