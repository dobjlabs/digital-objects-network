//! Hardcoded action catalog for the craft-basics world.
//!
//! Replaces the pod2-era dynamic Rhai/.pexe catalog. The 5 actions and 4
//! classes are statically known here — there's no runtime discovery, no
//! plugin loading. Adding a new action means: (1) add a validator in
//! `craft-actions`, (2) add an `ActionId` constant there, (3) add an
//! `ActionInfo` entry here, (4) wire the dispatch arm in
//! `craft-actions::dispatch`. A new guest ELF must then be built (image_id
//! changes — synchronizer + relayer pick up the new id via env var).

use craft_actions::{
    ACTION_CRAFT_STICKS, ACTION_CRAFT_WOOD, ACTION_CRAFT_WOOD_PICK, ACTION_FIND_LOG,
    ACTION_USE_WOOD_PICK,
};
use serde::Serialize;
use txlib_core::abi::ActionId;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ActionInfo {
    pub id: ActionId,
    pub name: &'static str,
    pub emoji: &'static str,
    pub description: &'static str,
    /// Class names this action consumes, in any order. Used by feasibility
    /// checks ("do I have at least one of each?"). For mutations the same
    /// class appears in both `inputs` and `outputs`.
    pub inputs: &'static [&'static str],
    pub outputs: &'static [&'static str],
    /// `false` — visible in the GUI. `true` — internal, hidden
    /// (the rhai-era `hidden = true` manifest flag).
    #[serde(default)]
    pub hidden: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClassInfo {
    pub name: &'static str,
    pub emoji: &'static str,
    pub description: &'static str,
}

const ACTIONS: &[ActionInfo] = &[
    ActionInfo {
        id: ACTION_FIND_LOG,
        name: "FindLog",
        emoji: "🌲",
        description: "Discover a log object by proving a short SHA-256 chain.",
        inputs: &[],
        outputs: &["Log"],
        hidden: false,
    },
    ActionInfo {
        id: ACTION_CRAFT_WOOD,
        name: "CraftWood",
        emoji: "🪵",
        description: "Refine one log into a wood object with PoW (commitment ≤ target).",
        inputs: &["Log"],
        outputs: &["Wood"],
        hidden: false,
    },
    ActionInfo {
        id: ACTION_CRAFT_STICKS,
        name: "CraftSticks",
        emoji: "🥢",
        description: "Split one wood object into two stick objects.",
        inputs: &["Wood"],
        outputs: &["Stick", "Stick"],
        hidden: false,
    },
    ActionInfo {
        id: ACTION_CRAFT_WOOD_PICK,
        name: "CraftWoodPick",
        emoji: "⛏️",
        description: "Combine wood and a stick to craft a wood pick (durability 100).",
        inputs: &["Wood", "Stick"],
        outputs: &["WoodPick"],
        hidden: false,
    },
    ActionInfo {
        id: ACTION_USE_WOOD_PICK,
        name: "UseWoodPick",
        emoji: "⛏️",
        description: "Internal durability/work update for wood pick usage.",
        inputs: &["WoodPick"],
        outputs: &["WoodPick"],
        hidden: true,
    },
];

const CLASSES: &[ClassInfo] = &[
    ClassInfo {
        name: "Log",
        emoji: "🌲",
        description: "A discovered log that can be refined into wood.",
    },
    ClassInfo {
        name: "Wood",
        emoji: "🪵",
        description: "Refined wood used for sticks and basic tools.",
    },
    ClassInfo {
        name: "Stick",
        emoji: "🥢",
        description: "A stick used as a handle in tool crafting.",
    },
    ClassInfo {
        name: "WoodPick",
        emoji: "⛏️",
        description: "A wood pick that can mine stone while durability remains.",
    },
];

pub fn all_actions() -> &'static [ActionInfo] {
    ACTIONS
}

pub fn all_classes() -> &'static [ClassInfo] {
    CLASSES
}

pub fn action_by_id(id: ActionId) -> Option<&'static ActionInfo> {
    ACTIONS.iter().find(|a| a.id == id)
}

pub fn action_by_name(name: &str) -> Option<&'static ActionInfo> {
    ACTIONS.iter().find(|a| a.name == name)
}

pub fn class_by_name(name: &str) -> Option<&'static ClassInfo> {
    CLASSES.iter().find(|c| c.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_action_id_is_unique() {
        let mut ids: Vec<ActionId> = ACTIONS.iter().map(|a| a.id).collect();
        ids.sort();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count, "duplicate action ids");
    }

    #[test]
    fn each_action_io_uses_known_classes() {
        for a in ACTIONS {
            for c in a.inputs.iter().chain(a.outputs.iter()) {
                assert!(class_by_name(c).is_some(), "{} references unknown class {}", a.name, c);
            }
        }
    }

    #[test]
    fn lookup_by_name_matches_lookup_by_id() {
        for a in ACTIONS {
            assert_eq!(action_by_name(a.name), Some(a));
            assert_eq!(action_by_id(a.id), Some(a));
        }
    }

    #[test]
    fn craft_wood_pick_is_visible() {
        assert!(!action_by_name("CraftWoodPick").unwrap().hidden);
    }

    #[test]
    fn use_wood_pick_is_hidden() {
        assert!(action_by_name("UseWoodPick").unwrap().hidden);
    }
}
