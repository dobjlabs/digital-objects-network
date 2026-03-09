use std::{any::Any, collections::HashMap, fmt, mem, slice, sync::Arc};

use anyhow::Result;
use fmt::Write;
use log::info;
use pod2::{
    backends::plonky2::{basetypes::DEFAULT_VD_SET, mainpod::Prover, mock::mainpod::MockProver},
    frontend::{MainPod, MultiPodBuilder, Operation},
    lang::{Module, load_module},
    middleware::{
        EMPTY_VALUE, Key, MainPodProver, Params, Statement, TypedValue, VDSet, Value,
        containers::Dictionary,
    },
};
use pod2utils::{dict, macros::BuildContext};
use serde::Serialize;
use tinytemplate::TinyTemplate;
use txlib::{StateRoot, Tx, TxBuilder, rand_raw_value};

pub mod api {
    use std::fmt;

    use pod2::middleware::{Hash, Statement, Value};

    /// Argument to an Update/Set detail
    pub enum Arg {
        /// A literal Value embedded in the statement template
        Literal(Value),
        /// Pick up the value from a variable in the predicate context (a wildcard)
        Var(&'static str),
    }

    impl fmt::Display for Arg {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Arg::Literal(v) => v.fmt(f),
                Arg::Var(v) => v.fmt(f),
            }
        }
    }

    /// Details about the object
    pub enum Detail {
        /// Introduce a variable in the context
        Var(
            // name
            &'static str,
            // function
            Box<dyn Fn(&mut super::Context) -> Value>,
        ),
        /// Define a condition that the object must fulfill via a predicate and the function that
        /// generates such predicate.
        Condition(
            // predicate
            &'static str,
            // function
            Box<dyn Fn(&mut super::Context) -> Statement>,
        ),
        /// Update a key of the object
        Update(
            // key
            &'static str,
            // value
            Arg,
        ),
        /// Set a key of the object (doesn't modify the object)
        Set(
            // key
            &'static str,
            // value
            Arg,
        ),
    }

    /// Each step involves a different object
    pub enum Step {
        /// The action consumes an object as input
        Input {
            name: &'static str,
            class: &'static str,
            details: Vec<Detail>,
        },
        /// The action produces an object as output
        Output {
            name: &'static str,
            class: &'static str,
            details: Vec<Detail>,
        },
        /// The action mutates an object, which is considered both input and output
        Mutate {
            name: &'static str,
            class: &'static str,
            details: Vec<Detail>,
        },
        /// The action depends on another action that affects an object
        Depend {
            name: &'static str,
            action: &'static str,
        },
    }

    /// An action consumes objects and produces objects via a list of steps
    pub struct Action {
        pub name: &'static str,
        pub steps: Vec<Step>,
    }

    /// Dependency towards another pod module or introduction pod
    pub enum Dependency {
        Module { name: &'static str, hash: Hash },
        Intro { pred: &'static str, hash: Hash },
    }
}

struct Class {
    name: String,
    // Actions that define the class with the index within the Action arguments that correspond to
    // the class.
    actions: Vec<(String, usize)>,
}

pub struct Helper {
    params: Params,
    data: Data,
    pub podlang_src: String,
    modules: Vec<Arc<Module>>,
    pub module: Arc<Module>,
}

impl Helper {
    pub fn new(mut dependencies: Vec<api::Dependency>, api_actions: Vec<api::Action>) -> Self {
        let params = Params::default();
        let txlib_mod = Arc::new(txlib::predicates::module());
        dependencies.push(api::Dependency::Module {
            name: "tx",
            hash: txlib_mod.id(),
        });

        let mut actions = Vec::new();
        for api_action in api_actions {
            actions.push(Data::new_action(api_action));
        }
        let classes = Data::classes(&actions);
        let output_index_class_st_index = Data::output_index_class_st_index(&actions);
        let data = Data {
            dependencies,
            actions,
            classes,
            output_index_class_st_index,
        };

        let mut podlang_src = String::new();
        data.format_podlang(&mut podlang_src).unwrap();
        let module = Arc::new(
            load_module(
                podlang_src.as_str(),
                "root",
                &params,
                slice::from_ref(&txlib_mod),
            )
            .expect("compiles"),
        );
        let modules = vec![txlib_mod, module.clone()];
        Self {
            params,
            data,
            podlang_src,
            modules,
            module,
        }
    }
    pub fn builder(&self, mock: bool, state_root: Arc<StateRoot>) -> ObjectBuilder<'_> {
        let mock_prover = MockProver {};
        let real_prover = Prover {};
        let (vd_set, prover): (_, Box<dyn MainPodProver>) = if mock {
            (VDSet::new(&[]), Box::new(mock_prover))
        } else {
            let vd_set = &*DEFAULT_VD_SET;
            (vd_set.clone(), Box::new(real_prover))
        };

        ObjectBuilder {
            mock,
            params: self.params.clone(),
            vd_set,
            state_root,
            prover,
            modules: self.modules.clone(),
            data: &self.data,
        }
    }
}

