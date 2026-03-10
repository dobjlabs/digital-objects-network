use std::{collections::HashMap, sync::Arc};

use pod2::{
    frontend::MultiPodBuilder,
    lang::Module,
    middleware::{
        containers::{Dictionary, Set},
        CustomPredicateRef, Key, Statement, Value,
    },
};

#[macro_export]
macro_rules! set {
    () => ({
        pod2::middleware::containers::Set::new(std::collections::HashSet::new())
    });
    ($($val:expr),* ,) => (
        $crate::set!($($val.clone()),*)
    );
    ($($val:expr),*) => ({
        let mut set = std::collections::HashSet::<pod2::middleware::Value>::new();
        $( set.insert(pod2::middleware::Value::from($val.clone())); )*
        pod2::middleware::containers::Set::new(set)
    });
}

#[macro_export]
macro_rules! dict {
    () => (
        pod2::middleware::containers::Dictionary::new(std::collections::HashMap::new())
    );
    ({ $($key:expr => $val:expr),* , }) => (
        $crate::dict!({ $($key => $val),* })
    );
    ({ $($key:expr => $val:expr),* }) => ({
        let mut map = std::collections::HashMap::new();
        $( map.insert(pod2::middleware::Key::from($key), pod2::middleware::Value::from($val)); )*
        pod2::middleware::containers::Dictionary::new( map)
    });
}

#[macro_export]
macro_rules! map {
    () => (
        std::collections::HashMap::new()
    );
    ({ $($key:expr => $val:expr),* , }) => (
        $crate::map!({ $($key => $val),* })
    );
    ({ $($key:expr => $val:expr),* }) => ({
        let mut map = std::collections::HashMap::new();
        $( map.insert(String::from($key), pod2::middleware::Value::from($val)); )*
        map
    });
}

#[macro_export]
macro_rules! dict_define {
    ({ $($key:expr => $val:expr),* , }) => (
        $crate::dict_define!({ $($key => $val),* })
    );
    ({ $($key:expr => $val:expr),* }) => ({
        let mut kvs = Vec::new();
        $( kvs.push((pod2::middleware::Key::from($key), pod2::middleware::Value::from($val.clone()))); )*
        $crate::macros::_dict_update(dict!(), kvs)
    });
}

#[macro_export]
macro_rules! dict_update {
    ($init:expr, { $($key:expr => $val:expr),* , }) => (
        $crate::dict_define!($init, { $($key => $val),* })
    );
    ($init:expr, { $($key:expr => $val:expr),* }) => ({
        let mut kvs = Vec::new();
        $( kvs.push((pod2::middleware::Key::from($key), pod2::middleware::Value::from($val.clone()))); )*
        $crate::macros::_dict_update($init, kvs)
    });
}

pub fn _dict_update<const N: usize>(
    mut init: Dictionary,
    kvs: Vec<(Key, Value)>,
) -> [Dictionary; N] {
    let mut dict_states = Vec::with_capacity(N);
    dict_states.push(init.clone());
    for (k, v) in kvs.into_iter() {
        init.insert(&k, &v).unwrap();
        dict_states.push(init.clone());
    }
    dict_states.try_into().unwrap()
}

#[macro_export]
macro_rules! set_insert {
    ($init:expr, $($val:expr),* , ) => (
        $crate::dict_define!($init, { $($key => $val),* })
    );
    ($init:expr, $($val:expr),*) => ({
        let mut values = Vec::new();
        $( values.push(pod2::middleware::Value::from($val.clone())); )*
        $crate::macros::_set_insert($init, values)
    });
}

pub fn _set_insert<const N: usize>(mut init: Set, values: Vec<Value>) -> [Set; N] {
    let mut set_states = Vec::with_capacity(N);
    set_states.push(init.clone());
    for v in values.into_iter() {
        init.insert(&v).unwrap();
        set_states.push(init.clone());
    }
    set_states.try_into().unwrap()
}

#[macro_export]
macro_rules! set_delete {
    ($init:expr, $($val:expr),* , ) => (
        $crate::dict_define!($init, { $($key => $val),* })
    );
    ($init:expr, $($val:expr),*) => ({
        let mut values = Vec::new();
        $( values.push(pod2::middleware::Value::from($val.clone())); )*
        $crate::macros::_set_delete($init, values)
    });
}

pub fn _set_delete<const N: usize>(mut init: Set, values: Vec<Value>) -> [Set; N] {
    let mut set_states = Vec::with_capacity(N);
    set_states.push(init.clone());
    for v in values.into_iter() {
        init.delete(&v).unwrap();
        set_states.push(init.clone());
    }
    set_states.try_into().unwrap()
}

