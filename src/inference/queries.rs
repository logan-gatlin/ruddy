//! Incremental inference. Recursive groups remain compiler-owned; Salsa only
//! tracks the acyclic dependencies between their published interfaces.
use super::*;
use salsa::{Database, Setter};
use std::sync::atomic::{AtomicUsize, Ordering};

#[salsa::db]
#[derive(Clone, Default)]
struct Db {
    storage: salsa::Storage<Self>,
    solved: Arc<AtomicUsize>,
}

#[salsa::db]
impl salsa::Database for Db {}

#[salsa::db]
trait QueryDb: Database {
    fn record_solve(&self);
}
#[salsa::db]
impl QueryDb for Db {
    fn record_solve(&self) {
        self.solved.fetch_add(1, Ordering::Relaxed);
    }
}

#[salsa::input]
struct Source {
    mint: Arc<Mint>,
    program: Arc<Program>,
    bodies: Vec<PreparedBody>,
}

// Equality belongs to the projection, not the complete source snapshot carried
// with it. Keep the actual canonical bytes, rather than accepting hash collisions.
#[derive(Clone)]
struct Projection<T> {
    key: Arc<[u8]>,
    value: T,
}
impl<T> PartialEq for Projection<T> {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}
impl<T> Eq for Projection<T> {}

#[derive(Clone)]
struct Context {
    mint: Arc<Mint>,
    program: Arc<Program>,
}

#[derive(Clone)]
struct PreparedBody {
    members: Vec<Symbol>,
    context: Projection<Context>,
}

#[salsa::tracked]
struct Header<'db> {
    #[tracked]
    context: Projection<Context>,
}

#[salsa::tracked]
struct Body<'db> {
    members: Vec<Symbol>,
    #[tracked]
    context: Projection<Context>,
}

#[derive(PartialEq, Eq, salsa::SalsaValue)]
struct Layout<'db> {
    header: Header<'db>,
    bodies: Vec<Body<'db>>,
    owners: HashMap<Symbol, Body<'db>>,
}

#[salsa::tracked]
fn layout(db: &dyn Database, source: Source) -> Layout<'_> {
    let mint = source.mint(db);
    let program = source.program(db);
    let mut key = Fingerprint::default();
    key.declarations(program);
    // Declaration inference reads the names, but never the term bodies.
    for symbol in program.terms.keys() {
        key.debug(symbol);
    }
    let header = Header::new(
        db,
        Projection {
            key: key.text.into(),
            value: Context {
                mint: mint.clone(),
                program: program.clone(),
            },
        },
    );
    let mut owners = HashMap::new();
    let bodies = source
        .bodies(db)
        .iter()
        .map(|prepared| {
            crate::cancellation::checkpoint();
            let body = Body::new(db, prepared.members.clone(), prepared.context.clone());
            for symbol in &prepared.members {
                owners.insert(*symbol, body);
            }
            body
        })
        .collect();
    Layout {
        header,
        bodies,
        owners,
    }
}

#[salsa::tracked(no_eq)]
fn declared(db: &dyn Database, header: Header<'_>) -> (Arc<Signatures>, GroupResult) {
    let context = &header.context(db).value;
    declarations(&context.mint, &context.program)
}

#[salsa::tracked]
fn published<'db>(
    db: &'db dyn QueryDb,
    source: Source,
    body: Body<'db>,
    symbol: Symbol,
) -> Projection<ExplainedScheme> {
    let result = solved(db, source, body);
    let value = result.published[&symbol].clone();
    let mut key = Fingerprint::default();
    key.scheme(&value.scheme);
    Projection {
        key: key.text.into(),
        value,
    }
}

#[derive(Clone)]
struct SolveInput {
    context: Context,
    signatures: Arc<Signatures>,
    env: Arc<HashMap<Symbol, Binding>>,
}

