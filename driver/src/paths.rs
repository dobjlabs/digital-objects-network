use anyhow::{Result, anyhow};

use crate::types::DriverPaths;

const APP_IDENTIFIER: &str = "com.dobjlabs.zk-craft";

pub fn default_paths() -> Result<DriverPaths> {
    let settings_path = dirs::config_dir()
        .ok_or_else(|| anyhow!("failed to resolve config directory"))?
        .join(APP_IDENTIFIER)
        .join("settings.json");
    let objects_dir = dirs::home_dir()
        .ok_or_else(|| anyhow!("failed to resolve home directory"))?
        .join(".objects");
    let nullified_objects_dir = objects_dir.join(".nullified");
    Ok(DriverPaths {
        settings_path,
        objects_dir,
        nullified_objects_dir,
    })
}

#[cfg(test)]
mod tests {
    use super::default_paths;

    #[test]
    fn test_default_paths_shape() {
        let paths = default_paths().unwrap();
        assert!(paths.settings_path.ends_with("com.dobjlabs.zk-craft/settings.json"));
        assert!(paths.objects_dir.ends_with(".objects"));
        assert!(paths.nullified_objects_dir.ends_with(".objects/.nullified"));
    }
}
