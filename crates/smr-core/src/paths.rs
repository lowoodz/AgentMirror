//! Default config/data directory helpers.

use std::path::PathBuf;

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("securemodelroute")
}

pub fn default_config_path() -> PathBuf {
    config_dir().join("smr.yaml")
}

pub fn ensure_config_dir() -> std::io::Result<PathBuf> {
    let dir = config_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn traffic_dir() -> PathBuf {
    config_dir().join("traffic")
}

pub fn init_default_config(example: &str) -> anyhow::Result<PathBuf> {
    ensure_config_dir()?;
    let path = default_config_path();
    if !path.exists() {
        std::fs::write(&path, example)?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_dir_has_name() {
        assert!(
            config_dir()
                .to_string_lossy()
                .contains("securemodelroute")
        );
    }
}
