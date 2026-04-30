//! MCP resources exposed to the agent. Post-migration we no longer ship
//! podlang source files (the new stack is plain Rust validators in
//! `craft-actions`); the only resource left is the object-lifecycle
//! walkthrough.

use rmcp::model::{Annotated, RawResource, ReadResourceResult, Resource, ResourceContents};

const OBJECT_LIFECYCLE_URI: &str = "zk-craft://docs/object-lifecycle";

pub fn list() -> Vec<Resource> {
    vec![Annotated::new(
        RawResource::new(OBJECT_LIFECYCLE_URI, "Object Lifecycle")
            .with_description(
                "Walkthrough of a Digital Object's lifecycle: creation, mutation, \
                 consumption, nullifiers, and what each step looks like in the inventory.",
            )
            .with_mime_type("text/markdown"),
        None,
    )]
}

pub fn read(uri: &str) -> Option<ReadResourceResult> {
    let (content, mime) = match uri {
        OBJECT_LIFECYCLE_URI => (OBJECT_LIFECYCLE, "text/markdown"),
        _ => return None,
    };
    Some(ReadResourceResult::new(vec![
        ResourceContents::text(content, uri).with_mime_type(mime),
    ]))
}

const OBJECT_LIFECYCLE: &str = include_str!("../docs/object-lifecycle.md");