/// Argument types: `&Into<StatementArg>`
#[macro_export]
macro_rules! op {
    (Equal($a:expr, $b:expr)) => {
        pod2::frontend::Operation::eq($a.clone(), $b.clone())
    };
    (NotEqual($a:expr, $b:expr)) => {
        pod2::frontend::Operation::ne($a.clone(), $b.clone())
    };
    (Gt($a:expr, $b:expr)) => {
        pod2::frontend::Operation::gt($a.clone(), $b.clone())
    };
    (SumOf($sum:expr, $a:expr, $b:expr)) => {
        pod2::frontend::Operation::sum_of($sum.clone(), $a.clone(), $b.clone())
    };
    (ProductOf($prod:expr, $a:expr, $b:expr)) => {
        pod2::frontend::Operation::product_of($prod.clone(), $a.clone(), $b.clone())
    };
    (HashOf($hash:expr, $a:expr, $b:expr)) => {
        pod2::frontend::Operation::hash_of($hash.clone(), $a.clone(), $b.clone())
    };
    (DictContains($dict:expr, $key:expr, $value:expr)) => {
        pod2::frontend::Operation::dict_contains($dict.clone(), $key.clone(), $value.clone())
    };
    (DictUpdate($dict:expr, $old_dict:expr, $key:expr, $value:expr)) => {
        pod2::frontend::Operation::dict_update(
            $dict.clone(),
            $old_dict.clone(),
            $key.clone(),
            $value.clone(),
        )
    };
    (DictInsert($dict:expr, $old_dict:expr, $key:expr, $value:expr)) => {
        pod2::frontend::Operation::dict_insert(
            $dict.clone(),
            $old_dict.clone(),
            $key.clone(),
            $value.clone(),
        )
    };
    (DictDelete($dict:expr, $old_dict:expr, $key:expr)) => {
        pod2::frontend::Operation::dict_delete($dict.clone(), $old_dict.clone(), $key.clone())
    };
    (SetContains($set:expr, $value:expr)) => {
        pod2::frontend::Operation::set_contains($set.clone(), $value.clone())
    };
    (SetInsert($set:expr, $old_set:expr, $value:expr)) => {
        pod2::frontend::Operation::set_insert($set.clone(), $old_set.clone(), $value.clone())
    };
    (SetDelete($set:expr, $old_set:expr, $value:expr)) => {
        pod2::frontend::Operation::set_delete($set.clone(), $old_set.clone(), $value.clone())
    };
}

/// Argument types:
/// $builder: &mut MultiPodBuilder
/// $input_sts: &mut Vec<Statement>
/// $pred: NativePredicate token
/// $arg: &Into<StatementArg>
/// $st: Statement
#[macro_export]
macro_rules! _st_custom_args {
    (process_st, $builder:expr, $input_sts:expr, $st:expr) => {{
        $input_sts.push($st);
    }};
    (process_op, $builder:expr, $input_sts:expr, $pred:ident($($arg:expr),+)) => {{
        $input_sts.push($builder.priv_op($crate::op!($pred($($arg),+)))?);
    }};

    // Munch native operation
    ($builder:expr, $input_sts:expr, $pred:ident($($arg:expr),+)) => {{
        $crate::_st_custom_args!(process_op, $builder, $input_sts, $pred($($arg),+));
    }};
    ($builder:expr, $input_sts:expr, $pred:ident($($arg:expr),+), $($tail:tt)*) => {{
        $crate::_st_custom_args!(process_op, $builder, $input_sts, $pred($($arg),+));
        $crate::_st_custom_args!($builder, $input_sts, $($tail)*)
    }};
    // Munch statement
    ($builder:expr, $input_sts:expr, $st:expr) => {{
        $crate::_st_custom_args!(process_st, $builder, $input_sts, $st);
    }};
    ($builder:expr, $input_sts:expr, $st:expr, $($tail:tt)*) => {{
        $crate::_st_custom_args!(process_st, $builder, $input_sts, $st);
        $crate::_st_custom_args!($builder, $input_sts, $($tail)*)
    }};
}

/// Argument types:
/// $values: HashMap<(String, Value)>
/// $name: Public wildcard name token
/// $value: Value
#[macro_export]
macro_rules! _wildcard_values {
    (process, $values:expr, $name:ident, $value:expr) => {{
        let name = stringify!($name);
        $values.insert(name.to_string(), Value::from($value.clone()));
    }};

    ($values:expr, []) => {{
    }};
    // Munch value
    ($values:expr, [$name:ident=$value:expr]) => {{
        $crate::_wildcard_values!(process, $values, $name, $value);
    }};
    ($values:expr, [$name:ident=$value:expr, $($tail:expr),*]) => {{
        $crate::_wildcard_values!(process, $values, $name, $value);
        $crate::_wildcard_values!($values, [$($tail),*]);
    }};
}

