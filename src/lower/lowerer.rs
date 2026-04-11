use std::collections::{HashMap, HashSet};

use crate::parser::ast;
use crate::reporting::{Diag, Diagnostic, TextRange};
use crate::resolver::ResolverDispatch;
use salsa::Accumulator;

use super::query::lower_module_query;
use super::*;

impl<'db> ModuleLowerer<'db> {
    pub(super) fn new(
        db: &'db dyn salsa::Database,
        request: ModuleRequest<'db>,
        source: Source,
        source_canon: String,
        module_path: Vec<String>,
        resolver: ResolverDispatch,
    ) -> Self {
        let source_contents = source.contents(db).clone();
        let bundle_name = module_path
            .first()
            .cloned()
            .unwrap_or_else(|| "_".to_owned());
        let mut module_requests = HashMap::new();
        module_requests.insert(module_path.join(PATH_SEP), request);

        Self {
            db,
            request,
            source_canon,
            source_contents,
            module_path,
            bundle_name,
            resolver,
            scope: ScopeState::default(),
            opened_modules: Vec::new(),
            module_aliases: HashMap::new(),
            module_requests,
            children: Vec::new(),
            wasm_scope: WasmModuleScope::default(),
            next_local_id: 0,
        }
    }

    pub(super) fn lower_statements(&mut self, statements: &[ast::Statement]) -> Vec<ir::Statement> {
        self.wasm_scope = self.collect_wasm_module_scope(statements);

        let mut lowered = Vec::new();
        for statement in statements {
            if let Some(statement) = self.lower_statement(statement) {
                lowered.push(statement);
            }
        }
        lowered
    }

    fn lower_statement(&mut self, statement: &ast::Statement) -> Option<ir::Statement> {
        match statement {
            ast::Statement::Bundle { .. } => None,
            ast::Statement::Module { name, range, .. } => {
                let Some(name_text) = self.opt_identifier_text_non_hole(name, "module") else {
                    self.error(*range, "expected module declaration name");
                    return Some(ir::Statement::Error(ir::ErrorNode { range: *range }));
                };

                let module_path = self.child_module_path(&name_text, *range);
                let module_path_text = module_path.text();
                let child_request = ModuleRequest::new(
                    self.db,
                    InternedString::new(self.db, module_path_text.clone()),
                    InternedString::new(self.db, self.source_canon.clone()),
                    self.request.file_root_path(self.db),
                    self.request.root_module_path(self.db),
                    self.request.root_source_canon(self.db),
                    self.request.resolver(self.db),
                    self.request.root_source(self.db),
                );

                self.register_module_decl(
                    name_text.clone(),
                    module_path.clone(),
                    child_request,
                    *range,
                );
                let _ = lower_module_query(self.db, child_request);

                Some(ir::Statement::ModuleDecl {
                    name: name_text,
                    module: module_path,
                    range: *range,
                })
            }
            ast::Statement::ModuleRef {
                name,
                in_loc,
                range,
            } => {
                let Some(name_text) = self.opt_identifier_text_non_hole(name, "module") else {
                    self.error(*range, "expected module reference name");
                    return Some(ir::Statement::Error(ir::ErrorNode { range: *range }));
                };

                let module_path = self.child_module_path(&name_text, *range);
                let module_path_text = module_path.text();
                let resolved_source = self.resolve_module_source_ref(&name_text, in_loc, *range);

                let source_canon = resolved_source.unwrap_or_else(|| self.source_canon.clone());
                let child_request = ModuleRequest::new(
                    self.db,
                    InternedString::new(self.db, module_path_text.clone()),
                    InternedString::new(self.db, source_canon),
                    InternedString::new(self.db, module_path_text),
                    self.request.root_module_path(self.db),
                    self.request.root_source_canon(self.db),
                    self.request.resolver(self.db),
                    None,
                );

                self.register_module_decl(
                    name_text.clone(),
                    module_path.clone(),
                    child_request,
                    *range,
                );
                let _ = lower_module_query(self.db, child_request);

                Some(ir::Statement::ModuleDecl {
                    name: name_text,
                    module: module_path,
                    range: *range,
                })
            }
            ast::Statement::Use {
                target,
                alias,
                range,
            } => {
                let mut opened_modules = self.opened_modules.clone();
                let mut module_aliases = self.module_aliases.clone();
                self.apply_use(
                    target,
                    alias,
                    *range,
                    &mut opened_modules,
                    &mut module_aliases,
                    None,
                );
                self.opened_modules = opened_modules;
                self.module_aliases = module_aliases;
                None
            }
            ast::Statement::Let { kind, range } => {
                let opened_modules = self.opened_modules.clone();
                let module_aliases = self.module_aliases.clone();
                let lookup = LookupContext {
                    opened_modules: &opened_modules,
                    module_aliases: &module_aliases,
                };
                let mut type_env = TypeEnv::new();
                let lowered_kind = match kind {
                    ast::LetStatementKind::PatternBinding { pattern, value } => {
                        let mut env = self.fresh_expr_env();
                        let lowered_pattern =
                            self.lower_pattern_global(pattern, &lookup, &mut type_env);
                        let mut recursive_binders = HashSet::new();
                        collect_global_term_binders(&lowered_pattern, &mut recursive_binders);
                        let lowered_value = self.lower_expr(value, &mut env, &mut type_env);
                        self.validate_global_recursion(&lowered_value, &recursive_binders);
                        ir::LetStatementKind::PatternBinding {
                            pattern: lowered_pattern,
                            value: lowered_value,
                        }
                    }
                    ast::LetStatementKind::ConstructorAlias { alias, target } => {
                        let target = if let Some(target) = target {
                            self.resolve_name_ref_global(
                                target,
                                Namespace::Constructor,
                                &lookup,
                                target.range(),
                            )
                        } else {
                            None
                        };

                        let alias_path = alias.as_ref().and_then(|alias| {
                            let alias_name = self.identifier_text_non_hole(alias, "constructor")?;
                            let path = self.path_in_current_module(&alias_name, alias.range);
                            self.insert_decl(
                                Namespace::Constructor,
                                alias_name.clone(),
                                path.clone(),
                                alias.range,
                                "constructor alias",
                            );
                            self.insert_decl(
                                Namespace::Term,
                                alias_name,
                                path.clone(),
                                alias.range,
                                "constructor alias",
                            );
                            Some(path)
                        });

                        ir::LetStatementKind::ConstructorAlias {
                            alias: alias_path,
                            target,
                        }
                    }
                };

                Some(ir::Statement::Let {
                    kind: lowered_kind,
                    range: *range,
                })
            }
            ast::Statement::Do { expr, range } => {
                let mut env = self.fresh_expr_env();
                let mut type_env = TypeEnv::new();
                let lowered_expr = self.lower_expr(expr, &mut env, &mut type_env);
                Some(ir::Statement::Let {
                    kind: ir::LetStatementKind::PatternBinding {
                        pattern: ir::Pattern::Hole { range: *range },
                        value: lowered_expr,
                    },
                    range: *range,
                })
            }
            ast::Statement::Type {
                name,
                declared_kind,
                kind,
                range,
            } => {
                let Some(name_ident) = name else {
                    self.error(*range, "expected type declaration name");
                    return Some(ir::Statement::Error(ir::ErrorNode { range: *range }));
                };
                let Some(name_text) = self.identifier_text_non_hole(name_ident, "type") else {
                    self.error(*range, "expected type declaration name text");
                    return Some(ir::Statement::Error(ir::ErrorNode { range: *range }));
                };

                let type_path = self.path_in_current_module(&name_text, name_ident.range);
                self.insert_decl(
                    Namespace::Type,
                    name_text.clone(),
                    type_path.clone(),
                    name_ident.range,
                    "type declaration",
                );
                self.insert_decl(
                    Namespace::Term,
                    name_text.clone(),
                    type_path.clone(),
                    name_ident.range,
                    "type constructor",
                );
                self.insert_decl(
                    Namespace::Constructor,
                    name_text,
                    type_path.clone(),
                    name_ident.range,
                    "type constructor",
                );

                let mut type_env = TypeEnv::new();
                let opened_modules = self.opened_modules.clone();
                let module_aliases = self.module_aliases.clone();
                let lookup = LookupContext {
                    opened_modules: &opened_modules,
                    module_aliases: &module_aliases,
                };
                let lowered_declared_kind = declared_kind
                    .as_ref()
                    .map(|kind| self.lower_kind_expr(kind));

                let lowered_kind = match kind {
                    ast::TypeStatementKind::Alias { value } => ir::TypeStatementKind::Alias {
                        value: self.lower_type_expr(value, &lookup, &mut type_env),
                    },
                    ast::TypeStatementKind::Nominal { definition } => {
                        ir::TypeStatementKind::Nominal {
                            definition: self.lower_type_definition(
                                definition,
                                &type_path,
                                &lookup,
                                &mut type_env,
                            ),
                        }
                    }
                };

                Some(ir::Statement::Type {
                    name: type_path,
                    declared_kind: lowered_declared_kind,
                    kind: lowered_kind,
                    range: *range,
                })
            }
            ast::Statement::Trait {
                name,
                params,
                items,
                range,
            } => {
                let Some(name_ident) = name else {
                    self.error(*range, "expected trait declaration name");
                    return Some(ir::Statement::Error(ir::ErrorNode { range: *range }));
                };
                let Some(name_text) = self.identifier_text_non_hole(name_ident, "trait") else {
                    self.error(*range, "expected trait declaration name text");
                    return Some(ir::Statement::Error(ir::ErrorNode { range: *range }));
                };

                let trait_path = self.path_in_current_module(&name_text, name_ident.range);
                self.insert_decl(
                    Namespace::Trait,
                    name_text,
                    trait_path.clone(),
                    name_ident.range,
                    "trait declaration",
                );

                let mut type_env = TypeEnv::new();
                let lowered_params = self.lower_plain_type_params(params, &mut type_env);
                let opened_modules = self.opened_modules.clone();
                let module_aliases = self.module_aliases.clone();
                let lookup = LookupContext {
                    opened_modules: &opened_modules,
                    module_aliases: &module_aliases,
                };

                let mut lowered_items = Vec::with_capacity(items.len());
                for item in items {
                    lowered_items.push(self.lower_trait_item(
                        item,
                        &trait_path,
                        &lookup,
                        &mut type_env,
                    ));
                }

                Some(ir::Statement::Trait {
                    name: trait_path,
                    params: lowered_params,
                    items: lowered_items,
                    range: *range,
                })
            }
            ast::Statement::TraitAlias {
                name,
                target,
                range,
            } => {
                let Some(name_ident) = name else {
                    self.error(*range, "expected trait alias name");
                    return Some(ir::Statement::Error(ir::ErrorNode { range: *range }));
                };
                let Some(name_text) = self.identifier_text_non_hole(name_ident, "trait") else {
                    self.error(*range, "expected trait alias name text");
                    return Some(ir::Statement::Error(ir::ErrorNode { range: *range }));
                };

                let alias_path = self.path_in_current_module(&name_text, name_ident.range);
                self.insert_decl(
                    Namespace::Trait,
                    name_text,
                    alias_path.clone(),
                    name_ident.range,
                    "trait alias",
                );

                let opened_modules = self.opened_modules.clone();
                let module_aliases = self.module_aliases.clone();
                let lookup = LookupContext {
                    opened_modules: &opened_modules,
                    module_aliases: &module_aliases,
                };
                let target = if let Some(target) = target {
                    self.resolve_name_ref_global(target, Namespace::Trait, &lookup, target.range())
                } else {
                    None
                };

                Some(ir::Statement::TraitAlias {
                    name: alias_path,
                    target,
                    range: *range,
                })
            }
            ast::Statement::Impl {
                trait_ref,
                for_types,
                items,
                range,
            } => {
                let opened_modules = self.opened_modules.clone();
                let module_aliases = self.module_aliases.clone();
                let lookup = LookupContext {
                    opened_modules: &opened_modules,
                    module_aliases: &module_aliases,
                };
                let mut type_env = TypeEnv::new();

                let trait_ref = if let Some(trait_ref) = trait_ref {
                    self.resolve_name_ref_global(
                        trait_ref,
                        Namespace::Trait,
                        &lookup,
                        trait_ref.range(),
                    )
                } else {
                    None
                };

                let mut lowered_for_types = Vec::with_capacity(for_types.len());
                for ty in for_types {
                    lowered_for_types.push(self.lower_type_expr(ty, &lookup, &mut type_env));
                }

                let mut lowered_items = Vec::with_capacity(items.len());
                for (idx, item) in items.iter().enumerate() {
                    lowered_items.push(self.lower_impl_item(item, idx, &lookup, &mut type_env));
                }

                Some(ir::Statement::Impl {
                    trait_ref,
                    for_types: lowered_for_types,
                    items: lowered_items,
                    range: *range,
                })
            }
            ast::Statement::Wasm {
                declarations,
                range,
            } => Some(self.lower_wasm_statement(declarations, *range)),
            ast::Statement::Error(error) => {
                Some(ir::Statement::Error(ir::ErrorNode { range: error.range }))
            }
        }
    }

