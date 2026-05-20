//! Functions used to format to podlang source code.
//!
//! This is the records-form emitter. Every action becomes
//! `Action(in <Action>In, out <Action>Out, chain0, chain, ...)` where the
//! `in`/`out` typed wildcards are pod2 records carrying one entry per
//! Object inst on that side. Each (action, object) tuple gets a bridge
//! predicate that pins the focused entry via `ArrayContains` and defers
//! to the action; the IsX OR is over those bridge predicates.

use crate::{
    ActionContext, ActionObjectRef, ClassMeta, Dependency, Inst, Intro, Loader, ObjectIO, Ref,
    VarOrValue,
};
use std::collections::HashMap;
use std::fmt;

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

/// Slot 0 placeholder in every records-form schema (`<Action>In`,
/// `<Action>Out`, `<Action>Chain`). Real entries start at slot 1.
/// Works around pod2 issue #513.
pub(crate) const PAD_ENTRY: &str = "_pad";

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
/// record's array layout is `[_pad, step_0_value, step_1_value, ...]`,
/// so the slot index equals `ts` and the step name is `step_{ts-1}`.
pub(crate) fn chain_step_at(ts: usize, chain_max_ts: usize) -> Option<usize> {
    (chain_packed(chain_max_ts) && ts > 0 && ts < chain_max_ts).then_some(ts)
}

#[derive(Clone, Copy)]
struct VarNameFmt<'a> {
    name: &'a str,
    ts: usize,
    max_ts: usize,
    /// Object Ref's pre-form collapses to `in.<name>`. False for
    /// non-Object vars and Output-only Objects.
    collapsed_in: bool,
    /// Object Ref's post-form collapses to `out.<name>`. False for
    /// non-Object vars and Input-only Objects.
    collapsed_out: bool,
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

impl<'a> fmt::Display for VarNameFmt<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // For a Mutate Object at ts=0=max (no updates), `in` wins.
        if self.collapsed_in && self.ts == 0 {
            return write!(f, "in.{}", self.name);
        }
        if self.collapsed_out && self.ts == self.max_ts {
            return write!(f, "out.{}", self.name);
        }
        if self.name == "chain"
            && let Some(slot) = chain_step_at(self.ts, self.max_ts)
        {
            return write!(f, "chain_steps.step_{}", slot - 1);
        }
        write!(f, "{}", fmt_var_at(self.name, self.ts, self.max_ts))
    }
}

/// One of the two record args in an action's signature
/// `Action(in <Action>In, out <Action>Out, ...)`. Each Object inst
/// contributes one entry to one (Input/Output) or both (Mutate) of
/// these records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Side {
    In,
    Out,
}

impl Side {
    pub(crate) fn arg_name(self) -> &'static str {
        match self {
            Side::In => "in",
            Side::Out => "out",
        }
    }
    fn schema_suffix(self) -> &'static str {
        match self {
            Side::In => "In",
            Side::Out => "Out",
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
/// dispatch on `in.X`; outputs and mutates dispatch on `out.X`.
/// The `in` form of a mutate is intentionally excluded; replay's
/// mutate guard fires on the post-mutation form.
pub(crate) fn dispatch_side(io: &ObjectIO) -> Side {
    match io {
        ObjectIO::Input => Side::In,
        ObjectIO::Output | ObjectIO::Mutate => Side::Out,
    }
}

/// Schema name for a (action, side) pair, e.g. `LogToWoodIn`.
fn schema_name(action_name: &str, side: Side) -> String {
    format!("{action_name}{}", side.schema_suffix())
}

/// Emit `record <Action><Side> = (<entries>)` lines for any non-empty
/// in/out schema across all actions, plus `<Action>Chain` records for
/// actions whose chain has 2+ intermediate states. Each schema is
/// prepended with `PAD_ENTRY` so real entries start at index 1.
fn fmt_record_decls(loader: &Loader, w: &mut dyn fmt::Write) -> fmt::Result {
    let render = |entries: &[String]| {
        std::iter::once(PAD_ENTRY.to_string())
            .chain(entries.iter().cloned())
            .collect::<Vec<_>>()
            .join(", ")
    };
    for meta in &loader.actions_meta {
        if !meta.in_entries.is_empty() {
            let names: Vec<String> = meta.in_entries.iter().map(|e| e.varname.clone()).collect();
            writeln!(
                w,
                "record {} = ({})",
                schema_name(&meta.name, Side::In),
                render(&names),
            )?;
        }
        if !meta.out_entries.is_empty() {
            let names: Vec<String> = meta.out_entries.iter().map(|e| e.varname.clone()).collect();
            writeln!(
                w,
                "record {} = ({})",
                schema_name(&meta.name, Side::Out),
                render(&names),
            )?;
        }
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
    }
    Ok(())
}

/// One sub-action call in the parent's body, with its synthesized
/// private wildcard names + record-shape info for the call.
struct SubActionCall {
    sub_name: String,
    /// Name of the parent's synthesized private wildcard for the sub's
    /// `in` record (None if the sub has no in record).
    sub_in_var: Option<String>,
    /// Same, for the sub's `out` record.
    sub_out_var: Option<String>,
    /// Script-side alias name (the `pick` in `var pick = action.subaction(...)`).
    /// `None` if the user didn't bind via `var`. Used to skip the alias from
    /// the parent's private wildcards list.
    alias: Option<String>,
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
            let sub_meta = loader
                .actions_meta
                .iter()
                .find(|m| &m.name == sub_name)
                .unwrap_or_else(|| panic!("sub-action {sub_name} not in loader.actions_meta"));
            let has_in = !sub_meta.in_entries.is_empty();
            let has_out = !sub_meta.out_entries.is_empty();

            let idx = *idx_counter.entry(sub_name.clone()).or_insert(0);
            *idx_counter.get_mut(sub_name).unwrap() += 1;

            let sub_in_var = if has_in {
                Some(format!("_{}_in_{}", sub_name, idx))
            } else {
                None
            };
            let sub_out_var = if has_out {
                Some(format!("_{}_out_{}", sub_name, idx))
            } else {
                None
            };

            let alias_name = obj.borrow().var_name().to_string();
            let alias = if alias_name == "?" {
                None
            } else {
                Some(alias_name)
            };

            calls.push(SubActionCall {
                sub_name: sub_name.clone(),
                sub_in_var,
                sub_out_var,
                alias,
            });
        }
    }
    calls
}

