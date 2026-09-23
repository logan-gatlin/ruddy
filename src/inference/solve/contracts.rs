//! The ordinary solver boundary for finite structural normalization.
use super::*;
use crate::contracts::Pattern;
use crate::inference::contracts::{self, Case, Context, Failure, MatchCases, Outcome};

mod coverage;
mod families;
mod results;

struct Structural<'s, 'a> {
    solve: &'s mut Solve<'a>,
    at: Anchor,
    effects: Row,
    case_guards: HashMap<usize, coverage::CaseGuard>,
    case_scopes: Vec<coverage::CaseScope>,
    argument_presences: bool,
}

impl<'s, 'a> Structural<'s, 'a> {
    fn new(solve: &'s mut Solve<'a>, at: Anchor, effects: Row) -> Self {
        Self {
            solve,
            at,
            effects,
            case_guards: HashMap::new(),
            case_scopes: Vec::new(),
            argument_presences: true,
        }
    }
}

impl Solve<'_> {
    pub(super) fn apply_contract(
        &mut self,
        at: Anchor,
        callee: &Arc<Ty>,
        argument: &Arc<Ty>,
        result: &Arc<Ty>,
        effects: &Row,
        allow_deferred: bool,
    ) {
        let mut context = Structural::new(self, at, effects.clone());
        let reported = context.solve.errors.len();
        let evaluated = contracts::apply(&mut context, callee.clone(), argument.clone());
        let ty = match evaluated {
            Ok(Outcome::Ready(ty)) => ty,
            Ok(Outcome::Deferred(_)) if !allow_deferred => {
                let mut error = Error::new(at, ErrorKind::StructuralContract {
                    message: "this unresolved structural call needs a function summary; its enclosing operation cannot preserve the argument requirements".into(),
                });
                error.id = context.solve.table.error_id();
                context.solve.errors.push(error);
                Arc::new(Ty::Undecided)
            }
            Ok(Outcome::Deferred(contract)) => {
                let fallback = context.applied_fallback(callee, argument);
                Arc::new(Ty::Contract { fallback, contract })
            }
            Err(error) => {
                // Ordinary compatibility is checked while the contract is
                // evaluated. If that already explained this bad call, the
                // structural failure is only a consequence of the same
                // malformed input and would duplicate the diagnostic.
                if context.solve.errors.len() == reported {
                    context.solve.errors.push(error);
                }
                Arc::new(Ty::Undecided)
            }
        };
        context.solve.unify(at, result, &ty);
    }

    pub(super) fn normalize_contract(&mut self, at: Anchor, ty: &Arc<Ty>) -> Arc<Ty> {
        // Ordinary named types must reach the solver's recursive-alias rule
        // unchanged. Unfolding them here would bypass its finite assumptions.
        if !matches!(&**ty, Ty::Contract { contract, .. } if contract.remaining_parameters() == 0) {
            return ty.clone();
        }
        let mut context = Structural::new(self, at, Row::of(Rest::Undecided));
        match contracts::normalize(&mut context, ty.clone()) {
            Ok(Outcome::Ready(ty)) => ty,
            Ok(Outcome::Deferred(contract)) => match &**ty {
                Ty::Contract { fallback, .. } => Arc::new(Ty::Contract {
                    fallback: fallback.clone(),
                    contract,
                }),
                _ => ty.clone(),
            },
            Err(error) => {
                context.solve.errors.push(error);
                Arc::new(Ty::Undecided)
            }
        }
    }

    pub(super) fn compare_contract(
        &mut self,
        at: Anchor,
        expected: &Arc<Ty>,
        actual: &Arc<Ty>,
    ) -> Option<(Arc<Ty>, Arc<Ty>)> {
        // A generic higher-order call can ask for an ordinary arrow before
        // its other arguments have fixed that arrow's domain. Refine such a
        // flexible domain with the source body's known input shape, then
        // replay the contract below. The representation result is never a
        // conformance proof: independent branches and deferred calls must
        // still establish every result and requirement through evaluation.
        if let (Ty::Arrow(from, _, _), Ty::Contract { fallback, .. }) = (&**expected, &**actual)
            && matches!(&*self.table.resolve(from), Ty::Var(_))
        {
            let mut fallback = self.table.resolve(fallback);
            while let Ty::Contract {
                fallback: inner, ..
            } = &*fallback
            {
                fallback = self.table.resolve(inner);
            }
            if let Ty::Arrow(domain, _, _) = &*fallback {
                self.unify(at, from, domain);
            }
        }
        let effects = match &**expected {
            Ty::Arrow(_, _, effects) => effects.clone(),
            _ => Row::closed(),
        };
        let mut context = Structural::new(self, at, effects);
        // Annotation inputs are universally quantified promises. Evaluating
        // the implementation must prove their admitted shapes; it cannot
        // specialize the annotation's presences as an ordinary call can.
        context.argument_presences = false;
        let mut equation = None;
        let checked = match &**expected {
            Ty::Contract { contract, .. } => {
                contracts::check_annotation_cases(&mut context, actual.clone(), contract)
            }
            Ty::Arrow(from, to, _) => {
                match contracts::apply(&mut context, actual.clone(), from.clone()) {
                    Ok(Outcome::Ready(result)) => {
                        // Continue this ordinary result obligation on the
                        // caller's work stack rather than nesting another
                        // complete conformance check on the host stack.
                        equation = Some((to.clone(), result));
                        Ok(true)
                    }
                    Ok(Outcome::Deferred(_)) => Ok(false),
                    Err(error) => Err(error),
                }
            }
            _ => Ok(false),
        };
        match checked {
            Ok(true) => {}
            checked => {
                let error = match checked {
                    Err(error) => error,
                    _ => Error::new(
                        at,
                        ErrorKind::StructuralContract {
                            message: "the structural contract does not establish the promised type"
                                .into(),
                        },
                    ),
                };
                context.solve.fail(
                    at,
                    Rule::Mismatch,
                    Goal::Type {
                        expected: expected.clone(),
                        actual: actual.clone(),
                    },
                    error,
                    &[Assigned::Ty(expected.clone()), Assigned::Ty(actual.clone())],
                );
            }
        }
        equation
    }

    /// Complete the effect obligation after corresponding contract captures
    /// have been checked by the ordinary solver's continuation stack.
    pub(super) fn compare_contract_effects(
        &mut self,
        at: Anchor,
        expected: &Arc<Ty>,
        actual: &Arc<Ty>,
    ) {
        let (Ty::Contract { contract: a, .. }, Ty::Contract { contract: b, .. }) =
            (&**expected, &**actual)
        else {
            unreachable!("contract-equation continuation retains both programs");
        };
        if let Some(want) = &a.effect_sink {
            let want = self.table.resolve(want);
            if let Ty::Arrow(_, _, allowance) = &*want {
                let mut cursor = actual.clone();
                let mut performed = Row::closed();
                for _ in 0..b.remaining_parameters() {
                    loop {
                        cursor = self.table.resolve(&cursor);
                        match &*cursor {
                            Ty::Contract { fallback, .. } => cursor = fallback.clone(),
                            _ => break,
                        }
                    }
                    if let Ty::Arrow(_, result, effects) = &*cursor {
                        performed = effects.clone();
                        cursor = result.clone();
                    }
                }
                if let Some(sink) = &b.effect_sink
                    && let Ty::Arrow(_, _, effects) = &*self.table.resolve(sink)
                {
                    performed = effects.clone();
                }
                self.relate_effects(at, &performed, allowance, true, true);
            }
        }
    }
}

