use rmcp::model::{Annotated, RawResource, ReadResourceResult, Resource, ResourceContents};

const PODLANG_REFERENCE_URI: &str = "bitcraft://docs/podlang-reference";
const TXLIB_PREDICATES_URI: &str = "bitcraft://source/txlib.podlang";
const OBJECT_LIFECYCLE_URI: &str = "bitcraft://docs/object-lifecycle";

pub fn list() -> Vec<Resource> {
    vec![
        Annotated::new(
            RawResource::new(PODLANG_REFERENCE_URI, "Podlang Reference")
                .with_description(
                    "Complete reference for the podlang predicate language: \
                     syntax, built-in operations, public/private arguments, \
                     OR state-machine pattern, and common pitfalls.",
                )
                .with_mime_type("text/markdown"),
            None,
        ),
        Annotated::new(
            RawResource::new(TXLIB_PREDICATES_URI, "txlib.podlang")
                .with_description(
                    "Core transaction-model predicates: StateRoot, TxInit, \
                     TxInsert, TxMutate, TxDelete, TxFinalized, and \
                     nullifier logic. This is the foundation all actions build on.",
                )
                .with_mime_type("text/plain"),
            None,
        ),
        Annotated::new(
            RawResource::new(OBJECT_LIFECYCLE_URI, "Object Lifecycle")
                .with_description(
                    "Worked example of a Digital Object's lifecycle: creation, \
                     mutation, consumption, nullifiers, and what each step \
                     looks like in the inventory.",
                )
                .with_mime_type("text/markdown"),
            None,
        ),
    ]
}

pub fn read(uri: &str) -> Option<ReadResourceResult> {
    let (content, mime) = match uri {
        PODLANG_REFERENCE_URI => (PODLANG_REFERENCE, "text/markdown"),
        TXLIB_PREDICATES_URI => (TXLIB_PREDICATES, "text/plain"),
        OBJECT_LIFECYCLE_URI => (OBJECT_LIFECYCLE, "text/markdown"),
        _ => return None,
    };
    Some(ReadResourceResult::new(vec![
        ResourceContents::text(content, uri).with_mime_type(mime),
    ]))
}

const PODLANG_REFERENCE: &str = include_str!("../docs/podlang-reference.md");
const OBJECT_LIFECYCLE: &str = include_str!("../docs/object-lifecycle.md");
const TXLIB_PREDICATES: &str = include_str!("../../txlib/src/predicates/txlib.podlang");