fn prove(builder: MultiPodBuilder, prover: &dyn MainPodProver) -> MainPod {
    let solution = builder.solve().unwrap();
    log::debug!("solution needs {} pods", solution.solution().pod_count);
    solution.prove(prover).unwrap().pods.pop().unwrap()
}

#[derive(Debug)]
pub struct SpendableObject {
    pub pod: MainPod,
    pub obj: Dictionary,
    pub tx: Tx,
}

impl SpendableObject {
    pub fn tx_input(&self) -> (Dictionary, Tx) {
        (self.obj.clone(), self.tx.clone())
    }
}

pub struct SpendableObjects {
    pub tx_pod: MainPod,
    pub obj_pods: Vec<MainPod>,
    pub objs: Vec<Dictionary>,
    pub tx: Tx,
}

impl SpendableObjects {
    pub fn obj(&self, index: usize) -> SpendableObject {
        SpendableObject {
            pod: self.obj_pods[index].clone(),
            obj: self.objs[index].clone(),
            tx: self.tx.clone(),
        }
    }
    pub fn objs<const N: usize>(&self) -> [SpendableObject; N] {
        let objs: Vec<_> = (0..N).map(|i| self.obj(i)).collect();
        objs.try_into().unwrap()
    }
}

pub struct ObjectBuilder<'a> {
    mock: bool,
    params: Params,
    vd_set: VDSet,
    state_root: Arc<StateRoot>,
    prover: Box<dyn MainPodProver>,
    modules: Vec<Arc<Module>>,
    data: &'a Data,
}

pub struct Context<'a> {
    pub vars: Vars<'a>,
    storage: HashMap<&'static str, Box<dyn Any>>,
    pub mock: bool,
    pub params: Params,
    pub vd_set: VDSet,
    pub bld: BuildContext,
}

impl<'a> Context<'a> {
    pub fn store(&mut self, key: &'static str, value: Box<dyn Any>) {
        self.storage.insert(key, value);
    }
    pub fn load<T: 'static>(&self, key: &'static str) -> &T {
        let value = self.storage.get(key).unwrap();
        value.downcast_ref::<T>().unwrap()
    }
    pub fn take<T: 'static>(&mut self, key: &'static str) -> Box<T> {
        let value = self.storage.remove(key).unwrap();
        value.downcast::<T>().unwrap()
    }
}

struct OutputData<'a> {
    class: &'a str,
    obj: Dictionary,
}

impl<'a> ObjectBuilder<'a> {
    fn new_builder(&self) -> MultiPodBuilder {
        MultiPodBuilder::new(&self.params, &self.vd_set)
    }

    fn new_tx_builder(&self, ctx: &mut BuildContext, inputs: &[(Dictionary, Tx)]) -> TxBuilder {
        TxBuilder::new(ctx, inputs, self.state_root.clone())
    }

