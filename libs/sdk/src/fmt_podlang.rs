//! Functions used to format to podlang source code.
//!
//! This is the records-form emitter. Every action becomes
//! `Action(io <Action>IO, state_header StateHeader, chain0, chain, ...)`
//! where `io` is a pod2 record carrying one `in_<var>` entry per Input
//! and one `out_<var>` entry per Output (a Mutate contributes one of
//! each) and `state_header` is the grounding state root record threaded
//! down from `TxFinalized`. Each (action, object) tuple gets a bridge
//! predicate that pins the focused entry via `ArrayContains` and defers
//! to the action; the IsX OR is over those bridge predicates.

use crate::{
    ActionContext, ActionMeta, ActionObjectRef, ClassMeta, Dependency, Inst, Intro, Loader,
    ObjectIO, Ref, VarOrValue,
};
use std::collections::HashMap;
use std::fmt;
use txlib::RECORD_STATE_HEADER_PODLANG;

fn fmt_dependency(dep: &Dependency, w: &mut dyn fmt::Write) -> fmt::Result {
    match dep {
        Dependency::Module { name, hash } => {
            writeln!(w, "use module {:#} as {name}", hash)?;
        }
        Dependency::Intro { pred, hash } => {
            writeln!(w, "use intro {pred} from {:#}", hash)?;
        }
    }
    Ok(())
}

impl fmt::Display for Intro {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Vdf => write!(f, "Vdf"),
            Self::LtEqU256 => write!(f, "LtEqU256"),
        }
    }
}

/// Render a var at a given ts. Final ts renders as the bare name;
/// intermediate ts suffix the index. The txlib chain var (registered
/// as `"chain"`) flows through the same machinery; its pub-arg labels
/// in action signatures are `chain` (final, ts=max) and `chain0`
/// (initial, ts=0).
fn fmt_var_at(name: &str, ts: usize, max_ts: usize) -> String {
    if ts == max_ts {
        name.to_string()
    } else {
        format!("{name}{ts}")
    }
}

/// Output Objects reserve one extra ts beyond the script's last
/// DictUpdate to account for the update performed when TxInsert adds
/// the `stable_identifier` entry to the object. This is always the final
/// update to the object's state before output.
pub(crate) const fn output_max_ts(base_ts: usize, is_output: bool) -> usize {
    if is_output { base_ts + 1 } else { base_ts }
}

/// An action's chain max_ts must be at least this for the SDK to pack
/// intermediate chain states into a `<Action>Chain` record. Below the
/// threshold, the per-step scalar wildcards (`chain1`, `chain2`, ...)
/// fit in fewer slots than the record-typed wildcard would cost.
pub(crate) const CHAIN_PACK_MIN_TS: usize = 3;

/// True iff this action's chain is packed into a `<Action>Chain` record:
/// the schema is emitted, a `chain_steps` typed private wildcard appears
/// in the action signature, and intermediate chain refs render as
/// anchored `chain_steps.step_N` instead of scalar wildcards.
pub(crate) fn chain_packed(chain_max_ts: usize) -> bool {
    chain_max_ts >= CHAIN_PACK_MIN_TS
}

/// Schema name for an action's chain record (e.g. `LogToWoodChain`).
pub(crate) fn chain_schema_name(action_name: &str) -> String {
    format!("{action_name}Chain")
}

/// Slot in the `<Action>Chain` record for an intermediate chain ts when
/// this action's chain is packed. Returns `None` for endpoints
/// (`ts=0=chain0`, `ts=max_ts=chain`) and for unpacked actions (whose
/// intermediates are scalar `chain1`, `chain2`, ... wildcards). The
/// record's array layout is `[step_0_value, step_1_value, ...]`, so the
/// slot index is `ts - 1` and the step name is `step_{ts-1}`.
pub(crate) fn chain_step_at(ts: usize, chain_max_ts: usize) -> Option<usize> {
    (chain_packed(chain_max_ts) && ts > 0 && ts < chain_max_ts).then(|| ts - 1)
}

