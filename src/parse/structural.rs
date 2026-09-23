//! The writable language of finite structural contracts. Captures use ordinary
//! type syntax; the body only describes shapes, projections, and calls.

use std::sync::Arc;

use super::*;
use crate::contracts::{Arm as ContractArm, Expr as ContractExpr, Pattern as ContractPattern};

type Bindings = Vec<(String, Arc<ContractExpr>)>;

impl Parser {
    pub(super) fn structural_type(&mut self) -> Option<Type> {
        let open = self.eat(&Kind::Fn)?;
        let mut bindings = Vec::new();
        let mut captures = Vec::new();
        let (parameters, body) = if self.at(&Kind::Pipe) {
            let scrutinee = Arc::new(ContractExpr::Input(0));
            let arms = self.structural_arms(&scrutinee, &mut bindings, &mut captures)?;
            (
                1,
                Arc::new(ContractExpr::Match {
                    scrutinee,
                    arms: arms.into(),
                }),
            )
        } else {
            while let Some(variable) = self.variable() {
                let index = bindings.len();
                bindings.push((variable.tracked, Arc::new(ContractExpr::Input(index))));
            }
            self.eat(&Kind::FatArrow)?;
            (
                bindings.len(),
                self.structural_expr(&mut bindings, &mut captures)?,
            )
        };
        let effect_sink = if self.at_plus() {
            let effects = self.effect_row()?;
            let at = effects.span;
            Some(Box::new(at.track(TypeKind::Arrow {
                from: Box::new(at.track(TypeKind::Unit)),
                to: Box::new(at.track(TypeKind::Unit)),
                effects: Some(Box::new(effects)),
            })))
        } else {
            None
        };
        let end = self.toks[self.pos - 1].span;
        Some(open.span.merge(end).track(TypeKind::Structural {
            parameters,
            body,
            captures,
            effect_sink,
        }))
    }

    fn structural_arms(
        &mut self,
        scrutinee: &Arc<ContractExpr>,
        bindings: &mut Bindings,
        captures: &mut Vec<Type>,
    ) -> Option<Vec<ContractArm>> {
        let mut arms = Vec::new();
        while self.eat_if(&Kind::Pipe).is_some() {
            let mark = bindings.len();
            let pattern = self.structural_pattern(scrutinee.clone(), bindings)?;
            self.eat(&Kind::FatArrow)?;
            let body = self.structural_expr(bindings, captures)?;
            bindings.truncate(mark);
            arms.push(ContractArm { pattern, body });
        }
        Some(arms)
    }

    fn at_structural_atom(&self) -> bool {
        matches!(
            self.peek().map(|token| &token.tracked),
            Some(
                Kind::Variable(_)
                    | Kind::Identifier(_)
                    | Kind::ColonColon
                    | Kind::Underscore
                    | Kind::LeftParen
                    | Kind::LeftBrace
                    | Kind::Tag(_)
                    | Kind::Match
                    | Kind::Do
            )
        ) && !self.at_keyword("where")
    }

    fn structural_expr(
        &mut self,
        bindings: &mut Bindings,
        captures: &mut Vec<Type>,
    ) -> Option<Arc<ContractExpr>> {
        let mut value = self.structural_project(bindings, captures)?;
        while self.at_structural_atom() {
            let argument = self.structural_project(bindings, captures)?;
            value = Arc::new(ContractExpr::Apply {
                function: value,
                argument,
            });
        }
        Some(value)
    }

    fn structural_project(
        &mut self,
        bindings: &mut Bindings,
        captures: &mut Vec<Type>,
    ) -> Option<Arc<ContractExpr>> {
        let mut value = self.structural_atom(bindings, captures)?;
        while self.eat_if(&Kind::Dot).is_some() {
            value = if let Some(label) = self.tag() {
                Arc::new(ContractExpr::Payload {
                    base: value,
                    label: label.tracked,
                })
            } else {
                let label = self.field_label()?;
                Arc::new(ContractExpr::Field {
                    base: value,
                    label: label.tracked,
                })
            };
        }
        Some(value)
    }