    // Returns Action statement, Output objects and data required to make the object class
    // statement for each output from that Action
    #[allow(clippy::type_complexity)]
    fn _action(
        &'a self,
        ctx: &mut Context<'a>,
        tx_builder: &mut TxBuilder,
        action: &'a Action,
        inputs: &[SpendableObject],
    ) -> (Statement, Vec<Dictionary>, Vec<(String, Vec<Statement>)>) {
        let mut io_info = String::new();
        for (class, name) in action.inputs() {
            write!(io_info, "Is{class}({name}) ").unwrap();
        }
        write!(io_info, "-> ").unwrap();
        for (class, name) in action.outputs() {
            write!(io_info, "Is{class}({name}) ").unwrap();
        }
        info!("action {}: {io_info}", action.name);

        // Statements used to build the Action custom statement
        let mut sts = Vec::new();
        // Copies of objects before Updates for Mutate cases
        let mut objs0: HashMap<&str, Dictionary> = HashMap::new();
        // Object OutputData's to be returned by this Action
        let mut outputs = Vec::new();
        // Object to be returned by this Action (including dependent actions)
        let mut output_objs = Vec::new();
        // Data necessary to make each output object' class statement
        let mut output_objs_st_class_data = Vec::new();
        // Counter of the input object we're processing
        let mut input_index = 0;
        // Offset for dependency inputs
        let mut input_offset = 0;
        for step in action.steps() {
            match step.kind {
                StepKind::Output => {
                    ctx.vars.insert(
                        step.name.as_str(),
                        dict!({"work" => EMPTY_VALUE, "key" => Value::from(rand_raw_value())}),
                    );
                }
                StepKind::Input | StepKind::Mutate => {
                    let input = &inputs[input_index];
                    let input_pod_sts = input.pod.pod.pub_statements();
                    let st_class = input_pod_sts[0].clone();
                    sts.push(st_class);
                    input_index += 1;
                }
                StepKind::Depends => {
                    let (_name, action) = (step.name.as_str(), step.class.as_str());
                    let action = self.data.action_by_name(action);
                    let current_vars = mem::take(&mut ctx.vars);

                    let mut input_count = 0;
                    for (input_index, (_class, name)) in action.inputs().enumerate() {
                        let input = &inputs[input_offset + input_index];
                        ctx.vars.insert(name, input.obj.clone());
                        input_count += 1;
                    }
                    input_offset += input_count;

                    let (st_action, objs, objs_st_class_data) =
                        self._action(ctx, tx_builder, action, inputs);
                    ctx.vars = current_vars;
                    output_objs.extend(objs.into_iter());
                    output_objs_st_class_data.extend(objs_st_class_data.into_iter());
                    sts.push(st_action);
                }
            }
            // On Mutate we save a copy of the initial object because it's required by the mutate
            // transaction
            if matches!(step.kind, StepKind::Mutate) {
                let obj = ctx.vars.get_dict(&step.name).unwrap();
                objs0.insert(&step.name, obj.clone());
            }

            let details_set_len = details_set_len(&step.details);
            let mut details_set_kv = Vec::new();
            let mut details_set_done = details_set_len == 0;
            for detail in &step.details {
                match detail {
                    api::Detail::Set(key, value) => {
                        let value = ctx.vars.value(value);
                        let obj = ctx.vars.get_dict_mut(&step.name).unwrap();
                        obj.insert(&Key::from(*key), &value).unwrap();
                        details_set_kv.push((*key, value));
                    }
                    api::Detail::Update(key, value) => {
                        if !details_set_done {
                            panic!("Update before last Set in the Step");
                        }
                        let value = ctx.vars.value(value);
                        let obj = ctx.vars.get_dict_mut(&step.name).unwrap();
                        let obj0 = obj.clone();
                        obj.update(&Key::from(*key), &value).unwrap();
                        sts.push(
                            ctx.bld
                                .builder
                                .priv_op(Operation::dict_update(obj.clone(), obj0, *key, value))
                                .unwrap(),
                        );
                    }
                    api::Detail::Var(name, f) => {
                        let value = f(ctx);
                        ctx.vars.insert(name, value.take_typed());
                    }
                    api::Detail::Condition(_pred, f) => {
                        if !details_set_done {
                            panic!("Condition before last Set in the Step");
                        }
                        let st = f(ctx);
                        sts.push(st);
                    }
                }
                // add Detail::Set statements just after they have been processed so that they all
                // refer to the same object state.
                if !details_set_done && details_set_kv.len() == details_set_len {
                    for (key, value) in mem::take(&mut details_set_kv).into_iter() {
                        let obj = ctx.vars.get_dict_mut(&step.name).unwrap();
                        sts.push(
                            ctx.bld
                                .builder
                                .priv_op(Operation::dict_contains(obj.clone(), key, value))
                                .unwrap(),
                        );
                    }
                    details_set_done = true;
                }
            }

            match step.kind {
                StepKind::Output => {
                    let obj = ctx.vars.get_dict(step.name.as_str()).unwrap().clone();
                    outputs.push(OutputData {
                        class: &step.class,
                        obj: obj.clone(),
                    });
                    sts.push(tx_builder.insert(&mut ctx.bld, obj));
                }
                StepKind::Input => {
                    let obj = ctx.vars.get_dict(step.name.as_str()).unwrap().clone();
                    sts.push(tx_builder.delete(&mut ctx.bld, obj));
                }
                StepKind::Mutate => {
                    let obj0 = objs0.get(step.name.as_str()).unwrap();
                    let obj = ctx.vars.get_dict(step.name.as_str()).unwrap().clone();
                    outputs.push(OutputData {
                        class: &step.class,
                        obj: obj.clone(),
                    });
                    sts.push(tx_builder.mutate(&mut ctx.bld, obj, obj0.clone()));
                }
                StepKind::Depends => {}
            }
        }

        // Action statement
        let st_action = ctx
            .bld
            .apply_custom_pred(false, &action.name, HashMap::new(), sts)
            .unwrap();
        ctx.bld.builder.reveal(&st_action).unwrap();

        // Output (includes Output & Mutate) Class(obj) statements
        for (index, output) in outputs.iter().enumerate() {
            let class = self.data.class_by_name(output.class);
            let mut sts = vec![Statement::None; class.actions.len()];
            let class_st_index =
                self.data.output_index_class_st_index[&(action.name.clone(), index)];
            sts[class_st_index] = st_action.clone();
            let pred = format!("Is{}", class.name);
            // We delay the creation of the class statement until we have created all actions
            // because the class statements go to different pods.
            output_objs_st_class_data.push((pred, sts));
        }

        output_objs.extend(outputs.into_iter().map(|out| out.obj));
        (st_action, output_objs, output_objs_st_class_data)
    }

