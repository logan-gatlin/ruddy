//! Finite structural summaries of functions.
//!
//! A summary is built by following the written term once. Applications keep
//! their callee as a type operand; synthesis never follows a value declaration
//! or unfolds a recursive call. Expressions contain no semantic types: captures
//! and supplied arguments are the complete substitution boundary.

pub(crate) mod representation;

use std::{collections::HashMap, fmt, sync::Arc};

use crate::{
    ir,
    symbol::Symbol,
    types::{Presence, Rest, Row, Ty},
};

mod graph;
pub(crate) use graph::check_type_graph;

/// A curried structural function, possibly supplied with some arguments.
#[derive(Clone)]
pub struct Contract {
    pub parameters: usize,
    pub body: Arc<Expr>,
    pub captures: Arc<[Arc<Ty>]>,
    pub arguments: Arc<[Arc<Ty>]>,
    /// The invocation's effect destination, packed as `() -> () + E`.
    /// Retained by a deferred call or supplied by a structural annotation.
    pub effect_sink: Option<Arc<Ty>>,
}

/// A structural operation. Types occur only in the enclosing contract.
#[derive(Clone, Eq)]
pub enum Expr {
    Input(usize),
    Capture(usize),
    Field {
        base: Arc<Expr>,
        label: String,
    },
    Payload {
        base: Arc<Expr>,
        label: String,
    },
    Record {
        fields: Arc<[(String, Arc<Expr>)]>,
        spread: Option<Arc<Expr>>,
    },
    Tag {
        label: String,
        payload: Arc<Expr>,
    },
    Apply {
        function: Arc<Expr>,
        argument: Arc<Expr>,
    },
    Match {
        scrutinee: Arc<Expr>,
        arms: Arc<[Arm]>,
    },
    /// Evaluate an initializer's requirements even when its value is unused.
    Then {
        value: Arc<Expr>,
        body: Arc<Expr>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Arm {
    pub pattern: Pattern,
    pub body: Arc<Expr>,
}

/// Tests only runtime structure, never arbitrary scalar values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pattern {
    Any,
    Record {
        fields: Arc<[(String, Pattern)]>,
        open: bool,
    },
    Tag {
        label: String,
        payload: Box<Pattern>,
    },
}

impl Contract {
    /// Infer a summary from the outer lambda spine and its structural body.
    /// Unsupported terms return `None`; their ordinary typing is unchanged.
    pub fn synthesize(term: &ir::Term) -> Option<Self> {
        synthesize(term)
    }

    /// The only semantic types inside this contract, in substitution order.
    pub fn type_operands(&self) -> impl DoubleEndedIterator<Item = &Arc<Ty>> {
        self.captures
            .iter()
            .chain(self.arguments.iter())
            .chain(self.effect_sink.iter())
    }

    /// Transform captures and supplied arguments without rewriting the DAG.
    pub fn map_types(&self, mut map: impl FnMut(&Arc<Ty>) -> Arc<Ty>) -> Self {
        Self {
            parameters: self.parameters,
            body: self.body.clone(),
            captures: self.captures.iter().map(&mut map).collect(),
            arguments: self.arguments.iter().map(&mut map).collect(),
            effect_sink: self.effect_sink.as_ref().map(map),
        }
    }

    pub fn remaining_parameters(&self) -> usize {
        self.parameters.saturating_sub(self.arguments.len())
    }

    /// Explicit `match | A => B ... end` annotation obligations. The compiler
    /// checks each complete arrow with annotation skolems; checking only the
    /// representation fallback would not establish the promised contract.
    pub fn annotation_cases(&self) -> Option<Vec<(Arc<Ty>, Arc<Ty>)>> {
        if self.parameters != 1 || !self.arguments.is_empty() || self.effect_sink.is_some() {
            return None;
        }
        let Expr::Match { scrutinee, arms } = &*self.body else {
            return None;
        };
        if !matches!(&**scrutinee, Expr::Input(0)) {
            return None;
        }
        arms.iter()
            .map(|arm| {
                let Expr::Apply { function, argument } = &*arm.body else {
                    return None;
                };
                if !matches!(&**argument, Expr::Input(0)) {
                    return None;
                }
                let Expr::Capture(index) = &**function else {
                    return None;
                };
                let Ty::Arrow(from, to, effects) = &**self.captures.get(*index)? else {
                    return None;
                };
                (effects.labels.is_empty()
                    && matches!(effects.rest, Rest::Closed)
                    && arm.pattern == annotation_pattern(from))
                .then(|| (from.clone(), to.clone()))
            })
            .collect()
    }

