use std::{cell::RefCell, collections::HashMap, rc::Rc};

use craft_sdk::{
    Context,
    api::{self, Arg, Step},
};
use lt_eq_u256_pod::LtEqU256Pod;
use plugin_api::*;
use pod2::{
    frontend::MainPod,
    middleware::{
        EMPTY_VALUE, F, Key, Pod, RawValue, Statement, VDSet, Value, containers::Dictionary,
    },
};
use pod2utils::{dict, rand_raw_value};
use rhai::{AST, Engine, ImmutableString, Scope};
use vdfpod::VdfPod;

const RUNTIME_STACK_KEY: &str = "__plugin_runtime_stack";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ExpectedEventKind {
    Var,
    Condition,
}

struct ActionRuntimeTemplate {
    action_name: String,
    fn_name: String,
    steps: Vec<StepMeta>,
    expected_events: Vec<ExpectedEventKind>,
    ast: Rc<AST>,
}

#[derive(Clone)]
struct RuntimeEnv {
    mock: bool,
    params: pod2::middleware::Params,
    vd_set: VDSet,
}

struct RuntimeExecState {
    template: Rc<ActionRuntimeTemplate>,
    env: RuntimeEnv,
    next_step_idx: usize,
    objects: HashMap<i64, Dictionary>,
    available_inputs: HashMap<String, Dictionary>,
    output_initials: HashMap<String, Dictionary>,
    events: Vec<RuntimeEvent>,
    pending_int_reads: HashMap<(i64, String), usize>,
    value_tokens: HashMap<String, Value>,
    next_token_id: usize,
}

struct ActionRuntimeState {
    action_name: String,
    next_event: usize,
    events: Vec<Option<RuntimeEvent>>,
}

enum RuntimeEvent {
    Var(Value),
    Condition(RuntimeCondition),
}

enum RuntimeCondition {
    StoredPod {
        pod: MainPod,
        statement: Statement,
    },
    Gt {
        object_name: String,
        key: String,
        value: i64,
    },
    SumOf {
        object_name: String,
        key: String,
        value: i64,
        b: i64,
    },
}

impl RuntimeEvent {
    fn kind(&self) -> ExpectedEventKind {
        match self {
            Self::Var(_) => ExpectedEventKind::Var,
            Self::Condition(_) => ExpectedEventKind::Condition,
        }
    }
}

impl RuntimeCondition {
    fn materialize(self, ctx: &mut Context<'_>) -> Statement {
        match self {
            Self::StoredPod { pod, statement } => {
                ctx.bld.builder.add_pod(pod).unwrap();
                statement
            }
            Self::Gt {
                object_name,
                key,
                value,
            } => {
                let obj = ctx.vars.get(object_name.as_str()).as_dictionary().unwrap();
                ctx.bld
                    .builder
                    .priv_op(pod2::frontend::Operation::gt((&obj, key.as_str()), value))
                    .unwrap()
            }
            Self::SumOf {
                object_name,
                key,
                value,
                b,
            } => {
                let obj = ctx.vars.get(object_name.as_str()).as_dictionary().unwrap();
                ctx.bld
                    .builder
                    .priv_op(pod2::frontend::Operation::sum_of(
                        (&obj, key.as_str()),
                        value,
                        b,
                    ))
                    .unwrap()
            }
        }
    }
}

impl ActionRuntimeTemplate {
    fn new(action_meta: &ActionMeta, ast: Rc<AST>) -> Rc<Self> {
        let expected_events = action_meta
            .steps
            .iter()
            .flat_map(|step| step.details.iter())
            .filter_map(|detail| match detail {
                DetailMeta::Var { .. } => Some(ExpectedEventKind::Var),
                DetailMeta::Condition { .. } => Some(ExpectedEventKind::Condition),
                DetailMeta::Set { .. } | DetailMeta::Update { .. } => None,
            })
            .collect();

        Rc::new(Self {
            action_name: action_meta.name.clone(),
            fn_name: action_meta.fn_name.clone(),
            steps: action_meta.steps.clone(),
            expected_events,
            ast,
        })
    }
}