impl Structural<'_, '_> {
    /// Contract inputs observe sealed package choices; ordinary inference is
    /// still allowed to bind its own existential presences normally.
    fn argument_presence(&mut self, left: &Presence, right: &Presence) {
        let sealed = |presence: &Presence| {
            matches!(presence, Presence::Var(var)
                if self.solve.table.abstract_existentials.contains(var))
        };
        let flexible = |presence: &Presence| {
            matches!(presence, Presence::Var(var)
                if !self.solve.table.abstract_existentials.contains(var))
        };
        if (sealed(left) && !flexible(right)) || (sealed(right) && !flexible(left)) {
            self.solve.guarded_presence(self.at, left, right);
        } else {
            self.solve.presences(self.at, left, right);
        }
    }
}

impl Context for Structural<'_, '_> {
    type Error = Error;
    fn effect_sink(&mut self) -> Arc<Ty> {
        Arc::new(Ty::Arrow(
            Arc::new(Ty::unit()),
            Arc::new(Ty::unit()),
            self.effects.clone(),
        ))
    }
    fn set_effect_sink(&mut self, sink: Arc<Ty>) {
        if let Ty::Arrow(_, _, effects) = &*self.solve.table.resolve(&sink) {
            self.effects = effects.clone();
        }
    }
    fn link_effect_sink(&mut self, sink: &Arc<Ty>) -> Result<(), Error> {
        let sink = self.solve.table.resolve(sink);
        if let Ty::Arrow(_, _, effects) = &*sink {
            self.solve
                .relate_effects(self.at, effects, &self.effects, true, true);
            Ok(())
        } else {
            Err(self.failure(Failure::MalformedContract))
        }
    }
    fn applied_fallback(&mut self, function: &Arc<Ty>, argument: &Arc<Ty>) -> Arc<Ty> {
        let table = &*self.solve.table;
        let aliases = self.solve.aliases;
        crate::contracts::representation::applied(
            function,
            argument,
            |ty| super::super::unfold(aliases, &table.resolve(ty)),
            |row| table.canon(row),
            |presence| table.presence_of(presence),
        )
    }
    fn resolve_type(&mut self, ty: &Arc<Ty>) -> Arc<Ty> {
        let mut ty = self.solve.table.resolve(ty);
        loop {
            ty = super::super::unfold(self.solve.aliases, &ty);
            if matches!(&*ty, Ty::Package(_)) {
                ty = self.solve.table.open_package(self.at, &ty);
            } else {
                break;
            }
        }
        let checked =
            super::super::contract_annotations::reconstruct(self.solve.table, self.at, ty);
        self.solve.errors.extend(checked.errors);
        checked.fallback
    }
    fn ordinary_apply(&mut self, function: Arc<Ty>, argument: Arc<Ty>) -> Result<Arc<Ty>, Error> {
        let function = self.resolve_type(&function);
        let (parameter, result, effects) = match &*function {
            Ty::Arrow(parameter, result, effects) => {
                (parameter.clone(), result.clone(), effects.clone())
            }
            _ => {
                let parameter = self
                    .solve
                    .table
                    .fresh_type_for(super::super::Subject::Argument);
                let result = self
                    .solve
                    .table
                    .fresh_type_for(super::super::Subject::Context);
                let effects = Row::of(
                    self.solve
                        .table
                        .fresh_row_for(super::super::Subject::AmbientEffects),
                );
                self.solve.unify(
                    self.at,
                    &Arc::new(Ty::Arrow(
                        parameter.clone(),
                        result.clone(),
                        effects.clone(),
                    )),
                    &function,
                );
                (parameter, result, effects)
            }
        };
        self.solve.unify(self.at, &parameter, &argument);
        self.solve
            .relate_effects(self.at, &effects, &self.effects, true, true);
        Ok(result)
    }