pub fn find_custom_pred_by_name(modules: &[Arc<Module>], name: &str) -> Option<CustomPredicateRef> {
    for module in modules {
        if let Some(cpr) = module.predicate_ref_by_name(name) {
            return Some(cpr);
        }
    }
    None
}

pub fn apply_custom_pred(
    builder: &mut MultiPodBuilder,
    modules: &[Arc<Module>],
    public: bool,
    name: &str,
    wildcard_map: HashMap<String, Value>,
    statements: Vec<Statement>,
) -> anyhow::Result<Statement> {
    for module in modules {
        if let Some(cpr) = module.predicate_ref_by_name(name) {
            return module.apply_predicate_with(name, statements, public, |is_public, op| {
                let mut wildcard_values: Vec<(usize, Value)> = Vec::new();
                for (i, name) in cpr.predicate().wildcard_names().iter().enumerate() {
                    if let Some(value) = wildcard_map.get(name) {
                        wildcard_values.push((i, value.clone()));
                    }
                }
                let st = builder.op(is_public, wildcard_values, op).unwrap();
                Ok(st)
            });
        }
    }
    panic!("predicate not found");
}

/// Argument types:
/// Same as `st_custom!` with destructured `ctx`
#[macro_export]
macro_rules! _st_custom {
    ($builder:expr, $modules:expr, $pub:expr, $pred:ident($($wc_name:ident=$wc_value:expr),*) = ($($sts:tt)*)) => {{
        // Macro wrapped in a closure so that it can return early on `Result::Error` via `?`
        (|| -> anyhow::Result<pod2::middleware::Statement> {
            let custom_pred = $crate::macros::find_custom_pred_by_name($modules, stringify!($pred))
                .expect("predicate exists");
            let mut input_sts = Vec::new();
            $crate::_st_custom_args!($builder, &mut input_sts, $($sts)*);
            let mut wildcard_values: std::collections::HashMap<String, pod2::middleware::Value> =
                std::collections::HashMap::new();
            $crate::_wildcard_values!(wildcard_values, [$($wc_name=$wc_value),*]);
            $crate::macros::apply_custom_pred($builder, $modules, $pub, stringify!($pred), wildcard_values, input_sts)
            // let op = pod2::frontend::Operation::custom(custom_pred, input_sts);
            // $builder.op($pub, wildcard_values, op)
        })()
    }};
}

pub struct BuildContext {
    pub builder: MultiPodBuilder,
    pub modules: Vec<Arc<Module>>,
}

impl BuildContext {
    pub fn new(builder: MultiPodBuilder, modules: Vec<Arc<Module>>) -> Self {
        Self { builder, modules }
    }
}

impl BuildContext {
    pub fn apply_custom_pred(
        &mut self,
        public: bool,
        name: &str,
        wildcard_map: HashMap<String, Value>,
        statements: Vec<Statement>,
    ) -> anyhow::Result<Statement> {
        for module in &self.modules {
            if let Some(cpr) = module.predicate_ref_by_name(name) {
                return module.apply_predicate_with(name, statements, public, |is_public, op| {
                    let mut wildcard_values: Vec<(usize, Value)> = Vec::new();
                    for (i, name) in cpr.predicate().wildcard_names().iter().enumerate() {
                        if let Some(value) = wildcard_map.get(name) {
                            wildcard_values.push((i, value.clone()));
                        }
                    }
                    let st = self.builder.op(is_public, wildcard_values, op).unwrap();
                    Ok(st)
                });
            }
        }
        panic!("predicate not found");
    }
}

/// Argument types:
/// Same as `st_custom!`
#[macro_export]
#[rustfmt::skip]
macro_rules! pub_st_custom {
    ($ctx:expr, $pred:ident($($wc_name:ident=$wc_value:expr),*) = ($($sts:tt)*)) => {{
        $crate::_st_custom!(&mut $ctx.builder, &$ctx.modules, true, $pred($($wc_name=$wc_value),*) = ($($sts)*))
    }};
}

/// Argument types:
/// $ctx: &mut BuildContext
/// $pred: NativePredicate token
/// $wc_name: Public wildcard name token
/// $wc_value: &Into<Value>
/// $sts: Operation|Statement
#[macro_export]
#[rustfmt::skip]
macro_rules! st_custom {
    ($ctx:expr, $pred:ident($($wc_name:ident=$wc_value:expr),*) = ($($sts:tt)*)) => {{
        $crate::_st_custom!(&mut $ctx.builder, &$ctx.modules, false, $pred($($wc_name=$wc_value),*) = ($($sts)*))
    }};
}