impl RuntimeExecState {
    fn new(ctx: &Context<'_>, template: Rc<ActionRuntimeTemplate>) -> Self {
        let available_inputs = template
            .steps
            .iter()
            .filter_map(|step| match step.kind {
                StepKindMeta::Input | StepKindMeta::Mutate => Some((
                    step.name.clone(),
                    ctx.vars
                        .get(step.name.as_str())
                        .as_dictionary()
                        .expect("input object should be a dictionary"),
                )),
                StepKindMeta::Output | StepKindMeta::Depends => None,
            })
            .collect();

        Self {
            template,
            env: RuntimeEnv {
                mock: ctx.mock,
                params: ctx.params.clone(),
                vd_set: ctx.vd_set.clone(),
            },
            next_step_idx: 0,
            objects: HashMap::new(),
            available_inputs,
            output_initials: HashMap::new(),
            events: Vec::new(),
            pending_int_reads: HashMap::new(),
            value_tokens: HashMap::new(),
            next_token_id: 0,
        }
    }

    fn expect_step(
        &mut self,
        expected_kind: StepKindMeta,
        name: &str,
        class: Option<&str>,
        action: Option<&str>,
    ) -> i64 {
        let idx = self.next_step_idx;
        let step = self.template.steps.get(idx).unwrap_or_else(|| {
            panic!(
                "{}: unexpected extra step {}",
                self.template.action_name, name
            )
        });

        assert_eq!(
            step.kind, expected_kind,
            "{}: step {} kind mismatch",
            self.template.action_name, name
        );
        assert_eq!(
            step.name, name,
            "{}: step name mismatch at index {}",
            self.template.action_name, idx
        );

        if let Some(class) = class {
            assert_eq!(
                step.class, class,
                "{}: class mismatch for step {}",
                self.template.action_name, name
            );
        }
        if let Some(action) = action {
            assert_eq!(
                step.action, action,
                "{}: dependency mismatch for step {}",
                self.template.action_name, name
            );
        }

        self.next_step_idx += 1;
        idx as i64
    }

    fn object_name(&self, handle: i64) -> &str {
        self.template.steps[handle as usize].name.as_str()
    }

    fn object(&self, handle: i64) -> &Dictionary {
        self.objects.get(&handle).unwrap_or_else(|| {
            panic!(
                "{}: missing object handle {}",
                self.template.action_name, handle
            )
        })
    }

    fn object_mut(&mut self, handle: i64) -> &mut Dictionary {
        self.objects.get_mut(&handle).unwrap_or_else(|| {
            panic!(
                "{}: missing object handle {}",
                self.template.action_name, handle
            )
        })
    }

    fn push_event(&mut self, event: RuntimeEvent) {
        self.events.push(event);
    }

    fn insert_token(&mut self, value: Value) -> ImmutableString {
        let token = format!("__plugin_value_{}", self.next_token_id);
        self.next_token_id += 1;
        self.value_tokens.insert(token.clone(), value);
        token.into()
    }

    fn resolve_token(&self, token: &str) -> Value {
        self.value_tokens
            .get(token)
            .cloned()
            .unwrap_or_else(|| Value::from(token))
    }

    fn backfill_int_read(&mut self, handle: i64, key: &str, value: i64) {
        if let Some(event_idx) = self
            .pending_int_reads
            .get(&(handle, key.to_string()))
            .copied()
        {
            self.events[event_idx] = RuntimeEvent::Var(Value::from(value));
        }
    }