    fn lower_expr(
        &mut self,
        expr: &ast::Expr,
        env: &mut ExprEnv,
        type_env: &mut TypeEnv,
    ) -> ir::Expr {
        match expr {
            ast::Expr::Let {
                pattern,
                value,
                body,
                range,
            } => {
                env.push_scope();
                let opened_modules = env.opened_modules.clone();
                let module_aliases = env.module_aliases.clone();
                let lookup = LookupContext {
                    opened_modules: &opened_modules,
                    module_aliases: &module_aliases,
                };
                let lowered_pattern = self.lower_pattern_local(pattern, &lookup, env, type_env);
                let mut recursive_locals = HashSet::new();
                collect_local_term_binders(&lowered_pattern, &mut recursive_locals);
                let lowered_value = self.lower_expr(value, env, type_env);
                self.validate_local_recursion(&lowered_value, &recursive_locals);
                let lowered_body = self.lower_expr(body, env, type_env);
                env.pop_scope();

                ir::Expr::Let {
                    pattern: lowered_pattern,
                    value: Box::new(lowered_value),
                    body: Box::new(lowered_body),
                    range: *range,
                }
            }
            ast::Expr::Use {
                target,
                alias,
                body,
                range,
            } => {
                let mut inserted_aliases = Vec::new();
                let opened_len = env.opened_modules.len();

                self.apply_use(
                    target,
                    alias,
                    *range,
                    &mut env.opened_modules,
                    &mut env.module_aliases,
                    Some(&mut inserted_aliases),
                );

                let lowered = self.lower_expr(body, env, type_env);

                env.opened_modules.truncate(opened_len);
                while let Some(alias) = inserted_aliases.pop() {
                    env.module_aliases.remove(&alias);
                }

                lowered
            }
            ast::Expr::Function {
                params,
                body,
                range,
            } => {
                let opened_modules = env.opened_modules.clone();
                let module_aliases = env.module_aliases.clone();
                let lookup = LookupContext {
                    opened_modules: &opened_modules,
                    module_aliases: &module_aliases,
                };

                env.push_scope();
                let mut function_type_env = type_env.clone();

                let (lowered_params, lowered_body) = match body {
                    ast::FunctionBody::Expr(body_expr) => {
                        let lowered_params = params
                            .iter()
                            .map(|param| {
                                self.lower_parameter_as_pattern(
                                    param,
                                    &lookup,
                                    env,
                                    &mut function_type_env,
                                )
                            })
                            .collect::<Vec<_>>();
                        let lowered_body = self.lower_expr(body_expr, env, &mut function_type_env);
                        (lowered_params, lowered_body)
                    }
                    ast::FunctionBody::MatchArms(arms) => {
                        let argument_name = self.bind_synthetic_local(env, "arg");
                        let argument_pattern = ir::Pattern::Binding {
                            name: argument_name.clone(),
                            range: TextRange::generated(),
                        };

                        let mut lowered_arms = Vec::with_capacity(arms.len());
                        for arm in arms {
                            env.push_scope();
                            let mut arm_type_env = function_type_env.clone();
                            let pattern = self.lower_pattern_local(
                                &arm.pattern,
                                &lookup,
                                env,
                                &mut arm_type_env,
                            );
                            let body = self.lower_expr(&arm.body, env, &mut arm_type_env);
                            env.pop_scope();

                            lowered_arms.push(ir::MatchArm {
                                pattern,
                                body,
                                range: arm.range,
                            });
                        }

                        let lowered_body = ir::Expr::Match {
                            scrutinee: Box::new(ir::Expr::Name(argument_name)),
                            arms: lowered_arms,
                            range: *range,
                        };

                        (vec![argument_pattern], lowered_body)
                    }
                };

                env.pop_scope();

                self.lower_curried_function(lowered_params, lowered_body, *range)
            }
            ast::Expr::If {
                condition,
                then_branch,
                else_branch,
                range,
            } => ir::Expr::If {
                condition: Box::new(self.lower_expr(condition, env, type_env)),
                then_branch: Box::new(self.lower_expr(then_branch, env, type_env)),
                else_branch: Box::new(self.lower_expr(else_branch, env, type_env)),
                range: *range,
            },
            ast::Expr::Match {
                scrutinee,
                arms,
                range,
            } => {
                let scrutinee = Box::new(self.lower_expr(scrutinee, env, type_env));
                let opened_modules = env.opened_modules.clone();
                let module_aliases = env.module_aliases.clone();
                let mut lowered_arms = Vec::with_capacity(arms.len());
                for arm in arms {
                    let lookup = LookupContext {
                        opened_modules: &opened_modules,
                        module_aliases: &module_aliases,
                    };
                    env.push_scope();
                    let pattern = self.lower_pattern_local(&arm.pattern, &lookup, env, type_env);
                    let body = self.lower_expr(&arm.body, env, type_env);
                    env.pop_scope();
                    lowered_arms.push(ir::MatchArm {
                        pattern,
                        body,
                        range: arm.range,
                    });
                }

                ir::Expr::Match {
                    scrutinee,
                    arms: lowered_arms,
                    range: *range,
                }
            }
            ast::Expr::Binary {
                op,
                lhs,
                rhs,
                range,
            } => {
                let lowered_lhs = self.lower_expr(lhs, env, type_env);
                let lowered_rhs = self.lower_expr(rhs, env, type_env);
                let opened_modules = env.opened_modules.clone();
                let module_aliases = env.module_aliases.clone();
                let lookup = LookupContext {
                    opened_modules: &opened_modules,
                    module_aliases: &module_aliases,
                };
                let operator = self.resolve_term_text(&format!("{op}"), env, &lookup, *range);

                let first = ir::Expr::Apply {
                    callee: Box::new(ir::Expr::Name(operator)),
                    argument: Box::new(lowered_lhs),
                    range: *range,
                };
                ir::Expr::Apply {
                    callee: Box::new(first),
                    argument: Box::new(lowered_rhs),
                    range: *range,
                }
            }
            ast::Expr::Unary { op, expr, range } => {
                let lowered_expr = self.lower_expr(expr, env, type_env);
                let opened_modules = env.opened_modules.clone();
                let module_aliases = env.module_aliases.clone();
                let lookup = LookupContext {
                    opened_modules: &opened_modules,
                    module_aliases: &module_aliases,
                };
                let operator = self.resolve_term_text(&format!("{op}"), env, &lookup, *range);

                ir::Expr::Apply {
                    callee: Box::new(ir::Expr::Name(operator)),
                    argument: Box::new(lowered_expr),
                    range: *range,
                }
            }
            ast::Expr::Apply {
                callee,
                argument,
                range,
            } => ir::Expr::Apply {
                callee: Box::new(self.lower_expr(callee, env, type_env)),
                argument: Box::new(self.lower_expr(argument, env, type_env)),
                range: *range,
            },
            ast::Expr::FieldAccess { expr, field, range } => ir::Expr::FieldAccess {
                expr: Box::new(self.lower_expr(expr, env, type_env)),
                field: field.as_ref().and_then(|field| self.identifier_text(field)),
                range: *range,
            },
            ast::Expr::Name(name) => {
                let lookup = LookupContext {
                    opened_modules: &env.opened_modules,
                    module_aliases: &env.module_aliases,
                };
                ir::Expr::Name(self.resolve_term_name(name, env, &lookup, name.range()))
            }
            ast::Expr::Literal(literal) => ir::Expr::Literal(self.lower_literal(literal)),
            ast::Expr::Grouped { inner, .. } => self.lower_expr(inner, env, type_env),
            ast::Expr::Unit { range } => ir::Expr::Unit { range: *range },
            ast::Expr::Tuple { elements, range } => ir::Expr::Tuple {
                elements: elements
                    .iter()
                    .map(|element| self.lower_expr(element, env, type_env))
                    .collect(),
                range: *range,
            },
            ast::Expr::Array { elements, range } => ir::Expr::Array {
                elements: elements
                    .iter()
                    .map(|element| match element {
                        ast::ArrayElement::Item(item) => {
                            ir::ArrayElement::Item(self.lower_expr(item, env, type_env))
                        }
                        ast::ArrayElement::Spread { expr, range } => ir::ArrayElement::Spread {
                            expr: self.lower_expr(expr, env, type_env),
                            range: *range,
                        },
                    })
                    .collect(),
                range: *range,
            },
            ast::Expr::Record { fields, range } => ir::Expr::Record {
                fields: fields
                    .iter()
                    .map(|field| ir::RecordField {
                        name: field
                            .name
                            .as_ref()
                            .and_then(|name| self.identifier_text(name)),
                        separator: field.separator,
                        value: self.lower_expr(&field.value, env, type_env),
                        range: field.range,
                    })
                    .collect(),
                range: *range,
            },
            ast::Expr::InlineWasm {
                result_type,
                body,
                range,
            } => {
                let lookup = LookupContext {
                    opened_modules: &env.opened_modules,
                    module_aliases: &env.module_aliases,
                };
                let (locals, instructions) = self.lower_inline_wasm_expression(body, *range);
                ir::Expr::InlineWasm {
                    result_type: self.lower_type_expr(result_type, &lookup, type_env),
                    locals,
                    instructions,
                    range: *range,
                }
            }
            ast::Expr::Error(error) => ir::Expr::Error(ir::ErrorNode { range: error.range }),
        }
    }

