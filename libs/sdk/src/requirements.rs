//! What an action's own statements demand of each object's fields, read
//! off the Load-time instruction list.
//!
//! The compiled custom predicates are a poor source for this. Lowering
//! splits an action across a chain of helper predicates and renames as it
//! goes, so one object's writes scatter under several local names and
//! repeated slots of one class stop being distinguishable. The `Inst`
//! list is upstream of that: it still names each object variable and each
//! field as the script wrote them, so nothing has to be recovered.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::rc::Rc;

use pod2::middleware::{NativePredicate, Value};

use crate::{ActionContext, Inst, Intro, Ref, Var, VarOrValue, arg_is_int};

/// A literal an action ties a field to.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pin {
    Int(i64),
    Text(String),
}

impl fmt::Display for Pin {
    /// As a plugin author would have written it. Goes through pod2's
    /// value formatting so a string with a quote in it stays readable.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(v) => Value::from(*v).fmt(f),
            Self::Text(t) => Value::from(t.as_str()).fmt(f),
        }
    }
}

/// The crypto an action applies to an object's identity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ObjectIdentity {
    /// A VDF intro constrains the object or a field of it.
    pub vdf: bool,
    /// An lt_eq_u256 intro constrains the object or a field of it, which
    /// is how a script gates minting on proof of work.
    pub proof_of_work: bool,
}

impl ObjectIdentity {
    pub fn is_constrained(&self) -> bool {
        self.vdf || self.proof_of_work
    }
    /// Fold in another action's view of the same object.
    pub fn absorb(&mut self, other: Self) {
        self.vdf |= other.vdf;
        self.proof_of_work |= other.proof_of_work;
    }
}

/// What an action's statements say about one field of one object.
///
/// Facts only. Choosing a value that satisfies them, and reporting when
/// none can, belongs to whoever is building the value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldFacts {
    /// Values the statements pin the field to. More than one is a
    /// contradiction no input can satisfy.
    pub pinned: BTreeSet<Pin>,
    /// Greatest lower bound the statements place on the field.
    pub min: Option<i64>,
    /// Set when the statements force other fields to hold the same
    /// value; every member of one equality group carries the same id.
    pub group: Option<String>,
    /// The field appears where an integer is required.
    pub integer: bool,
}

impl FieldFacts {
    fn pin(&mut self, value: Pin) {
        self.pinned.insert(value);
    }
    fn floor(&mut self, min: i64) {
        self.min = Some(self.min.map_or(min, |cur| cur.max(min)));
    }
    /// Merge facts the statements force to be equal. Only values belong
    /// here: an equality says two fields hold the same value, so a
    /// requirement on one is a requirement on both.
    fn absorb(&mut self, other: &Self) {
        self.pinned.extend(other.pinned.iter().cloned());
        if let Some(m) = other.min {
            self.floor(m);
        }
        self.integer |= other.integer;
    }
}

/// What an action puts into one field of the object it leaves behind.
///
/// Deliberately separate from [`FieldFacts`]: an equality between two
/// fields says they hold the same value, which is a fact about the value
/// and so propagates, whereas a write lands in one named slot and does
/// not. Folding these through the equality groups would attribute one
/// field's write to every field equal to it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldWrites {
    /// Literals the action writes into the field.
    pub values: BTreeSet<Pin>,
    /// The field is written from the output of a VDF intro.
    pub from_vdf: bool,
}

impl FieldWrites {
    fn write(&mut self, value: Pin) {
        self.values.insert(value);
    }
    /// Fold in another action's writes to the same field of the same class.
    pub fn absorb(&mut self, other: &Self) {
        self.values.extend(other.values.iter().cloned());
        self.from_vdf |= other.from_vdf;
    }
}

/// One field of one object: what the action requires of the state it
/// consumes, and what it writes into the state it leaves behind.
#[derive(Debug)]
pub struct FieldEntry {
    pub name: Box<str>,
    pub facts: FieldFacts,
    pub writes: FieldWrites,
}

/// Everything the walk learns about one object, sorted by field name.
pub(crate) type ObjectFields = Rc<[FieldEntry]>;

pub(crate) struct ObjectRequirements {
    pub fields: ObjectFields,
    pub identity: ObjectIdentity,
}

/// A statement argument, resolved as far as Load-time data allows.
enum Term {
    /// `<object or local>.<field>`.
    Field(String, String),
    Int(i64),
    Str(String),
    /// A plain script variable: an `unsafe {}` result, an intro output,
    /// or a whole object.
    Local(String),
    Opaque,
}

