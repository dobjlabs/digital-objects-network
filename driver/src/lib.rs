//! Headless driver for the craft-basics object world.
//!
//! Owns:
//! - local `.dobj` storage under `~/.dobj/objects`
//! - settings (synchronizer + relayer URLs)
//! - the built-in action catalog (5 craft-basics actions, hardcoded)
//! - synchronizer / relayer HTTP clients
//! - risc0 prover invocation + receipt → blob payload assembly
//! - end-to-end action execution (build proof, submit, wait, persist outputs)
//!
//! Blocking API. GUI callers should spawn-blocking.

pub mod catalog;
pub mod clients;
pub mod driver;
pub mod execute;
pub mod object;
pub mod paths;
pub mod settings;
pub mod store;

pub use crate::catalog::{ActionInfo, ClassInfo, action_by_id, action_by_name, all_actions, all_classes};
pub use crate::driver::Driver;
pub use crate::object::{ObjectRecord, ObjectStatus, SourceTxData};
pub use crate::paths::{DriverPaths, default_paths};
pub use crate::settings::DriverSettings;