    fn lower_parameter_as_pattern(
        &mut self,
        parameter: &ast::Parameter,
        lookup: &LookupContext,
        env: &mut ExprEnv,
        type_env: &mut TypeEnv,
    ) -> ir::Pattern {
        match parameter {
            ast::Parameter::Named(identifier) => {
                let Some(name) = self.identifier_text(identifier) else {
                    return ir::Pattern::Error(ir::ErrorNode {
                        range: identifier.range,
                    });
                };
                if name == "_" {
                    return ir::Pattern::Hole {
                        range: identifier.range,
                    };
                }
                let binding = self.bind_local(name, identifier.range, env);
                ir::Pattern::Binding {
                    name: binding,
                    range: identifier.range,
                }
            }
            ast::Parameter::Typed { name, ty, range } => {
                let pattern = if let Some(name) = name {
                    if let Some(name_text) = self.identifier_text(name) {
                        if name_text == "_" {
                            ir::Pattern::Hole { range: name.range }
                        } else {
                            ir::Pattern::Binding {
                                name: self.bind_local(name_text, name.range, env),
                                range: name.range,
                            }
                        }
                    } else {
                        ir::Pattern::Error(ir::ErrorNode { range: name.range })
                    }
                } else {
                    self.error(*range, "typed parameter is missing a name");
                    ir::Pattern::Error(ir::ErrorNode { range: *range })
                };

                ir::Pattern::Annotated {
                    pattern: Box::new(pattern),
                    ty: self.lower_type_expr(ty, lookup, type_env),
                    range: *range,
                }
            }
            ast::Parameter::Error(error) => {
                ir::Pattern::Error(ir::ErrorNode { range: error.range })
            }
        }
    }

    fn lower_curried_function(
        &self,
        params: Vec<ir::Pattern>,
        body: ir::Expr,
        range: TextRange,
    ) -> ir::Expr {
        if params.is_empty() {
            return ir::Expr::Function {
                params,
                body: Box::new(body),
                range,
            };
        }

        params
            .into_iter()
            .rev()
            .fold(body, |acc, param| ir::Expr::Function {
                params: vec![param],
                body: Box::new(acc),
                range,
            })
    }

    fn lower_pattern_global(
        &mut self,
        pattern: &ast::Pattern,
        lookup: &LookupContext,
        type_env: &mut TypeEnv,
    ) -> ir::Pattern {
        match pattern {
            ast::Pattern::Constructor {
                constructor,
                argument,
                range,
            } => {
                let constructor = self
                    .resolve_name_ref_global(
                        constructor,
                        Namespace::Constructor,
                        lookup,
                        constructor.range(),
                    )
                    .unwrap_or_else(|| ir::QualifiedName {
                        segments: vec!["<error>".to_owned()],
                        range: constructor.range(),
                    });
                ir::Pattern::Constructor {
                    constructor,
                    argument: Box::new(self.lower_pattern_global(argument, lookup, type_env)),
                    range: *range,
                }
            }
            ast::Pattern::Annotated { pattern, ty, range } => ir::Pattern::Annotated {
                pattern: Box::new(self.lower_pattern_global(pattern, lookup, type_env)),
                ty: self.lower_type_expr(ty, lookup, type_env),
                range: *range,
            },
            ast::Pattern::Name(name_ref) => {
                if let ast::NameRef::Identifier(identifier) = name_ref
                    && let Some(name_text) = self.identifier_text(identifier)
                    && name_text == "_"
                {
                    return ir::Pattern::Hole {
                        range: identifier.range,
                    };
                }

                if let ast::NameRef::Identifier(identifier) = name_ref
                    && let Some(name_text) = self.identifier_text(identifier)
                    && let Some(constructor) =
                        self.resolve_constructor_identifier(&name_text, lookup, identifier.range)
                {
                    return ir::Pattern::ConstructorName {
                        constructor,
                        range: identifier.range,
                    };
                }

                match name_ref {
                    ast::NameRef::Path(_) => {
                        let constructor = self
                            .resolve_name_ref_global(
                                name_ref,
                                Namespace::Constructor,
                                lookup,
                                name_ref.range(),
                            )
                            .unwrap_or_else(|| ir::QualifiedName {
                                segments: vec!["<error>".to_owned()],
                                range: name_ref.range(),
                            });
                        ir::Pattern::ConstructorName {
                            constructor,
                            range: name_ref.range(),
                        }
                    }
                    ast::NameRef::Identifier(identifier) => {
                        let binding = self.bind_global(identifier);
                        ir::Pattern::Binding {
                            name: binding,
                            range: identifier.range,
                        }
                    }
                }
            }
            ast::Pattern::Literal(literal) => ir::Pattern::Literal(self.lower_literal(literal)),
            ast::Pattern::Grouped { inner, .. } => {
                self.lower_pattern_global(inner, lookup, type_env)
            }
            ast::Pattern::Tuple { elements, range } => ir::Pattern::Tuple {
                elements: elements
                    .iter()
                    .map(|element| self.lower_pattern_global(element, lookup, type_env))
                    .collect(),
                range: *range,
            },
            ast::Pattern::Array { elements, range } => ir::Pattern::Array {
                elements: elements
                    .iter()
                    .map(|element| match element {
                        ast::ArrayPatternElement::Item(item) => ir::ArrayPatternElement::Item(
                            self.lower_pattern_global(item, lookup, type_env),
                        ),
                        ast::ArrayPatternElement::Rest { binding, range } => {
                            let binding = binding.as_ref().and_then(|binding| {
                                self.identifier_text(binding).and_then(|name| {
                                    if name == "_" {
                                        None
                                    } else {
                                        Some(self.bind_global(binding))
                                    }
                                })
                            });
                            ir::ArrayPatternElement::Rest {
                                binding,
                                range: *range,
                            }
                        }
                    })
                    .collect(),
                range: *range,
            },
            ast::Pattern::Record {
                fields,
                open,
                range,
            } => ir::Pattern::Record {
                fields: fields
                    .iter()
                    .map(|field| {
                        let name_text = field
                            .name
                            .as_ref()
                            .and_then(|name| self.identifier_text(name));
                        let value = if let Some(value) = &field.value {
                            Some(self.lower_pattern_global(value, lookup, type_env))
                        } else if let Some(name) = &field.name {
                            self.identifier_text(name).map(|name_text| {
                                if name_text == "_" {
                                    ir::Pattern::Hole { range: name.range }
                                } else {
                                    let binding = self.bind_global(name);
                                    ir::Pattern::Binding {
                                        name: binding,
                                        range: name.range,
                                    }
                                }
                            })
                        } else {
                            None
                        };
                        ir::RecordPatternField {
                            name: name_text,
                            value,
                            range: field.range,
                        }
                    })
                    .collect(),
                open: *open,
                range: *range,
            },
            ast::Pattern::Error(error) => ir::Pattern::Error(ir::ErrorNode { range: error.range }),
        }
    }

