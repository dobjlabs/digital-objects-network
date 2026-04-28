//! Functions used to format to podlang source code

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

/// Public-arg labels for the chain var endpoints. Hard-coupled to the
/// txlib's `TxFinalized` / `ReplayActions` predicate signatures.
const CHAIN_START: &str = "chain_start";
const CHAIN_END: &str = "chain_end";

/// Render the txlib chain var at a given timestamp. The naming scheme
/// is a protocol concern: the txlib's `TxFinalized` and `ReplayActions`
/// predicates expect those literal labels for the chain endpoints.
fn fmt_chain_at(ts: usize, max_ts: usize) -> String {
    match ts {
        0 => CHAIN_START.to_string(),
        t if t == max_ts => CHAIN_END.to_string(),
        t => format!("chain_{t}"),
    }
}

/// Render a user-defined var at a given timestamp. Final ts renders as
/// the bare name; intermediate timestamps suffix the index. SSA-style
/// disambiguation for vars rewritten by mutations.
fn fmt_var_at(name: &str, ts: usize, max_ts: usize) -> String {
    if ts == max_ts {
        name.to_string()
    } else {
        format!("{name}{ts}")
    }
}

/// Two variants encode the protocol/implementation split between
/// `fmt_chain_at` and `fmt_var_at` so dispatch is at the type level
/// instead of by string comparison.
enum VarNameFmt<'a> {
    Chain {
        ts: usize,
        max_ts: usize,
    },
    Var {
        name: &'a str,
        ts: usize,
        max_ts: usize,
    },
}

impl<'a> VarNameFmt<'a> {
    fn inc(&mut self) {
        match self {
            Self::Chain { ts, .. } | Self::Var { ts, .. } => *ts += 1,
        }
    }
    fn next(&self) -> Self {
        match *self {
            Self::Chain { ts, max_ts } => Self::Chain { ts: ts + 1, max_ts },
            Self::Var { name, ts, max_ts } => Self::Var {
                name,
                ts: ts + 1,
                max_ts,
            },
        }
    }
}

impl<'a> fmt::Display for VarNameFmt<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Chain { ts, max_ts } => write!(f, "{}", fmt_chain_at(*ts, *max_ts)),
            Self::Var { name, ts, max_ts } => write!(f, "{}", fmt_var_at(name, *ts, *max_ts)),
        }
    }
}

struct ArgFmt<'a>(&'a HashMap<&'a str, VarNameFmt<'a>>, &'a Ref);

impl<'a> fmt::Display for ArgFmt<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let arg = self.1.borrow();
        match &*arg {
            VarOrValue::Var(Var {
                name, key: None, ..
            }) => write!(f, "{}", self.0[name.as_str()]),
            VarOrValue::Var(Var {
                name,
                key: Some(key),
                ..
            }) => write!(f, "{}.{key}", self.0[name.as_str()]),
            VarOrValue::Value(value) => write!(f, "{value}"),
        }
    }
}

fn fmt_action_vars(action: &ActionContext) -> Vec<String> {
    let mut vars = Vec::new();
    for i in 0..=action.chain_ts {
        vars.push(fmt_chain_at(i, action.chain_ts));
    }
    for var in &action.vars {
        let state = &action.var_state[var];
        for i in 0..=state.ts {
            vars.push(fmt_var_at(var, i, state.ts));
        }
    }
    vars
}

fn fmt_action_pub_vars(action: &ActionContext) -> Vec<String> {
    let mut vars = Vec::new();
    for inst in &action.insts {
        // Every direct Object inst is public: Inputs (deletes),
        // Outputs (inserts), and Mutates all become arguments so the
        // class's IsX OR can dispatch on any of them at replay time.
        // Sub-action references stay private — a sub-action's I/O
        // appears in the sub-action's own predicate signature, and
        // its output is a private witness within the parent.
        if let Inst::Object { obj, .. } = inst {
            vars.push(obj.borrow().var_name().to_string());
        }
    }
    vars.extend_from_slice(&[CHAIN_START.to_string(), CHAIN_END.to_string()]);
    vars
}

