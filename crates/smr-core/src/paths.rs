//! Default config/data directory helpers.

use std::path::PathBuf;

pub fn config_dir() -> PathBuf {
    if let Ok(override_dir) = std::env::var("SMR_CONFIG_DIR") {
        let trimmed = override_dir.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
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

pub fn insight_graphs_dir() -> PathBuf {
    config_dir().join("data").join("insight").join("graphs")
}

pub fn data_dir() -> PathBuf {
    config_dir().join("data")
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
    fn config_dir_honors_smr_config_dir_override() {
        let prior = std::env::var("SMR_CONFIG_DIR").ok();
        let tmp = tempfile::TempDir::new().unwrap();
        std::env::set_var("SMR_CONFIG_DIR", tmp.path());
        assert_eq!(config_dir(), tmp.path());
        match prior {
            Some(v) => std::env::set_var("SMR_CONFIG_DIR", v),
            None => std::env::remove_var("SMR_CONFIG_DIR"),
        }
    }

    #[test]
    fn config_dir_has_name() {
        assert!(
            config_dir()
                .to_string_lossy()
                .contains("securemodelroute")
        );
    }
}