    fn finish(self) -> (HashMap<String, Dictionary>, Vec<RuntimeEvent>) {
        assert_eq!(
            self.next_step_idx,
            self.template.steps.len(),
            "{}: script did not traverse all recorded steps",
            self.template.action_name
        );

        let actual_event_kinds: Vec<_> = self.events.iter().map(RuntimeEvent::kind).collect();
        assert_eq!(
            actual_event_kinds, self.template.expected_events,
            "{}: proof-time runtime diverged from recorded detail structure",
            self.template.action_name
        );

        (self.output_initials, self.events)
    }
}

pub(crate) fn script_to_actions(meta: &PluginMetadata, script_source: &str) -> Vec<api::Action> {
    let ast = Rc::new(
        crate::create_engine()
            .compile(script_source)
            .expect("plugin script compiled during load"),
    );

    meta.actions
        .iter()
        .map(|action_meta| build_action(action_meta, ast.clone()))
        .collect()
}

fn build_action(action_meta: &ActionMeta, ast: Rc<AST>) -> api::Action {
    let template = ActionRuntimeTemplate::new(action_meta, ast);
    api::Action {
        name: action_meta.name.clone().leak(),
        steps: action_meta
            .steps
            .iter()
            .map(|step_meta| build_step(step_meta, template.clone()))
            .collect(),
        prepare: Some(prepare_closure(template)),
    }
}

fn build_step(step_meta: &StepMeta, template: Rc<ActionRuntimeTemplate>) -> Step {
    let mut step = match step_meta.kind {
        StepKindMeta::Input => Step::input(&step_meta.name, &step_meta.class),
        StepKindMeta::Output => Step::output(&step_meta.name, &step_meta.class),
        StepKindMeta::Mutate => Step::mutate(&step_meta.name, &step_meta.class),
        StepKindMeta::Depends => Step::depends(&step_meta.name, &step_meta.action),
    };

    for detail in &step_meta.details {
        step = apply_detail(step, detail, template.clone());
    }

    step
}

fn apply_detail(step: Step, detail: &DetailMeta, template: Rc<ActionRuntimeTemplate>) -> Step {
    match detail {
        DetailMeta::Set { key, value } => step.set(key, Arg::literal(literal_to_value(value))),
        DetailMeta::Update { key, source } => step.update(key, Arg::var(source)),
        DetailMeta::Var { name, .. } => step.var(name, make_var_closure(template)),
        DetailMeta::Condition { pred, .. } => {
            step.condition(pred.clone().leak(), make_condition_closure(template))
        }
    }
}

fn literal_to_value(lit: &LiteralValue) -> Value {
    match lit {
        LiteralValue::Int(n) => Value::from(*n),
        LiteralValue::Str(s) => Value::from(s.as_str()),
    }
}

fn prepare_closure(template: Rc<ActionRuntimeTemplate>) -> Box<dyn Fn(&mut Context<'_>)> {
    Box::new(move |ctx| {
        let engine = create_runtime_engine(ctx, template.clone());
        let mut scope = Scope::new();
        engine
            .call_fn::<()>(&mut scope, &template.ast, &template.fn_name, ())
            .unwrap_or_else(|err| {
                panic!(
                    "{}: failed to execute Rhai action {}: {err}",
                    template.action_name, template.fn_name
                )
            });

        let runtime_state = engine
            .take_state()
            .borrow_mut()
            .take()
            .expect("runtime execution state should be present");
        let (output_initials, events) = runtime_state.finish();

        for (name, dict) in output_initials {
            ctx.stage_output(name, dict);
        }
        if events.is_empty() {
            return;
        }

        let mut stack = take_runtime_stack(ctx);
        stack.push(ActionRuntimeState {
            action_name: template.action_name.clone(),
            next_event: 0,
            events: events.into_iter().map(Some).collect(),
        });
        store_runtime_stack(ctx, stack);
    })
}

fn make_var_closure(template: Rc<ActionRuntimeTemplate>) -> Box<dyn Fn(&mut Context<'_>) -> Value> {
    Box::new(
        move |ctx| match take_next_event(ctx, &template.action_name, ExpectedEventKind::Var) {
            RuntimeEvent::Var(value) => value,
            RuntimeEvent::Condition(_) => unreachable!(),
        },
    )
}

