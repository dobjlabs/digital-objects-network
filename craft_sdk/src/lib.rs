use std::{any::Any, collections::HashMap, fmt, mem, slice, sync::Arc};

use anyhow::Result;
use fmt::Write;
use log::info;
use pod2::{
    backends::plonky2::{basetypes::DEFAULT_VD_SET, mainpod::Prover, mock::mainpod::MockProver},
    frontend::{MainPod, MultiPodBuilder, Operation},
    lang::{Module, load_module},
    middleware::{
        EMPTY_VALUE, Hash, Key, MainPodProver, Params, Predicate, Statement, VDSet, Value,
        containers::Dictionary,
    },
};
use pod2utils::{dict, macros::BuildContext, rand_raw_value};
use serde::Serialize;
use tinytemplate::TinyTemplate;
use txlib::{GroundingWitness, Tx, TxBuilder};

pub mod api {
    use std::fmt;

    use pod2::middleware::{Hash, Statement, Value};

    /// Argument to an Update/Set detail
    pub enum Arg {
        /// A literal Value embedded in the statement template
        Literal(Value),
        /// Pick up the value from a variable in the predicate context (a wildcard)
        Var(String),
    }

    impl Arg {
        pub fn literal(v: impl Into<Value>) -> Self {
            Self::Literal(v.into())
        }
        pub fn var(name: impl Into<String>) -> Self {
            Self::Var(name.into())
        }
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
        Var {
            name: String,
            f: Box<dyn Fn(&mut super::Context) -> Value>,
        },
        /// Define a condition that the object must fulfill via a predicate and the function that
        /// generates such predicate.
        Condition {
            pred: String,
            f: Box<dyn Fn(&mut super::Context) -> Statement>,
        },
        /// Update a key of the object
        Update { key: String, value: Arg },
        /// Set a key of the object (doesn't modify the object)
        Set { key: String, value: Arg },
    }

    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub enum StepKind {
        #[default]
        Depends,
        Input,
        Mutate,
        Output,
    }

    #[derive(Default)]
    pub struct Step {
        pub(crate) kind: StepKind,
        pub(crate) name: String,
        pub(crate) class: String,
        pub(crate) action: String,
        pub(crate) details: Vec<Detail>,
    }

    impl Step {
        pub fn kind(&self) -> StepKind {
            self.kind
        }
        pub fn name(&self) -> &str {
            &self.name
        }
        pub fn class(&self) -> Option<&str> {
            if matches!(self.kind, StepKind::Depends) {
                None
            } else {
                Some(self.class.as_str())
            }
        }
        pub fn action(&self) -> Option<&str> {
            if matches!(self.kind, StepKind::Depends) {
                Some(self.action.as_str())
            } else {
                None
            }
        }
        /// The action consumes an object as input
        pub fn input(name: impl Into<String>, class: impl Into<String>) -> Self {
            Self {
                kind: StepKind::Input,
                name: name.into(),
                class: class.into(),
                ..Default::default()
            }
        }
        /// The action produces an object as output
        pub fn output(name: impl Into<String>, class: impl Into<String>) -> Self {
            Self {
                kind: StepKind::Output,
                name: name.into(),
                class: class.into(),
                ..Default::default()
            }
        }
        /// The action mutates an object, which is considered both input and output
        pub fn mutate(name: impl Into<String>, class: impl Into<String>) -> Self {
            Self {
                kind: StepKind::Mutate,
                name: name.into(),
                class: class.into(),
                ..Default::default()
            }
        }
        /// The action depends on another action that affects an object
        pub fn depends(name: impl Into<String>, action: impl Into<String>) -> Self {
            Self {
                kind: StepKind::Depends,
                name: name.into(),
                action: action.into(),
                ..Default::default()
            }
        }
        /// Introduce a variable in the context
        pub fn var(
            mut self,
            name: impl Into<String>,
            f: Box<dyn Fn(&mut super::Context) -> Value>,
        ) -> Self {
            self.details.push(Detail::Var {
                name: name.into(),
                f,
            });
            self
        }
        /// Define a condition that the object must fulfill via a predicate and the function that
        /// generates such predicate.
        pub fn condition(
            mut self,
            pred: impl Into<String>,
            f: Box<dyn Fn(&mut super::Context) -> Statement>,
        ) -> Self {
            self.details.push(Detail::Condition {
                pred: pred.into(),
                f,
            });
            self
        }
        /// Update a key of the object
        pub fn update(mut self, key: impl Into<String>, arg: Arg) -> Self {
            if matches!(self.kind, StepKind::Depends) {
                panic!("kind doesn't allow \"update\"");
            }
            self.details.push(Detail::Update {
                key: key.into(),
                value: arg,
            });
            self
        }
        /// Set a key of the object (doesn't modify the object)
        pub fn set(mut self, key: impl Into<String>, arg: Arg) -> Self {
            if matches!(
                self.kind,
                StepKind::Input | StepKind::Mutate | StepKind::Depends
            ) {
                panic!("kind doesn't allow \"set\"");
            }
            self.details.push(Detail::Set {
                key: key.into(),
                value: arg,
            });
            self
        }
        /// Apply a snippet function to the Step
        pub fn snippet(self, f: impl Fn(Step) -> Self) -> Self {
            f(self)
        }
    }