    fn lower_pattern_local(
        &mut self,
        pattern: &ast::Pattern,
        lookup: &LookupContext,
        env: &mut ExprEnv,
        type_env: &mut TypeEnv,
    ) -> ir::Pattern {
        match pattern {
            ast::Pattern::Constructor {
                constructor,
                argument,
                range,
            } => {
                let constructor = self
                    .resolve_name_ref_global(
                        constructor,
                        Namespace::Constructor,
                        lookup,
                        constructor.range(),
                    )
                    .unwrap_or_else(|| ir::QualifiedName {
                        segments: vec!["<error>".to_owned()],
                        range: constructor.range(),
                    });
                ir::Pattern::Constructor {
                    constructor,
                    argument: Box::new(self.lower_pattern_local(argument, lookup, env, type_env)),
                    range: *range,
                }
            }
            ast::Pattern::Annotated { pattern, ty, range } => ir::Pattern::Annotated {
                pattern: Box::new(self.lower_pattern_local(pattern, lookup, env, type_env)),
                ty: self.lower_type_expr(ty, lookup, type_env),
                range: *range,
            },
            ast::Pattern::Name(name_ref) => {
                if let ast::NameRef::Identifier(identifier) = name_ref
                    && let Some(name_text) = self.identifier_text(identifier)
                    && name_text == "_"
                {
                    return ir::Pattern::Hole {
                        range: identifier.range,
                    };
                }

                if let ast::NameRef::Identifier(identifier) = name_ref
                    && let Some(name_text) = self.identifier_text(identifier)
                    && let Some(constructor) =
                        self.resolve_constructor_identifier(&name_text, lookup, identifier.range)
                {
                    return ir::Pattern::ConstructorName {
                        constructor,
                        range: identifier.range,
                    };
                }

                match name_ref {
                    ast::NameRef::Path(_) => {
                        let constructor = self
                            .resolve_name_ref_global(
                                name_ref,
                                Namespace::Constructor,
                                lookup,
                                name_ref.range(),
                            )
                            .unwrap_or_else(|| ir::QualifiedName {
                                segments: vec!["<error>".to_owned()],
                                range: name_ref.range(),
                            });
                        ir::Pattern::ConstructorName {
                            constructor,
                            range: name_ref.range(),
                        }
                    }
                    ast::NameRef::Identifier(identifier) => {
                        let Some(name_text) = self.identifier_text(identifier) else {
                            return ir::Pattern::Error(ir::ErrorNode {
                                range: identifier.range,
                            });
                        };
                        let binding = self.bind_local(name_text, identifier.range, env);
                        ir::Pattern::Binding {
                            name: binding,
                            range: identifier.range,
                        }
                    }
                }
            }
            ast::Pattern::Literal(literal) => ir::Pattern::Literal(self.lower_literal(literal)),
            ast::Pattern::Grouped { inner, .. } => {
                self.lower_pattern_local(inner, lookup, env, type_env)
            }
            ast::Pattern::Tuple { elements, range } => ir::Pattern::Tuple {
                elements: elements
                    .iter()
                    .map(|element| self.lower_pattern_local(element, lookup, env, type_env))
                    .collect(),
                range: *range,
            },
            ast::Pattern::Array { elements, range } => ir::Pattern::Array {
                elements: elements
                    .iter()
                    .map(|element| match element {
                        ast::ArrayPatternElement::Item(item) => ir::ArrayPatternElement::Item(
                            self.lower_pattern_local(item, lookup, env, type_env),
                        ),
                        ast::ArrayPatternElement::Rest { binding, range } => {
                            let binding = binding.as_ref().and_then(|binding| {
                                self.identifier_text(binding).and_then(|name| {
                                    if name == "_" {
                                        None
                                    } else {
                                        Some(self.bind_local(name, binding.range, env))
                                    }
                                })
                            });
                            ir::ArrayPatternElement::Rest {
                                binding,
                                range: *range,
                            }
                        }
                    })
                    .collect(),
                range: *range,
            },
            ast::Pattern::Record {
                fields,
                open,
                range,
            } => ir::Pattern::Record {
                fields: fields
                    .iter()
                    .map(|field| {
                        let name_text = field
                            .name
                            .as_ref()
                            .and_then(|name| self.identifier_text(name));
                        let value = if let Some(value) = &field.value {
                            Some(self.lower_pattern_local(value, lookup, env, type_env))
                        } else if let Some(name) = &field.name {
                            self.identifier_text(name).map(|name_text| {
                                if name_text == "_" {
                                    ir::Pattern::Hole { range: name.range }
                                } else {
                                    ir::Pattern::Binding {
                                        name: self.bind_local(name_text, name.range, env),
                                        range: name.range,
                                    }
                                }
                            })
                        } else {
                            None
                        };

                        ir::RecordPatternField {
                            name: name_text,
                            value,
                            range: field.range,
                        }
                    })
                    .collect(),
                open: *open,
                range: *range,
            },
            ast::Pattern::Error(error) => ir::Pattern::Error(ir::ErrorNode { range: error.range }),
        }
    }

    fn lower_type_expr(
        &mut self,
        type_expr: &ast::TypeExpr,
        lookup: &LookupContext,
        type_env: &mut TypeEnv,
    ) -> ir::TypeExpr {
        match type_expr {
            ast::TypeExpr::Forall {
                params,
                body,
                constraints,
                range,
            } => {
                type_env.push_scope();
                let params = self.lower_type_binders(params, type_env);

                let body = Box::new(self.lower_type_expr(body, lookup, type_env));
                let constraints = constraints
                    .iter()
                    .map(|constraint| ir::TraitConstraint {
                        trait_ref: constraint.trait_ref.as_ref().and_then(|trait_ref| {
                            self.resolve_name_ref_global(
                                trait_ref,
                                Namespace::Trait,
                                lookup,
                                trait_ref.range(),
                            )
                        }),
                        args: constraint
                            .args
                            .iter()
                            .map(|arg| self.lower_type_expr(arg, lookup, type_env))
                            .collect(),
                        range: constraint.range,
                    })
                    .collect();
                type_env.pop_scope();

                ir::TypeExpr::Forall {
                    params,
                    body,
                    constraints,
                    range: *range,
                }
            }
            ast::TypeExpr::Lambda {
                params,
                body,
                range,
            } => {
                type_env.push_scope();
                let params = self.lower_type_binders(params, type_env);
                let body = Box::new(self.lower_type_expr(body, lookup, type_env));
                type_env.pop_scope();

                ir::TypeExpr::Lambda {
                    params,
                    body,
                    range: *range,
                }
            }
            ast::TypeExpr::Function {
                param,
                result,
                range,
            } => ir::TypeExpr::Function {
                param: Box::new(self.lower_type_expr(param, lookup, type_env)),
                result: Box::new(self.lower_type_expr(result, lookup, type_env)),
                range: *range,
            },
            ast::TypeExpr::Apply {
                callee,
                argument,
                range,
            } => ir::TypeExpr::Apply {
                callee: Box::new(self.lower_type_expr(callee, lookup, type_env)),
                argument: Box::new(self.lower_type_expr(argument, lookup, type_env)),
                range: *range,
            },
            ast::TypeExpr::Name(name_ref) => {
                if let ast::NameRef::Identifier(identifier) = name_ref
                    && let Some(name_text) = self.identifier_text(identifier)
                    && name_text == "_"
                {
                    ir::TypeExpr::Hole {
                        range: identifier.range,
                    }
                } else {
                    ir::TypeExpr::Name {
                        name: self.resolve_type_name(name_ref, lookup, type_env, name_ref.range()),
                    }
                }
            }
            ast::TypeExpr::Record { members, range } => ir::TypeExpr::Record {
                members: self.lower_record_type_members(members, lookup, type_env),
                range: *range,
            },
            ast::TypeExpr::Grouped { inner, .. } => self.lower_type_expr(inner, lookup, type_env),
            ast::TypeExpr::Tuple { elements, range } => ir::TypeExpr::Tuple {
                elements: elements
                    .iter()
                    .map(|element| self.lower_type_expr(element, lookup, type_env))
                    .collect(),
                range: *range,
            },
            ast::TypeExpr::Unit { range } => ir::TypeExpr::Unit { range: *range },
            ast::TypeExpr::Array { range } => ir::TypeExpr::Array { range: *range },
            ast::TypeExpr::Error(error) => {
                ir::TypeExpr::Error(ir::ErrorNode { range: error.range })
            }
        }
    }