#[salsa::tracked]
fn group_input<'db>(
    db: &'db dyn QueryDb,
    source: Source,
    body: Body<'db>,
) -> Projection<SolveInput> {
    let graph = layout(db, source);
    let (signatures, _) = declared(db, graph.header);
    let context = &body.context(db).value;
    let mut env = HashMap::new();
    let mut references = Vec::new();
    for decl in context.program.terms.values() {
        ir::references(&decl.value, &mut references);
    }
    references.sort_unstable();
    references.dedup();
    let mut key = Fingerprint {
        text: body.context(db).key.to_vec(),
    };
    for symbol in &references {
        let binding = if let Some(owner) = graph.owners.get(symbol).filter(|owner| **owner != body)
        {
            Some(Binding::Poly(
                published(db, source, *owner, *symbol).value.clone(),
            ))
        } else {
            signatures.env.get(symbol).cloned()
        };
        if let Some(binding) = binding {
            key.debug(symbol);
            match &binding {
                Binding::Poly(value) => {
                    key.scheme(&value.scheme);
                }
                Binding::Mono(ty) => key.ty(ty),
                Binding::Local => unreachable!("a group imports generalized bindings"),
            }
            env.insert(*symbol, binding);
        }
    }
    let mut used: HashSet<_> = references
        .into_iter()
        .chain(body.members(db).iter().copied())
        .collect();
    let mut effect_keys = HashSet::new();
    for binding in env.values() {
        if let Binding::Poly(value) = binding {
            semantic_names(value.scheme.body(), &mut used, &mut effect_keys);
        }
    }
    for decl in context.program.terms.values() {
        if let Some(annotation) = &decl.annotation {
            written_names(&annotation.ty, &mut used);
        }
        for term in decl.value.walk() {
            match &term.kind {
                ir::TermKind::Let {
                    annotation: Some(annotation),
                    ..
                } => written_names(&annotation.ty, &mut used),
                ir::TermKind::Operation { effect, .. } => {
                    used.insert(effect.anchored);
                }
                ir::TermKind::Handle { handler, .. } => {
                    used.extend(handler.discharges.iter().map(|effect| effect.anchored));
                    used.extend(handler.arms.iter().map(|arm| arm.effect.anchored));
                }
                _ => {}
            }
        }
    }
    // Follow transparent aliases and operation interfaces to their declarations.
    // Reading the inferred metadata of these declarations also catches changes
    // propagated by kind, variance, and structural-effect fixpoints.
    let mut visited = HashSet::new();
    let mut pending: Vec<_> = used.iter().copied().collect();
    while let Some(symbol) = pending.pop() {
        crate::cancellation::checkpoint();
        if !visited.insert(symbol) {
            continue;
        }
        let mut discovered = HashSet::new();
        if let Some(scheme) = signatures.aliases.get(&symbol) {
            semantic_names(scheme.body(), &mut discovered, &mut effect_keys);
        }
        for ((effect, _), (from, to)) in &signatures.operations {
            if *effect == symbol {
                semantic_names(from, &mut discovered, &mut effect_keys);
                semantic_names(to, &mut discovered, &mut effect_keys);
            }
        }
        for named in discovered {
            if used.insert(named) {
                pending.push(named);
            }
        }
    }
    let current = &graph.header.context(db).value.program;
    let mut symbols: Vec<_> = used.into_iter().collect();
    symbols.sort_unstable();
    for symbol in symbols {
        key.debug(&symbol);
        key.debug(&signatures.binding_names.get(&symbol));
        key.debug(&signatures.params.get(&symbol));
        key.debug(&signatures.nominal.contains(&symbol));
        if let Some(params) = signatures.params.get(&symbol) {
            for at in 0..params.len() {
                key.debug(&signatures.variances.get(&(symbol, at as u32)));
            }
        }
        if let Some(alias) = signatures.aliases.get(&symbol) {
            key.scheme(alias);
        }
        for ((effect, selector), (from, to)) in &signatures.operations {
            if *effect == symbol {
                key.debug(selector);
                key.ty(from);
                key.ty(to);
            }
        }
        key.debug(&current.effect_ids.get(&symbol));
        key.debug(&current.effect_params.get(&symbol));
        if let Some(id) = current.effect_ids.get(&symbol) {
            effect_keys.insert(id.row_key());
        }
    }
    let mut effect_keys: Vec<_> = effect_keys.into_iter().collect();
    effect_keys.sort();
    for name in effect_keys {
        key.debug(&name);
        key.debug(&signatures.effect_kinds.get(&name));
    }
    let mut program = (*context.program).clone();
    program.effect_ids = current.effect_ids.clone();
    program.effect_params = current.effect_params.clone();
    Projection {
        key: key.text.into(),
        value: SolveInput {
            context: Context {
                mint: context.mint.clone(),
                program: Arc::new(program),
            },
            signatures: signatures.clone(),
            env: Arc::new(env),
        },
    }
}