fn term(r: &Ref) -> Term {
    match &*r.borrow() {
        VarOrValue::Value(v) => {
            if let Some(i) = v.as_int() {
                Term::Int(i)
            } else if let Some(s) = v.as_string() {
                Term::Str(s)
            } else {
                Term::Opaque
            }
        }
        VarOrValue::Var(Var {
            name, key: Some(k), ..
        }) => Term::Field(name.clone(), k.clone()),
        VarOrValue::Var(Var {
            name, key: None, ..
        }) => Term::Local(name.clone()),
    }
}

type FieldKey = (String, String);

/// Union-find over `(object, field)` pairs that statements force to be
/// equal.
#[derive(Default)]
struct Dsu {
    parent: HashMap<FieldKey, FieldKey>,
}

impl Dsu {
    fn find(&mut self, key: &FieldKey) -> FieldKey {
        let mut cur = key.clone();
        loop {
            let Some(up) = self.parent.get(&cur) else {
                self.parent.insert(cur.clone(), cur.clone());
                return cur;
            };
            if *up == cur {
                return cur;
            }
            let up = up.clone();
            // Path halving: point at the grandparent as we walk, so a
            // long equality chain flattens instead of being rewalked.
            if let Some(grand) = self.parent.get(&up).cloned() {
                self.parent.insert(cur.clone(), grand);
            }
            cur = up;
        }
    }
    fn union(&mut self, a: &FieldKey, b: &FieldKey) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            // Keep the smaller root so the group id is stable across runs.
            let (keep, drop) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.parent.insert(drop, keep);
        }
    }
}

/// Facts about every object the action declares, keyed by script-side
/// variable name. `action` namespaces the group ids so that splicing a
/// sub-action's facts into a caller cannot collide.
pub(crate) fn object_facts(
    action: &str,
    ctx: &ActionContext,
) -> HashMap<String, ObjectRequirements> {
    let mut intros = IntroFacts::collect(ctx);

    let mut objects: Vec<String> = Vec::new();
    let mut touched: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut facts: HashMap<FieldKey, FieldFacts> = HashMap::new();
    let mut writes: HashMap<FieldKey, FieldWrites> = HashMap::new();
    let mut dsu = Dsu::default();
    // Statements are replayed after the walk, because a bound on a field
    // is only known once every bound on the locals feeding it is.
    let mut statements: Vec<(NativePredicate, Vec<Term>)> = Vec::new();
    let mut local_min: HashMap<String, i64> = HashMap::new();

    let touch = |touched: &mut HashMap<String, BTreeSet<String>>, t: &Term| {
        if let Term::Field(var, field) = t {
            touched
                .entry(var.clone())
                .or_default()
                .insert(field.clone());
        }
    };

    for inst in &ctx.insts {
        match inst {
            Inst::Object { obj, .. } => objects.push(obj.borrow().var_name().to_string()),
            Inst::Set { obj, kvs, .. } => {
                for (key, value) in kvs {
                    touched.entry(obj.clone()).or_default().insert(key.clone());
                    let value = term(value);
                    touch(&mut touched, &value);
                    // Only an Output can be `set`, so this is never an
                    // input's own value; it matters because an equality
                    // can carry it to one.
                    let target = (obj.clone(), key.clone());
                    intros.note_write(&mut writes, &target, &value);
                    // A `set` literal is also a requirement, because an
                    // equality can carry it to a field of an input.
                    match value {
                        Term::Int(v) => facts.entry(target).or_default().pin(Pin::Int(v)),
                        Term::Str(t) => facts.entry(target).or_default().pin(Pin::Text(t)),
                        Term::Field(v, f) => dsu.union(&target, &(v, f)),
                        _ => {}
                    }
                }
            }
            Inst::Update {
                obj, key, value, ..
            } => {
                // The written value is the object's next state, not a
                // constraint on the one the caller supplies.
                touched.entry(obj.clone()).or_default().insert(key.clone());
                let value = term(value);
                touch(&mut touched, &value);
                intros.note_write(&mut writes, &(obj.clone(), key.clone()), &value);
            }
            Inst::Statement { pred, args } => {
                let terms: Vec<Term> = args.iter().map(term).collect();
                for (i, t) in terms.iter().enumerate() {
                    touch(&mut touched, t);
                    if let (Term::Field(var, field), true) = (t, arg_is_int(*pred, i)) {
                        facts
                            .entry((var.clone(), field.clone()))
                            .or_default()
                            .integer = true;
                    }
                }
                if let Some((name, min)) = local_floor(*pred, &terms) {
                    let slot = local_min.entry(name).or_insert(min);
                    *slot = (*slot).max(min);
                }
                statements.push((*pred, terms));
            }
            Inst::Intro { args, .. } => {
                for t in args.iter().map(term) {
                    touch(&mut touched, &t);
                }
            }
            Inst::SubAction { .. } => {}
        }
    }

    for (pred, terms) in &statements {
        apply(*pred, terms, &local_min, &mut facts, &mut dsu);
    }

    // Fold each equality group into one set of facts, then hand every
    // member the same answer.
    let mut groups: HashMap<FieldKey, FieldFacts> = HashMap::new();
    let mut members: HashMap<FieldKey, usize> = HashMap::new();
    let keys: Vec<FieldKey> = facts
        .keys()
        .cloned()
        .chain(dsu.parent.keys().cloned())
        .chain(
            touched
                .iter()
                .flat_map(|(v, fs)| fs.iter().map(|f| (v.clone(), f.clone()))),
        )
        .collect();
    let mut root_of: HashMap<FieldKey, FieldKey> = HashMap::with_capacity(keys.len());
    for key in keys {
        let root = dsu.find(&key);
        if root_of.insert(key.clone(), root.clone()).is_some() {
            continue;
        }
        *members.entry(root.clone()).or_default() += 1;
        if let Some(f) = facts.get(&key) {
            groups.entry(root).or_default().absorb(f);
        } else {
            groups.entry(root).or_default();
        }
    }

    let mut out: HashMap<String, ObjectRequirements> = HashMap::with_capacity(objects.len());
    for var in objects {
        let mut fields: Vec<FieldEntry> = touched
            .get(&var)
            .into_iter()
            .flatten()
            .map(|field| {
                let key = (var.clone(), field.clone());
                let root = root_of.get(&key).cloned().unwrap_or_else(|| key.clone());
                let mut facts = groups.get(&root).cloned().unwrap_or_default();
                if members.get(&root).copied().unwrap_or(1) > 1 {
                    facts.group = Some(format!("{action}:{}.{}", root.0, root.1));
                }
                FieldEntry {
                    name: field.as_str().into(),
                    facts,
                    writes: writes.get(&key).cloned().unwrap_or_default(),
                }
            })
            .collect();
        fields.sort_by(|a, b| a.name.cmp(&b.name));
        let identity = intros.identity.get(&var).copied().unwrap_or_default();
        out.insert(
            var,
            ObjectRequirements {
                fields: fields.into(),
                identity,
            },
        );
    }
    out
}