    /// An action consumes objects and produces objects via a list of steps
    pub struct Action {
        pub name: &'static str,
        pub steps: Vec<Step>,
    }

    impl Action {
        pub fn name(&self) -> &str {
            self.name
        }
        pub fn steps(&self) -> &[Step] {
            &self.steps
        }
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
    pub fn builder(
        &self,
        mock: bool,
        grounding_witness: Arc<GroundingWitness>,
    ) -> ObjectBuilder<'_> {
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
            grounding_witness,
            prover,
            modules: self.modules.clone(),
            data: &self.data,
        }
    }

    pub fn action_hash(&self, action_name: &str) -> Option<Hash> {
        self.data
            .actions
            .iter()
            .find(|action| action.name == action_name)
            .and_then(|action| action.hash(&self.module))
    }

    pub fn action_hashes(&self) -> HashMap<String, Hash> {
        self.data
            .actions
            .iter()
            .filter_map(|action| {
                action
                    .hash(&self.module)
                    .map(|hash| (action.name.clone(), hash))
            })
            .collect()
    }

    pub fn class_hash(&self, class_name: &str) -> Option<Hash> {
        let predicate_name = format!("Is{class_name}");
        self.module
            .predicate_ref_by_name(predicate_name.as_str())
            .map(Predicate::Custom)
            .map(|predicate| predicate.hash())
    }

    pub fn class_hashes(&self) -> HashMap<String, Hash> {
        self.data
            .classes
            .iter()
            .filter_map(|class| {
                self.class_hash(class.name.as_str())
                    .map(|hash| (class.name.clone(), hash))
            })
            .collect()
    }
}

fn prove(builder: MultiPodBuilder, prover: &dyn MainPodProver) -> MainPod {
    let solution = builder.solve().unwrap();
    log::debug!("solution needs {} pods", solution.solution().pod_count);
    solution.prove(prover).unwrap().pods.pop().unwrap()
}