#[salsa::tracked(no_eq)]
fn solved<'db>(db: &'db dyn QueryDb, source: Source, body: Body<'db>) -> GroupResult {
    let input = &group_input(db, source, body).value;
    db.record_solve();
    let mut result = infer_group(
        &input.context.mint,
        &input.context.program,
        &input.signatures,
        input.env.clone(),
        body.members(db),
    );
    publish_evidence(&mut result, input.signatures.clone());
    result
}

fn written_names(root: &ir::Type, out: &mut HashSet<Symbol>) {
    use ir::TypeKind as T;
    let mut work = vec![root];
    while let Some(ty) = work.pop() {
        match &ty.anchored {
            T::Ident(symbol) => {
                out.insert(*symbol);
            }
            T::Apply { head, args, .. } => {
                out.insert(*head);
                work.extend(args);
            }
            T::Array(element) => work.push(element),
            T::Mut(region, element) => {
                work.push(region);
                work.push(element);
            }
            T::Arrow { from, to, effects } => {
                work.push(from);
                work.push(to);
                written_effects(effects, out, &mut work);
            }
            T::Effects(effects) => written_effects(effects, out, &mut work),
            T::Struct { fields, .. } => {
                work.extend(fields.values().filter_map(|field| field.value()))
            }
            T::Sum { cases, .. } => work.extend(cases.values().filter_map(|case| case.payload())),
            T::Param { .. } | T::Prim(_) | T::Var(_) | T::Hole | T::Error => {}
        }
    }
}

fn written_effects<'a>(
    row: &'a ir::EffectRow,
    out: &mut HashSet<Symbol>,
    work: &mut Vec<&'a ir::Type>,
) {
    for effect in row.effects.values() {
        match effect {
            ir::EffectLabel::Written { symbol, args, .. }
            | ir::EffectLabel::Absent { symbol, args, .. } => {
                out.insert(*symbol);
                work.extend(args);
            }
        }
    }
}

fn semantic_names(root: &Ty, out: &mut HashSet<Symbol>, effects: &mut HashSet<String>) {
    let mut work = vec![root];
    let mut seen = HashSet::new();
    while let Some(ty) = work.pop() {
        if !seen.insert(ty as *const Ty) {
            continue;
        }
        let row = match ty {
            Ty::Named { symbol, args, .. } => {
                out.insert(*symbol);
                work.extend(args.iter().map(|ty| &**ty));
                None
            }
            Ty::Array(element) | Ty::Package(element) => {
                work.push(element);
                None
            }
            Ty::Mut(region, element) => {
                work.push(region);
                work.push(element);
                None
            }
            Ty::Arrow(from, to, row) => {
                work.push(from);
                work.push(to);
                Some((row, true))
            }
            Ty::Struct(row) | Ty::Sum(row) => Some((row, false)),
            _ => None,
        };
        if let Some((mut row, effect)) = row {
            loop {
                if effect {
                    effects.extend(row.labels.keys().cloned());
                }
                work.extend(row.labels.values().map(|field| &*field.ty));
                match &row.rest {
                    crate::types::Rest::More(next) => row = next,
                    _ => break,
                }
            }
        }
    }
}

/// A persistent inference database. Updating the program retains unchanged
/// recursive groups; callers may request complete diagnostic replay separately.
#[derive(Default)]
pub struct Session {
    db: Db,
    source: Option<Source>,
    requested: usize,
    body_inputs: HashMap<Vec<Symbol>, Projection<Context>>,
}

impl Session {
    /// The number of group solves performed, useful for incremental work budgets.
    pub fn solved_groups(&self) -> usize {
        self.db.solved.load(Ordering::Relaxed)
    }

    pub fn reused_groups(&self) -> usize {
        self.requested - self.solved_groups()
    }

    pub fn infer(&mut self, mint: &Mint, program: &Program, trace: Trace) -> Output {
        self.infer_shared(
            Arc::new(mint.clone()),
            Arc::new(program.clone()),
            trace,
            None,
        )
    }

