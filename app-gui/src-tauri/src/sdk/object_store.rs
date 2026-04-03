use anyhow::{Result, anyhow};
use std::{fs, path::Path};

use crate::objects::ObjectRecord;

fn parse_object_file(contents: &str, file_name: &str) -> Result<ObjectRecord> {
    serde_json::from_str::<ObjectRecord>(contents)
        .map_err(|err| anyhow!("failed to parse {file_name} as object file: {err}"))
}

pub(crate) fn parse_object_file_from_path(path: &Path) -> Result<ObjectRecord> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow!("invalid input path (missing file name): {}", path.display()))?;
    let contents = fs::read_to_string(path)
        .map_err(|err| anyhow!("failed to read input file {}: {err}", path.display()))?;
    parse_object_file(&contents, file_name)
}