    fn structural_atom(
        &mut self,
        bindings: &mut Bindings,
        captures: &mut Vec<Type>,
    ) -> Option<Arc<ContractExpr>> {
        if let Some(variable) = self.variable() {
            if let Some((_, value)) = bindings
                .iter()
                .rev()
                .find(|(name, _)| *name == variable.tracked)
            {
                return Some(value.clone());
            }
            let index = captures.len();
            captures.push(variable.span.track(TypeKind::Variable { name: variable }));
            return Some(Arc::new(ContractExpr::Capture(index)));
        }
        if self.at_keyword("capture") {
            self.advance();
            self.eat(&Kind::LeftParen)?;
            let ty = self.type_expr()?;
            self.eat(&Kind::RightParen)?;
            let index = captures.len();
            captures.push(ty);
            return Some(Arc::new(ContractExpr::Capture(index)));
        }
        if self.eat_if(&Kind::Match).is_some() {
            let scrutinee = self.structural_expr(bindings, captures)?;
            self.eat(&Kind::With)?;
            let arms = self.structural_arms(&scrutinee, bindings, captures)?;
            self.eat(&Kind::End)?;
            return Some(Arc::new(ContractExpr::Match {
                scrutinee,
                arms: arms.into(),
            }));
        }
        if self.eat_if(&Kind::Do).is_some() {
            let mark = bindings.len();
            let mut initializers = Vec::new();
            while !self.at(&Kind::Return) {
                let name = if self.eat_if(&Kind::Let).is_some() {
                    Some(self.variable().or_else(|| self.expected(Expected::Name))?)
                } else {
                    self.eat(&Kind::Underscore)?;
                    None
                };
                self.eat(&Kind::Equal)?;
                let value = self.structural_expr(bindings, captures)?;
                let force = name.is_none();
                if let Some(name) = name {
                    bindings.push((name.tracked, value.clone()));
                }
                initializers.push((value, force));
                self.eat_if(&Kind::Semicolon);
            }
            self.eat(&Kind::Return)?;
            let mut body = self.structural_expr(bindings, captures)?;
            self.eat(&Kind::End)?;
            bindings.truncate(mark);
            for (value, force) in initializers.into_iter().rev() {
                let mut seen = std::collections::HashSet::new();
                let mut pending = vec![&body];
                let mut used = false;
                while let Some(expr) = pending.pop() {
                    if Arc::ptr_eq(expr, &value) {
                        used = true;
                        break;
                    }
                    if seen.insert(Arc::as_ptr(expr)) {
                        expr.children(&mut pending);
                    }
                }
                if force || !used {
                    body = Arc::new(ContractExpr::Then { value, body });
                }
            }
            return Some(body);
        }
        if self.eat_if(&Kind::LeftBrace).is_some() {
            let mut fields = Vec::new();
            let mut spread = None;
            while !self.at(&Kind::RightBrace) {
                if self.eat_if(&Kind::DotDot).is_some() {
                    spread = Some(self.structural_expr(bindings, captures)?);
                    self.eat_if(&Kind::Comma);
                    break;
                }
                let label = self.field_label()?;
                self.eat(&Kind::Colon)?;
                fields.push((label.tracked, self.structural_expr(bindings, captures)?));
                if self.eat_if(&Kind::Comma).is_none() {
                    break;
                }
            }
            self.eat(&Kind::RightBrace)?;
            return Some(Arc::new(ContractExpr::Record {
                fields: fields.into(),
                spread,
            }));
        }
        if self.eat_if(&Kind::LeftParen).is_some() {
            if self.eat_if(&Kind::RightParen).is_some() {
                return Some(Self::structural_unit());
            }
            let first = self.structural_expr(bindings, captures)?;
            if self.eat_if(&Kind::Comma).is_none() {
                self.eat(&Kind::RightParen)?;
                return Some(first);
            }
            let mut fields = vec![("0".into(), first)];
            while !self.at(&Kind::RightParen) {
                fields.push((
                    fields.len().to_string(),
                    self.structural_expr(bindings, captures)?,
                ));
                if self.eat_if(&Kind::Comma).is_none() {
                    break;
                }
            }
            self.eat(&Kind::RightParen)?;
            return Some(Arc::new(ContractExpr::Record {
                fields: fields.into(),
                spread: None,
            }));
        }
        if let Some(label) = self.tag() {
            let payload = if self.at_structural_atom() {
                self.structural_project(bindings, captures)?
            } else {
                Self::structural_unit()
            };
            return Some(Arc::new(ContractExpr::Tag {
                label: label.tracked,
                payload,
            }));
        }
        let ty = self.type_atom()?;
        let index = captures.len();
        captures.push(ty);
        Some(Arc::new(ContractExpr::Capture(index)))
    }