    fn contract_argument(&mut self, function: &Arc<Ty>, argument: &Arc<Ty>) -> Result<(), Error> {
        let mut function = self.resolve_type(function);
        while let Ty::Contract { fallback, .. } = &*function {
            function = self.resolve_type(fallback);
        }
        let Ty::Arrow(parameter, _, _) = &*function else {
            return Ok(());
        };
        let mut pending = vec![(parameter.clone(), argument.clone())];
        let mut seen = HashSet::new();
        let mut retained = Vec::new();
        let mut remaining = 32_768usize;
        let mut copied = HashMap::new();
        while let Some((expected, actual)) = pending.pop() {
            remaining = remaining
                .checked_sub(1)
                .ok_or_else(|| self.failure(Failure::WorkLimit))?;
            let expected = self.resolve_type(&expected);
            let actual = self.resolve_type(&actual);
            if Arc::ptr_eq(&expected, &actual)
                || !seen.insert((Arc::as_ptr(&expected), Arc::as_ptr(&actual)))
            {
                continue;
            }
            retained.push((expected.clone(), actual.clone()));
            match (&*expected, &*actual) {
                (Ty::Contract { fallback, .. }, _) => pending.push((fallback.clone(), actual)),
                (_, Ty::Contract { fallback, .. }) => pending.push((expected, fallback.clone())),
                (Ty::Struct(left), Ty::Struct(right)) | (Ty::Sum(left), Ty::Sum(right)) => {
                    let sum = matches!(&*expected, Ty::Sum(_));
                    let left = self.solve.table.canon(left);
                    let right = self.solve.table.canon(right);
                    for (label, field) in &left.labels {
                        if let Some(other) = right.labels.get(label) {
                            let annotation_choice = matches!(
                                self.solve.table.presence_of(&other.presence),
                                Presence::Var(var)
                                    if self.solve.table.var_meta[var as usize].subject
                                        == super::super::Subject::Annotation
                            );
                            if self.argument_presences && !annotation_choice {
                                self.argument_presence(&field.presence, &other.presence);
                            }
                            if !matches!(
                                self.solve.table.presence_of(&field.presence),
                                Presence::Absent
                            ) && !matches!(
                                self.solve.table.presence_of(&other.presence),
                                Presence::Absent
                            ) {
                                pending.push((field.ty.clone(), other.ty.clone()));
                            }
                        } else if self.argument_presences
                            && (sum || matches!(right.rest, Rest::Closed))
                        {
                            self.argument_presence(&field.presence, &Presence::Absent);
                        }
                    }
                }
                (Ty::Var(_), _) => {
                    let actual = self.payload_shape(&actual, 0, &mut remaining, &mut copied)?;
                    self.solve.unify(self.at, &expected, &actual);
                }
                (_, Ty::Var(_)) => {
                    let expected = self.payload_shape(&expected, 0, &mut remaining, &mut copied)?;
                    self.solve.unify(self.at, &expected, &actual);
                }
                (Ty::Arrow(a, b, effects), Ty::Arrow(c, d, performed)) => {
                    pending.push((a.clone(), c.clone()));
                    pending.push((b.clone(), d.clone()));
                    self.solve.unify(
                        self.at,
                        &Arc::new(Ty::Arrow(
                            Arc::new(Ty::unit()),
                            Arc::new(Ty::unit()),
                            effects.clone(),
                        )),
                        &Arc::new(Ty::Arrow(
                            Arc::new(Ty::unit()),
                            Arc::new(Ty::unit()),
                            performed.clone(),
                        )),
                    );
                }
                (Ty::Array(a), Ty::Array(b))
                | (Ty::Mirror(a), Ty::Mirror(b))
                | (Ty::TypeInfo(a), Ty::TypeInfo(b)) => pending.push((a.clone(), b.clone())),
                _ => self.solve.unify(self.at, &expected, &actual),
            }
        }
        Ok(())
    }
    fn project(&mut self, base: Arc<Ty>, label: &str) -> Result<Arc<Ty>, Error> {
        let base = self.resolve_type(&base);

        let result = self
            .solve
            .table
            .fresh_type_for(super::super::Subject::ProjectionResult);
        self.solve.project(self.at, self.at, &base, label, &result);
        Ok(self.solve.table.resolve(&result))
    }
    fn payload(&mut self, base: Arc<Ty>, label: &str) -> Result<Arc<Ty>, Error> {
        let base = self.resolve_type(&base);
        if let Ty::Sum(row) = &*base {
            let row = self.solve.table.canon(row);
            if let Some(field) = row.labels.get(label)
                && matches!(
                    self.solve.table.presence_of(&field.presence),
                    Presence::Present
                )
            {
                return Ok(field.ty.clone());
            }
        }
        Err(self.failure(Failure::UncoveredMatch))
    }
    fn record(
        &mut self,
        fields: Vec<(String, Arc<Ty>)>,
        spread: Option<Arc<Ty>>,
    ) -> Result<Arc<Ty>, Error> {
        let mut row = match spread {
            None => Row::closed(),
            Some(base) => {
                let base = self.resolve_type(&base);
                match &*base {
                    Ty::Struct(row) => self.solve.table.canon(row),
                    _ => {
                        let row = Row::of(
                            self.solve
                                .table
                                .fresh_row_for(super::super::Subject::StructSpread),
                        );
                        self.solve
                            .unify(self.at, &Arc::new(Ty::Struct(row.clone())), &base);
                        self.solve.table.canon(&row)
                    }
                }
            }
        };
        // Remove overwritten labels from a symbolic tail through the existing
        // spread relation, which permits a different previous payload type.
        if !matches!(row.rest, Rest::Closed)
            && fields
                .iter()
                .any(|(label, _)| !row.labels.contains_key(label))
        {
            let rest = self
                .solve
                .table
                .fresh_row_for(super::super::Subject::StructSpread);
            let demand = Arc::new(Ty::Struct(Row {
                labels: fields
                    .iter()
                    .map(|(label, _)| {
                        (
                            label.clone(),
                            RowField {
                                presence: self
                                    .solve
                                    .table
                                    .fresh_presence_for(super::super::Subject::StructSpread),
                                ty: self
                                    .solve
                                    .table
                                    .fresh_type_for(super::super::Subject::StructSpread),
                            },
                        )
                    })
                    .collect(),
                rest: rest.clone(),
            }));
            self.solve
                .unify(self.at, &demand, &Arc::new(Ty::Struct(row)));
            row = Row::of(rest);
        }
        for (label, ty) in fields {
            row.labels.insert(label, RowField::present(ty));
        }
        let result = Arc::new(Ty::Struct(row));
        self.solve.table.note_lacks(&result);
        Ok(result)
    }
    fn tag(&mut self, label: &str, payload: Arc<Ty>) -> Result<Arc<Ty>, Error> {
        Ok(Arc::new(Ty::Sum(Row {
            labels: [(label.to_owned(), RowField::present(payload))].into(),
            rest: self.solve.table.fresh_row_for(super::super::Subject::Term),
        })))
    }
    fn split_match(&mut self, input: Arc<Ty>, patterns: &[Pattern]) -> Result<MatchCases, Error> {
        if !self.match_family(&input, patterns)? {
            return Ok(MatchCases::Deferred);
        }
        let original = input.clone();
        let mut remaining = vec![input];
        let mut cases = Vec::new();
        let mut budget = 4096;
        for (arm, pattern) in patterns.iter().enumerate() {
            let mut rest = Vec::new();
            for input in remaining {
                let Some((yes, no)) = self.partition(input, pattern, &mut budget)? else {
                    return Ok(MatchCases::Deferred);
                };
                cases.extend(yes.into_iter().map(|input| Case { arm, input }));
                rest.extend(no);
            }
            remaining = rest;
        }
        // Each normalization case owns its guard identity, including a
        // nested catch-all that returns its input without refining it.
        for case in &mut cases {
            case.input = Arc::new((*case.input).clone());
        }
        self.guard_cases(&original, &cases, &remaining)?;
        Ok(MatchCases::Known(cases))
    }
    fn begin_case(&mut self, input: &Arc<Ty>) -> Result<bool, Error> {
        self.push_case(input)
    }
    fn end_case(&mut self) {
        self.pop_case();
    }
    fn join(&mut self, alternatives: Vec<(Arc<Ty>, Arc<Ty>)>) -> Result<Arc<Ty>, Error> {
        self.join_results(alternatives)
    }
    fn failure(&mut self, failure: Failure) -> Error {
        let mut error = Error::new(
            self.at,
            ErrorKind::StructuralContract {
                message: failure.to_string(),
            },
        );
        error.id = self.solve.table.error_id();
        error
    }
}

