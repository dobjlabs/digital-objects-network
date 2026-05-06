//! Functions used to format to podlang source code.
//!
//! This is the records-form emitter. Every action becomes
//! `Action(in <Action>In, out <Action>Out, chain0, chain, ...)` where the
//! `in`/`out` typed wildcards are pod2 records carrying one entry per
//! Object inst on that side. Each (action, object) tuple gets a bridge
//! predicate that pins the focused entry via `ArrayContains` and defers
//! to the action; the IsX OR is over those bridge predicates.
//!
//! See `docs/plans/action_records.md` for the full design.

use crate::{
    ActionContext, ClassMeta, Dependency, Inst, Intro, Loader, ObjectIO, Ref, Var, VarOrValue,
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

/// Render a var at a given timestamp. Final ts renders as the bare
/// name; intermediate timestamps suffix the index. SSA-style
/// disambiguation for vars rewritten by mutations. The txlib chain
/// var (registered as `"chain"`) flows through the same machinery; its
/// pub-arg labels in action signatures are `chain` (final, ts=max) and
/// `chain0` (initial, ts=0).
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
        write!(f, "{}", fmt_var_at(self.name, self.ts, self.max_ts))
    }
}

#[derive(Clone, Copy)]
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

/// Render a Var arg as podlang text. Bare-named Vars become their
/// SSA-rendered name; `var.key` becomes `<rendered>.<key>`; concrete
/// values render literally.
struct ArgFmt<'a> {
    vars: &'a HashMap<&'a str, VarNameFmt<'a>>,
    arg: &'a Ref,
}

impl<'a> fmt::Display for ArgFmt<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let arg = self.arg.borrow();
        match &*arg {
            VarOrValue::Var(Var {
                name, key: None, ..
            }) => write!(f, "{}", self.vars[name.as_str()]),
            VarOrValue::Var(Var {
                name,
                key: Some(key),
                ..
            }) => write!(f, "{}.{key}", self.vars[name.as_str()]),
            VarOrValue::Value(value) => write!(f, "{value}"),
        }
    }
}

