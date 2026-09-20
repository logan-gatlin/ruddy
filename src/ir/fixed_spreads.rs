//! Expand fixed row operands before parameter kinds and inference read fields.
//!
//! Named payloads stay named. Only row positions unfold aliases, and each
//! expansion tracks the names it crosses so cyclic rows cannot keep growing.
use super::*;

struct Fields {
    labels: IndexMap<String, TypeField>,
    tail: Option<Tail>,
}

impl Fields {
    fn from_type(ty: Type) -> Result<Self, ErrorKind> {
        let at = ty.at;
        match ty.anchored {
            TypeKind::Struct { fields, tail, .. } => Ok(Self {
                labels: fields,
                tail,
            }),
            TypeKind::Sum { cases, tail, .. } => Ok(Self {
                labels: cases
                    .into_iter()
                    .map(|(name, case)| {
                        (
                            name,
                            match case {
                                SumCase::Written {
                                    name_at,
                                    when,
                                    payload,
                                } => TypeField::Written {
                                    name_at,
                                    when,
                                    value: payload.unwrap_or_else(|| {
                                        at.anchor(TypeKind::Struct {
                                            fields: IndexMap::new(),
                                            spreads: Vec::new(),
                                            tail: None,
                                        })
                                    }),
                                },
                                SumCase::Absent { name_at } => TypeField::Absent { name_at },
                            },
                        )
                    })
                    .collect(),
                tail,
            }),
            TypeKind::Param { symbol, index } => Ok(Self {
                labels: IndexMap::new(),
                tail: Some(Tail {
                    at,
                    of: Row::Param { symbol, index },
                }),
            }),
            TypeKind::Var(name) => Ok(Self {
                labels: IndexMap::new(),
                tail: Some(Tail {
                    at,
                    of: Row::Named(name),
                }),
            }),
            TypeKind::Error => Ok(Self {
                labels: IndexMap::new(),
                tail: None,
            }),
            _ => Err(ErrorKind::NotARow { sense: Sense::Row }),
        }
    }

    fn merge(&mut self, other: Self, shape: Shape) -> Result<(), ErrorKind> {
        for (name, field) in other.labels {
            if self.labels.contains_key(&name) {
                return Err(ErrorKind::RepeatedRowField { shape, field: name });
            }
            self.labels.insert(name, field);
        }
        self.tail = other.tail;
        Ok(())
    }
}

/// An argument keeps its caller's bindings and alias path. Following a
/// parameter returns to that caller, so nested forwarding applications do not
/// look like recursive declarations.
#[derive(Clone)]
struct Argument {
    ty: Type,
    environment: Arc<Vec<Argument>>,
    active: HashSet<Symbol>,
}

pub(super) struct Expander<'a> {
    imported: HashMap<Symbol, (&'a artifact::Type, usize)>,
    symbols: HashMap<(Namespace, String), Symbol>,
    effect_rows: ImportedEffectRows,
    declarations: HashMap<Symbol, Type>,
    done: HashMap<Symbol, Type>,
    active: HashSet<Symbol>,
    pub errors: Vec<Error>,
}