/// What the action's intro calls say about its objects, gathered before
/// the main walk.
///
/// The pre-pass is load-bearing for `pow_values`: `intro_lt_eq_u256`
/// takes refs that already exist, so a script may write an intro-bounded
/// value into a field before the intro itself appears in the list.
#[derive(Default)]
struct IntroFacts {
    /// Locals holding a VDF's output.
    vdf_outputs: HashSet<String>,
    /// Locals an lt_eq_u256 bounds, which gate identity once written in.
    pow_values: HashSet<String>,
    identity: HashMap<String, ObjectIdentity>,
}

impl IntroFacts {
    fn collect(ctx: &ActionContext) -> Self {
        let mut out = Self::default();
        for inst in &ctx.insts {
            let Inst::Intro { pred, args, .. } = inst else {
                continue;
            };
            let local = |i: usize| match args.get(i).map(term) {
                Some(Term::Local(name)) => Some(name),
                _ => Option::None,
            };
            for subject in pred.subject_args().iter().filter_map(|i| local(*i)) {
                let entry = out.identity.entry(subject.clone()).or_default();
                match pred {
                    Intro::Vdf => entry.vdf = true,
                    Intro::LtEqU256 => {
                        entry.proof_of_work = true;
                        // The same arg may name a value bound for a field
                        // rather than the object itself.
                        out.pow_values.insert(subject);
                    }
                }
            }
            if let Some(output) = pred.output_arg().and_then(local) {
                out.vdf_outputs.insert(output);
            }
        }
        out
    }

