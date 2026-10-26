//! This computer's LetsWritePaper profile.
//!
//! The desktop setup wizard writes `config.toml` here. The display name is
//! what edits are attributed to when `PAPER_AUTHOR` is unset. Helper programs
//! the wizard installs (tectonic) live in [`bin_dir`], which `paper-compile`
//! searches before the system `PATH`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub setup_done: bool,
}

/// `{config}/letswritepaper`, or `PAPER_CONFIG_DIR` when set.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("PAPER_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("letswritepaper")
}

/// `{local data}/letswritepaper/bin`. The setup wizard installs tectonic here
/// so a desktop launch, which often has a short `PATH`, can still find it.
pub fn bin_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("PAPER_BIN_DIR") {
        return PathBuf::from(dir);
    }
    dirs::data_local_dir()
        .or_else(dirs::config_dir)
        .unwrap_or_else(std::env::temp_dir)
        .join("letswritepaper")
        .join("bin")
}

pub fn load() -> UserConfig {
    load_from(&config_dir())
}

pub fn load_from(dir: &Path) -> UserConfig {
    std::fs::read_to_string(dir.join("config.toml"))
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

pub fn save(config: &UserConfig) -> Result<()> {
    save_to(&config_dir(), config)
}

pub fn save_to(dir: &Path, config: &UserConfig) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let text = toml::to_string_pretty(config).map_err(|e| Error::Invalid(e.to_string()))?;
    std::fs::write(dir.join("config.toml"), text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        assert!(load_from(dir.path()).name.is_empty());
        let config = UserConfig {
            name: "Ada Lovelace".into(),
            setup_done: true,
        };
        save_to(dir.path(), &config).unwrap();
        assert_eq!(load_from(dir.path()), config);
    }

    #[test]
    fn corrupt_file_is_a_blank_profile() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "not toml :::").unwrap();
        assert_eq!(load_from(dir.path()), UserConfig::default());
    }
}