    pub fn action(self, action: &str, inputs: Vec<SpendableObject>) -> SpendableObjects {
        let action = self.data.action_by_name(action);

        let builder = self.new_builder();
        let mut ctx = Context {
            vars: Vars::default(),
            storage: HashMap::new(),
            mock: self.mock,
            params: self.params.clone(),
            vd_set: self.vd_set.clone(),
            bld: BuildContext {
                builder,
                modules: self.modules.clone(),
            },
        };

        let mut tx_inputs = Vec::new();
        for (input_index, (_class, name)) in action.inputs().enumerate() {
            let input = &inputs[input_index];
            ctx.vars.insert(name, input.obj.clone());
            tx_inputs.push(input.tx_input());
            ctx.bld.builder.add_pod(input.pod.clone()).unwrap();
        }

        let mut tx_builder = self.new_tx_builder(&mut ctx.bld, &tx_inputs);

        let (_st_action, objs, objs_st_class_data) =
            self._action(&mut ctx, &mut tx_builder, action, &inputs);

        // Prove a pod with the class statements and the last tx statement
        ctx.bld.builder.reveal(&tx_builder.st_tx).unwrap();
        let pod = prove(ctx.bld.builder, &*self.prover);
        pod.pod.verify().unwrap();

        // Finalize tx and prove it in another pod
        let tx = tx_builder.tx;
        ctx.bld.builder = self.new_builder();
        ctx.bld.builder.add_pod(pod.clone()).unwrap();
        let tx_builder = TxBuilder::new_from_tx(&ctx.bld, tx);
        let (st_tx_finalize, tx) = tx_builder.finalize(&mut ctx.bld);
        ctx.bld.builder.reveal(&st_tx_finalize).unwrap();

        let tx_pod = prove(ctx.bld.builder, &*self.prover);
        tx_pod.pod.verify().unwrap();

        // Make one pod for each object with just the corresponding class statement.
        let mut obj_pods = Vec::new();
        for (pred, sts) in objs_st_class_data {
            ctx.bld.builder = self.new_builder();
            ctx.bld.builder.add_pod(pod.clone()).unwrap();
            let st_class = ctx
                .bld
                .apply_custom_pred(false, &pred, HashMap::new(), sts)
                .unwrap();
            ctx.bld.builder.reveal(&st_class).unwrap();

            let obj_pod = prove(ctx.bld.builder, &*self.prover);
            obj_pod.pod.verify().unwrap();
            obj_pods.push(obj_pod);
        }

        SpendableObjects {
            tx_pod,
            obj_pods,
            objs,
            tx,
        }
    }
}