fn make_condition_closure(
    template: Rc<ActionRuntimeTemplate>,
) -> Box<dyn Fn(&mut Context<'_>) -> Statement> {
    Box::new(move |ctx| {
        match take_next_event(ctx, &template.action_name, ExpectedEventKind::Condition) {
            RuntimeEvent::Condition(condition) => condition.materialize(ctx),
            RuntimeEvent::Var(_) => unreachable!(),
        }
    })
}

fn take_runtime_stack(ctx: &mut Context<'_>) -> Vec<ActionRuntimeState> {
    if ctx.contains(RUNTIME_STACK_KEY) {
        *ctx.take::<Vec<ActionRuntimeState>>(RUNTIME_STACK_KEY)
    } else {
        Vec::new()
    }
}

fn store_runtime_stack(ctx: &mut Context<'_>, stack: Vec<ActionRuntimeState>) {
    if !stack.is_empty() {
        ctx.store(RUNTIME_STACK_KEY, Box::new(stack));
    }
}

fn take_next_event(
    ctx: &mut Context<'_>,
    action_name: &str,
    expected_kind: ExpectedEventKind,
) -> RuntimeEvent {
    let mut stack = take_runtime_stack(ctx);
    let top = stack
        .last_mut()
        .unwrap_or_else(|| panic!("{action_name}: missing prepared runtime state"));
    assert_eq!(
        top.action_name, action_name,
        "{}: runtime stack mismatch, found {}",
        action_name, top.action_name
    );

    let event = top
        .events
        .get_mut(top.next_event)
        .and_then(Option::take)
        .unwrap_or_else(|| panic!("{action_name}: missing runtime event {}", top.next_event));

    assert_eq!(
        event.kind(),
        expected_kind,
        "{action_name}: runtime event kind mismatch at {}",
        top.next_event
    );

    top.next_event += 1;
    if top.next_event == top.events.len() {
        stack.pop();
    }
    store_runtime_stack(ctx, stack);
    event
}

struct RuntimeEngine {
    engine: Engine,
    state: Rc<RefCell<Option<RuntimeExecState>>>,
}

impl RuntimeEngine {
    fn call_fn<T: Clone + 'static>(
        &self,
        scope: &mut Scope<'_>,
        ast: &AST,
        fn_name: &str,
        args: impl rhai::FuncArgs,
    ) -> Result<T, Box<rhai::EvalAltResult>> {
        self.engine.call_fn(scope, ast, fn_name, args)
    }

    fn take_state(&self) -> Rc<RefCell<Option<RuntimeExecState>>> {
        self.state.clone()
    }
}