type Partition = Option<(Vec<Arc<Ty>>, Vec<Arc<Ty>>)>;
impl Structural<'_, '_> {
    /// Copy only structural shape variables. Ordinary payload variables remain
    /// shared, but checking a contract must not widen the caller's known tags
    /// or fields, which its structural program can still select precisely.
    fn payload_shape(
        &mut self,
        ty: &Arc<Ty>,
        depth: usize,
        budget: &mut usize,
        copied: &mut HashMap<*const Ty, (Arc<Ty>, Arc<Ty>)>,
    ) -> Result<Arc<Ty>, Error> {
        *budget = budget
            .checked_sub(1)
            .ok_or_else(|| self.failure(Failure::WorkLimit))?;
        if depth > crate::contracts::MAX_DEPTH {
            return Err(self.failure(Failure::DepthLimit));
        }
        let ty = self.solve.table.resolve(ty);
        if let Some((_, result)) = copied.get(&Arc::as_ptr(&ty)) {
            return Ok(result.clone());
        }
        let copy = match &*ty {
            Ty::Struct(row) | Ty::Sum(row) => {
                let row = self.solve.table.canon(row);
                let mut labels = IndexMap::new();
                for (label, field) in &row.labels {
                    if matches!(
                        self.solve.table.presence_of(&field.presence),
                        Presence::Absent
                    ) {
                        continue;
                    }
                    labels.insert(
                        label.clone(),
                        RowField {
                            presence: self
                                .solve
                                .table
                                .fresh_presence_for(super::super::Subject::Argument),
                            ty: self.payload_shape(&field.ty, depth + 1, budget, copied)?,
                        },
                    );
                }
                let row = Row {
                    labels,
                    rest: self
                        .solve
                        .table
                        .fresh_row_for(super::super::Subject::Argument),
                };
                if matches!(&*ty, Ty::Struct(_)) {
                    Ty::Struct(row)
                } else {
                    Ty::Sum(row)
                }
            }
            Ty::Arrow(from, to, effects) => Ty::Arrow(
                self.payload_shape(from, depth + 1, budget, copied)?,
                self.payload_shape(to, depth + 1, budget, copied)?,
                effects.clone(),
            ),
            Ty::Array(element) => {
                Ty::Array(self.payload_shape(element, depth + 1, budget, copied)?)
            }
            Ty::Mirror(element) => {
                Ty::Mirror(self.payload_shape(element, depth + 1, budget, copied)?)
            }
            Ty::TypeInfo(element) => {
                Ty::TypeInfo(self.payload_shape(element, depth + 1, budget, copied)?)
            }
            Ty::Contract { fallback, .. } => {
                return self.payload_shape(fallback, depth + 1, budget, copied);
            }
            _ => return Ok(ty),
        };
        let copy = Arc::new(copy);
        copied.insert(Arc::as_ptr(&ty), (ty, copy.clone()));
        Ok(copy)
    }
    fn partition(
        &mut self,
        input: Arc<Ty>,
        pattern: &Pattern,
        budget: &mut usize,
    ) -> Result<Partition, Error> {
        *budget = budget
            .checked_sub(1)
            .ok_or_else(|| self.failure(Failure::WorkLimit))?;
        let input = self.resolve_type(&input);
        if matches!(pattern, Pattern::Any) {
            return Ok(Some((vec![input], vec![])));
        }

        match (&*input, pattern) {
            (Ty::Var(_) | Ty::Rigid { .. } | Ty::Bound(_) | Ty::Contract { .. }, _) => Ok(None),
            (Ty::Sum(row), Pattern::Tag { label, payload }) => {
                let mut row = self.solve.table.canon(row);
                match row.rest {
                    Rest::Var(var) => {
                        // A flexible sum tail admits widening, but introduces no
                        // additional runtime case of its own. Fix it to empty.
                        let goal = Goal::Type {
                            expected: input.clone(),
                            actual: input.clone(),
                        };
                        self.solve.assign(
                            self.at,
                            goal,
                            var,
                            Assigned::Row(Arc::new(Row::closed())),
                        );
                        row.rest = Rest::Closed;
                    }
                    Rest::Closed | Rest::Undecided => {}
                    _ => return Ok(None),
                }
                let mut yes = Vec::new();
                let mut no = Vec::new();
                for (name, field) in row.into_parts().0 {
                    if matches!(
                        self.solve.table.presence_of(&field.presence),
                        Presence::Absent
                    ) {
                        continue;
                    }
                    let single = |ty| {
                        Arc::new(Ty::Sum(Row {
                            labels: [(name.clone(), RowField::present(ty))].into(),
                            rest: Rest::Closed,
                        }))
                    };
                    if &name == label {
                        let Some((y, n)) = self.partition(field.ty, payload, budget)? else {
                            return Ok(None);
                        };
                        yes.extend(y.into_iter().map(&single));
                        no.extend(n.into_iter().map(&single));
                    } else {
                        no.push(single(field.ty));
                    }
                }
                Ok(Some((yes, no)))
            }
            (Ty::Struct(row), Pattern::Record { fields, open }) => {
                let row = self.solve.table.canon(row);
                if !*open && !matches!(row.rest, Rest::Closed | Rest::Undecided) {
                    return Ok(None);
                }
                let mut yes = vec![row];
                let mut no = Vec::new();
                for (label, subpattern) in fields.iter() {
                    let mut next = Vec::new();
                    for mut row in yes {
                        let Some(field) = row.labels.get(label).cloned() else {
                            if !matches!(row.rest, Rest::Closed) {
                                return Ok(None);
                            }
                            no.push(Arc::new(Ty::Struct(row)));
                            continue;
                        };
                        match self.solve.table.presence_of(&field.presence) {
                            Presence::Absent => {
                                no.push(Arc::new(Ty::Struct(row)));
                                continue;
                            }
                            Presence::Present => {}
                            _ => {
                                let mut absent = row.clone();
                                absent.labels.shift_remove(label);
                                no.push(Arc::new(Ty::Struct(absent)));
                            }
                        }
                        let Some((y, n)) = self.partition(field.ty, subpattern, budget)? else {
                            return Ok(None);
                        };
                        for ty in n {
                            let mut failed = row.clone();
                            failed.labels.insert(label.clone(), RowField::present(ty));
                            no.push(Arc::new(Ty::Struct(failed)));
                        }
                        for ty in y {
                            row.labels.insert(label.clone(), RowField::present(ty));
                            next.push(row.clone());
                        }
                    }
                    yes = next;
                }
                let mut accepted = Vec::new();
                for mut row in yes {
                    if !*open {
                        let extra: Vec<_> = row
                            .labels
                            .iter()
                            .filter(|(name, _)| !fields.iter().any(|(label, _)| label == *name))
                            .map(|(name, field)| (name.clone(), field.clone()))
                            .collect();
                        let mut failed = false;
                        for (label, field) in extra {
                            match self.solve.table.presence_of(&field.presence) {
                                Presence::Absent => {
                                    row.labels.shift_remove(&label);
                                }
                                Presence::Present => {
                                    failed = true;
                                    break;
                                }
                                _ => {
                                    let mut with = row.clone();
                                    with.labels
                                        .insert(label.clone(), RowField::present(field.ty));
                                    no.push(Arc::new(Ty::Struct(with)));
                                    row.labels.shift_remove(&label);
                                }
                            }
                        }
                        if failed {
                            no.push(Arc::new(Ty::Struct(row)));
                            continue;
                        }
                    }
                    accepted.push(Arc::new(Ty::Struct(row)));
                }
                Ok(Some((accepted, no)))
            }
            (Ty::Undecided, _) => Ok(Some((vec![input], vec![]))),
            _ => Ok(Some((vec![], vec![input]))),
        }
    }
}