#[derive(Default)]
pub struct Vars<'a> {
    vars: HashMap<&'a str, TypedValue>,
}

impl<'a> Vars<'a> {
    pub fn insert(&mut self, name: &'a str, value: impl Into<TypedValue>) {
        self.vars.insert(name, value.into());
    }
    pub fn get(&self, name: &'a str) -> &TypedValue {
        self.vars.get(name).unwrap()
    }
    pub fn get_dict_mut(&mut self, name: &'a str) -> Option<&mut Dictionary> {
        let v = self.vars.get_mut(name).unwrap();
        match v {
            TypedValue::Dictionary(v) => Some(v),
            _ => None,
        }
    }
    pub fn get_dict(&self, name: &'a str) -> Option<&Dictionary> {
        let v = self.vars.get(name).unwrap();
        match v {
            TypedValue::Dictionary(v) => Some(v),
            _ => None,
        }
    }
    // Resolve a Arg::Var or return its Literal value
    pub fn value(&self, arg: &api::Arg) -> Value {
        match arg {
            api::Arg::Literal(v) => v.clone(),
            api::Arg::Var(name) => Value::new(self.vars[*name].clone()),
        }
    }
}

struct Action {
    name: String,
    depends: Vec<Step>,
    inputs: Vec<Step>,
    mutates: Vec<Step>,
    outputs: Vec<Step>,
}

impl Action {
    fn steps(&self) -> impl Iterator<Item = &Step> {
        self.depends
            .iter()
            .chain(self.inputs.iter())
            .chain(self.mutates.iter())
            .chain(self.outputs.iter())
    }
    // List of (class, object name) that the action takes as input.  Includes Input and Mutate.
    fn inputs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.steps()
            .filter(|s| {
                matches!(
                    s.kind,
                    StepKind::Input | StepKind::Mutate | StepKind::Depends
                )
            })
            .map(|s| (s.class.as_str(), s.name.as_str()))
    }
    // List of (class, object name) that the action returns as output.  Includes Output and Mutate.
    fn outputs(&self) -> impl Iterator<Item = (&str, &str)> {
        self.steps()
            .filter(|s| matches!(s.kind, StepKind::Output | StepKind::Mutate))
            .map(|s| (s.class.as_str(), s.name.as_str()))
    }
    fn steps_len(&self) -> usize {
        self.depends.len() + self.inputs.len() + self.mutates.len() + self.outputs.len()
    }
    fn public_vars(&self) -> Vec<&str> {
        let mut vars = Vec::new();
        for step in self.steps() {
            match step.kind {
                StepKind::Mutate | StepKind::Output => vars.push(step.name.as_str()),
                _ => {}
            }
        }
        vars
    }
    fn public_vars_len(&self) -> usize {
        self.outputs.len() + self.mutates.len()
    }
    fn private_vars(&self) -> Vec<String> {
        let mut vars: Vec<String> = Vec::new();
        // Intermediate transactions
        for i in 1..self.steps_len() {
            vars.push(format!("tx{}", i));
        }
        for step in self.steps() {
            // Non-output/mutate item states
            if !matches!(step.kind, StepKind::Output | StepKind::Mutate) {
                vars.push(step.name.to_string());
            }
            // Intermediate item states
            for i in 0..step.details_update_len() {
                vars.push(format!("{}{}", step.name, i));
            }
            // Details variables
            for detail in &step.details {
                if let api::Detail::Var(name, ..) = detail {
                    vars.push(name.to_string());
                }
            }
        }
        vars
    }
}

