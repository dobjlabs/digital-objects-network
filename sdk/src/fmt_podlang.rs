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
enum Side {
    In,
    Out,
}

impl Side {
    fn arg_name(self) -> &'static str {
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

/// AKE rewrite: a script-side var name maps to `<side>.<entry>` instead
/// of a flat wildcard. Populated for non-bridged Object insts (output
/// without `.update`, input without sub-field access). The `entry`
/// equals the var name; we keep both fields for clarity.
#[derive(Clone)]
struct ObjAke {
    side: Side,
    entry: String,
}

/// Render a Var arg, accounting for AKE rewrites.
struct ArgFmt<'a> {
    vars: &'a HashMap<&'a str, VarNameFmt<'a>>,
    obj_ake: &'a HashMap<String, ObjAke>,
    arg: &'a Ref,
}

impl<'a> fmt::Display for ArgFmt<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let arg = self.arg.borrow();
        match &*arg {
            VarOrValue::Var(Var {
                name, key: None, ..
            }) => {
                if let Some(ake) = self.obj_ake.get(name) {
                    write!(f, "{}.{}", ake.side.arg_name(), ake.entry)
                } else {
                    write!(f, "{}", self.vars[name.as_str()])
                }
            }
            VarOrValue::Var(Var {
                name,
                key: Some(key),
                ..
            }) => write!(f, "{}.{key}", self.vars[name.as_str()]),
            VarOrValue::Value(value) => write!(f, "{value}"),
        }
    }
}

/// Render an Object reference (the obj name, used for type guard +
/// Tx event lines) honoring AKE rewrites.
fn fmt_obj_ref(
    obj: &str,
    vars: &HashMap<&str, VarNameFmt>,
    obj_ake: &HashMap<String, ObjAke>,
) -> String {
    if let Some(ake) = obj_ake.get(obj) {
        format!("{}.{}", ake.side.arg_name(), ake.entry)
    } else {
        format!("{}", vars[obj])
    }
}

/// IsX dispatch side for an Object inst. Per the plan: outputs and
/// mutates dispatch on `out.X`; inputs dispatch on `in.X`. The input
/// side of a mutate is intentionally excluded (decision #2).
fn dispatch_side(io: &ObjectIO) -> Side {
    match io {
        ObjectIO::Input => Side::In,
        ObjectIO::Output | ObjectIO::Mutate => Side::Out,
    }
}

/// Schema name for a (action, side) pair, e.g. `LogToWoodIn`.
fn schema_name(action_name: &str, side: Side) -> String {
    format!("{action_name}{}", side.schema_suffix())
}

