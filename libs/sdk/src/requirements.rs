//! What an action's own statements demand of each object's fields, read
//! off the Load-time instruction list.
//!
//! The compiled custom predicates are a poor source for this. Lowering
//! splits an action across a chain of helper predicates and renames as it
//! goes, so one object's writes scatter under several local names and
//! repeated slots of one class stop being distinguishable. The `Inst`
//! list is upstream of that: it still names each object variable and each
//! field as the script wrote them, so nothing has to be recovered.

use std::collections::{BTreeSet, HashMap};
use std::rc::Rc;

use pod2::middleware::NativePredicate;

use crate::{ActionContext, Inst, Ref, Var, VarOrValue, arg_is_int};

/// What an action's statements say about one field of one object.
///
/// Facts only. Choosing a value that satisfies them, and reporting when
/// none can, belongs to whoever is building the value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldFacts {
    /// Values the statements pin the field to. More than one is a
    /// contradiction no input can satisfy.
    pub pinned: BTreeSet<i64>,
    /// Greatest lower bound the statements place on the field.
    pub min: Option<i64>,
    /// Set when the statements force other fields to hold the same
    /// value; every member of one equality group carries the same id.
    pub group: Option<String>,
    /// The field appears where an integer is required.
    pub integer: bool,
}

impl FieldFacts {
    fn pin(&mut self, value: i64) {
        self.pinned.insert(value);
    }
    fn floor(&mut self, min: i64) {
        self.min = Some(self.min.map_or(min, |cur| cur.max(min)));
    }
    fn absorb(&mut self, other: &Self) {
        self.pinned.extend(&other.pinned);
        if let Some(m) = other.min {
            self.floor(m);
        }
        self.integer |= other.integer;
    }
}

/// Per-object field facts, sorted by field name.
pub(crate) type ObjectFacts = Rc<[(Box<str>, FieldFacts)]>;

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
pub(crate) fn object_facts(action: &str, ctx: &ActionContext) -> HashMap<String, ObjectFacts> {
    let mut objects: Vec<String> = Vec::new();
    let mut touched: HashMap<String, BTreeSet<String>> = HashMap::new();
    let mut facts: HashMap<FieldKey, FieldFacts> = HashMap::new();
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
                    match value {
                        Term::Int(v) => facts.entry(target).or_default().pin(v),
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
                touch(&mut touched, &term(value));
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

    let mut out: HashMap<String, ObjectFacts> = HashMap::with_capacity(objects.len());
    for var in objects {
        let mut fields: Vec<(Box<str>, FieldFacts)> = touched
            .get(&var)
            .into_iter()
            .flatten()
            .map(|field| {
                let key = (var.clone(), field.clone());
                let root = root_of.get(&key).cloned().unwrap_or_else(|| key.clone());
                let mut resolved = groups.get(&root).cloned().unwrap_or_default();
                if members.get(&root).copied().unwrap_or(1) > 1 {
                    resolved.group = Some(format!("{action}:{}.{}", root.0, root.1));
                }
                (field.as_str().into(), resolved)
            })
            .collect();
        fields.sort_by(|a, b| a.0.cmp(&b.0));
        out.insert(var, fields.into());
    }
    out
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
    let mut pin = |v: &String, f: &String, value: i64| {
        facts.entry((v.clone(), f.clone())).or_default().pin(value);
    };
    match (pred, terms) {
        // `Sum(v0, v1, v2)` asserts `v2 = v0 + v1`.
        (Sum, [Term::Field(v, f), Term::Int(b), Term::Int(c)]) => pin(v, f, c - b),
        (Sum, [Term::Int(a), Term::Field(v, f), Term::Int(c)]) => pin(v, f, c - a),
        (Sum, [Term::Int(a), Term::Int(b), Term::Field(v, f)]) => pin(v, f, a + b),
        (Equal, [Term::Field(v, f), Term::Int(k)]) | (Equal, [Term::Int(k), Term::Field(v, f)]) => {
            pin(v, f, *k)
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
            Term::Int(v) => pin(o, k, *v),
            Term::Field(v2, f2) => dsu.union(&(o.clone(), k.clone()), &(v2.clone(), f2.clone())),
            _ => {}
        },
        _ => {}
    }
}