#[derive(Clone, Copy)]
struct VarNameFmt<'a> {
    name: &'a str,
    ts: usize,
    /// The owning action's metadata: the single source for this var's
    /// max ts and which record namespace (if any) it collapses to at a
    /// given ts. See `collapses_at`.
    meta: &'a ActionMeta,
}

impl<'a> VarNameFmt<'a> {
    fn inc(&mut self) {
        self.ts += 1;
    }
    fn next(&self) -> Self {
        Self {
            ts: self.ts + 1,
            ..*self
        }
    }
}

impl<'a> VarNameFmt<'a> {
    /// The record namespace this var pins at its current `ts`, or
    /// `None` to render as a bare wildcard. Delegates to
    /// `ActionMeta::collapsed_at`, which owns the in/out/initials
    /// resolution (and its precedence).
    fn collapses_at(&self) -> Option<Collapse> {
        self.meta.collapsed_at(self.name, self.ts)
    }
}

impl<'a> fmt::Display for VarNameFmt<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ns) = self.collapses_at() {
            return write!(f, "{}", ns.arg_name(self.name));
        }
        let max_ts = self.meta.max_ts(self.name);
        if self.name == "chain"
            && let Some(slot) = chain_step_at(self.ts, max_ts)
        {
            return write!(f, "chain_steps.step_{slot}");
        }
        write!(f, "{}", fmt_var_at(self.name, self.ts, max_ts))
    }
}

/// Which side of the `<Action>IO` record an Object inst's entry sits
/// on: `in_<var>` entries come first, then `out_<var>` entries (Mutate
/// contributes one entry to each side).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    In,
    Out,
}

/// The record namespace a collapsed Object state dict belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Collapse {
    IO(Side),
    Initials,
}

impl Side {
    pub(crate) fn arg_name(self, name: &str) -> String {
        match self {
            Side::In => format!("in_{name}"),
            Side::Out => format!("out_{name}"),
        }
    }
}

impl Collapse {
    pub(crate) fn arg_name(self, name: &str) -> String {
        match self {
            Collapse::IO(side) => format!("io.{}", side.arg_name(name)),
            Collapse::Initials => format!("initials.{name}"),
        }
    }
}

/// Render a Var arg as podlang text. Bare-named Vars use their
/// `VarNameFmt` rendering; `var.key` becomes `<rendered>.<key>`;
/// concrete values render literally.
struct ArgFmt<'a> {
    vars: &'a HashMap<&'a str, VarNameFmt<'a>>,
    arg: &'a Ref,
}

impl<'a> fmt::Display for ArgFmt<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let arg = self.arg.borrow();
        match &*arg {
            VarOrValue::Value(value) => write!(f, "{value}"),
            VarOrValue::Var(var) => match &var.key {
                Some(key) => write!(f, "{}.{key}", self.vars[var.name.as_str()]),
                None => write!(f, "{}", self.vars[var.name.as_str()]),
            },
        }
    }
}

/// Which record an Object's IsX OR branch dispatches on: inputs
/// dispatch on `io.in_X`; outputs and mutates dispatch on `io.out_X`.
/// The `io.in_` form of a mutate is intentionally excluded; replay's
/// mutate guard fires on the post-mutation form.
pub(crate) fn dispatch_side(io: &ObjectIO) -> Side {
    match io {
        ObjectIO::Input => Side::In,
        ObjectIO::Output | ObjectIO::Mutate => Side::Out,
    }
}

/// Initials schema name for a (action, namespace) pair, e.g. `LogToWoodInitials`.
fn schema_name_initials(action_name: &str) -> String {
    format!("{action_name}Initials")
}

/// IO schema name for a (action, namespace) pair, e.g. `LogToWoodIO`.
fn schema_name_io(action_name: &str) -> String {
    format!("{action_name}IO")
}

