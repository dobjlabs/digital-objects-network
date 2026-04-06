use anyhow::Result;
use craft_sdk::{SpendableObject, SpendableObjects};
use txlib::GroundingWitness;

use crate::types::ActionSummary;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogClass {
    pub name: String,
    pub emoji: String,
    pub hash: String,
    pub description: String,
    pub produced_by: Vec<String>,
    pub consumed_by: Vec<String>,
    pub predicate_source: String,
}

pub trait ActionCatalog: Send + Sync {
    fn list_actions(&self) -> Vec<ActionSummary>;
    fn get_action(&self, action_id: &str) -> Option<ActionSummary>;
    fn list_classes(&self) -> Vec<CatalogClass>;
    fn get_class(&self, class_name: &str) -> Option<CatalogClass>;
    fn execute_action(
        &self,
        action_id: String,
        grounding_witness: GroundingWitness,
        inputs: Vec<SpendableObject>,
    ) -> Result<SpendableObjects>;
    fn generated_podlang(&self) -> Option<String> {
        None
    }
}

/// Extract a named predicate definition from podlang source.
///
/// Podlang definitions have the form:
///   Name(args) = AND/OR(\n  ...\n)\n
///
/// We find `name(` at a line start, then scan forward to find the
/// `= AND(` or `= OR(` combiner, and track paren depth from there
/// to find the closing `)`.
pub(crate) fn extract_predicate(podlang_src: &str, name: &str) -> Option<String> {
    let prefix = format!("{name}(");
    let start = podlang_src.find(&prefix)?;
    let after_prefix = &podlang_src[start..];

    // Find the combiner `= AND(` or `= OR(`
    let combiner_pos = after_prefix
        .find("= AND(")
        .or_else(|| after_prefix.find("= OR("))?;

    // Find the opening `(` of the combiner
    let open = start + combiner_pos + after_prefix[combiner_pos..].find('(')?;

    // Track depth from the combiner's `(`
    let mut depth = 0;
    for (i, ch) in podlang_src[open..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(podlang_src[start..open + i + 1].trim().to_string());
                }
            }
            _ => {}
        }
    }
    None
}