    /// Check references and the finite resource bounds at a trust boundary.
    /// This visits shared nodes once and does not expand the represented DAG.
    pub fn validate(&self) -> bool {
        if self.parameters > MAX_NODES
            || self.captures.len() > MAX_NODES
            || self.arguments.len() > self.parameters
        {
            return false;
        }
        if self
            .effect_sink
            .as_ref()
            .is_some_and(|sink| !matches!(&**sink, Ty::Arrow(..)))
        {
            return false;
        }
        let mut heights = HashMap::new();
        let mut pending = vec![(&self.body, false)];
        let mut steps = 0usize;
        let mut pattern_budget = MAX_NODES;
        while let Some((expr, finish)) = pending.pop() {
            steps += 1;
            let key = Arc::as_ptr(expr) as usize;
            if heights.contains_key(&key) {
                continue;
            }
            if steps > MAX_NODES * 8 || heights.len() >= MAX_NODES {
                return false;
            }
            let mut children = Vec::new();
            expr.children(&mut children);
            if finish {
                let height = children
                    .iter()
                    .map(|child| heights[&(Arc::as_ptr(child) as usize)] + 1)
                    .max()
                    .unwrap_or(0);
                if height > MAX_DEPTH {
                    return false;
                }
                heights.insert(key, height);
                continue;
            }
            match &**expr {
                Expr::Input(index) if *index >= self.parameters => return false,
                Expr::Capture(index) if *index >= self.captures.len() => return false,
                Expr::Match { arms, .. } => {
                    if arms.is_empty()
                        || arms
                            .iter()
                            .any(|arm| !arm.pattern.validate_with_budget(&mut pattern_budget))
                    {
                        return false;
                    }
                }
                Expr::Record { fields, .. } => {
                    let mut labels = std::collections::HashSet::new();
                    if fields.iter().any(|(label, _)| !labels.insert(label)) {
                        return false;
                    }
                }
                _ => {}
            }
            pending.push((expr, true));
            pending.extend(children.into_iter().map(|child| (child, false)));
        }
        true
    }
}

/// A structural discriminator for an annotation arm. This deliberately
/// over-approximates its domain; the captured ordinary arrow checks the full
/// input type after selecting the arm. No scalar or alias equality is guessed.
pub fn annotation_pattern(ty: &Arc<Ty>) -> Pattern {
    fn pattern(ty: &Ty, depth: usize, remaining: &mut usize) -> Option<Pattern> {
        if depth > MAX_DEPTH {
            return None;
        }
        *remaining = remaining.checked_sub(1)?;
        Some(match ty {
            Ty::Struct(row) => Pattern::Record {
                fields: row
                    .labels
                    .iter()
                    .filter(|(_, field)| matches!(field.presence, Presence::Present))
                    .map(|(label, field)| {
                        Some((label.clone(), pattern(&field.ty, depth + 1, remaining)?))
                    })
                    .collect::<Option<Vec<_>>>()?
                    .into(),
                open: !matches!(row.rest, Rest::Closed)
                    || row.labels.values().any(|field| {
                        !matches!(field.presence, Presence::Present | Presence::Absent)
                    }),
            },
            Ty::Sum(row)
                if matches!(row.rest, Rest::Closed)
                    && row
                        .labels
                        .values()
                        .filter(|field| !matches!(field.presence, Presence::Absent))
                        .count()
                        == 1 =>
            {
                let (label, field) = row
                    .labels
                    .iter()
                    .find(|(_, field)| !matches!(field.presence, Presence::Absent))
                    .expect("the row has one possible case");
                Pattern::Tag {
                    label: label.clone(),
                    payload: Box::new(pattern(&field.ty, depth + 1, remaining)?),
                }
            }
            _ => Pattern::Any,
        })
    }
    let mut remaining = MAX_NODES;
    pattern(ty, 0, &mut remaining).unwrap_or(Pattern::Any)
}

impl Expr {
    /// Structural equality without unfolding shared subexpressions repeatedly.
    pub fn same_structure(left: &Arc<Self>, right: &Arc<Self>) -> bool {
        left.as_ref() == right.as_ref()
    }

    /// Append direct children in source order, without following type operands.
    pub fn children<'a>(&'a self, children: &mut Vec<&'a Arc<Expr>>) {
        match self {
            Self::Input(_) | Self::Capture(_) => {}
            Self::Field { base, .. } | Self::Payload { base, .. } => children.push(base),
            Self::Record { fields, spread } => {
                children.extend(fields.iter().map(|(_, value)| value));
                children.extend(spread.iter());
            }
            Self::Tag { payload, .. } => children.push(payload),
            Self::Apply { function, argument } => {
                children.push(function);
                children.push(argument);
            }
            Self::Match { scrutinee, arms } => {
                children.push(scrutinee);
                children.extend(arms.iter().map(|arm| &arm.body));
            }
            Self::Then { value, body } => {
                children.push(value);
                children.push(body);
            }
        }
    }
}

