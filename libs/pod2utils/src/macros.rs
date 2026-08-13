use std::{collections::HashMap, sync::Arc};

use pod2::{
    frontend::MultiPodBuilder,
    lang::Module,
    middleware::{
        CustomPredicateRef, OperationType, Statement, StrKey, Value,
        containers::{Dictionary, Set},
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
        $( map.insert(pod2::middleware::StrKey::from($key), pod2::middleware::Value::from($val)); )*
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
        $( kvs.push((pod2::middleware::StrKey::from($key), pod2::middleware::Value::from($val.clone()))); )*
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
        $( kvs.push((pod2::middleware::StrKey::from($key), pod2::middleware::Value::from($val.clone()))); )*
        $crate::macros::_dict_update($init, kvs)
    });
}

pub fn _dict_update<const N: usize>(
    mut init: Dictionary,
    kvs: Vec<(StrKey, Value)>,
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
    (Sum($a:expr, $b:expr, $sum:expr)) => {
        pod2::frontend::Operation::sum($a.clone(), $b.clone(), $sum.clone())
    };
    (Product($a:expr, $b:expr, $prod:expr)) => {
        pod2::frontend::Operation::product($a.clone(), $b.clone(), $prod.clone())
    };
    (Hash($a:expr, $b:expr, $hash:expr)) => {
        pod2::frontend::Operation::hash($a.clone(), $b.clone(), $hash.clone())
    };
    (DictContains($dict:expr, $key:expr, $value:expr)) => {
        pod2::frontend::Operation::dict_contains($dict.clone(), $key.clone(), $value.clone())
    };
    (DictUpdate($old_dict:expr, $key:expr, $value:expr, $dict:expr)) => {
        pod2::frontend::Operation::dict_update(
            $old_dict.clone(),
            $key.clone(),
            $value.clone(),
            $dict.clone(),
        )
    };
    (DictInsert($old_dict:expr, $key:expr, $value:expr, $dict:expr)) => {
        pod2::frontend::Operation::dict_insert(
            $old_dict.clone(),
            $key.clone(),
            $value.clone(),
            $dict.clone(),
        )
    };
    (DictDelete($old_dict:expr, $key:expr, $dict:expr)) => {
        pod2::frontend::Operation::dict_delete($old_dict.clone(), $key.clone(), $dict.clone())
    };
    (SetContains($set:expr, $value:expr)) => {
        pod2::frontend::Operation::set_contains($set.clone(), $value.clone())
    };
    (SetInsert($old_set:expr, $value:expr, $set:expr)) => {
        pod2::frontend::Operation::set_insert($old_set.clone(), $value.clone(), $set.clone())
    };
    (SetDelete($old_set:expr, $value:expr, $set:expr)) => {
        pod2::frontend::Operation::set_delete($old_set.clone(), $value.clone(), $set.clone())
    };
    (GtEq($a:expr, $b:expr)) => {
        pod2::frontend::Operation::gt_eq($a.clone(), $b.clone())
    };
    (ArrayContains($array:expr, $idx:expr, $value:expr)) => {
        pod2::frontend::Operation::array_contains($array.clone(), $idx.clone(), $value.clone())
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

/// Find the one module defining `name`.
///
/// Ambiguity is an error rather than a first-match win: two plugin
/// batches can each define an action or class of the same name, and
/// resolving to whichever was loaded first would prove a predicate from
/// the wrong batch. Callers that already know the batch should use the
/// `_in` variants and skip resolution entirely.
pub fn resolve_module<'a>(
    modules: &'a [Arc<Module>],
    name: &str,
) -> anyhow::Result<&'a Arc<Module>> {
    let mut found: Option<&Arc<Module>> = None;
    for module in modules {
        if module.predicate_ref_by_name(name).is_none() {
            continue;
        }
        if let Some(previous) = found {
            anyhow::bail!(
                "predicate {name} is defined in both module {} and module {}; \
                 qualify the call with the intended module",
                previous.batch.name,
                module.batch.name,
            );
        }
        found = Some(module);
    }
    found.ok_or_else(|| anyhow::anyhow!("predicate {name} is not defined in any loaded module"))
}

pub fn find_custom_pred_by_name(
    modules: &[Arc<Module>],
    name: &str,
) -> anyhow::Result<CustomPredicateRef> {
    let module = resolve_module(modules, name)?;
    module.predicate_ref_by_name(name).ok_or_else(|| {
        anyhow::anyhow!(
            "predicate {name} vanished from module {}",
            module.batch.name
        )
    })
}

pub fn apply_custom_pred(
    builder: &mut MultiPodBuilder,
    modules: &[Arc<Module>],
    public: bool,
    name: &str,
    wildcard_map: HashMap<String, Value>,
    statements: Vec<Statement>,
) -> anyhow::Result<Statement> {
    let module = resolve_module(modules, name)?.clone();
    apply_custom_pred_in(&module, builder, public, name, wildcard_map, statements)
}

