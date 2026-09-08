//! Structural runtime type representations and compiler-owned reflection.
//!
//! A representation is a finite graph, not a declaration identity. Aliases
//! remain graph edges so constructing a recursive representation never unfolds
//! the recursive type into an infinite tree.

use std::{
    collections::{BTreeSet, HashMap, HashSet},
    sync::Arc,
};

use indexmap::IndexMap;

use crate::{
    inference,
    ir::{Program, Term, TermKind},
    symbol::Symbol,
    types::{FixedInt, Presence, Rest, Row, Scheme, Ty, same_finite_syntax},
};

pub fn explain(scheme: &Scheme) -> Option<String> {
    (!scheme.representations().is_empty()).then(|| {
        format!(
            "Uses runtime type information for {}. Ruddy supplies it at each instantiation.",
            scheme
                .representations()
                .iter()
                .map(|index| Ty::Bound(*index).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })
}

/// Row substitutions use an empty record/sum around their remainder, while
/// ordinary type substitutions name the bound parameter directly.
pub fn parameter_index(ty: &Ty) -> Option<u32> {
    match ty {
        Ty::Bound(index) => Some(*index),
        Ty::Struct(row) | Ty::Sum(row) => {
            let row = flattened(row);
            if row.labels.is_empty()
                && let Rest::Bound(index) = row.rest
            {
                Some(index)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Evidence needed when a polymorphic binding is instantiated. Its resulting
/// callable captures that evidence, so passing it through an ordinary
/// higher-order parameter does not expose a second calling convention.
#[derive(Debug, Clone)]
pub struct Binding {
    pub ty: Arc<Ty>,
    pub parameters: BTreeSet<u32>,
}

#[derive(Debug, Clone, Default)]
pub struct Analysis {
    pub bindings: IndexMap<Symbol, Binding>,
}

impl Analysis {
    /// Check every construction before lowering can assume evidence exists.
    /// The environment is lexical: a local generic binding adds its own
    /// parameters while retaining the enclosing closure's descriptors.
    pub fn review(
        &self,
        program: &Program,
        semantics: &inference::Semantics,
    ) -> Vec<crate::ir::Error> {
        let aliases = semantics.aliases();
        let mut errors = Vec::new();
        let mut report = |at, message| {
            errors.push(crate::ir::Error {
                at,
                kind: crate::ir::ErrorKind::RuntimeTypeInformation { message },
            })
        };
        for (symbol, declaration) in &program.externs {
            let binding = &self.bindings[symbol];
            if matches!(
                declaration.value.target.anchored.as_str(),
                "$anyUpcast" | "$anyDowncast" | "$jsDecode"
            ) && Intrinsic::recognize(&declaration.value.target.anchored, &binding.ty, aliases)
                .is_none()
            {
                report(
                    declaration.name_at,
                    "this compiler reflection intrinsic has an incompatible structural signature"
                        .into(),
                );
                continue;
            }
            if !declaration.value.array_intrinsic
                && Intrinsic::recognize(&declaration.value.target.anchored, &binding.ty, aliases)
                    .is_none()
                && let Err(message) = native_parameters(&binding.ty, aliases)
            {
                report(declaration.name_at, message);
            }
        }
        for (symbol, declaration) in semantics.typed() {
            let mut work = vec![(&declaration.value, self.bindings[symbol].parameters.clone())];
            while let Some((term, available)) = work.pop() {
                if let TermKind::Ident(target) = &term.kind
                    && let Some(binding) = self.bindings.get(target)
                {
                    let supplied = instantiate(&binding.ty, &term.ty, aliases);
                    for parameter in &binding.parameters {
                        let result = supplied.get(parameter)
                            .ok_or_else(|| "cannot determine a required runtime type at this use".to_owned())
                            .and_then(|ty| Descriptor::template(ty, aliases))
                            .and_then(|(_, parameters)| {
                                if parameters.iter().all(|parameter| available.contains(parameter)) { Ok(()) }
                                else { Err("this expression needs runtime information for a type that is not available in its environment".into()) }
                            });
                        if let Err(message) = result {
                            report(term.at, message);
                        }
                    }
                }
                if let TermKind::Let {
                    name, value, body, ..
                } = &term.kind
                {
                    let mut local = available.clone();
                    local.extend(&self.bindings[&name.anchored].parameters);
                    work.push((value, local));
                    work.push((body, available));
                } else {
                    let mut nested = Vec::new();
                    children(term, &mut nested);
                    work.extend(nested.into_iter().map(|term| (term, available.clone())));
                }
            }
        }
        errors
    }

    pub fn infer(program: &Program, semantics: &inference::Semantics) -> Self {
        let aliases = semantics.aliases();
        let mut bindings = IndexMap::new();
        for (symbol, scheme) in &program.external_schemes {
            bindings.insert(
                *symbol,
                Binding {
                    ty: scheme.body().clone(),
                    parameters: scheme.representations().iter().copied().collect(),
                },
            );
        }
        for (symbol, declaration) in &program.externs {
            let ty = semantics.externs()[symbol].body().clone();
            let parameters = Intrinsic::recognize(&declaration.value.target.anchored, &ty, aliases)
                .map(|intrinsic| parameters(&intrinsic.represented(&ty, aliases)))
                .unwrap_or_else(|| {
                    if declaration.value.array_intrinsic {
                        BTreeSet::new()
                    } else {
                        native_parameters(&ty, aliases).unwrap_or_default()
                    }
                });
            bindings.insert(*symbol, Binding { ty, parameters });
        }
        let mut bodies = Vec::new();
        for (symbol, declaration) in semantics.typed() {
            bindings.insert(
                *symbol,
                Binding {
                    ty: declaration.value.ty.clone(),
                    parameters: BTreeSet::new(),
                },
            );
            bodies.push((*symbol, &declaration.value));
            let mut work = vec![&declaration.value];
            while let Some(term) = work.pop() {
                if let TermKind::Let { name, value, .. } = &term.kind {
                    bindings.insert(
                        name.anchored,
                        Binding {
                            ty: value.ty.clone(),
                            parameters: BTreeSet::new(),
                        },
                    );
                    bodies.push((name.anchored, value));
                }
                children(term, &mut work);
            }
        }
        if bindings
            .values()
            .all(|binding| binding.parameters.is_empty())
        {
            return Self { bindings };
        }
        // Every update adds a parameter from one finite, already-solved source
        // type. Recursive calls transform demands, not source bodies or types.
        loop {
            let mut changed = false;
            for (symbol, body) in &bodies {
                let allowed = parameters(&bindings[symbol].ty);
                let mut found = BTreeSet::new();
                let mut work = vec![*body];
                while let Some(term) = work.pop() {
                    match &term.kind {
                        TermKind::Ident(target) => {
                            if let Some(binding) = bindings.get(target)
                                && !binding.parameters.is_empty()
                            {
                                let supplied = instantiate(&binding.ty, &term.ty, aliases);
                                for parameter in &binding.parameters {
                                    if let Some(ty) = supplied.get(parameter) {
                                        found.extend(parameters(ty));
                                    }
                                }
                            }
                        }
                        TermKind::Let { value, body, .. } => {
                            // A generalized local function supplies its own
                            // parameters at instantiation. Captured outer types
                            // are still owned by this enclosing computation.
                            let local_parameters = parameters(&value.ty);
                            let mut nested = vec![&**value];
                            while let Some(inner) = nested.pop() {
                                if let TermKind::Ident(target) = &inner.kind
                                    && let Some(binding) = bindings.get(target)
                                {
                                    let supplied = instantiate(&binding.ty, &inner.ty, aliases);
                                    for parameter in &binding.parameters {
                                        if let Some(ty) = supplied.get(parameter) {
                                            found.extend(
                                                parameters(ty)
                                                    .difference(&local_parameters)
                                                    .copied(),
                                            );
                                        }
                                    }
                                }
                                children(inner, &mut nested);
                            }
                            work.push(body);
                            continue;
                        }
                        _ => {}
                    }
                    children(term, &mut work);
                }
                let binding = bindings.get_mut(symbol).expect("a collected binding");
                let previous = binding.parameters.len();
                binding.parameters.extend(found.intersection(&allowed));
                changed |= previous != binding.parameters.len();
            }
            if !changed {
                break;
            }
        }
        Self { bindings }
    }

    pub fn parameters(&self, symbol: Symbol) -> Vec<u32> {
        self.bindings
            .get(&symbol)
            .map(|binding| binding.parameters.iter().copied().collect())
            .unwrap_or_default()
    }
}

/// Native callable adapters already carry effect evidence separately. Only
/// their value positions require descriptors; effects are never type demands.
pub fn native_parameters(
    ty: &Arc<Ty>,
    aliases: &IndexMap<Symbol, Scheme>,
) -> Result<BTreeSet<u32>, String> {
    let (root, graph) = crate::ir::representation_graph(ty, aliases);
    let mut result = BTreeSet::new();
    let mut work = vec![root];
    let mut seen = HashSet::new();
    while let Some(at) = work.pop() {
        if !seen.insert(at) {
            continue;
        }
        let (label, edges) = &graph[at];
        match label.as_str() {
            "arrow" => work.extend(
                edges
                    .iter()
                    .filter_map(|(name, child)| (name != "effects").then_some(*child)),
            ),
            "mut" => {}
            "package" => {
                let mut node = at;
                let mut packages = HashSet::new();
                while graph[node].0 == "package" && packages.insert(node) {
                    node = graph[node].1[0].1;
                }
                if graph[node].0 != "arrow" {
                    return Err(
                        "runtime type information is unavailable for a hidden presence package"
                            .into(),
                    );
                }
                work.push(node);
            }
            _ => result.extend(Descriptor::from_graph(at, &graph)?.1),
        }
    }
    Ok(result)
}

pub fn parameters(ty: &Arc<Ty>) -> BTreeSet<u32> {
    let mut parameters = BTreeSet::new();
    let mut work = vec![ty.clone()];
    let mut visited = HashSet::new();
    while let Some(ty) = work.pop() {
        if !visited.insert(Arc::as_ptr(&ty)) {
            continue;
        }
        match &*ty {
            Ty::Bound(index) => {
                parameters.insert(*index);
            }
            Ty::Arrow(from, to, row) => {
                work.extend([from.clone(), to.clone()]);
                row_parameters(row, &mut work, &mut parameters);
            }
            Ty::Array(inner) | Ty::Package(inner) => work.push(inner.clone()),
            Ty::Mut(region, inner) => work.extend([region.clone(), inner.clone()]),
            Ty::Struct(row) | Ty::Sum(row) => row_parameters(row, &mut work, &mut parameters),
            Ty::Named { args, .. } => work.extend(args.iter().cloned()),
            _ => {}
        }
    }
    parameters
}

fn row_parameters(row: &Row, work: &mut Vec<Arc<Ty>>, parameters: &mut BTreeSet<u32>) {
    let mut row = row;
    loop {
        work.extend(
            row.labels
                .values()
                .filter(|f| !matches!(f.presence, Presence::Absent))
                .map(|f| f.ty.clone()),
        );
        match &row.rest {
            Rest::Bound(index) => {
                parameters.insert(*index);
                break;
            }
            Rest::More(more) => row = more,
            _ => break,
        }
    }
}

/// Recover an ordinary inference instantiation from its declared and used
/// types. Bound positions are scoped by the declaration, never by their names.
pub fn instantiate(
    declared: &Arc<Ty>,
    used: &Arc<Ty>,
    aliases: &IndexMap<Symbol, Scheme>,
) -> HashMap<u32, Arc<Ty>> {
    let mut result = HashMap::new();
    let mut work = vec![(declared.clone(), used.clone())];
    let mut visited = Vec::new();
    let mut aliases_seen = HashSet::new();
    while let Some((declared, used)) = work.pop() {
        if let Ty::Bound(index) = &*declared {
            result.entry(*index).or_insert(used);
            continue;
        }
        if visited
            .iter()
            .any(|(a, b)| same_finite_syntax(a, &declared) && same_finite_syntax(b, &used))
        {
            continue;
        }
        visited.push((declared.clone(), used.clone()));
        match (&*declared, &*used) {
            (
                Ty::Named {
                    symbol: a,
                    args: xs,
                    ..
                },
                Ty::Named {
                    symbol: b,
                    args: ys,
                    ..
                },
            ) if a == b => {
                work.extend(xs.iter().cloned().zip(ys.iter().cloned()));
            }
            (Ty::Named { .. }, _) | (_, Ty::Named { .. }) => {
                let (Some(a), Some(b)) = (
                    crate::ir::representation_key(&declared, aliases),
                    crate::ir::representation_key(&used, aliases),
                ) else {
                    continue;
                };
                if !aliases_seen.insert((a, b)) {
                    continue;
                }
                work.push((
                    inference::unfold(aliases, &declared),
                    inference::unfold(aliases, &used),
                ));
            }
            (Ty::Package(inner), _) => work.push((inner.clone(), used)),
            (_, Ty::Package(inner)) => work.push((declared, inner.clone())),
            (Ty::Arrow(a, b, _), Ty::Arrow(x, y, _)) | (Ty::Mut(a, b), Ty::Mut(x, y)) => {
                work.extend([(a.clone(), x.clone()), (b.clone(), y.clone())]);
            }
            (Ty::Array(a), Ty::Array(b)) => work.push((a.clone(), b.clone())),
            (Ty::Struct(a), Ty::Struct(b)) | (Ty::Sum(a), Ty::Sum(b)) => {
                let a = flattened(a);
                let mut b = flattened(b);
                for (name, field) in &a.labels {
                    if let Some(other) = b.labels.shift_remove(name) {
                        work.push((field.ty.clone(), other.ty));
                    }
                }
                if let Rest::Bound(index) = a.rest {
                    result.entry(index).or_insert_with(|| {
                        Arc::new(if matches!(&*declared, Ty::Struct(_)) {
                            Ty::Struct(b)
                        } else {
                            Ty::Sum(b)
                        })
                    });
                }
            }
            _ => {}
        }
    }
    result
}

fn children<'a>(term: &'a Term, work: &mut Vec<&'a Term>) {
    match &term.kind {
        TermKind::Unary { value, .. } => work.push(value),
        TermKind::Binary { left, right, .. } => work.extend([&**left, &**right]),
        TermKind::Apply { func, arg } => work.extend([&**func, &**arg]),
        TermKind::Fn { body, .. } => work.push(body),
        TermKind::Let { value, body, .. } => work.extend([&**value, &**body]),
        TermKind::Struct { fields, spread } => {
            work.extend(fields.values().map(|field| &field.value));
            if let Some(spread) = spread {
                work.push(&spread.value);
            }
        }
        TermKind::Array(items) => work.extend(items.iter().map(|item| &item.value)),
        TermKind::Project { base, .. } => work.push(base),
        TermKind::Tag { payload, .. } => {
            if let Some(payload) = payload {
                work.push(payload);
            }
        }
        TermKind::Match { scrutinee, arms } => {
            work.push(scrutinee);
            work.extend(arms.iter().map(|(_, body)| body));
        }
        TermKind::Handle { body, handler } => {
            work.push(body);
            work.extend(handler.arms.iter().map(|arm| &arm.body));
            if let Some(ret) = &handler.ret {
                work.push(&ret.body);
            }
        }
        TermKind::Raise(value) => work.push(value),
        _ => {}
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Intrinsic {
    Upcast,
    Downcast,
    Decode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Direction {
    ToJs,
    FromJs,
}

impl Intrinsic {
    /// Recognition uses resolved structural types, never source type names.
    pub fn recognize(
        target: &str,
        ty: &Arc<Ty>,
        aliases: &IndexMap<Symbol, Scheme>,
    ) -> Option<Self> {
        let ty = inference::unfold(aliases, ty);
        let Ty::Arrow(from, to, effects) = &*ty else {
            return None;
        };
        if !effects.labels.is_empty() || !matches!(effects.rest, Rest::Closed) {
            return None;
        }
        match target {
            "$jsDecode" if matches!(&**from, Ty::JsValue) => {
                let result = inference::unfold(aliases, to);
                let Ty::Sum(row) = &*result else {
                    return None;
                };
                let some = row.labels.get("Some")?;
                let error = row.labels.get("Error")?;
                let error = inference::unfold(aliases, &error.ty);
                let Ty::Struct(fields) = &*error else {
                    return None;
                };
                (row.labels.len() == 2
                    && matches!(row.rest, Rest::Closed)
                    && row
                        .labels
                        .values()
                        .all(|field| matches!(field.presence, Presence::Present))
                    && matches!(&*some.ty, Ty::Bound(_))
                    && fields.labels.len() == 3
                    && matches!(fields.rest, Rest::Closed)
                    && ["path", "expected", "message"].iter().all(|name| {
                        fields.labels.get(*name).is_some_and(|field| {
                            matches!(field.presence, Presence::Present)
                                && matches!(&*field.ty, Ty::String)
                        })
                    }))
                .then_some(Self::Decode)
            }
            "$anyUpcast" if matches!(&**from, Ty::Bound(_)) && matches!(&**to, Ty::Any) => {
                Some(Self::Upcast)
            }
            "$anyDowncast" if matches!(&**from, Ty::Any) => {
                let result = inference::unfold(aliases, to);
                let Ty::Sum(row) = &*result else {
                    return None;
                };
                let some = row.labels.get("Some")?;
                let none = row.labels.get("None")?;
                (row.labels.len() == 2
                    && matches!(row.rest, Rest::Closed)
                    && matches!(some.presence, Presence::Present)
                    && matches!(none.presence, Presence::Present)
                    && matches!(&*some.ty, Ty::Bound(_))
                    && same_finite_syntax(&none.ty, &Arc::new(Ty::unit())))
                .then_some(Self::Downcast)
            }
            _ => None,
        }
    }

    pub fn represented(self, ty: &Arc<Ty>, aliases: &IndexMap<Symbol, Scheme>) -> Arc<Ty> {
        let ty = inference::unfold(aliases, ty);
        let Ty::Arrow(from, to, _) = &*ty else {
            unreachable!("a reviewed reflection intrinsic is a function")
        };
        match self {
            Self::Upcast => from.clone(),
            Self::Downcast | Self::Decode => {
                let result = inference::unfold(aliases, to);
                let Ty::Sum(row) = &*result else {
                    unreachable!("a reviewed downcast returns Option")
                };
                row.labels["Some"].ty.clone()
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Descriptor {
    pub nodes: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Node {
    Nat,
    Int,
    Fixed(FixedInt),
    Real,
    String,
    Boolean,
    Any,
    JsValue,
    Array(u32),
    Arrow([u32; 2]),
    Extend([u32; 2]),
    Struct(Vec<(String, u32)>),
    Sum(Vec<(String, u32)>),
    Alias(u32),
    Parameter(u32),
}

impl Descriptor {
    pub fn of(ty: &Arc<Ty>, aliases: &IndexMap<Symbol, Scheme>) -> Result<Self, String> {
        let (descriptor, parameters) = Self::template(ty, aliases)?;
        if !parameters.is_empty() {
            return Err(format!("runtime type information is unavailable for {ty}"));
        }
        Ok(descriptor)
    }

    pub fn template(
        ty: &Arc<Ty>,
        aliases: &IndexMap<Symbol, Scheme>,
    ) -> Result<(Self, Vec<u32>), String> {
        let (root, graph) = crate::ir::representation_graph(ty, aliases);
        Self::from_graph(root, &graph)
    }

    fn from_graph(
        root: usize,
        graph: &[(String, Vec<(String, usize)>)],
    ) -> Result<(Self, Vec<u32>), String> {
        let mut parameters = Vec::new();
        let mut nodes = vec![Node::JsValue];
        let mut shared = HashMap::from([(root, 0u32)]);
        let mut work = vec![(0, root)];
        while let Some((at, source)) = work.pop() {
            let (label, edges) = &graph[source];
            let mut child = |source: usize| {
                *shared.entry(source).or_insert_with(|| {
                    let index = nodes.len() as u32;
                    nodes.push(Node::JsValue);
                    work.push((index as usize, source));
                    index
                })
            };
            let edge = |name: &str| {
                edges
                    .iter()
                    .find_map(|(label, index)| (label == name).then_some(*index))
            };
            let node = match label.as_str() {
                "Nat" => Node::Nat,
                "Int" => Node::Int,
                "Real" => Node::Real,
                "String" => Node::String,
                "Boolean" => Node::Boolean,
                "Any" => Node::Any,
                "JsValue" => Node::JsValue,
                "Unit" => Node::Struct(Vec::new()),
                "array" => Node::Array(child(edge("element").expect("array graph edge"))),
                "arrow" => {
                    let effects = &graph[edge("effects").expect("arrow effect graph edge")];
                    if effects.0 != "effects"
                        || effects
                            .1
                            .iter()
                            .any(|(name, node)| name != "tail" || graph[*node].0 != "closed")
                    {
                        return Err(
                            "runtime type information requires a pure callable contract".into()
                        );
                    }
                    Node::Arrow([child(edge("from").unwrap()), child(edge("to").unwrap())])
                }
                "fields" | "sum" => {
                    let record = label == "fields";
                    let tail_name = if record { "core" } else { "tail" };
                    let mut fields = Vec::new();
                    let mut tail = None;
                    for (name, node) in edges {
                        if name == tail_name {
                            if graph[*node].0 != if record { "Unit" } else { "closed" } {
                                tail = Some(child(*node));
                            }
                            continue;
                        }
                        let (name, presence) =
                            name.rsplit_once(':').expect("field presence graph edge");
                        match presence {
                            "+" => fields
                                .push((name.split_once(':').unwrap().1.to_string(), child(*node))),
                            "\\" => {}
                            _ => {
                                return Err(
                                    "runtime type information requires a settled field presence"
                                        .into(),
                                );
                            }
                        }
                    }
                    fields.sort_by(|a, b| a.0.cmp(&b.0));
                    let base = if record {
                        Node::Struct(fields)
                    } else {
                        Node::Sum(fields)
                    };
                    if let Some(tail) = tail {
                        let index = nodes.len() as u32;
                        nodes.push(base);
                        Node::Extend([index, tail])
                    } else {
                        base
                    }
                }
                name if name.starts_with("param:") => {
                    let index: u32 = name[6..].parse().expect("a bound graph parameter");
                    let slot = parameters
                        .iter()
                        .position(|p| *p == index)
                        .unwrap_or_else(|| {
                            parameters.push(index);
                            parameters.len() - 1
                        });
                    Node::Parameter(slot as u32)
                }
                name => match FixedInt::ALL
                    .iter()
                    .copied()
                    .find(|kind| kind.name() == name)
                {
                    Some(kind) => Node::Fixed(kind),
                    None => {
                        return Err(
                            "runtime type information is unavailable for this structural type"
                                .into(),
                        );
                    }
                },
            };
            nodes[at] = node;
        }
        let descriptor = Self { nodes };
        descriptor
            .validate(parameters.len())
            .map_err(str::to_owned)?;
        Ok((descriptor, parameters))
    }

    /// Descriptors are public artifact data; validate references before codegen.
    pub fn validate(&self, parameters: usize) -> Result<(), &'static str> {
        if self.nodes.is_empty() {
            return Err("empty runtime type descriptor");
        }
        for node in &self.nodes {
            let references: Vec<_> = match node {
                Node::Parameter(index) if *index as usize >= parameters => {
                    return Err("invalid runtime type parameter");
                }
                Node::Array(index) | Node::Alias(index) => vec![*index],
                Node::Arrow(indices) | Node::Extend(indices) => indices.to_vec(),
                Node::Struct(fields) | Node::Sum(fields) => {
                    if fields.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
                        return Err("runtime type fields must be unique and sorted");
                    }
                    fields.iter().map(|(_, index)| *index).collect()
                }
                _ => Vec::new(),
            };
            if references
                .iter()
                .any(|index| *index as usize >= self.nodes.len())
            {
                return Err("invalid runtime type descriptor reference");
            }
        }
        // Alias and extension edges add no constructor guard. Check their
        // whole dependency graph before trusting row kinds or flattening it.
        // Constructor children may point back to an earlier row: that is an
        // ordinary guarded recursive type, not an extension cycle.
        #[derive(Clone)]
        enum Head {
            Value,
            Parameter,
            Row(bool, BTreeSet<String>),
        }
        let mut done = vec![0u8; self.nodes.len()];
        let mut heads = vec![Head::Value; self.nodes.len()];
        for start in 0..self.nodes.len() {
            let mut work = vec![(start, false)];
            while let Some((at, finish)) = work.pop() {
                if finish {
                    heads[at] = match &self.nodes[at] {
                        Node::Alias(next) => heads[*next as usize].clone(),
                        Node::Extend([left, right]) => {
                            let a = &heads[*left as usize];
                            let b = &heads[*right as usize];
                            match (a, b) {
                                (Head::Row(kind, fields), Head::Parameter)
                                | (Head::Parameter, Head::Row(kind, fields)) => {
                                    Head::Row(*kind, fields.clone())
                                }
                                (Head::Row(a, xs), Head::Row(b, ys)) if a == b => {
                                    if !xs.is_disjoint(ys) {
                                        return Err("duplicate runtime row field");
                                    }
                                    Head::Row(*a, xs.union(ys).cloned().collect())
                                }
                                _ => return Err("invalid runtime row extension"),
                            }
                        }
                        Node::Struct(fields) | Node::Sum(fields) => Head::Row(
                            matches!(self.nodes[at], Node::Struct(_)),
                            fields.iter().map(|(name, _)| name.clone()).collect(),
                        ),
                        Node::Parameter(_) => Head::Parameter,
                        _ => Head::Value,
                    };
                    done[at] = 2;
                    continue;
                }
                if done[at] == 2 {
                    continue;
                }
                if done[at] == 1 {
                    return Err("unguarded runtime type descriptor cycle");
                }
                done[at] = 1;
                work.push((at, true));
                match &self.nodes[at] {
                    Node::Alias(next) => work.push((*next as usize, false)),
                    Node::Extend(indices) => {
                        work.extend(indices.iter().map(|next| (*next as usize, false)))
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }
}

fn flattened(row: &Row) -> Row {
    let mut result = row.clone();
    while let Rest::More(more) = result.rest.clone() {
        result.labels.extend(more.labels.clone());
        result.rest = more.rest.clone();
    }
    result
}