/// IsX dispatch side for an Object inst: inputs dispatch on `in.X`;
/// outputs and mutates dispatch on `out.X`. The input side of a mutate
/// is intentionally excluded; replay's mutate guard fires on the
/// post-mutation form.
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
    /// Object insts in declaration order. Each Object becomes a flat SSA
    /// wildcard plus a leading `ArrayContains` boundary clause.
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
                io: io.clone(),
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
/// in/out schema across all actions.
fn fmt_record_decls(loader: &Loader, w: &mut dyn fmt::Write) -> fmt::Result {
    for action_handle in &loader.actions {
        let action = action_handle.0.borrow();
        let info = collect_action_info(&action);
        if !info.in_entries.is_empty() {
            writeln!(
                w,
                "record {} = ({})",
                schema_name(&action.name, Side::In),
                info.in_entries.join(", ")
            )?;
        }
        if !info.out_entries.is_empty() {
            writeln!(
                w,
                "record {} = ({})",
                schema_name(&action.name, Side::Out),
                info.out_entries.join(", ")
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

/// Emit one action predicate. Each Object inst gets a leading
/// `ArrayContains` boundary clause + a flat SSA wildcard; body refs
/// resolve to the flat wildcard. Witness vars (e.g., values passed to
/// `obj.update(k, v)`) appear as plain private wildcards. Sub-action
/// calls are emitted with synthesized typed-private wildcards
/// `_<Sub>_in_<n>` / `_<Sub>_out_<n>` matching the sub's record schemas.
fn fmt_action(
    action: &ActionContext,
    loader: &Loader,
    w: &mut dyn fmt::Write,
) -> fmt::Result {
    let info = collect_action_info(action);
    let sub_calls = collect_sub_action_calls(action, loader);

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
    let alias_names: std::collections::HashSet<String> = sub_calls
        .iter()
        .filter_map(|c| c.alias.clone())
        .collect();

    // Private wildcards: every (var, ts) except sub-action aliases and
    // the chain's ts=0/max (which are public as chain0/chain).
    let mut private_vars: Vec<String> = Vec::new();
    for var in &action.vars {
        if alias_names.contains(var.as_str()) {
            // Sub-action alias: replaced by the synthesized sub_in/sub_out
            // wildcards added below.
            continue;
        }
        let max_ts = action.var_state[var].ts;
        for i in 0..=max_ts {
            // Skip the chain's public timestamps.
            if var == "chain" && (i == 0 || i == max_ts) {
                continue;
            }
            private_vars.push(fmt_var_at(var, i, max_ts));
        }
    }
    // Append synthesized sub-action typed privates after the regular
    // SSA/witness ones, matching the order users typically expect
    // (non-typed first, typed last).
    for c in &sub_calls {
        if let Some(name) = &c.sub_in_var {
            private_vars.push(format!(
                "{name} {}",
                schema_name(&c.sub_name, Side::In)
            ));
        }
        if let Some(name) = &c.sub_out_var {
            private_vars.push(format!(
                "{name} {}",
                schema_name(&c.sub_name, Side::Out)
            ));
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

    // SSA tracker for body emission. One entry per var.
    let mut vars: HashMap<&str, VarNameFmt> = action
        .vars
        .iter()
        .map(|v| {
            (
                v.as_str(),
                VarNameFmt {
                    name: v,
                    ts: 0,
                    max_ts: action.var_state[v].ts,
                },
            )
        })
        .collect();

    // ---- ArrayContains bridges (for every Object) ----
    //
    // Output:  ArrayContains(out, <Schema>Out::<entry>, <var_post_ssa>)
    // Input:   ArrayContains(in,  <Schema>In::<entry>,  <var_pre_ssa>)
    // Mutate:  emit both. Pre-form is ts=0; post-form is ts=max.
    for o in &info.objects {
        let max_ts = action.var_state[&o.varname].ts;
        let pre_ssa = fmt_var_at(&o.varname, 0, max_ts);
        let post_ssa = fmt_var_at(&o.varname, max_ts, max_ts);
        match o.io {
            ObjectIO::Output => {
                writeln!(
                    w,
                    "  ArrayContains(out, {}::{}, {})",
                    schema_name(&action.name, Side::Out),
                    o.varname,
                    post_ssa,
                )?;
            }
            ObjectIO::Input => {
                writeln!(
                    w,
                    "  ArrayContains(in, {}::{}, {})",
                    schema_name(&action.name, Side::In),
                    o.varname,
                    pre_ssa,
                )?;
            }
            ObjectIO::Mutate => {
                writeln!(
                    w,
                    "  ArrayContains(in, {}::{}, {})",
                    schema_name(&action.name, Side::In),
                    o.varname,
                    pre_ssa,
                )?;
                writeln!(
                    w,
                    "  ArrayContains(out, {}::{}, {})",
                    schema_name(&action.name, Side::Out),
                    o.varname,
                    post_ssa,
                )?;
            }
        }
    }

    // ---- Body (Insts other than Object) ----
    let mut objs: Vec<(ObjectIO, String, String)> = Vec::new();
    let mut sub_call_idx: usize = 0;
    for inst in &action.insts {
        match inst {
            Inst::Object { io, obj, class, .. } => {
                let varname = obj.borrow().var_name().to_string();
                objs.push((io.clone(), varname, class.clone()));
            }
            Inst::Set { obj, kvs } => {
                let obj_str = vars[obj.as_str()];
                for (key, value) in kvs {
                    let value = ArgFmt {
                        vars: &vars,
                        arg: value,
                    };
                    writeln!(w, r#"  DictContains({obj_str}, "{key}", {value})"#,)?;
                }
            }
            Inst::Update { obj, key, value } => {
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
                    write!(
                        w,
                        "{}",
                        ArgFmt {
                            vars: &vars,
                            arg,
                        }
                    )?;
                }
                writeln!(w, ")")?;
            }
            Inst::Intro { pred, args } => {
                write!(w, "  {pred}(")?;
                for (i, arg) in args.iter().enumerate() {
                    if i != 0 {
                        write!(w, ", ")?;
                    }
                    write!(
                        w,
                        "{}",
                        ArgFmt {
                            vars: &vars,
                            arg,
                        }
                    )?;
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
        let max_ts = action.var_state[varname.as_str()].ts;
        let guard_obj = match io {
            ObjectIO::Mutate => fmt_var_at(varname, 0, max_ts),
            _ => format!("{}", vars[varname.as_str()]),
        };
        writeln!(
            w,
            r#"  DictContains({guard_obj}, "type", @self_predicate(Is{class}))"#
        )?;
        let chain = vars["chain"];
        let chain_next = chain.next();
        let obj_str = vars[varname.as_str()];
        match io {
            ObjectIO::Input => writeln!(w, "  tx::TxDelete({chain_next}, {chain}, {obj_str})")?,
            ObjectIO::Output => writeln!(w, "  tx::TxInsert({chain_next}, {chain}, {obj_str})")?,
            ObjectIO::Mutate => {
                let pre = fmt_var_at(varname, 0, max_ts);
                writeln!(w, "  tx::TxMutate({chain_next}, {chain}, {obj_str}, {pre})")?;
            }
        }
        vars.get_mut("chain").expect("chain exists").inc();
    }
    writeln!(w, ")")?;
    Ok(())
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
        // Multi-detection: count Object insts per class. The bridge
        // OR enumerates one branch per (action, object-of-class), so
        // any class appearing more than once in an action needs its
        // bridges differentiated by varname suffix, regardless of
        // whether the duplicates are on the same side.
        let mut multi_keys: HashMap<String, usize> = HashMap::new();
        for o in &info.objects {
            *multi_keys.entry(o.class.clone()).or_insert(0) += 1;
        }
        for o in &info.objects {
            let side = dispatch_side(&o.io);
            let multi = multi_keys.get(&o.class).copied().unwrap_or(0) > 1;
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

        // Multi-detection: count Object insts per class. Same logic
        // as `fmt_bridges` so the names match.
        let mut multi_keys: HashMap<String, usize> = HashMap::new();
        for obj in &info.objects {
            *multi_keys.entry(obj.class.clone()).or_insert(0) += 1;
        }
        let multi = multi_keys.get(&o.class).copied().unwrap_or(0) > 1;
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
