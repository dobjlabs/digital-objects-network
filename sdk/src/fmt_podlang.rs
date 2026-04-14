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

struct VarNameFmt<'a> {
    name: &'a str,
    ts: usize,
    max_ts: usize,
}

impl<'a> VarNameFmt<'a> {
    fn inc(&mut self) {
        self.ts += 1;
    }
    fn next(&'a self) -> Self {
        Self {
            name: self.name,
            ts: self.ts + 1,
            max_ts: self.max_ts,
        }
    }
}

impl<'a> fmt::Display for VarNameFmt<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)?;
        if self.ts != self.max_ts {
            write!(f, "{}", self.ts)?;
        }
        Ok(())
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
    for var in &action.vars {
        let state = &action.var_state[var];
        for i in 0..state.ts + 1 {
            if i == state.ts {
                vars.push(var.clone());
            } else {
                vars.push(format!("{var}{i}"));
            }
        }
    }
    vars
}

fn fmt_action_pub_vars(action: &ActionContext) -> Vec<String> {
    let mut vars = Vec::new();
    for inst in &action.insts {
        if let Inst::Object {
            io: ObjectIO::Mutate | ObjectIO::Output,
            obj,
            class: _,
        } = inst
        {
            vars.push(obj.borrow().var_name().to_string());
        }
    }
    vars.extend_from_slice(&["tx".to_string(), "tx0".to_string()]);
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
                VarNameFmt {
                    name: v,
                    ts: 0,
                    max_ts: action.var_state[v].ts,
                },
            )
        })
        .collect();
    writeln!(w, ") = AND(")?;
    let mut objs = Vec::new();
    for inst in &action.insts {
        match inst {
            Inst::Object { io, obj, class } => {
                match io {
                    ObjectIO::Input => {
                        writeln!(w, "  Is{}({})", class, ArgFmt(&vars, obj))?;
                    }
                    ObjectIO::Output => {}
                    ObjectIO::Mutate => {
                        writeln!(w, "  Is{}({})", class, ArgFmt(&vars, obj))?;
                    }
                }
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
        }
    }
    for (io, obj) in &objs {
        let tx = &vars["tx"];
        let tx_next = tx.next();
        match io {
            ObjectIO::Input => writeln!(w, "  tx::TxDeleted({tx_next}, {tx}, {obj})")?,
            ObjectIO::Output => writeln!(w, "  tx::TxInserted({tx_next}, {tx}, {obj})")?,
            ObjectIO::Mutate => writeln!(w, "  tx::TxMutated({tx_next}, {tx}, {obj}, {obj}0)")?,
        }
        vars.get_mut("tx").expect("tx exists").inc();
    }
    writeln!(w, ")")?;
    Ok(())
}

fn fmt_class(loader: &Loader, w: &mut dyn fmt::Write, class: &ClassMeta) -> fmt::Result {
    let name = &class.name;
    write!(w, "Is{name}(state, private: tx, tx0")?;

    let other_len = class
        .actions
        .iter()
        .map(|(action_name, _)| loader.action_by_name(action_name).outputs.len())
        .max()
        .unwrap()
        - 1;
    if other_len != 0 {
        write!(w, ", ")?;
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
        for i in 0..action.outputs.len() {
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
        writeln!(w, ", tx, tx0)")?;
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