    /// Record a write, and the identity it constrains when the value came
    /// from an intro. A script gates an object's identity by writing the
    /// intro's result into it, so the two are one observation.
    fn note_write(
        &mut self,
        writes: &mut HashMap<FieldKey, FieldWrites>,
        target: &FieldKey,
        value: &Term,
    ) {
        let slot = writes.entry(target.clone()).or_default();
        match value {
            Term::Int(v) => slot.write(Pin::Int(*v)),
            Term::Str(t) => slot.write(Pin::Text(t.clone())),
            Term::Local(name) => {
                if self.vdf_outputs.contains(name) {
                    slot.from_vdf = true;
                    self.identity.entry(target.0.clone()).or_default().vdf = true;
                }
                if self.pow_values.contains(name) {
                    self.identity
                        .entry(target.0.clone())
                        .or_default()
                        .proof_of_work = true;
                }
            }
            _ => {}
        }
    }
}

/// The floor a comparison puts on a plain local, so the
/// `remainder = field - k` idiom can become a bound on the field.
/// `Gt(v0, v1)` reads `v0 > v1`.
fn local_floor(pred: NativePredicate, terms: &[Term]) -> Option<(String, i64)> {
    use NativePredicate::*;
    match (pred, terms) {
        (Gt, [Term::Local(l), Term::Int(m)]) | (Lt, [Term::Int(m), Term::Local(l)]) => {
            Some((l.clone(), m + 1))
        }
        (GtEq, [Term::Local(l), Term::Int(m)]) | (LtEq, [Term::Int(m), Term::Local(l)]) => {
            Some((l.clone(), *m))
        }
        // Qualified because the glob import above brings a `None`
        // predicate variant into scope.
        _ => Option::None,
    }
}