    fn structural_unit() -> Arc<ContractExpr> {
        Arc::new(ContractExpr::Record {
            fields: Arc::new([]),
            spread: None,
        })
    }

    fn at_structural_pattern(&self) -> bool {
        matches!(
            self.peek().map(|token| &token.tracked),
            Some(
                Kind::Variable(_)
                    | Kind::Underscore
                    | Kind::LeftParen
                    | Kind::LeftBrace
                    | Kind::Tag(_)
            )
        )
    }

    fn structural_pattern(
        &mut self,
        base: Arc<ContractExpr>,
        bindings: &mut Bindings,
    ) -> Option<ContractPattern> {
        if self.eat_if(&Kind::Underscore).is_some() {
            return Some(ContractPattern::Any);
        }
        if let Some(variable) = self.variable() {
            bindings.push((variable.tracked, base));
            return Some(ContractPattern::Any);
        }
        if let Some(label) = self.tag() {
            let payload = if self.at_structural_pattern() {
                self.structural_pattern(
                    Arc::new(ContractExpr::Payload {
                        base,
                        label: label.tracked.clone(),
                    }),
                    bindings,
                )?
            } else {
                ContractPattern::Record {
                    fields: Arc::new([]),
                    open: false,
                }
            };
            return Some(ContractPattern::Tag {
                label: label.tracked,
                payload: Box::new(payload),
            });
        }
        if self.eat_if(&Kind::LeftBrace).is_some() {
            let mut fields = Vec::new();
            let mut open = false;
            while !self.at(&Kind::RightBrace) {
                if self.eat_if(&Kind::DotDot).is_some() {
                    open = true;
                    break;
                }
                let label = self.field_label()?;
                self.eat(&Kind::Colon)?;
                let field = Arc::new(ContractExpr::Field {
                    base: base.clone(),
                    label: label.tracked.clone(),
                });
                fields.push((label.tracked, self.structural_pattern(field, bindings)?));
                if self.eat_if(&Kind::Comma).is_none() {
                    break;
                }
            }
            self.eat(&Kind::RightBrace)?;
            return Some(ContractPattern::Record {
                fields: fields.into(),
                open,
            });
        }
        if self.eat_if(&Kind::LeftParen).is_some() {
            if self.eat_if(&Kind::RightParen).is_some() {
                return Some(ContractPattern::Record {
                    fields: Arc::new([]),
                    open: false,
                });
            }
            // Delay choosing grouping versus a tuple until its comma. A group
            // binds against the original base; tuples bind against slot zero.
            let mark = bindings.len();
            let start = self.pos;
            let first = self.structural_pattern(base.clone(), bindings)?;
            if self.eat_if(&Kind::Comma).is_none() {
                self.eat(&Kind::RightParen)?;
                return Some(first);
            }
            bindings.truncate(mark);
            self.pos = start;
            let mut fields = Vec::new();
            let mut open = false;
            while !self.at(&Kind::RightParen) {
                if self.eat_if(&Kind::DotDot).is_some() {
                    open = true;
                    break;
                }
                let label = fields.len().to_string();
                let field = Arc::new(ContractExpr::Field {
                    base: base.clone(),
                    label: label.clone(),
                });
                fields.push((label, self.structural_pattern(field, bindings)?));
                if self.eat_if(&Kind::Comma).is_none() {
                    break;
                }
            }
            self.eat(&Kind::RightParen)?;
            return Some(ContractPattern::Record {
                fields: fields.into(),
                open,
            });
        }
        self.expected(Expected::Pattern)
    }
}
