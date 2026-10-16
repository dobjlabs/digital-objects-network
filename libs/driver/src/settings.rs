use std::{fs, io::ErrorKind};

use anyhow::{Context, Result, anyhow};

use crate::types::DriverPaths;
use wire_types::{DriverSettings, DriverSettingsPatch};

const DEFAULT_SYNCHRONIZER_API_URL: &str = "http://127.0.0.1:3000";
const DEFAULT_RELAYER_API_URL: &str = "http://127.0.0.1:3200";

fn default_settings() -> DriverSettings {
    DriverSettings {
        synchronizer_api_url: option_env!("DEFAULT_SYNCHRONIZER_API_URL")
            .unwrap_or(DEFAULT_SYNCHRONIZER_API_URL)
            .to_string(),
        relayer_api_url: option_env!("DEFAULT_RELAYER_API_URL")
            .unwrap_or(DEFAULT_RELAYER_API_URL)
            .to_string(),
        mcp_enabled: false,
    }
}

pub fn read_settings(paths: &DriverPaths) -> Result<DriverSettings> {
    read_settings_with_defaults(paths, default_settings())
}

fn read_settings_with_defaults(
    paths: &DriverPaths,
    mut defaults: DriverSettings,
) -> Result<DriverSettings> {
    let raw = match fs::read_to_string(&paths.settings_path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(defaults),
        Err(err) => {
            return Err(err).with_context(|| {
                format!(
                    "failed to read settings file {}",
                    paths.settings_path.display()
                )
            });
        }
    };
    let overrides = serde_json::from_str::<DriverSettingsPatch>(&raw).map_err(|err| {
        anyhow!(
            "failed to parse settings file {}: {err}",
            paths.settings_path.display()
        )
    })?;
    overrides.apply_to(&mut defaults);
    Ok(defaults)
}

pub fn write_settings(paths: &DriverPaths, settings: &DriverSettings) -> Result<()> {
    write_settings_with_defaults(paths, settings, &default_settings())
}

fn write_settings_with_defaults(
    paths: &DriverPaths,
    settings: &DriverSettings,
    defaults: &DriverSettings,
) -> Result<()> {
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
    let overrides = DriverSettingsPatch {
        synchronizer_api_url: (settings.synchronizer_api_url != defaults.synchronizer_api_url)
            .then(|| settings.synchronizer_api_url.clone()),
        relayer_api_url: (settings.relayer_api_url != defaults.relayer_api_url)
            .then(|| settings.relayer_api_url.clone()),
        mcp_enabled: (settings.mcp_enabled != defaults.mcp_enabled).then_some(settings.mcp_enabled),
    };
    let serialized = serde_json::to_string(&overrides)
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

    use super::{
        default_settings, read_settings, read_settings_with_defaults, write_settings,
        write_settings_with_defaults,
    };
    use crate::types::DriverPaths;
    use std::fs;
    use wire_types::DriverSettings;

    fn temp_paths() -> DriverPaths {
        let dir = tempdir().unwrap();
        DriverPaths::from_dobj_root(dir.keep())
    }

    #[test]
    fn test_settings_round_trip() {
        let paths = temp_paths();
        let mut settings = default_settings();
        settings.synchronizer_api_url = "https://custom.example".into();
        settings.mcp_enabled = true;
        write_settings(&paths, &settings).unwrap();
        assert_eq!(read_settings(&paths).unwrap(), settings);
    }

    #[test]
    fn test_read_missing_settings() {
        let paths = temp_paths();
        assert_eq!(read_settings(&paths).unwrap(), default_settings());
        assert!(!paths.settings_path.exists());
    }

    #[test]
    fn updates_inherit_new_defaults_without_pinning_unchanged_fields() {
        let paths = temp_paths();
        let defaults = default_settings();
        let mut settings = defaults.clone();
        settings.mcp_enabled = true;
        write_settings_with_defaults(&paths, &settings, &defaults).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(
                &fs::read_to_string(&paths.settings_path).unwrap()
            )
            .unwrap(),
            serde_json::json!({"mcpEnabled": true})
        );

        let updated = DriverSettings {
            synchronizer_api_url: "https://new-sync.example".into(),
            relayer_api_url: "https://new-relay.example".into(),
            mcp_enabled: false,
        };
        let mut expected = updated.clone();
        expected.mcp_enabled = true;
        assert_eq!(
            read_settings_with_defaults(&paths, updated).unwrap(),
            expected
        );
    }

    #[test]
    fn custom_endpoint_survives_default_changes_and_deletion_restores_defaults() {
        let paths = temp_paths();
        fs::write(
            &paths.settings_path,
            r#"{"synchronizerApiUrl":"https://custom.example"}"#,
        )
        .unwrap();
        let defaults = DriverSettings {
            synchronizer_api_url: "https://hosted-sync.example".into(),
            relayer_api_url: "https://hosted-relay.example".into(),
            mcp_enabled: false,
        };
        let settings = read_settings_with_defaults(&paths, defaults.clone()).unwrap();
        assert_eq!(settings.synchronizer_api_url, "https://custom.example");
        assert_eq!(settings.relayer_api_url, defaults.relayer_api_url);
        fs::remove_file(&paths.settings_path).unwrap();
        assert_eq!(
            read_settings_with_defaults(&paths, defaults.clone()).unwrap(),
            defaults
        );
        assert!(!paths.settings_path.exists());
    }

    #[test]
    fn saving_defaults_clears_overrides() {
        let paths = temp_paths();
        fs::write(&paths.settings_path, r#"{"mcpEnabled":true}"#).unwrap();
        let defaults = default_settings();
        write_settings(&paths, &defaults).unwrap();
        assert_eq!(fs::read_to_string(&paths.settings_path).unwrap(), "{}");
        assert_eq!(read_settings(&paths).unwrap(), defaults);
    }

    #[test]
    fn invalid_overrides_do_not_fall_back_to_defaults() {
        let paths = temp_paths();
        fs::write(&paths.settings_path, r#"{"mcpEnabled":"yes"}"#).unwrap();
        assert!(read_settings(&paths).is_err());
    }
}