impl PartialEq for Expr {
    fn eq(&self, other: &Self) -> bool {
        let mut pending: Vec<(&Self, &Self)> = vec![(self, other)];
        let mut seen = std::collections::HashSet::new();
        while let Some((left, right)) = pending.pop() {
            if std::ptr::eq(left, right)
                || !seen.insert((left as *const Self as usize, right as *const Self as usize))
            {
                continue;
            }
            match (left, right) {
                (Self::Input(a), Self::Input(b)) | (Self::Capture(a), Self::Capture(b))
                    if a == b => {}
                (Self::Field { base: a, label: x }, Self::Field { base: b, label: y })
                | (Self::Payload { base: a, label: x }, Self::Payload { base: b, label: y })
                    if x == y =>
                {
                    pending.push((a, b))
                }
                (
                    Self::Record {
                        fields: a,
                        spread: x,
                    },
                    Self::Record {
                        fields: b,
                        spread: y,
                    },
                ) if a.len() == b.len() => {
                    match (x, y) {
                        (Some(x), Some(y)) => pending.push((x, y)),
                        (None, None) => {}
                        _ => return false,
                    }
                    for ((x, a), (y, b)) in a.iter().zip(b.iter()) {
                        if x != y {
                            return false;
                        }
                        pending.push((a, b));
                    }
                }
                (
                    Self::Tag {
                        label: x,
                        payload: a,
                    },
                    Self::Tag {
                        label: y,
                        payload: b,
                    },
                ) if x == y => pending.push((a, b)),
                (
                    Self::Apply {
                        function: a,
                        argument: x,
                    },
                    Self::Apply {
                        function: b,
                        argument: y,
                    },
                ) => {
                    pending.push((a, b));
                    pending.push((x, y));
                }
                (Self::Then { value: a, body: x }, Self::Then { value: b, body: y }) => {
                    pending.push((a, b));
                    pending.push((x, y));
                }
                (
                    Self::Match {
                        scrutinee: a,
                        arms: x,
                    },
                    Self::Match {
                        scrutinee: b,
                        arms: y,
                    },
                ) if x.len() == y.len() => {
                    pending.push((a, b));
                    for (x, y) in x.iter().zip(y.iter()) {
                        if x.pattern != y.pattern {
                            return false;
                        }
                        pending.push((&x.body, &y.body));
                    }
                }
                _ => return false,
            }
        }
        true
    }
}

impl Pattern {
    pub fn validate(&self) -> bool {
        let mut budget = MAX_NODES;
        self.validate_with_budget(&mut budget)
    }

    fn validate_with_budget(&self, budget: &mut usize) -> bool {
        let mut pending = vec![(self, 0)];
        while let Some((pattern, depth)) = pending.pop() {
            let Some(remaining) = budget.checked_sub(1) else {
                return false;
            };
            *budget = remaining;
            if depth > MAX_DEPTH {
                return false;
            }
            match pattern {
                Self::Any => {}
                Self::Record { fields, .. } => {
                    let mut labels = std::collections::HashSet::new();
                    for (label, pattern) in fields.iter() {
                        if !labels.insert(label) {
                            return false;
                        }
                        pending.push((pattern, depth + 1));
                    }
                }
                Self::Tag { payload, .. } => pending.push((payload, depth + 1)),
            }
        }
        true
    }
}

impl fmt::Debug for Contract {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Type walkers own the capture graph. Recursively debugging every
        // capture here can turn a shared library signature into an enormous
        // tree before a diagnostic or fingerprint even reaches its consumer.
        f.debug_struct("Contract")
            .field("parameters", &self.parameters)
            .field("body", &self.body)
            .field("captures", &self.captures.len())
            .field("arguments", &self.arguments.len())
            .field("effect_sink", &self.effect_sink.is_some())
            .finish()
    }
}

impl fmt::Debug for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Name each shared node once. Numbering follows source order, so this
        // stays deterministic without exposing allocation addresses.
        let mut nodes = vec![self];
        let mut ids = HashMap::from([(self as *const Self as usize, 0)]);
        let mut index = 0;
        let mut edges = 0usize;
        while index < nodes.len() {
            let mut children = Vec::new();
            nodes[index].children(&mut children);
            edges = edges.saturating_add(children.len());
            if nodes.len() > MAX_NODES || edges > MAX_NODES * 8 {
                return f.write_str("Expr(<graph exceeds diagnostic limit>)");
            }
            for child in children {
                let key = Arc::as_ptr(child) as usize;
                if let std::collections::hash_map::Entry::Vacant(entry) = ids.entry(key) {
                    entry.insert(nodes.len());
                    nodes.push(child);
                }
            }
            index += 1;
        }
        let id = |child: &Arc<Expr>| ids[&(Arc::as_ptr(child) as usize)];
        f.write_str("Expr {")?;
        for (index, node) in nodes.into_iter().enumerate() {
            write!(f, " @{index} = ")?;
            match node {
                Expr::Input(index) => write!(f, "Input({index})")?,
                Expr::Capture(index) => write!(f, "Capture({index})")?,
                Expr::Field { base, label } => write!(f, "Field(@{}, {label:?})", id(base))?,
                Expr::Payload { base, label } => write!(f, "Payload(@{}, {label:?})", id(base))?,
                Expr::Record { fields, spread } => {
                    f.write_str("Record(")?;
                    for (label, child) in fields.iter() {
                        write!(f, "{label:?}: @{}, ", id(child))?;
                    }
                    if let Some(spread) = spread {
                        write!(f, "..@{}", id(spread))?;
                    }
                    f.write_str(")")?;
                }
                Expr::Tag { label, payload } => write!(f, "Tag({label:?}, @{})", id(payload))?,
                Expr::Apply { function, argument } => {
                    write!(f, "Apply(@{}, @{})", id(function), id(argument))?;
                }
                Expr::Match { scrutinee, arms } => {
                    write!(f, "Match(@{}", id(scrutinee))?;
                    for arm in arms.iter() {
                        write!(f, ", {:?} => @{}", arm.pattern, id(&arm.body))?;
                    }
                    f.write_str(")")?;
                }
                Expr::Then { value, body } => write!(f, "Then(@{}, @{})", id(value), id(body))?,
            }
            f.write_str(";")?;
        }
        f.write_str(" }")
    }
}