/// Emit one action predicate. For each Object inst, sides whose
/// `needs_wildcard` is set get a leading `ArrayContains` clause + a
/// private wildcard; collapsed sides drop both and let body refs render
/// as `in.<entry>` / `out.<entry>` anchored refs. Witness vars (e.g.,
/// values passed to `obj.update(k, v)`) appear as plain private
/// wildcards. Sub-action calls are emitted with synthesized typed-
/// private wildcards `_<Sub>_in_<n>` / `_<Sub>_out_<n>` matching the
/// sub's record schemas.
fn fmt_action(action: &ActionContext, loader: &Loader, w: &mut dyn fmt::Write) -> fmt::Result {
    let meta = loader
        .actions_meta
        .iter()
        .find(|m| m.name == action.name)
        .expect("ActionMeta exists at fmt time");
    let sub_calls = collect_sub_action_calls(action, loader);

    // ---- Signature ----
    write!(w, "{}(", action.name)?;
    let mut wrote_pub = false;
    if !meta.in_entries.is_empty() {
        write!(w, "in {}", schema_name(&action.name, Side::In))?;
        wrote_pub = true;
    }
    if !meta.out_entries.is_empty() {
        if wrote_pub {
            write!(w, ", ")?;
        }
        write!(w, "out {}", schema_name(&action.name, Side::Out))?;
        wrote_pub = true;
    }
    if wrote_pub {
        write!(w, ", ")?;
    }
    write!(w, "chain0, chain")?;

    // Sub-action aliases: parent vars that hold a sub's first producing
    // Object Ref. They're not real wildcards in the parent's predicate
    // (the binding is structural to the script, not the proof) so we
    // skip them from the private list.
    let alias_names: std::collections::HashSet<String> =
        sub_calls.iter().filter_map(|c| c.alias.clone()).collect();

    // Private wildcards: every (var, ts) except sub-action aliases,
    // chain endpoints (public chain0/chain), packed chain intermediates
    // (anchored via the `chain_steps` record), and Object pre/post-form
    // ts on collapsed sides. Unpacked chain intermediates appear as
    // scalar `chain1, chain2, ...` privates.
    let mut private_vars: Vec<String> = Vec::new();
    for var in &action.vars {
        if alias_names.contains(var.as_str()) {
            continue;
        }
        let max_ts = action.var_state[var].ts;
        for i in 0..=max_ts {
            let skip = if var == "chain" {
                i == 0 || i == max_ts || chain_step_at(i, max_ts).is_some()
            } else {
                meta.collapsed_at(var, i, max_ts).is_some()
            };
            if skip {
                continue;
            }
            private_vars.push(fmt_var_at(var, i, max_ts));
        }
    }
    // Append synthesized sub-action typed privates last.
    for c in &sub_calls {
        if let Some(name) = &c.sub_in_var {
            private_vars.push(format!("{name} {}", schema_name(&c.sub_name, Side::In)));
        }
        if let Some(name) = &c.sub_out_var {
            private_vars.push(format!("{name} {}", schema_name(&c.sub_name, Side::Out)));
        }
    }
    // Append the chain record typed private when packed.
    if chain_packed(meta.chain_max_ts) {
        private_vars.push(format!("chain_steps {}", chain_schema_name(&action.name)));
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

    // Per-var rendering state for body emission. Object vars carry
    // the per-side collapse flags so `VarNameFmt` can pick
    // `in.<name>` / `out.<name>` over the bare name when collapsed.
    let mut vars: HashMap<&str, VarNameFmt> = action
        .vars
        .iter()
        .map(|v| {
            let max_ts = action.var_state[v].ts;
            let collapsed_in = meta.collapsed_at(v, 0, max_ts) == Some(Side::In);
            let collapsed_out = meta.collapsed_at(v, max_ts, max_ts) == Some(Side::Out);
            (
                v.as_str(),
                VarNameFmt {
                    name: v,
                    ts: 0,
                    max_ts,
                    collapsed_in,
                    collapsed_out,
                },
            )
        })
        .collect();

    // ---- ArrayContains clauses for each Object's pre/post-form on
    // sides that need a wildcard; collapsed sides drop the clause.
    for o in &meta.object_refs {
        let max_ts = action.var_state[&o.varname].ts;
        if meta
            .in_entry(&o.varname)
            .is_some_and(|(_, e)| e.needs_wildcard)
        {
            writeln!(
                w,
                "  ArrayContains(in, {}::{}, {})",
                schema_name(&action.name, Side::In),
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
                "  ArrayContains(out, {}::{}, {})",
                schema_name(&action.name, Side::Out),
                o.varname,
                fmt_var_at(&o.varname, max_ts, max_ts),
            )?;
        }
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
                    r#"  DictUpdate({obj_next}, {obj_fmt}, "{key}", {value})"#,
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
                if let Some(name) = &call.sub_in_var {
                    args.push(name.clone());
                }
                if let Some(name) = &call.sub_out_var {
                    args.push(name.clone());
                }
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
    for o in &meta.object_refs {
        let chain = vars["chain"];
        let chain_next = chain.next();
        let obj_str = vars[o.varname.as_str()];
        let class = &o.class;
        match o.io {
            ObjectIO::Input => writeln!(
                w,
                "  tx::TxDelete({chain_next}, {chain}, {obj_str}, @self_predicate(Is{class}))"
            )?,
            ObjectIO::Output => writeln!(
                w,
                "  tx::TxInsert({chain_next}, {chain}, {obj_str}, @self_predicate(Is{class}))"
            )?,
            ObjectIO::Mutate => {
                let mut pre = vars[o.varname.as_str()];
                pre.ts = 0;
                writeln!(
                    w,
                    "  tx::TxMutate({chain_next}, {chain}, {obj_str}, {pre}, @self_predicate(Is{class}))"
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

            // Bridge predicate signature: state, chain0, chain (public);
            // in <ActionIn>, out <ActionOut> private as needed.
            write!(w, "{bridge_name}(state, chain0, chain")?;
            let mut priv_parts: Vec<String> = Vec::new();
            if !meta.in_entries.is_empty() {
                priv_parts.push(format!("in {}", schema_name(&meta.name, Side::In)));
            }
            if !meta.out_entries.is_empty() {
                priv_parts.push(format!("out {}", schema_name(&meta.name, Side::Out)));
            }
            if !priv_parts.is_empty() {
                write!(w, ", private: {}", priv_parts.join(", "))?;
            }
            writeln!(w, ") = AND(")?;

            // ArrayContains(<side>, <Schema>::<entry>, state)
            writeln!(
                w,
                "  ArrayContains({}, {}::{}, state)",
                side.arg_name(),
                schema_name(&meta.name, side),
                o.varname,
            )?;

            // Action call.
            let mut call_args: Vec<String> = Vec::new();
            if !meta.in_entries.is_empty() {
                call_args.push("in".to_string());
            }
            if !meta.out_entries.is_empty() {
                call_args.push("out".to_string());
            }
            call_args.push("chain0".to_string());
            call_args.push("chain".to_string());
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
    writeln!(w, "Is{name}(state, chain0, chain) = OR(")?;
    for (action_name, obj_index) in &class.actions {
        let meta = loader
            .actions_meta
            .iter()
            .find(|m| &m.name == action_name)
            .expect("action exists");
        let o = &meta.object_refs[*obj_index];
        let multi = is_multi_class(&meta.object_refs, &o.class);
        let bridge_name = bridge_predicate_name(&o.class, action_name, &o.varname, multi);
        writeln!(w, "  {bridge_name}(state, chain0, chain)")?;
    }
    writeln!(w, ")")?;
    Ok(())
}

pub(crate) fn fmt(loader: &Loader, w: &mut dyn fmt::Write) -> fmt::Result {
    for dep in &loader.dependencies {
        fmt_dependency(dep, w).unwrap();
    }
    writeln!(w)?;
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
