//! Conservative callable summaries, produced while each bundle is independent.
//! Unknown higher-order calls remain potentially suspending. This analysis never
//! uses an escaping effect row as a non-suspension proof.
use super::*;
use std::collections::HashMap;

#[derive(Clone, Copy)]
enum Target {
    Local(FuncId),
    Imported(Suspension),
}

pub(super) fn summarize(output: &mut Output, accepted: &AcceptedProgram) {
    let mut targets = HashMap::new();
    for f in &output.functions {
        for i in f.blocks.iter().flat_map(|b| &b.instrs) {
            if let Op::Closure { func, .. } = i.op {
                targets.insert(i.temp, Target::Local(func));
            }
        }
    }
    let mut globals = HashMap::new();
    for g in &output.globals {
        let f = &output.functions[g.initializer];
        // A direct closure initializer publishes its callable summary. A value
        // computed by arbitrary control flow is conservatively unknown.
        if f.blocks.len() == 1
            && let End::Continue { value, .. } = f.blocks[0].end.kind
            && let Some(target) = targets.get(&value)
        {
            globals.insert(g.symbol, *target);
        }
    }
    for f in &output.functions {
        for i in f.blocks.iter().flat_map(|b| &b.instrs) {
            if let Op::Global { symbol, .. } = i.op {
                let target = globals.get(&symbol).copied().or_else(|| {
                    accepted
                        .mint()
                        .external(symbol)
                        .and_then(|name| accepted.imported_suspension(name))
                        .map(Target::Imported)
                });
                if let Some(target) = target {
                    targets.insert(i.temp, target);
                }
            }
        }
    }
    let mut may = vec![false; output.functions.len()];
    loop {
        let mut changed = false;
        for (index, f) in output.functions.iter().enumerate() {
            if may[index] {
                continue;
            }
            let suspends = f.blocks.iter().any(|b| match &b.end.kind {
                End::RawCall { completion, .. } => {
                    *completion != crate::externs::Completion::Immediate
                }
                End::Call { callee, .. } => {
                    let target = match callee {
                        Callee::Direct(f) => Some(Target::Local(*f)),
                        Callee::Indirect(t) => targets.get(t).copied(),
                    };
                    match target {
                        Some(Target::Local(f)) => may[f],
                        Some(Target::Imported(s)) => s == Suspension::MaySuspend,
                        None => true,
                    }
                }
                _ => false,
            });
            if suspends {
                may[index] = true;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    for (f, may) in output.functions.iter_mut().zip(&may) {
        f.suspension = if *may {
            Suspension::MaySuspend
        } else {
            Suspension::Synchronous
        };
    }
    for g in &mut output.globals {
        g.adapter = accepted
            .ir()
            .terms
            .get(&g.symbol)
            .and_then(|d| crate::externs::export_request(&d.metadata).ok().flatten());
        g.callable = globals.get(&g.symbol).map(|target| match target {
            Target::Local(f) => output.functions[*f].suspension,
            Target::Imported(s) => *s,
        });
    }
    let summaries: Vec<_> = output.functions.iter().map(|f| f.suspension).collect();
    for i in output
        .functions
        .iter_mut()
        .flat_map(|f| &mut f.blocks)
        .flat_map(|b| &mut b.instrs)
    {
        if let Op::Global { callable, .. } = &mut i.op {
            *callable = targets.get(&i.temp).map(|target| match target {
                Target::Local(f) => summaries[*f],
                Target::Imported(s) => *s,
            });
        }
    }
}