pub(crate) const MAX_NODES: usize = 4096;
pub(crate) const MAX_DEPTH: usize = 128;

/// Synthesize without examining other value bodies or evaluating user code.
pub fn synthesize(term: &ir::Term) -> Option<Contract> {
    synthesize_spine(None, term, None, None, None)
}

/// Synthesis while generation already borrows the outer lambda's IR fields.
pub fn synthesize_lambda(parameter: Symbol, body: &ir::Term) -> Option<Contract> {
    synthesize_spine(Some(parameter), body, None, None, None)
}

/// Written local annotations retain the exact checked semantic promise.
pub fn synthesize_lambda_with_annotations(
    parameter: Symbol,
    body: &ir::Term,
    annotations: &HashMap<Symbol, Arc<Ty>>,
    fresh_type: &mut impl FnMut() -> Arc<Ty>,
    allow_capture: &impl Fn(Symbol) -> bool,
) -> Option<Contract> {
    synthesize_spine(
        Some(parameter),
        body,
        Some(annotations),
        Some(fresh_type),
        Some(allow_capture),
    )
}

fn synthesize_spine<'a>(
    parameter: Option<Symbol>,
    term: &ir::Term,
    annotations: Option<&'a HashMap<Symbol, Arc<Ty>>>,
    fresh_type: Option<&'a mut dyn FnMut() -> Arc<Ty>>,
    allow_capture: Option<&'a dyn Fn(Symbol) -> bool>,
) -> Option<Contract> {
    let mut builder = Builder {
        captures: Vec::new(),
        remaining: MAX_NODES,
        annotations,
        fresh_type,
        allow_capture,
    };
    let mut env = HashMap::new();
    let mut body = term;
    let mut parameters = 0;
    if let Some(parameter) = parameter {
        env.insert(parameter, Arc::new(Expr::Input(0)));
        parameters = 1;
    }
    while let ir::TermKind::Fn { arg, body: next } = &body.kind {
        builder.consume(0)?;
        env.insert(arg.anchored, Arc::new(Expr::Input(parameters)));
        parameters += 1;
        body = next;
    }
    if parameters == 0 {
        return None;
    }
    let body = builder.term(body, &mut env, 0)?;
    let contract = Contract {
        parameters,
        body,
        captures: builder.captures.into(),
        arguments: Arc::from([]),
        effect_sink: None,
    };
    contract.validate().then_some(contract)
}

/// Keep ordinary short signatures when no conditional relationship is gained.
pub fn benefits(contract: &Contract) -> bool {
    let mut pending = vec![(&contract.body, false)];
    let mut depends = HashMap::new();
    let mut beneficial = false;
    while let Some((expr, finish)) = pending.pop() {
        let key = Arc::as_ptr(expr) as usize;
        if depends.contains_key(&key) {
            continue;
        }
        if !finish {
            pending.push((expr, true));
            let mut children = Vec::new();
            expr.children(&mut children);
            pending.extend(children.into_iter().map(|child| (child, false)));
            continue;
        }
        let child_depends = |child: &Arc<Expr>| depends[&(Arc::as_ptr(child) as usize)];
        let value = match &**expr {
            Expr::Input(_) => true,
            Expr::Capture(_) => false,
            Expr::Field { base, .. } | Expr::Payload { base, .. } => child_depends(base),
            Expr::Record { fields, spread } => {
                let spread_depends = spread.as_ref().is_some_and(&child_depends);
                beneficial |= spread_depends;
                spread_depends || fields.iter().any(|(_, value)| child_depends(value))
            }
            Expr::Tag { payload, .. } => child_depends(payload),
            Expr::Apply { function, argument } => {
                let value = child_depends(function) || child_depends(argument);
                beneficial |= value;
                value
            }
            Expr::Match { scrutinee, arms } => {
                let selected = child_depends(scrutinee);
                beneficial |= selected;
                selected || arms.iter().any(|arm| child_depends(&arm.body))
            }
            Expr::Then { value, body } => child_depends(value) || child_depends(body),
        };
        depends.insert(key, value);
    }
    beneficial
}