enum StepKind {
    Depends,
    Input,
    Mutate,
    Output,
}

struct Step {
    kind: StepKind,
    name: String,
    class: String,
    details: Vec<api::Detail>,
}

impl Step {
    fn details_update_len(&self) -> usize {
        self.details
            .iter()
            .filter(|d| matches!(*d, api::Detail::Update(..)))
            .count()
    }
}

#[derive(Serialize)]
struct TmplContext {
    state: String,
}

struct StateVar<'a> {
    name: &'a str,
    ts: usize,
    max_ts: usize,
}

impl<'a> StateVar<'a> {
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

impl<'a> fmt::Display for StateVar<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.name)?;
        if self.ts != self.max_ts {
            write!(f, "{}", self.ts)?;
        }
        Ok(())
    }
}

struct Data {
    actions: Vec<Action>,
    classes: Vec<Class>,
    dependencies: Vec<api::Dependency>,
    // Maps from output index in the Action to statement index in the Class predicate
    output_index_class_st_index: HashMap<(String, usize), usize>,
}

fn details_set_len(details: &[api::Detail]) -> usize {
    details
        .iter()
        .filter(|d| matches!(d, api::Detail::Set(..)))
        .count()
}

impl Data {
    fn new_action(api_action: api::Action) -> Action {
        let mut depends = Vec::new();
        let mut inputs = Vec::new();
        let mut mutates = Vec::new();
        let mut outputs = Vec::new();
        for api_step in api_action.steps {
            use api::Step::*;
            match api_step {
                Depend { name, action } => depends.push(Step {
                    kind: StepKind::Depends,
                    name: name.to_string(),
                    class: action.to_string(),
                    details: vec![],
                }),
                Input {
                    name,
                    class,
                    details,
                } => {
                    assert_eq!(0, details_set_len(&details));
                    inputs.push(Step {
                        kind: StepKind::Input,
                        name: name.to_string(),
                        class: class.to_string(),
                        details,
                    })
                }
                Mutate {
                    name,
                    class,
                    details,
                } => {
                    assert_eq!(0, details_set_len(&details));
                    mutates.push(Step {
                        kind: StepKind::Mutate,
                        name: name.to_string(),
                        class: class.to_string(),
                        details,
                    })
                }
                Output {
                    name,
                    class,
                    details,
                } => outputs.push(Step {
                    kind: StepKind::Output,
                    name: name.to_string(),
                    class: class.to_string(),
                    details,
                }),
            }
        }

        Action {
            name: api_action.name.to_string(),
            depends,
            inputs,
            mutates,
            outputs,
        }
    }

    fn action_by_name(&self, name: &str) -> &Action {
        self.actions.iter().find(|a| a.name == name).unwrap()
    }

    fn class_by_name(&self, name: &str) -> &Class {
        self.classes.iter().find(|c| c.name == name).unwrap()
    }

    fn classes(actions: &[Action]) -> Vec<Class> {
        let mut class_to_actions: HashMap<&str, Vec<(String, usize)>> = HashMap::new();
        let mut classes_ordered = Vec::new();
        for action in actions {
            let mut classes = Vec::new();
            for step in action.steps() {
                let class_name = step.class.as_str();
                match step.kind {
                    StepKind::Mutate | StepKind::Output => {
                        classes.push(class_name);
                        if !classes_ordered.contains(&class_name) {
                            classes_ordered.push(class_name);
                        }
                    }
                    _ => {}
                }
            }
            for (i, class) in classes.iter().enumerate() {
                let actions = class_to_actions.entry(class).or_default();
                actions.push((action.name.clone(), i));
            }
        }
        let mut classes = Vec::new();
        for class in classes_ordered {
            classes.push(Class {
                name: class.to_string(),
                actions: class_to_actions[class].clone(),
            });
        }
        classes
    }

