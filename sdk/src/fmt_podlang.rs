//! Functions used to format to podlang source code.
//!
//! This is the records-form emitter. Every action becomes
//! `Action(in <Action>In, out <Action>Out, chain0, chain, ...)` where the
//! `in`/`out` typed wildcards are pod2 records carrying one entry per
//! Object inst on that side. Each (action, object) tuple gets a bridge
//! predicate that pins the focused entry via `ArrayContains` and defers
//! to the action; the IsX OR is over those bridge predicates.

use crate::{ActionContext, ClassMeta, Dependency, Inst, Intro, Loader, ObjectIO, Ref, VarOrValue};
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

#[derive(Clone, Copy)]
struct VarNameFmt<'a> {
    name: &'a str,
    ts: usize,
    max_ts: usize,
    /// For Object Refs, the IO of the Object. None for non-Object vars.
    obj_io: Option<ObjectIO>,
    /// Whether the Object's `in` entry needs a wildcard. Meaningless
    /// for non-Object vars.
    needs_in_wildcard: bool,
    /// Whether the Object's `out` entry needs a wildcard. Meaningless
    /// for non-Object vars.
    needs_out_wildcard: bool,
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
    /// True if this Object Ref is at its pre-form (ts=0) AND its `in`
    /// entry is collapsed. Triggers `in.<name>` rendering instead of
    /// the bare name.
    fn at_collapsed_in(&self) -> bool {
        match self.obj_io {
            Some(ObjectIO::Input) | Some(ObjectIO::Mutate) => {
                self.ts == 0 && !self.needs_in_wildcard
            }
            _ => false,
        }
    }
    /// True if this Object Ref is at its post-form (ts=max) AND its
    /// `out` entry is collapsed.
    fn at_collapsed_out(&self) -> bool {
        match self.obj_io {
            Some(ObjectIO::Output) | Some(ObjectIO::Mutate) => {
                self.ts == self.max_ts && !self.needs_out_wildcard
            }
            _ => false,
        }
    }
}

impl<'a> fmt::Display for VarNameFmt<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // For a Mutate Object at ts=0=max (no updates), `in` wins.
        if self.at_collapsed_in() {
            return write!(f, "in.{}", self.name);
        }
        if self.at_collapsed_out() {
            return write!(f, "out.{}", self.name);
        }
        write!(f, "{}", fmt_var_at(self.name, self.ts, self.max_ts))
    }
}

/// One of the two record args in an action's signature
/// `Action(in <Action>In, out <Action>Out, ...)`. Each Object inst
/// contributes one entry to one (Input/Output) or both (Mutate) of
/// these records.
#[derive(Clone, Copy, Debug)]
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

/// Per-action info needed to emit the records-form predicate. Owned
/// strings throughout to keep clear of `RefCell` borrow lifetimes.
struct ActionInfo {
    /// Object insts in declaration order.
    objects: Vec<ObjectInfo>,
    in_entries: Vec<String>,  // var names (script-side) on the in side
    out_entries: Vec<String>, // ditto, out side
}

struct ObjectInfo {
    varname: String,
    io: ObjectIO,
    class: String,
}

fn collect_action_info(action: &ActionContext) -> ActionInfo {
    let mut objects = Vec::new();
    let mut in_entries: Vec<String> = Vec::new();
    let mut out_entries: Vec<String> = Vec::new();
    for inst in &action.insts {
        if let Inst::Object { io, obj, class, .. } = inst {
            let varname: String = obj.borrow().var_name().to_string();
            match io {
                ObjectIO::Input | ObjectIO::Mutate => in_entries.push(varname.clone()),
                ObjectIO::Output => {}
            }
            match io {
                ObjectIO::Output | ObjectIO::Mutate => out_entries.push(varname.clone()),
                ObjectIO::Input => {}
            }
            objects.push(ObjectInfo {
                varname,
                io: *io,
                class: class.clone(),
            });
        }
    }
    ActionInfo {
        objects,
        in_entries,
        out_entries,
    }
}

