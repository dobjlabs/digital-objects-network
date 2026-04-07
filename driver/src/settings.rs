use std::fs;

use anyhow::{Result, anyhow};

use crate::types::{DriverPaths, DriverSettings};

const DEFAULT_SYNCHRONIZER_API_URL: &str = "http://127.0.0.1:3000";
const DEFAULT_RELAYER_API_URL: &str = "http://127.0.0.1:3200";

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
    let raw = fs::read_to_string(&paths.settings_path).map_err(|err| {
        anyhow!(
            "failed to read settings file {}: {err}",
            paths.settings_path.display()
        )
    })?;
    let settings = serde_json::from_str::<DriverSettings>(&raw).map_err(|err| {
        anyhow!(
            "failed to parse settings file {}: {err}",
            paths.settings_path.display()
        )
    })?;
    Ok(Some(settings))
}

pub fn write_settings(paths: &DriverPaths, settings: &DriverSettings) -> Result<()> {
    let settings_dir = paths.settings_path.parent().ok_or_else(|| {
        anyhow!(
            "settings path has no parent directory: {}",
            paths.settings_path.display()
        )
    })?;
    fs::create_dir_all(settings_dir).map_err(|err| {
        anyhow!(
            "failed to create settings directory {}: {err}",
            settings_dir.display()
        )
    })?;
    let serialized = serde_json::to_string(settings)
        .map_err(|err| anyhow!("failed to serialize settings: {err}"))?;
    fs::write(&paths.settings_path, serialized).map_err(|err| {
        anyhow!(
            "failed to write settings file {}: {err}",
            paths.settings_path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::{default_settings, read_settings, write_settings};
    use crate::types::DriverPaths;

    fn temp_paths() -> DriverPaths {
        let dir = tempdir().unwrap();
        let root = dir.keep();
        let settings_path = root.join("settings.json");
        let objects_dir = root.join("objects");
        let nullified_objects_dir = objects_dir.join(".nullified");
        DriverPaths {
            settings_path,
            objects_dir,
            nullified_objects_dir,
        }
    }

    #[test]
    fn test_settings_round_trip() {
        let paths = temp_paths();
        let settings = default_settings();
        write_settings(&paths, &settings).unwrap();
        assert_eq!(read_settings(&paths).unwrap(), Some(settings));
    }

    #[test]
    fn test_read_missing_settings() {
        let paths = temp_paths();
        assert_eq!(read_settings(&paths).unwrap(), None);
    }
}