    fn output_index_class_st_index(actions: &[Action]) -> HashMap<(String, usize), usize> {
        let mut output_index_class_st_index = HashMap::new();
        let mut class_action_count = HashMap::new();
        for action in actions {
            for (output_index, (class, _name)) in action.outputs().enumerate() {
                let class_st_index = class_action_count.entry(class).or_insert(0);
                output_index_class_st_index
                    .insert((action.name.clone(), output_index), *class_st_index);
                *class_st_index += 1;
            }
        }
        output_index_class_st_index
    }

    fn format_podlang(&self, w: &mut dyn fmt::Write) -> Result<()> {
        for dependency in &self.dependencies {
            Self::format_dependency(w, dependency)?;
        }
        writeln!(w)?;
        writeln!(w, "// Actions\n")?;
        for action in &self.actions {
            Self::format_action(w, action)?;
        }
        writeln!(w, "// Classes\n")?;
        for class in &self.classes {
            self.format_class(w, class)?;
        }
        Ok(())
    }

    fn format_dependency(w: &mut dyn fmt::Write, dependency: &api::Dependency) -> Result<()> {
        match dependency {
            api::Dependency::Module { name, hash } => {
                writeln!(w, "use module {:#} as {name}", hash)?;
            }
            api::Dependency::Intro { pred, hash } => {
                writeln!(w, "use intro {pred} from {:#}", hash)?;
            }
        }
        Ok(())
    }