    fn lower_record_type_members(
        &mut self,
        members: &[ast::RecordTypeMember],
        lookup: &LookupContext,
        type_env: &mut TypeEnv,
    ) -> Vec<ir::RecordTypeMember> {
        members
            .iter()
            .map(|member| match member {
                ast::RecordTypeMember::Field { name, ty, range } => ir::RecordTypeMember::Field {
                    name: name.as_ref().and_then(|name| self.identifier_text(name)),
                    ty: self.lower_type_expr(ty, lookup, type_env),
                    range: *range,
                },
                ast::RecordTypeMember::Spread { ty, range } => ir::RecordTypeMember::Spread {
                    ty: self.lower_type_expr(ty, lookup, type_env),
                    range: *range,
                },
            })
            .collect()
    }

    fn lower_nominal_record_definition(
        &mut self,
        type_expr: &ast::TypeExpr,
        lookup: &LookupContext,
        type_env: &mut TypeEnv,
    ) -> Option<ir::TypeDefinition> {
        match type_expr {
            ast::TypeExpr::Record { members, range } => Some(ir::TypeDefinition::Struct {
                members: self.lower_record_type_members(members, lookup, type_env),
                range: *range,
            }),
            ast::TypeExpr::Grouped { inner, .. } => {
                self.lower_nominal_record_definition(inner, lookup, type_env)
            }
            _ => None,
        }
    }

    fn lower_type_definition(
        &mut self,
        definition: &ast::TypeDefinition,
        type_path: &ir::QualifiedName,
        lookup: &LookupContext,
        type_env: &mut TypeEnv,
    ) -> ir::TypeDefinition {
        match definition {
            ast::TypeDefinition::Lambda {
                params,
                body,
                range,
            } => {
                type_env.push_scope();
                let params = self.lower_type_binders(params, type_env);
                let body = Box::new(self.lower_type_definition(body, type_path, lookup, type_env));
                type_env.pop_scope();

                ir::TypeDefinition::Lambda {
                    params,
                    body,
                    range: *range,
                }
            }
            ast::TypeDefinition::Struct { members, range } => ir::TypeDefinition::Struct {
                members: self.lower_record_type_members(members, lookup, type_env),
                range: *range,
            },
            ast::TypeDefinition::Sum { variants, range } => {
                let lowered_variants = variants
                    .iter()
                    .map(|variant| {
                        let lowered_name = variant.name.as_ref().and_then(|name| {
                            let name_text = self.identifier_text_non_hole(name, "constructor")?;
                            let path = ir::QualifiedName {
                                segments: {
                                    let mut segments = type_path.segments.clone();
                                    segments.push(name_text.clone());
                                    segments
                                },
                                range: name.range,
                            };
                            self.insert_decl(
                                Namespace::Constructor,
                                name_text.clone(),
                                path.clone(),
                                name.range,
                                "sum variant",
                            );
                            self.insert_decl(
                                Namespace::Term,
                                name_text,
                                path.clone(),
                                name.range,
                                "sum variant",
                            );
                            Some(path)
                        });
                        let lowered_argument = variant
                            .argument
                            .as_ref()
                            .map(|argument| self.lower_type_expr(argument, lookup, type_env));

                        ir::SumVariant {
                            name: lowered_name,
                            argument: lowered_argument,
                            range: variant.range,
                        }
                    })
                    .collect();

                ir::TypeDefinition::Sum {
                    variants: lowered_variants,
                    range: *range,
                }
            }
            ast::TypeDefinition::Expr(expr) => self
                .lower_nominal_record_definition(expr, lookup, type_env)
                .unwrap_or_else(|| ir::TypeDefinition::Opaque {
                    representation: self.lower_type_expr(expr, lookup, type_env),
                }),
        }
    }

    fn lower_trait_item(
        &mut self,
        item: &ast::TraitItem,
        trait_path: &ir::QualifiedName,
        lookup: &LookupContext,
        type_env: &mut TypeEnv,
    ) -> ir::TraitItem {
        match item {
            ast::TraitItem::Method { name, ty, range } => {
                let name_path = name.as_ref().and_then(|name| {
                    let name_text = self.identifier_text_non_hole(name, "term")?;
                    let mut segments = trait_path.segments.clone();
                    segments.push(name_text);
                    Some(ir::QualifiedName {
                        segments,
                        range: name.range,
                    })
                });
                ir::TraitItem::Method {
                    name: name_path,
                    ty: self.lower_type_expr(ty, lookup, type_env),
                    range: *range,
                }
            }
            ast::TraitItem::Type { name, range } => {
                let name_path = name.as_ref().and_then(|name| {
                    let name_text = self.identifier_text_non_hole(name, "type")?;
                    let mut segments = trait_path.segments.clone();
                    segments.push(name_text);
                    Some(ir::QualifiedName {
                        segments,
                        range: name.range,
                    })
                });
                ir::TraitItem::Type {
                    name: name_path,
                    range: *range,
                }
            }
            ast::TraitItem::Error(error) => {
                ir::TraitItem::Error(ir::ErrorNode { range: error.range })
            }
        }
    }

    fn lower_impl_item(
        &mut self,
        item: &ast::ImplItem,
        index: usize,
        lookup: &LookupContext,
        type_env: &mut TypeEnv,
    ) -> ir::ImplItem {
        match item {
            ast::ImplItem::Method { name, value, range } => {
                let name = name.as_ref().and_then(|name| {
                    let name_text = self.identifier_text_non_hole(name, "term")?;
                    let mut segments = self.module_path.clone();
                    segments.push(format!("impl#{index}"));
                    segments.push(name_text);
                    Some(ir::QualifiedName {
                        segments,
                        range: name.range,
                    })
                });
                let mut env = self.fresh_expr_env();
                let value = self.lower_expr(value, &mut env, type_env);
                ir::ImplItem::Method {
                    name,
                    value,
                    range: *range,
                }
            }
            ast::ImplItem::Type { name, value, range } => {
                let name = name.as_ref().and_then(|name| {
                    let name_text = self.identifier_text_non_hole(name, "type")?;
                    let mut segments = self.module_path.clone();
                    segments.push(format!("impl#{index}"));
                    segments.push(name_text);
                    Some(ir::QualifiedName {
                        segments,
                        range: name.range,
                    })
                });
                let value = self.lower_type_expr(value, lookup, type_env);
                ir::ImplItem::Type {
                    name,
                    value,
                    range: *range,
                }
            }
            ast::ImplItem::Error(error) => {
                ir::ImplItem::Error(ir::ErrorNode { range: error.range })
            }
        }
    }

    fn lower_literal(&self, literal: &ast::Literal) -> ir::Literal {
        let Some(text) = literal.range.text(&self.source_contents) else {
            self.error(literal.range, "failed to read literal text");
            return ir::Literal {
                value: match literal.kind {
                    ast::LiteralKind::Integer => ir::LiteralValue::Integer(0),
                    ast::LiteralKind::Natural => ir::LiteralValue::Natural(0),
                    ast::LiteralKind::Real => ir::LiteralValue::Real(ir::RealLiteral::new(0.0)),
                    ast::LiteralKind::String => ir::LiteralValue::String(String::new()),
                    ast::LiteralKind::Glyph => ir::LiteralValue::Glyph(String::new()),
                    ast::LiteralKind::FormatString => ir::LiteralValue::FormatString(Vec::new()),
                    ast::LiteralKind::BoolTrue => ir::LiteralValue::Bool(true),
                    ast::LiteralKind::BoolFalse => ir::LiteralValue::Bool(false),
                },
                range: literal.range,
            };
        };

        ir::Literal {
            value: match literal.kind {
                ast::LiteralKind::Integer => self
                    .parse_integer_literal(&text, literal.range)
                    .unwrap_or(ir::LiteralValue::Integer(0)),
                ast::LiteralKind::Natural => self
                    .parse_natural_literal(&text, literal.range)
                    .unwrap_or(ir::LiteralValue::Natural(0)),
                ast::LiteralKind::Real => self.parse_real_literal(&text, literal.range),
                ast::LiteralKind::String => ir::LiteralValue::String(text),
                ast::LiteralKind::Glyph => ir::LiteralValue::Glyph(text),
                ast::LiteralKind::FormatString => {
                    self.parse_format_string_literal(&text, literal.range)
                }
                ast::LiteralKind::BoolTrue => ir::LiteralValue::Bool(true),
                ast::LiteralKind::BoolFalse => ir::LiteralValue::Bool(false),
            },
            range: literal.range,
        }
    }

