//! File-backed store for user-authored commands: one directory per command,
//! `~/.dobj/commands/<name>/`, holding a `README.md` (YAML frontmatter `name`
//! and `description`, then the instruction body the model follows) plus any
//! sibling scripts the command runs. The MCP server owns this directory; the
//! driver is not involved. Commands are surfaced as MCP prompts, so they work
//! in any MCP client; sibling scripts run in clients that have a shell.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use schemars::JsonSchema;
use serde::Serialize;

/// The instruction file inside each command's directory.
const README: &str = "README.md";

/// A user-authored command: a named, reusable block of instructions the model
/// follows when the command is invoked. The `name` is the directory slug; the
/// `description` and `body` come from its `README.md`.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub struct UserCommand {
    pub name: String,
    pub description: String,
    pub body: String,
}

/// Tool-output wrapper for a listing of user commands.
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CommandList {
    pub commands: Vec<UserCommand>,
}

/// A directory of `<name>/README.md` command definitions (plus their scripts).
#[derive(Debug, Clone)]
pub struct CommandStore {
    dir: PathBuf,
}

impl CommandStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// All stored commands, sorted by name. A missing directory yields an empty
    /// list; entries without a readable `README.md` are skipped rather than
    /// failing the whole listing.
    pub fn list(&self) -> Vec<UserCommand> {
        let mut commands = Vec::new();
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return commands;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir()
                && let Some(command) = read_command(&path)
            {
                commands.push(command);
            }
        }
        commands.sort_by(|a, b| a.name.cmp(&b.name));
        commands
    }

    pub fn get(&self, name: &str) -> Option<UserCommand> {
        let name = normalize_name(name).ok()?;
        read_command(&self.dir.join(name))
    }

    /// Validate and persist a command. The raw `name` is normalized to a slug;
    /// the body and description land in `<name>/README.md`. Re-saving an
    /// existing command rewrites its README and leaves any sibling scripts in
    /// place.
    pub fn save(&self, name: &str, description: &str, body: &str) -> Result<UserCommand> {
        let name = normalize_name(name)?;
        let body = body.trim();
        if body.is_empty() {
            return Err(anyhow!("command body must not be empty"));
        }
        let command = UserCommand {
            name,
            description: single_line(description),
            body: body.to_string(),
        };
        let dir = self.dir.join(&command.name);
        std::fs::create_dir_all(&dir)
            .map_err(|err| anyhow!("failed to create command dir {}: {err}", dir.display()))?;
        std::fs::write(dir.join(README), render_readme(&command))
            .map_err(|err| anyhow!("failed to write command {}: {err}", command.name))?;
        Ok(command)
    }

    /// Remove a command and its directory (README + any scripts). Returns
    /// whether a command of that name existed.
    pub fn delete(&self, name: &str) -> Result<bool> {
        let name = normalize_name(name)?;
        let dir = self.dir.join(name);
        if !dir.exists() {
            return Ok(false);
        }
        std::fs::remove_dir_all(&dir)
            .map_err(|err| anyhow!("failed to delete command {}: {err}", dir.display()))?;
        Ok(true)
    }
}

/// Read `<dir>/README.md` into a command. The name is the directory's own name,
/// so renaming the directory renames the command; the description comes from
/// the frontmatter and the body is everything after it.
fn read_command(dir: &Path) -> Option<UserCommand> {
    let name = dir.file_name()?.to_str()?.to_string();
    let text = std::fs::read_to_string(dir.join(README)).ok()?;
    let (frontmatter, body) = split_frontmatter(&text);
    let description = frontmatter
        .and_then(|front| field(front, "description"))
        .unwrap_or_default();
    Some(UserCommand {
        name,
        description,
        body: body.trim().to_string(),
    })
}

/// Render a command's `README.md`: YAML frontmatter, then the body.
fn render_readme(command: &UserCommand) -> String {
    format!(
        "---\nname: {}\ndescription: {}\n---\n\n{}\n",
        command.name, command.description, command.body
    )
}