fn create_runtime_engine(ctx: &Context<'_>, template: Rc<ActionRuntimeTemplate>) -> RuntimeEngine {
    let state = Rc::new(RefCell::new(Some(RuntimeExecState::new(ctx, template))));
    let mut engine = crate::create_engine();

    let s = state.clone();
    engine.register_fn(
        "output",
        move |name: ImmutableString, class: ImmutableString| -> i64 {
            let mut state = s.borrow_mut();
            let state = state.as_mut().unwrap();
            let handle = state.expect_step(StepKindMeta::Output, &name, Some(&class), None);
            let init = default_output_dict();
            state.objects.insert(handle, init.clone());
            state.output_initials.insert(name.to_string(), init);
            handle
        },
    );

    let s = state.clone();
    engine.register_fn(
        "input",
        move |name: ImmutableString, class: ImmutableString| -> i64 {
            let mut state = s.borrow_mut();
            let state = state.as_mut().unwrap();
            let handle = state.expect_step(StepKindMeta::Input, &name, Some(&class), None);
            let dict = state
                .available_inputs
                .get(name.as_str())
                .unwrap_or_else(|| panic!("missing input object {}", name))
                .clone();
            state.objects.insert(handle, dict);
            handle
        },
    );

    let s = state.clone();
    engine.register_fn(
        "mutate",
        move |name: ImmutableString, class: ImmutableString| -> i64 {
            let mut state = s.borrow_mut();
            let state = state.as_mut().unwrap();
            let handle = state.expect_step(StepKindMeta::Mutate, &name, Some(&class), None);
            let dict = state
                .available_inputs
                .get(name.as_str())
                .unwrap_or_else(|| panic!("missing mutate object {}", name))
                .clone();
            state.objects.insert(handle, dict);
            handle
        },
    );

    let s = state.clone();
    engine.register_fn(
        "depends",
        move |name: ImmutableString, action: ImmutableString| {
            let mut state = s.borrow_mut();
            let state = state.as_mut().unwrap();
            state.expect_step(StepKindMeta::Depends, &name, None, Some(&action));
        },
    );

    let s = state.clone();
    engine.register_fn(
        "set",
        move |handle: i64, key: ImmutableString, value: ImmutableString| {
            let mut state = s.borrow_mut();
            let state = state.as_mut().unwrap();
            state
                .object_mut(handle)
                .insert(&Key::from(key.to_string()), &Value::from(value.as_str()))
                .unwrap();
        },
    );

    let s = state.clone();
    engine.register_fn(
        "set_int",
        move |handle: i64, key: ImmutableString, value: i64| {
            let mut state = s.borrow_mut();
            let state = state.as_mut().unwrap();
            state
                .object_mut(handle)
                .insert(&Key::from(key.to_string()), &Value::from(value))
                .unwrap();
        },
    );

    let s = state.clone();
    engine.register_fn(
        "update",
        move |handle: i64, key: ImmutableString, source: ImmutableString| {
            let mut state = s.borrow_mut();
            let state = state.as_mut().unwrap();
            let value = state.resolve_token(source.as_str());
            state
                .object_mut(handle)
                .update(&Key::from(key.to_string()), &value)
                .unwrap();
        },
    );

    let s = state.clone();
    engine.register_fn(
        "update_int",
        move |handle: i64, key: ImmutableString, value: i64| {
            let mut state = s.borrow_mut();
            let state = state.as_mut().unwrap();
            state.backfill_int_read(handle, key.as_str(), value);
            state
                .object_mut(handle)
                .update(&Key::from(key.to_string()), &Value::from(value))
                .unwrap();
        },
    );

    let s = state.clone();
    engine.register_fn("get_int", move |handle: i64, key: ImmutableString| -> i64 {
        let mut state = s.borrow_mut();
        let state = state.as_mut().unwrap();
        let value = state
            .object(handle)
            .get(&Key::from(key.to_string()))
            .unwrap()
            .unwrap()
            .as_int()
            .unwrap();
        let event_idx = state.events.len();
        state.push_event(RuntimeEvent::Var(Value::from(value)));
        state
            .pending_int_reads
            .insert((handle, key.to_string()), event_idx);
        value
    });

    engine.register_fn("obj_raw", |handle: i64| -> i64 { handle });

    let s = state.clone();
    engine.register_fn("vdf", move |iters: i64, handle: i64| -> ImmutableString {
        let mut state = s.borrow_mut();
        let state = state.as_mut().unwrap();
        let input = Value::from(state.object(handle).clone()).as_raw();
        let (pod, statement, work) = run_vdf(&state.env, iters as usize, input);
        let token = state.insert_token(work.clone());
        state.push_event(RuntimeEvent::Var(work));
        state.push_event(RuntimeEvent::Condition(RuntimeCondition::StoredPod {
            pod,
            statement,
        }));
        token
    });

    let s = state.clone();
    engine.register_fn(
        "pow_grind",
        move |handle: i64, difficulty: i64| -> ImmutableString {
            let mut state = s.borrow_mut();
            let state = state.as_mut().unwrap();
            let mut obj = state.object(handle).clone();
            let mut key = Value::from(rand_raw_value());
            if !state.env.mock {
                while RawValue::from(obj.commitment()).0[3].0 > difficulty as u64 {
                    key = Value::from(rand_raw_value());
                    obj.update(&Key::from("key"), &key).unwrap();
                }
            }
            let token = state.insert_token(key.clone());
            state.push_event(RuntimeEvent::Var(key));
            token
        },
    );

    let s = state.clone();
    engine.register_fn("lt_eq_u256", move |handle: i64, difficulty: i64| {
        let mut state = s.borrow_mut();
        let state = state.as_mut().unwrap();
        let lhs = Value::from(state.object(handle).clone()).as_raw();
        let rhs = RawValue([F(0), F(0), F(0), F(difficulty as u64)]);
        let (pod, statement) = run_lt_eq_u256(&state.env, lhs, rhs);
        state.push_event(RuntimeEvent::Condition(RuntimeCondition::StoredPod {
            pod,
            statement,
        }));
    });

    let s = state.clone();
    engine.register_fn(
        "gt",
        move |handle: i64, key: ImmutableString, value: i64| {
            let mut state = s.borrow_mut();
            let state = state.as_mut().unwrap();
            state.push_event(RuntimeEvent::Condition(RuntimeCondition::Gt {
                object_name: state.object_name(handle).to_string(),
                key: key.to_string(),
                value,
            }));
        },
    );

    let s = state.clone();
    engine.register_fn(
        "sum_of",
        move |handle: i64, key: ImmutableString, value: i64, b: i64| {
            let mut state = s.borrow_mut();
            let state = state.as_mut().unwrap();
            state.backfill_int_read(handle, key.as_str(), value);
            state.push_event(RuntimeEvent::Condition(RuntimeCondition::SumOf {
                object_name: state.object_name(handle).to_string(),
                key: key.to_string(),
                value,
                b,
            }));
        },
    );

    let s = state.clone();
    engine.register_fn("random_key", move |_handle: i64| -> ImmutableString {
        let mut state = s.borrow_mut();
        let state = state.as_mut().unwrap();
        let key = Value::from(rand_raw_value());
        let token = state.insert_token(key.clone());
        state.push_event(RuntimeEvent::Var(key));
        token
    });

    RuntimeEngine { engine, state }
}