    fn parse_integer_literal(&self, text: &str, range: TextRange) -> Option<ir::LiteralValue> {
        let cleaned = text.replace('_', "");
        let (negative, value_text) = if let Some(rest) = cleaned.strip_prefix('-') {
            (true, rest)
        } else {
            (false, cleaned.as_str())
        };

        let (radix, digits) = if value_text.starts_with("0x") || value_text.starts_with("0X") {
            (16u32, &value_text[2..])
        } else if value_text.starts_with("0o") || value_text.starts_with("0O") {
            (8u32, &value_text[2..])
        } else if value_text.starts_with("0b") || value_text.starts_with("0B") {
            (2u32, &value_text[2..])
        } else {
            (10u32, value_text)
        };

        if digits.is_empty() {
            self.error(range, format!("failed to parse integer literal `{text}`"));
            return None;
        }

        let parsed = i128::from_str_radix(digits, radix).ok()?;
        let value = if negative { -parsed } else { parsed };
        let value = i64::try_from(value).ok()?;

        Some(ir::LiteralValue::Integer(value))
    }

    fn parse_natural_literal(&self, text: &str, range: TextRange) -> Option<ir::LiteralValue> {
        if !text.ends_with('n') {
            self.error(range, format!("failed to parse natural literal `{text}`"));
            return None;
        }

        let cleaned = text[..text.len() - 1].replace('_', "");

        if cleaned.starts_with('-') {
            self.error(range, format!("failed to parse natural literal `{text}`"));
            return None;
        }

        let (radix, digits) = if cleaned.starts_with("0x") || cleaned.starts_with("0X") {
            (16u32, &cleaned[2..])
        } else if cleaned.starts_with("0o") || cleaned.starts_with("0O") {
            (8u32, &cleaned[2..])
        } else if cleaned.starts_with("0b") || cleaned.starts_with("0B") {
            (2u32, &cleaned[2..])
        } else {
            (10u32, cleaned.as_str())
        };

        if digits.is_empty() {
            self.error(range, format!("failed to parse natural literal `{text}`"));
            return None;
        }

        let parsed = u128::from_str_radix(digits, radix).ok()?;
        let value = u64::try_from(parsed).ok()?;

        Some(ir::LiteralValue::Natural(value))
    }

    fn parse_real_literal(&self, text: &str, range: TextRange) -> ir::LiteralValue {
        let cleaned = text.replace('_', "");
        match cleaned.parse::<f64>() {
            Ok(value) => ir::LiteralValue::Real(ir::RealLiteral::new(value)),
            Err(_) => {
                self.error(range, format!("failed to parse real literal `{text}`"));
                ir::LiteralValue::Real(ir::RealLiteral::new(0.0))
            }
        }
    }

    fn parse_format_string_literal(&self, text: &str, range: TextRange) -> ir::LiteralValue {
        let Some(content) = text.strip_prefix('`').and_then(|s| s.strip_suffix('`')) else {
            self.error(
                range,
                format!("failed to parse format string literal `{text}`"),
            );
            return ir::LiteralValue::FormatString(Vec::new());
        };

        let mut segments = Vec::new();
        let mut current_text = String::new();
        let mut chars = content.chars().peekable();

        while let Some(ch) = chars.next() {
            match ch {
                '{' => match chars.peek().copied() {
                    Some('{') => {
                        chars.next();
                        current_text.push('{');
                    }
                    Some('}') => {
                        chars.next();
                        if !current_text.is_empty() {
                            segments.push(ir::FormatStringSegment::Text(std::mem::take(
                                &mut current_text,
                            )));
                        }
                        segments.push(ir::FormatStringSegment::Placeholder);
                    }
                    _ => {
                        self.error(
                            range,
                            format!("failed to parse format string literal `{text}`"),
                        );
                        return ir::LiteralValue::FormatString(Vec::new());
                    }
                },
                '}' => match chars.peek().copied() {
                    Some('}') => {
                        chars.next();
                        current_text.push('}');
                    }
                    _ => {
                        self.error(
                            range,
                            format!("failed to parse format string literal `{text}`"),
                        );
                        return ir::LiteralValue::FormatString(Vec::new());
                    }
                },
                _ => current_text.push(ch),
            }
        }

        if !current_text.is_empty() {
            segments.push(ir::FormatStringSegment::Text(current_text));
        }

        ir::LiteralValue::FormatString(segments)
    }

    fn lower_plain_type_params(
        &mut self,
        params: &[ast::Identifier],
        type_env: &mut TypeEnv,
    ) -> Vec<ir::TypeBinder> {
        params
            .iter()
            .filter_map(|param| {
                let name = self.identifier_text_non_hole(param, "type parameter")?;
                let id = self.next_local();
                type_env.bind_local(name.clone(), id);
                Some(ir::TypeBinder {
                    id,
                    name,
                    kind_annotation: None,
                    range: param.range,
                })
            })
            .collect()
    }

    fn lower_type_binders(
        &mut self,
        params: &[ast::TypeBinder],
        type_env: &mut TypeEnv,
    ) -> Vec<ir::TypeBinder> {
        params
            .iter()
            .filter_map(|param| {
                let name = self.identifier_text_non_hole(&param.name, "type parameter")?;
                let id = self.next_local();
                type_env.bind_local(name.clone(), id);
                Some(ir::TypeBinder {
                    id,
                    name,
                    kind_annotation: param.kind.as_ref().map(|kind| self.lower_kind_expr(kind)),
                    range: param.range,
                })
            })
            .collect()
    }

    fn lower_kind_expr(&mut self, kind_expr: &ast::KindExpr) -> ir::KindExpr {
        match kind_expr {
            ast::KindExpr::Type { range } => ir::KindExpr::Type { range: *range },
            ast::KindExpr::Row { range } => ir::KindExpr::Row { range: *range },
            ast::KindExpr::Grouped { inner, .. } => self.lower_kind_expr(inner),
            ast::KindExpr::Arrow {
                param,
                result,
                range,
            } => ir::KindExpr::Arrow {
                param: Box::new(self.lower_kind_expr(param)),
                result: Box::new(self.lower_kind_expr(result)),
                range: *range,
            },
            ast::KindExpr::Error(error) => {
                ir::KindExpr::Error(ir::ErrorNode { range: error.range })
            }
        }
    }

    fn bind_global(&mut self, identifier: &ast::Identifier) -> ir::ResolvedName {
        let Some(name_text) = self.identifier_text(identifier) else {
            return ir::ResolvedName::Error {
                name: "<error>".to_owned(),
                range: identifier.range,
            };
        };

        if name_text == "_" {
            self.error(identifier.range, "`_` is not a valid term name");
            return ir::ResolvedName::Error {
                name: "_".to_owned(),
                range: identifier.range,
            };
        }

        let path = self.path_in_current_module(&name_text, identifier.range);
        let inserted = self.insert_decl(
            Namespace::Term,
            name_text.clone(),
            path.clone(),
            identifier.range,
            "binding",
        );

        if inserted {
            ir::ResolvedName::Global(path)
        } else {
            let existing = self.scope.terms.get(&name_text).cloned().unwrap_or(path);
            ir::ResolvedName::Global(existing)
        }
    }

    fn bind_local(
        &mut self,
        name: String,
        range: TextRange,
        env: &mut ExprEnv,
    ) -> ir::ResolvedName {
        if name == "_" {
            self.error(range, "`_` is not a valid term name");
            return ir::ResolvedName::Error { name, range };
        }

        let local = self.next_local();
        env.bind_local(name.clone(), local);
        ir::ResolvedName::Local {
            id: local,
            name,
            range,
        }
    }

    fn bind_synthetic_local(&mut self, env: &mut ExprEnv, hint: &str) -> ir::ResolvedName {
        let local = self.next_local();
        let name = format!("${hint}#{}", local.0);
        env.bind_local(name.clone(), local);
        ir::ResolvedName::Local {
            id: local,
            name,
            range: TextRange::generated(),
        }
    }

    fn resolve_term_name(
        &mut self,
        name_ref: &ast::NameRef,
        env: &ExprEnv,
        lookup: &LookupContext,
        range: TextRange,
    ) -> ir::ResolvedName {
        match name_ref {
            ast::NameRef::Identifier(identifier) => {
                let Some(name_text) = self.identifier_text(identifier) else {
                    self.error(range, "failed to resolve term name");
                    return ir::ResolvedName::Error {
                        name: range.text(&self.source_contents).unwrap_or_default(),
                        range,
                    };
                };
                self.resolve_term_text(&name_text, env, lookup, identifier.range)
            }
            ast::NameRef::Path(_) => {
                if let Some(path) =
                    self.resolve_name_ref_global(name_ref, Namespace::Term, lookup, range)
                {
                    ir::ResolvedName::Global(path)
                } else {
                    self.error(range, "failed to resolve term name");
                    ir::ResolvedName::Error {
                        name: range.text(&self.source_contents).unwrap_or_default(),
                        range,
                    }
                }
            }
        }
    }

    fn resolve_term_text(
        &mut self,
        name_text: &str,
        env: &ExprEnv,
        lookup: &LookupContext,
        range: TextRange,
    ) -> ir::ResolvedName {
        if name_text == "_" {
            self.error(range, "`_` is not a valid term name");
            return ir::ResolvedName::Error {
                name: "_".to_owned(),
                range,
            };
        }

        if let Some(local) = env.lookup_local(name_text) {
            return ir::ResolvedName::Local {
                id: local,
                name: name_text.to_owned(),
                range,
            };
        }

        let segments = vec![name_text.to_owned()];
        if let Some(path) =
            self.resolve_relative_segments(Namespace::Term, &segments, lookup, range)
        {
            ir::ResolvedName::Global(path)
        } else {
            self.error(range, format!("failed to resolve term name `{name_text}`"));
            ir::ResolvedName::Error {
                name: name_text.to_owned(),
                range,
            }
        }
    }

