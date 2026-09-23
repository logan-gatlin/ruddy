//! Generation-time checks for structural summaries.

use super::*;

impl Constrain<'_> {
    /// Check the exact finite synthesis grammar before retaining a summary. Captured types and local annotation promises are dummy
    /// operands here; their checked types are installed by the ordinary walk.
    pub(super) fn can_summarize(&self, parameter: Symbol, body: &Term) -> bool {
        if crate::contracts::references_any(body, self.group_members) {
            return false;
        }
        let mut annotations = HashMap::new();
        let mut pending = vec![body];
        while let Some(term) = pending.pop() {
            match &term.kind {
                TermKind::Let {
                    name,
                    annotation,
                    value,
                    body,
                } => {
                    if annotation.is_some() {
                        annotations.insert(name.anchored, Arc::new(Ty::Undecided));
                    }
                    pending.push(value);
                    pending.push(body);
                }
                TermKind::Fn { body, .. } | TermKind::Unary { value: body, .. } => {
                    pending.push(body)
                }
                TermKind::Apply { func, arg } => {
                    pending.push(func);
                    pending.push(arg);
                }
                TermKind::Binary { left, right, .. } => {
                    pending.push(left);
                    pending.push(right);
                }
                TermKind::Match { scrutinee, arms } => {
                    pending.push(scrutinee);
                    pending.extend(arms.iter().map(|(_, body)| body));
                }
                TermKind::Struct { fields, spread } => {
                    pending.extend(fields.values().map(|field| &field.value));
                    pending.extend(spread.iter().map(|spread| &*spread.value));
                }
                TermKind::Array(items) => pending.extend(items.iter().map(|item| &item.value)),
                TermKind::Tag { payload, .. } => {
                    pending.extend(payload.iter().map(|payload| &**payload))
                }
                TermKind::Project { base, .. } => pending.push(base),
                _ => {}
            }
        }
        crate::contracts::synthesize_lambda_with_annotations(
            parameter,
            body,
            &annotations,
            &mut || Arc::new(Ty::Undecided),
            &|symbol| match self.env.get(symbol) {
                Some(Binding::Poly(known)) => known.scheme.formula().is_true(),
                Some(Binding::Local) => false,
                Some(Binding::Mono(ty)) => !matches!(&*self.table.resolve(ty), Ty::Var(variable)
                    if matches!(self.table.var_meta[*variable as usize].subject,
                        Subject::TopLevelBinding | Subject::LocalBinding)),
                None => true,
            },
        )
        .is_some()
    }
}