    pub(crate) fn infer_shared(
        &mut self,
        mint: Arc<Mint>,
        program: Arc<Program>,
        trace: Trace,
        selected: Option<&HashSet<Symbol>>,
    ) -> Output {
        let bodies = self.prepare_bodies(&mint, &program);
        let source = match self.source {
            Some(source) => {
                source.set_mint(&mut self.db).to(mint);
                source.set_program(&mut self.db).to(program);
                source.set_bodies(&mut self.db).to(bodies);
                source
            }
            None => {
                let source = Source::new(&self.db, mint, program, bodies);
                self.source = Some(source);
                source
            }
        };
        self.output(source, trace, selected)
    }

    // Input preparation preserves immutable body snapshots before handing them
    // to Salsa. Comparing canonical source keys needs no solver; unchanged
    // bodies avoid allocating another AST and name table on every keystroke.
    fn prepare_bodies(&mut self, mint: &Mint, program: &Program) -> Vec<PreparedBody> {
        let mut previous = std::mem::take(&mut self.body_inputs);
        let mut next = HashMap::new();
        let mut bodies = Vec::new();
        for group in &program.groups {
            crate::cancellation::checkpoint();
            let mut key = Fingerprint::default();
            for symbol in &group.members {
                let decl = &program.terms[symbol];
                key.symbol(*symbol);
                key.anchor(decl.name_at);
                key.debug(&decl.annotation);
                key.debug(&decl.params);
                key.debug(&decl.metadata);
                key.term(&decl.value);
            }
            let context = if let Some(previous) = previous
                .remove(&group.members)
                .filter(|previous| previous.key.as_ref() == key.text.as_slice())
            {
                previous
            } else {
                let terms: IndexMap<_, _> = group
                    .members
                    .iter()
                    .map(|symbol| (*symbol, program.terms[symbol].clone()))
                    .collect();
                let mut names: HashSet<_> = group.members.iter().copied().collect();
                for decl in terms.values() {
                    if let Some(annotation) = &decl.annotation {
                        written_names(&annotation.ty, &mut names);
                    }
                    for term in decl.value.walk() {
                        match &term.kind {
                            ir::TermKind::Ident(symbol) => {
                                names.insert(*symbol);
                            }
                            ir::TermKind::Let {
                                name, annotation, ..
                            } => {
                                names.insert(name.anchored);
                                if let Some(annotation) = annotation {
                                    written_names(&annotation.ty, &mut names);
                                }
                            }
                            ir::TermKind::Fn { arg, .. } => {
                                names.insert(arg.anchored);
                            }
                            _ => {}
                        }
                    }
                }

                Projection {
                    key: key.text.into(),
                    value: Context {
                        mint: Arc::new(mint.project(names)),
                        program: Arc::new(Program {
                            terms,
                            ..Default::default()
                        }),
                    },
                }
            };
            next.insert(group.members.clone(), context.clone());
            bodies.push(PreparedBody {
                members: group.members.clone(),
                context,
            });
        }
        self.body_inputs = next;
        bodies
    }

    pub(crate) fn request(&mut self, selected: &HashSet<Symbol>) -> Output {
        self.output(
            self.source.expect("a prepared inference revision"),
            Trace::Off,
            Some(selected),
        )
    }

    pub(crate) fn complete(&mut self, trace: Trace) -> Output {
        self.output(
            self.source.expect("a prepared inference revision"),
            trace,
            None,
        )
    }

    fn output(
        &mut self,
        source: Source,
        trace: Trace,
        selected: Option<&HashSet<Symbol>>,
    ) -> Output {
        let mint = source.mint(&self.db);
        let program = source.program(&self.db);
        let graph = layout(&self.db, source);
        let (signatures, declaration_result) = declared(&self.db, graph.header);
        let mut results = vec![declaration_result];
        let wanted = selected.map(|selected| {
            let mut wanted = HashSet::new();
            let mut work: Vec<_> = selected
                .iter()
                .filter_map(|symbol| graph.owners.get(symbol).copied())
                .collect();
            while let Some(body) = work.pop() {
                if !wanted.insert(body) {
                    continue;
                }
                for symbol in body.members(&self.db) {
                    let mut references = Vec::new();
                    ir::references(&program.terms[symbol].value, &mut references);
                    work.extend(
                        references
                            .iter()
                            .filter_map(|symbol| graph.owners.get(symbol).copied()),
                    );
                }
            }
            wanted
        });
        for body in &graph.bodies {
            if wanted.as_ref().is_some_and(|wanted| !wanted.contains(body)) {
                continue;
            }
            crate::cancellation::checkpoint();
            self.requested += 1;
            results.push(solved(&self.db, source, *body));
        }
        assemble(mint, program, signatures.clone(), results, trace)
    }
}

