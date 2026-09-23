//! Bounded normalization of finite structural contracts.
//!
//! The surrounding inference solver owns ordinary type compatibility, row
//! constraints, effects, and diagnostics. This module owns substitution and
//! structural control flow. It never looks up or unfolds a value definition.

use std::{collections::HashMap, collections::HashSet, fmt, sync::Arc};

use crate::{
    contracts::{Contract, Expr, Pattern},
    types::Ty,
};

/// One nonempty portion of a match input and the first arm that accepts it.
#[derive(Debug, Clone)]
pub struct Case {
    pub arm: usize,
    pub input: Arc<Ty>,
}

#[derive(Debug, Clone)]
pub enum MatchCases {
    /// Cases must cover the input. A missing possible case is a solver error,
    /// never a reason to silently discard an alternative.
    Known(Vec<Case>),
    /// The input is symbolic and its shape cannot yet select structural arms.
    Deferred,
}

#[derive(Debug, Clone)]
pub enum Outcome {
    Ready(Arc<Ty>),
    /// A saturated contract whose remaining structural tests are symbolic.
    /// The caller retains it beside an appropriate ordinary result fallback.
    Deferred(Arc<Contract>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    MalformedContract,
    RecursiveContract,
    WorkLimit,
    DepthLimit,
    UncoveredMatch,
}

impl fmt::Display for Failure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::MalformedContract => "invalid structural contract",
            Self::RecursiveContract => {
                "structural contract application re-enters an active contract"
            }
            Self::WorkLimit => "structural contract normalization exceeds its finite work budget",
            Self::DepthLimit => "structural contract normalization exceeds its nesting limit",
            Self::UncoveredMatch => "structural contract match does not cover its input",
        })
    }
}

/// Ordinary inference operations used by the structural normalizer.
pub trait Context {
    type Error;

    fn resolve_type(&mut self, ty: &Arc<Ty>) -> Arc<Ty>;

    /// Apply an ordinary arrow, checking its argument and recording effects.
    fn ordinary_apply(
        &mut self,
        function: Arc<Ty>,
        argument: Arc<Ty>,
    ) -> Result<Arc<Ty>, Self::Error>;

    /// Retain shared ordinary payload equations while structural evaluation
    /// decides which labels are required or produced.
    fn contract_argument(
        &mut self,
        function: &Arc<Ty>,
        argument: &Arc<Ty>,
    ) -> Result<(), Self::Error>;

    fn project(&mut self, base: Arc<Ty>, label: &str) -> Result<Arc<Ty>, Self::Error>;
    fn payload(&mut self, base: Arc<Ty>, label: &str) -> Result<Arc<Ty>, Self::Error>;

    /// Construct or update a record. Explicit fields replace spread fields.
    fn record(
        &mut self,
        fields: Vec<(String, Arc<Ty>)>,
        spread: Option<Arc<Ty>>,
    ) -> Result<Arc<Ty>, Self::Error>;

    fn tag(&mut self, label: &str, payload: Arc<Ty>) -> Result<Arc<Ty>, Self::Error>;

    /// Partition known shapes using first-match semantics. The implementation
    /// must report uncovered inputs and preserve every reachable alternative.
    /// A tag's admission and the selected runtime tag are different facts.
    fn split_match(
        &mut self,
        input: Arc<Ty>,
        patterns: &[Pattern],
    ) -> Result<MatchCases, Self::Error>;

    /// Install the selected case's presence assumptions. A false result means
    /// existing constraints prove the case unreachable. Every successful entry
    /// is paired with `end_case`, including failed or deferred computations.
    fn begin_case(&mut self, _input: &Arc<Ty>) -> Result<bool, Self::Error> {
        Ok(true)
    }
    fn end_case(&mut self) {}

    fn join(&mut self, alternatives: Vec<(Arc<Ty>, Arc<Ty>)>) -> Result<Arc<Ty>, Self::Error>;
    fn failure(&mut self, failure: Failure) -> Self::Error;

    /// Current call's effect destination, represented as `() -> () + E` so it
    /// participates in ordinary type generalization and substitution.
    fn effect_sink(&mut self) -> Arc<Ty>;
    fn set_effect_sink(&mut self, sink: Arc<Ty>);
    /// A written or retained sink may not authorize effects outside the
    /// current call's ambient. Relate it before temporarily installing it.
    fn link_effect_sink(&mut self, sink: &Arc<Ty>) -> Result<(), Self::Error>;

