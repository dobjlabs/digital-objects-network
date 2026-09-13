//! Static analysis of field constraints and writes extracted from the action's
//! Load-time instruction list.
//!
//! Inspects the unlowered `Inst` list to preserve original variable and field names
//! across helper predicates and sub-actions.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::fmt;
use std::rc::Rc;

use pod2::middleware::{NativePredicate, Value};

use crate::{ActionContext, Inst, Intro, Ref, Var, VarOrValue, arg_is_int};

/// Literal value constraint for a field.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Pin {
    Int(i64),
    Text(String),
}

impl fmt::Display for Pin {
    /// Formats the literal value matching pod2 value representation.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Int(v) => Value::from(*v).fmt(f),
            Self::Text(t) => Value::from(t.as_str()).fmt(f),
        }
    }
}

/// Cryptographic identity constraints applied to an object.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ObjectIdentity {
    /// Constrained by a VDF intro.
    pub vdf: bool,
    /// Constrained by a proof-of-work (`lt_eq_u256`) intro.
    pub proof_of_work: bool,
}

impl ObjectIdentity {
    pub fn is_constrained(&self) -> bool {
        self.vdf || self.proof_of_work
    }
    /// Merges identity constraints from another object reference.
    pub fn absorb(&mut self, other: Self) {
        self.vdf |= other.vdf;
        self.proof_of_work |= other.proof_of_work;
    }
}

/// Constraints and type requirements imposed on a field by an action.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldFacts {
    /// Literal values this field is constrained to equal.
    pub pinned: BTreeSet<Pin>,
    /// Minimum allowed integer value.
    pub min: Option<i64>,
    /// Identifier of the equality group if this field is coupled with other fields.
    pub group: Option<String>,
    /// Whether the field is used in an integer context.
    pub integer: bool,
}

impl FieldFacts {
    fn pin(&mut self, value: Pin) {
        self.pinned.insert(value);
    }
    fn floor(&mut self, min: i64) {
        self.min = Some(self.min.map_or(min, |cur| cur.max(min)));
    }
    /// Merges constraints from an equal field.
    fn absorb(&mut self, other: &Self) {
        self.pinned.extend(other.pinned.iter().cloned());
        if let Some(m) = other.min {
            self.floor(m);
        }
        self.integer |= other.integer;
    }
}

/// Values written to an object field by an action.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldWrites {
    /// Literal values written to the field.
    pub values: BTreeSet<Pin>,
    /// Whether the field is populated from a VDF output.
    pub from_vdf: bool,
}

impl FieldWrites {
    fn write(&mut self, value: Pin) {
        self.values.insert(value);
    }
    /// Merges field write records from another action.
    pub fn absorb(&mut self, other: &Self) {
        self.values.extend(other.values.iter().cloned());
        self.from_vdf |= other.from_vdf;
    }
}

/// Combined constraint requirements and write records for a single field.
#[derive(Debug)]
pub struct FieldEntry {
    pub name: Box<str>,
    pub facts: FieldFacts,
    pub writes: FieldWrites,
}

/// Field entries for an object, sorted by field name.
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

/// Disjoint-set union tracking coupled `(object, field)` equality groups.
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
            // Path halving to compress equality chains.
            if let Some(grand) = self.parent.get(&up).cloned() {
                self.parent.insert(cur.clone(), grand);
            }
            cur = up;
        }
    }
    fn union(&mut self, a: &FieldKey, b: &FieldKey) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            // Preserve the smaller key to ensure deterministic group IDs.
            let (keep, drop) = if ra < rb { (ra, rb) } else { (rb, ra) };
            self.parent.insert(drop, keep);
        }
    }
}

/// Extracts field constraints and writes for all objects declared by `action`.
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
    // Defer statement evaluation until all local variable bounds are collected.
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
                    let target = (obj.clone(), key.clone());
                    intros.note_write(&mut writes, &target, &value);
                    // Record `set` literal as a requirement in case equality propagates it to an input.
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
                // Record write for next state without constraining input.
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

    // Aggregate constraints for each equality group and apply to all members.
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

/// Pre-pass state tracking intro calls and identity constraints.
#[derive(Default)]
struct IntroFacts {
    /// Local variables holding VDF outputs.
    vdf_outputs: HashSet<String>,
    /// Local variables constrained by `lt_eq_u256`.
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

    /// Records a field write and updates identity constraints if the value originated from an intro.
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

/// Extracts lower bounds on local variables from comparison statements.
fn local_floor(pred: NativePredicate, terms: &[Term]) -> Option<(String, i64)> {
    use NativePredicate::*;
    match (pred, terms) {
        (Gt, [Term::Local(l), Term::Int(m)]) | (Lt, [Term::Int(m), Term::Local(l)]) => {
            Some((l.clone(), m + 1))
        }
        (GtEq, [Term::Local(l), Term::Int(m)]) | (LtEq, [Term::Int(m), Term::Local(l)]) => {
            Some((l.clone(), *m))
        }
        // Explicit `Option::None` to disambiguate from `NativePredicate::None`.
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
        // Sum constraint: v2 = v0 + v1.
        (Sum, [Term::Field(v, f), Term::Int(b), Term::Int(c)]) => pin(v, f, Pin::Int(c - b)),
        (Sum, [Term::Int(a), Term::Field(v, f), Term::Int(c)]) => pin(v, f, Pin::Int(c - a)),
        (Sum, [Term::Int(a), Term::Int(b), Term::Field(v, f)]) => pin(v, f, Pin::Int(a + b)),
        (Equal, [Term::Field(v, f), Term::Int(k)]) | (Equal, [Term::Int(k), Term::Field(v, f)]) => {
            pin(v, f, Pin::Int(*k))
        }
        // Lower bound derived from local variable: field = local + k.
        (Sum, [Term::Local(l), Term::Int(k), Term::Field(v, f)])
        | (Sum, [Term::Int(k), Term::Local(l), Term::Field(v, f)]) => {
            if let Some(lo) = local_min.get(l) {
                facts
                    .entry((v.clone(), f.clone()))
                    .or_default()
                    .floor(lo + k);
            }
        }
        // Equality between two fields.
        (Sum, [Term::Field(v1, f1), Term::Int(0), Term::Field(v2, f2)])
        | (Sum, [Term::Int(0), Term::Field(v1, f1), Term::Field(v2, f2)])
        | (Equal, [Term::Field(v1, f1), Term::Field(v2, f2)]) => {
            dsu.union(&(v1.clone(), f1.clone()), &(v2.clone(), f2.clone()))
        }
        // Direct lower bound comparison.
        (Gt, [Term::Field(v, f), Term::Int(m)]) | (Lt, [Term::Int(m), Term::Field(v, f)]) => facts
            .entry((v.clone(), f.clone()))
            .or_default()
            .floor(m + 1),
        (GtEq, [Term::Field(v, f), Term::Int(m)]) | (LtEq, [Term::Int(m), Term::Field(v, f)]) => {
            facts.entry((v.clone(), f.clone())).or_default().floor(*m)
        }
        // Dictionary contains constraint.
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

    /// Returns field constraints and writes for a given action and object variable.
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

    /// Verifies that equality propagates field constraints without propagating writes.
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