// A consumer records a route through a published type, not the producer's
// current solver allocation numbers. The producer supplies those edges anew
// during assembly, so a cached consumer follows the current revision's evidence.
fn publish_evidence(result: &mut GroupResult, signatures: Arc<Signatures>) {
    for (symbol, explained) in &mut result.published {
        // A failed implementation still publishes its written contract. Give
        // that contract the same evidence routes as a successful implementation;
        // empty routes contribute no facts until the producer is repaired.
        if explained.provenance.nodes.is_empty() {
            let table = Table {
                signatures: signatures.clone(),
                ..Default::default()
            };
            let scheme = &explained.scheme;
            explained.provenance = table.scheme_provenance(
                scheme.body(),
                scheme.body(),
                &Subst::default(),
                scheme.count(),
            );
            let regions = table.region_bounds(scheme.body());
            let rows = quantified_rows(scheme.body());
            for (index, slot) in explained.provenance.quantified.iter_mut().enumerate() {
                slot.sort = if index < scheme.presences() as usize {
                    VarSort::Presence
                } else if regions.contains(&(index as u32)) {
                    VarSort::Region
                } else if rows.contains(&(index as u32)) {
                    VarSort::Row
                } else {
                    VarSort::Type
                };
            }
        }
        let mut next = u32::MAX;
        let mut bridge = |roots: &mut Vec<ReasonId>, omitted: &mut usize, sort| {
            let id = ReasonId {
                scope: *symbol,
                index: next,
            };
            next = next
                .checked_sub(1)
                .expect("published evidence fits the reason address space");
            assert!(
                next > result.reasons.len() as u32,
                "publication and solver reason spaces are disjoint"
            );
            result.reasons.push(Reason {
                id,
                parents: std::mem::replace(roots, vec![id]),
                origin: ReasonOrigin::Variable {
                    sort,
                    subject: Subject::Scheme,
                },
                reachable: true,
            });
            if *omitted != 0 {
                result
                    .omitted_reason_parents
                    .insert(id, std::mem::take(omitted));
            }
        };
        for index in evidence_order(&explained.provenance) {
            let node = &mut explained.provenance.nodes[index];
            let sort = match node.shape {
                ProvenanceShape::Ty(_) => VarSort::Type,
                ProvenanceShape::Row(_) => VarSort::Row,
                ProvenanceShape::Presence => VarSort::Presence,
            };
            bridge(&mut node.roots, &mut node.omitted, sort);
        }
        for slot in &mut explained.provenance.quantified {
            bridge(&mut slot.roots, &mut slot.omitted, slot.sort);
        }
    }
}

// Evidence addresses follow semantic field names, independently of source row
// order. Keep the skeleton itself aligned with its scheme for instantiation.
fn evidence_order(provenance: &SchemeProvenance) -> Vec<usize> {
    let mut ordered = Vec::with_capacity(provenance.nodes.len());
    let mut work = vec![0];
    while let Some(index) = work.pop() {
        ordered.push(index);
        let node = &provenance.nodes[index];
        if let ProvenanceShape::Row(labels) = &node.shape {
            let mut offset = 0;
            let mut fields: Vec<_> = labels
                .iter()
                .map(|(name, payload)| {
                    let start = offset;
                    offset += 1 + usize::from(*payload);
                    (name, &node.children[start..offset])
                })
                .collect();
            fields.sort_unstable_by_key(|(name, _)| *name);
            for (_, children) in fields.into_iter().rev() {
                work.extend(children.iter().rev());
            }
        } else {
            work.extend(node.children.iter().rev());
        }
    }
    ordered
}

