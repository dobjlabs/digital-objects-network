use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};

use crate::types::DriverPaths;

/// Top-level directory under the user's home that the driver owns.
pub const DOBJ_HOME_DIR: &str = ".dobj";

/// File name (inside [`DOBJ_HOME_DIR`]) holding persisted driver settings.
pub const SETTINGS_FILE: &str = "settings.json";

/// Subdirectory (inside [`DOBJ_HOME_DIR`]) holding live `.dobj` objects.
pub const OBJECTS_DIR: &str = "objects";

/// Subdirectory (inside [`OBJECTS_DIR`]) holding already-nullified objects.
pub const NULLIFIED_DIR: &str = ".nullified";

/// Subdirectory (inside [`DOBJ_HOME_DIR`]) holding installed `.pexe` plugins.
pub const ACTIONS_DIR: &str = "actions";

/// File extension (no leading dot) of a digital-object file.
pub const DOBJ_EXTENSION: &str = "dobj";

impl DriverPaths {
    /// Build the full layout rooted at `root` (e.g. `~/.dobj`). This is the
    /// single source of truth for where everything lives; every other
    /// constructor in the crate routes through here.
    pub fn from_dobj_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let objects_dir = root.join(OBJECTS_DIR);
        let nullified_objects_dir = objects_dir.join(NULLIFIED_DIR);
        Self {
            settings_path: root.join(SETTINGS_FILE),
            objects_dir,
            nullified_objects_dir,
            actions_dir: root.join(ACTIONS_DIR),
        }
    }
}

/// The `.dobj` directory under the user's home.
pub fn default_dobj_root() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("failed to resolve home directory"))?;
    Ok(home.join(DOBJ_HOME_DIR))
}

pub fn default_paths() -> Result<DriverPaths> {
    Ok(DriverPaths::from_dobj_root(default_dobj_root()?))
}

/// Directory the driver scans for installed `.pexe` plugins. Used by the
/// packaging CLI to default its `--install` target.
pub fn default_install_dir() -> Result<PathBuf> {
    Ok(default_dobj_root()?.join(ACTIONS_DIR))
}

/// True if `path`'s extension is `.dobj`.
pub fn is_dobj_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some(DOBJ_EXTENSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_paths_shape() {
        let paths = default_paths().unwrap();
        let expect_settings = format!("{DOBJ_HOME_DIR}/{SETTINGS_FILE}");
        let expect_objects = format!("{DOBJ_HOME_DIR}/{OBJECTS_DIR}");
        let expect_nullified = format!("{DOBJ_HOME_DIR}/{OBJECTS_DIR}/{NULLIFIED_DIR}");
        let expect_actions = format!("{DOBJ_HOME_DIR}/{ACTIONS_DIR}");
        assert!(paths.settings_path.ends_with(&expect_settings));
        assert!(paths.objects_dir.ends_with(&expect_objects));
        assert!(paths.nullified_objects_dir.ends_with(&expect_nullified));
        assert!(paths.actions_dir.ends_with(&expect_actions));
    }

    #[test]
    fn test_from_dobj_root_layout() {
        let paths = DriverPaths::from_dobj_root("/tmp/fake-home/.dobj");
        assert_eq!(
            paths.settings_path,
            PathBuf::from("/tmp/fake-home/.dobj").join(SETTINGS_FILE)
        );
        assert_eq!(
            paths.objects_dir,
            PathBuf::from("/tmp/fake-home/.dobj").join(OBJECTS_DIR)
        );
        assert_eq!(
            paths.nullified_objects_dir,
            PathBuf::from("/tmp/fake-home/.dobj")
                .join(OBJECTS_DIR)
                .join(NULLIFIED_DIR)
        );
        assert_eq!(
            paths.actions_dir,
            PathBuf::from("/tmp/fake-home/.dobj").join(ACTIONS_DIR)
        );
    }

    #[test]
    fn test_is_dobj_file() {
        assert!(is_dobj_file(Path::new("foo.dobj")));
        assert!(!is_dobj_file(Path::new("foo.pexe")));
        assert!(!is_dobj_file(Path::new("foo")));
    }
}