    fn validate_global_recursion(&self, expr: &ir::Expr, recursive_binders: &HashSet<String>) {
        if recursive_binders.is_empty() {
            return;
        }

        Self::walk_expr_names(expr, 0, &mut |name, function_depth| {
            if function_depth > 0 {
                return;
            }

            if let ir::ResolvedName::Global(path) = name {
                let path_text = path.text();
                if recursive_binders.contains(&path_text) {
                    self.error(
                        path.range,
                        format!(
                            "recursive reference to `{path_text}` is only allowed inside a function"
                        ),
                    );
                }
            }
        });
    }

    fn validate_local_recursion(&self, expr: &ir::Expr, recursive_binders: &HashSet<ir::LocalId>) {
        if recursive_binders.is_empty() {
            return;
        }

        Self::walk_expr_names(expr, 0, &mut |name, function_depth| {
            if function_depth > 0 {
                return;
            }

            if let ir::ResolvedName::Local { id, name, range } = name
                && recursive_binders.contains(id)
            {
                self.error(
                    *range,
                    format!("recursive reference to `{name}` is only allowed inside a function"),
                );
            }
        });
    }

    fn walk_expr_names<F>(expr: &ir::Expr, function_depth: u32, on_name: &mut F)
    where
        F: FnMut(&ir::ResolvedName, u32),
    {
        match expr {
            ir::Expr::Let { value, body, .. } => {
                Self::walk_expr_names(value, function_depth, on_name);
                Self::walk_expr_names(body, function_depth, on_name);
            }
            ir::Expr::Function { body, .. } => {
                Self::walk_expr_names(body, function_depth.saturating_add(1), on_name);
            }
            ir::Expr::If {
                condition,
                then_branch,
                else_branch,
                ..
            } => {
                Self::walk_expr_names(condition, function_depth, on_name);
                Self::walk_expr_names(then_branch, function_depth, on_name);
                Self::walk_expr_names(else_branch, function_depth, on_name);
            }
            ir::Expr::Match {
                scrutinee, arms, ..
            } => {
                Self::walk_expr_names(scrutinee, function_depth, on_name);
                for arm in arms {
                    Self::walk_expr_names(&arm.body, function_depth, on_name);
                }
            }
            ir::Expr::Apply {
                callee, argument, ..
            } => {
                Self::walk_expr_names(callee, function_depth, on_name);
                Self::walk_expr_names(argument, function_depth, on_name);
            }
            ir::Expr::FieldAccess { expr, .. } => {
                Self::walk_expr_names(expr, function_depth, on_name);
            }
            ir::Expr::Name(name) => on_name(name, function_depth),
            ir::Expr::Tuple { elements, .. } => {
                for element in elements {
                    Self::walk_expr_names(element, function_depth, on_name);
                }
            }
            ir::Expr::Array { elements, .. } => {
                for element in elements {
                    match element {
                        ir::ArrayElement::Item(item) => {
                            Self::walk_expr_names(item, function_depth, on_name);
                        }
                        ir::ArrayElement::Spread { expr, .. } => {
                            Self::walk_expr_names(expr, function_depth, on_name);
                        }
                    }
                }
            }
            ir::Expr::Record { fields, .. } => {
                for field in fields {
                    Self::walk_expr_names(&field.value, function_depth, on_name);
                }
            }
            ir::Expr::Literal(_)
            | ir::Expr::Unit { .. }
            | ir::Expr::InlineWasm { .. }
            | ir::Expr::Error(_) => {}
        }
    }

    fn resolve_type_name(
        &mut self,
        name_ref: &ast::NameRef,
        lookup: &LookupContext,
        type_env: &TypeEnv,
        range: TextRange,
    ) -> ir::ResolvedName {
        if let ast::NameRef::Identifier(identifier) = name_ref
            && let Some(name_text) = self.identifier_text(identifier)
            && let Some(local) = type_env.lookup_local(&name_text)
        {
            return ir::ResolvedName::Local {
                id: local,
                name: name_text,
                range: identifier.range,
            };
        }

        if let Some(path) = self.resolve_name_ref_global(name_ref, Namespace::Type, lookup, range) {
            ir::ResolvedName::Global(path)
        } else {
            self.error(range, "failed to resolve type name");
            ir::ResolvedName::Error {
                name: range.text(&self.source_contents).unwrap_or_default(),
                range,
            }
        }
    }

    fn resolve_name_ref_global(
        &mut self,
        name_ref: &ast::NameRef,
        namespace: Namespace,
        lookup: &LookupContext,
        range: TextRange,
    ) -> Option<ir::QualifiedName> {
        let resolved = match name_ref {
            ast::NameRef::Identifier(identifier) => {
                let name = self.identifier_text(identifier)?;
                if name == "_" {
                    self.error(
                        identifier.range,
                        format!("`_` is not a valid {namespace} name"),
                    );
                    None
                } else {
                    self.resolve_relative_segments(namespace, &[name], lookup, range)
                }
            }
            ast::NameRef::Path(path) => {
                let mut segments = Vec::with_capacity(path.segments.len());
                let mut invalid_segment = false;
                for segment in &path.segments {
                    let Some(text) = self.identifier_text(segment) else {
                        invalid_segment = true;
                        break;
                    };
                    if text == "_" {
                        self.error(
                            segment.range,
                            format!("`_` is not a valid {namespace} name"),
                        );
                        invalid_segment = true;
                        break;
                    }
                    segments.push(text);
                }

                if invalid_segment {
                    None
                } else if segments.is_empty() {
                    match path.root {
                        ast::PathRoot::Bundle => Some(ir::QualifiedName {
                            segments: vec![self.bundle_name.clone()],
                            range: path.range,
                        }),
                        ast::PathRoot::Root | ast::PathRoot::Relative => None,
                    }
                } else {
                    match path.root {
                        ast::PathRoot::Root => Some(ir::QualifiedName {
                            segments,
                            range: path.range,
                        }),
                        ast::PathRoot::Bundle => {
                            let mut full = vec![self.bundle_name.clone()];
                            full.extend(segments);
                            Some(ir::QualifiedName {
                                segments: full,
                                range: path.range,
                            })
                        }
                        ast::PathRoot::Relative => {
                            self.resolve_relative_segments(namespace, &segments, lookup, range)
                        }
                    }
                }
            }
        };

        if resolved.is_none() && matches!(name_ref, ast::NameRef::Path(_)) {
            let text = range.text(&self.source_contents).unwrap_or_default();
            if text.trim().is_empty() {
                self.error(range, format!("failed to resolve {namespace} path"));
            } else {
                self.error(
                    range,
                    format!("failed to resolve {namespace} path `{text}`",),
                );
            }
        }

        resolved
    }

    fn resolve_relative_segments(
        &mut self,
        namespace: Namespace,
        segments: &[String],
        lookup: &LookupContext,
        range: TextRange,
    ) -> Option<ir::QualifiedName> {
        if segments.is_empty() {
            return None;
        }

        let head = &segments[0];

        if let Some(path) = self.namespace_map(namespace).get(head) {
            return Some(
                path.clone()
                    .extend(segments[1..].iter().cloned())
                    .range(range),
            );
        }

        if let Some(alias_base) = lookup.module_aliases.get(head)
            && segments.len() > 1
        {
            return Some(
                alias_base
                    .clone()
                    .extend(segments[1..].iter().cloned())
                    .range(range),
            );
        }

        if namespace != Namespace::Module
            && let Some(module_base) = self.scope.modules.get(head)
            && segments.len() > 1
        {
            return Some(
                module_base
                    .clone()
                    .extend(segments[1..].iter().cloned())
                    .range(range),
            );
        }

        if segments.len() == 1 {
            let mut candidates = Vec::new();
            for opened_module in lookup.opened_modules {
                if let Some(candidate) =
                    self.lookup_opened_export(opened_module, namespace, head, range)
                {
                    candidates.push(candidate);
                }
            }

            if let Some(only) = candidates.first().cloned()
                && candidates.len() == 1
            {
                return Some(only);
            }

            if candidates.len() > 1 {
                self.error(
                    range,
                    format!("ambiguous name `{head}` from multiple opened modules"),
                );
                return None;
            }
        }
        None
    }

    fn resolve_constructor_identifier(
        &mut self,
        name: &str,
        lookup: &LookupContext,
        range: TextRange,
    ) -> Option<ir::QualifiedName> {
        let segments = vec![name.to_owned()];
        self.resolve_relative_segments(Namespace::Constructor, &segments, lookup, range)
    }

    fn lookup_opened_export(
        &mut self,
        opened_module: &ir::QualifiedName,
        namespace: Namespace,
        name: &str,
        range: TextRange,
    ) -> Option<ir::QualifiedName> {
        let request = self.request_for_module_path(&opened_module.text());
        let Some(request) = request else {
            self.error(
                range,
                format!(
                    "module `{}` is not available in lowering graph",
                    opened_module.text()
                ),
            );
            return None;
        };

        let lowered = lower_module_query(self.db, request);
        lowered.module.exports.get(namespace, name)
    }

