use anyhow::Result;
use pod2::middleware::Hash;
use sdk::{SpendableObject, SpendableObjects};
use txlib::GroundingWitness;

use crate::types::ActionSummary;
use wire_types::QualifiedName;

/// A class entry surfaced by an [`ActionCatalog`]. `class` is the canonical
/// plugin-scoped handle; `hash` is the on-chain `Is{class}` predicate hash
/// that distinguishes classes which share a bare name but live in
/// different plugins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogClass {
    pub class: QualifiedName,
    pub emoji: String,
    pub hash: String,
    pub description: String,
    pub produced_by: Vec<QualifiedName>,
    pub consumed_by: Vec<QualifiedName>,
    pub predicate_source: String,
}

pub trait ActionCatalog: Send + Sync {
    fn list_actions(&self) -> Vec<ActionSummary>;
    fn get_action(&self, action: &QualifiedName) -> Option<ActionSummary>;
    fn list_classes(&self) -> Vec<CatalogClass>;
    fn get_class(&self, class: &QualifiedName) -> Option<CatalogClass>;
    fn get_class_by_hash(&self, class_hash: &Hash) -> Option<CatalogClass>;
    fn execute_action(
        &self,
        action: QualifiedName,
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
