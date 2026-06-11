//! File-backed store for user-authored commands ("macros"): named instruction
//! blocks the player defines at runtime via the `define_command` tool. They are
//! plain text, not driver state -- no objects, proofs, or chain -- so the MCP
//! server owns them directly (a `commands/` dir beside the driver's objects
//! dir, i.e. `~/.dobj/commands/`), surfaces each as a dynamic MCP prompt, and
//! never involves the driver.

use std::path::PathBuf;

use anyhow::{Result, anyhow};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Prompt names the server reserves for built-ins; a user command may not
/// shadow them. Kept in sync with the built-ins in [`crate::prompts`].
const RESERVED_NAMES: [&str; 5] = ["play", "help", "create-command", "consult-docs", "start"];

/// A user-authored command: a named, reusable block of instructions the model
/// follows when the command is invoked.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct UserCommand {
    /// Slug name (lowercase, dash-separated), e.g. `build-rocket`.
    pub name: String,
    /// One-line summary shown in the command menu.
    pub description: String,
    /// The steps the model follows when the command runs.
    pub body: String,
}

/// Tool-output wrapper for a listing of user commands.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CommandList {
    pub commands: Vec<UserCommand>,
}

/// A directory of `<name>.json` command definitions.
#[derive(Debug, Clone)]
pub struct CommandStore {
    dir: PathBuf,
}

impl CommandStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// All stored commands, sorted by name. A missing directory yields an empty
    /// list; unreadable or malformed files are skipped rather than failing the
    /// whole listing.
    pub fn list(&self) -> Vec<UserCommand> {
        let mut commands = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return commands;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                continue;
            }
            if let Some(command) = std::fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str::<UserCommand>(&text).ok())
            {
                commands.push(command);
            }
        }
        commands.sort_by(|a, b| a.name.cmp(&b.name));
        commands
    }

    pub fn get(&self, name: &str) -> Option<UserCommand> {
        let name = normalize_name(name).ok()?;
        let text = std::fs::read_to_string(self.path_for(&name)).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Validate and persist a command, overwriting any existing one with the
    /// same slug. The raw `name` is normalized to a slug; the stored and
    /// returned command carries the normalized name.
    pub fn save(&self, name: &str, description: &str, body: &str) -> Result<UserCommand> {
        let name = normalize_name(name)?;
        let body = body.trim();
        if body.is_empty() {
            return Err(anyhow!("command body must not be empty"));
        }
        let command = UserCommand {
            name,
            description: description.trim().to_string(),
            body: body.to_string(),
        };
        std::fs::create_dir_all(&self.dir)
            .map_err(|err| anyhow!("failed to create command dir {}: {err}", self.dir.display()))?;
        let json = serde_json::to_string_pretty(&command)?;
        std::fs::write(self.path_for(&command.name), json)
            .map_err(|err| anyhow!("failed to write command {}: {err}", command.name))?;
        Ok(command)
    }

    /// Remove a command. Returns whether a command of that name existed.
    pub fn delete(&self, name: &str) -> Result<bool> {
        let name = normalize_name(name)?;
        let path = self.path_for(&name);
        if !path.exists() {
            return Ok(false);
        }
        std::fs::remove_file(&path)
            .map_err(|err| anyhow!("failed to delete command {name}: {err}"))?;
        Ok(true)
    }

    fn path_for(&self, normalized_name: &str) -> PathBuf {
        self.dir.join(format!("{normalized_name}.json"))
    }
}

/// Normalize a raw command name to a filesystem-safe slug, rejecting empty and
/// reserved names. Lowercases, collapses each run of non-alphanumeric
/// characters into a single dash, and trims leading/trailing dashes -- so the
/// result is always a bare `[a-z0-9-]` token with no path separators, which is
/// what makes `path_for` traversal-safe.
pub fn normalize_name(raw: &str) -> Result<String> {
    let mut slug = String::new();
    let mut prev_dash = false;
    for ch in raw.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            slug.push('-');
            prev_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        return Err(anyhow!("command name must contain a letter or digit"));
    }
    if RESERVED_NAMES.contains(&slug.as_str()) {
        return Err(anyhow!("'{slug}' is a reserved command name"));
    }
    Ok(slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, CommandStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = CommandStore::new(dir.path());
        (dir, store)
    }

    #[test]
    fn save_then_get_roundtrips_and_slugifies() {
        let (_dir, store) = store();
        let saved = store
            .save(
                "Build Rocket",
                "assemble the finished rocket",
                "run the craft-rocket steps",
            )
            .unwrap();
        assert_eq!(saved.name, "build-rocket");
        assert_eq!(store.get("build-rocket").unwrap(), saved);
        // lookup normalizes too
        assert_eq!(store.get("Build Rocket").unwrap(), saved);
    }

    #[test]
    fn list_is_sorted_and_skips_non_json() {
        let (dir, store) = store();
        store.save("zeta", "z", "step").unwrap();
        store.save("alpha", "a", "step").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "ignore me").unwrap();
        let names: Vec<String> = store.list().into_iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["alpha", "zeta"]);
    }

    #[test]
    fn delete_reports_prior_existence() {
        let (_dir, store) = store();
        store.save("temp", "t", "step").unwrap();
        assert!(store.delete("temp").unwrap());
        assert!(!store.delete("temp").unwrap());
        assert!(store.get("temp").is_none());
    }

    #[test]
    fn reserved_empty_and_traversal_names_are_handled() {
        let (_dir, store) = store();
        assert!(store.save("play", "x", "step").is_err());
        assert!(store.save("help", "x", "step").is_err());
        assert!(store.save("create-command", "x", "step").is_err());
        assert!(store.save("consult-docs", "x", "step").is_err());
        assert!(store.save("start", "x", "step").is_err());
        assert!(store.save("!!!", "x", "step").is_err());
        // path separators never survive normalization
        assert_eq!(normalize_name("../etc/passwd").unwrap(), "etc-passwd");
    }

    #[test]
    fn empty_body_rejected() {
        let (_dir, store) = store();
        assert!(store.save("ok", "x", "   ").is_err());
    }
}