    fn request_for_module_path(&self, path: &str) -> Option<ModuleRequest<'db>> {
        if let Some(request) = self.module_requests.get(path).copied() {
            return Some(request);
        }

        let mut queue = self
            .module_requests
            .values()
            .copied()
            .filter(|request| *request != self.request)
            .collect::<Vec<_>>();
        let mut seen = HashSet::new();

        while let Some(request) = queue.pop() {
            if !seen.insert(request) {
                continue;
            }

            let lowered = lower_module_query(self.db, request);
            for child in lowered.children {
                let child_path = child.module_path(self.db).text(self.db).clone();
                if child_path == path {
                    return Some(child);
                }
                queue.push(child);
            }
        }

        None
    }

    fn apply_use(
        &mut self,
        target: &Option<ast::NameRef>,
        alias: &Option<ast::Identifier>,
        range: TextRange,
        opened_modules: &mut Vec<ir::QualifiedName>,
        module_aliases: &mut HashMap<String, ir::QualifiedName>,
        inserted_aliases: Option<&mut Vec<String>>,
    ) {
        let Some(target) = target else {
            self.error(range, "expected use target");
            return;
        };

        let lookup = LookupContext {
            opened_modules,
            module_aliases,
        };
        let Some(module_path) =
            self.resolve_name_ref_global(target, Namespace::Module, &lookup, target.range())
        else {
            self.error(range, "failed to resolve use target module");
            return;
        };

        if let Some(alias) = alias {
            let Some(alias_text) = self.identifier_text_non_hole(alias, "module alias") else {
                self.error(alias.range, "expected use alias name");
                return;
            };

            if module_aliases.contains_key(&alias_text) {
                self.error(alias.range, format!("duplicate use alias `{alias_text}`"));
                return;
            }

            module_aliases.insert(alias_text.clone(), module_path);
            if let Some(inserted_aliases) = inserted_aliases {
                inserted_aliases.push(alias_text);
            }
        } else {
            opened_modules.push(module_path);
        }
    }

    fn resolve_module_source_ref(
        &mut self,
        name_text: &str,
        in_loc: &Option<ast::Literal>,
        range: TextRange,
    ) -> Option<String> {
        if let Some(in_loc) = in_loc {
            let raw = in_loc.range.text(&self.source_contents)?;
            let Some(unescaped) = bake_string(&raw) else {
                self.error(
                    in_loc.range,
                    "failed to decode module reference string literal",
                );
                return None;
            };

            let canon = (self.resolver.canonize)(&unescaped, &self.source_canon);
            if canon.is_none() {
                self.error(
                    range,
                    format!("failed to resolve module `{name_text}` at `{unescaped}`"),
                );
            }
            return canon;
        }

        let canon = (self.resolver.canonize_bare)(name_text, &self.source_canon);
        if canon.is_none() {
            self.error(
                range,
                format!(
                    "failed to resolve module `{name_text}` from `{}`",
                    self.source_canon
                ),
            );
        }
        canon
    }

    fn register_module_decl(
        &mut self,
        name: String,
        module_path: ir::QualifiedName,
        request: ModuleRequest<'db>,
        range: TextRange,
    ) {
        self.insert_decl(
            Namespace::Module,
            name,
            module_path.clone(),
            range,
            "module declaration",
        );
        let module_path_text = module_path.text();
        self.module_requests.insert(module_path_text, request);
        self.children.push(request);
    }

    fn insert_decl(
        &mut self,
        namespace: Namespace,
        name: String,
        path: ir::QualifiedName,
        range: TextRange,
        kind: &str,
    ) -> bool {
        let map = self.namespace_map_mut(namespace);
        let contains = map.contains_key(&name);
        if contains {
            self.error(range, format!("duplicate {kind} `{name}`"));
        } else {
            map.insert(name, path);
        }
        !contains
    }

    fn namespace_map(&self, namespace: Namespace) -> &HashMap<String, ir::QualifiedName> {
        match namespace {
            Namespace::Module => &self.scope.modules,
            Namespace::Type => &self.scope.types,
            Namespace::Trait => &self.scope.traits,
            Namespace::Term => &self.scope.terms,
            Namespace::Constructor => &self.scope.constructors,
        }
    }

    fn namespace_map_mut(
        &mut self,
        namespace: Namespace,
    ) -> &mut HashMap<String, ir::QualifiedName> {
        match namespace {
            Namespace::Module => &mut self.scope.modules,
            Namespace::Type => &mut self.scope.types,
            Namespace::Trait => &mut self.scope.traits,
            Namespace::Term => &mut self.scope.terms,
            Namespace::Constructor => &mut self.scope.constructors,
        }
    }

    fn path_in_current_module(&self, name: &str, range: TextRange) -> ir::QualifiedName {
        let mut segments = self.module_path.clone();
        segments.push(name.to_owned());
        ir::QualifiedName { segments, range }
    }

    fn child_module_path(&self, child_name: &str, range: TextRange) -> ir::QualifiedName {
        let mut segments = self.module_path.clone();
        segments.push(child_name.to_owned());
        ir::QualifiedName { segments, range }
    }

    fn fresh_expr_env(&self) -> ExprEnv {
        ExprEnv::new(self.opened_modules.clone(), self.module_aliases.clone())
    }

    fn identifier_text(&self, identifier: &ast::Identifier) -> Option<String> {
        identifier.range.text(&self.source_contents)
    }

    fn identifier_text_non_hole(&self, identifier: &ast::Identifier, kind: &str) -> Option<String> {
        let text = self.identifier_text(identifier)?;
        if text == "_" {
            self.error(identifier.range, format!("`_` is not a valid {kind} name"));
            None
        } else {
            Some(text)
        }
    }

    fn opt_identifier_text_non_hole(
        &self,
        identifier: &Option<ast::Identifier>,
        kind: &str,
    ) -> Option<String> {
        identifier
            .as_ref()
            .and_then(|identifier| self.identifier_text_non_hole(identifier, kind))
    }

    pub(super) fn error(&self, range: TextRange, message: impl Into<String>) {
        Diag(Diagnostic::error(range, message)).accumulate(self.db);
    }

    fn next_local(&mut self) -> ir::LocalId {
        let current = self.next_local_id;
        self.next_local_id = self.next_local_id.saturating_add(1);
        ir::LocalId(current)
    }

    pub(super) fn exports(&self) -> ir::ModuleExports {
        self.scope.exports()
    }
}

fn collect_global_term_binders(pattern: &ir::Pattern, binders: &mut HashSet<String>) {
    match pattern {
        ir::Pattern::Constructor { argument, .. } => {
            collect_global_term_binders(argument, binders);
        }
        ir::Pattern::ConstructorName { .. } => {}
        ir::Pattern::Binding { name, .. } => {
            if let ir::ResolvedName::Global(path) = name {
                binders.insert(path.text());
            }
        }
        ir::Pattern::Hole { .. } => {}
        ir::Pattern::Annotated { pattern, .. } => {
            collect_global_term_binders(pattern, binders);
        }
        ir::Pattern::Literal(_) => {}
        ir::Pattern::Tuple { elements, .. } => {
            for element in elements {
                collect_global_term_binders(element, binders);
            }
        }
        ir::Pattern::Array { elements, .. } => {
            for element in elements {
                match element {
                    ir::ArrayPatternElement::Item(item) => {
                        collect_global_term_binders(item, binders);
                    }
                    ir::ArrayPatternElement::Rest { binding, .. } => {
                        if let Some(ir::ResolvedName::Global(path)) = binding {
                            binders.insert(path.text());
                        }
                    }
                }
            }
        }
        ir::Pattern::Record { fields, .. } => {
            for field in fields {
                if let Some(value) = &field.value {
                    collect_global_term_binders(value, binders);
                }
            }
        }
        ir::Pattern::Error(_) => {}
    }
}

fn collect_local_term_binders(pattern: &ir::Pattern, binders: &mut HashSet<ir::LocalId>) {
    match pattern {
        ir::Pattern::Constructor { argument, .. } => {
            collect_local_term_binders(argument, binders);
        }
        ir::Pattern::ConstructorName { .. } => {}
        ir::Pattern::Binding { name, .. } => {
            if let ir::ResolvedName::Local { id, .. } = name {
                binders.insert(*id);
            }
        }
        ir::Pattern::Hole { .. } => {}
        ir::Pattern::Annotated { pattern, .. } => {
            collect_local_term_binders(pattern, binders);
        }
        ir::Pattern::Literal(_) => {}
        ir::Pattern::Tuple { elements, .. } => {
            for element in elements {
                collect_local_term_binders(element, binders);
            }
        }
        ir::Pattern::Array { elements, .. } => {
            for element in elements {
                match element {
                    ir::ArrayPatternElement::Item(item) => {
                        collect_local_term_binders(item, binders);
                    }
                    ir::ArrayPatternElement::Rest { binding, .. } => {
                        if let Some(ir::ResolvedName::Local { id, .. }) = binding {
                            binders.insert(*id);
                        }
                    }
                }
            }
        }
        ir::Pattern::Record { fields, .. } => {
            for field in fields {
                if let Some(value) = &field.value {
                    collect_local_term_binders(value, binders);
                }
            }
        }
        ir::Pattern::Error(_) => {}
    }
}