/// Synthetic checker applications for arithmetic do not by themselves justify
/// replacing an ordinary compact arrow with a structural summary.
pub fn source_benefits(term: &ir::Term, resolve: impl Fn(&Arc<Ty>) -> Arc<Ty>) -> bool {
    // An application's result has not necessarily been solved while the
    // surrounding function is being generated. Recover the known arrow spine
    // directly rather than mistaking every curried ordinary call for a callback.
    fn callee_type(term: &ir::Term, resolve: &impl Fn(&Arc<Ty>) -> Arc<Ty>) -> Arc<Ty> {
        let mut current = term;
        let mut fallback = Vec::new();
        let mut known = loop {
            let known = resolve(&current.ty);
            if matches!(&*known, Ty::Var(_))
                && let ir::TermKind::Apply { func, .. } = &current.kind
            {
                fallback.push(known);
                current = func;
                continue;
            }
            break known;
        };
        while let Some(otherwise) = fallback.pop() {
            known = match &*known {
                Ty::Arrow(_, result, _) => resolve(result),
                _ => otherwise,
            };
        }
        known
    }
    let mut pending = vec![term];
    while let Some(term) = pending.pop() {
        match &term.kind {
            ir::TermKind::Apply { func, arg } => {
                // Known ordinary arrows already express their complete input
                // requirements. Keep programs for structural callees and for
                // callbacks whose relationships are chosen at the call site.
                if !matches!(&*callee_type(func, &resolve), Ty::Arrow(..)) {
                    return true;
                }
                pending.push(func);
                pending.push(arg);
            }
            ir::TermKind::Match { .. } => return true,
            ir::TermKind::Struct {
                spread: Some(_), ..
            } => return true,
            ir::TermKind::Struct { fields, .. } => {
                pending.extend(fields.values().map(|field| &field.value));
            }
            ir::TermKind::Unary { value, .. } => pending.push(value),
            ir::TermKind::Binary { left, right, .. } => {
                pending.push(left);
                pending.push(right);
            }
            ir::TermKind::Fn { body, .. } => pending.push(body),
            ir::TermKind::Let { value, body, .. } => {
                pending.push(value);
                pending.push(body);
            }
            ir::TermKind::Array(items) => pending.extend(items.iter().map(|item| &item.value)),
            ir::TermKind::Tag { payload, .. } => pending.extend(payload.iter().map(|x| &**x)),
            ir::TermKind::Project { base, .. } => pending.push(base),
            _ => {}
        }
    }
    false
}

/// A recursive group is an explicit inference boundary, even when an
/// annotation makes one of its recursive references look like an ordinary
/// arrow. Structural summaries never unfold or partially publish that group.
pub fn references_any(term: &ir::Term, symbols: &[Symbol]) -> bool {
    let mut pending = vec![term];
    while let Some(term) = pending.pop() {
        match &term.kind {
            ir::TermKind::Ident(symbol) if symbols.contains(symbol) => return true,
            ir::TermKind::Apply { func, arg } => {
                pending.push(func);
                pending.push(arg);
            }
            ir::TermKind::Match { scrutinee, arms } => {
                pending.push(scrutinee);
                pending.extend(arms.iter().map(|(_, body)| body));
            }
            ir::TermKind::Struct { fields, spread } => {
                pending.extend(fields.values().map(|field| &field.value));
                pending.extend(spread.iter().map(|spread| &*spread.value));
            }
            ir::TermKind::Unary { value, .. } => pending.push(value),
            ir::TermKind::Binary { left, right, .. } => {
                pending.push(left);
                pending.push(right);
            }
            ir::TermKind::Fn { body, .. } => pending.push(body),
            ir::TermKind::Let { value, body, .. } => {
                pending.push(value);
                pending.push(body);
            }
            ir::TermKind::Array(items) => pending.extend(items.iter().map(|item| &item.value)),
            ir::TermKind::Tag { payload, .. } => pending.extend(payload.iter().map(|x| &**x)),
            ir::TermKind::Project { base, .. } => pending.push(base),
            _ => {}
        }
    }
    false
}

struct Builder<'a> {
    captures: Vec<Arc<Ty>>,
    remaining: usize,
    annotations: Option<&'a HashMap<Symbol, Arc<Ty>>>,
    fresh_type: Option<&'a mut dyn FnMut() -> Arc<Ty>>,
    allow_capture: Option<&'a dyn Fn(Symbol) -> bool>,
}

type Environment = HashMap<Symbol, Arc<Expr>>;

