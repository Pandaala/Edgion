use crate::core::gateway::observe::SysLogConfig;
use anyhow::Result;
use std::path::PathBuf;

/// Logging configuration for Edgion Gateway
#[derive(Debug, Clone)]
pub struct GatewayLogConfig {
    /// Log directory path
    pub log_dir: PathBuf,
    
    /// Log file prefix
    pub file_prefix: String,
    
    /// Whether to use JSON format
    pub json_format: bool,
    
    /// Whether to log to console
    pub console: bool,
    
    /// Log level filter (e.g., "info", "debug", "warn")
    pub level: String,
}

impl GatewayLogConfig {
    /// Create a new log configuration with sensible defaults
    pub fn new(log_dir: Option<PathBuf>) -> Self {
        // Use RUST_LOG environment variable if set, otherwise default to "info"
        let log_level = std::env::var("RUST_LOG").unwrap_or_else(|_| "info".to_string());
        
        // Determine log directory
        let log_dir = log_dir.unwrap_or_else(|| {
            // Default to ./logs relative to current working directory
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join("logs")
        });
        
        Self {
            log_dir,
            file_prefix: "edgion-gateway".to_string(),
            json_format: false,
            console: true,
            level: log_level,
        }
    }
    
    /// Set JSON format
    pub fn with_json_format(mut self, json_format: bool) -> Self {
        self.json_format = json_format;
        self
    }
    
    /// Set console output
    pub fn with_console(mut self, console: bool) -> Self {
        self.console = console;
        self
    }
    
    /// Set log level
    pub fn with_level(mut self, level: String) -> Self {
        self.level = level;
        self
    }
    
    /// Set file prefix
    pub fn with_file_prefix(mut self, prefix: String) -> Self {
        self.file_prefix = prefix;
        self
    }
    
    /// Convert to core SysLogConfig
    pub fn to_log_config(self) -> SysLogConfig {
        SysLogConfig {
            log_dir: self.log_dir,
            file_prefix: self.file_prefix,
            json_format: self.json_format,
            console: self.console,
            level: self.level,
        }
    }
    
    /// Validate the configuration
    pub fn validate(&self) -> Result<()> {
        // Check if we can create the log directory
        if !self.log_dir.exists() {
            std::fs::create_dir_all(&self.log_dir)?;
        }
        
        // Validate log level
        let valid_levels = ["trace", "debug", "info", "warn", "error"];
        let base_level = self.level.split(',').next().unwrap_or(&self.level);
        
        if !valid_levels.iter().any(|&l| base_level.starts_with(l)) {
            tracing::warn!("Unknown log level: {}, using 'info'", self.level);
        }
        
        Ok(())
    }
}

impl Default for GatewayLogConfig {
    fn default() -> Self {
        Self::new(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_default_config() {
        let config = GatewayLogConfig::default();
        assert_eq!(config.file_prefix, "edgion-gateway");
        assert_eq!(config.console, true);
        assert_eq!(config.json_format, false);
    }
    
    #[test]
    fn test_custom_log_dir() {
        let custom_dir = PathBuf::from("/var/log/edgion");
        let config = GatewayLogConfig::new(Some(custom_dir.clone()));
        assert_eq!(config.log_dir, custom_dir);
    }
    
    #[test]
    fn test_builder_pattern() {
        let config = GatewayLogConfig::default()
            .with_json_format(true)
            .with_console(false)
            .with_level("debug".to_string())
            .with_file_prefix("test".to_string());
        
        assert_eq!(config.json_format, true);
        assert_eq!(config.console, false);
        assert_eq!(config.level, "debug");
        assert_eq!(config.file_prefix, "test");
    }
}