/// Emit `record <Action><Side> = (<entries>)` lines for any non-empty
/// io schema across all actions, plus `<Action>Chain` records for
/// actions whose chain has 2+ intermediate states.
fn fmt_record_decls(loader: &Loader, w: &mut dyn fmt::Write) -> fmt::Result {
    let render = |entries: &[String]| entries.join(", ");
    for meta in &loader.actions_meta {
        let names: Vec<String> = meta
            .in_entries
            .iter()
            .map(|e| Side::In.arg_name(&e.varname))
            .chain(
                meta.out_entries
                    .iter()
                    .map(|e| Side::Out.arg_name(&e.varname)),
            )
            .collect();
        writeln!(
            w,
            "record {} = ({})",
            schema_name_io(&meta.name),
            render(&names),
        )?;
        if chain_packed(meta.chain_max_ts) {
            // Intermediates: ts=1..=chain_max_ts-1 -> step_0..step_(K-2).
            let steps: Vec<String> = (0..meta.chain_max_ts - 1)
                .map(|i| format!("step_{i}"))
                .collect();
            writeln!(
                w,
                "record {} = ({})",
                chain_schema_name(&meta.name),
                render(&steps),
            )?;
        }
        if let Some(initials) = &meta.initials_entries {
            writeln!(
                w,
                "record {} = ({})",
                schema_name_initials(&meta.name),
                render(initials),
            )?;
        }
    }
    Ok(())
}

/// One sub-action call in the parent's body, with its synthesized
/// private wildcard names + record-shape info for the call.
struct SubActionCall {
    sub_name: String,
    /// Name of the parent's synthesized private wildcard for the sub's
    /// `io` record
    sub_io_var: String,
    /// Script-side alias name (the `pick` in `var pick = action.subaction(...)`).
    /// `None` if the user didn't bind via `var`. An alias referenced by
    /// the parent body becomes a parent wildcard pinned to the sub's io
    /// record; an unreferenced alias is skipped from the private
    /// wildcards list.
    alias: Option<String>,
    /// Name of the sub's first out entry, the one the alias refers to.
    /// `None` if the sub produces nothing.
    first_out_entry: Option<String>,
}

/// Walk the parent action's Insts and gather one `SubActionCall` per
/// `Inst::SubAction`. Looks up each sub's record shape from the loader's
/// `actions_meta`.
fn collect_sub_action_calls(action: &ActionContext, loader: &Loader) -> Vec<SubActionCall> {
    let mut calls = Vec::new();
    let mut idx_counter: HashMap<String, usize> = HashMap::new();
    for inst in &action.insts {
        if let Inst::SubAction {
            action: sub_name,
            obj,
            ..
        } = inst
        {
            let idx = *idx_counter.entry(sub_name.clone()).or_insert(0);
            *idx_counter.get_mut(sub_name).unwrap() += 1;

            let sub_io_var = format!("_{}_io_{}", sub_name, idx);

            let alias_name = obj.borrow().var_name().to_string();
            let alias = if alias_name == "?" {
                None
            } else {
                Some(alias_name)
            };
            let sub_meta = loader
                .actions_meta
                .iter()
                .find(|m| m.name == *sub_name)
                .expect("sub-action meta exists at fmt time");
            let first_out_entry = sub_meta.out_entries.first().map(|e| e.varname.clone());

            calls.push(SubActionCall {
                sub_name: sub_name.clone(),
                sub_io_var,
                alias,
                first_out_entry,
            });
        }
    }
    calls
}

