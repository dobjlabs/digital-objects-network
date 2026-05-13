//! `QualifiedName` is the canonical handle for both classes and actions
//! across the driver. It carries the originating plugin and the bare name as
//! two separate fields so callers can reason about them directly without
//! having to remember to keep `(plugin_name, name, id)` triples in sync. The
//! string presentation `<plugin>::<name>` matches podlang's namespaced
//! predicates and is produced by [`QualifiedName::id`] when the GUI or a
//! human reader needs a single string.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A name scoped to a plugin. Used for both classes and actions — both have
/// the same shape — so `QualifiedName` shows up as the field type in
/// summaries (`ObjectSummary::class`, `ActionSummary::action`, …).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub struct QualifiedName {
    pub plugin_name: String,
    pub name: String,
}

impl QualifiedName {
    pub fn new(plugin_name: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            plugin_name: plugin_name.into(),
            name: name.into(),
        }
    }

    /// Canonical string form `<plugin>::<name>` — matches podlang's
    /// namespaced-predicate syntax and is what the GUI shows the user.
    pub fn id(&self) -> String {
        format!("{}::{}", self.plugin_name, self.name)
    }

    /// Parse the canonical string form. Errors if the input does not
    /// contain `::` or if either component is empty.
    pub fn parse(s: &str) -> Result<Self, String> {
        let (plugin, name) = s
            .split_once("::")
            .ok_or_else(|| format!("invalid qualified name {s:?}: missing '::' separator"))?;
        if plugin.is_empty() {
            return Err(format!("invalid qualified name {s:?}: empty plugin"));
        }
        if name.is_empty() {
            return Err(format!("invalid qualified name {s:?}: empty name"));
        }
        Ok(Self {
            plugin_name: plugin.to_string(),
            name: name.to_string(),
        })
    }

    /// Lowercase, filename-safe prefix for `.dobj` files. Both plugin
    /// names (validated at catalog load) and class names (validated by
    /// the SDK at module compile time) are already restricted to
    /// `[A-Za-z0-9_-]`, so the only normalization this needs to do is
    /// lowercase. The fallback to `_` for any non-allowlisted byte stays
    /// in place as a defense-in-depth measure: if a future SDK regression
    /// or an entirely different catalog implementation ever feeds in a
    /// stray path separator, written files will still be confined to a
    /// single filename component.
    pub fn file_prefix(&self) -> String {
        let mut out = String::with_capacity(self.plugin_name.len() + 2 + self.name.len());
        push_safe_lower(&mut out, &self.plugin_name);
        out.push_str("__");
        push_safe_lower(&mut out, &self.name);
        out
    }
}

impl fmt::Display for QualifiedName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}::{}", self.plugin_name, self.name)
    }
}

fn push_safe_lower(out: &mut String, s: &str) {
    for ch in s.chars() {
        let lower = ch.to_ascii_lowercase();
        if lower.is_ascii_alphanumeric() || lower == '-' || lower == '_' {
            out.push(lower);
        } else {
            out.push('_');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_round_trips_via_parse() {
        let q = QualifiedName::new("craft-basics", "WoodPick");
        assert_eq!(q.id(), "craft-basics::WoodPick");
        assert_eq!(QualifiedName::parse(&q.id()).unwrap(), q);
    }

    #[test]
    fn parse_rejects_missing_separator() {
        assert!(QualifiedName::parse("craft-basics-Wood").is_err());
    }

    #[test]
    fn parse_rejects_empty_components() {
        assert!(QualifiedName::parse("::Wood").is_err());
        assert!(QualifiedName::parse("craft-basics::").is_err());
    }

    #[test]
    fn file_prefix_is_path_safe_for_normal_names() {
        let q = QualifiedName::new("craft-basics", "Wood");
        assert_eq!(q.file_prefix(), "craft-basics__wood");
    }

    #[test]
    fn file_prefix_sanitizes_path_chars_in_name() {
        let q = QualifiedName::new("plugin", "weird/class");
        assert_eq!(q.file_prefix(), "plugin__weird_class");
        let q = QualifiedName::new("plugin", "..\\Stone");
        assert_eq!(q.file_prefix(), "plugin_____stone");
        let q = QualifiedName::new("p", "c");
        let p = q.file_prefix();
        assert!(!p.contains(':') && !p.contains('/') && !p.contains('\\'));
    }

    #[test]
    fn serde_round_trips_as_object() {
        let q = QualifiedName::new("craft-basics", "Wood");
        let json = serde_json::to_string(&q).unwrap();
        assert_eq!(json, r#"{"pluginName":"craft-basics","name":"Wood"}"#);
        let back: QualifiedName = serde_json::from_str(&json).unwrap();
        assert_eq!(back, q);
    }
}