/// True if the action has any reference to `var.<field>` anywhere in
/// its insts -- used to decide whether an input needs a flat bridge
/// wildcard or can be referenced as `in.<entry>` directly.
fn has_dot_access(action: &ActionContext, varname: &str) -> bool {
    fn ref_has_dot(r: &Ref, varname: &str) -> bool {
        let arg = r.borrow();
        match &*arg {
            VarOrValue::Var(Var {
                name, key: Some(_), ..
            }) => name == varname,
            _ => false,
        }
    }
    for inst in &action.insts {
        match inst {
            Inst::Set { kvs, .. } => {
                if kvs.iter().any(|(_, v)| ref_has_dot(v, varname)) {
                    return true;
                }
            }
            Inst::Update { value, .. } => {
                if ref_has_dot(value, varname) {
                    return true;
                }
            }
            Inst::Statement { args, .. } | Inst::Intro { args, .. } => {
                if args.iter().any(|a| ref_has_dot(a, varname)) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

/// Per-action info needed to emit the records-form predicate. Owned
/// strings throughout to keep clear of `RefCell` borrow lifetimes.
struct ActionInfo {
    /// Object insts in declaration order, with each Object's: var name,
    /// io, class, and bridged flag (true => keep flat SSA wildcards;
    /// false => rewrite refs to `in.<entry>` / `out.<entry>` AKE).
    objects: Vec<ObjectInfo>,
    in_entries: Vec<String>,  // var names (script-side) on the in side
    out_entries: Vec<String>, // ditto, out side
}

struct ObjectInfo {
    varname: String,
    io: ObjectIO,
    class: String,
    bridged: bool,
}

fn collect_action_info(action: &ActionContext) -> ActionInfo {
    let mut objects = Vec::new();
    let mut in_entries: Vec<String> = Vec::new();
    let mut out_entries: Vec<String> = Vec::new();
    for inst in &action.insts {
        if let Inst::Object { io, obj, class, .. } = inst {
            let varname: String = obj.borrow().var_name().to_string();
            let bridged = match io {
                // Output: bridge iff there's at least one `.update` call
                // (var_state.ts > 0 means SSA chain has intermediate
                // forms). Without `.update`, the script-side var is the
                // single ts=0 form -- safe to alias to `out.<entry>`.
                ObjectIO::Output => action.var_state[&varname].ts > 0,
                // Input: bridge iff the body sub-field-accesses the
                // input via `<varname>.<field>`. Plain whole-object
                // references can use `in.<entry>` directly (1-level AKE
                // through the in record).
                ObjectIO::Input => has_dot_access(action, &varname),
                // Mutate: TODO Phase 2B. The output side is always
                // bridged (uniform rule); the input side conditional.
                // For Phase 2A, fall back to bridging both, which is
                // structurally fine but adds the input bridge clause
                // even when not strictly needed.
                ObjectIO::Mutate => true,
            };
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
                bridged,
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

/// Emit one action predicate. Body uses `in.<entry>`/`out.<entry>` AKE
/// for non-bridged Objects; bridged Objects get a leading `ArrayContains`
/// clause and the body keeps using flat SSA wildcards.
fn fmt_action(action: &ActionContext, w: &mut dyn fmt::Write) -> fmt::Result {
    let info = collect_action_info(action);

    // Build the AKE rewrite map: non-bridged Objects only.
    let mut obj_ake: HashMap<String, ObjAke> = HashMap::new();
    for o in &info.objects {
        if !o.bridged {
            let side = match o.io {
                ObjectIO::Input => Side::In,
                ObjectIO::Output => Side::Out,
                // Mutate is forced bridged in Phase 2A, so this arm is unreachable.
                ObjectIO::Mutate => continue,
            };
            obj_ake.insert(
                o.varname.clone(),
                ObjAke {
                    side,
                    entry: o.varname.clone(),
                },
            );
        }
    }

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

    // Private wildcards: every (var, ts) except those rewritten to AKE
    // and except the chain's ts=0/max (which are public as chain0/chain).
    let mut private_vars: Vec<String> = Vec::new();
    for var in &action.vars {
        if obj_ake.contains_key(var.as_str()) {
            // Non-bridged Object: never appears as a wildcard.
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

    // SSA tracker for body emission. Has an entry per var (including
    // non-bridged Objects, which are just never read through this map).
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

    // ---- ArrayContains bridges (for bridged Objects) ----
    for o in &info.objects {
        if !o.bridged {
            continue;
        }
        let side = dispatch_side(&o.io);
        let max_ts = action.var_state[&o.varname].ts;
        let bridge_var = fmt_var_at(&o.varname, max_ts, max_ts);
        // Output: bridge wildcard is the FINAL SSA form (ts=max). For mutate
        // (Phase 2A's forced-bridge fallback) we emit the same way; refining
        // to in0/out_final separation is Phase 2B.
        writeln!(
            w,
            "  ArrayContains({}, {}::{}, {})",
            side.arg_name(),
            schema_name(&action.name, side),
            o.varname,
            bridge_var,
        )?;
    }

    // ---- Body (Insts other than Object) ----
    let mut objs: Vec<(ObjectIO, String, String, bool)> = Vec::new();
    for inst in &action.insts {
        match inst {
            Inst::Object { io, obj, class, .. } => {
                let varname = obj.borrow().var_name().to_string();
                // Look up bridged flag for this object.
                let bridged = info
                    .objects
                    .iter()
                    .find(|o| o.varname == varname)
                    .map(|o| o.bridged)
                    .unwrap_or(false);
                objs.push((io.clone(), varname, class.clone(), bridged));
            }
            Inst::Set { obj, kvs } => {
                let obj_str = fmt_obj_ref(obj.as_str(), &vars, &obj_ake);
                for (key, value) in kvs {
                    let value = ArgFmt {
                        vars: &vars,
                        obj_ake: &obj_ake,
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
                    obj_ake: &obj_ake,
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
                            obj_ake: &obj_ake,
                            arg
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
                            obj_ake: &obj_ake,
                            arg
                        }
                    )?;
                }
                writeln!(w, ")")?;
            }
            Inst::SubAction { action, obj } => {
                let chain = vars["chain"];
                let chain_next = chain.next();
                writeln!(
                    w,
                    "  {action}({obj}, {chain}, {chain_next})",
                    obj = ArgFmt {
                        vars: &vars,
                        obj_ake: &obj_ake,
                        arg: obj
                    }
                )?;
                vars.get_mut("chain").expect("chain exists").inc();
            }
        }
    }

    // ---- Per-Object type guard + Tx event lines ----
    for (io, varname, class, bridged) in &objs {
        let _ = bridged;
        let max_ts = action.var_state[varname.as_str()].ts;
        let guard_obj = match io {
            ObjectIO::Mutate => fmt_var_at(varname, 0, max_ts),
            _ => fmt_obj_ref(varname.as_str(), &vars, &obj_ake),
        };
        writeln!(
            w,
            r#"  DictContains({guard_obj}, "type", @self_predicate(Is{class}))"#
        )?;
        let chain = vars["chain"];
        let chain_next = chain.next();
        let obj_str = fmt_obj_ref(varname.as_str(), &vars, &obj_ake);
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

fn bridge_predicate_name(class: &str, action: &str, entry: &str, multi: bool) -> String {
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
        // Multi-detection: count Object insts per (side, class).
        let mut multi_keys: HashMap<(&'static str, String), usize> = HashMap::new();
        for o in &info.objects {
            let side_key = dispatch_side(&o.io).arg_name();
            *multi_keys.entry((side_key, o.class.clone())).or_insert(0) += 1;
        }
        for o in &info.objects {
            let side = dispatch_side(&o.io);
            let multi = multi_keys
                .get(&(side.arg_name(), o.class.clone()))
                .copied()
                .unwrap_or(0)
                > 1;
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

        // Multi-detection per (side, class) within the action.
        let mut multi_keys: HashMap<(&'static str, String), usize> = HashMap::new();
        for obj in &info.objects {
            let side_key = dispatch_side(&obj.io).arg_name();
            *multi_keys.entry((side_key, obj.class.clone())).or_insert(0) += 1;
        }
        let side = dispatch_side(&o.io);
        let multi = multi_keys
            .get(&(side.arg_name(), o.class.clone()))
            .copied()
            .unwrap_or(0)
            > 1;
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
        fmt_action(&action.0.borrow(), w)?;
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