/// Emit `record <Action><Side> = (<entries>)` lines for any non-empty
/// in/out schema across all actions. Each schema is prepended with a
/// `_pad` entry so real entries start at index 1; see the comment in
/// `ActionHandle::exe_action` (around the `in_dicts` init) for why.
fn fmt_record_decls(loader: &Loader, w: &mut dyn fmt::Write) -> fmt::Result {
    let render = |entries: &[String]| {
        std::iter::once("_pad".to_string())
            .chain(entries.iter().cloned())
            .collect::<Vec<_>>()
            .join(", ")
    };
    for action_handle in &loader.actions {
        let action = action_handle.0.borrow();
        let info = collect_action_info(&action);
        if !info.in_entries.is_empty() {
            writeln!(
                w,
                "record {} = ({})",
                schema_name(&action.name, Side::In),
                render(&info.in_entries),
            )?;
        }
        if !info.out_entries.is_empty() {
            writeln!(
                w,
                "record {} = ({})",
                schema_name(&action.name, Side::Out),
                render(&info.out_entries),
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
            let has_in = sub_meta.local_inputs().count() > 0;
            let has_out = sub_meta.local_outputs().count() > 0;

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
/// `needs_*_wildcard` is set get a leading `ArrayContains` clause + a
/// private wildcard; collapsed sides drop both and let body refs render
/// as `in.<entry>` / `out.<entry>` anchored refs. Witness vars (e.g.,
/// values passed to `obj.update(k, v)`) appear as plain private
/// wildcards. Sub-action calls are emitted with synthesized typed-
/// private wildcards `_<Sub>_in_<n>` / `_<Sub>_out_<n>` matching the
/// sub's record schemas.
fn fmt_action(action: &ActionContext, loader: &Loader, w: &mut dyn fmt::Write) -> fmt::Result {
    let info = collect_action_info(action);
    let sub_calls = collect_sub_action_calls(action, loader);
    let meta = loader
        .actions_meta
        .iter()
        .find(|m| m.name == action.name)
        .expect("ActionMeta exists at fmt time");

    let object_io: HashMap<&str, ObjectIO> = info
        .objects
        .iter()
        .map(|o| (o.varname.as_str(), o.io))
        .collect();

    // ---- Signature ----
    write!(w, "{}(", action.name)?;
    let mut wrote_pub = false;
    if !info.in_entries.is_empty() {
        write!(w, "in {}", schema_name(&action.name, Side::In))?;
        wrote_pub = true;
    }
    if !info.out_entries.is_empty() {
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

    let needs_in = |varname: &str| meta.needs_in_wildcard.contains(varname);
    let needs_out = |varname: &str| meta.needs_out_wildcard.contains(varname);

    // Private wildcards: every (var, ts) except sub-action aliases,
    // the chain's ts=0/max (public as chain0/chain), and Object pre/
    // post-form ts on collapsed sides.
    let mut private_vars: Vec<String> = Vec::new();
    for var in &action.vars {
        if alias_names.contains(var.as_str()) {
            continue;
        }
        let max_ts = action.var_state[var].ts;
        let obj = object_io.get(var.as_str()).copied();
        for i in 0..=max_ts {
            if var == "chain" && (i == 0 || i == max_ts) {
                continue;
            }
            if let Some(io) = obj {
                let at_in =
                    matches!(io, ObjectIO::Input | ObjectIO::Mutate) && i == 0 && !needs_in(var);
                let at_out = matches!(io, ObjectIO::Output | ObjectIO::Mutate)
                    && i == max_ts
                    && !needs_out(var);
                if at_in || at_out {
                    continue;
                }
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
    // their io + per-side `needs_*_wildcard` so VarNameFmt can pick
    // `in.<name>` / `out.<name>` over the bare name when collapsed.
    let mut vars: HashMap<&str, VarNameFmt> = action
        .vars
        .iter()
        .map(|v| {
            let obj_io = object_io.get(v.as_str()).copied();
            (
                v.as_str(),
                VarNameFmt {
                    name: v,
                    ts: 0,
                    max_ts: action.var_state[v].ts,
                    obj_io,
                    needs_in_wildcard: needs_in(v),
                    needs_out_wildcard: needs_out(v),
                },
            )
        })
        .collect();

    // ---- ArrayContains clauses for each Object's pre/post-form on
    // sides that need a wildcard; collapsed sides drop the clause.
    for o in &info.objects {
        let max_ts = action.var_state[&o.varname].ts;
        let pre_ssa = fmt_var_at(&o.varname, 0, max_ts);
        let post_ssa = fmt_var_at(&o.varname, max_ts, max_ts);
        let in_collapsed = !needs_in(&o.varname);
        let out_collapsed = !needs_out(&o.varname);
        let emit_in = matches!(o.io, ObjectIO::Input | ObjectIO::Mutate) && !in_collapsed;
        let emit_out = matches!(o.io, ObjectIO::Output | ObjectIO::Mutate) && !out_collapsed;
        if emit_in {
            writeln!(
                w,
                "  ArrayContains(in, {}::{}, {})",
                schema_name(&action.name, Side::In),
                o.varname,
                pre_ssa,
            )?;
        }
        if emit_out {
            writeln!(
                w,
                "  ArrayContains(out, {}::{}, {})",
                schema_name(&action.name, Side::Out),
                o.varname,
                post_ssa,
            )?;
        }
    }

    // ---- Body (Insts other than Object) ----
    let mut objs: Vec<(ObjectIO, String, String)> = Vec::new();
    let mut sub_call_idx: usize = 0;
    for inst in &action.insts {
        match inst {
            Inst::Object { io, obj, class, .. } => {
                let varname = obj.borrow().var_name().to_string();
                objs.push((*io, varname, class.clone()));
            }
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

    // ---- Per-Object type guard + Tx event lines ----
    for (io, varname, class) in &objs {
        // Mutate guards the pre-form (ts=0); Input/Output guard the
        // post-form (the current ts). VarNameFmt picks the bare or
        // anchored rendering per the side's `needs_*_wildcard`.
        let mut guard_fmt = vars[varname.as_str()];
        if matches!(io, ObjectIO::Mutate) {
            guard_fmt.ts = 0;
        }
        writeln!(
            w,
            r#"  DictContains({guard_fmt}, "type", @self_predicate(Is{class}))"#
        )?;
        let chain = vars["chain"];
        let chain_next = chain.next();
        let obj_str = vars[varname.as_str()];
        match io {
            ObjectIO::Input => writeln!(w, "  tx::TxDelete({chain_next}, {chain}, {obj_str})")?,
            ObjectIO::Output => writeln!(w, "  tx::TxInsert({chain_next}, {chain}, {obj_str})")?,
            ObjectIO::Mutate => {
                let mut pre = vars[varname.as_str()];
                pre.ts = 0;
                writeln!(w, "  tx::TxMutate({chain_next}, {chain}, {obj_str}, {pre})")?;
            }
        }
        vars.get_mut("chain").expect("chain exists").inc();
    }
    writeln!(w, ")")?;
    Ok(())
}

/// True if `class` appears on more than one Object inst (any side) in
/// this action's `objects`. Such classes need their bridge predicate
/// names differentiated by varname suffix; the OR over bridges
/// enumerates one branch per (action, object-of-class).
fn is_multi_class(objects: &[ObjectInfo], class: &str) -> bool {
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
    for action_handle in &loader.actions {
        let action = action_handle.0.borrow();
        let info = collect_action_info(&action);
        for o in &info.objects {
            let side = dispatch_side(&o.io);
            let multi = is_multi_class(&info.objects, &o.class);
            let bridge_name = bridge_predicate_name(&o.class, &action.name, &o.varname, multi);

            // Bridge predicate signature: state, chain0, chain (public);
            // in <ActionIn>, out <ActionOut> private as needed.
            write!(w, "{bridge_name}(state, chain0, chain")?;
            let mut priv_parts: Vec<String> = Vec::new();
            if !info.in_entries.is_empty() {
                priv_parts.push(format!("in {}", schema_name(&action.name, Side::In)));
            }
            if !info.out_entries.is_empty() {
                priv_parts.push(format!("out {}", schema_name(&action.name, Side::Out)));
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
                schema_name(&action.name, side),
                o.varname,
            )?;

            // Action call.
            let mut call_args: Vec<String> = Vec::new();
            if !info.in_entries.is_empty() {
                call_args.push("in".to_string());
            }
            if !info.out_entries.is_empty() {
                call_args.push("out".to_string());
            }
            call_args.push("chain0".to_string());
            call_args.push("chain".to_string());
            writeln!(w, "  {}({})", action.name, call_args.join(", "))?;

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
        let action_handle = loader
            .actions
            .iter()
            .find(|h| &h.0.borrow().name == action_name)
            .expect("action exists");
        let action = action_handle.0.borrow();
        let info = collect_action_info(&action);
        let o = &info.objects[*obj_index];
        let multi = is_multi_class(&info.objects, &o.class);
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