    /// Specialize only the runtime result layout; never add compatibility
    /// equations for the structural contract's semantic parameters.
    fn applied_fallback(&mut self, function: &Arc<Ty>, _argument: &Arc<Ty>) -> Arc<Ty> {
        self.result_fallback(function)
    }

    /// Advance the representation fallback without equating its parameter
    /// with the argument: that equation would reintroduce the old contract.
    /// Structural evaluation itself records the effects of executed calls.
    fn result_fallback(&mut self, function: &Arc<Ty>) -> Arc<Ty> {
        let mut function = function.clone();
        loop {
            function = self.resolve_type(&function);
            match &*function {
                Ty::Arrow(_, result, _) => return result.clone(),
                Ty::Contract { fallback, .. } => function = fallback.clone(),
                _ => return Arc::new(Ty::Undecided),
            }
        }
    }
}

/// Apply one argument. Each invocation uses its own finite normalization state.
pub fn apply<C: Context>(
    context: &mut C,
    function: Arc<Ty>,
    argument: Arc<Ty>,
) -> Result<Outcome, C::Error> {
    Evaluator {
        context,
        remaining: 32_768,
        active: HashSet::new(),
    }
    .apply(function, argument, 0)
}

/// Resolve a saturated result contract when its inputs have become known.
pub fn normalize<C: Context>(context: &mut C, ty: Arc<Ty>) -> Result<Outcome, C::Error> {
    Evaluator {
        context,
        remaining: 32_768,
        active: HashSet::new(),
    }
    .value(ty, 0)
}

/// Check the complete arrow obligations of an explicit match annotation.
///
/// Inputs must already contain the annotation's rigid variables and row
/// assumptions. `false` means this finite check cannot establish conformance;
/// callers must not turn it into success by checking only erased fallbacks.
/// For overlapping ordered arms this deliberately checks the stronger complete
/// arrows, so it can refuse a valid annotation instead of weakening a promise.
pub fn check_annotation_cases<C: Context>(
    context: &mut C,
    actual: Arc<Ty>,
    expected: &Contract,
) -> Result<bool, C::Error> {
    let Some(cases) = expected.annotation_cases() else {
        return Ok(false);
    };
    for (input, output) in cases {
        let result = match apply(context, actual.clone(), input)? {
            Outcome::Ready(result) => result,
            Outcome::Deferred(_) => return Ok(false),
        };
        // An ordinary checker arrow asks the existing solver exactly whether
        // the inferred result satisfies the promised output. It introduces no
        // new structural reduction or annotation inference rule.
        let checker = Arc::new(Ty::Arrow(
            output,
            Arc::new(Ty::unit()),
            crate::types::Row::closed(),
        ));
        let _ = context.ordinary_apply(checker, result)?;
    }
    Ok(true)
}

struct Evaluator<'a, C> {
    context: &'a mut C,
    remaining: usize,
    /// Re-entry is conservatively rejected, even at a different input shape.
    active: HashSet<usize>,
}

#[derive(Default, Clone)]
struct Frame {
    refined: HashMap<usize, Arc<Ty>>,
    memo: HashMap<usize, Arc<Ty>>,
}

#[derive(Clone)]
struct Scope {
    contract: Arc<Contract>,
    frame: usize,
}

struct Children {
    expr: Arc<Expr>,
    scope: Scope,
    pending: std::vec::IntoIter<Arc<Expr>>,
    values: Vec<Arc<Ty>>,
}

struct Cases {
    scrutinee: Arc<Expr>,
    arms: Arc<[crate::contracts::Arm]>,
    scope: Scope,
    pending: std::vec::IntoIter<Case>,
    values: Vec<(Arc<Ty>, Arc<Ty>)>,
}

/// Every continuation is heap owned. A long chain of finite summaries consumes
/// the same bounded work as before, without borrowing the Rust call stack.
/// Graph validation bounds each written expression; the work budget bounds
/// composition across summaries, and active bodies prohibit recursive calls.
enum Work {
    Apply(Arc<Ty>, Arc<Ty>),
    Value(Arc<Ty>),
    Evaluate(Arc<Contract>),
    Expression(Arc<Expr>, Scope),
    Children(Children),
    Child(Children),
    Cases(Cases),
    Case(Cases, Arc<Ty>),
    AppliedSaturated(Arc<Ty>, Arc<Ty>),
    AppliedSupplied(Arc<Contract>),
    EvaluatedValue(Arc<Contract>),
    OutcomeExpression,
    NormalizeExpression(usize, usize),
    MemoExpression(usize, usize),
    Leave,
}