/// Collapse `text` to one line. The frontmatter keeps the description on a
/// single `description:` line, so an embedded newline -- especially a `\n---\n`
/// -- would otherwise close the frontmatter early and swallow part of the body
/// on the next read. Runs of whitespace become a single space.
fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Split a leading `---\n...\n---\n` YAML frontmatter block from the body.
/// Returns `(frontmatter, body)`; frontmatter is `None` when absent.
fn split_frontmatter(text: &str) -> (Option<&str>, &str) {
    let Some(rest) = text.strip_prefix("---\n") else {
        return (None, text);
    };
    match rest.split_once("\n---\n") {
        Some((frontmatter, body)) => (Some(frontmatter), body),
        None => (None, text),
    }
}

/// The value of the first `key: value` line in simple frontmatter.
fn field(frontmatter: &str, key: &str) -> Option<String> {
    frontmatter.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.trim() == key).then(|| value.trim().to_string())
    })
}

/// Normalize a raw command name to a filesystem-safe slug, rejecting empty and
/// reserved names. Lowercases, collapses each run of non-alphanumeric
/// characters into a single dash, and trims leading/trailing dashes -- so the
/// result is always a bare `[a-z0-9-]` token with no path separators, which is
/// what keeps the directory joins traversal-safe.
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
    if crate::prompts::is_reserved(&slug) {
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
                "run the steps",
            )
            .unwrap();
        assert_eq!(saved.name, "build-rocket");
        assert_eq!(store.get("build-rocket").unwrap(), saved);
        // lookup normalizes too
        assert_eq!(store.get("Build Rocket").unwrap(), saved);
    }

    #[test]
    fn save_writes_readme_with_frontmatter() {
        let (dir, store) = store();
        store.save("greet", "say hi", "say hello").unwrap();
        let readme = std::fs::read_to_string(dir.path().join("greet").join("README.md")).unwrap();
        assert!(readme.starts_with("---\nname: greet\ndescription: say hi\n---\n"));
        assert!(readme.contains("say hello"));
    }

    #[test]
    fn resave_keeps_scripts_and_delete_removes_them() {
        let (dir, store) = store();
        store.save("tool", "first", "body").unwrap();
        let script = dir.path().join("tool").join("run.py");
        std::fs::write(&script, "print('hi')").unwrap();
        // re-saving rewrites the README but leaves the script
        store.save("tool", "second", "body2").unwrap();
        assert!(script.exists());
        assert_eq!(store.get("tool").unwrap().description, "second");
        // delete takes the whole directory
        assert!(store.delete("tool").unwrap());
        assert!(!script.exists());
    }

    #[test]
    fn list_is_sorted_and_skips_dirs_without_readme() {
        let (dir, store) = store();
        store.save("zeta", "z", "step").unwrap();
        store.save("alpha", "a", "step").unwrap();
        std::fs::create_dir_all(dir.path().join("empty")).unwrap(); // no README
        std::fs::write(dir.path().join("notes.txt"), "ignore").unwrap(); // not a dir
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
        assert!(store.save("start", "x", "step").is_err());
        assert!(store.save("help", "x", "step").is_err());
        assert!(store.save("create-command", "x", "step").is_err());
        assert!(store.save("consult-docs", "x", "step").is_err());
        assert!(store.save("dashboard", "x", "step").is_err());
        assert!(store.save("!!!", "x", "step").is_err());
        // path separators never survive normalization
        assert_eq!(normalize_name("../etc/passwd").unwrap(), "etc-passwd");
    }

    #[test]
    fn empty_body_rejected() {
        let (_dir, store) = store();
        assert!(store.save("ok", "x", "   ").is_err());
    }

    #[test]
    fn description_collapsed_to_one_line_keeps_body_intact() {
        let (_dir, store) = store();
        // A newline -- even a frontmatter delimiter -- in the description must
        // not terminate the frontmatter early or bleed into the body.
        let saved = store
            .save("danger", "line one\n---\nline two", "the real body")
            .unwrap();
        assert_eq!(saved.description, "line one --- line two");
        let reloaded = store.get("danger").unwrap();
        assert_eq!(reloaded.description, "line one --- line two");
        assert_eq!(reloaded.body, "the real body");
    }
}