/// Exact destructuring demands contain only holes in their payload positions.
/// Written holes have the same meaning: they demand shape, not whichever
/// payload happened to inhabit the ordinary inference fallback.
fn shape_demand(annotation: &ir::Annotation) -> Option<Pattern> {
    fn pattern(ty: &ir::Type, depth: usize) -> Option<Pattern> {
        if depth > MAX_DEPTH {
            return None;
        }
        match &ty.anchored {
            ir::TypeKind::Hole => Some(Pattern::Any),
            ir::TypeKind::Struct {
                fields,
                spreads,
                tail,
            } if spreads.is_empty() && tail.is_none() => Some(Pattern::Record {
                fields: fields
                    .iter()
                    .map(|(label, field)| {
                        let ir::TypeField::Written {
                            when: None, value, ..
                        } = field
                        else {
                            return None;
                        };
                        Some((label.clone(), pattern(value, depth + 1)?))
                    })
                    .collect::<Option<Vec<_>>>()?
                    .into(),
                open: false,
            }),
            _ => None,
        }
    }
    if !annotation.variables.is_empty() || annotation.clause.is_some() {
        return None;
    }
    pattern(&annotation.ty, 0)
}

impl Builder<'_> {
    fn consume(&mut self, depth: usize) -> Option<()> {
        if depth > MAX_DEPTH {
            return None;
        }
        self.remaining = self.remaining.checked_sub(1)?;
        Some(())
    }

    fn capture(&mut self, ty: Arc<Ty>) -> Arc<Expr> {
        let index = self.captures.len();
        self.captures.push(ty);
        Arc::new(Expr::Capture(index))
    }

    fn checked(&mut self, value: Arc<Expr>, expected: Arc<Ty>) -> Arc<Expr> {
        let checker = self.capture(Arc::new(Ty::Arrow(
            expected.clone(),
            expected,
            Row::closed(),
        )));
        Arc::new(Expr::Apply {
            function: checker,
            argument: value,
        })
    }

    fn term(&mut self, term: &ir::Term, env: &mut Environment, depth: usize) -> Option<Arc<Expr>> {
        self.consume(depth)?;
        let expr = match &term.kind {
            ir::TermKind::Ident(symbol) => {
                return Some(match env.get(symbol) {
                    Some(value) => value.clone(),
                    // Each occurrence keeps its own inferred instantiation.
                    // Equal source symbols need not have equal type variables.
                    None => {
                        if self.allow_capture.is_some_and(|allowed| !allowed(*symbol)) {
                            return None;
                        }
                        self.capture(term.ty.clone())
                    }
                });
            }
            ir::TermKind::Natural(_) => return Some(self.capture(Arc::new(Ty::Nat))),
            ir::TermKind::Integer(_) => return Some(self.capture(Arc::new(Ty::Int))),
            ir::TermKind::Fixed(value) => {
                return Some(self.capture(Arc::new(Ty::Fixed(value.kind()))));
            }
            ir::TermKind::Real(_) => return Some(self.capture(Arc::new(Ty::Real))),
            ir::TermKind::String(_) => return Some(self.capture(Arc::new(Ty::String))),
            ir::TermKind::Bool(_) => return Some(self.capture(Arc::new(Ty::Bool))),
            ir::TermKind::Unary {
                op: ir::UnaryOp::Neg | ir::UnaryOp::Not,
                value,
            } => {
                let value = self.term(value, env, depth + 1)?;
                return Some(self.checked(value, term.ty.clone()));
            }
            ir::TermKind::Binary { op, left, right } if !matches!(op, ir::BinaryOp::Write) => {
                let left = self.term(left, env, depth + 1)?;
                let right = self.term(right, env, depth + 1)?;
                Expr::Then {
                    value: self.checked(left, term.ty.clone()),
                    body: self.checked(right, term.ty.clone()),
                }
            }
            ir::TermKind::Array(items) => {
                // The ordinary element may already contain a deferred call on
                // the definition's symbolic inputs. The constructor needs its
                // own variable, decided by this invocation's actual elements.
                // Captures participate in generalization, so each use receives
                // a fresh instance while all elements of one array agree.
                let element = self.fresh_type.as_mut()?();
                let array = Arc::new(Ty::Array(element.clone()));
                let mut requirements = Vec::with_capacity(items.len());
                for item in items {
                    let value = self.term(&item.value, env, depth + 1)?;
                    let expected = if item.spread.is_some() {
                        array.clone()
                    } else {
                        element.clone()
                    };
                    requirements.push(self.checked(value, expected));
                }
                let mut result = self.capture(array);
                for value in requirements.into_iter().rev() {
                    result = Arc::new(Expr::Then {
                        value,
                        body: result,
                    });
                }
                return Some(result);
            }
            ir::TermKind::Project { base, field } => Expr::Field {
                base: self.term(base, env, depth + 1)?,
                label: field.anchored.clone(),
            },
            ir::TermKind::Struct { fields, spread } => Expr::Record {
                fields: fields
                    .iter()
                    .map(|(label, field)| {
                        Some((label.clone(), self.term(&field.value, env, depth + 1)?))
                    })
                    .collect::<Option<Vec<_>>>()?
                    .into(),
                spread: match spread {
                    Some(spread) => Some(self.term(&spread.value, env, depth + 1)?),
                    None => None,
                },
            },
            ir::TermKind::Tag { name, payload } => Expr::Tag {
                label: name.anchored.clone(),
                payload: match payload {
                    Some(payload) => self.term(payload, env, depth + 1)?,
                    None => Arc::new(Expr::Record {
                        fields: Arc::from([]),
                        spread: None,
                    }),
                },
            },
            ir::TermKind::Apply { func, arg } => Expr::Apply {
                function: self.term(func, env, depth + 1)?,
                argument: self.term(arg, env, depth + 1)?,
            },
            ir::TermKind::Let {
                name,
                annotation,
                value,
                body,
            } => {
                let mut value = self.term(value, env, depth + 1)?;
                if let Some(annotation) = annotation {
                    // Do not reuse the initializer's inferred result: that can
                    // be a deferred contract, while the annotation promises a
                    // more restrictive type. Generation supplies its exact
                    // lowered promise, including rigid identities.
                    value = if let Some(pattern) = shape_demand(annotation) {
                        Arc::new(Expr::Match {
                            scrutinee: value.clone(),
                            arms: Arc::from([Arm {
                                pattern,
                                body: value,
                            }]),
                        })
                    } else {
                        let promised = self.annotations?.get(&name.anchored)?.clone();
                        self.checked(value, promised)
                    };
                }
                let previous = env.insert(name.anchored, value.clone());
                let body = self.term(body, env, depth + 1);
                match previous {
                    Some(previous) => {
                        env.insert(name.anchored, previous);
                    }
                    None => {
                        env.remove(&name.anchored);
                    }
                }
                Expr::Then { value, body: body? }
            }
            ir::TermKind::Match { scrutinee, arms } => {
                let scrutinee = self.term(scrutinee, env, depth + 1)?;
                let mut translated = Vec::with_capacity(arms.len());
                for (pattern, body) in arms {
                    let mut local = env.clone();
                    let pattern = self.pattern(pattern, &scrutinee, &mut local, depth + 1)?;
                    // A direct function-valued arm has already passed the
                    // ordinary shared-result check. Preserve that checked
                    // arrow as a selected result without translating the
                    // callback body into the structural language.
                    let body = if matches!(body.kind, ir::TermKind::Fn { .. }) {
                        self.consume(depth + 1)?;
                        self.capture(body.ty.clone())
                    } else {
                        self.term(body, &mut local, depth + 1)?
                    };
                    translated.push(Arm { pattern, body });
                }
                Expr::Match {
                    scrutinee,
                    arms: translated.into(),
                }
            }
            // No source-level evaluation is hidden in a structural summary.
            ir::TermKind::Unary { .. }
            | ir::TermKind::Binary { .. }
            | ir::TermKind::Fn { .. }
            | ir::TermKind::Handle { .. }
            | ir::TermKind::Raise(_)
            | ir::TermKind::Operation { .. }
            | ir::TermKind::Error => return None,
        };
        Some(Arc::new(expr))
    }

    fn pattern(
        &mut self,
        pattern: &ir::Pattern,
        value: &Arc<Expr>,
        env: &mut Environment,
        depth: usize,
    ) -> Option<Pattern> {
        self.consume(depth)?;
        Some(match &pattern.anchored {
            ir::PatternKind::Bind(name) => {
                env.insert(name.anchored, value.clone());
                Pattern::Any
            }
            ir::PatternKind::Wildcard => Pattern::Any,
            ir::PatternKind::Unit => Pattern::Record {
                fields: Arc::from([]),
                open: false,
            },
            ir::PatternKind::Struct { fields, rest } => Pattern::Record {
                fields: fields
                    .iter()
                    .map(|(label, field)| {
                        let projected = Arc::new(Expr::Field {
                            base: value.clone(),
                            label: label.clone(),
                        });
                        Some((
                            label.clone(),
                            self.pattern(&field.value, &projected, env, depth + 1)?,
                        ))
                    })
                    .collect::<Option<Vec<_>>>()?
                    .into(),
                open: rest.is_some(),
            },
            ir::PatternKind::Tag { name, payload } => {
                let projected = Arc::new(Expr::Payload {
                    base: value.clone(),
                    label: name.anchored.clone(),
                });
                Pattern::Tag {
                    label: name.anchored.clone(),
                    payload: Box::new(match payload {
                        Some(payload) => self.pattern(payload, &projected, env, depth + 1)?,
                        None => Pattern::Record {
                            fields: Arc::from([]),
                            open: false,
                        },
                    }),
                }
            }
            ir::PatternKind::Hidden { .. }
            | ir::PatternKind::Natural(_)
            | ir::PatternKind::Integer(_)
            | ir::PatternKind::Fixed(_)
            | ir::PatternKind::Real(_)
            | ir::PatternKind::String(_)
            | ir::PatternKind::Bool(_)
            | ir::PatternKind::Array { .. } => return None,
        })
    }
}