/// Compare programs independently of sharing, capture numbering, or how many
/// arguments were supplied before the displayed residual was written back.
/// The finite pair walk collects ordinary type obligations only after every
/// structural operation has matched.
pub(super) fn structural_equations(
    expected: &crate::contracts::Contract,
    actual: &crate::contracts::Contract,
) -> Option<Vec<(Arc<Ty>, Arc<Ty>)>> {
    use crate::contracts::Expr;
    if expected.remaining_parameters() != actual.remaining_parameters() {
        return None;
    }
    fn leaf(contract: &crate::contracts::Contract, expr: &Expr) -> Option<Arc<Ty>> {
        match expr {
            Expr::Capture(index) => contract.captures.get(*index).cloned(),
            Expr::Input(index) => contract.arguments.get(*index).cloned(),
            _ => None,
        }
    }
    let mut pending = vec![(&expected.body, &actual.body)];
    let mut seen = HashSet::new();
    let mut equations = Vec::new();
    while let Some((left, right)) = pending.pop() {
        if !seen.insert((Arc::as_ptr(left), Arc::as_ptr(right))) {
            continue;
        }
        if seen.len() > 32768 {
            return None;
        }
        match (leaf(expected, left), leaf(actual, right)) {
            (Some(left), Some(right)) => {
                equations.push((left, right));
                continue;
            }
            (None, None) => {}
            _ => return None,
        }
        match (&**left, &**right) {
            (Expr::Input(a), Expr::Input(b))
                if a.checked_sub(expected.arguments.len())
                    == b.checked_sub(actual.arguments.len()) => {}
            (Expr::Field { base: a, label: x }, Expr::Field { base: b, label: y })
            | (Expr::Payload { base: a, label: x }, Expr::Payload { base: b, label: y })
                if x == y =>
            {
                pending.push((a, b))
            }
            (
                Expr::Apply {
                    function: a,
                    argument: x,
                },
                Expr::Apply {
                    function: b,
                    argument: y,
                },
            ) => {
                pending.push((a, b));
                pending.push((x, y));
            }
            (Expr::Then { value: a, body: x }, Expr::Then { value: b, body: y }) => {
                pending.push((a, b));
                pending.push((x, y));
            }
            (
                Expr::Tag {
                    label: a,
                    payload: x,
                },
                Expr::Tag {
                    label: b,
                    payload: y,
                },
            ) if a == b => pending.push((x, y)),
            (
                Expr::Record {
                    fields: a,
                    spread: x,
                },
                Expr::Record {
                    fields: b,
                    spread: y,
                },
            ) if a.len() == b.len() => {
                match (x, y) {
                    (Some(x), Some(y)) => pending.push((x, y)),
                    (None, None) => {}
                    _ => return None,
                }
                for (label, value) in a.iter() {
                    let other = b.iter().find(|(name, _)| name == label)?;
                    pending.push((value, &other.1));
                }
            }
            (
                Expr::Match {
                    scrutinee: a,
                    arms: x,
                },
                Expr::Match {
                    scrutinee: b,
                    arms: y,
                },
            ) if x.len() == y.len() => {
                pending.push((a, b));
                for (a, b) in x.iter().zip(y.iter()) {
                    if a.pattern != b.pattern {
                        return None;
                    }
                    pending.push((&a.body, &b.body));
                }
            }
            _ => return None,
        }
    }
    Some(equations)
}