fn default_output_dict() -> Dictionary {
    dict!({"work" => EMPTY_VALUE, "key" => Value::from(rand_raw_value())})
}

fn main_pod(env: &RuntimeEnv, pod: Box<dyn Pod>) -> MainPod {
    let public_statements = pod.pub_statements();
    MainPod {
        pod,
        public_statements,
        params: env.params.clone(),
    }
}

fn run_vdf(env: &RuntimeEnv, iters: usize, input: RawValue) -> (MainPod, Statement, Value) {
    let pod = if env.mock {
        VdfPod::new_boxed_mock(&env.params, env.vd_set.clone(), iters, input)
    } else {
        VdfPod::new_boxed(&env.params, env.vd_set.clone(), iters, input)
    }
    .unwrap();
    let statement = pod.pub_statements()[0].clone();
    let work = statement.args()[2].literal().unwrap();
    (main_pod(env, pod), statement, work)
}

fn run_lt_eq_u256(env: &RuntimeEnv, lhs: RawValue, rhs: RawValue) -> (MainPod, Statement) {
    let pod = if env.mock {
        LtEqU256Pod::new_boxed_mock(&env.params, env.vd_set.clone(), lhs, rhs)
    } else {
        LtEqU256Pod::new_boxed(&env.params, env.vd_set.clone(), lhs, rhs)
    }
    .unwrap();
    let statement = pod.pub_statements()[0].clone();
    (main_pod(env, pod), statement)
}