#[derive(Debug, Clone)]
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
    grounding_witness: Arc<GroundingWitness>,
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
        TxBuilder::new(ctx, inputs, self.grounding_witness.clone())
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
                    let (_name, action) = (step.name.as_str(), step.action.as_str());
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
                let obj = ctx.vars.get(&step.name).as_dictionary().unwrap();
                objs0.insert(&step.name, obj.clone());
            }

            let details_set_len = details_set_len(&step.details);
            let mut details_set_kv = Vec::new();
            let mut details_set_done = details_set_len == 0;
            for detail in &step.details {
                match detail {
                    api::Detail::Set { key, value } => {
                        let value = ctx.vars.value(value);
                        ctx.vars.mut_dict(&step.name, |obj| {
                            obj.insert(&Key::from(key.clone()), &value).unwrap();
                        });
                        details_set_kv.push((key.clone(), value));
                    }
                    api::Detail::Update { key, value } => {
                        if !details_set_done {
                            panic!("Update before last Set in the Step");
                        }
                        let value = ctx.vars.value(value);
                        let (obj0, obj) = ctx.vars.mut_dict(&step.name, |obj| {
                            let obj0 = obj.clone();
                            obj.update(&Key::from(key.clone()), &value).unwrap();
                            (obj0, obj.clone())
                        });
                        sts.push(
                            ctx.bld
                                .builder
                                .priv_op(Operation::dict_update(obj, obj0, key.clone(), value))
                                .unwrap(),
                        );
                    }
                    api::Detail::Var { name, f } => {
                        let value = f(ctx);
                        ctx.vars.insert(name, value);
                    }
                    api::Detail::Condition { pred: _, f } => {
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
                        let obj = ctx.vars.get(&step.name).as_dictionary().unwrap();
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
                    let obj = ctx.vars.get(step.name.as_str()).as_dictionary().unwrap();
                    outputs.push(OutputData {
                        class: &step.class,
                        obj: obj.clone(),
                    });
                    sts.push(tx_builder.insert(&mut ctx.bld, obj));
                }
                StepKind::Input => {
                    let obj = ctx.vars.get(step.name.as_str()).as_dictionary().unwrap();
                    sts.push(tx_builder.delete(&mut ctx.bld, obj));
                }
                StepKind::Mutate => {
                    let obj0 = objs0.get(step.name.as_str()).unwrap();
                    let obj = ctx.vars.get(step.name.as_str()).as_dictionary().unwrap();
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
        ctx.bld.builder.reveal(tx_builder.st_tx()).unwrap();
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
    vars: HashMap<&'a str, Value>,
}

impl<'a> Vars<'a> {
    pub(crate) fn insert(&mut self, name: &'a str, value: impl Into<Value>) {
        self.vars.insert(name, value.into());
    }
    pub fn get(&self, name: &'a str) -> &Value {
        self.vars.get(name).unwrap()
    }
    pub(crate) fn mut_dict<T>(
        &mut self,
        name: &'a str,
        mut f: impl FnMut(&mut Dictionary) -> T,
    ) -> T {
        let obj = self.vars.get_mut(name).unwrap();
        let mut dict = obj.as_dictionary().unwrap();
        let output = f(&mut dict);
        *obj = Value::from(dict);
        output
    }
    // Resolve a Arg::Var or return its Literal value
    pub fn value(&self, arg: &api::Arg) -> Value {
        match arg {
            api::Arg::Literal(v) => v.clone(),
            api::Arg::Var(name) => self.vars[name.as_str()].clone(),
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
                if let api::Detail::Var { name, .. } = detail {
                    vars.push(name.to_string());
                }
            }
        }
        vars
    }

    fn hash(&self, module: &Module) -> Option<Hash> {
        module
            .predicate_ref_by_name(self.name.as_str())
            .map(Predicate::Custom)
            .map(|predicate| predicate.hash())
    }
}

use api::{Step, StepKind};

impl Step {
    fn details_update_len(&self) -> usize {
        self.details
            .iter()
            .filter(|d| matches!(*d, api::Detail::Update { .. }))
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
        .filter(|d| matches!(d, api::Detail::Set { .. }))
        .count()
}

impl Data {
    fn new_action(api_action: api::Action) -> Action {
        let mut depends = Vec::new();
        let mut inputs = Vec::new();
        let mut mutates = Vec::new();
        let mut outputs = Vec::new();
        for api_step in api_action.steps {
            use api::StepKind::*;
            match api_step.kind {
                Depends => depends.push(api_step),
                Input => inputs.push(api_step),
                Mutate => mutates.push(api_step),
                Output => outputs.push(api_step),
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
                    api::Detail::Condition { pred, .. } => {
                        let mut tt = TinyTemplate::new();
                        tt.add_template("pred", pred)?;
                        let ctx = TmplContext {
                            state: format!("{}", state),
                        };
                        writeln!(w, "  {}", tt.render("pred", &ctx)?)?;
                    }
                    api::Detail::Update { key, value } => {
                        writeln!(
                            w,
                            r#"  DictUpdate({state_next}, {state}, "{key}", {value})"#,
                            state_next = state.next()
                        )?;
                        state.inc();
                    }
                    api::Detail::Set { key, value } => {
                        writeln!(w, r#"  DictContains({state}, "{key}", {value})"#,)?;
                    }
                    api::Detail::Var { .. } => {}
                }
            }
            match step.kind {
                StepKind::Depends => writeln!(
                    w,
                    "  {action}({state}, {tx_next}, {tx})",
                    action = step.action
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

    use std::{collections::HashSet, sync::Arc};

    use hex::FromHex;
    use lt_eq_u256_pod::LtEqU256Pod;
    use pod2::{
        frontend::{MainPod, Operation},
        middleware::{F, Hash, Key, Pod, RawValue, Statement, Value, containers::{Array, Set}},
    };
    use pod2utils::rand_raw_value;
    use txlib::{GroundingWitness, StateRoot, Tx};
    use vdfpod::VdfPod;

    use super::{Context, Helper, api::*};

    const WOOD_POW_DIFFICULTY: u64 = 0x0020_0000_0000_0000;

    #[derive(Default)]
    struct TestState {
        block_number: i64,
        transactions: HashSet<Hash>,
        nullifiers: HashSet<Hash>,
        gsrs: Vec<Hash>,
    }

    impl TestState {
        fn state_root(&self) -> StateRoot {
            let transactions_root =
                Set::new(self.transactions.iter().map(|hash| Value::from(*hash)).collect())
                    .commitment();
            let nullifiers_root =
                Set::new(self.nullifiers.iter().map(|hash| Value::from(*hash)).collect())
                    .commitment();
            let gsrs_root =
                Array::new(self.gsrs.iter().map(|hash| Value::from(*hash)).collect()).commitment();
            StateRoot::new(
                self.block_number,
                transactions_root,
                nullifiers_root,
                gsrs_root,
            )
        }

        fn grounding_witness(&self, inputs: &[Tx]) -> GroundingWitness {
            let tx_set = Set::new(
                self.transactions
                    .iter()
                    .map(|hash| Value::from(*hash))
                    .collect(),
            );
            let source_tx_proofs = inputs
                .iter()
                .map(|tx| {
                    let tx_hash = tx.dict().commitment();
                    let proof = tx_set.prove(&Value::from(tx_hash)).unwrap();
                    (tx_hash, proof)
                })
                .collect();
            GroundingWitness::new(self.state_root(), source_tx_proofs)
        }

        fn apply_tx(&mut self, tx: &Tx) {
            self.transactions.insert(tx.dict().commitment());
            for nullifier in tx.nullifiers.iter() {
                let nullifier = nullifier.unwrap();
                self.nullifiers.insert(Hash(nullifier.raw().0));
            }
            self.block_number += 1;
        }
    }

    fn main_pod(ctx: &Context, pod: Box<dyn Pod>) -> MainPod {
        let pub_statements = pod.pub_statements();
        MainPod {
            pod,
            public_statements: pub_statements,
            params: ctx.params.clone(),
        }
    }

    // Returns VdfPod, Vdf statement, work
    fn vdf(ctx: &mut Context, n_iters: usize, input: RawValue) -> (MainPod, Statement, Value) {
        let vdf_pod = if ctx.mock {
            VdfPod::new_boxed_mock(&ctx.params, ctx.vd_set.clone(), n_iters, input)
        } else {
            VdfPod::new_boxed(&ctx.params, ctx.vd_set.clone(), n_iters, input)
        }
        .unwrap();
        let st_vdf = vdf_pod.pub_statements()[0].clone();
        let work = st_vdf.args()[2].literal().unwrap();
        (main_pod(ctx, vdf_pod), st_vdf, work)
    }

    // Returns LtEqU256Pod and LtEqU256 statement used to verify PoW.
    fn lt_eq_u256(ctx: &mut Context, lhs: RawValue, rhs: RawValue) -> (MainPod, Statement) {
        let lt_eq_u256_pod = if ctx.mock {
            LtEqU256Pod::new_boxed_mock(&ctx.params, ctx.vd_set.clone(), lhs, rhs)
        } else {
            LtEqU256Pod::new_boxed(&ctx.params, ctx.vd_set.clone(), lhs, rhs)
        }
        .unwrap();
        let st_lt_eq_u256 = lt_eq_u256_pod.pub_statements()[0].clone();
        (main_pod(ctx, lt_eq_u256_pod), st_lt_eq_u256)
    }

    #[test]
    fn test_sdk() {
        let _ = env_logger::builder().try_init();

        let find_log = Action {
            name: "FindLog",
            steps: vec![
                Step::output("log", "Log")
                    .set("blueprint", Arg::literal("Log"))
                    .var(
                        "work",
                        Box::new(|ctx| {
                            let log = ctx.vars.get("log");
                            let log_raw = log.as_raw();
                            let (vdf_pod, st_vdf, work) = vdf(ctx, 3, log_raw);
                            ctx.store("vdf_pod", Box::new(vdf_pod));
                            ctx.store("st_vdf", Box::new(st_vdf));
                            work
                        }),
                    )
                    .condition(
                        "Vdf(3, {state}, work)",
                        Box::new(|ctx| {
                            let vdf_pod: Box<MainPod> = ctx.take("vdf_pod");
                            let st_vdf: Box<Statement> = ctx.take("st_vdf");
                            ctx.bld.builder.add_pod(*vdf_pod).unwrap();
                            *st_vdf
                        }),
                    )
                    .update("work", Arg::var("work")),
            ],
        };

        let craft_wood = Action {
            name: "CraftWood",
            steps: vec![
                Step::input("log", "Log"),
                Step::output("wood", "Wood")
                    .set("blueprint", Arg::literal("Wood"))
                    .var(
                        "key",
                        Box::new(|ctx| {
                            let mut wood = ctx.vars.get("wood").as_dictionary().unwrap();
                            let mut key = Value::from(rand_raw_value());
                            if !ctx.mock {
                                while RawValue::from(wood.commitment()).0[3].0
                                    > WOOD_POW_DIFFICULTY
                                {
                                    key = Value::from(rand_raw_value());

                                    wood.update(&Key::from("key"), &key).unwrap();
                                }
                            }
                            key
                        }),
                    )
                    .update("key", Arg::var("key"))
                    .condition(
                        "LtEqU256({state}, Raw(0x0020000000000000000000000000000000000000000000000000000000000000))",
                        Box::new(|ctx| {
                            let wood = ctx.vars.get("wood");
                            let wood_raw = wood.as_raw();
                            let (lt_eq_u256_pod, st_lt_eq_u256) = lt_eq_u256(
                                ctx,
                                wood_raw,
                                RawValue([F(0), F(0), F(0), F(WOOD_POW_DIFFICULTY)]),
                            );
                            ctx.bld.builder.add_pod(lt_eq_u256_pod).unwrap();
                            st_lt_eq_u256
                        }),
                    )
                ]
        };

        let craft_sticks = Action {
            name: "CraftSticks",
            steps: vec![
                Step::input("wood", "Wood"),
                Step::output("stick_a", "Stick").set("blueprint", Arg::literal("Stick")),
                Step::output("stick_b", "Stick").set("blueprint", Arg::literal("Stick")),
            ],
        };

        let craft_wood_pick = Action {
            name: "CraftWoodPick",
            steps: vec![
                Step::input("wood", "Wood"),
                Step::input("stick", "Stick"),
                Step::output("wood_pick", "WoodPick")
                    .set("blueprint", Arg::literal("WoodPick"))
                    .set("durability", Arg::literal(100i64)),
            ],
        };

        let craft_stone_pick = Action {
            name: "CraftStonePick",
            steps: vec![
                Step::input("stone", "Stone"),
                Step::input("stick", "Stick"),
                Step::output("stone_pick", "StonePick")
                    .set("blueprint", Arg::literal("StonePick"))
                    .set("durability", Arg::literal(200i64)),
            ],
        };

        fn use_pick_details(step: Step, name: &'static str, vdf_iters: usize) -> Step {
            step.condition(
                "Gt({state}.durability, 0)",
                Box::new(|ctx| {
                    let obj = ctx.vars.get(name).as_dictionary().unwrap();
                    ctx.bld
                        .builder
                        .priv_op(Operation::gt((&obj, "durability"), 0))
                        .unwrap()
                }),
            )
            .var(
                "durability",
                Box::new(|ctx| {
                    let obj = ctx.vars.get(name).as_dictionary().unwrap();
                    let mut durability = obj
                        .get(&Key::from("durability"))
                        .unwrap()
                        .unwrap()
                        .as_int()
                        .unwrap();
                    durability -= 1;
                    ctx.store("durability", Box::new(durability));
                    Value::from(durability)
                }),
            )
            .condition(
                "SumOf({state}.durability, durability, 1)",
                Box::new(|ctx| {
                    let durability: Box<i64> = ctx.take("durability");
                    let obj = ctx.vars.get(name).as_dictionary().unwrap();
                    ctx.bld
                        .builder
                        .priv_op(Operation::sum_of((&obj, "durability"), *durability, 1))
                        .unwrap()
                }),
            )
            .update("durability", Arg::var("durability"))
            .var("key", Box::new(|_ctx| Value::from(rand_raw_value())))
            .update("key", Arg::var("key"))
            .var(
                "work",
                Box::new(move |ctx| {
                    let obj = ctx.vars.get(name);
                    let obj_raw = obj.as_raw();
                    let (vdf_pod, st_vdf, work) = vdf(ctx, vdf_iters, obj_raw);
                    ctx.store("vdf_pod", Box::new(vdf_pod));
                    ctx.store("st_vdf", Box::new(st_vdf));
                    work
                }),
            )
            .condition(
                format!("Vdf({vdf_iters}, {{state}}, work)").leak(),
                Box::new(|ctx| {
                    let vdf_pod: Box<MainPod> = ctx.take("vdf_pod");
                    let st_vdf: Box<Statement> = ctx.take("st_vdf");
                    ctx.bld.builder.add_pod(*vdf_pod).unwrap();
                    *st_vdf
                }),
            )
            .update("work", Arg::var("work"))
        }

        let use_wood_pick = Action {
            name: "UseWoodPick",
            steps: vec![
                Step::mutate("wood_pick", "WoodPick")
                    .snippet(|step| use_pick_details(step, "wood_pick", 10)),
            ],
        };

        let mine_stone_with_wood_pick = Action {
            name: "MineStoneWithWoodPick",
            steps: vec![
                Step::depends("pick", "UseWoodPick"),
                Step::output("stone", "Stone").set("blueprint", Arg::literal("Stone")),
            ],
        };

        let use_stone_pick = Action {
            name: "UseStonePick",
            steps: vec![
                Step::mutate("stone_pick", "StonePick")
                    .snippet(|step| use_pick_details(step, "stone_pick", 5)),
            ],
        };

        let mine_stone_with_stone_pick = Action {
            name: "MineStoneWithStonePick",
            steps: vec![
                Step::depends("pick", "UseStonePick"),
                Step::output("stone", "Stone").set("blueprint", Arg::literal("Stone")),
            ],
        };

        let dependencies = vec![
            Dependency::Intro {
                pred: "Vdf(count, input, output)",
                hash: Hash::from_hex(
                    "b77a964de74c8569e6c6172692bb50147df9334fd9b572abc8d4d9c688a40e06",
                )
                .unwrap(),
            },
            Dependency::Intro {
                pred: "LtEqU256(lhs, rhs)",
                hash: Hash::from_hex(
                    "2e79114ee823f4783ab5b6eb93b49abba87fb69b4d14de4cf1d78648ade73529",
                )
                .unwrap(),
            },
        ];

        let helper = Helper::new(
            dependencies,
            vec![
                find_log,
                craft_wood,
                craft_sticks,
                craft_wood_pick,
                craft_stone_pick,
                use_wood_pick,
                mine_stone_with_wood_pick,
                use_stone_pick,
                mine_stone_with_stone_pick,
            ],
        );
        println!("{}", helper.podlang_src);

        let mut state = TestState::default();

        let mock = true;

        let builder = helper.builder(mock, Arc::new(state.grounding_witness(&[])));
        let [log_a] = builder.action("FindLog", vec![]).objs();
        state.apply_tx(&log_a.tx);

        let builder = helper.builder(mock, Arc::new(state.grounding_witness(&[log_a.tx.clone()])));
        let [wood_a] = builder.action("CraftWood", vec![log_a]).objs();
        state.apply_tx(&wood_a.tx);

        let builder = helper.builder(
            mock,
            Arc::new(state.grounding_witness(&[wood_a.tx.clone()])),
        );
        let [stick_a, stick_b] = builder.action("CraftSticks", vec![wood_a]).objs();
        state.apply_tx(&stick_a.tx);

        let builder = helper.builder(mock, Arc::new(state.grounding_witness(&[])));
        let [log_b] = builder.action("FindLog", vec![]).objs();
        state.apply_tx(&log_b.tx);

        let builder = helper.builder(mock, Arc::new(state.grounding_witness(&[log_b.tx.clone()])));
        let [wood_b] = builder.action("CraftWood", vec![log_b]).objs();
        state.apply_tx(&wood_b.tx);

        let builder = helper.builder(
            mock,
            Arc::new(state.grounding_witness(&[wood_b.tx.clone(), stick_a.tx.clone()])),
        );
        let [wood_pick] = builder
            .action("CraftWoodPick", vec![wood_b, stick_a])
            .objs();
        state.apply_tx(&wood_pick.tx);

        let builder = helper.builder(
            mock,
            Arc::new(state.grounding_witness(&[wood_pick.tx.clone()])),
        );
        let [wood_pick, stone_a] = builder
            .action("MineStoneWithWoodPick", vec![wood_pick])
            .objs();
        state.apply_tx(&wood_pick.tx);

        let builder = helper.builder(
            mock,
            Arc::new(state.grounding_witness(&[stone_a.tx.clone(), stick_b.tx.clone()])),
        );
        let [stone_pick] = builder
            .action("CraftStonePick", vec![stone_a, stick_b])
            .objs();
        state.apply_tx(&stone_pick.tx);

        let builder = helper.builder(
            mock,
            Arc::new(state.grounding_witness(&[stone_pick.tx.clone()])),
        );
        let [stone_pick, _stone_b] = builder
            .action("MineStoneWithStonePick", vec![stone_pick])
            .objs();
        state.apply_tx(&stone_pick.tx);

        let builder = helper.builder(
            mock,
            Arc::new(state.grounding_witness(&[stone_pick.tx.clone()])),
        );
        let [stone_pick, _stone_c] = builder
            .action("MineStoneWithStonePick", vec![stone_pick])
            .objs();
        state.apply_tx(&stone_pick.tx);
    }
}
