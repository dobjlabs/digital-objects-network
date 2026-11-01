//! Proof-of-work search for actions that accept the value they would search
//! for as an argument.
//!
//! A plugin's `<plugin>.pow_hints.json`, installed beside its `.pexe`, names
//! for each such action the argument that takes that value and the arguments
//! the action would otherwise draw at random before its search. The grinder
//! runs the action itself, on the real input objects but with a mock prover
//! against made-up chain state, supplies fresh random values for those
//! arguments, and searches on several threads where the action calls
//! `pow_obj_grind`. Given the same input objects and every argument the
//! grinder found, the action builds the object the search hashed and skips
//! its own search.

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    num::NonZeroUsize,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use plonky2::field::types::Field;
use pod2::middleware::{F, RawValue, StrKey, Value, containers::Dictionary};
use pod2utils::rand_raw_value;
use rand::{RngCore, SeedableRng, rngs::StdRng};
use sdk::{ActionArgs, Sdk, SdkModule};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct PowHint {
    /// The action's argument that takes the value its search finds.
    pub arg: String,
    /// The action's arguments that it would otherwise draw at random before
    /// its search.
    pub random_args: Vec<String>,
}

/// Hints keyed by qualified action name.
pub type PowHints = BTreeMap<String, PowHint>;

pub fn hints_path(actions_dir: &Path, plugin: &str) -> PathBuf {
    actions_dir.join(format!("{plugin}.pow_hints.json"))
}

pub fn plugin_path(actions_dir: &Path, plugin: &str) -> PathBuf {
    actions_dir.join(format!("{plugin}.{}", pexe::PEXE_EXTENSION))
}