enum Cleanup {
    Evaluation { key: usize, sink: Option<Arc<Ty>> },
    Case,
}

impl<C: Context> Evaluator<'_, C> {
    fn step(&mut self) -> Result<(), C::Error> {
        self.remaining = self
            .remaining
            .checked_sub(1)
            .ok_or_else(|| self.context.failure(Failure::WorkLimit))?;
        Ok(())
    }

    fn apply(
        &mut self,
        function: Arc<Ty>,
        argument: Arc<Ty>,
        _depth: usize,
    ) -> Result<Outcome, C::Error> {
        self.run(Work::Apply(function, argument))
    }

    fn value(&mut self, ty: Arc<Ty>, _depth: usize) -> Result<Outcome, C::Error> {
        self.run(Work::Value(ty))
    }

    fn leave(&mut self, cleanup: Cleanup) {
        match cleanup {
            Cleanup::Evaluation { key, sink } => {
                if let Some(sink) = sink {
                    self.context.set_effect_sink(sink);
                }
                self.active.remove(&key);
            }
            Cleanup::Case => self.context.end_case(),
        }
    }

    fn run(&mut self, initial: Work) -> Result<Outcome, C::Error> {
        let mut work = vec![initial];
        let mut values: Vec<Option<Arc<Ty>>> = Vec::new();
        let mut outcomes: Vec<Outcome> = Vec::new();
        let mut frames: Vec<Frame> = Vec::new();
        let mut cleanups = Vec::new();
        let result = (|| {
            while let Some(part) = work.pop() {
                self.step()?;
                match part {
                    Work::Apply(function, argument) => {
                        let function = self.context.resolve_type(&function);
                        let argument = self.context.resolve_type(&argument);
                        match &*function {
                            Ty::Contract { fallback, contract } => {
                                if !contract.validate() {
                                    return Err(self.context.failure(Failure::MalformedContract));
                                }
                                if contract.arguments.len() == contract.parameters {
                                    work.push(Work::AppliedSaturated(function.clone(), argument));
                                    work.push(Work::Evaluate(contract.clone()));
                                } else {
                                    self.context.contract_argument(fallback, &argument)?;
                                    let mut arguments = contract.arguments.to_vec();
                                    arguments.push(argument.clone());
                                    let supplied = Arc::new(Contract {
                                        arguments: arguments.into(),
                                        ..(**contract).clone()
                                    });
                                    if supplied.arguments.len() < supplied.parameters {
                                        outcomes.push(Outcome::Ready(Arc::new(Ty::Contract {
                                            fallback: self
                                                .context
                                                .applied_fallback(fallback, &argument),
                                            contract: supplied,
                                        })));
                                    } else {
                                        work.push(Work::AppliedSupplied(supplied.clone()));
                                        work.push(Work::Evaluate(supplied));
                                    }
                                }
                            }
                            _ => outcomes.push(Outcome::Ready(
                                self.context.ordinary_apply(function, argument)?,
                            )),
                        }
                    }
                    Work::AppliedSaturated(function, argument) => {
                        match values.pop().expect("evaluated callee") {
                            Some(function) => work.push(Work::Apply(function, argument)),
                            None => outcomes.push(Outcome::Deferred(deferred_application(
                                function,
                                argument,
                                self.context.effect_sink(),
                            ))),
                        }
                    }
                    Work::AppliedSupplied(contract) => {
                        outcomes.push(match values.pop().expect("evaluated application") {
                            Some(ty) => Outcome::Ready(ty),
                            None => Outcome::Deferred(self.retain_sink(&contract)),
                        })
                    }
                    Work::Value(ty) => {
                        let ty = self.context.resolve_type(&ty);
                        if let Ty::Contract { contract, .. } = &*ty
                            && contract.arguments.len() == contract.parameters
                        {
                            work.push(Work::EvaluatedValue(contract.clone()));
                            work.push(Work::Evaluate(contract.clone()));
                        } else {
                            outcomes.push(Outcome::Ready(ty));
                        }
                    }
                    Work::EvaluatedValue(contract) => {
                        match values.pop().expect("evaluated value") {
                            Some(ty) => work.push(Work::Value(ty)),
                            None => outcomes.push(Outcome::Deferred(self.retain_sink(&contract))),
                        }
                    }
                    Work::Evaluate(contract) => {
                        if !contract.validate() || contract.arguments.len() != contract.parameters {
                            return Err(self.context.failure(Failure::MalformedContract));
                        }
                        let key = Arc::as_ptr(&contract.body) as usize;
                        if !self.active.insert(key) {
                            return Err(self.context.failure(Failure::RecursiveContract));
                        }
                        let sink = if let Some(sink) = &contract.effect_sink {
                            if let Err(error) = self.context.link_effect_sink(sink) {
                                self.active.remove(&key);
                                return Err(error);
                            }
                            let previous = self.context.effect_sink();
                            self.context.set_effect_sink(sink.clone());
                            Some(previous)
                        } else {
                            None
                        };
                        cleanups.push(Cleanup::Evaluation { key, sink });
                        let frame = frames.len();
                        frames.push(Frame::default());
                        work.push(Work::Leave);
                        work.push(Work::Expression(
                            contract.body.clone(),
                            Scope { contract, frame },
                        ));
                    }
                    Work::Leave => {
                        self.leave(cleanups.pop().expect("normalization scope"));
                        frames.pop().expect("normalization frame");
                    }
                    Work::Expression(expr, scope) => {
                        let key = Arc::as_ptr(&expr) as usize;
                        if let Some(ty) = frames[scope.frame]
                            .refined
                            .get(&key)
                            .or_else(|| frames[scope.frame].memo.get(&key))
                        {
                            values.push(Some(ty.clone()));
                            continue;
                        }
                        work.push(Work::NormalizeExpression(scope.frame, key));
                        match &*expr {
                            Expr::Input(index) => {
                                values.push(Some(scope.contract.arguments[*index].clone()))
                            }
                            Expr::Capture(index) => {
                                values.push(Some(scope.contract.captures[*index].clone()))
                            }
                            _ => {
                                let mut children = Vec::new();
                                if let Expr::Match { scrutinee, .. } = &*expr {
                                    children.push(scrutinee);
                                } else {
                                    expr.children(&mut children);
                                }
                                let pending = children
                                    .into_iter()
                                    .cloned()
                                    .collect::<Vec<_>>()
                                    .into_iter();
                                work.push(Work::Children(Children {
                                    expr,
                                    scope,
                                    pending,
                                    values: Vec::new(),
                                }));
                            }
                        }
                    }
                    Work::Children(mut children) => {
                        if let Some(child) = children.pending.next() {
                            let scope = children.scope.clone();
                            work.push(Work::Child(children));
                            work.push(Work::Expression(child, scope));
                            continue;
                        }
                        let mut inputs = children.values.into_iter();
                        let result = match &*children.expr {
                            Expr::Field { label, .. } => self
                                .context
                                .project(inputs.next().expect("field base"), label)?,
                            Expr::Payload { label, .. } => self
                                .context
                                .payload(inputs.next().expect("tag base"), label)?,
                            Expr::Record { fields, spread } => {
                                let fields = fields
                                    .iter()
                                    .map(|(label, _)| {
                                        (label.clone(), inputs.next().expect("record field"))
                                    })
                                    .collect();
                                let spread = spread
                                    .as_ref()
                                    .map(|_| inputs.next().expect("record spread"));
                                self.context.record(fields, spread)?
                            }
                            Expr::Tag { label, .. } => self
                                .context
                                .tag(label, inputs.next().expect("tag payload"))?,
                            Expr::Apply { .. } => {
                                work.push(Work::OutcomeExpression);
                                work.push(Work::Apply(
                                    inputs.next().expect("function"),
                                    inputs.next().expect("argument"),
                                ));
                                continue;
                            }
                            Expr::Then { .. } => inputs.nth(1).expect("sequenced result"),
                            Expr::Match { scrutinee, arms } => {
                                let patterns: Vec<_> =
                                    arms.iter().map(|arm| arm.pattern.clone()).collect();
                                match self
                                    .context
                                    .split_match(inputs.next().expect("match input"), &patterns)?
                                {
                                    MatchCases::Deferred => values.push(None),
                                    MatchCases::Known(cases) => work.push(Work::Cases(Cases {
                                        scrutinee: scrutinee.clone(),
                                        arms: arms.clone(),
                                        scope: children.scope,
                                        pending: cases.into_iter(),
                                        values: Vec::new(),
                                    })),
                                }
                                continue;
                            }
                            Expr::Input(_) | Expr::Capture(_) => {
                                unreachable!("leaves have no child continuation")
                            }
                        };
                        values.push(Some(result));
                    }
                    Work::Child(mut children) => match values.pop().expect("evaluated child") {
                        Some(ty) => {
                            children.values.push(ty);
                            work.push(Work::Children(children));
                        }
                        None => values.push(None),
                    },
                    Work::Cases(mut cases) => {
                        let mut selected = None;
                        for case in cases.pending.by_ref() {
                            if case.arm >= cases.arms.len() {
                                return Err(self.context.failure(Failure::MalformedContract));
                            }
                            if self.context.begin_case(&case.input)? {
                                selected = Some(case);
                                break;
                            }
                        }
                        if let Some(case) = selected {
                            cleanups.push(Cleanup::Case);
                            let frame = frames.len();
                            frames.push(Frame {
                                refined: frames[cases.scope.frame].refined.clone(),
                                memo: HashMap::new(),
                            });
                            self.refine(&cases.scrutinee, case.input.clone(), &mut frames[frame])?;
                            let body = cases.arms[case.arm].body.clone();
                            let scope = Scope {
                                contract: cases.scope.contract.clone(),
                                frame,
                            };
                            work.push(Work::Case(cases, case.input));
                            work.push(Work::Leave);
                            work.push(Work::Expression(body, scope));
                        } else {
                            values.push(Some(self.context.join(cases.values)?));
                        }
                    }
                    Work::Case(mut cases, input) => match values.pop().expect("evaluated case") {
                        Some(ty) => {
                            cases.values.push((input, ty));
                            work.push(Work::Cases(cases));
                        }
                        None => values.push(None),
                    },
                    Work::OutcomeExpression => {
                        values.push(match outcomes.pop().expect("expression application") {
                            Outcome::Ready(ty) => Some(ty),
                            Outcome::Deferred(_) => None,
                        })
                    }
                    Work::NormalizeExpression(frame, key) => {
                        match values.pop().expect("expression value") {
                            Some(ty) => {
                                work.push(Work::MemoExpression(frame, key));
                                work.push(Work::Value(ty));
                            }
                            None => values.push(None),
                        }
                    }
                    Work::MemoExpression(frame, key) => {
                        match outcomes.pop().expect("normalized expression") {
                            Outcome::Ready(ty) => {
                                frames[frame].memo.insert(key, ty.clone());
                                values.push(Some(ty));
                            }
                            Outcome::Deferred(_) => values.push(None),
                        }
                    }
                }
            }
            Ok(outcomes.pop().expect("normalization result"))
        })();
        // Both failed and deferred branches restore every entered case and
        // effect destination; callers may continue checking after a failure.
        for cleanup in cleanups.into_iter().rev() {
            self.leave(cleanup);
        }
        result
    }

    fn retain_sink(&mut self, contract: &Contract) -> Arc<Contract> {
        Arc::new(Contract {
            effect_sink: Some(
                contract
                    .effect_sink
                    .clone()
                    .unwrap_or_else(|| self.context.effect_sink()),
            ),
            ..contract.clone()
        })
    }

    /// Project a refined constructed scrutinee back to its input DAG leaves.
    fn refine(
        &mut self,
        expr: &Arc<Expr>,
        input: Arc<Ty>,
        frame: &mut Frame,
    ) -> Result<(), C::Error> {
        let mut work = vec![(expr.clone(), input)];
        while let Some((expr, input)) = work.pop() {
            self.step()?;
            frame
                .refined
                .insert(Arc::as_ptr(&expr) as usize, input.clone());
            match &*expr {
                Expr::Record { fields, .. } => {
                    for (label, value) in fields.iter().rev() {
                        let projected = self.context.project(input.clone(), label)?;
                        work.push((value.clone(), projected));
                    }
                }
                Expr::Tag { label, payload } => {
                    let projected = self.context.payload(input, label)?;
                    work.push((payload.clone(), projected));
                }
                _ => {}
            }
        }
        Ok(())
    }
}

fn deferred_application(
    function: Arc<Ty>,
    argument: Arc<Ty>,
    effect_sink: Arc<Ty>,
) -> Arc<Contract> {
    Arc::new(Contract {
        parameters: 1,
        body: Arc::new(Expr::Apply {
            function: Arc::new(Expr::Capture(0)),
            argument: Arc::new(Expr::Input(0)),
        }),
        captures: Arc::from([function]),
        arguments: Arc::from([argument]),
        effect_sink: Some(effect_sink),
    })
}