impl fmt::Display for Expr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_structure(f, Render::Expr(self))
    }
}

impl fmt::Display for Pattern {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_structure(f, Render::Pattern(self))
    }
}

enum Render<'a> {
    Text(&'a str),
    Index(&'a str, usize),
    Expr(&'a Expr),
    Pattern(&'a Pattern),
}

/// Explicit work stack: rendering does not borrow one Rust frame per node.
fn write_structure(f: &mut fmt::Formatter<'_>, root: Render<'_>) -> fmt::Result {
    let mut pending = vec![root];
    let mut remaining = MAX_NODES * 16;
    while let Some(part) = pending.pop() {
        let Some(next) = remaining.checked_sub(1) else {
            return f.write_str("…<diagnostic limit>");
        };
        remaining = next;
        match part {
            Render::Text(text) => f.write_str(text)?,
            Render::Index(prefix, index) => write!(f, "{prefix}{index}")?,
            Render::Expr(expr) => match expr {
                Expr::Input(index) => pending.push(Render::Index("'input", *index)),
                Expr::Capture(index) => pending.push(Render::Index("'capture", *index)),
                Expr::Field { base, label } | Expr::Payload { base, label } => {
                    pending.push(Render::Text(label));
                    pending.push(Render::Text(match expr {
                        Expr::Payload { .. } => ".#",
                        _ => ".",
                    }));
                    pending.push(Render::Text(")"));
                    pending.push(Render::Expr(base));
                    pending.push(Render::Text("("));
                }
                Expr::Record { fields, spread } => {
                    pending.push(Render::Text(" }"));
                    if let Some(spread) = spread {
                        pending.push(Render::Expr(spread));
                        pending.push(Render::Text(".."));
                        if !fields.is_empty() {
                            pending.push(Render::Text(", "));
                        }
                    }
                    for (index, (label, value)) in fields.iter().enumerate().rev() {
                        pending.push(Render::Expr(value));
                        pending.push(Render::Text(": "));
                        pending.push(Render::Text(label));
                        if index != 0 {
                            pending.push(Render::Text(", "));
                        }
                    }
                    pending.push(Render::Text("{ "));
                }
                Expr::Tag { label, payload } => {
                    pending.push(Render::Text(")"));
                    pending.push(Render::Expr(payload));
                    pending.push(Render::Text(" ("));
                    pending.push(Render::Text(label));
                    pending.push(Render::Text("#"));
                }
                Expr::Apply { function, argument } => {
                    pending.push(Render::Text(")"));
                    pending.push(Render::Expr(argument));
                    pending.push(Render::Text(") ("));
                    pending.push(Render::Expr(function));
                    pending.push(Render::Text("("));
                }
                Expr::Match { scrutinee, arms } => {
                    pending.push(Render::Text(" end"));
                    for arm in arms.iter().rev() {
                        pending.push(Render::Expr(&arm.body));
                        pending.push(Render::Text(" => "));
                        pending.push(Render::Pattern(&arm.pattern));
                        pending.push(Render::Text(" | "));
                    }
                    pending.push(Render::Text(" with"));
                    pending.push(Render::Expr(scrutinee));
                    pending.push(Render::Text("match "));
                }
                Expr::Then { value, body } => {
                    pending.push(Render::Text(" end"));
                    pending.push(Render::Expr(body));
                    pending.push(Render::Text("; return "));
                    pending.push(Render::Expr(value));
                    pending.push(Render::Text("do _ = "));
                }
            },
            Render::Pattern(pattern) => match pattern {
                Pattern::Any => pending.push(Render::Text("_")),
                Pattern::Record { fields, open } => {
                    pending.push(Render::Text(" }"));
                    if *open {
                        pending.push(Render::Text(".."));
                        if !fields.is_empty() {
                            pending.push(Render::Text(", "));
                        }
                    }
                    for (index, (label, pattern)) in fields.iter().enumerate().rev() {
                        pending.push(Render::Pattern(pattern));
                        pending.push(Render::Text(": "));
                        pending.push(Render::Text(label));
                        if index != 0 {
                            pending.push(Render::Text(", "));
                        }
                    }
                    pending.push(Render::Text("{ "));
                }
                Pattern::Tag { label, payload } => {
                    pending.push(Render::Text(")"));
                    pending.push(Render::Pattern(payload));
                    pending.push(Render::Text(" ("));
                    pending.push(Render::Text(label));
                    pending.push(Render::Text("#"));
                }
            },
        }
    }
    Ok(())
}
