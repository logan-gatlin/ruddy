//! Internal HM inferencer/checker implementation.
//!
//! The checker consumes lowered IR (`crate::lower::ir`) and produces typed IR
//! (`crate::ty::typed_ir`) by:
//! - inferring expression and pattern types,
//! - unifying constraints through [`super::UnificationTable`],
//! - generalizing `let`-bound definitions into [`crate::ty::TypeScheme`], and
//! - zonking solved meta-variables into stable output types.
//!
//! This first pass intentionally focuses on core HM inference.
//! Trait solving is deferred, while predicative higher-ranked polymorphism is
//! handled during inference via outer `forall` skolemization/instantiation.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::lower::ir as lir;
use crate::reporting::{Diagnostic, TextRange};
use crate::ty::store::TypeStore;
use crate::ty::typed_ir as tir;
use crate::ty::{
    Kind, KindId, MetaTypeVariableId, TraitPredicate, TypeBinder, TypeBinderId, TypeConstructor,
    TypeId, TypeKind, TypeScheme,
};

use super::{UnificationError, UnificationTable};

/// Full output of checking a lowered source.
#[derive(Debug)]
pub struct CheckResult {
    /// Type-annotated IR after inference and zonking.
    pub source: tir::Source,
    /// Diagnostics produced while checking.
    pub diagnostics: Vec<Diagnostic>,
    /// Canonical type store backing all `TypeId`s in `source`.
    pub type_store: TypeStore,
}

