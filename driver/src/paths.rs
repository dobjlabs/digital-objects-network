//! Filesystem layout under `~/.dobj`.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

pub const DOBJ_HOME_DIR: &str = ".dobj";
pub const SETTINGS_FILE: &str = "settings.json";
pub const OBJECTS_DIR: &str = "objects";
pub const NULLIFIED_DIR: &str = ".nullified";
pub const DOBJ_EXTENSION: &str = "dobj";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DriverPaths {
    pub settings_path: PathBuf,
    pub objects_dir: PathBuf,
    pub nullified_objects_dir: PathBuf,
}

impl DriverPaths {
    pub fn from_dobj_root(root: impl Into<PathBuf>) -> Self {
        let root = root.into();
        let objects_dir = root.join(OBJECTS_DIR);
        let nullified_objects_dir = objects_dir.join(NULLIFIED_DIR);
        Self {
            settings_path: root.join(SETTINGS_FILE),
            objects_dir,
            nullified_objects_dir,
        }
    }
}

pub fn default_dobj_root() -> Result<PathBuf> {
    let home = dirs::home_dir().ok_or_else(|| anyhow!("failed to resolve home directory"))?;
    Ok(home.join(DOBJ_HOME_DIR))
}

pub fn default_paths() -> Result<DriverPaths> {
    Ok(DriverPaths::from_dobj_root(default_dobj_root()?))
}

pub fn is_dobj_file(path: &Path) -> bool {
    path.extension().and_then(|ext| ext.to_str()) == Some(DOBJ_EXTENSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_dobj_root_layout() {
        let p = DriverPaths::from_dobj_root("/tmp/.dobj");
        assert_eq!(p.settings_path, PathBuf::from("/tmp/.dobj/settings.json"));
        assert_eq!(p.objects_dir, PathBuf::from("/tmp/.dobj/objects"));
        assert_eq!(
            p.nullified_objects_dir,
            PathBuf::from("/tmp/.dobj/objects/.nullified")
        );
    }

    #[test]
    fn is_dobj_file_recognizes_extension() {
        assert!(is_dobj_file(Path::new("foo.dobj")));
        assert!(!is_dobj_file(Path::new("foo.txt")));
        assert!(!is_dobj_file(Path::new("foo")));
    }
}
