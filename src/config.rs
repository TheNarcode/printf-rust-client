use std::fs::File;
use std::path::PathBuf;

use crate::types::Config;

/// Reads the application config from the platform-standard config directory.
///
/// Windows path: `%LOCALAPPDATA%\printf\config.json`
/// macOS/Linux:  `~/.config/printf/config.json`
pub fn read_config() -> Result<Config, Box<dyn std::error::Error + Send + Sync>> {
    let path = get_config_path()?;
    let file = File::open(&path)
        .map_err(|e| format!("Cannot open config file at {}: {}", path.display(), e))?;
    serde_json::from_reader(file)
        .map_err(|e| format!("Invalid config JSON in {}: {}", path.display(), e).into())
}

/// Returns the absolute path to the config file.
pub fn get_config_path() -> Result<PathBuf, Box<dyn std::error::Error + Send + Sync>> {
    dirs::config_local_dir()
        .ok_or_else(|| "Cannot determine the local config directory for this platform".into())
        .map(|base| base.join("printf").join("config.json"))
}


