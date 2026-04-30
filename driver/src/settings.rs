//! `~/.dobj/settings.json` load/save.

use std::fs;

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

use crate::paths::DriverPaths;

const DEFAULT_SYNCHRONIZER_API_URL: &str = "http://127.0.0.1:3000";
const DEFAULT_RELAYER_API_URL: &str = "http://127.0.0.1:3200";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DriverSettings {
    pub synchronizer_api_url: String,
    pub relayer_api_url: String,
}

pub fn default_settings() -> DriverSettings {
    DriverSettings {
        synchronizer_api_url: option_env!("DEFAULT_SYNCHRONIZER_API_URL")
            .unwrap_or(DEFAULT_SYNCHRONIZER_API_URL)
            .to_string(),
        relayer_api_url: option_env!("DEFAULT_RELAYER_API_URL")
            .unwrap_or(DEFAULT_RELAYER_API_URL)
            .to_string(),
    }
}

pub fn read_settings(paths: &DriverPaths) -> Result<Option<DriverSettings>> {
    if !paths.settings_path.exists() {
        return Ok(None);
    }
    let raw = fs::read_to_string(&paths.settings_path)
        .map_err(|e| anyhow!("read {}: {e}", paths.settings_path.display()))?;
    let s: DriverSettings = serde_json::from_str(&raw)
        .map_err(|e| anyhow!("parse {}: {e}", paths.settings_path.display()))?;
    Ok(Some(s))
}

pub fn write_settings(paths: &DriverPaths, settings: &DriverSettings) -> Result<()> {
    let parent = paths
        .settings_path
        .parent()
        .ok_or_else(|| anyhow!("settings path has no parent"))?;
    fs::create_dir_all(parent).map_err(|e| anyhow!("mkdir {}: {e}", parent.display()))?;
    let s = serde_json::to_string_pretty(settings)?;
    fs::write(&paths.settings_path, s)
        .map_err(|e| anyhow!("write {}: {e}", paths.settings_path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn settings_roundtrip() {
        let dir = tempdir().unwrap();
        let p = DriverPaths::from_dobj_root(dir.path());
        let s = default_settings();
        write_settings(&p, &s).unwrap();
        assert_eq!(read_settings(&p).unwrap(), Some(s));
    }

    #[test]
    fn read_missing_returns_none() {
        let dir = tempdir().unwrap();
        let p = DriverPaths::from_dobj_root(dir.path());
        assert_eq!(read_settings(&p).unwrap(), None);
    }
}