fn fmt_action(action: &ActionContext, w: &mut dyn fmt::Write) -> fmt::Result {
    write!(w, "{}(", action.name)?;
    let pub_var_names = fmt_action_pub_vars(action);
    for var in &pub_var_names {
        write!(w, "{var}, ")?;
    }
    write!(w, "private: ")?;
    let var_names = fmt_action_vars(action);
    for (i, var) in var_names
        .iter()
        .filter(|v| !pub_var_names.contains(v))
        .enumerate()
    {
        if i != 0 {
            write!(w, ", ")?;
        }
        write!(w, "{var}")?;
    }
    let mut vars: HashMap<&str, VarNameFmt> = action
        .vars
        .iter()
        .map(|v| {
            (
                v.as_str(),
                VarNameFmt::Var {
                    name: v,
                    ts: 0,
                    max_ts: action.var_state[v].ts,
                },
            )
        })
        .collect();
    let mut chain = VarNameFmt::Chain {
        ts: 0,
        max_ts: action.chain_ts,
    };
    writeln!(w, ") = AND(")?;
    let mut objs = Vec::new();
    for inst in &action.insts {
        match inst {
            Inst::Object { io, obj, .. } => {
                // Input/Mutate type-guards are enforced at replay time by
                // the new txlib (ReplayDelete/ReplayMutate call the obj's
                // type predicate). Nothing to emit inline.
                let obj = obj.borrow();
                objs.push((io, obj.var_name().to_string()));
            }
            Inst::Set { obj, kvs } => {
                let obj = &vars[obj.as_str()];
                for (key, value) in kvs {
                    let value = ArgFmt(&vars, value);
                    writeln!(w, r#"  DictContains({obj}, "{key}", {value})"#,)?;
                }
            }
            Inst::Update { obj, key, value } => {
                let obj_name = obj.as_str();
                let obj = &vars[obj_name];
                let obj_next = obj.next();
                let value = ArgFmt(&vars, value);
                writeln!(w, r#"  DictUpdate({obj_next}, {obj}, "{key}", {value})"#,)?;
                vars.get_mut(obj_name).expect("obj exists").inc();
            }
            Inst::Statement { pred, args } => {
                write!(w, "  {pred}(")?;
                for (i, arg) in args.iter().enumerate() {
                    if i != 0 {
                        write!(w, ", ")?;
                    }
                    write!(w, "{}", ArgFmt(&vars, arg))?;
                }
                writeln!(w, ")")?;
            }
            Inst::Intro { pred, args } => {
                write!(w, "  {pred}(")?;
                for (i, arg) in args.iter().enumerate() {
                    if i != 0 {
                        write!(w, ", ")?;
                    }
                    write!(w, "{}", ArgFmt(&vars, arg))?;
                }
                writeln!(w, ")")?;
            }
            Inst::SubAction { action, obj } => {
                // The sub-action's ReplayAction encapsulates all its
                // events as one chain step from the parent's view.
                let chain_next = chain.next();
                writeln!(
                    w,
                    "  {action}({obj}, {chain}, {chain_next})",
                    obj = ArgFmt(&vars, obj)
                )?;
                chain.inc();
            }
        }
    }
    for (io, obj) in &objs {
        let chain_next = chain.next();
        match io {
            ObjectIO::Input => writeln!(w, "  tx::TxDeleted({chain_next}, {chain}, {obj})")?,
            ObjectIO::Output => writeln!(w, "  tx::TxInserted({chain_next}, {chain}, {obj})")?,
            ObjectIO::Mutate => {
                writeln!(w, "  tx::TxMutated({chain_next}, {chain}, {obj}, {obj}0)")?
            }
        }
        chain.inc();
    }
    writeln!(w, ")")?;
    Ok(())
}

fn fmt_class(loader: &Loader, w: &mut dyn fmt::Write, class: &ClassMeta) -> fmt::Result {
    let name = &class.name;
    write!(w, "Is{name}(state, chain_start, chain_end")?;

    let other_len = class
        .actions
        .iter()
        .map(|(action_name, _)| loader.action_by_name(action_name).object_refs.len())
        .max()
        .unwrap()
        - 1;
    if other_len != 0 {
        write!(w, ", private: ")?;
    }
    for i in 0..other_len {
        if i != 0 {
            write!(w, ", ")?;
        }
        write!(w, "_other_{i}")?;
    }
    writeln!(w, ") = OR(")?;
    for (action_name, index) in &class.actions {
        write!(w, "  {action_name}(")?;
        let action = loader.action_by_name(action_name);
        let mut count = 0;
        for i in 0..action.object_refs.len() {
            if i != 0 {
                write!(w, ", ")?;
            }
            if i == *index {
                write!(w, "state")?;
            } else {
                write!(w, "_other_{count}")?;
                count += 1;
            }
        }
        writeln!(w, ", chain_start, chain_end)")?;
    }
    writeln!(w, ")")?;
    Ok(())
}

pub(crate) fn fmt(loader: &Loader, w: &mut dyn fmt::Write) -> fmt::Result {
    for dep in &loader.dependencies {
        fmt_dependency(dep, w).unwrap();
    }
    writeln!(w, "\n// Actions\n")?;
    for action in &loader.actions {
        fmt_action(&action.0.borrow(), w)?;
        writeln!(w)?;
    }
    writeln!(w, "// Classes\n")?;
    for class in &loader.classes {
        fmt_class(loader, w, class)?;
        writeln!(w)?;
    }
    Ok(())
}