fn quantified_rows(root: &Ty) -> HashSet<u32> {
    let mut found = HashSet::new();
    let mut work = vec![root];
    while let Some(ty) = work.pop() {
        let row = match ty {
            Ty::Arrow(from, to, row) => {
                work.push(from);
                work.push(to);
                Some(row)
            }
            Ty::Struct(row) | Ty::Sum(row) => Some(row),
            Ty::Array(inner) | Ty::Package(inner) => {
                work.push(inner);
                None
            }
            Ty::Mut(region, inner) => {
                work.push(region);
                work.push(inner);
                None
            }
            Ty::Named { args, .. } => {
                work.extend(args.iter().map(|arg| &**arg));
                None
            }
            _ => None,
        };
        if let Some(mut row) = row {
            loop {
                work.extend(row.labels.values().map(|field| &*field.ty));
                match &row.rest {
                    crate::types::Rest::Bound(index) => {
                        found.insert(*index);
                        break;
                    }
                    crate::types::Rest::More(next) => row = next,
                    _ => break,
                }
            }
        }
    }
    found
}

impl Fingerprint {
    fn symbol(&mut self, symbol: Symbol) {
        self.word(symbol.bundle().bits());
        self.word(symbol.bits());
    }
    fn anchor(&mut self, at: Anchor) {
        self.symbol(at.definition);
        self.word(at.node.bits());
    }

    // Encode only source structure and resolved identities. Inference overwrites
    // term.ty; serializing its Debug tree duplicated every nested term and made
    // unchanged body projection a substantial part of editor latency.
    fn term(&mut self, root: &ir::Term) {
        use ir::TermKind as T;
        for term in root.walk() {
            crate::cancellation::checkpoint();
            self.anchor(term.at);
            match &term.kind {
                T::Unary { op, .. } => {
                    self.word(1);
                    self.debug(op);
                }
                T::Binary { op, .. } => {
                    self.word(2);
                    self.debug(op);
                }
                T::Apply { .. } => self.word(3),
                T::Fn { arg, .. } => {
                    self.word(4);
                    self.debug(arg);
                }
                T::Let {
                    name, annotation, ..
                } => {
                    self.word(5);
                    self.debug(name);
                    self.debug(annotation);
                }
                T::Struct { fields, spread } => {
                    self.word(6);
                    self.word(fields.len() as u64);
                    for (label, field) in fields {
                        self.debug(label);
                        self.debug(&field.name_at);
                    }
                    self.debug(&spread.as_ref().map(|spread| spread.at));
                }
                T::Array(items) => {
                    self.word(7);
                    self.word(items.len() as u64);
                    for item in items {
                        self.debug(&item.spread);
                    }
                }
                T::Tag { name, payload } => {
                    self.word(8);
                    self.debug(name);
                    self.word(payload.is_some() as u64);
                }
                T::Project { field, .. } => {
                    self.word(9);
                    self.debug(field);
                }
                T::Match { arms, .. } => {
                    self.word(10);
                    self.word(arms.len() as u64);
                    for (pattern, _) in arms {
                        self.debug(pattern);
                    }
                }
                T::Handle { handler, .. } => {
                    self.word(11);
                    self.debug(&handler.discharges);
                    self.word(handler.arms.len() as u64);
                    for arm in &handler.arms {
                        self.debug(&arm.effect);
                        self.debug(&arm.selector);
                        self.debug(&arm.binder);
                    }
                    self.debug(&handler.ret.as_ref().map(|ret| (ret.at, &ret.binder)));
                }
                T::Raise(_) => self.word(12),
                T::Operation { effect, selector } => {
                    self.word(13);
                    self.debug(effect);
                    self.debug(selector);
                }
                T::Ident(symbol) => {
                    self.word(14);
                    self.symbol(*symbol);
                }
                T::Natural(value) => {
                    self.word(15);
                    self.word(*value);
                }
                T::Integer(value) => {
                    self.word(16);
                    self.word(*value as u64);
                }
                T::Fixed(value) => {
                    self.word(17);
                    self.debug(value);
                }
                T::Real(value) => {
                    self.word(18);
                    self.word(value.to_bits());
                }
                T::String(value) => {
                    self.word(19);
                    self.debug(value);
                }
                T::Bool(value) => {
                    self.word(20);
                    self.word(*value as u64);
                }
                T::Error => self.word(21),
            }
        }
    }
}
