use anyhow::{Result, anyhow};

use crate::types::DriverPaths;

pub fn default_paths() -> Result<DriverPaths> {
    let root = dirs::home_dir()
        .ok_or_else(|| anyhow!("failed to resolve home directory"))?
        .join(".dobj");
    let settings_path = root.join("settings.json");
    let objects_dir = root.join("objects");
    let nullified_objects_dir = objects_dir.join(".nullified");
    let actions_dir = root.join("actions");
    Ok(DriverPaths {
        settings_path,
        objects_dir,
        nullified_objects_dir,
        actions_dir,
    })
}

#[cfg(test)]
mod tests {
    use super::default_paths;

    #[test]
    fn test_default_paths_shape() {
        let paths = default_paths().unwrap();
        assert!(paths.settings_path.ends_with(".dobj/settings.json"));
        assert!(paths.objects_dir.ends_with(".dobj/objects"));
        assert!(
            paths
                .nullified_objects_dir
                .ends_with(".dobj/objects/.nullified")
        );
        assert!(paths.actions_dir.ends_with(".dobj/actions"));
    }
}