/// Record what one statement says about the fields it mentions.
fn apply(
    pred: NativePredicate,
    terms: &[Term],
    local_min: &HashMap<String, i64>,
    facts: &mut HashMap<FieldKey, FieldFacts>,
    dsu: &mut Dsu,
) {
    use NativePredicate::*;
    let mut pin = |v: &String, f: &String, value: Pin| {
        facts.entry((v.clone(), f.clone())).or_default().pin(value);
    };
    match (pred, terms) {
        // `Sum(v0, v1, v2)` asserts `v2 = v0 + v1`.
        (Sum, [Term::Field(v, f), Term::Int(b), Term::Int(c)]) => pin(v, f, Pin::Int(c - b)),
        (Sum, [Term::Int(a), Term::Field(v, f), Term::Int(c)]) => pin(v, f, Pin::Int(c - a)),
        (Sum, [Term::Int(a), Term::Int(b), Term::Field(v, f)]) => pin(v, f, Pin::Int(a + b)),
        (Equal, [Term::Field(v, f), Term::Int(k)]) | (Equal, [Term::Int(k), Term::Field(v, f)]) => {
            pin(v, f, Pin::Int(*k))
        }
        // `field = local + k`, bounded by whatever bounds the local.
        (Sum, [Term::Local(l), Term::Int(k), Term::Field(v, f)])
        | (Sum, [Term::Int(k), Term::Local(l), Term::Field(v, f)]) => {
            if let Some(lo) = local_min.get(l) {
                facts
                    .entry((v.clone(), f.clone()))
                    .or_default()
                    .floor(lo + k);
            }
        }
        // One field is the other plus zero, so the two are equal.
        (Sum, [Term::Field(v1, f1), Term::Int(0), Term::Field(v2, f2)])
        | (Sum, [Term::Int(0), Term::Field(v1, f1), Term::Field(v2, f2)])
        | (Equal, [Term::Field(v1, f1), Term::Field(v2, f2)]) => {
            dsu.union(&(v1.clone(), f1.clone()), &(v2.clone(), f2.clone()))
        }
        // A field compared straight against a literal floor.
        (Gt, [Term::Field(v, f), Term::Int(m)]) | (Lt, [Term::Int(m), Term::Field(v, f)]) => facts
            .entry((v.clone(), f.clone()))
            .or_default()
            .floor(m + 1),
        (GtEq, [Term::Field(v, f), Term::Int(m)]) | (LtEq, [Term::Int(m), Term::Field(v, f)]) => {
            facts.entry((v.clone(), f.clone())).or_default().floor(*m)
        }
        // `DictContains(obj, "f", value)` pins or couples `obj.f`.
        (Contains | DictContains, [Term::Local(o), Term::Str(k), rest]) => match rest {
            Term::Int(v) => pin(o, k, Pin::Int(*v)),
            Term::Str(t) => pin(o, k, Pin::Text(t.clone())),
            Term::Field(v2, f2) => dsu.union(&(o.clone(), k.clone()), &(v2.clone(), f2.clone())),
            _ => {}
        },
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Sdk;

    /// Requirements and writes for one object of one action, by field.
    fn facts_for(src: &str, action: &str, var: &str) -> Vec<(String, FieldFacts, FieldWrites)> {
        let module = Sdk::default()
            .load_module_from_src_actions(src, &[action])
            .unwrap();
        let meta = module.actions().iter().find(|a| a.name == action).unwrap();
        let obj = meta
            .object_refs
            .iter()
            .find(|r| r.varname == var)
            .unwrap_or_else(|| panic!("no object {var} in {action}"));
        obj.fields
            .iter()
            .map(|e| (e.name.to_string(), e.facts.clone(), e.writes.clone()))
            .collect()
    }

    fn field<'a>(
        all: &'a [(String, FieldFacts, FieldWrites)],
        name: &str,
    ) -> &'a (String, FieldFacts, FieldWrites) {
        all.iter()
            .find(|(f, _, _)| f == name)
            .unwrap_or_else(|| panic!("no field {name}"))
    }

    #[test]
    fn statement_literals_become_requirements_not_writes() {
        let all = facts_for(
            r#"
            fn Consume(action) {
                var ore = action.mutate("Ore");
                action.st_sum(ore.grade, 0, 7);
                action.st_gt(ore.depth, 4);
            }
        "#,
            "Consume",
            "ore",
        );
        let (_, grade, grade_writes) = field(&all, "grade");
        assert_eq!(grade.pinned, [Pin::Int(7)].into_iter().collect());
        assert!(grade.integer);
        assert!(grade_writes.values.is_empty(), "a read is not a write");

        let (_, depth, _) = field(&all, "depth");
        assert_eq!(depth.min, Some(5));
        assert!(depth.pinned.is_empty());
    }

    #[test]
    fn set_literals_are_both_written_and_required() {
        let all = facts_for(
            r#"
            fn Mint(action) {
                var badge = action.output("Badge");
                badge.set([["tier", "gold"], ["level", 3]]);
            }
        "#,
            "Mint",
            "badge",
        );
        let (_, tier, tier_writes) = field(&all, "tier");
        let gold = [Pin::Text("gold".to_string())].into_iter().collect();
        assert_eq!(tier_writes.values, gold);
        assert_eq!(tier.pinned, gold, "an equality can carry a set literal");

        let (_, _, level_writes) = field(&all, "level");
        assert_eq!(level_writes.values, [Pin::Int(3)].into_iter().collect());
    }

    #[test]
    fn update_literals_are_written_but_not_required() {
        let all = facts_for(
            r#"
            fn Drain(action) {
                var tank = action.mutate("Tank");
                tank.update("fuel", 0);
            }
        "#,
            "Drain",
            "tank",
        );
        let (_, fuel, fuel_writes) = field(&all, "fuel");
        assert_eq!(fuel_writes.values, [Pin::Int(0)].into_iter().collect());
        assert!(
            fuel.pinned.is_empty(),
            "the next state does not constrain the supplied one"
        );
    }

    /// An equality shares a value between two fields, so a requirement on
    /// one binds both. A write does not: it lands in one named slot.
    #[test]
    fn equality_shares_requirements_but_not_writes() {
        let all = facts_for(
            r#"
            fn Copy(action) {
                var tank = action.mutate("Tank");
                var receipt = action.output("Receipt");
                receipt.set([["seen", tank.fuel]]);
                tank.update("fuel", 0);
            }
        "#,
            "Copy",
            "receipt",
        );
        let (_, seen, seen_writes) = field(&all, "seen");
        assert!(
            seen_writes.values.is_empty(),
            "the write to tank.fuel is not a write to receipt.seen"
        );
        assert!(
            seen.group.is_some(),
            "the two fields are one equality group"
        );
    }

    #[test]
    fn vdf_output_marks_the_field_and_the_identity() {
        let module = Sdk::default()
            .load_module_from_src_actions(
                r#"
            fn Find(action) {
                var log = action.output("Log");
                var work = action.intro_vdf(3, log);
                log.update("work", work);
            }
        "#,
                &["Find"],
            )
            .unwrap();
        let meta = &module.actions()[0];
        let log = &meta.object_refs[0];
        let (_, writes) = log
            .field_writes()
            .find(|(name, _)| *name == "work")
            .expect("work is written");
        assert!(writes.from_vdf);
        assert_eq!(
            log.identity(),
            ObjectIdentity {
                vdf: true,
                proof_of_work: false
            }
        );
    }
}