    fn format_action(w: &mut dyn fmt::Write, action: &Action) -> Result<()> {
        write!(w, "{}(", action.name)?;
        for public_var in &action.public_vars() {
            write!(w, "{}, ", public_var)?;
        }
        write!(w, "tx, tx0, private: ")?;
        let private_vars = action.private_vars();
        for (i, var) in private_vars.iter().enumerate() {
            if i != 0 {
                write!(w, ", ")?;
            }
            write!(w, "{var}")?;
        }

        writeln!(w, ") = AND (")?;
        let steps_len = action.steps_len();
        for (step_idx, step) in action.steps().enumerate() {
            let mut state = StateVar {
                name: step.name.as_str(),
                ts: 0,
                max_ts: step.details_update_len(),
            };
            let tx = format!("tx{}", step_idx);
            let tx_next = if step_idx == steps_len - 1 {
                "tx".to_string()
            } else {
                format!("tx{}", step_idx + 1)
            };
            match step.kind {
                StepKind::Input => {
                    writeln!(w, "  // Input")?;
                    writeln!(w, "  Is{}({})", step.class, state)?
                }
                StepKind::Mutate => {
                    writeln!(w, "  // Mutate")?;
                    writeln!(w, "  Is{}({})", step.class, state)?
                }
                StepKind::Output => {
                    writeln!(w, "  // Output")?;
                }
                StepKind::Depends => {
                    writeln!(w, "  // Action dependency")?;
                }
            }
            for detail in &step.details {
                match detail {
                    api::Detail::Condition(pred, ..) => {
                        let mut tt = TinyTemplate::new();
                        tt.add_template("pred", pred)?;
                        let ctx = TmplContext {
                            state: format!("{}", state),
                        };
                        writeln!(w, "  {}", tt.render("pred", &ctx)?)?;
                    }
                    api::Detail::Update(key, value) => {
                        writeln!(
                            w,
                            r#"  DictUpdate({state_next}, {state}, "{key}", {value})"#,
                            state_next = state.next()
                        )?;
                        state.inc();
                    }
                    api::Detail::Set(key, value) => {
                        writeln!(w, r#"  DictContains({state}, "{key}", {value})"#,)?;
                    }
                    api::Detail::Var(..) => {}
                }
            }
            match step.kind {
                StepKind::Depends => writeln!(
                    w,
                    "  {action}({state}, {tx_next}, {tx})",
                    action = step.class
                )?,
                StepKind::Input => writeln!(w, "  tx::TxDeleted({tx_next}, {tx}, {state})")?,
                StepKind::Mutate => {
                    writeln!(w, "  tx::TxMutated({tx_next}, {tx}, {state}, {state}0)",)?
                }
                StepKind::Output => writeln!(w, "  tx::TxInserted({tx_next}, {tx}, {state})")?,
            }
        }
        writeln!(w, ")")?;
        writeln!(w)?;
        Ok(())
    }

    fn format_class(&self, w: &mut dyn fmt::Write, class: &Class) -> Result<()> {
        let name = &class.name;
        write!(w, "Is{name}(state, private: tx, tx0")?;

        let other_len = class
            .actions
            .iter()
            .map(|(action_name, _)| self.action_by_name(action_name).public_vars_len())
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
            let action = self.action_by_name(action_name);
            let mut count = 0;
            for i in 0..action.public_vars_len() {
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
        writeln!(w)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {

    use std::sync::Arc;

    use pod2::middleware::Value;
    use pod2utils::set;
    use txlib::{StateRoot, Tx};

    use super::Helper;
    use crate::scenario::test_sdk;

    fn update_state_root(state_root: &mut StateRoot, tx: &Tx) {
        state_root
            .transactions
            .insert(&Value::from(tx.dict()))
            .unwrap();
        for nullifier in tx.nullifiers.set() {
            state_root.transactions.insert(nullifier).unwrap();
        }
    }

    #[test]
    fn test_sdk() {
        let _ = env_logger::builder().try_init();

        let helper = Helper::new(test_sdk::dependencies(), test_sdk::actions());

        let mut state_root = StateRoot {
            transactions: set!(),
            nullifiers: set!(),
        };

        let mock = true;

        let builder = helper.builder(mock, Arc::new(state_root.clone()));
        let [log_a] = builder.action("FindLog", vec![]).objs();
        update_state_root(&mut state_root, &log_a.tx);

        let builder = helper.builder(mock, Arc::new(state_root.clone()));
        let [wood_a] = builder.action("CraftWood", vec![log_a]).objs();
        update_state_root(&mut state_root, &wood_a.tx);

        let builder = helper.builder(mock, Arc::new(state_root.clone()));
        let [stick_a, stick_b] = builder.action("CraftSticks", vec![wood_a]).objs();
        update_state_root(&mut state_root, &stick_a.tx);

        let builder = helper.builder(mock, Arc::new(state_root.clone()));
        let [log_b] = builder.action("FindLog", vec![]).objs();
        update_state_root(&mut state_root, &log_b.tx);

        let builder = helper.builder(mock, Arc::new(state_root.clone()));
        let [wood_b] = builder.action("CraftWood", vec![log_b]).objs();
        update_state_root(&mut state_root, &wood_b.tx);

        let builder = helper.builder(mock, Arc::new(state_root.clone()));
        let [wood_pick] = builder
            .action("CraftWoodPick", vec![wood_b, stick_a])
            .objs();
        update_state_root(&mut state_root, &wood_pick.tx);

        let builder = helper.builder(mock, Arc::new(state_root.clone()));
        let [wood_pick, stone_a] = builder
            .action("MineStoneWithWoodPick", vec![wood_pick])
            .objs();
        update_state_root(&mut state_root, &wood_pick.tx);

        let builder = helper.builder(mock, Arc::new(state_root.clone()));
        let [stone_pick] = builder
            .action("CraftStonePick", vec![stone_a, stick_b])
            .objs();
        update_state_root(&mut state_root, &stone_pick.tx);

        let builder = helper.builder(mock, Arc::new(state_root.clone()));
        let [stone_pick, _stone_b] = builder
            .action("MineStoneWithStonePick", vec![stone_pick])
            .objs();
        update_state_root(&mut state_root, &stone_pick.tx);

        let builder = helper.builder(mock, Arc::new(state_root.clone()));
        let [stone_pick, _stone_c] = builder
            .action("MineStoneWithStonePick", vec![stone_pick])
            .objs();
        update_state_root(&mut state_root, &stone_pick.tx);
    }
}
