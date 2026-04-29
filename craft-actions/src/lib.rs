//! Action validators for the craft-basics world.
//!
//! Each action corresponds to one `fn` in [`actions`]. The unified entry
//! point is [`validate`], which takes a [`GuestInput`] and dispatches by
//! `action_id`.
//!
//! Validators are *pure assertions* — they panic on violation. The guest
//! treats a panic as "this proof can't be built"; the driver treats a panic
//! (or its non-panicking variant in the future) as "this action wouldn't be
//! accepted by the synchronizer".
//!
//! ## Action IDs
//!
//! Pinned: do not renumber. The guest's `image_id` commits to this dispatch
//! table, and `tx_final` commits to `action_id` directly.
//!
//! | id | action            |
//! | -- | ----------------- |
//! | 1  | `FindLog`         |
//! | 2  | `CraftWood`       |
//! | 3  | `CraftSticks`     |
//! | 4  | `CraftWoodPick`   |
//! | 5  | `UseWoodPick`     |

#![cfg_attr(not(feature = "std"), no_std)]

extern crate alloc;

pub mod actions;
pub mod grounding;
pub mod intro;
pub mod tx_build;

use txlib_core::abi::{ActionId, GuestInput, GuestJournal};

pub const ACTION_FIND_LOG: ActionId = 1;
pub const ACTION_CRAFT_WOOD: ActionId = 2;
pub const ACTION_CRAFT_STICKS: ActionId = 3;
pub const ACTION_CRAFT_WOOD_PICK: ActionId = 4;
pub const ACTION_USE_WOOD_PICK: ActionId = 5;

/// Run the full guest pipeline for one action invocation:
///
/// 1. Verify two-level grounding for every input
/// 2. Compute the consumed objects' nullifiers
/// 3. Dispatch on `action_id` and run the action-specific predicate
/// 4. Build the new tx (live + nullifiers SMT roots, action_nonce, tx_final)
/// 5. Return the [`GuestJournal`] the guest will commit
///
/// Panics on any validation failure — that's the desired behavior in the
/// risc0 guest (a failed predicate produces no receipt).
pub fn validate(input: &GuestInput) -> GuestJournal {
    grounding::verify_all(input);
    let nullifiers = tx_build::nullifiers_for(input);
    dispatch(input);
    tx_build::build_journal(input, nullifiers)
}

fn dispatch(input: &GuestInput) {
    match input.action_id {
        ACTION_FIND_LOG => actions::find_log(input),
        ACTION_CRAFT_WOOD => actions::craft_wood(input),
        ACTION_CRAFT_STICKS => actions::craft_sticks(input),
        ACTION_CRAFT_WOOD_PICK => actions::craft_wood_pick(input),
        ACTION_USE_WOOD_PICK => actions::use_wood_pick(input),
        other => panic!("unknown action_id: {other}"),
    }
}