/// Apply a predicate from a known module, bypassing name resolution.
fn apply_custom_pred_in(
    module: &Arc<Module>,
    builder: &mut MultiPodBuilder,
    public: bool,
    name: &str,
    wildcard_map: HashMap<String, Value>,
    statements: Vec<Statement>,
) -> anyhow::Result<Statement> {
    let cpr = module
        .predicate_ref_by_name(name)
        .ok_or_else(|| anyhow::anyhow!("module {} defines no {name}", module.batch.name))?;
    module.apply_predicate_with(name, statements, public, |is_public, op| {
        let mut wildcard_values: Vec<(usize, Value)> = Vec::new();
        for (i, name) in cpr.predicate().wildcard_names().iter().enumerate() {
            if let Some(value) = wildcard_map.get(name) {
                wildcard_values.push((i, value.clone()));
            }
        }
        Ok(builder.op(is_public, wildcard_values, op)?)
    })
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
        let module = resolve_module(&self.modules, name)?.clone();
        self.apply_custom_pred_in(&module, public, name, wildcard_map, statements)
    }

    /// Apply a predicate from a known module, bypassing name resolution.
    pub fn apply_custom_pred_in(
        &mut self,
        module: &Arc<Module>,
        public: bool,
        name: &str,
        wildcard_map: HashMap<String, Value>,
        statements: Vec<Statement>,
    ) -> anyhow::Result<Statement> {
        module.apply_predicate_with(name, statements, public, |is_public, op| {
            let mut wildcard_values: Vec<(usize, Value)> = Vec::new();
            // Get the CustomPredicateRef from the closure because this may be a chain in a
            // split predicate where the wildcard indices are different than the top level
            // predicate.
            let cpr = match &op.0 {
                OperationType::Custom(cpr) => cpr,
                _ => unreachable!(),
            };
            for (i, name) in cpr.predicate().wildcard_names().iter().enumerate() {
                if let Some(value) = wildcard_map.get(name) {
                    wildcard_values.push((i, value.clone()));
                }
            }
            Ok(self.builder.op(is_public, wildcard_values, op)?)
        })
    }

    /// Apply a custom predicate without wildcard value hints.
    /// Safe for both split and unsplit predicates -- the operations
    /// alone determine all wildcard values.
    pub fn apply_custom_pred_simple(
        &mut self,
        public: bool,
        name: &str,
        statements: Vec<Statement>,
    ) -> anyhow::Result<Statement> {
        let module = resolve_module(&self.modules, name)?.clone();
        self.apply_custom_pred_simple_in(&module, public, name, statements)
    }

    /// Apply a predicate from a known module, bypassing name resolution.
    pub fn apply_custom_pred_simple_in(
        &mut self,
        module: &Arc<Module>,
        public: bool,
        name: &str,
        statements: Vec<Statement>,
    ) -> anyhow::Result<Statement> {
        module.apply_predicate_with(
            name,
            statements,
            public,
            |is_public, op| -> anyhow::Result<Statement> {
                Ok(self.builder.op(is_public, vec![], op)?)
            },
        )
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

#[cfg(test)]
mod tests {
    use super::*;
    use pod2::{lang::load_module, middleware::Params};

    /// A one-predicate module, so two of them can be made to collide on
    /// a name the way two independently compiled plugins would.
    fn module_named(module: &str, predicate: &str) -> Arc<Module> {
        let params = Params::default();
        let source = format!("{predicate}(a, b) = AND(\n  Equal(a, b)\n)\n");
        Arc::new(load_module(&source, module, &params, &[]).expect("test module compiles"))
    }

    #[test]
    fn resolves_a_name_defined_once() {
        let modules = vec![
            module_named("plug_a", "Claim"),
            module_named("plug_b", "Ship"),
        ];
        let found = resolve_module(&modules, "Claim").expect("Claim resolves");
        assert_eq!(found.batch.name, "plug_a");
    }

    #[test]
    fn rejects_a_name_two_modules_define() {
        let modules = vec![
            module_named("plug_a", "Claim"),
            module_named("plug_b", "Claim"),
        ];
        let err = resolve_module(&modules, "Claim").expect_err("ambiguity must not resolve");
        let message = format!("{err}");
        assert!(message.contains("plug_a"), "{message}");
        assert!(message.contains("plug_b"), "{message}");
    }

    #[test]
    fn reports_a_name_no_module_defines() {
        let modules = vec![module_named("plug_a", "Claim")];
        let err = resolve_module(&modules, "Swap").expect_err("missing name must not resolve");
        assert!(format!("{err}").contains("not defined in any loaded module"));
    }
}