pub fn read_hints(path: &Path) -> Result<PowHints> {
    let bytes = fs::read(path)
        .with_context(|| format!("Cannot read proof-of-work hints at {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("Invalid proof-of-work hints at {}", path.display()))
}

/// Compiles the plugin installed at `path`, which must be named `plugin`.
pub fn load_plugin(path: &Path, plugin: &str) -> Result<Rc<SdkModule>> {
    let bytes = pexe::read_pexe_file(path)?;
    let (manifest, script) = pexe::unpack(&bytes)
        .with_context(|| format!("Cannot unpack the plugin at {}", path.display()))?;
    if manifest.plugin.name != plugin {
        bail!(
            "{} holds plugin {:?}, not {plugin:?}",
            path.display(),
            manifest.plugin.name
        );
    }
    Sdk::default()
        .load_module_from_src_manifest(&script, &manifest)
        .map_err(|error| anyhow!("Cannot load the plugin at {}: {error}", path.display()))
}

/// Reads the object dobjd would take as the input `path`: the file of that
/// name in `objects_dir`, whatever directory `path` itself names.
pub fn read_input_object(objects_dir: &Path, path: &str) -> Result<Dictionary> {
    #[derive(Deserialize)]
    struct ObjectFile {
        obj: Dictionary,
    }
    let file_name = Path::new(path)
        .file_name()
        .with_context(|| format!("{path:?} names no object file"))?;
    let file = objects_dir.join(file_name);
    let bytes = fs::read(&file)
        .with_context(|| format!("Cannot read the input object {}", file.display()))?;
    let object: ObjectFile = serde_json::from_slice(&bytes)
        .with_context(|| format!("Invalid object file {}", file.display()))?;
    Ok(object.obj)
}

/// Runs `action` of `module` on `inputs`, given in the order the action takes
/// them, with fresh random values for the hint's random arguments, and
/// searches with `search` where the action searches. Returns every argument
/// the action needs to build the object `search` hashed.
pub fn find_args(
    module: &Rc<SdkModule>,
    action: &str,
    hint: &PowHint,
    inputs: Vec<Dictionary>,
    search: impl Fn(&Dictionary, &StrKey, RawValue) -> RawValue + Send + Sync + 'static,
) -> Result<ActionArgs> {
    let mut args: ActionArgs = hint
        .random_args
        .iter()
        .map(|name| (name.clone(), Value::from(rand_raw_value())))
        .collect();
    let state = pexe::fixtures::build_synthetic_state(&inputs)?;
    let found = Arc::new(Mutex::new(Vec::new()));
    let recorded = found.clone();
    module
        .executor(true, state.grounding_witness)
        .with_pow_search(Arc::new(move |object, field, target| {
            let value = search(object, field, target);
            recorded.lock().unwrap().push(value);
            value
        }))
        .plan_action_with_args(action, state.spendable, &args)
        .map_err(|error| anyhow!("Cannot run {action}: {error}"))?;
    let found = found.lock().unwrap();
    let [value] = found[..] else {
        bail!("{action} searches {} times, not once", found.len());
    };
    args.insert(hint.arg.clone(), Value::from(value));
    Ok(args)
}

fn meets_target(object: &Dictionary, target: RawValue) -> bool {
    RawValue::from(object.commitment()) <= target
}

/// Draws a value as `pod2utils::rand_raw_value` does, from a generator
/// seeded once per search instead of once per value.
fn random_value(rng: &mut StdRng) -> RawValue {
    RawValue(std::array::from_fn(|_| {
        F::from_noncanonical_u64(rng.next_u64())
    }))
}

const ATTEMPTS_PER_REPORT: u64 = 1024;

const PROGRESS_INTERVAL: Duration = Duration::from_secs(15);

/// Values tried between rebuilds of the object, which discard the tree nodes
/// that earlier values left in its store.
const VALUES_PER_REBUILD: u64 = 256;

/// An object as entries, which unlike a `Dictionary` can be shared between
/// threads.
type Entries = HashMap<StrKey, Value>;

fn with_value(entries: &Entries, field: &StrKey, value: RawValue) -> Dictionary {
    let mut entries = entries.clone();
    entries.insert(field.clone(), Value::from(value));
    Dictionary::new(entries)
}

fn search(
    entries: &Entries,
    field: &StrKey,
    target: RawValue,
    found: &AtomicBool,
    attempts: &AtomicU64,
) -> Option<RawValue> {
    let mut rng = StdRng::from_os_rng();
    let mut value = random_value(&mut rng);
    let mut object = with_value(entries, field, value);
    let mut values_tried: u64 = 1;
    while !meets_target(&object, target) {
        if found.load(Ordering::Relaxed) {
            return None;
        }
        value = random_value(&mut rng);
        if values_tried.is_multiple_of(VALUES_PER_REBUILD) {
            object = with_value(entries, field, value);
        } else {
            object
                .update(field, &Value::from(value))
                .expect("the searched object holds the field");
        }
        values_tried += 1;
        if values_tried.is_multiple_of(ATTEMPTS_PER_REPORT) {
            attempts.fetch_add(ATTEMPTS_PER_REPORT, Ordering::Relaxed);
        }
    }
    found.store(true, Ordering::Relaxed);
    attempts.fetch_add(values_tried % ATTEMPTS_PER_REPORT, Ordering::Relaxed);
    Some(value)
}

pub struct GrindResult {
    pub value: RawValue,
    /// Values tried across all threads, counted in batches, so it can trail
    /// the true count by a batch per thread.
    pub attempts: u64,
}

/// Searches on `threads` threads for a value of `field` that brings
/// `object`'s commitment to at most `target`, calling `on_progress` with the
/// attempts so far every 15 seconds.
pub fn grind(
    object: &Dictionary,
    field: &StrKey,
    target: RawValue,
    threads: NonZeroUsize,
    mut on_progress: impl FnMut(u64),
) -> GrindResult {
    let entries: Entries = object
        .iter()
        .map(|entry| {
            let (key, value) = entry.expect("dictionary entry");
            (StrKey::from(key), value)
        })
        .collect();
    let found = AtomicBool::new(false);
    let attempts = AtomicU64::new(0);
    let value = std::thread::scope(|scope| {
        let (sender, receiver) = mpsc::channel();
        for _ in 0..threads.get() {
            let sender = sender.clone();
            let (entries, found, attempts) = (&entries, &found, &attempts);
            scope.spawn(move || {
                if let Some(value) = search(entries, field, target, found, attempts) {
                    let _ = sender.send(value);
                }
            });
        }
        drop(sender);
        loop {
            match receiver.recv_timeout(PROGRESS_INTERVAL) {
                Ok(value) => break value,
                Err(RecvTimeoutError::Timeout) => on_progress(attempts.load(Ordering::Relaxed)),
                Err(RecvTimeoutError::Disconnected) => {
                    unreachable!("every search runs until one of them finds a value")
                }
            }
        }
    });
    GrindResult {
        value,
        attempts: attempts.load(Ordering::Relaxed),
    }
}

#[cfg(test)]
mod tests {
    use std::iter;

    use lt_eq_u256_pod::LtEqU256Pod;
    use pod2::{
        backends::plonky2::basetypes::DEFAULT_VD_SET,
        middleware::{Hash, Params, Pod},
    };

    use super::*;

    const BITCRAFT_PLUGIN_DIR: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../../examples/bitcraft");

    struct Bitcraft {
        module: Rc<SdkModule>,
        plugin: String,
        script: String,
    }

    fn bitcraft() -> Bitcraft {
        let source = pexe::PluginSource::read(BITCRAFT_PLUGIN_DIR).unwrap();
        let manifest = source.parse_manifest().unwrap();
        let module = Sdk::default()
            .load_module_from_src_manifest(&source.script, &manifest)
            .unwrap();
        Bitcraft {
            module,
            plugin: manifest.plugin.name,
            script: source.script,
        }
    }

    fn bitcraft_hints() -> PowHints {
        read_hints(&Path::new(BITCRAFT_PLUGIN_DIR).join("pow_hints.json")).unwrap()
    }

    /// The lines of the action's function in the plugin script.
    fn script_lines<'a>(script: &'a str, action: &str) -> Vec<&'a str> {
        let header = format!("fn {action}(action) {{");
        script
            .lines()
            .skip_while(|line| *line != header)
            .skip(1)
            .take_while(|line| *line != "}")
            .collect()
    }

    /// Inputs minted for `action`, with what the action requires of its tool:
    /// any `level` set to the number that ends the action's name, and any
    /// `durability` left.
    fn synthetic_inputs(module: &SdkModule, action: &str) -> Vec<Dictionary> {
        let level: Option<i64> = action
            .trim_start_matches(|character: char| !character.is_ascii_digit())
            .parse()
            .ok();
        let action = module
            .actions()
            .iter()
            .find(|candidate| candidate.name == action)
            .unwrap();
        let classes: Vec<String> = action
            .total_inputs()
            .map(|input| input.class.clone())
            .collect();
        let mut inputs = pexe::fixtures::mint_classes(module, &classes).unwrap();
        let required = [("level", level), ("durability", Some(1))];
        for input in &mut inputs {
            for (field, value) in required {
                let field = StrKey::from(field);
                if let Some(value) = value
                    && input.get(&field).unwrap().is_some()
                {
                    input.update(&field, &Value::from(value)).unwrap();
                }
            }
        }
        inputs
    }

    /// What a search was given and what it returned.
    #[derive(Clone)]
    struct Searched {
        object: Dictionary,
        field: StrKey,
        target: RawValue,
        value: RawValue,
    }

    impl Searched {
        fn found_object(&self) -> Dictionary {
            let mut object = self.object.clone();
            object
                .update(&self.field, &Value::from(self.value))
                .unwrap();
            object
        }
    }

    /// Finds the arguments for `action` with `search`, recording what the
    /// search was given.
    fn find_args_recording(
        module: &Rc<SdkModule>,
        action: &str,
        hint: &PowHint,
        inputs: Vec<Dictionary>,
        search: impl Fn(&Dictionary, &StrKey, RawValue) -> RawValue + Send + Sync + 'static,
    ) -> (ActionArgs, Searched) {
        let searched = Arc::new(Mutex::new(None));
        let recorded = searched.clone();
        let args = find_args(
            module,
            action,
            hint,
            inputs,
            move |object, field, target| {
                let value = search(object, field, target);
                *recorded.lock().unwrap() = Some(Searched {
                    object: object.clone(),
                    field: field.clone(),
                    target,
                    value,
                });
                value
            },
        )
        .unwrap();
        let searched = searched.lock().unwrap().clone().unwrap();
        (args, searched)
    }

    #[test]
    fn a_found_value_brings_the_commitment_within_the_target() {
        let field = StrKey::from("salt");
        let object = Dictionary::new(HashMap::from([
            (
                StrKey::from("type"),
                Value::from(Hash([F(1), F(2), F(3), F(4)])),
            ),
            (field.clone(), Value::from(0)),
        ]));
        let target = RawValue([F(0), F(0), F(0), F(1 << 62)]);
        let result = grind(
            &object,
            &field,
            target,
            NonZeroUsize::new(2).unwrap(),
            |_| {},
        );
        let mut found = object.clone();
        found.update(&field, &Value::from(result.value)).unwrap();
        assert!(meets_target(&found, target));
        assert!(result.attempts >= 1);
    }

    #[test]
    fn bitcraft_hints_name_exactly_the_actions_that_search_and_arguments_they_read() {
        let Bitcraft {
            module,
            plugin,
            script,
        } = bitcraft();
        let hints = bitcraft_hints();
        for qualified in hints.keys() {
            assert!(
                module
                    .actions()
                    .iter()
                    .any(|action| format!("{plugin}::{}", action.name) == *qualified),
                "{qualified} is not an action"
            );
        }
        for action in module.actions() {
            let qualified = format!("{plugin}::{}", action.name);
            let searches = script_lines(&script, &action.name)
                .iter()
                .any(|line| line.contains("pow_obj_grind("));
            let Some(hint) = hints.get(&qualified) else {
                assert!(!searches, "{qualified} searches but has no hint");
                continue;
            };
            assert!(searches, "{qualified} has a hint but does not search");
            for arg in iter::once(&hint.arg).chain(&hint.random_args) {
                assert!(
                    action.args().contains(arg),
                    "{qualified} does not read {arg}"
                );
            }
        }
    }

    #[test]
    fn found_arguments_make_every_hinted_action_build_the_object_its_search_hashed() {
        let hints: Vec<(String, PowHint)> = bitcraft_hints().into_iter().collect();
        let threads = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
        std::thread::scope(|scope| {
            for chunk in hints.chunks(hints.len().div_ceil(threads)) {
                // A module cannot cross threads, so each thread compiles its own.
                scope.spawn(move || {
                    let Bitcraft { module, plugin, .. } = bitcraft();
                    for (qualified, hint) in chunk {
                        let action = qualified
                            .strip_prefix(&format!("{plugin}::"))
                            .unwrap()
                            .to_string();
                        let inputs = synthetic_inputs(&module, &action);
                        let (args, searched) = find_args_recording(
                            &module,
                            &action,
                            hint,
                            inputs.clone(),
                            |_, _, _| rand_raw_value(),
                        );
                        let state = pexe::fixtures::build_synthetic_state(&inputs).unwrap();
                        let outputs = module
                            .executor(true, state.grounding_witness)
                            .with_pow_search(Arc::new(|_, _, _| {
                                panic!("an action given every argument does not search")
                            }))
                            .action_with_args(&action, state.spendable, &args)
                            .unwrap();
                        let expected = searched.found_object().commitment();
                        assert!(
                            outputs
                                .objs
                                .iter()
                                .any(|output| output.obj.commitment() == expected),
                            "{qualified} built another object than its search hashed"
                        );
                    }
                });
            }
        });
    }

    #[test]
    #[ignore = "builds a real proof"]
    fn a_found_value_passes_the_real_target_proof() {
        let Bitcraft { module, .. } = bitcraft();
        let hint = &bitcraft_hints()["bitcraft::GatherWood"];
        let inputs = synthetic_inputs(&module, "GatherWood");
        let (_, searched) = find_args_recording(
            &module,
            "GatherWood",
            hint,
            inputs,
            |object, field, target| {
                grind(object, field, target, NonZeroUsize::new(4).unwrap(), |_| {}).value
            },
        );
        let commitment = RawValue::from(searched.found_object().commitment());
        let pod = LtEqU256Pod::new(
            &Params::default(),
            DEFAULT_VD_SET.clone(),
            commitment,
            searched.target,
        )
        .unwrap();
        pod.verify().unwrap();
    }
}