/// Emit one action predicate. For each Object inst, sides whose
/// `needs_wildcard` is set get a leading `ArrayContains` clause + a
/// private wildcard; collapsed sides drop both and let body refs render
/// as `io.in_<entry>` / `io.out_<entry>` anchored refs. Witness vars
/// (e.g., values passed to `obj.update(k, v)`) appear as plain private
/// wildcards. Sub-action calls are emitted with a synthesized typed-
/// private wildcard `_<Sub>_io_<n>` matching the sub's record schema,
/// and pass the parent's `state_header` through.
fn fmt_action(action: &ActionContext, loader: &Loader, w: &mut dyn fmt::Write) -> fmt::Result {
    let meta = loader
        .actions_meta
        .iter()
        .find(|m| m.name == action.name)
        .expect("ActionMeta exists at fmt time");
    let sub_calls = collect_sub_action_calls(action, loader);

    // ---- Signature ----
    write!(w, "{}(", action.name)?;
    write!(w, "io {}, ", schema_name_io(&action.name))?;
    write!(w, "state_header StateHeader, chain0, chain")?;

    // Sub-action aliases: parent vars that hold a sub's first out
    // entry's Object Ref. A referenced alias becomes a real parent
    // wildcard (pinned to the sub's io record by an ArrayContains
    // clause below); an unreferenced alias is structural only and is
    // skipped from the private list.
    let referenced = crate::body_referenced_vars(&action.insts);
    let alias_names: std::collections::HashSet<String> = sub_calls
        .iter()
        .filter_map(|c| c.alias.clone())
        .filter(|alias| !referenced.contains(alias))
        .collect();

    // Private wildcards: every (var, ts) except sub-action aliases, state_header, chain
    // endpoints (public chain0/chain), packed chain intermediates (anchored via the
    // `chain_steps` record), Object pre/post-form ts on collapsed sides, and Output Objects'
    // script-final ts when packed into the `initials` record. Unpacked chain intermediates
    // appear as scalar `chain1, chain2, ...` privates.
    let mut private_vars: Vec<String> = Vec::new();
    for var in &action.vars {
        if alias_names.contains(var.as_str()) {
            continue;
        }
        let max_ts = meta.max_ts(var);
        for i in 0..=max_ts {
            let skip = if var == "chain" {
                i == 0 || i == max_ts || chain_step_at(i, max_ts).is_some()
            } else if var == "state_header" {
                true
            } else {
                meta.collapsed_at(var, i).is_some()
            };
            if skip {
                continue;
            }
            private_vars.push(fmt_var_at(var, i, max_ts));
        }
    }
    // Append synthesized sub-action typed privates last.
    for c in &sub_calls {
        let name = &c.sub_io_var;
        private_vars.push(format!("{name} {}", schema_name_io(&c.sub_name)));
    }
    // Append the chain record typed private when packed.
    if chain_packed(meta.chain_max_ts) {
        private_vars.push(format!("chain_steps {}", chain_schema_name(&action.name)));
    }
    // Append the initials record typed private when packed.
    if meta.initials_entries.is_some() {
        private_vars.push(format!("initials {}", schema_name_initials(&action.name),));
    }
    if !private_vars.is_empty() {
        write!(w, ", private: ")?;
        for (i, v) in private_vars.iter().enumerate() {
            if i != 0 {
                write!(w, ", ")?;
            }
            write!(w, "{v}")?;
        }
    }
    writeln!(w, ") = AND(")?;

    // Per-var rendering state for body emission. Each var holds a
    // back-reference to `meta` so `VarNameFmt::collapses_at` can
    // resolve whether it renders as `io.in_<name>` / `io.out_<name>` /
    // `initials.<name>` at a given ts.
    let mut vars: HashMap<&str, VarNameFmt> = action
        .vars
        .iter()
        .map(|v| {
            (
                v.as_str(),
                VarNameFmt {
                    name: v,
                    ts: 0,
                    meta,
                },
            )
        })
        .collect();

    // ---- ArrayContains clauses for each Object's pre/post-form on
    // sides that need a wildcard; collapsed sides drop the clause.
    for o in &meta.object_refs {
        let max_ts = meta.max_ts(&o.varname);
        if meta
            .in_entry(&o.varname)
            .is_some_and(|(_, e)| e.needs_wildcard)
        {
            writeln!(
                w,
                "  ArrayContains(io, {}::in_{}, {})",
                schema_name_io(&action.name),
                o.varname,
                fmt_var_at(&o.varname, 0, max_ts),
            )?;
        }
        if meta
            .out_entry(&o.varname)
            .is_some_and(|(_, e)| e.needs_wildcard)
        {
            writeln!(
                w,
                "  ArrayContains(io, {}::out_{}, {})",
                schema_name_io(&action.name),
                o.varname,
                fmt_var_at(&o.varname, max_ts, max_ts),
            )?;
        }
    }
    // Pin each referenced sub-action alias to its sub's first out
    // entry.
    for call in &sub_calls {
        let Some(alias) = &call.alias else { continue };
        if !referenced.contains(alias) {
            continue;
        }
        let Some(entry) = &call.first_out_entry else {
            continue;
        };
        writeln!(
            w,
            "  ArrayContains({}, {}::out_{}, {})",
            call.sub_io_var,
            schema_name_io(&call.sub_name),
            entry,
            fmt_var_at(alias, 0, meta.max_ts(alias)),
        )?;
    }

    // ---- Body (Insts other than Object) ----
    let mut sub_call_idx: usize = 0;
    for inst in &action.insts {
        match inst {
            Inst::Object { .. } => {}
            Inst::Set { obj, kvs, .. } => {
                let obj_str = vars[obj.as_str()];
                for (key, value) in kvs {
                    let value = ArgFmt {
                        vars: &vars,
                        arg: value,
                    };
                    writeln!(w, r#"  DictContains({obj_str}, "{key}", {value})"#,)?;
                }
            }
            Inst::Update {
                obj, key, value, ..
            } => {
                let obj_name = obj.as_str();
                let obj_fmt = vars[obj_name];
                let obj_next = obj_fmt.next();
                let value = ArgFmt {
                    vars: &vars,
                    arg: value,
                };
                writeln!(
                    w,
                    r#"  DictUpdate({obj_fmt}, "{key}", {value}, {obj_next})"#,
                )?;
                vars.get_mut(obj_name).expect("obj exists").inc();
            }
            Inst::Statement { pred, args } => {
                write!(w, "  {pred}(")?;
                for (i, arg) in args.iter().enumerate() {
                    if i != 0 {
                        write!(w, ", ")?;
                    }
                    write!(w, "{}", ArgFmt { vars: &vars, arg })?;
                }
                writeln!(w, ")")?;
            }
            Inst::Intro { pred, args, .. } => {
                write!(w, "  {pred}(")?;
                for (i, arg) in args.iter().enumerate() {
                    if i != 0 {
                        write!(w, ", ")?;
                    }
                    write!(w, "{}", ArgFmt { vars: &vars, arg })?;
                }
                writeln!(w, ")")?;
            }
            Inst::SubAction {
                action: sub_name, ..
            } => {
                let call = &sub_calls[sub_call_idx];
                sub_call_idx += 1;
                let chain = vars["chain"];
                let chain_next = chain.next();
                let mut args: Vec<String> = Vec::new();
                args.push(call.sub_io_var.clone());
                args.push("state_header".to_string());
                args.push(format!("{chain}"));
                args.push(format!("{chain_next}"));
                writeln!(w, "  {sub_name}({})", args.join(", "))?;
                vars.get_mut("chain").expect("chain exists").inc();
            }
        }
    }

    // ---- Per-Object Tx event lines ----
    // The Tx primitive checks `DictContains(<obj>, "type", <guard>)`
    // internally, so the guard predicate ref is passed as the last
    // arg to TxInsert / TxDelete / TxMutate (and pins both sides for
    // mutate, making the type-preservation check implicit).
    // For Output, TxInsert produces a final state with a
    // "stable_identifier" entry, so it takes the object in two forms.
    // `obj_str` (the
    // script's final wildcard) is the pre-identity `initial` dict the
    // script built; `obj_with_id` (the same wildcard one ts later, the
    // extra ts that `output_max_ts` reserves) is the post-identity `new`
    // dict TxInsert produces. The post-identity form is the object's
    // output state: it is what the transaction inserts, and it collapses
    // to `io.out_<name>` to bind the `<Action>IO` record entry.
    for o in &meta.object_refs {
        let chain = vars["chain"];
        let chain_next = chain.next();
        let obj_str = vars[o.varname.as_str()];
        let class = &o.class;
        match o.io {
            ObjectIO::Input => writeln!(
                w,
                "  tx::TxDelete({chain}, {chain_next}, {obj_str}, @self_predicate(Is{class}))"
            )?,
            ObjectIO::Output => {
                let obj_with_id = obj_str.next();
                writeln!(
                    w,
                    "  tx::TxInsert({chain}, {chain_next}, {obj_str}, {obj_with_id}, @self_predicate(Is{class}))"
                )?;
            }
            ObjectIO::Mutate => {
                let mut pre = vars[o.varname.as_str()];
                pre.ts = 0;
                writeln!(
                    w,
                    "  tx::TxMutate({chain}, {chain_next}, {pre}, {obj_str}, @self_predicate(Is{class}))"
                )?;
            }
        }
        vars.get_mut("chain").expect("chain exists").inc();
    }
    writeln!(w, ")")?;
    Ok(())
}

/// True if `class` appears on more than one Object inst (any side) in
/// this action's `object_refs`. Such classes need their bridge predicate
/// names differentiated by varname suffix; the OR over bridges
/// enumerates one branch per (action, object-of-class).
fn is_multi_class(objects: &[ActionObjectRef], class: &str) -> bool {
    objects.iter().filter(|o| o.class == class).count() > 1
}

pub(crate) fn bridge_predicate_name(class: &str, action: &str, entry: &str, multi: bool) -> String {
    if multi {
        format!("Is{class}From{action}_{entry}")
    } else {
        format!("Is{class}From{action}")
    }
}

/// Emit one bridge predicate per (action, object) tuple.
fn fmt_bridges(loader: &Loader, w: &mut dyn fmt::Write) -> fmt::Result {
    for meta in &loader.actions_meta {
        for o in &meta.object_refs {
            let side = dispatch_side(&o.io);
            let multi = is_multi_class(&meta.object_refs, &o.class);
            let bridge_name = bridge_predicate_name(&o.class, &meta.name, &o.varname, multi);

            // Bridge predicate signature: state, state_header, chain0, chain (public);
            // io <ActionIO> private when the action has any entries.
            write!(w, "{bridge_name}(state, state_header, chain0, chain")?;
            write!(w, ", private: io {}", schema_name_io(&meta.name))?;
            writeln!(w, ") = AND(")?;

            // ArrayContains(io, <Schema>::<entry>, state)
            writeln!(
                w,
                "  ArrayContains(io, {}::{}, state)",
                schema_name_io(&meta.name),
                side.arg_name(&o.varname),
            )?;

            // Action call.
            let call_args = ["io", "state_header", "chain0", "chain"];
            writeln!(w, "  {}({})", meta.name, call_args.join(", "))?;

            writeln!(w, ")")?;
            writeln!(w)?;
        }
    }
    Ok(())
}

/// Emit IsX OR over bridge predicates.
fn fmt_class(loader: &Loader, w: &mut dyn fmt::Write, class: &ClassMeta) -> fmt::Result {
    let name = &class.name;
    writeln!(
        w,
        "Is{name}(state, state_header StateHeader, chain0, chain) = OR("
    )?;
    for (action_name, obj_index) in &class.actions {
        let meta = loader
            .actions_meta
            .iter()
            .find(|m| &m.name == action_name)
            .expect("action exists");
        let o = &meta.object_refs[*obj_index];
        let multi = is_multi_class(&meta.object_refs, &o.class);
        let bridge_name = bridge_predicate_name(&o.class, action_name, &o.varname, multi);
        writeln!(w, "  {bridge_name}(state, state_header, chain0, chain)")?;
    }
    // Transfer of control: every generated class is transferable. The
    // self-predicate hash pins the transfer to this class, so a Rekey
    // proven for some other class cannot satisfy this guard.
    writeln!(
        w,
        "  rk::Rekey(state, chain0, chain, @self_predicate(Is{name}))"
    )?;
    writeln!(w, ")")?;
    Ok(())
}

pub(crate) fn fmt(loader: &Loader, w: &mut dyn fmt::Write) -> fmt::Result {
    for dep in &loader.dependencies {
        fmt_dependency(dep, w).unwrap();
    }
    writeln!(w)?;

    // TODO: Support importing records via `use module`, so that we can import this record from
    // `tx`
    writeln!(w, "{}", &*RECORD_STATE_HEADER_PODLANG)?;
    fmt_record_decls(loader, w)?;
    writeln!(w, "\n// Actions\n")?;
    for action in &loader.actions {
        fmt_action(&action.0.borrow(), loader, w)?;
        writeln!(w)?;
    }
    writeln!(w, "// Bridges\n")?;
    fmt_bridges(loader, w)?;
    writeln!(w, "// Classes\n")?;
    for class in &loader.classes {
        fmt_class(loader, w, class)?;
        writeln!(w)?;
    }
    Ok(())
}