impl<'a> Expander<'a> {
    pub fn new(
        program: &Program,
        imported: &HashMap<Symbol, (&'a artifact::Type, usize)>,
        symbols: &HashMap<(Namespace, String), Symbol>,
        effect_rows: &ImportedEffectRows,
    ) -> Self {
        let declarations = program
            .types
            .iter()
            .map(|(symbol, decl)| (*symbol, decl.value.clone()))
            .collect();
        Self {
            declarations,
            imported: imported.clone(),
            symbols: symbols.clone(),
            effect_rows: effect_rows.clone(),
            done: HashMap::new(),
            active: HashSet::new(),
            errors: Vec::new(),
        }
    }

    fn named(&mut self, symbol: Symbol) -> Result<Type, ErrorKind> {
        if self.active.contains(&symbol) {
            return Err(ErrorKind::EndlessFields);
        }
        if let Some(ty) = self.done.get(&symbol) {
            return Ok(ty.clone());
        }
        let mut ty = match self.declarations.get(&symbol) {
            Some(ty) => ty.clone(),
            None => match self.imported.get(&symbol) {
                Some((ty, arity)) => imported_syntax_resolved(
                    ty,
                    &vec![Symbol::GENERATED; *arity],
                    &mut |namespace, name| self.symbols.get(&(namespace, name.to_owned())).copied(),
                    &self.effect_rows,
                ),
                None => return Ok(Anchor::GENERATED.anchor(TypeKind::Error)),
            },
        };
        self.active.insert(symbol);
        self.expand(&mut ty);
        self.active.remove(&symbol);
        self.done.insert(symbol, ty.clone());
        Ok(ty)
    }

    fn row(&mut self, ty: Type) -> Result<Fields, ErrorKind> {
        self.resolve(Argument {
            ty,
            environment: Arc::default(),
            active: HashSet::new(),
        })
    }

    fn resolve(&mut self, mut argument: Argument) -> Result<Fields, ErrorKind> {
        loop {
            let Argument {
                mut ty,
                environment,
                mut active,
            } = argument;
            argument = match &ty.anchored {
                TypeKind::Param { index, .. } if !environment.is_empty() => environment
                    .get(*index as usize)
                    .cloned()
                    .unwrap_or_else(|| Argument {
                        ty: ty.at.anchor(TypeKind::Error),
                        environment: Arc::default(),
                        active,
                    }),
                TypeKind::Ident(symbol) => {
                    if !active.insert(*symbol) {
                        return Err(ErrorKind::EndlessFields);
                    }
                    Argument {
                        ty: self.named(*symbol)?,
                        environment: Arc::default(),
                        active,
                    }
                }
                TypeKind::Apply { head, args, .. } => {
                    let supplied = args
                        .iter()
                        .map(|ty| Argument {
                            ty: ty.clone(),
                            environment: environment.clone(),
                            active: active.clone(),
                        })
                        .collect();
                    if !active.insert(*head) {
                        return Err(ErrorKind::EndlessFields);
                    }
                    Argument {
                        ty: self.named(*head)?,
                        environment: Arc::new(supplied),
                        active,
                    }
                }
                _ => {
                    self.substitute(&mut ty, &environment)?;
                    return Fields::from_type(ty);
                }
            };
        }
    }

    /// Substitute parameters without unfolding names in payload positions.
    fn substitute(
        &mut self,
        ty: &mut Type,
        environment: &Arc<Vec<Argument>>,
    ) -> Result<(), ErrorKind> {
        if environment.is_empty() {
            return Ok(());
        }
        if let TypeKind::Param { index, .. } = ty.anchored {
            match environment.get(index as usize) {
                Some(argument) => {
                    *ty = argument.ty.clone();
                    self.substitute(ty, &argument.environment)?;
                }
                None => *ty = ty.at.anchor(TypeKind::Error),
            }
            return Ok(());
        }
        let mut result = Ok(());
        children(ty, &mut |child| {
            if result.is_ok() {
                result = self.substitute(child, environment);
            }
        });
        result?;
        match &mut ty.anchored {
            TypeKind::Arrow { effects, .. } | TypeKind::Effects(effects) => {
                self.substitute_effect_tail(effects, environment)?;
            }
            _ => {}
        }
        let (tail, shape) = match &ty.anchored {
            TypeKind::Struct { tail, .. } => (tail.clone(), Shape::Struct),
            TypeKind::Sum { tail, .. } => (tail.clone(), Shape::Sum),
            _ => return Ok(()),
        };
        if let Some(Tail {
            of: Row::Param { index, .. },
            ..
        }) = tail
        {
            let argument = environment
                .get(index as usize)
                .cloned()
                .unwrap_or_else(|| Argument {
                    ty: ty.at.anchor(TypeKind::Error),
                    environment: Arc::default(),
                    active: HashSet::new(),
                });
            let more = self.resolve(argument)?;
            let mut fields = Fields::from_type(ty.clone())?;
            fields.merge(more, shape)?;
            install(ty, fields);
        }
        Ok(())
    }

    fn substitute_effect_tail(
        &mut self,
        row: &mut EffectRow,
        environment: &Arc<Vec<Argument>>,
    ) -> Result<(), ErrorKind> {
        let Some(Tail {
            at,
            of: Row::Param { index, .. },
        }) = row.tail.clone()
        else {
            return Ok(());
        };
        let Some(argument) = environment.get(index as usize) else {
            row.tail = None;
            return Ok(());
        };
        let mut ty = argument.ty.clone();
        self.substitute(&mut ty, &argument.environment)?;
        match ty.anchored {
            TypeKind::Sum {
                ref cases,
                tail: None,
                ..
            } if cases.is_empty() => row.tail = None,
            TypeKind::Effects(more) => {
                for (id, label) in more.effects {
                    if row.effects.contains_key(&id) {
                        return Err(ErrorKind::RepeatedRowField {
                            shape: Shape::Effect,
                            field: id.name().to_owned(),
                        });
                    }
                    row.effects.insert(id, label);
                }
                row.tail = more.tail;
            }
            TypeKind::Param { symbol, index } => {
                row.tail = Some(Tail {
                    at,
                    of: Row::Param { symbol, index },
                })
            }
            TypeKind::Var(name) => {
                row.tail = Some(Tail {
                    at,
                    of: Row::Named(name),
                })
            }
            TypeKind::Error => row.tail = None,
            _ => {
                return Err(ErrorKind::NotARow {
                    sense: Sense::Effects,
                });
            }
        }
        Ok(())
    }

    pub fn expand(&mut self, ty: &mut Type) {
        children(ty, &mut |child| self.expand(child));
        self.expand_shallow(ty);
    }

    pub fn expand_shallow(&mut self, ty: &mut Type) {
        let (spreads, shape) = match &ty.anchored {
            TypeKind::Struct { spreads, .. } => (spreads.clone(), Shape::Struct),
            TypeKind::Sum { spreads, .. } => (spreads.clone(), Shape::Sum),
            _ => return,
        };
        if spreads.iter().all(|spread| spread.expanded) {
            return;
        }
        let mut fields = Fields::from_type(ty.clone()).expect("a structural row");
        let tail = fields.tail.take();
        let mut failed = HashSet::new();
        for (index, spread) in spreads.into_iter().enumerate() {
            if spread.expanded {
                continue;
            }
            let result = self.row(spread.value.clone()).and_then(|more| {
                if more.tail.is_some() {
                    return Err(ErrorKind::OpenFixedSpread);
                }
                fields.merge(more, shape)
            });
            if let Err(kind) = result {
                failed.insert(index);
                self.errors.push(Error {
                    at: spread.value.at,
                    kind,
                });
            }
        }
        fields.tail = tail;
        match &mut ty.anchored {
            TypeKind::Struct { spreads, .. } | TypeKind::Sum { spreads, .. } => {
                for (index, spread) in spreads.iter_mut().enumerate() {
                    spread.expanded = true;
                    if failed.contains(&index) {
                        spread.value.anchored = TypeKind::Error;
                    }
                }
            }
            _ => {}
        }
        install(ty, fields);
    }

    pub fn declarations(&mut self, program: &mut Program) {
        for (symbol, decl) in &mut program.types {
            match self.named(*symbol) {
                Ok(ty) => decl.value = ty,
                Err(kind) => {
                    self.errors.push(Error {
                        at: decl.value.at,
                        kind,
                    });
                    decl.value.anchored = TypeKind::Error;
                }
            }
        }
        for decl in program.effects.values_mut() {
            match &mut decl.value {
                Effect::Operations(operations) => {
                    for operation in operations.values_mut() {
                        self.expand(&mut operation.from);
                        self.expand(&mut operation.to);
                    }
                }
                Effect::Alias(alias) => self.expand(&mut alias.expanded),
            }
        }
    }
}

fn install(ty: &mut Type, fields: Fields) {
    match &mut ty.anchored {
        TypeKind::Struct {
            fields: labels,
            tail,
            ..
        } => {
            *labels = fields.labels;
            *tail = fields.tail;
        }
        TypeKind::Sum { cases, tail, .. } => {
            *cases = fields
                .labels
                .into_iter()
                .map(|(name, field)| {
                    (
                        name,
                        match field {
                            TypeField::Written {
                                name_at,
                                when,
                                value,
                            } => SumCase::Written {
                                name_at,
                                when,
                                payload: Some(value),
                            },
                            TypeField::Absent { name_at } => SumCase::Absent { name_at },
                        },
                    )
                })
                .collect();
            *tail = fields.tail;
        }
        _ => unreachable!("only rows receive fields"),
    }
}

/// Every type-bearing child, including operands retained for kind validation.
pub(super) fn children(ty: &mut Type, visit: &mut impl FnMut(&mut Type)) {
    super::when_arguments_mut(ty, visit);
    match &mut ty.anchored {
        TypeKind::Struct {
            fields, spreads, ..
        } => {
            for field in fields.values_mut() {
                if let TypeField::Written { value, .. } = field {
                    visit(value);
                }
            }
            for spread in spreads {
                visit(&mut spread.value);
            }
        }
        TypeKind::Sum { cases, spreads, .. } => {
            for case in cases.values_mut() {
                if let SumCase::Written {
                    payload: Some(value),
                    ..
                } = case
                {
                    visit(value);
                }
            }
            for spread in spreads {
                visit(&mut spread.value);
            }
        }
        TypeKind::Arrow { from, to, effects } => {
            visit(from);
            visit(to);
            for label in effects.effects.values_mut() {
                for arg in label.args_mut() {
                    visit(arg);
                }
            }
        }
        TypeKind::Effects(row) => {
            for label in row.effects.values_mut() {
                for arg in label.args_mut() {
                    visit(arg);
                }
            }
        }
        TypeKind::Apply { args, .. } => {
            for arg in args {
                visit(arg);
            }
        }
        TypeKind::Array(element)
        | TypeKind::Mirror(element)
        | TypeKind::TypeInfo(element)
        | TypeKind::Hidden { body: element, .. } => visit(element),
        TypeKind::Mut(region, element) => {
            visit(region);
            visit(element);
        }
        _ => {}
    }
}