/// Run HM checking over a lowered source graph.
pub fn check_lowered_source(lowered: &lir::LoweredSource) -> CheckResult {
    Checker::new().check_source(lowered)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum BinderKey {
    Local(lir::LocalId),
    Global(String),
}

#[derive(Clone)]
struct BoundVar {
    key: BinderKey,
    ty: TypeId,
    range: TextRange,
}

#[derive(Clone, Default)]
struct LocalEnv {
    terms: HashMap<lir::LocalId, TypeScheme>,
}

#[derive(Default)]
struct TypeExprEnv {
    locals: HashMap<lir::LocalId, TypeId>,
}

struct Checker {
    store: TypeStore,
    table: UnificationTable,
    diagnostics: Vec<Diagnostic>,
    deferred_predicates: Vec<TraitPredicate>,
    global_terms: HashMap<String, TypeScheme>,
    alias_schemes: HashMap<String, TypeScheme>,
    record_schemes: HashMap<String, TypeScheme>,
    opaque_schemes: HashMap<String, TypeScheme>,
    named_type_constructors: HashMap<String, TypeId>,
    declaration_type_transparency_depth: usize,
    next_type_binder: u32,
}

impl Checker {
    fn new() -> Self {
        Self {
            store: TypeStore::new(),
            table: UnificationTable::new(),
            diagnostics: Vec::new(),
            deferred_predicates: Vec::new(),
            global_terms: HashMap::new(),
            alias_schemes: HashMap::new(),
            record_schemes: HashMap::new(),
            opaque_schemes: HashMap::new(),
            named_type_constructors: HashMap::new(),
            declaration_type_transparency_depth: 0,
            next_type_binder: 0,
        }
    }

    fn check_source(mut self, lowered: &lir::LoweredSource) -> CheckResult {
        let modules = lowered
            .modules
            .iter()
            .map(|module| self.check_module(module))
            .collect();

        let mut source = tir::Source {
            root_module: lowered.root_module.clone(),
            modules,
        };

        self.zonk_source(&mut source);

        CheckResult {
            source,
            diagnostics: self.diagnostics,
            type_store: self.store,
        }
    }

    fn check_module(&mut self, module: &lir::LoweredModule) -> tir::Module {
        let statements = module
            .statements
            .iter()
            .map(|statement| self.check_statement(statement))
            .collect();

        tir::Module {
            path: module.path.clone(),
            source_name: module.source_name.clone(),
            range: module.range,
            statements,
            exports: module.exports.clone(),
        }
    }

    fn check_statement(&mut self, statement: &lir::Statement) -> tir::Statement {
        match statement {
            lir::Statement::ModuleDecl {
                name,
                module,
                range,
            } => tir::Statement::ModuleDecl {
                name: name.clone(),
                module: module.clone(),
                range: *range,
            },
            lir::Statement::Let { kind, range } => self.check_let_statement(kind, *range),
            lir::Statement::Type {
                name,
                declared_kind,
                kind,
                range,
            } => {
                self.check_type_statement(name, declared_kind.as_ref(), kind, *range);
                tir::Statement::Type {
                    name: name.clone(),
                    declared_kind: declared_kind.clone(),
                    kind: kind.clone(),
                    range: *range,
                }
            }
            lir::Statement::Trait {
                name,
                params,
                items,
                range,
            } => tir::Statement::Trait {
                name: name.clone(),
                params: params.clone(),
                items: items.clone(),
                range: *range,
            },
            lir::Statement::TraitAlias {
                name,
                target,
                range,
            } => tir::Statement::TraitAlias {
                name: name.clone(),
                target: target.clone(),
                range: *range,
            },
            lir::Statement::Impl {
                trait_ref,
                for_types,
                items,
                range,
            } => tir::Statement::Impl {
                trait_ref: trait_ref.clone(),
                for_types: for_types.clone(),
                items: items.clone(),
                range: *range,
            },
            lir::Statement::Wasm {
                declarations,
                range,
            } => tir::Statement::Wasm {
                declarations: declarations.clone(),
                range: *range,
            },
            lir::Statement::Error(error) => tir::Statement::Error(error.clone()),
        }
    }

    fn check_let_statement(
        &mut self,
        kind: &lir::LetStatementKind,
        range: TextRange,
    ) -> tir::Statement {
        match kind {
            lir::LetStatementKind::PatternBinding { pattern, value } => {
                let mut locals = LocalEnv::default();
                let keys = pattern.collect_binders();
                let prebound = self.prebind_keys(&mut locals, &keys);
                let snapshot = self.table.type_var_count();

                let typed_value = self.infer_expr(value, &mut locals);
                let (typed_pattern, bound_vars) =
                    self.check_pattern(pattern, typed_value.ty, &prebound);

                for bound in bound_vars {
                    let scheme = self.generalize_type(bound.ty, snapshot, bound.range);
                    self.insert_scheme_for_key(&mut locals, &bound.key, scheme, bound.range);
                }

                tir::Statement::Let {
                    kind: tir::LetStatementKind::PatternBinding {
                        pattern: typed_pattern,
                        value: typed_value,
                    },
                    range,
                }
            }
            lir::LetStatementKind::ConstructorAlias { alias, target } => {
                if let (Some(alias), Some(target)) = (alias, target) {
                    let alias_key = alias.text();
                    let target_key = target.text();
                    if let Some(target_scheme) = self.global_terms.get(&target_key).cloned() {
                        self.global_terms.insert(alias_key, target_scheme);
                    } else {
                        self.error(
                            target.range,
                            format!(
                                "failed to resolve constructor alias target `{}` during type checking",
                                target_key
                            ),
                        );
                    }
                }

                tir::Statement::Let {
                    kind: tir::LetStatementKind::ConstructorAlias {
                        alias: alias.clone(),
                        target: target.clone(),
                    },
                    range,
                }
            }
        }
    }

    fn check_type_statement(
        &mut self,
        name: &lir::QualifiedName,
        declared_kind: Option<&lir::KindExpr>,
        kind: &lir::TypeStatementKind,
        range: TextRange,
    ) {
        match kind {
            lir::TypeStatementKind::Alias { value } => {
                let (params, body) = value.peel_lambdas();
                let (decl_binders, mut type_env, _) = self.build_type_binder_env(&params);
                let alias_ty = self.lower_type_expr(body, &mut type_env);
                let alias_kind = self.store.get_type(alias_ty).kind_id;
                let computed_head_kind = self.declaration_head_kind(&decl_binders, alias_kind);

                if let Some(declared_kind) = declared_kind {
                    let declared_kind = self.lower_kind_expr(declared_kind);
                    self.constrain_kinds(range, computed_head_kind, declared_kind);
                }

                let scheme_binders = self.finalize_decl_binders(&decl_binders);
                let final_body_kind = self.default_unresolved_kind_to_type(alias_kind);
                let final_head_kind = self.declaration_head_kind(&scheme_binders, final_body_kind);
                self.ensure_named_type_constructor(name, Some(final_head_kind), range);

                self.alias_schemes.insert(
                    name.text(),
                    TypeScheme {
                        binders: scheme_binders,
                        predicates: Vec::new(),
                        body: alias_ty,
                        range,
                    },
                );
            }
            lir::TypeStatementKind::Nominal { definition } => {
                let (params, definition) = definition.peel_lambdas();
                let (decl_binders, mut type_env, param_types) = self.build_type_binder_env(&params);

                self.with_declaration_type_transparency(|this| match definition {
                    lir::TypeDefinition::Lambda { .. } => {
                        unreachable!("leading lambdas are peeled")
                    }
                    lir::TypeDefinition::Struct { members, .. } => {
                        let body_result_kind = this.store.kind_type();
                        let computed_head_kind =
                            this.declaration_head_kind(&decl_binders, body_result_kind);

                        if let Some(declared_kind) = declared_kind {
                            let declared_kind = this.lower_kind_expr(declared_kind);
                            this.constrain_kinds(range, computed_head_kind, declared_kind);
                        }

                        this.ensure_named_type_constructor(name, Some(computed_head_kind), range);

                        let fields = this.lower_record_type_members(members, &mut type_env);
                        let record_ty = this.mk_closed_record_from_fields(&fields);
                        let scheme_binders = this.finalize_decl_binders(&decl_binders);
                        let final_head_kind =
                            this.declaration_head_kind(&scheme_binders, body_result_kind);
                        this.ensure_named_type_constructor(name, Some(final_head_kind), range);

                        let nominal_ty = this.instantiate_type_head(
                            name,
                            &param_types,
                            range,
                            Some(final_head_kind),
                        );
                        let constructor_ty = this.store.mk_arrow(
                            record_ty,
                            nominal_ty,
                        );
                        this.global_terms.insert(
                            name.text(),
                            TypeScheme {
                                binders: scheme_binders.clone(),
                                predicates: Vec::new(),
                                body: constructor_ty,
                                range,
                            },
                        );
                        this.record_schemes.insert(
                            name.text(),
                            TypeScheme {
                                binders: scheme_binders,
                                predicates: Vec::new(),
                                body: record_ty,
                                range,
                            },
                        );
                    }
                    lir::TypeDefinition::Sum { variants, .. } => {
                        let body_result_kind = this.store.kind_type();
                        let computed_head_kind =
                            this.declaration_head_kind(&decl_binders, body_result_kind);

                        if let Some(declared_kind) = declared_kind {
                            let declared_kind = this.lower_kind_expr(declared_kind);
                            this.constrain_kinds(range, computed_head_kind, declared_kind);
                        }

                        this.ensure_named_type_constructor(name, Some(computed_head_kind), range);
                        let head_ty = this.instantiate_type_head(
                            name,
                            &param_types,
                            range,
                            Some(computed_head_kind),
                        );

                        let mut variant_entries = Vec::new();

                        for variant in variants {
                            let Some(variant_name) = &variant.name else {
                                continue;
                            };

                            let variant_ty = if let Some(argument) = &variant.argument {
                                let argument_ty = this.lower_type_expr(argument, &mut type_env);
                                this.expect_type_kind(argument.range(), argument_ty);
                                this.store.mk_arrow(argument_ty, head_ty)
                            } else {
                                head_ty
                            };

                            variant_entries.push((variant_name.text(), variant_ty, variant.range));
                        }

                        let scheme_binders = this.finalize_decl_binders(&decl_binders);
                        let final_head_kind =
                            this.declaration_head_kind(&scheme_binders, body_result_kind);
                        this.ensure_named_type_constructor(name, Some(final_head_kind), range);

                        for (variant_name, variant_ty, variant_range) in variant_entries {
                            this.global_terms.insert(
                                variant_name,
                                TypeScheme {
                                    binders: scheme_binders.clone(),
                                    predicates: Vec::new(),
                                    body: variant_ty,
                                    range: variant_range,
                                },
                            );
                        }
                    }
                    lir::TypeDefinition::Opaque { representation } => {
                        let (_, provisional_result_kind) =
                            this.table.fresh_kind_var(&mut this.store);
                        let provisional_head_kind =
                            this.declaration_head_kind(&decl_binders, provisional_result_kind);
                        this.ensure_named_type_constructor(
                            name,
                            Some(provisional_head_kind),
                            range,
                        );

                        let repr_ty = this.lower_type_expr(representation, &mut type_env);
                        let representation_kind = this.store.get_type(repr_ty).kind_id;
                        this.constrain_kinds(
                            representation.range(),
                            provisional_result_kind,
                            representation_kind,
                        );

                        let computed_head_kind =
                            this.declaration_head_kind(&decl_binders, representation_kind);
                        if let Some(declared_kind) = declared_kind {
                            let declared_kind = this.lower_kind_expr(declared_kind);
                            this.constrain_kinds(range, computed_head_kind, declared_kind);
                        }

                        let scheme_binders = this.finalize_decl_binders(&decl_binders);
                        let finalized_body_result_kind =
                            this.default_unresolved_kind_to_type(representation_kind);
                        let final_head_kind =
                            this.declaration_head_kind(&scheme_binders, finalized_body_result_kind);
                        this.ensure_named_type_constructor(name, Some(final_head_kind), range);

                        let nominal_ty = this.instantiate_type_head(
                            name,
                            &param_types,
                            range,
                            Some(final_head_kind),
                        );
                        let constructor_ty = this.store.mk_arrow(
                            repr_ty,
                            nominal_ty,
                        );
                        this.opaque_schemes.insert(
                            name.text(),
                            TypeScheme {
                                binders: scheme_binders.clone(),
                                predicates: Vec::new(),
                                body: repr_ty,
                                range,
                            },
                        );
                        this.global_terms.insert(
                            name.text(),
                            TypeScheme {
                                binders: scheme_binders,
                                predicates: Vec::new(),
                                body: constructor_ty,
                                range,
                            },
                        );
                    }
                });
            }
        }
    }

    fn build_type_binder_env(
        &mut self,
        params: &[lir::TypeBinder],
    ) -> (Vec<TypeBinder>, TypeExprEnv, Vec<TypeId>) {
        let mut env = TypeExprEnv::default();
        let (binders, _, param_tys) = self.bind_type_binders(params, &mut env);
        (binders, env, param_tys)
    }

    fn with_declaration_type_transparency<R>(
        &mut self,
        f: impl FnOnce(&mut Self) -> R,
    ) -> R {
        self.declaration_type_transparency_depth += 1;
        let result = f(self);
        self.declaration_type_transparency_depth -= 1;
        result
    }

    fn declaration_type_transparency_enabled(&self) -> bool {
        self.declaration_type_transparency_depth > 0
    }

    fn instantiate_type_head(
        &mut self,
        name: &lir::QualifiedName,
        params: &[TypeId],
        range: TextRange,
        expected_kind: Option<KindId>,
    ) -> TypeId {
        let mut head = self.ensure_named_type_constructor(name, expected_kind, range);
        for param in params {
            head = self.apply_type(head, *param, range);
        }

        head
    }

    fn apply_type(&mut self, callee_ty: TypeId, argument_ty: TypeId, range: TextRange) -> TypeId {
        let (_, result_kind) = self.table.fresh_kind_var(&mut self.store);

        let argument_kind = self.store.get_type(argument_ty).kind_id;
        let expected_callee_kind = self.store.kind_arrow(argument_kind, result_kind);
        let callee_kind = self.store.get_type(callee_ty).kind_id;
        self.constrain_kinds(range, callee_kind, expected_callee_kind);

        let resolved_callee = self.table.shallow_resolve_type(&self.store, callee_ty);
        if let TypeKind::Lambda(binder, body) = self.store.get_type(resolved_callee).kind.clone() {
            let reduced =
                self.substitute_rigid_type(body, &HashMap::from_iter([(binder.id, argument_ty)]));
            let reduced_kind = self.store.get_type(reduced).kind_id;
            self.constrain_kinds(range, reduced_kind, result_kind);
            reduced
        } else {
            self.store
                .mk_application(callee_ty, argument_ty, result_kind)
        }
    }

    fn declaration_head_kind(&mut self, binders: &[TypeBinder], result_kind: KindId) -> KindId {
        let mut kind = result_kind;
        for binder in binders.iter().rev() {
            kind = self.store.kind_arrow(binder.kind, kind);
        }
        kind
    }

    fn finalize_decl_binders(&mut self, binders: &[TypeBinder]) -> Vec<TypeBinder> {
        binders
            .iter()
            .map(|binder| TypeBinder {
                id: binder.id,
                name: binder.name.clone(),
                kind: self.default_unresolved_kind_to_type(binder.kind),
                range: binder.range,
            })
            .collect()
    }

    fn bind_type_binders(
        &mut self,
        params: &[lir::TypeBinder],
        env: &mut TypeExprEnv,
    ) -> (Vec<TypeBinder>, Vec<lir::LocalId>, Vec<TypeId>) {
        let mut binders = Vec::with_capacity(params.len());
        let mut added_ids = Vec::with_capacity(params.len());
        let mut param_tys = Vec::with_capacity(params.len());

        for param in params {
            let binder_id = self.fresh_type_binder_id();
            let kind = if let Some(kind_annotation) = &param.kind_annotation {
                self.lower_kind_expr(kind_annotation)
            } else {
                let (_, fresh_kind) = self.table.fresh_kind_var(&mut self.store);
                fresh_kind
            };
            let binder = TypeBinder {
                id: binder_id,
                name: param.name.clone(),
                kind,
                range: param.range,
            };
            let rigid = self.store.mk_rigid(binder_id, kind);
            env.locals.insert(param.id, rigid);
            binders.push(binder);
            added_ids.push(param.id);
            param_tys.push(rigid);
        }

        (binders, added_ids, param_tys)
    }

    fn lower_kind_expr(&mut self, kind_expr: &lir::KindExpr) -> KindId {
        match kind_expr {
            lir::KindExpr::Type { .. } => self.store.kind_type(),
            lir::KindExpr::Row { .. } => self.store.kind_row(),
            lir::KindExpr::Arrow { param, result, .. } => {
                let param = self.lower_kind_expr(param);
                let result = self.lower_kind_expr(result);
                self.store.kind_arrow(param, result)
            }
            lir::KindExpr::Error(_) => self.store.alloc_kind(Kind::Error),
        }
    }

    fn infer_expr(&mut self, expr: &lir::Expr, locals: &mut LocalEnv) -> tir::Expr {
        match expr {
            lir::Expr::Let {
                pattern,
                value,
                body,
                range,
            } => {
                let mut scoped = locals.clone();
                let keys = pattern.collect_binders();
                let prebound = self.prebind_keys(&mut scoped, &keys);
                let snapshot = self.table.type_var_count();

                let typed_value = self.infer_expr(value, &mut scoped);
                let (typed_pattern, bound_vars) =
                    self.check_pattern(pattern, typed_value.ty, &prebound);

                for bound in bound_vars {
                    let scheme = self.generalize_type(bound.ty, snapshot, bound.range);
                    self.insert_scheme_for_key(&mut scoped, &bound.key, scheme, bound.range);
                }

                let typed_body = self.infer_expr(body, &mut scoped);
                let result_ty = typed_body.ty;

                tir::Expr {
                    kind: tir::ExprKind::Let {
                        pattern: typed_pattern,
                        value: Box::new(typed_value),
                        body: Box::new(typed_body),
                    },
                    ty: result_ty,
                    range: *range,
                }
            }
            lir::Expr::Function {
                params,
                body,
                range,
            } => {
                let mut function_locals = locals.clone();
                let mut typed_params = Vec::with_capacity(params.len());
                let mut parameter_types = Vec::with_capacity(params.len());

                for param in params {
                    let param_ty = self.fresh_type_meta();
                    let (typed_param, bound_vars) =
                        self.check_pattern(param, param_ty, &HashMap::new());
                    let parameter_ty = match &typed_param.kind {
                        tir::PatternKind::Annotated { annotation, .. } => *annotation,
                        _ => param_ty,
                    };
                    for bound in bound_vars {
                        let scheme = self.mono_scheme(bound.ty, bound.range);
                        self.insert_scheme_for_key(
                            &mut function_locals,
                            &bound.key,
                            scheme,
                            bound.range,
                        );
                    }
                    typed_params.push(typed_param);
                    parameter_types.push(parameter_ty);
                }

                let typed_body = self.infer_expr(body, &mut function_locals);

                let mut fn_ty = typed_body.ty;
                for parameter_ty in parameter_types.into_iter().rev() {
                    fn_ty = self.store.mk_arrow(parameter_ty, fn_ty);
                }

                tir::Expr {
                    kind: tir::ExprKind::Function {
                        params: typed_params,
                        body: Box::new(typed_body),
                    },
                    ty: fn_ty,
                    range: *range,
                }
            }
            lir::Expr::If {
                condition,
                then_branch,
                else_branch,
                range,
            } => {
                let typed_condition = self.infer_expr(condition, locals);
                let bool_ty = self.bool_type();
                self.constrain_types(*range, typed_condition.ty, bool_ty);

                let typed_then = self.infer_expr(then_branch, locals);
                let typed_else = self.infer_expr(else_branch, locals);
                self.constrain_types(*range, typed_then.ty, typed_else.ty);

                tir::Expr {
                    kind: tir::ExprKind::If {
                        condition: Box::new(typed_condition),
                        then_branch: Box::new(typed_then.clone()),
                        else_branch: Box::new(typed_else),
                    },
                    ty: typed_then.ty,
                    range: *range,
                }
            }
            lir::Expr::Match {
                scrutinee,
                arms,
                range,
            } => {
                let typed_scrutinee = self.infer_expr(scrutinee, locals);
                let result_ty = self.fresh_type_meta();
                let mut typed_arms = Vec::with_capacity(arms.len());

                if arms.is_empty() {
                    self.error(*range, "match expression must have at least one arm");
                }

                for arm in arms {
                    let mut arm_locals = locals.clone();
                    let (typed_pattern, bound_vars) =
                        self.check_pattern(&arm.pattern, typed_scrutinee.ty, &HashMap::new());
                    for bound in bound_vars {
                        let scheme = self.mono_scheme(bound.ty, bound.range);
                        self.insert_scheme_for_key(
                            &mut arm_locals,
                            &bound.key,
                            scheme,
                            bound.range,
                        );
                    }

                    let typed_body = self.infer_expr(&arm.body, &mut arm_locals);
                    self.constrain_types(arm.range, typed_body.ty, result_ty);

                    typed_arms.push(tir::MatchArm {
                        pattern: typed_pattern,
                        body: typed_body,
                        range: arm.range,
                    });
                }

                tir::Expr {
                    kind: tir::ExprKind::Match {
                        scrutinee: Box::new(typed_scrutinee),
                        arms: typed_arms,
                    },
                    ty: result_ty,
                    range: *range,
                }
            }
            lir::Expr::Apply {
                callee,
                argument,
                range,
            } => {
                let typed_callee = self.infer_expr(callee, locals);
                // Instantiate outer `forall`s at each use-site so repeated calls
                // get fresh quantifier instantiations.
                let opened_callee_ty = self.instantiate_type_for_use(typed_callee.ty, *range);

                let (argument_ty, result_ty) =
                    if let Some((argument_ty, result_ty)) = self.as_arrow_type(opened_callee_ty) {
                        (argument_ty, result_ty)
                    } else {
                        let argument_ty = self.fresh_type_meta();
                        let result_ty = self.fresh_type_meta();
                        let expected_callee_ty = self.store.mk_arrow(argument_ty, result_ty);
                        self.constrain_types(*range, opened_callee_ty, expected_callee_ty);
                        (argument_ty, result_ty)
                    };
                let typed_argument = self.check_expr_against(argument, argument_ty, locals);

                tir::Expr {
                    kind: tir::ExprKind::Apply {
                        callee: Box::new(typed_callee),
                        argument: Box::new(typed_argument),
                    },
                    ty: result_ty,
                    range: *range,
                }
            }
            lir::Expr::FieldAccess { expr, field, range } => {
                let typed_expr = self.infer_expr(expr, locals);
                let Some(field_name) = field.clone() else {
                    self.error(*range, "field access is missing a field name");
                    return tir::Expr {
                        kind: tir::ExprKind::FieldAccess {
                            expr: Box::new(typed_expr),
                            field: field.clone(),
                        },
                        ty: self.error_type(),
                        range: *range,
                    };
                };

                let result_ty = self.fresh_type_meta();
                let tail = self.fresh_row_meta();
                let row = self.store.mk_row_extend(field_name, result_ty, tail);
                let expected_record = self.store.mk_record(row);
                self.constrain_types(*range, typed_expr.ty, expected_record);

                tir::Expr {
                    kind: tir::ExprKind::FieldAccess {
                        expr: Box::new(typed_expr),
                        field: field.clone(),
                    },
                    ty: result_ty,
                    range: *range,
                }
            }
            lir::Expr::Name(name) => {
                let ty = self.instantiate_name(name, locals);
                tir::Expr {
                    kind: tir::ExprKind::Name(name.clone()),
                    ty,
                    range: name.range(),
                }
            }
            lir::Expr::Literal(literal) => tir::Expr {
                kind: tir::ExprKind::Literal(literal.clone()),
                ty: self.literal_type(literal),
                range: literal.range,
            },
            lir::Expr::Unit { range } => tir::Expr {
                kind: tir::ExprKind::Unit,
                ty: self.unit_type(),
                range: *range,
            },
            lir::Expr::Tuple { elements, range } => {
                let typed_elements = elements
                    .iter()
                    .map(|element| self.infer_expr(element, locals))
                    .collect::<Vec<_>>();
                let element_types = typed_elements
                    .iter()
                    .map(|element| element.ty)
                    .collect::<Vec<_>>();
                let tuple_ty = self.store.mk_tuple(&element_types);

                tir::Expr {
                    kind: tir::ExprKind::Tuple {
                        elements: typed_elements,
                    },
                    ty: tuple_ty,
                    range: *range,
                }
            }
            lir::Expr::Array { elements, range } => {
                let element_ty = self.fresh_type_meta();
                let array_ty = self.array_type(element_ty);
                let mut typed_elements = Vec::with_capacity(elements.len());

                for element in elements {
                    match element {
                        lir::ArrayElement::Item(item) => {
                            let typed_item = self.infer_expr(item, locals);
                            self.constrain_types(item.range(), typed_item.ty, element_ty);
                            typed_elements.push(tir::ArrayElement::Item(typed_item));
                        }
                        lir::ArrayElement::Spread { expr, range } => {
                            let typed_expr = self.infer_expr(expr, locals);
                            self.constrain_types(*range, typed_expr.ty, array_ty);
                            typed_elements.push(tir::ArrayElement::Spread {
                                expr: typed_expr,
                                range: *range,
                            });
                        }
                    }
                }

                tir::Expr {
                    kind: tir::ExprKind::Array {
                        elements: typed_elements,
                    },
                    ty: array_ty,
                    range: *range,
                }
            }
            lir::Expr::Record { fields, range } => {
                let mut field_types = BTreeMap::new();
                let mut typed_fields = Vec::with_capacity(fields.len());
                for field in fields {
                    let typed_value = self.infer_expr(&field.value, locals);

                    if let Some(name) = field.name.clone() {
                        if let Some(existing) = field_types.insert(name.clone(), typed_value.ty) {
                            self.error(field.range, format!("duplicate record field `{name}`"));
                            self.constrain_types(field.range, existing, typed_value.ty);
                        }
                    } else {
                        self.error(field.range, "record field is missing a name");
                    }

                    typed_fields.push(tir::RecordField {
                        name: field.name.clone(),
                        separator: field.separator,
                        value: typed_value,
                        range: field.range,
                    });
                }

                tir::Expr {
                    kind: tir::ExprKind::Record {
                        fields: typed_fields,
                    },
                    ty: self.mk_closed_record_from_fields(&field_types),
                    range: *range,
                }
            }
            lir::Expr::InlineWasm {
                result_type,
                locals: wasm_locals,
                instructions,
                range,
            } => {
                let mut type_env = TypeExprEnv::default();
                let result_ty = self.lower_type_expr(result_type, &mut type_env);
                self.expect_type_kind(result_type.range(), result_ty);

                tir::Expr {
                    kind: tir::ExprKind::InlineWasm {
                        result_type: result_ty,
                        locals: wasm_locals.clone(),
                        instructions: instructions.clone(),
                    },
                    ty: result_ty,
                    range: *range,
                }
            }
            lir::Expr::Error(error) => tir::Expr {
                kind: tir::ExprKind::Error(error.clone()),
                ty: self.error_type(),
                range: error.range,
            },
        }
    }

    /// Bidirectional checking entrypoint used when an expected type is known.
    ///
    /// This enables predicative higher-rank checking:
    /// - expected outer `forall` types are skolemized,
    /// - inferred outer `forall` types are instantiated with fresh metas,
    /// - resulting monotypes are unified.
    fn check_expr_against(
        &mut self,
        expr: &lir::Expr,
        expected: TypeId,
        locals: &mut LocalEnv,
    ) -> tir::Expr {
        let expected_resolved = self.table.shallow_resolve_type(&self.store, expected);
        if matches!(
            self.store.get_type(expected_resolved).kind,
            TypeKind::Forall(_, _, _)
        ) {
            // Expected polymorphism is checked via skolemization per use-site to
            // preserve predicativity.
            let (skolemized, _) = self.skolemize_outer_foralls(expected_resolved, expr.range());
            let mut typed = self.check_expr_against(expr, skolemized, locals);
            typed.ty = expected;
            return typed;
        }

        if let lir::Expr::Function {
            params,
            body,
            range,
        } = expr
            && let Some((parameter_types, result_ty)) =
                self.split_function_type(expected_resolved, params.len())
        {
            let mut function_locals = locals.clone();
            let mut typed_params = Vec::with_capacity(params.len());

            for (param, parameter_ty) in params.iter().zip(parameter_types.into_iter()) {
                let (typed_param, bound_vars) =
                    self.check_pattern(param, parameter_ty, &HashMap::new());
                for bound in bound_vars {
                    let scheme = self.mono_scheme(bound.ty, bound.range);
                    self.insert_scheme_for_key(
                        &mut function_locals,
                        &bound.key,
                        scheme,
                        bound.range,
                    );
                }
                typed_params.push(typed_param);
            }

            let typed_body = self.check_expr_against(body, result_ty, &mut function_locals);

            return tir::Expr {
                kind: tir::ExprKind::Function {
                    params: typed_params,
                    body: Box::new(typed_body),
                },
                ty: expected,
                range: *range,
            };
        }

        let mut typed = self.infer_expr(expr, locals);
        let diagnostic_count = self.diagnostics.len();
        self.constrain_poly_subsumption(expr.range(), typed.ty, expected);
        if self.diagnostics.len() == diagnostic_count {
            typed.ty = expected;
        }
        typed
    }

    fn split_function_type(&self, ty: TypeId, arity: usize) -> Option<(Vec<TypeId>, TypeId)> {
        let mut params = Vec::with_capacity(arity);
        let mut cursor = self.table.shallow_resolve_type(&self.store, ty);

        for _ in 0..arity {
            let (param, result) = self.as_arrow_type(cursor)?;
            params.push(param);
            cursor = self.table.shallow_resolve_type(&self.store, result);
        }

        Some((params, cursor))
    }

    fn as_arrow_type(&self, ty: TypeId) -> Option<(TypeId, TypeId)> {
        let ty = self.table.shallow_resolve_type(&self.store, ty);
        let TypeKind::Application(partial, to) = self.store.get_type(ty).kind else {
            return None;
        };
        let partial = self.table.shallow_resolve_type(&self.store, partial);
        let TypeKind::Application(head, from) = self.store.get_type(partial).kind else {
            return None;
        };
        let head = self.table.shallow_resolve_type(&self.store, head);
        let TypeKind::Constructor(TypeConstructor::Arrow) = self.store.get_type(head).kind else {
            return None;
        };
        Some((from, to))
    }

    fn type_contains_forall(&self, ty: TypeId) -> bool {
        let ty = self.table.shallow_resolve_type(&self.store, ty);
        match &self.store.get_type(ty).kind {
            TypeKind::Forall(_, _, _) => true,
            TypeKind::Lambda(_, body) => self.type_contains_forall(*body),
            TypeKind::Application(func, arg) => {
                self.type_contains_forall(*func) || self.type_contains_forall(*arg)
            }
            TypeKind::Record(row) => self.type_contains_forall(*row),
            TypeKind::RowExtend { field, tail, .. } => {
                self.type_contains_forall(*field) || self.type_contains_forall(*tail)
            }
            TypeKind::MetaTypeVariable(_)
            | TypeKind::RigidTypeVariable(_)
            | TypeKind::Constructor(_)
            | TypeKind::RowEmpty
            | TypeKind::Error => false,
        }
    }

    fn check_pattern(
        &mut self,
        pattern: &lir::Pattern,
        expected: TypeId,
        prebound: &HashMap<BinderKey, TypeId>,
    ) -> (tir::Pattern, Vec<BoundVar>) {
        match pattern {
            lir::Pattern::Constructor {
                constructor,
                argument,
                range,
            } => {
                let ctor_ty = self.instantiate_constructor(constructor);
                let argument_ty = self.fresh_type_meta();
                let result_ty = self.fresh_type_meta();
                let expected_ctor_ty = self.store.mk_arrow(argument_ty, result_ty);
                self.constrain_types(*range, ctor_ty, expected_ctor_ty);
                self.constrain_types(*range, expected, result_ty);

                let (typed_argument, bound) = self.check_pattern(argument, argument_ty, prebound);

                (
                    tir::Pattern {
                        kind: tir::PatternKind::Constructor {
                            constructor: constructor.clone(),
                            argument: Box::new(typed_argument),
                        },
                        ty: expected,
                        range: *range,
                    },
                    bound,
                )
            }
            lir::Pattern::ConstructorName { constructor, range } => {
                let ctor_ty = self.instantiate_constructor(constructor);
                self.constrain_types(*range, expected, ctor_ty);

                (
                    tir::Pattern {
                        kind: tir::PatternKind::ConstructorName {
                            constructor: constructor.clone(),
                        },
                        ty: expected,
                        range: *range,
                    },
                    Vec::new(),
                )
            }
            lir::Pattern::Binding { name, range } => {
                let key = name.binder_key();
                let mut bound = Vec::new();

                if let Some(key) = key {
                    if let Some(prebound_ty) = prebound.get(&key)
                        && !self.type_contains_forall(expected)
                    {
                        self.constrain_poly_subsumption(*range, *prebound_ty, expected);
                    }
                    let binding_ty = expected;

                    bound.push(BoundVar {
                        key,
                        ty: binding_ty,
                        range: *range,
                    });
                } else if let lir::ResolvedName::Error { name, .. } = name {
                    self.error(*range, format!("failed to type-check binding `{name}`"));
                }

                (
                    tir::Pattern {
                        kind: tir::PatternKind::Binding { name: name.clone() },
                        ty: expected,
                        range: *range,
                    },
                    bound,
                )
            }
            lir::Pattern::Hole { range } => (
                tir::Pattern {
                    kind: tir::PatternKind::Hole,
                    ty: expected,
                    range: *range,
                },
                Vec::new(),
            ),
            lir::Pattern::Annotated { pattern, ty, range } => {
                let mut type_env = TypeExprEnv::default();
                let annotation = self.lower_type_expr(ty, &mut type_env);
                self.expect_type_kind(ty.range(), annotation);
                // Subsumption is the boundary for annotated patterns, so
                // polymorphic annotations are checked without forcing
                // monomorphization.
                self.constrain_poly_subsumption(*range, expected, annotation);

                let (typed_pattern, bound) = self.check_pattern(pattern, annotation, prebound);

                (
                    tir::Pattern {
                        kind: tir::PatternKind::Annotated {
                            pattern: Box::new(typed_pattern),
                            annotation,
                        },
                        ty: expected,
                        range: *range,
                    },
                    bound,
                )
            }
            lir::Pattern::Literal(literal) => {
                let literal_ty = self.literal_type(literal);
                self.constrain_types(literal.range, expected, literal_ty);

                (
                    tir::Pattern {
                        kind: tir::PatternKind::Literal(literal.clone()),
                        ty: expected,
                        range: literal.range,
                    },
                    Vec::new(),
                )
            }
            lir::Pattern::Tuple { elements, range } => {
                let mut element_types = Vec::with_capacity(elements.len());
                for _ in elements {
                    element_types.push(self.fresh_type_meta());
                }

                let tuple_ty = self.store.mk_tuple(&element_types);
                self.constrain_types(*range, expected, tuple_ty);

                let mut typed_elements = Vec::with_capacity(elements.len());
                let mut bound = Vec::new();
                for (element, element_ty) in elements.iter().zip(element_types.into_iter()) {
                    let (typed_element, mut element_bound) =
                        self.check_pattern(element, element_ty, prebound);
                    typed_elements.push(typed_element);
                    bound.append(&mut element_bound);
                }

                (
                    tir::Pattern {
                        kind: tir::PatternKind::Tuple {
                            elements: typed_elements,
                        },
                        ty: expected,
                        range: *range,
                    },
                    bound,
                )
            }
            lir::Pattern::Array { elements, range } => {
                let element_ty = self.fresh_type_meta();
                let array_ty = self.array_type(element_ty);
                self.constrain_types(*range, expected, array_ty);

                let mut typed_elements = Vec::with_capacity(elements.len());
                let mut bound = Vec::new();

                for element in elements {
                    match element {
                        lir::ArrayPatternElement::Item(item) => {
                            let (typed_item, mut item_bound) =
                                self.check_pattern(item, element_ty, prebound);
                            typed_elements.push(tir::ArrayPatternElement::Item(typed_item));
                            bound.append(&mut item_bound);
                        }
                        lir::ArrayPatternElement::Rest { binding, range } => {
                            let mut typed_binding = None;

                            if let Some(binding_name) = binding
                                && let Some(key) = binding_name.binder_key()
                            {
                                let binding_ty = if let Some(prebound_ty) = prebound.get(&key) {
                                    self.constrain_types(*range, *prebound_ty, array_ty);
                                    *prebound_ty
                                } else {
                                    array_ty
                                };

                                bound.push(BoundVar {
                                    key,
                                    ty: binding_ty,
                                    range: *range,
                                });
                                typed_binding = Some(binding_name.clone());
                            }

                            typed_elements.push(tir::ArrayPatternElement::Rest {
                                binding: typed_binding,
                                range: *range,
                            });
                        }
                    }
                }

                (
                    tir::Pattern {
                        kind: tir::PatternKind::Array {
                            elements: typed_elements,
                        },
                        ty: expected,
                        range: *range,
                    },
                    bound,
                )
            }
            lir::Pattern::Record {
                fields,
                open,
                range,
            } => {
                let mut typed_fields = Vec::with_capacity(fields.len());
                let mut bound = Vec::new();
                let mut field_types = BTreeMap::new();

                for field in fields {
                    let field_ty = self.fresh_type_meta();
                    let typed_value = if let Some(value) = &field.value {
                        let (typed_value, mut value_bound) =
                            self.check_pattern(value, field_ty, prebound);
                        bound.append(&mut value_bound);
                        Some(typed_value)
                    } else {
                        None
                    };

                    if let Some(name) = field.name.clone() {
                        if let Some(existing) = field_types.insert(name.clone(), field_ty) {
                            self.error(
                                field.range,
                                format!("duplicate record field `{name}` in pattern"),
                            );
                            self.constrain_types(field.range, existing, field_ty);
                        }
                    } else {
                        self.error(field.range, "record pattern field is missing a name");
                    }

                    typed_fields.push(tir::RecordPatternField {
                        name: field.name.clone(),
                        value: typed_value,
                        range: field.range,
                    });
                }

                let tail = if *open {
                    self.fresh_row_meta()
                } else {
                    self.store.mk_row_empty()
                };
                let row = self.mk_row_from_fields(field_types, tail);
                let record_ty = self.store.mk_record(row);
                self.constrain_types(*range, expected, record_ty);

                (
                    tir::Pattern {
                        kind: tir::PatternKind::Record {
                            fields: typed_fields,
                            open: *open,
                        },
                        ty: expected,
                        range: *range,
                    },
                    bound,
                )
            }
            lir::Pattern::Error(error) => (
                tir::Pattern {
                    kind: tir::PatternKind::Error(error.clone()),
                    ty: self.error_type(),
                    range: error.range,
                },
                Vec::new(),
            ),
        }
    }

    fn instantiate_name(&mut self, name: &lir::ResolvedName, locals: &LocalEnv) -> TypeId {
        match name {
            lir::ResolvedName::Local { id, name, range } => {
                if let Some(scheme) = locals.terms.get(id).cloned() {
                    let typed = self.instantiate_scheme(&scheme);
                    self.instantiate_type_for_use(typed, *range)
                } else {
                    self.error(*range, format!("unbound local name `{name}`"));
                    self.error_type()
                }
            }
            lir::ResolvedName::Global(path) => {
                let key = path.text();
                if let Some(scheme) = self.global_terms.get(&key).cloned() {
                    let typed = self.instantiate_scheme(&scheme);
                    self.instantiate_type_for_use(typed, path.range)
                } else {
                    self.error(path.range, format!("unbound global name `{key}`"));
                    self.error_type()
                }
            }
            lir::ResolvedName::Error { name, range } => {
                self.error(*range, format!("failed to resolve name `{name}`"));
                self.error_type()
            }
        }
    }

    fn instantiate_constructor(&mut self, constructor: &lir::QualifiedName) -> TypeId {
        let key = constructor.text();
        if let Some(scheme) = self.global_terms.get(&key).cloned() {
            let typed = self.instantiate_scheme(&scheme);
            self.instantiate_type_for_use(typed, constructor.range)
        } else {
            self.error(
                constructor.range,
                format!("unknown constructor `{key}` in pattern"),
            );
            self.error_type()
        }
    }

    fn instantiate_type_for_use(&mut self, ty: TypeId, range: TextRange) -> TypeId {
        // Freshly instantiate outer `forall`s for each term usage to avoid
        // accidentally sharing polymorphic identity across call-sites.
        self.instantiate_outer_foralls_with_metas(ty, range)
    }

    fn instantiate_scheme(&mut self, scheme: &TypeScheme) -> TypeId {
        if scheme.binders.is_empty() {
            return scheme.body;
        }

        let mut substitution = HashMap::with_capacity(scheme.binders.len());
        for binder in &scheme.binders {
            let kind = self.default_unresolved_kind_to_type(binder.kind);
            let (_, meta) = self.table.fresh_type_var(&mut self.store, kind);
            substitution.insert(binder.id, meta);
        }

        self.substitute_rigid_type(scheme.body, &substitution)
    }

    fn substitute_rigid_type(
        &mut self,
        ty: TypeId,
        substitution: &HashMap<TypeBinderId, TypeId>,
    ) -> TypeId {
        let resolved = self.table.shallow_resolve_type(&self.store, ty);
        let ty_data = self.store.get_type(resolved).clone();

        match ty_data.kind {
            TypeKind::RigidTypeVariable(id) => substitution.get(&id).copied().unwrap_or(resolved),
            TypeKind::MetaTypeVariable(_)
            | TypeKind::Constructor(_)
            | TypeKind::RowEmpty
            | TypeKind::Error => resolved,
            TypeKind::Lambda(binder, body) => {
                let mut inner_substitution = substitution.clone();
                inner_substitution.remove(&binder.id);
                let sub_body = self.substitute_rigid_type(body, &inner_substitution);
                if sub_body == body {
                    resolved
                } else {
                    self.store.mk_lambda(binder, sub_body)
                }
            }
            TypeKind::Application(func, arg) => {
                let sub_func = self.substitute_rigid_type(func, substitution);
                let sub_arg = self.substitute_rigid_type(arg, substitution);
                let sub_kind = self.table.zonk_kind(&mut self.store, ty_data.kind_id);
                if sub_func == func && sub_arg == arg {
                    resolved
                } else {
                    self.store.mk_application(sub_func, sub_arg, sub_kind)
                }
            }
            TypeKind::Record(row) => {
                let sub_row = self.substitute_rigid_type(row, substitution);
                if sub_row == row {
                    resolved
                } else {
                    self.store.mk_record(sub_row)
                }
            }
            TypeKind::RowExtend { label, field, tail } => {
                let sub_field = self.substitute_rigid_type(field, substitution);
                let sub_tail = self.substitute_rigid_type(tail, substitution);
                if sub_field == field && sub_tail == tail {
                    resolved
                } else {
                    self.store.mk_row_extend(label, sub_field, sub_tail)
                }
            }
            TypeKind::Forall(binders, predicates, body) => {
                let mut inner_substitution = substitution.clone();
                for binder in &binders {
                    inner_substitution.remove(&binder.id);
                }

                let sub_body = self.substitute_rigid_type(body, &inner_substitution);
                let sub_predicates = predicates
                    .iter()
                    .map(|predicate| crate::ty::TraitPredicate {
                        trait_ref: predicate.trait_ref.clone(),
                        arguments: predicate
                            .arguments
                            .iter()
                            .map(|&argument| {
                                self.substitute_rigid_type(argument, &inner_substitution)
                            })
                            .collect(),
                        range: predicate.range,
                    })
                    .collect::<Vec<_>>();

                if sub_body == body && sub_predicates == predicates {
                    resolved
                } else {
                    self.store.mk_forall(binders, sub_predicates, sub_body)
                }
            }
        }
    }

    fn generalize_type(&mut self, ty: TypeId, threshold: usize, range: TextRange) -> TypeScheme {
        let zonked_ty = self.table.zonk_type(&mut self.store, ty);
        let mut vars = HashSet::new();
        self.collect_unsolved_metas(zonked_ty, &mut vars);

        let mut vars = vars
            .into_iter()
            .filter(|var| (var.0 as usize) >= threshold)
            .collect::<Vec<_>>();
        vars.sort_by_key(|var| var.0);

        let mut binders = Vec::with_capacity(vars.len());
        let mut substitution = HashMap::with_capacity(vars.len());

        for var in vars {
            if self.table.probe_type_var(var).is_some() {
                continue;
            }

            let kind = self
                .table
                .zonk_kind(&mut self.store, self.table.type_var_kind(var));
            let binder_id = self.fresh_type_binder_id();
            binders.push(TypeBinder {
                id: binder_id,
                name: format!("t{}", binder_id.0),
                kind,
                range,
            });
            substitution.insert(var, self.store.mk_rigid(binder_id, kind));
        }

        let body = self.substitute_meta_type(zonked_ty, &substitution);

        TypeScheme {
            binders,
            predicates: Vec::new(),
            body,
            range,
        }
    }

    fn collect_unsolved_metas(&self, ty: TypeId, vars: &mut HashSet<MetaTypeVariableId>) {
        let resolved = self.table.shallow_resolve_type(&self.store, ty);
        match &self.store.get_type(resolved).kind {
            TypeKind::MetaTypeVariable(var) => {
                if self.table.probe_type_var(*var).is_none() {
                    vars.insert(*var);
                }
            }
            TypeKind::Lambda(_, body) => self.collect_unsolved_metas(*body, vars),
            TypeKind::Application(func, arg) => {
                self.collect_unsolved_metas(*func, vars);
                self.collect_unsolved_metas(*arg, vars);
            }
            TypeKind::Record(row) => {
                self.collect_unsolved_metas(*row, vars);
            }
            TypeKind::RowExtend { field, tail, .. } => {
                self.collect_unsolved_metas(*field, vars);
                self.collect_unsolved_metas(*tail, vars);
            }
            TypeKind::Forall(_, predicates, body) => {
                self.collect_unsolved_metas(*body, vars);
                for predicate in predicates {
                    for argument in &predicate.arguments {
                        self.collect_unsolved_metas(*argument, vars);
                    }
                }
            }
            TypeKind::Constructor(_)
            | TypeKind::RigidTypeVariable(_)
            | TypeKind::RowEmpty
            | TypeKind::Error => {}
        }
    }

    fn substitute_meta_type(
        &mut self,
        ty: TypeId,
        substitution: &HashMap<MetaTypeVariableId, TypeId>,
    ) -> TypeId {
        let resolved = self.table.shallow_resolve_type(&self.store, ty);
        let ty_data = self.store.get_type(resolved).clone();

        match ty_data.kind {
            TypeKind::MetaTypeVariable(var) => substitution.get(&var).copied().unwrap_or(resolved),
            TypeKind::Constructor(_)
            | TypeKind::RigidTypeVariable(_)
            | TypeKind::RowEmpty
            | TypeKind::Error => resolved,
            TypeKind::Lambda(binder, body) => {
                let sub_body = self.substitute_meta_type(body, substitution);
                if sub_body == body {
                    resolved
                } else {
                    self.store.mk_lambda(binder, sub_body)
                }
            }
            TypeKind::Application(func, arg) => {
                let sub_func = self.substitute_meta_type(func, substitution);
                let sub_arg = self.substitute_meta_type(arg, substitution);
                if sub_func == func && sub_arg == arg {
                    resolved
                } else {
                    let sub_kind = self.table.zonk_kind(&mut self.store, ty_data.kind_id);
                    self.store.mk_application(sub_func, sub_arg, sub_kind)
                }
            }
            TypeKind::Record(row) => {
                let sub_row = self.substitute_meta_type(row, substitution);
                if sub_row == row {
                    resolved
                } else {
                    self.store.mk_record(sub_row)
                }
            }
            TypeKind::RowExtend { label, field, tail } => {
                let sub_field = self.substitute_meta_type(field, substitution);
                let sub_tail = self.substitute_meta_type(tail, substitution);
                if sub_field == field && sub_tail == tail {
                    resolved
                } else {
                    self.store.mk_row_extend(label, sub_field, sub_tail)
                }
            }
            TypeKind::Forall(binders, predicates, body) => {
                let sub_body = self.substitute_meta_type(body, substitution);
                let sub_predicates = predicates
                    .iter()
                    .map(|predicate| crate::ty::TraitPredicate {
                        trait_ref: predicate.trait_ref.clone(),
                        arguments: predicate
                            .arguments
                            .iter()
                            .map(|&argument| self.substitute_meta_type(argument, substitution))
                            .collect(),
                        range: predicate.range,
                    })
                    .collect::<Vec<_>>();

                if sub_body == body && sub_predicates == predicates {
                    resolved
                } else {
                    self.store.mk_forall(binders, sub_predicates, sub_body)
                }
            }
        }
    }

    fn lower_type_expr(&mut self, type_expr: &lir::TypeExpr, env: &mut TypeExprEnv) -> TypeId {
        match type_expr {
            lir::TypeExpr::Forall {
                params,
                body,
                constraints,
                range,
            } => {
                let (binders, added_ids, _) = self.bind_type_binders(params, env);

                let body_ty = self.lower_type_expr(body, env);
                self.expect_type_kind(body.range(), body_ty);

                let predicates = constraints
                    .iter()
                    .map(|constraint| crate::ty::TraitPredicate {
                        trait_ref: constraint.trait_ref.clone(),
                        arguments: constraint
                            .args
                            .iter()
                            .map(|argument| self.lower_type_expr(argument, env))
                            .collect(),
                        range: constraint.range,
                    })
                    .collect::<Vec<_>>();

                for id in added_ids {
                    env.locals.remove(&id);
                }

                let binders = self.finalize_decl_binders(&binders);
                let ty = self.store.mk_forall(binders, predicates, body_ty);
                self.expect_type_kind(*range, ty);
                ty
            }
            lir::TypeExpr::Lambda { params, body, .. } => {
                let (binders, added_ids, _) = self.bind_type_binders(params, env);
                let body_ty = self.lower_type_expr(body, env);

                for id in added_ids {
                    env.locals.remove(&id);
                }

                let binders = self.finalize_decl_binders(&binders);
                binders
                    .into_iter()
                    .rev()
                    .fold(body_ty, |body, binder| self.store.mk_lambda(binder, body))
            }
            lir::TypeExpr::Function {
                param,
                result,
                range,
            } => {
                let param_ty = self.lower_type_expr(param, env);
                let result_ty = self.lower_type_expr(result, env);
                self.expect_type_kind(param.range(), param_ty);
                self.expect_type_kind(result.range(), result_ty);
                let fn_ty = self.store.mk_arrow(param_ty, result_ty);
                self.expect_type_kind(*range, fn_ty);
                fn_ty
            }
            lir::TypeExpr::Apply {
                callee,
                argument,
                range,
            } => {
                if let Some(transparent_ty) =
                    self.try_lower_transparent_type_application(type_expr, env)
                {
                    return transparent_ty;
                }

                let callee_ty = self.lower_type_expr(callee, env);
                let argument_ty = self.lower_type_expr(argument, env);
                self.apply_type(callee_ty, argument_ty, *range)
            }
            lir::TypeExpr::Record { members, .. } => {
                let fields = self.lower_record_type_members(members, env);
                self.mk_closed_record_from_fields(&fields)
            }
            lir::TypeExpr::Name { name } => self.lower_type_name(name, env),
            lir::TypeExpr::Hole { .. } => self.fresh_type_meta(),
            lir::TypeExpr::Tuple { elements, .. } => {
                let element_tys = elements
                    .iter()
                    .map(|element| {
                        let element_ty = self.lower_type_expr(element, env);
                        self.expect_type_kind(element.range(), element_ty);
                        element_ty
                    })
                    .collect::<Vec<_>>();
                self.store.mk_tuple(&element_tys)
            }
            lir::TypeExpr::Unit { .. } => self.unit_type(),
            lir::TypeExpr::Array { .. } => self.array_constructor_type(),
            lir::TypeExpr::Error(_) => self.error_type(),
        }
    }

    fn lower_type_name(&mut self, name: &lir::ResolvedName, env: &TypeExprEnv) -> TypeId {
        match name {
            lir::ResolvedName::Local { id, name, range } => {
                if let Some(ty) = env.locals.get(id).copied() {
                    ty
                } else {
                    self.error(*range, format!("unknown type parameter `{name}`"));
                    self.error_type()
                }
            }
            lir::ResolvedName::Global(path) => {
                let key = path.text();
                if let Some(scheme) = self.alias_type_scheme(&key) {
                    self.scheme_to_type_lambda(&scheme)
                } else if self.declaration_type_transparency_enabled() {
                    if let Some(scheme) = self.record_type_scheme(&key) {
                        self.instantiate_scheme(&scheme)
                    } else {
                        self.ensure_named_type_constructor(path, None, path.range)
                    }
                } else {
                    self.ensure_named_type_constructor(path, None, path.range)
                }
            }
            lir::ResolvedName::Error { name, range } => {
                self.error(*range, format!("failed to resolve type name `{name}`"));
                self.error_type()
            }
        }
    }

    fn try_lower_transparent_type_application(
        &mut self,
        type_expr: &lir::TypeExpr,
        env: &mut TypeExprEnv,
    ) -> Option<TypeId> {
        let (head, args) = type_expr.collect_apply_chain();
        let lir::TypeExpr::Name { name } = head else {
            return None;
        };
        let lir::ResolvedName::Global(path) = name else {
            return None;
        };

        if args.is_empty() {
            return None;
        }

        let key = path.text();
        if let Some(scheme) = self.alias_type_scheme(&key) {
            let mut head_ty = self.scheme_to_type_lambda(&scheme);
            let expected_arity = self.kind_arity(self.store.get_type(head_ty).kind_id);
            let mut argument_types = Vec::with_capacity(args.len());
            for argument in args {
                let argument_ty = self.lower_type_expr(argument, env);
                argument_types.push(argument_ty);
            }

            if argument_types.len() > expected_arity {
                self.error(
                    type_expr.range(),
                    format!(
                        "type `{key}` expects {} type argument(s), found {}",
                        expected_arity,
                        argument_types.len()
                    ),
                );
                return Some(self.error_type());
            }

            for argument_ty in argument_types {
                head_ty = self.apply_type(head_ty, argument_ty, type_expr.range());
            }

            return Some(head_ty);
        }

        if !self.declaration_type_transparency_enabled() {
            return None;
        }

        let scheme = self.record_type_scheme(&key)?;

        let mut argument_types = Vec::with_capacity(args.len());
        for argument in args {
            let argument_ty = self.lower_type_expr(argument, env);
            argument_types.push(argument_ty);
        }

        if argument_types.len() > scheme.binders.len() {
            self.error(
                type_expr.range(),
                format!(
                    "type `{key}` expects {} type argument(s), found {}",
                    scheme.binders.len(),
                    argument_types.len()
                ),
            );
            return Some(self.error_type());
        }

        while argument_types.len() < scheme.binders.len() {
            let binder = &scheme.binders[argument_types.len()];
            let kind = self.default_unresolved_kind_to_type(binder.kind);
            argument_types.push(self.fresh_type_meta_with_kind(kind));
        }

        Some(self.instantiate_scheme_with_args(&scheme, &argument_types, type_expr.range()))
    }

    fn alias_type_scheme(&self, key: &str) -> Option<TypeScheme> {
        self.alias_schemes.get(key).cloned()
    }

    fn record_type_scheme(&self, key: &str) -> Option<TypeScheme> {
        self.record_schemes.get(key).cloned()
    }

    fn pierceable_named_type_scheme(&self, key: &str) -> Option<TypeScheme> {
        self.record_schemes
            .get(key)
            .or_else(|| self.opaque_schemes.get(key))
            .or_else(|| self.alias_schemes.get(key))
            .cloned()
    }

    fn scheme_to_type_lambda(&mut self, scheme: &TypeScheme) -> TypeId {
        scheme
            .binders
            .iter()
            .cloned()
            .rev()
            .fold(scheme.body, |body, binder| {
                self.store.mk_lambda(binder, body)
            })
    }

    fn kind_arity(&mut self, kind: KindId) -> usize {
        let mut current = self.table.zonk_kind(&mut self.store, kind);
        let mut arity = 0;

        while let Kind::Arrow(_, result) = self.store.get_kind(current).clone() {
            arity += 1;
            current = result;
        }

        arity
    }

    fn instantiate_scheme_with_args(
        &mut self,
        scheme: &TypeScheme,
        args: &[TypeId],
        range: TextRange,
    ) -> TypeId {
        let mut substitution = HashMap::with_capacity(scheme.binders.len());

        for (binder, argument) in scheme.binders.iter().zip(args.iter().copied()) {
            let expected_kind = self.default_unresolved_kind_to_type(binder.kind);
            let found_kind = self.store.get_type(argument).kind_id;
            self.constrain_kinds(range, expected_kind, found_kind);
            substitution.insert(binder.id, argument);
        }

        self.substitute_rigid_type(scheme.body, &substitution)
    }

    fn ensure_named_type_constructor(
        &mut self,
        path: &lir::QualifiedName,
        expected_kind: Option<KindId>,
        range: TextRange,
    ) -> TypeId {
        let key = path.text();
        if let Some(existing) = self.named_type_constructors.get(&key).copied() {
            if let Some(expected_kind) = expected_kind {
                let existing_kind = self.store.get_type(existing).kind_id;
                self.constrain_kinds(range, existing_kind, expected_kind);
            }
            return existing;
        }

        let kind = if let Some(expected_kind) = expected_kind {
            expected_kind
        } else {
            let (_, kind_var) = self.table.fresh_kind_var(&mut self.store);
            kind_var
        };

        let ty = self
            .store
            .mk_constructor(TypeConstructor::Named(path.clone()), kind);
        self.named_type_constructors.insert(key, ty);
        ty
    }

    fn kind_for_arity(&mut self, arity: usize) -> KindId {
        let kind_type = self.store.kind_type();
        let mut kind = kind_type;
        for _ in 0..arity {
            kind = self.store.kind_arrow(kind_type, kind);
        }
        kind
    }

    fn default_unresolved_kind_to_type(&mut self, kind: KindId) -> KindId {
        let zonked = self.table.zonk_kind(&mut self.store, kind);
        match self.store.get_kind(zonked).clone() {
            Kind::Variable(_) => {
                let type_kind = self.store.kind_type();
                let _ = self.table.unify_kinds(&self.store, zonked, type_kind);
                self.table.zonk_kind(&mut self.store, zonked)
            }
            Kind::Arrow(from, to) => {
                let default_from = self.default_unresolved_kind_to_type(from);
                let default_to = self.default_unresolved_kind_to_type(to);
                if default_from == from && default_to == to {
                    zonked
                } else {
                    self.store.kind_arrow(default_from, default_to)
                }
            }
            Kind::Type | Kind::Row | Kind::Error => zonked,
        }
    }

    fn fresh_row_meta(&mut self) -> TypeId {
        let kind = self.store.kind_row();
        let (_, ty) = self.table.fresh_type_var(&mut self.store, kind);
        ty
    }

    fn mk_row_from_fields(&mut self, fields: BTreeMap<String, TypeId>, tail: TypeId) -> TypeId {
        let mut row = tail;
        for (name, ty) in fields.into_iter().rev() {
            row = self.store.mk_row_extend(name, ty, row);
        }
        row
    }

    fn mk_closed_record_from_fields(&mut self, fields: &BTreeMap<String, TypeId>) -> TypeId {
        let row_empty = self.store.mk_row_empty();
        let row = self.mk_row_from_fields(fields.clone(), row_empty);
        self.store.mk_record(row)
    }

    fn lower_record_type_members(
        &mut self,
        members: &[lir::RecordTypeMember],
        env: &mut TypeExprEnv,
    ) -> BTreeMap<String, TypeId> {
        let mut fields = BTreeMap::new();

        for member in members {
            match member {
                lir::RecordTypeMember::Field { name, ty, range } => {
                    let member_ty = self.lower_type_expr(ty, env);
                    self.expect_type_kind(*range, member_ty);

                    let Some(name) = name.clone() else {
                        self.error(*range, "record field is missing a name");
                        continue;
                    };

                    if let Some(existing) = fields.insert(name.clone(), member_ty) {
                        self.error(*range, format!("duplicate record field `{name}`"));
                        self.constrain_types(*range, existing, member_ty);
                    }
                }
                lir::RecordTypeMember::Spread { ty, range } => {
                    let spread_ty = self.lower_type_expr(ty, env);
                    self.expect_type_kind(*range, spread_ty);

                    if let Some(spread_fields) = self.collect_closed_record_fields(spread_ty, *range)
                    {
                        for (name, member_ty) in spread_fields {
                            if let Some(existing) = fields.insert(name.clone(), member_ty) {
                                self.error(
                                    *range,
                                    format!("record spread introduces duplicate field `{name}`"),
                                );
                                self.constrain_types(*range, existing, member_ty);
                            }
                        }
                    }
                }
            }
        }

        fields
    }

    fn collect_type_apply_chain(&self, ty: TypeId) -> (TypeId, Vec<TypeId>) {
        let mut args_rev = Vec::new();
        let mut current = self.table.shallow_resolve_type(&self.store, ty);

        while let TypeKind::Application(func, arg) = self.store.get_type(current).kind {
            args_rev.push(arg);
            current = self.table.shallow_resolve_type(&self.store, func);
        }

        args_rev.reverse();
        (current, args_rev)
    }

    fn try_expand_declaration_record_type(
        &mut self,
        ty: TypeId,
        range: TextRange,
    ) -> Option<TypeId> {
        let mut visited_named_types = HashSet::new();
        let mut current = self.table.shallow_resolve_type(&self.store, ty);

        loop {
            let current_kind = self.store.get_type(current).kind.clone();
            if matches!(current_kind, TypeKind::Record(_) | TypeKind::Error) {
                return Some(current);
            }

            let (head, mut args) = self.collect_type_apply_chain(current);
            let TypeKind::Constructor(TypeConstructor::Named(path)) =
                self.store.get_type(head).kind.clone()
            else {
                return None;
            };

            let key = path.text();
            if !visited_named_types.insert(key.clone()) {
                return Some(current);
            }

            let scheme = self.pierceable_named_type_scheme(&key)?;

            if args.len() > scheme.binders.len() {
                self.error(
                    range,
                    format!(
                        "type `{key}` expects {} type argument(s), found {}",
                        scheme.binders.len(),
                        args.len()
                    ),
                );
                return Some(self.error_type());
            }

            while args.len() < scheme.binders.len() {
                let binder = &scheme.binders[args.len()];
                let kind = self.default_unresolved_kind_to_type(binder.kind);
                args.push(self.fresh_type_meta_with_kind(kind));
            }

            current = self.instantiate_scheme_with_args(&scheme, &args, range);
            current = self.table.shallow_resolve_type(&self.store, current);
        }
    }

    fn collect_closed_record_fields(
        &mut self,
        record_ty: TypeId,
        range: TextRange,
    ) -> Option<BTreeMap<String, TypeId>> {
        let mut record_ty = self.table.shallow_resolve_type(&self.store, record_ty);
        if !matches!(self.store.get_type(record_ty).kind, TypeKind::Record(_))
            && let Some(expanded) = self.try_expand_declaration_record_type(record_ty, range)
        {
            record_ty = self.table.shallow_resolve_type(&self.store, expanded);
        }

        let mut row = match self.store.get_type(record_ty).kind.clone() {
            TypeKind::Record(row) => row,
            TypeKind::Error => return None,
            _ => {
                self.error(range, "record spread target must be a record type");
                return None;
            }
        };

        let mut fields = BTreeMap::new();

        loop {
            row = self.table.shallow_resolve_type(&self.store, row);
            match self.store.get_type(row).kind.clone() {
                TypeKind::RowEmpty | TypeKind::Error => break,
                TypeKind::RowExtend { label, field, tail } => {
                    if let Some(existing) = fields.insert(label.clone(), field) {
                        self.error(
                            range,
                            format!("duplicate record field `{label}` in spread target"),
                        );
                        self.constrain_types(range, existing, field);
                    }
                    row = tail;
                }
                _ => {
                    self.error(range, "record spread target must be a closed record type");
                    return None;
                }
            }
        }

        Some(fields)
    }

    fn prebind_keys(
        &mut self,
        locals: &mut LocalEnv,
        keys: &[(BinderKey, TextRange)],
    ) -> HashMap<BinderKey, TypeId> {
        let mut seen = HashSet::new();
        let mut map = HashMap::new();

        for (key, range) in keys {
            if !seen.insert(key.clone()) {
                self.error(*range, "duplicate binding in pattern");
                continue;
            }

            let ty = self.fresh_type_meta();
            let scheme = self.mono_scheme(ty, *range);
            self.insert_scheme_for_key(locals, key, scheme, *range);
            map.insert(key.clone(), ty);
        }

        map
    }

    fn insert_scheme_for_key(
        &mut self,
        locals: &mut LocalEnv,
        key: &BinderKey,
        scheme: TypeScheme,
        range: TextRange,
    ) {
        match key {
            BinderKey::Local(id) => {
                locals.terms.insert(*id, scheme);
            }
            BinderKey::Global(name) => {
                if name.is_empty() {
                    self.error(range, "cannot bind unnamed global term");
                    return;
                }
                self.global_terms.insert(name.clone(), scheme);
            }
        }
    }

    fn constrain_types(&mut self, range: TextRange, expected: TypeId, found: TypeId) {
        if let Err(error) = self.table.unify_types(&mut self.store, expected, found) {
            self.error(range, self.unification_message(error));
        }
    }

    /// Constrain subtype relation between `actual` and `expected` under predicative
    /// higher-rank rules:
    /// - instantiate outer `forall`s in the source (`actual`) side with fresh metas,
    /// - skolemize outer `forall`s in the target (`expected`) side,
    /// - recurse through function arrows contravariantly on parameters and
    ///   covariantly on results.
    fn constrain_poly_subsumption(&mut self, range: TextRange, actual: TypeId, expected: TypeId) {
        let instantiated_actual = self.instantiate_type_for_use(actual, range);
        let (skolemized_expected, _) = self.skolemize_outer_foralls(expected, range);

        if let (Some((actual_param, actual_result)), Some((expected_param, expected_result))) = (
            self.as_arrow_type(instantiated_actual),
            self.as_arrow_type(skolemized_expected),
        ) {
            // Function subsumption is contravariant in arguments and covariant
            // in results.
            self.constrain_poly_subsumption(range, expected_param, actual_param);
            self.constrain_poly_subsumption(range, actual_result, expected_result);
            return;
        }

        self.constrain_types(range, instantiated_actual, skolemized_expected);
    }

    fn instantiate_outer_foralls_with_metas(&mut self, ty: TypeId, _range: TextRange) -> TypeId {
        let mut current = self.table.shallow_resolve_type(&self.store, ty);

        loop {
            let TypeKind::Forall(binders, predicates, body) =
                self.store.get_type(current).kind.clone()
            else {
                return current;
            };

            let mut substitution = HashMap::with_capacity(binders.len());
            for binder in &binders {
                let kind = self.table.zonk_kind(&mut self.store, binder.kind);
                let (_, meta) = self.table.fresh_type_var(&mut self.store, kind);
                substitution.insert(binder.id, meta);
            }

            self.defer_predicates_with_substitution(&predicates, &substitution);
            current = self.substitute_rigid_type(body, &substitution);
            current = self.table.shallow_resolve_type(&self.store, current);
        }
    }

    fn skolemize_outer_foralls(
        &mut self,
        ty: TypeId,
        _range: TextRange,
    ) -> (TypeId, Vec<TypeBinderId>) {
        let mut current = self.table.shallow_resolve_type(&self.store, ty);
        let mut skolems = Vec::new();

        loop {
            let TypeKind::Forall(binders, predicates, body) =
                self.store.get_type(current).kind.clone()
            else {
                return (current, skolems);
            };

            let mut substitution = HashMap::with_capacity(binders.len());
            for binder in &binders {
                let kind = self.table.zonk_kind(&mut self.store, binder.kind);
                let skolem_id = self.fresh_type_binder_id();
                let skolem = self.store.mk_rigid(skolem_id, kind);
                substitution.insert(binder.id, skolem);
                skolems.push(skolem_id);
            }

            self.defer_predicates_with_substitution(&predicates, &substitution);
            current = self.substitute_rigid_type(body, &substitution);
            current = self.table.shallow_resolve_type(&self.store, current);
        }
    }

    fn defer_predicates_with_substitution(
        &mut self,
        predicates: &[TraitPredicate],
        substitution: &HashMap<TypeBinderId, TypeId>,
    ) {
        let deferred = predicates
            .iter()
            .map(|predicate| TraitPredicate {
                trait_ref: predicate.trait_ref.clone(),
                arguments: predicate
                    .arguments
                    .iter()
                    .map(|&argument| self.substitute_rigid_type(argument, substitution))
                    .collect(),
                range: predicate.range,
            })
            .collect::<Vec<_>>();
        self.deferred_predicates.extend(deferred);
    }

    fn constrain_kinds(&mut self, range: TextRange, expected: KindId, found: KindId) {
        if let Err(error) = self.table.unify_kinds(&self.store, expected, found) {
            self.error(range, self.unification_message(error));
        }
    }

    fn expect_type_kind(&mut self, range: TextRange, ty: TypeId) {
        let kind = self.store.get_type(ty).kind_id;
        let type_kind = self.store.kind_type();
        self.constrain_kinds(range, kind, type_kind);
    }

    fn unification_message(&self, error: UnificationError) -> String {
        match error {
            UnificationError::TypeMismatch { .. } => "type mismatch".to_owned(),
            UnificationError::KindMismatch { .. } => "kind mismatch".to_owned(),
            UnificationError::OccursCheck { .. } => "infinite type detected".to_owned(),
            UnificationError::KindOccursCheck { .. } => "infinite kind detected".to_owned(),
        }
    }

    fn literal_type(&mut self, literal: &lir::Literal) -> TypeId {
        let k = self.store.kind_type();
        let ctor = match literal.value {
            lir::LiteralValue::Integer(_) => TypeConstructor::Integer,
            lir::LiteralValue::Natural(_) => TypeConstructor::Natural,
            lir::LiteralValue::Real(_) => TypeConstructor::Real,
            lir::LiteralValue::String(_) | lir::LiteralValue::FormatString(_) => {
                TypeConstructor::String
            }
            lir::LiteralValue::Glyph(_) => TypeConstructor::Glyph,
            lir::LiteralValue::Bool(_) => TypeConstructor::Bool,
        };
        self.store.mk_constructor(ctor, k)
    }

    fn unit_type(&mut self) -> TypeId {
        let k = self.store.kind_type();
        self.store.mk_constructor(TypeConstructor::UNIT, k)
    }

    fn bool_type(&mut self) -> TypeId {
        let k = self.store.kind_type();
        self.store.mk_constructor(TypeConstructor::Bool, k)
    }

    fn array_constructor_type(&mut self) -> TypeId {
        let kind = self.kind_for_arity(1);
        self.store.mk_constructor(TypeConstructor::Array, kind)
    }

    fn array_type(&mut self, element_ty: TypeId) -> TypeId {
        let result_kind = self.store.kind_type();
        let ctor = self.array_constructor_type();
        self.store.mk_application(ctor, element_ty, result_kind)
    }

    fn fresh_type_meta(&mut self) -> TypeId {
        self.fresh_type_meta_with_kind(self.store.kind_type())
    }

    fn fresh_type_meta_with_kind(&mut self, kind: KindId) -> TypeId {
        let (_, ty) = self.table.fresh_type_var(&mut self.store, kind);
        ty
    }

    fn fresh_type_binder_id(&mut self) -> TypeBinderId {
        let id = TypeBinderId(self.next_type_binder);
        self.next_type_binder = self.next_type_binder.saturating_add(1);
        id
    }

    fn error_type(&mut self) -> TypeId {
        self.store.mk_error()
    }

    fn mono_scheme(&self, ty: TypeId, range: TextRange) -> TypeScheme {
        TypeScheme {
            binders: Vec::new(),
            predicates: Vec::new(),
            body: ty,
            range,
        }
    }

    fn error(&mut self, range: TextRange, message: impl Into<String>) {
        self.diagnostics.push(Diagnostic::error(range, message));
    }

    fn zonk_source(&mut self, source: &mut tir::Source) {
        for module in &mut source.modules {
            for statement in &mut module.statements {
                self.zonk_statement(statement);
            }
        }
    }

    fn zonk_statement(&mut self, statement: &mut tir::Statement) {
        match statement {
            tir::Statement::Let { kind, .. } => match kind {
                tir::LetStatementKind::PatternBinding { pattern, value } => {
                    self.zonk_pattern(pattern);
                    self.zonk_expr(value);
                }
                tir::LetStatementKind::ConstructorAlias { .. } => {}
            },
            tir::Statement::ModuleDecl { .. }
            | tir::Statement::Type { .. }
            | tir::Statement::Trait { .. }
            | tir::Statement::TraitAlias { .. }
            | tir::Statement::Impl { .. }
            | tir::Statement::Wasm { .. }
            | tir::Statement::Error(_) => {}
        }
    }

    fn zonk_expr(&mut self, expr: &mut tir::Expr) {
        expr.ty = self.table.zonk_type(&mut self.store, expr.ty);
        match &mut expr.kind {
            tir::ExprKind::Let {
                pattern,
                value,
                body,
            } => {
                self.zonk_pattern(pattern);
                self.zonk_expr(value);
                self.zonk_expr(body);
            }
            tir::ExprKind::Function { params, body } => {
                for param in params {
                    self.zonk_pattern(param);
                }
                self.zonk_expr(body);
            }
            tir::ExprKind::If {
                condition,
                then_branch,
                else_branch,
            } => {
                self.zonk_expr(condition);
                self.zonk_expr(then_branch);
                self.zonk_expr(else_branch);
            }
            tir::ExprKind::Match { scrutinee, arms } => {
                self.zonk_expr(scrutinee);
                for arm in arms {
                    self.zonk_pattern(&mut arm.pattern);
                    self.zonk_expr(&mut arm.body);
                }
            }
            tir::ExprKind::Apply { callee, argument } => {
                self.zonk_expr(callee);
                self.zonk_expr(argument);
            }
            tir::ExprKind::FieldAccess { expr, .. } => {
                self.zonk_expr(expr);
            }
            tir::ExprKind::Tuple { elements } => {
                for element in elements {
                    self.zonk_expr(element);
                }
            }
            tir::ExprKind::Array { elements } => {
                for element in elements {
                    match element {
                        tir::ArrayElement::Item(item) => self.zonk_expr(item),
                        tir::ArrayElement::Spread { expr, .. } => self.zonk_expr(expr),
                    }
                }
            }
            tir::ExprKind::Record { fields } => {
                for field in fields {
                    self.zonk_expr(&mut field.value);
                }
            }
            tir::ExprKind::InlineWasm { result_type, .. } => {
                *result_type = self.table.zonk_type(&mut self.store, *result_type);
            }
            tir::ExprKind::Name(_)
            | tir::ExprKind::Literal(_)
            | tir::ExprKind::Unit
            | tir::ExprKind::Error(_) => {}
        }
    }

    fn zonk_pattern(&mut self, pattern: &mut tir::Pattern) {
        pattern.ty = self.table.zonk_type(&mut self.store, pattern.ty);
        match &mut pattern.kind {
            tir::PatternKind::Constructor { argument, .. } => self.zonk_pattern(argument),
            tir::PatternKind::Annotated {
                pattern,
                annotation,
            } => {
                self.zonk_pattern(pattern);
                *annotation = self.table.zonk_type(&mut self.store, *annotation);
            }
            tir::PatternKind::Tuple { elements } => {
                for element in elements {
                    self.zonk_pattern(element);
                }
            }
            tir::PatternKind::Array { elements } => {
                for element in elements {
                    if let tir::ArrayPatternElement::Item(item) = element {
                        self.zonk_pattern(item);
                    }
                }
            }
            tir::PatternKind::Record { fields, .. } => {
                for field in fields {
                    if let Some(value) = &mut field.value {
                        self.zonk_pattern(value);
                    }
                }
            }
            tir::PatternKind::ConstructorName { .. }
            | tir::PatternKind::Binding { .. }
            | tir::PatternKind::Hole
            | tir::PatternKind::Literal(_)
            | tir::PatternKind::Error(_) => {}
        }
    }
}

impl lir::TypeExpr {
    fn collect_apply_chain(&self) -> (&lir::TypeExpr, Vec<&lir::TypeExpr>) {
        let mut args_rev = Vec::new();
        let mut cursor = self;

        while let lir::TypeExpr::Apply {
            callee, argument, ..
        } = cursor
        {
            args_rev.push(argument.as_ref());
            cursor = callee.as_ref();
        }

        args_rev.reverse();
        (cursor, args_rev)
    }

    fn peel_lambdas(&self) -> (Vec<lir::TypeBinder>, &lir::TypeExpr) {
        let mut params = Vec::new();
        let mut cursor = self;

        while let lir::TypeExpr::Lambda {
            params: lambda_params,
            body,
            ..
        } = cursor
        {
            params.extend(lambda_params.iter().cloned());
            cursor = body.as_ref();
        }

        (params, cursor)
    }

    fn range(&self) -> TextRange {
        match self {
            lir::TypeExpr::Forall { range, .. }
            | lir::TypeExpr::Lambda { range, .. }
            | lir::TypeExpr::Function { range, .. }
            | lir::TypeExpr::Apply { range, .. }
            | lir::TypeExpr::Record { range, .. }
            | lir::TypeExpr::Hole { range }
            | lir::TypeExpr::Tuple { range, .. }
            | lir::TypeExpr::Unit { range }
            | lir::TypeExpr::Array { range } => *range,
            lir::TypeExpr::Name { name } => name.range(),
            lir::TypeExpr::Error(error) => error.range,
        }
    }
}

impl lir::TypeDefinition {
    fn peel_lambdas(&self) -> (Vec<lir::TypeBinder>, &lir::TypeDefinition) {
        let mut params = Vec::new();
        let mut cursor = self;

        while let lir::TypeDefinition::Lambda {
            params: lambda_params,
            body,
            ..
        } = cursor
        {
            params.extend(lambda_params.iter().cloned());
            cursor = body.as_ref();
        }

        (params, cursor)
    }
}

impl lir::Pattern {
    fn collect_binders(&self) -> Vec<(BinderKey, TextRange)> {
        let mut binders = Vec::new();
        self.collect_bindings_into(&mut binders);
        binders
    }

    fn collect_bindings_into(&self, binders: &mut Vec<(BinderKey, TextRange)>) {
        match self {
            lir::Pattern::Constructor { argument, .. } => {
                argument.collect_bindings_into(binders);
            }
            lir::Pattern::ConstructorName { .. }
            | lir::Pattern::Literal(_)
            | lir::Pattern::Hole { .. } => {}
            lir::Pattern::Binding { name, range } => {
                if let Some(key) = name.binder_key() {
                    binders.push((key, *range));
                }
            }
            lir::Pattern::Annotated { pattern, .. } => {
                pattern.collect_bindings_into(binders);
            }
            lir::Pattern::Tuple { elements, .. } => {
                for element in elements {
                    element.collect_bindings_into(binders);
                }
            }
            lir::Pattern::Array { elements, .. } => {
                for element in elements {
                    match element {
                        lir::ArrayPatternElement::Item(item) => {
                            item.collect_bindings_into(binders);
                        }
                        lir::ArrayPatternElement::Rest { binding, range } => {
                            if let Some(binding) = binding
                                && let Some(key) = binding.binder_key()
                            {
                                binders.push((key, *range));
                            }
                        }
                    }
                }
            }
            lir::Pattern::Record { fields, .. } => {
                for field in fields {
                    if let Some(value) = &field.value {
                        value.collect_bindings_into(binders);
                    }
                }
            }
            lir::Pattern::Error(_) => {}
        }
    }
}

impl lir::ResolvedName {
    fn binder_key(&self) -> Option<BinderKey> {
        match self {
            lir::ResolvedName::Local { id, .. } => Some(BinderKey::Local(*id)),
            lir::ResolvedName::Global(path) => Some(BinderKey::Global(path.text())),
            lir::ResolvedName::Error { .. } => None,
        }
    }

    fn range(&self) -> TextRange {
        match self {
            lir::ResolvedName::Global(path) => path.range,
            lir::ResolvedName::Local { range, .. } | lir::ResolvedName::Error { range, .. } => {
                *range
            }
        }
    }
}

impl lir::Expr {
    fn range(&self) -> TextRange {
        match self {
            lir::Expr::Let { range, .. }
            | lir::Expr::Function { range, .. }
            | lir::Expr::If { range, .. }
            | lir::Expr::Match { range, .. }
            | lir::Expr::Apply { range, .. }
            | lir::Expr::FieldAccess { range, .. }
            | lir::Expr::Unit { range }
            | lir::Expr::Tuple { range, .. }
            | lir::Expr::Array { range, .. }
            | lir::Expr::Record { range, .. }
            | lir::Expr::InlineWasm { range, .. } => *range,
            lir::Expr::Name(name) => name.range(),
            lir::Expr::Literal(literal) => literal.range,
            lir::Expr::Error(error) => error.range,
        }
    }
}
