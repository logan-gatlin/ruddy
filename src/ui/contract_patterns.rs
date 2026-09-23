//! Recover lexical pattern names from the semantic projection graph. The names
//! are presentation only: parsing them reconstructs the same projections.

use std::collections::{HashMap, HashSet};

use crate::contracts::{Contract, Expr, Pattern};

// Naming is optional. Bound the discovery work even for recovery graphs that
// have not passed structural validation; the ordinary projection spelling is
// always available when an arm exceeds these presentation limits.
const MAX_PATH: usize = 128;
const MAX_DISCOVERY: usize = 8_192;

#[derive(Clone, Default)]
pub(super) struct Bindings {
    pub nodes: HashMap<usize, usize>,
    patterns: PatternNames,
    next_pattern: u32,
}

pub(super) type PatternNames = HashMap<Projection, String>;

#[derive(Clone, PartialEq, Eq, Hash)]
enum Root {
    Input(usize),
    Capture(usize),
    // Independently evaluated computations must not become the match's value
    // merely because their syntax is alike.
    Computation(usize),
}

#[derive(Clone, PartialEq, Eq, Hash)]
enum Step {
    Field(String),
    Payload(String),
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct Projection {
    root: Root,
    path: Vec<Step>,
}

impl Projection {
    pub fn of(mut expr: &Expr) -> Self {
        let original = expr as *const Expr as usize;
        let mut path = Vec::new();
        let root = loop {
            if path.len() == MAX_PATH && matches!(expr, Expr::Field { .. } | Expr::Payload { .. }) {
                return Self {
                    root: Root::Computation(original),
                    path: Vec::new(),
                };
            }
            match expr {
                Expr::Input(index) => break Root::Input(*index),
                Expr::Capture(index) => break Root::Capture(*index),
                Expr::Field { base, label } => {
                    path.push(Step::Field(label.clone()));
                    expr = base;
                }
                Expr::Payload { base, label } => {
                    path.push(Step::Payload(label.clone()));
                    expr = base;
                }
                _ => break Root::Computation(expr as *const Expr as usize),
            }
        };
        path.reverse();
        Self { root, path }
    }

    pub fn field(&self, label: &str) -> Self {
        let mut projected = self.clone();
        projected.path.push(Step::Field(label.to_owned()));
        projected
    }

    pub fn payload(&self, label: &str) -> Self {
        let mut projected = self.clone();
        projected.path.push(Step::Payload(label.to_owned()));
        projected
    }
}

impl Bindings {
    /// A match function has no name for its implicit input. Only omit that
    /// name when every use is expressible through the arm's pattern binders.
    /// Keep the explicit form if discovery runs out of budget or a branch
    /// needs the whole structured input rather than one of its leaves.
    pub fn match_function(contract: &Contract) -> Option<Vec<(Self, PatternNames)>> {
        let input = contract.arguments.len();
        if contract.parameters.checked_sub(input) != Some(1) {
            return None;
        }
        let Expr::Match { scrutinee, arms } = &*contract.body else {
            return None;
        };
        if !matches!(&**scrutinee, Expr::Input(index) if *index == input) || arms.is_empty() {
            return None;
        }
        let inherited = Self::default();
        let mut remaining = MAX_DISCOVERY;
        let mut scopes = Vec::new();
        for arm in arms.iter() {
            let (bindings, names) = inherited.arm(scrutinee, &arm.pattern, &arm.body);
            let mut pending = vec![&*arm.body];
            let mut seen = HashSet::new();
            while let Some(expr) = pending.pop() {
                if !seen.insert(expr as *const Expr as usize) {
                    continue;
                }
                if remaining == 0 {
                    return None;
                }
                remaining -= 1;
                if bindings.pattern_name(expr).is_some() {
                    continue;
                }
                if matches!(expr, Expr::Input(index) if *index == input) {
                    return None;
                }
                let mut children = Vec::new();
                expr.children(&mut children);
                pending.extend(children.into_iter().map(|child| &**child));
            }
            scopes.push((bindings, names));
        }
        Some(scopes)
    }

    pub fn pattern_name(&self, expr: &Expr) -> Option<&str> {
        if self.patterns.is_empty() {
            return None;
        }
        self.patterns.get(&Projection::of(expr)).map(String::as_str)
    }

    pub fn contains(&self, expr: &Expr) -> bool {
        self.nodes.contains_key(&(expr as *const Expr as usize))
            || self.pattern_name(expr).is_some()
    }

    /// Only unconstrained pattern leaves can become binders without replacing
    /// a structural test. Names belong to this arm; sibling arms start again
    /// from their inherited scope, while nested arms reserve outer names.
    pub fn arm(&self, scrutinee: &Expr, pattern: &Pattern, body: &Expr) -> (Self, PatternNames) {
        let mut leaves = Vec::new();
        let mut pending = vec![(pattern, Projection::of(scrutinee))];
        let mut remaining = MAX_DISCOVERY;
        while let Some((pattern, projection)) = pending.pop() {
            if remaining == 0 || projection.path.len() > MAX_PATH {
                return (self.clone(), HashMap::new());
            }
            remaining -= 1;
            match pattern {
                Pattern::Any => leaves.push(projection),
                Pattern::Tag { label, payload } => {
                    pending.push((payload, projection.payload(label)));
                }
                Pattern::Record { fields, open } => {
                    if fields.len() > remaining {
                        return (self.clone(), HashMap::new());
                    }
                    let order = (!open)
                        .then(|| {
                            super::tuple_field_order(fields.iter().map(|(name, _)| name.as_str()))
                        })
                        .flatten()
                        .unwrap_or_else(|| (0..fields.len()).collect());
                    for index in order.into_iter().rev() {
                        let (label, pattern) = &fields[index];
                        pending.push((pattern, projection.field(label)));
                    }
                }
            }
        }

        if leaves.is_empty() {
            return (self.clone(), HashMap::new());
        }

        let candidates: HashSet<_> = leaves.iter().collect();
        let mut used = HashSet::new();
        let mut seen = HashSet::new();
        let mut expressions = vec![body];
        while let Some(expr) = expressions.pop() {
            if !seen.insert(expr as *const Expr as usize) {
                continue;
            }
            if remaining == 0 {
                return (self.clone(), HashMap::new());
            }
            remaining -= 1;
            let projection = Projection::of(expr);
            if candidates.contains(&projection) {
                used.insert(projection);
            }
            // Outer pattern names can be used inside nested matches. This
            // discovery walk may enter their arms; expression factoring still
            // stops at the branch boundary in the main printer.
            let mut children = Vec::new();
            expr.children(&mut children);
            expressions.extend(children.into_iter().map(|child| &**child));
        }

        let mut bindings = self.clone();
        let mut names = HashMap::new();
        for projection in leaves {
            if !used.contains(&projection) || bindings.patterns.contains_key(&projection) {
                continue;
            }
            let name = super::name_at(bindings.next_pattern);
            bindings.next_pattern += 1;
            bindings.patterns.insert(projection.clone(), name.clone());
            names.insert(projection, name);
        }
        (bindings, names)
    }
}
