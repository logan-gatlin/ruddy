use super::ast;
use super::token::{Keyword, Operator, Punct, Token, TokenKind};
use crate::engine::Source;
use crate::parser::BundleDependencySource;
use crate::reporting::{Diag, Diagnostic, TextRange, TextSize};
use salsa::Accumulator;
use semver::{Version, VersionReq};

pub(super) struct ParseResult {
    pub file: ast::AstFile,
}

const DEFAULT_BUNDLE_VERSION_TEXT: &str = "0.0.0";

pub(super) fn parse_file(
    db: &dyn salsa::Database,
    source: Source,
    tokens: &[Token],
    source_len: TextSize,
) -> ParseResult {
    let mut parser = Parser::new(db, source, tokens, source_len);
    let file = parser.parse_file();
    parser.finish(file)
}

#[derive(Clone, Copy)]
enum ExprBoundary {
    Statement,
    ImplItem,
}

#[derive(Clone, Copy)]
struct ExprStop {
    boundary: ExprBoundary,
    stop_on_in: bool,
    stop_on_then: bool,
    stop_on_else: bool,
    stop_on_with: bool,
    stop_on_pipe: bool,
    stop_on_comma: bool,
    stop_on_rparen: bool,
    stop_on_rbracket: bool,
    stop_on_rbrace: bool,
}

impl ExprStop {
    fn for_boundary(boundary: ExprBoundary) -> Self {
        Self {
            boundary,
            stop_on_in: false,
            stop_on_then: false,
            stop_on_else: false,
            stop_on_with: false,
            stop_on_pipe: false,
            stop_on_comma: false,
            stop_on_rparen: false,
            stop_on_rbracket: false,
            stop_on_rbrace: false,
        }
    }

    fn with_in(mut self) -> Self {
        self.stop_on_in = true;
        self
    }

    fn with_then(mut self) -> Self {
        self.stop_on_then = true;
        self
    }

    fn with_else(mut self) -> Self {
        self.stop_on_else = true;
        self
    }

    fn with_with(mut self) -> Self {
        self.stop_on_with = true;
        self
    }

    fn with_pipe(mut self) -> Self {
        self.stop_on_pipe = true;
        self
    }

    fn with_comma(mut self) -> Self {
        self.stop_on_comma = true;
        self
    }

    fn with_rparen(mut self) -> Self {
        self.stop_on_rparen = true;
        self
    }

    fn with_rbracket(mut self) -> Self {
        self.stop_on_rbracket = true;
        self
    }

    fn with_rbrace(mut self) -> Self {
        self.stop_on_rbrace = true;
        self
    }
}

struct Parser<'a> {
    tokens: &'a [Token],
    db: &'a dyn salsa::Database,
    source: Source,
    source_len: TextSize,
    pos: usize,
    last_bundle_declaration_metadata: Option<ast::BundleMetadata>,
}

impl<'a> Parser<'a> {
    fn new(
        db: &'a dyn salsa::Database,
        source: Source,
        tokens: &'a [Token],
        source_len: TextSize,
    ) -> Self {
        Self {
            db,
            tokens,
            source,
            source_len,
            pos: 0,
            last_bundle_declaration_metadata: None,
        }
    }

    fn finish(self, file: ast::AstFile) -> ParseResult {
        ParseResult { file }
    }

    fn parse_file(&mut self) -> ast::AstFile {
        let mut statements = Vec::new();
        let mut bundle_name = None;
        let mut bundle_metadata = None;

        while !self.at_eof() {
            let is_file_top = statements.is_empty();
            if let Some(statement) = self.parse_statement(is_file_top) {
                if is_file_top
                    && let ast::Statement::Bundle {
                        name: declared_name,
                        ..
                    } = &statement
                {
                    bundle_name = declared_name.clone();
                    bundle_metadata = self.last_bundle_declaration_metadata.clone();
                }
                statements.push(statement);
            } else {
                let error_start = self.pos;
                self.error_current("expected statement");
                self.recover_statement();
                statements.push(ast::Statement::Error(self.error_node(error_start)));
            }
        }

        ast::AstFile {
            bundle_name,
            bundle_metadata,
            range: TextRange::new(self.source, TextSize::ZERO, self.source_len),
            statements,
        }
    }

    fn parse_statement(&mut self, allow_bundle_declaration: bool) -> Option<ast::Statement> {
        self.last_bundle_declaration_metadata = None;
        match self.current_keyword() {
            Some(Keyword::Bundle) => Some(self.parse_bundle_declaration(allow_bundle_declaration)),
            Some(Keyword::Module) => Some(self.parse_module_statement()),
            Some(Keyword::Let) => Some(self.parse_let_statement()),
            Some(Keyword::Do) => Some(self.parse_do_statement()),
            Some(Keyword::Use) => Some(self.parse_use_statement()),
            Some(Keyword::Type) => Some(self.parse_type_statement()),
            Some(Keyword::Trait) => Some(self.parse_trait_statement()),
            Some(Keyword::Impl) => Some(self.parse_impl_statement()),
            Some(Keyword::Wasm) => Some(self.parse_wasm_statement()),
            _ => None,
        }
    }

    fn parse_bundle_declaration(&mut self, allow_bundle_declaration: bool) -> ast::Statement {
        let start = self.pos;
        if !allow_bundle_declaration {
            self.error_current("bundle declaration must be the first statement in the file");
        }
        self.bump();
        let name = self.expect_ident_node("bundle name");
        let raw_metadata = if self.eat_keyword(Keyword::With) {
            if let Some(payload) = self.parse_sexpr() {
                Some(payload)
            } else {
                self.error_current(
                    "expected S-expression metadata payload after `with` in bundle declaration",
                );
                None
            }
        } else {
            None
        };
        let range = self.range_from(start);
        self.last_bundle_declaration_metadata =
            Some(self.normalize_bundle_metadata_payload(range, raw_metadata));
        ast::Statement::Bundle { name, range }
    }

    fn normalize_bundle_metadata_payload(
        &mut self,
        declaration_range: TextRange,
        raw: Option<ast::SExpr>,
    ) -> ast::BundleMetadata {
        let mut version = None;
        let mut saw_version_entry = false;
        let mut version_entries = 0usize;
        let mut dependencies_entries = 0usize;
        let mut metadata_entries = 0usize;
        let mut dependencies = Vec::new();
        let mut metadata = Vec::new();

        if let Some(payload) = raw.as_ref()
            && let Some(entries) = self.sexpr_list_items(payload)
        {
            for entry in entries {
                let Some(entry_items) = self.sexpr_list_items(entry) else {
                    self.error_at(
                        entry.range(),
                        "bundle metadata entry must be a list form `(key ...)`",
                    );
                    continue;
                };

                let Some((head, tail)) = entry_items.split_first() else {
                    self.error_at(entry.range(), "bundle metadata entry must not be empty");
                    continue;
                };

                let Some(key) = self.bundle_metadata_entry_key(head) else {
                    self.error_at(head.range(), "bundle metadata key must be an identifier");
                    continue;
                };

                match key.as_str() {
                    "version" => {
                        version_entries += 1;
                        if version_entries > 1 {
                            self.warn_at(
                                entry.range(),
                                "duplicate `version` metadata entry; last valid value wins",
                            );
                        }

                        saw_version_entry = true;
                        if let Some(parsed_version) =
                            self.parse_bundle_version_metadata(entry, tail)
                        {
                            version = Some(parsed_version);
                        }
                    }
                    "dependencies" => {
                        dependencies_entries += 1;
                        if dependencies_entries > 1 {
                            self.warn_at(
                                entry.range(),
                                "duplicate `dependencies` metadata entry; concatenating entries",
                            );
                        }
                        for dependency in tail {
                            if let Some(dependency) = self.parse_bundle_dependency(dependency) {
                                dependencies.push(dependency);
                            }
                        }
                    }
                    "metadata" => {
                        metadata_entries += 1;
                        if metadata_entries > 1 {
                            self.warn_at(
                                entry.range(),
                                "duplicate `metadata` metadata entry; concatenating entries",
                            );
                        }
                        metadata.extend(tail.iter().cloned());
                    }
                    _ => self.warn_at(
                        entry.range(),
                        format!("unknown bundle metadata key `{key}`"),
                    ),
                }
            }

            if !saw_version_entry {
                self.error_at(
                    payload.range(),
                    format!(
                        "bundle metadata requires `(version \"...\")`; defaulting to `{DEFAULT_BUNDLE_VERSION_TEXT}`"
                    ),
                );
            }
        }

        let range = raw
            .as_ref()
            .map(ast::SExpr::range)
            .unwrap_or(declaration_range);

        ast::BundleMetadata {
            range,
            raw,
            version: version.unwrap_or_else(default_bundle_version),
            dependencies,
            metadata,
        }
    }

    fn bundle_metadata_entry_key(&self, node: &ast::SExpr) -> Option<String> {
        let ast::SExpr::Atom {
            kind: ast::SExprAtomKind::Ident,
            range,
        } = node
        else {
            return None;
        };

        range.text(self.source.contents(self.db))
    }

    fn parse_bundle_version_metadata(
        &mut self,
        entry: &ast::SExpr,
        args: &[ast::SExpr],
    ) -> Option<Version> {
        if args.len() != 1 {
            self.error_at(
                entry.range(),
                "bundle metadata `version` must have shape `(version \"<value>\")`",
            );
            return None;
        }

        let value = &args[0];
        let parsed = self.parse_sexpr_string_literal(value, "bundle metadata `version` value")?;

        match Version::parse(&parsed) {
            Ok(version) => Some(version),
            Err(error) => {
                self.error_at(
                    value.range(),
                    format!("bundle metadata `version` must be a valid semver version: {error}"),
                );
                None
            }
        }
    }

    fn parse_bundle_dependency(
        &mut self,
        dependency: &ast::SExpr,
    ) -> Option<ast::BundleDependency> {
        let Some(items) = self.sexpr_list_items(dependency) else {
            self.error_at(
                dependency.range(),
                "bundle dependency entry must be a list form `(dep <name> \"<version-req>\" ((path|git) \"...\")?)`",
            );
            return None;
        };

        let Some((head, args)) = items.split_first() else {
            self.error_at(
                dependency.range(),
                "bundle dependency entry must not be empty",
            );
            return None;
        };

        let Some(head_text) = self.bundle_metadata_entry_key(head) else {
            self.error_at(
                head.range(),
                "bundle dependency entry head must be an identifier",
            );
            return None;
        };

        if head_text != "dep" {
            self.error_at(
                head.range(),
                format!("expected bundle dependency entry head `dep`, found `{head_text}`"),
            );
            return None;
        }

        if args.len() < 2 {
            self.error_at(
                dependency.range(),
                "bundle dependency entry must have shape `(dep <name> \"<version-req>\" ((path|git) \"...\")?)`",
            );
            return None;
        }

        let Some(name) = self.bundle_metadata_entry_key(&args[0]) else {
            self.error_at(
                args[0].range(),
                "bundle dependency name must be an identifier",
            );
            return None;
        };

        let version_text =
            self.parse_sexpr_string_literal(&args[1], "bundle dependency version requirement")?;

        let version = match VersionReq::parse(&version_text) {
            Ok(version) => version,
            Err(error) => {
                self.error_at(
                    args[1].range(),
                    format!(
                        "bundle dependency version requirement must be a valid semver requirement: {error}"
                    ),
                );
                return None;
            }
        };

        let mut source = BundleDependencySource::default();
        let mut has_source_errors = false;

        if args.len() == 3 {
            let source_node = &args[2];
            if let Some(parsed_source) = self.parse_bundle_dependency_source(source_node) {
                source = parsed_source;
            } else {
                has_source_errors = true;
            }
        } else if args.len() > 3 {
            self.error_at(
                dependency.range(),
                "bundle dependency entry has too many arguments; expected `(dep <name> \"<version-req>\" ((path|git) \"...\")?)`",
            );
            has_source_errors = true;
        }

        if has_source_errors {
            return None;
        }

        Some(ast::BundleDependency {
            range: dependency.range(),
            name,
            version,
            source,
        })
    }

    fn parse_bundle_dependency_source(
        &mut self,
        source: &ast::SExpr,
    ) -> Option<ast::BundleDependencySource> {
        let Some(items) = self.sexpr_list_items(source) else {
            self.error_at(
                source.range(),
                "bundle dependency source must be a list form `(path \"...\")` or `(git \"...\")`",
            );
            return None;
        };

        let Some((head, args)) = items.split_first() else {
            self.error_at(source.range(), "bundle dependency source must not be empty");
            return None;
        };

        let Some(kind) = self.bundle_metadata_entry_key(head) else {
            self.error_at(
                head.range(),
                "bundle dependency source key must be an identifier",
            );
            return None;
        };

        if args.len() != 1 {
            self.error_at(
                source.range(),
                format!("bundle dependency `{kind}` source must have shape `({kind} \"...\")`"),
            );
            return None;
        }

        let value = match kind.as_str() {
            "path" => {
                self.parse_sexpr_string_literal(&args[0], "bundle dependency `path` source value")?
            }
            "git" => {
                self.parse_sexpr_string_literal(&args[0], "bundle dependency `git` source value")?
            }
            _ => {
                self.error_at(
                    head.range(),
                    format!("bundle dependency source must be `path` or `git`, found `{kind}`"),
                );
                return None;
            }
        };

        match kind.as_str() {
            "path" => Some(ast::BundleDependencySource::Path(value)),
            "git" => Some(ast::BundleDependencySource::Git(value)),
            _ => None,
        }
    }

    fn parse_sexpr_string_literal(&mut self, value: &ast::SExpr, context: &str) -> Option<String> {
        let ast::SExpr::Atom {
            kind: ast::SExprAtomKind::String,
            range,
        } = value
        else {
            self.error_at(value.range(), format!("{context} must be a string literal"));
            return None;
        };

        let Some(raw_text) = range.text(self.source.contents(self.db)) else {
            self.error_at(value.range(), format!("failed to read {context} literal"));
            return None;
        };

        let Some(parsed) = decode_string_literal(&raw_text) else {
            self.error_at(
                value.range(),
                format!("failed to decode {context} string literal"),
            );
            return None;
        };

        Some(parsed)
    }

    fn parse_module_statement(&mut self) -> ast::Statement {
        let start = self.pos;
        self.bump();
        let name = self.expect_ident_node("module name");
        // Inline module
        if self.eat_punct(Punct::Equals) {
            let mut body = Vec::new();
            while !self.at_eof() && !self.at_keyword(Keyword::End) {
                if let Some(statement) = self.parse_statement(false) {
                    body.push(statement);
                } else {
                    let error_start = self.pos;
                    self.error_current("expected statement in module body");
                    self.recover_statement();
                    body.push(ast::Statement::Error(self.error_node(error_start)));
                }
            }

            self.expect_keyword(Keyword::End, "expected `end` to close module statement");

            ast::Statement::Module {
                name,
                body,
                range: self.range_from(start),
            }
        }
        // Module reference
        else {
            let in_loc = if self.eat_keyword(Keyword::In) {
                self.eat_string_literal_node()
            } else {
                None
            };
            ast::Statement::ModuleRef {
                name,
                in_loc,
                range: self.range_from(start),
            }
        }
    }

    fn parse_let_statement(&mut self) -> ast::Statement {
        let start = self.pos;
        self.bump();

        let kind = if self.eat_punct(Punct::Pipe) {
            let alias = self.expect_ident_node("let pipe alias name");
            self.expect_punct(Punct::Equals, "expected `=` in let pipe statement");
            let target = self.expect_ident_or_path_node("identifier or path in let pipe statement");
            ast::LetStatementKind::ConstructorAlias { alias, target }
        } else {
            let pattern = if let Some(pattern) = self.parse_pattern() {
                pattern
            } else {
                let error_start = self.pos;
                self.error_current("expected pattern in let statement");
                self.recover_until(|parser| parser.at_punct(Punct::Equals) || parser.at_eof());
                ast::Pattern::Error(self.error_node(error_start))
            };

            self.expect_punct(Punct::Equals, "expected `=` in let statement");
            let value = self.parse_expression(ExprBoundary::Statement);

            ast::LetStatementKind::PatternBinding { pattern, value }
        };

        ast::Statement::Let {
            kind,
            range: self.range_from(start),
        }
    }

    fn parse_do_statement(&mut self) -> ast::Statement {
        let start = self.pos;
        self.bump();
        let expr = self.parse_expression(ExprBoundary::Statement);

        ast::Statement::Do {
            expr,
            range: self.range_from(start),
        }
    }

    fn parse_use_statement(&mut self) -> ast::Statement {
        let start = self.pos;
        self.bump();
        let target = self.expect_use_target_node("use statement");
        let alias = if self.eat_keyword(Keyword::As) {
            self.expect_ident_node("use alias name")
        } else {
            None
        };

        ast::Statement::Use {
            target,
            alias,
            range: self.range_from(start),
        }
    }

    fn parse_type_statement(&mut self) -> ast::Statement {
        let start = self.pos;
        self.bump();

        let is_alias = self.eat_punct(Punct::Tilde);
        let name = self.expect_ident_node("type name");
        let declared_kind = self.parse_kind_annotation_opt();
        self.expect_punct(Punct::Equals, "expected `=` in type statement");

        let kind = if is_alias {
            ast::TypeStatementKind::Alias {
                value: self.parse_type_expr(),
            }
        } else {
            ast::TypeStatementKind::Nominal {
                definition: self.parse_type_def(),
            }
        };

        ast::Statement::Type {
            name,
            declared_kind,
            kind,
            range: self.range_from(start),
        }
    }

    fn parse_trait_statement(&mut self) -> ast::Statement {
        let start = self.pos;
        self.bump();

        if self.eat_punct(Punct::Tilde) {
            let name = self.expect_ident_node("trait alias name");
            self.expect_punct(Punct::Equals, "expected `=` in trait alias statement");
            let target =
                self.expect_ident_or_path_node("trait name or path in trait alias statement");

            return ast::Statement::TraitAlias {
                name,
                target,
                range: self.range_from(start),
            };
        }

        let name = self.expect_ident_node("trait name");
        let params = self.parse_type_params_opt();
        self.expect_punct(Punct::Equals, "expected `=` in trait statement");

        let mut items = Vec::new();
        while !self.at_eof() && !self.at_keyword(Keyword::End) {
            if let Some(item) = self.parse_trait_item_decl() {
                items.push(item);
            } else {
                let error_start = self.pos;
                self.error_current("expected trait item declaration");
                self.recover_trait_item();
                items.push(ast::TraitItem::Error(self.error_node(error_start)));
            }
        }

        self.expect_keyword(Keyword::End, "expected `end` to close trait statement");

        ast::Statement::Trait {
            name,
            params,
            items,
            range: self.range_from(start),
        }
    }

    fn parse_impl_statement(&mut self) -> ast::Statement {
        let start = self.pos;
        self.bump();

        let trait_ref = self.expect_ident_or_path_node("trait name or path in impl statement");

        let mut for_types = vec![self.parse_type_expr()];
        while self.eat_punct(Punct::Comma) {
            if self.at_punct(Punct::Equals) {
                break;
            }
            for_types.push(self.parse_type_expr());
        }

        self.expect_punct(Punct::Equals, "expected `=` in impl statement");

        let mut items = Vec::new();
        while !self.at_eof() && !self.at_keyword(Keyword::End) {
            if let Some(item) = self.parse_impl_item_def() {
                items.push(item);
            } else {
                let error_start = self.pos;
                self.error_current("expected impl item definition");
                self.recover_impl_item();
                items.push(ast::ImplItem::Error(self.error_node(error_start)));
            }
        }

        self.expect_keyword(Keyword::End, "expected `end` to close impl statement");

        ast::Statement::Impl {
            trait_ref,
            for_types,
            items,
            range: self.range_from(start),
        }
    }

    fn parse_wasm_statement(&mut self) -> ast::Statement {
        let start = self.pos;
        self.bump();
        self.expect_operator(Operator::FatArrow, "expected `=>` in wasm statement");

        let declarations = if self.at_punct(Punct::LParen)
            && (self.peek_punct(1, Punct::LParen) || self.peek_punct(1, Punct::RParen))
        {
            self.parse_wasm_declaration_list()
        } else {
            vec![self.parse_wasm_declaration()]
        };

        ast::Statement::Wasm {
            declarations,
            range: self.range_from(start),
        }
    }

    fn parse_wasm_declaration_list(&mut self) -> Vec<ast::SExpr> {
        self.expect_punct(Punct::LParen, "expected `(` to start wasm declaration list");

        let mut declarations = Vec::new();
        while !self.at_eof() && !self.at_punct(Punct::RParen) {
            declarations.push(self.parse_wasm_declaration());
        }

        self.expect_punct(Punct::RParen, "expected `)` to close wasm declaration list");
        declarations
    }

    fn parse_wasm_declaration(&mut self) -> ast::SExpr {
        let start = self.pos;
        if let Some(declaration) = self.parse_sexpr() {
            declaration
        } else {
            self.error_current("expected wasm declaration S-expression");
            if !self.at_eof() {
                self.bump();
            }
            ast::SExpr::Error(self.error_node(start))
        }
    }

    fn parse_trait_item_decl(&mut self) -> Option<ast::TraitItem> {
        let start = self.pos;

        match self.current_keyword() {
            Some(Keyword::Let) => {
                self.bump();
                let name = self.expect_ident_node("trait method name");
                self.expect_punct(Punct::Colon, "expected `:` in trait method declaration");
                let ty = self.parse_type_expr();
                Some(ast::TraitItem::Method {
                    name,
                    ty,
                    range: self.range_from(start),
                })
            }
            Some(Keyword::Type) => {
                self.bump();
                let name = self.expect_ident_node("trait associated type name");
                Some(ast::TraitItem::Type {
                    name,
                    range: self.range_from(start),
                })
            }
            _ => None,
        }
    }

    fn parse_impl_item_def(&mut self) -> Option<ast::ImplItem> {
        let start = self.pos;

        match self.current_keyword() {
            Some(Keyword::Let) => {
                self.bump();
                let name = self.expect_ident_node("impl method name");
                self.expect_punct(Punct::Equals, "expected `=` in impl method definition");
                let value = self.parse_expression(ExprBoundary::ImplItem);
                Some(ast::ImplItem::Method {
                    name,
                    value,
                    range: self.range_from(start),
                })
            }
            Some(Keyword::Type) => {
                self.bump();
                let name = self.expect_ident_node("impl associated type name");
                self.expect_punct(Punct::Equals, "expected `=` in impl type definition");
                let value = self.parse_type_expr();
                Some(ast::ImplItem::Type {
                    name,
                    value,
                    range: self.range_from(start),
                })
            }
            _ => None,
        }
    }

    fn parse_type_params_opt(&mut self) -> Vec<ast::Identifier> {
        if !self.eat_punct(Punct::Colon) {
            return Vec::new();
        }

        let mut params = Vec::new();
        if let Some(param) = self.expect_ident_node("type parameter") {
            params.push(param);
        } else {
            return params;
        }

        while let Some(param) = self.eat_ident_node() {
            params.push(param);
        }

        params
    }

    fn parse_kind_annotation_opt(&mut self) -> Option<ast::KindExpr> {
        if self.eat_operator(Operator::PathSep) {
            Some(self.parse_kind_expr())
        } else {
            None
        }
    }

    fn parse_kind_expr(&mut self) -> ast::KindExpr {
        let mut expr = if let Some(expr) = self.parse_kind_atom() {
            expr
        } else {
            let error_start = self.pos;
            self.error_current("expected kind expression");
            return ast::KindExpr::Error(self.error_node(error_start));
        };

        if self.eat_operator(Operator::Arrow) {
            let result = self.parse_kind_expr();
            let range = self.merge_ranges(expr.range(), result.range());
            expr = ast::KindExpr::Arrow {
                param: Box::new(expr),
                result: Box::new(result),
                range,
            };
        }

        expr
    }

    fn parse_kind_atom(&mut self) -> Option<ast::KindExpr> {
        if self.at_ident_token() {
            let identifier = self.eat_ident_node()?;
            return match self.identifier_text(&identifier).as_deref() {
                Some("Type") => Some(ast::KindExpr::Type {
                    range: identifier.range,
                }),
                Some("Row") => Some(ast::KindExpr::Row {
                    range: identifier.range,
                }),
                _ => {
                    self.error_current("expected kind expression");
                    Some(ast::KindExpr::Error(ast::ErrorNode::new(identifier.range)))
                }
            };
        }

        if self.at_punct(Punct::LParen) {
            let start = self.pos;
            self.bump();
            let inner = self.parse_kind_expr();
            self.expect_punct(Punct::RParen, "expected `)` to close kind expression");
            return Some(ast::KindExpr::Grouped {
                inner: Box::new(inner),
                range: self.range_from(start),
            });
        }

        None
    }

    fn type_binder_starts(&self) -> bool {
        self.at_ident_token() || self.at_punct(Punct::LParen)
    }

    fn parse_type_binders(&mut self, context: &str) -> Vec<ast::TypeBinder> {
        let mut params = Vec::new();

        if let Some(param) = self.parse_type_binder(context) {
            params.push(param);
        } else {
            self.error_current(format!("expected identifier for {context}"));
            return params;
        }

        while self.type_binder_starts() {
            if let Some(param) = self.parse_type_binder(context) {
                params.push(param);
            } else {
                break;
            }
        }

        params
    }

    fn parse_type_binder(&mut self, context: &str) -> Option<ast::TypeBinder> {
        if self.at_ident_token() {
            let start = self.pos;
            let name = self.expect_ident_node(context)?;
            return Some(ast::TypeBinder {
                name,
                kind: None,
                range: self.range_from(start),
            });
        }

        if !self.at_punct(Punct::LParen) {
            return None;
        }

        let start = self.pos;
        self.bump();
        let name = self.expect_ident_node(context)?;
        let kind = if self.eat_operator(Operator::PathSep) {
            Some(self.parse_kind_expr())
        } else {
            self.error_current("expected `::` in annotated type binder");
            None
        };
        self.expect_punct(Punct::RParen, "expected `)` to close annotated type binder");

        Some(ast::TypeBinder {
            name,
            kind,
            range: self.range_from(start),
        })
    }

    fn parse_type_def(&mut self) -> ast::TypeDefinition {
        if self.at_keyword(Keyword::Fn) {
            self.parse_type_def_lambda()
        } else if self.at_punct(Punct::Pipe) {
            self.parse_sum_type_def()
        } else {
            ast::TypeDefinition::Expr(self.parse_type_expr())
        }
    }

    fn parse_type_def_lambda(&mut self) -> ast::TypeDefinition {
        let start = self.pos;
        self.bump();
        let params = self.parse_type_binders("type lambda parameter");
        self.expect_operator(
            Operator::FatArrow,
            "expected `=>` in type definition lambda",
        );
        let body = self.parse_type_def();

        ast::TypeDefinition::Lambda {
            params,
            body: Box::new(body),
            range: self.range_from(start),
        }
    }

    fn parse_record_type_expr(&mut self) -> ast::TypeExpr {
        let start = self.pos;
        self.bump();

        let mut members = Vec::new();
        if !self.eat_punct(Punct::RBrace) {
            loop {
                if self.eat_operator(Operator::Spread) {
                    let member_start = self.pos.saturating_sub(1);
                    let ty = self.parse_type_expr();
                    members.push(ast::RecordTypeMember::Spread {
                        ty,
                        range: self.range_from(member_start),
                    });
                } else {
                    let member_start = self.pos;
                    let name = self.expect_ident_node("record field name");
                    self.expect_punct(Punct::Colon, "expected `:` in record field declaration");
                    let ty = self.parse_type_expr();
                    members.push(ast::RecordTypeMember::Field {
                        name,
                        ty,
                        range: self.range_from(member_start),
                    });
                }

                if !self.eat_punct(Punct::Comma) {
                    break;
                }

                if self.at_punct(Punct::RBrace) {
                    break;
                }
            }

            self.expect_punct(Punct::RBrace, "expected `}` to close record type");
        }

        ast::TypeExpr::Record {
            members,
            range: self.range_from(start),
        }
    }

    fn parse_sum_type_def(&mut self) -> ast::TypeDefinition {
        let start = self.pos;
        self.bump();

        let mut variants = vec![self.parse_variant()];
        while self.eat_punct(Punct::Pipe) {
            variants.push(self.parse_variant());
        }

        ast::TypeDefinition::Sum {
            variants,
            range: self.range_from(start),
        }
    }

    fn parse_variant(&mut self) -> ast::SumVariant {
        let start = self.pos;
        let name = self.expect_ident_node("sum type variant name");
        let argument = if self.type_atom_starts() {
            Some(self.parse_type_expr())
        } else {
            None
        };

        ast::SumVariant {
            name,
            argument,
            range: self.range_from(start),
        }
    }

    fn parse_type_expr(&mut self) -> ast::TypeExpr {
        if self.at_keyword(Keyword::For) {
            self.parse_type_forall_expr()
        } else if self.at_keyword(Keyword::Fn) {
            self.parse_type_expr_lambda()
        } else {
            self.parse_type_fn_expr()
        }
    }

    fn parse_type_forall_expr(&mut self) -> ast::TypeExpr {
        let start = self.pos;
        self.bump();

        let params = self.parse_type_binders("forall type parameter");

        self.expect_keyword(Keyword::In, "expected `in` in forall type expression");
        let body = self.parse_type_expr();

        let constraints = if self.eat_keyword(Keyword::Where) {
            self.parse_trait_constraint_list()
        } else {
            Vec::new()
        };

        ast::TypeExpr::Forall {
            params,
            body: Box::new(body),
            constraints,
            range: self.range_from(start),
        }
    }

    fn parse_type_expr_lambda(&mut self) -> ast::TypeExpr {
        let start = self.pos;
        self.bump();
        let params = self.parse_type_binders("type lambda parameter");
        self.expect_operator(Operator::FatArrow, "expected `=>` in type lambda");
        let body = self.parse_type_expr();

        ast::TypeExpr::Lambda {
            params,
            body: Box::new(body),
            range: self.range_from(start),
        }
    }

    fn parse_trait_constraint_list(&mut self) -> Vec<ast::TraitConstraint> {
        let mut constraints = Vec::new();

        if let Some(constraint) = self.parse_trait_constraint() {
            constraints.push(constraint);
        } else {
            self.error_current("expected trait constraint");
            return constraints;
        }

        while self.eat_punct(Punct::Comma) {
            if let Some(constraint) = self.parse_trait_constraint() {
                constraints.push(constraint);
            } else {
                break;
            }
        }

        constraints
    }

    fn parse_trait_constraint(&mut self) -> Option<ast::TraitConstraint> {
        let start = self.pos;
        let trait_ref = self.parse_ident_or_path()?;

        let mut args = Vec::new();
        while self.type_atom_starts() {
            if let Some(arg) = self.parse_type_atom() {
                args.push(arg);
            } else {
                break;
            }
        }

        Some(ast::TraitConstraint {
            trait_ref: Some(trait_ref),
            args,
            range: self.range_from(start),
        })
    }

    fn parse_type_fn_expr(&mut self) -> ast::TypeExpr {
        let mut expr = if let Some(expr) = self.parse_type_apply_expr() {
            expr
        } else {
            let error_start = self.pos;
            self.error_current("expected type expression");
            return ast::TypeExpr::Error(self.error_node(error_start));
        };

        if self.eat_operator(Operator::Arrow) {
            let result = self.parse_type_fn_expr();
            let range = self.merge_ranges(expr.range(), result.range());
            expr = ast::TypeExpr::Function {
                param: Box::new(expr),
                result: Box::new(result),
                range,
            };
        }

        expr
    }

    fn parse_type_apply_expr(&mut self) -> Option<ast::TypeExpr> {
        let mut expr = self.parse_type_atom()?;

        while self.type_atom_starts() {
            let argument = if let Some(argument) = self.parse_type_atom() {
                argument
            } else {
                break;
            };

            let range = self.merge_ranges(expr.range(), argument.range());
            expr = ast::TypeExpr::Apply {
                callee: Box::new(expr),
                argument: Box::new(argument),
                range,
            };
        }

        Some(expr)
    }

    fn type_atom_starts(&self) -> bool {
        self.at_ident_token()
            || self.at_keyword(Keyword::Root)
            || self.at_keyword(Keyword::Bundle)
            || self.at_punct(Punct::LParen)
            || self.at_punct(Punct::LBracket)
            || self.at_punct(Punct::LBrace)
    }

    fn parse_type_atom(&mut self) -> Option<ast::TypeExpr> {
        if let Some(name) = self.parse_ident_or_path() {
            return Some(ast::TypeExpr::Name(name));
        }

        if self.at_punct(Punct::LBrace) {
            return Some(self.parse_record_type_expr());
        }

        if self.at_punct(Punct::LParen) {
            let start = self.pos;
            self.bump();

            if self.eat_punct(Punct::RParen) {
                return Some(ast::TypeExpr::Unit {
                    range: self.range_from(start),
                });
            }

            let first = self.parse_type_expr();
            if self.eat_punct(Punct::Comma) {
                let mut elements = vec![first];
                if !self.at_punct(Punct::RParen) {
                    loop {
                        elements.push(self.parse_type_expr());
                        if !self.eat_punct(Punct::Comma) {
                            break;
                        }
                        if self.at_punct(Punct::RParen) {
                            break;
                        }
                    }
                }

                self.expect_punct(Punct::RParen, "expected `)` to close type expression");
                return Some(ast::TypeExpr::Tuple {
                    elements,
                    range: self.range_from(start),
                });
            }

            self.expect_punct(Punct::RParen, "expected `)` to close type expression");
            return Some(ast::TypeExpr::Grouped {
                inner: Box::new(first),
                range: self.range_from(start),
            });
        }

        if self.at_punct(Punct::LBracket) {
            let start = self.pos;
            self.bump();
            self.expect_punct(Punct::RBracket, "expected `]` to close array type");
            return Some(ast::TypeExpr::Array {
                range: self.range_from(start),
            });
        }

        None
    }

    fn parse_pattern(&mut self) -> Option<ast::Pattern> {
        let checkpoint = self.pos;
        if let Some(constructor) = self.parse_ident_or_path()
            && let Some(argument) = self.parse_pattern_arg()
        {
            let range = self.merge_ranges(constructor.range(), argument.range());
            return Some(ast::Pattern::Constructor {
                constructor,
                argument: Box::new(argument),
                range,
            });
        }
        self.pos = checkpoint;

        self.parse_annot_pattern()
    }

    fn parse_pattern_arg(&mut self) -> Option<ast::Pattern> {
        self.parse_annot_pattern()
    }

    fn parse_annot_pattern(&mut self) -> Option<ast::Pattern> {
        let start = self.pos;
        let pattern = self.parse_pattern_atom()?;

        if self.eat_punct(Punct::Colon) {
            let ty = self.parse_type_expr();
            return Some(ast::Pattern::Annotated {
                pattern: Box::new(pattern),
                ty,
                range: self.range_from(start),
            });
        }

        Some(pattern)
    }

    fn parse_pattern_atom(&mut self) -> Option<ast::Pattern> {
        if let Some(literal) = self.parse_literal() {
            return Some(ast::Pattern::Literal(literal));
        }

        if let Some(name) = self.parse_ident_or_path() {
            return Some(ast::Pattern::Name(name));
        }

        if self.at_punct(Punct::LParen) {
            let start = self.pos;
            self.bump();

            if self.eat_punct(Punct::RParen) {
                self.error_current("unit `()` is not a valid pattern");
                return Some(ast::Pattern::Error(self.error_node(start)));
            }

            let first = self
                .parse_pattern()
                .unwrap_or_else(|| ast::Pattern::Error(self.error_node(self.pos)));

            if self.eat_punct(Punct::Comma) {
                let mut elements = vec![first];
                if !self.at_punct(Punct::RParen) {
                    loop {
                        let next = self
                            .parse_pattern()
                            .unwrap_or_else(|| ast::Pattern::Error(self.error_node(self.pos)));
                        elements.push(next);

                        if !self.eat_punct(Punct::Comma) {
                            break;
                        }
                        if self.at_punct(Punct::RParen) {
                            break;
                        }
                    }
                }

                self.expect_punct(Punct::RParen, "expected `)` to close pattern");
                return Some(ast::Pattern::Tuple {
                    elements,
                    range: self.range_from(start),
                });
            }

            self.expect_punct(Punct::RParen, "expected `)` to close pattern");
            return Some(ast::Pattern::Grouped {
                inner: Box::new(first),
                range: self.range_from(start),
            });
        }

        if self.at_punct(Punct::LBracket) {
            let start = self.pos;
            self.bump();

            let mut elements = Vec::new();
            if !self.eat_punct(Punct::RBracket) {
                loop {
                    if self.eat_operator(Operator::Spread) {
                        let spread_start = self.pos.saturating_sub(1);
                        let binding = self.eat_ident_node();
                        elements.push(ast::ArrayPatternElement::Rest {
                            binding,
                            range: self.range_from(spread_start),
                        });
                    } else {
                        let item = self
                            .parse_pattern()
                            .unwrap_or_else(|| ast::Pattern::Error(self.error_node(self.pos)));
                        elements.push(ast::ArrayPatternElement::Item(item));
                    }

                    if !self.eat_punct(Punct::Comma) {
                        break;
                    }

                    if self.at_punct(Punct::RBracket) {
                        break;
                    }
                }

                self.expect_punct(Punct::RBracket, "expected `]` to close array pattern");
            }

            return Some(ast::Pattern::Array {
                elements,
                range: self.range_from(start),
            });
        }

        if self.at_punct(Punct::LBrace) {
            let start = self.pos;
            self.bump();

            let mut fields = Vec::new();
            let mut open = false;
            if !self.eat_punct(Punct::RBrace) {
                loop {
                    if self.eat_operator(Operator::Spread) {
                        open = true;
                        if !self.at_punct(Punct::RBrace) {
                            self.error_current(
                                "`..` must be the final element in a record pattern",
                            );
                        }
                        break;
                    }

                    let field_start = self.pos;
                    let name = self.expect_ident_node("record pattern field");
                    let value = if self.eat_punct(Punct::Equals) {
                        Some(
                            self.parse_pattern()
                                .unwrap_or_else(|| ast::Pattern::Error(self.error_node(self.pos))),
                        )
                    } else {
                        None
                    };

                    fields.push(ast::RecordPatternField {
                        name,
                        value,
                        range: self.range_from(field_start),
                    });

                    if !self.eat_punct(Punct::Comma) {
                        break;
                    }

                    if self.at_punct(Punct::RBrace) {
                        break;
                    }
                }

                self.expect_punct(Punct::RBrace, "expected `}` to close record pattern");
            }

            return Some(ast::Pattern::Record {
                fields,
                open,
                range: self.range_from(start),
            });
        }

        None
    }

    fn parse_literal(&mut self) -> Option<ast::Literal> {
        let kind = match self.current_kind() {
            TokenKind::IntegerLiteral => Some(ast::LiteralKind::Integer),
            TokenKind::NaturalLiteral => Some(ast::LiteralKind::Natural),
            TokenKind::RealLiteral => Some(ast::LiteralKind::Real),
            TokenKind::StringLiteral => Some(ast::LiteralKind::String),
            TokenKind::GlyphLiteral => Some(ast::LiteralKind::Glyph),
            TokenKind::FormatStringLiteral => Some(ast::LiteralKind::FormatString),
            _ => None,
        };

        if let Some(kind) = kind {
            let range = self.current_range();
            self.bump();
            return Some(ast::Literal { kind, range });
        }

        if self.at_keyword(Keyword::True) {
            let range = self.current_range();
            self.bump();
            return Some(ast::Literal {
                kind: ast::LiteralKind::BoolTrue,
                range,
            });
        }

        if self.at_keyword(Keyword::False) {
            let range = self.current_range();
            self.bump();
            return Some(ast::Literal {
                kind: ast::LiteralKind::BoolFalse,
                range,
            });
        }

        None
    }

    fn parse_expression(&mut self, boundary: ExprBoundary) -> ast::Expr {
        self.parse_expr_with_stop(ExprStop::for_boundary(boundary))
            .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)))
    }

    fn parse_expr_with_stop(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        match self.current_keyword() {
            Some(Keyword::Let) => return self.parse_let_expr(stop),
            Some(Keyword::Use) => return self.parse_use_expr(stop),
            Some(Keyword::Fn) => return self.parse_fn_expr(stop),
            Some(Keyword::If) => return self.parse_if_expr(stop),
            Some(Keyword::Match) => return self.parse_match_expr(stop),
            _ => {}
        }

        if self.should_stop_expr(stop) {
            self.error_current("expected expression");
            return None;
        }

        self.parse_seq_expr(stop)
    }

    fn parse_let_expr(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        let start = self.pos;
        self.bump();

        let pattern = if let Some(pattern) = self.parse_pattern() {
            pattern
        } else {
            let error_start = self.pos;
            self.error_current("expected pattern in let expression");
            self.recover_until(|parser| {
                parser.at_eof() || parser.at_punct(Punct::Equals) || parser.at_keyword(Keyword::In)
            });
            ast::Pattern::Error(self.error_node(error_start))
        };

        self.expect_punct(Punct::Equals, "expected `=` in let expression");
        let value = self
            .parse_expr_with_stop(stop.with_in())
            .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));
        self.expect_keyword(Keyword::In, "expected `in` in let expression");
        let body = self
            .parse_expr_with_stop(stop)
            .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));

        Some(ast::Expr::Let {
            pattern,
            value: Box::new(value),
            body: Box::new(body),
            range: self.range_from(start),
        })
    }

    fn parse_use_expr(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        let start = self.pos;
        self.bump();

        let target = self.expect_use_target_node("use expression");
        let alias = if self.eat_keyword(Keyword::As) {
            self.expect_ident_node("use expression alias name")
        } else {
            None
        };

        self.expect_keyword(Keyword::In, "expected `in` in use expression");
        let body = self
            .parse_expr_with_stop(stop)
            .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));

        Some(ast::Expr::Use {
            target,
            alias,
            body: Box::new(body),
            range: self.range_from(start),
        })
    }

    fn parse_fn_expr(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        let start = self.pos;
        self.bump();

        if self.at_punct(Punct::Pipe) {
            let mut arms = Vec::new();
            while self.at_punct(Punct::Pipe) {
                arms.push(self.parse_match_arm(stop));
            }

            if arms.is_empty() {
                self.error_current("expected match arm in function expression");
                return Some(ast::Expr::Error(self.error_node(start)));
            }

            return Some(ast::Expr::Function {
                params: Vec::new(),
                body: ast::FunctionBody::MatchArms(arms),
                range: self.range_from(start),
            });
        }

        let mut params = Vec::new();
        while !self.at_eof() && !self.at_operator(Operator::FatArrow) {
            if let Some(parameter) = self.parse_parameter() {
                params.push(parameter);
            } else {
                let error_start = self.pos;
                self.error_current("expected parameter or `=>` in function expression");
                self.recover_until(|parser| {
                    parser.at_eof()
                        || parser.at_operator(Operator::FatArrow)
                        || parser.should_stop_expr(stop)
                });
                params.push(ast::Parameter::Error(self.error_node(error_start)));
                break;
            }
        }

        self.expect_operator(Operator::FatArrow, "expected `=>` in function expression");
        let body = self
            .parse_expr_with_stop(stop)
            .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));

        Some(ast::Expr::Function {
            params,
            body: ast::FunctionBody::Expr(Box::new(body)),
            range: self.range_from(start),
        })
    }

    fn parse_parameter(&mut self) -> Option<ast::Parameter> {
        if let Some(identifier) = self.eat_ident_node() {
            return Some(ast::Parameter::Named(identifier));
        }

        if !self.at_punct(Punct::LParen) {
            return None;
        }

        let start = self.pos;
        self.bump();

        let name = self.expect_ident_node("typed parameter name");
        self.expect_punct(Punct::Colon, "expected `:` in typed parameter");
        let ty = self.parse_type_expr();
        self.expect_punct(Punct::RParen, "expected `)` to close typed parameter");

        Some(ast::Parameter::Typed {
            name,
            ty,
            range: self.range_from(start),
        })
    }

    fn parse_if_expr(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        let start = self.pos;
        self.bump();

        let condition = self
            .parse_expr_with_stop(stop.with_then())
            .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));
        self.expect_keyword(Keyword::Then, "expected `then` in if expression");
        let then_branch = self
            .parse_expr_with_stop(stop.with_else())
            .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));
        self.expect_keyword(Keyword::Else, "expected `else` in if expression");
        let else_branch = self
            .parse_expr_with_stop(stop)
            .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));

        Some(ast::Expr::If {
            condition: Box::new(condition),
            then_branch: Box::new(then_branch),
            else_branch: Box::new(else_branch),
            range: self.range_from(start),
        })
    }

    fn parse_match_expr(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        let start = self.pos;
        self.bump();

        let scrutinee = self
            .parse_expr_with_stop(stop.with_with())
            .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));
        self.expect_keyword(Keyword::With, "expected `with` in match expression");

        let mut arms = Vec::new();
        if !self.at_punct(Punct::Pipe) {
            self.error_current("expected at least one match arm");
            return Some(ast::Expr::Error(self.error_node(start)));
        }

        while self.at_punct(Punct::Pipe) {
            arms.push(self.parse_match_arm(stop));
        }

        Some(ast::Expr::Match {
            scrutinee: Box::new(scrutinee),
            arms,
            range: self.range_from(start),
        })
    }

    fn parse_match_arm(&mut self, stop: ExprStop) -> ast::MatchArm {
        let start = self.pos;
        self.expect_punct(Punct::Pipe, "expected `|` to start match arm");
        let pattern = if let Some(pattern) = self.parse_pattern() {
            pattern
        } else {
            let error_start = self.pos;
            self.error_current("expected pattern in match arm");
            ast::Pattern::Error(self.error_node(error_start))
        };
        self.expect_operator(Operator::FatArrow, "expected `=>` in match arm");
        let body = self
            .parse_expr_with_stop(stop.with_pipe())
            .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));

        ast::MatchArm {
            pattern,
            body,
            range: self.range_from(start),
        }
    }

    fn parse_seq_expr(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        let mut expr = self.parse_and_expr(stop)?;

        if self.eat_operator(Operator::Semicolon) {
            let rhs = self
                .parse_seq_expr(stop)
                .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));
            expr = self.make_binary_expr(ast::BinaryOperator::Sequence, expr, rhs);
        }

        Some(expr)
    }

    fn parse_and_expr(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        let mut expr = self.parse_cmp_expr(stop)?;

        while !self.should_stop_expr(stop) && self.eat_keyword(Keyword::And) {
            let rhs = self
                .parse_cmp_expr(stop)
                .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));
            expr = self.make_binary_expr(ast::BinaryOperator::And, expr, rhs);
        }

        Some(expr)
    }

    fn parse_cmp_expr(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        let mut expr = self.parse_or_pipe_expr(stop)?;

        if !self.should_stop_expr(stop)
            && let Some(op) = self.eat_cmp_operator()
        {
            let rhs = self
                .parse_or_pipe_expr(stop)
                .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));
            expr = self.make_binary_expr(op, expr, rhs);

            if self.at_cmp_operator() {
                self.error_current("comparison operators are non-associative");
                while let Some(chain_op) = self.eat_cmp_operator() {
                    let rhs = self
                        .parse_or_pipe_expr(stop)
                        .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));
                    expr = self.make_binary_expr(chain_op, expr, rhs);
                }
            }
        }

        Some(expr)
    }

    fn parse_or_pipe_expr(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        let mut expr = self.parse_xor_expr(stop)?;

        while !self.should_stop_expr(stop) {
            let op = if self.eat_keyword(Keyword::Or) {
                Some(ast::BinaryOperator::Or)
            } else if self.eat_operator(Operator::PipeRight) {
                Some(ast::BinaryOperator::PipeRight)
            } else if self.eat_operator(Operator::PlusPipe) {
                Some(ast::BinaryOperator::PlusPipe)
            } else if self.eat_operator(Operator::StarPipe) {
                Some(ast::BinaryOperator::StarPipe)
            } else {
                None
            };

            let Some(op) = op else {
                break;
            };

            let rhs = self
                .parse_xor_expr(stop)
                .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));
            expr = self.make_binary_expr(op, expr, rhs);
        }

        Some(expr)
    }

    fn parse_xor_expr(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        let mut expr = self.parse_shift_expr(stop)?;

        while !self.should_stop_expr(stop) && self.eat_keyword(Keyword::Xor) {
            let rhs = self
                .parse_shift_expr(stop)
                .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));
            expr = self.make_binary_expr(ast::BinaryOperator::Xor, expr, rhs);
        }

        Some(expr)
    }

    fn parse_shift_expr(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        let mut expr = self.parse_add_expr(stop)?;

        while !self.should_stop_expr(stop) {
            let op = if self.eat_operator(Operator::ComposeRight) {
                Some(ast::BinaryOperator::ShiftRight)
            } else if self.eat_operator(Operator::ComposeLeft) {
                Some(ast::BinaryOperator::ShiftLeft)
            } else {
                None
            };

            let Some(op) = op else {
                break;
            };

            let rhs = self
                .parse_add_expr(stop)
                .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));
            expr = self.make_binary_expr(op, expr, rhs);
        }

        Some(expr)
    }

    fn parse_add_expr(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        let mut expr = self.parse_mul_expr(stop)?;

        while !self.should_stop_expr(stop) {
            let op = if self.eat_operator(Operator::Plus) {
                Some(ast::BinaryOperator::Add)
            } else if self.eat_operator(Operator::Minus) {
                Some(ast::BinaryOperator::Subtract)
            } else {
                None
            };

            let Some(op) = op else {
                break;
            };

            let rhs = self
                .parse_mul_expr(stop)
                .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));
            expr = self.make_binary_expr(op, expr, rhs);
        }

        Some(expr)
    }

    fn parse_mul_expr(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        let mut expr = self.parse_unary_expr(stop)?;

        while !self.should_stop_expr(stop) {
            let op = if self.eat_operator(Operator::Star) {
                Some(ast::BinaryOperator::Multiply)
            } else if self.eat_operator(Operator::Slash) {
                Some(ast::BinaryOperator::Divide)
            } else if self.eat_keyword(Keyword::Mod) {
                Some(ast::BinaryOperator::Modulo)
            } else {
                None
            };

            let Some(op) = op else {
                break;
            };

            let rhs = self
                .parse_unary_expr(stop)
                .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));
            expr = self.make_binary_expr(op, expr, rhs);
        }

        Some(expr)
    }

    fn parse_unary_expr(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        if self.at_keyword(Keyword::Not) {
            let start = self.pos;
            self.bump();
            let expr = self
                .parse_unary_expr(stop)
                .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));
            return Some(ast::Expr::Unary {
                op: ast::UnaryOperator::Not,
                expr: Box::new(expr),
                range: self.range_from(start),
            });
        }

        if self.at_operator(Operator::Minus) {
            let start = self.pos;
            self.bump();
            let expr = self
                .parse_unary_expr(stop)
                .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));
            return Some(ast::Expr::Unary {
                op: ast::UnaryOperator::Negate,
                expr: Box::new(expr),
                range: self.range_from(start),
            });
        }

        self.parse_apply_expr(stop)
    }

    fn parse_apply_expr(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        let mut expr = self.parse_postfix_expr(stop)?;

        while !self.should_stop_expr(stop) && self.atom_starts() {
            let argument = self
                .parse_postfix_expr(stop)
                .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));
            let range = self.merge_ranges(expr.range(), argument.range());
            expr = ast::Expr::Apply {
                callee: Box::new(expr),
                argument: Box::new(argument),
                range,
            };
        }

        Some(expr)
    }

    fn parse_postfix_expr(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        let mut expr = self.parse_atom_expr(stop)?;

        while !self.should_stop_expr(stop) && self.eat_punct(Punct::Dot) {
            let field_start = self.pos.saturating_sub(1);
            let field = self.expect_ident_node("field access name");
            let tail_range = field
                .as_ref()
                .map(|identifier| identifier.range)
                .unwrap_or_else(|| self.range_from(field_start));
            let range = self.merge_ranges(expr.range(), tail_range);

            expr = ast::Expr::FieldAccess {
                expr: Box::new(expr),
                field,
                range,
            };
        }

        Some(expr)
    }

    fn parse_atom_expr(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        if let Some(literal) = self.parse_literal() {
            return Some(ast::Expr::Literal(literal));
        }

        if let Some(name) = self.parse_ident_or_path() {
            return Some(ast::Expr::Name(name));
        }

        if self.is_inline_wasm_expr_start() {
            return Some(self.parse_inline_wasm_expr());
        }

        if self.at_punct(Punct::LParen) {
            return self.parse_parenthesized_or_tuple_expr(stop);
        }

        if self.at_punct(Punct::LBracket) {
            return self.parse_array_expr(stop);
        }

        if self.at_punct(Punct::LBrace) {
            return self.parse_record_expr(stop);
        }

        self.error_current("expected expression atom");
        None
    }

    fn parse_parenthesized_or_tuple_expr(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        let start = self.pos;
        self.bump();

        if self.eat_punct(Punct::RParen) {
            return Some(ast::Expr::Unit {
                range: self.range_from(start),
            });
        }

        let inner_stop = stop.with_comma().with_rparen();
        let first = self
            .parse_expr_with_stop(inner_stop)
            .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));

        if self.eat_punct(Punct::Comma) {
            let mut elements = vec![first];
            while !self.at_eof() && !self.at_punct(Punct::RParen) {
                let element = self
                    .parse_expr_with_stop(inner_stop)
                    .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));
                elements.push(element);
                if !self.eat_punct(Punct::Comma) {
                    break;
                }
            }

            self.expect_punct(
                Punct::RParen,
                "expected `)` to close parenthesized expression",
            );
            return Some(ast::Expr::Tuple {
                elements,
                range: self.range_from(start),
            });
        }

        self.expect_punct(
            Punct::RParen,
            "expected `)` to close parenthesized expression",
        );
        Some(ast::Expr::Grouped {
            inner: Box::new(first),
            range: self.range_from(start),
        })
    }

    fn parse_array_expr(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        let start = self.pos;
        self.bump();

        let mut elements = Vec::new();
        if !self.eat_punct(Punct::RBracket) {
            let elem_stop = stop.with_comma().with_rbracket();
            loop {
                elements.push(self.parse_array_elem(elem_stop));

                if !self.eat_punct(Punct::Comma) {
                    break;
                }

                if self.at_punct(Punct::RBracket) {
                    break;
                }
            }

            self.expect_punct(Punct::RBracket, "expected `]` to close array expression");
        }

        Some(ast::Expr::Array {
            elements,
            range: self.range_from(start),
        })
    }

    fn parse_array_elem(&mut self, stop: ExprStop) -> ast::ArrayElement {
        if self.eat_operator(Operator::Spread) {
            let start = self.pos.saturating_sub(1);
            let expr = self
                .parse_expr_with_stop(stop)
                .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));
            return ast::ArrayElement::Spread {
                expr,
                range: self.range_from(start),
            };
        }

        let expr = self
            .parse_expr_with_stop(stop)
            .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));
        ast::ArrayElement::Item(expr)
    }

    fn parse_record_expr(&mut self, stop: ExprStop) -> Option<ast::Expr> {
        let start = self.pos;
        self.bump();

        let mut fields = Vec::new();
        if !self.eat_punct(Punct::RBrace) {
            let field_stop = stop.with_comma().with_rbrace();
            loop {
                let field_start = self.pos;
                let name = self.expect_ident_node("record field name");
                let separator = if self.eat_punct(Punct::Equals) {
                    ast::RecordFieldSeparator::Equals
                } else if self.eat_punct(Punct::Colon) {
                    ast::RecordFieldSeparator::Colon
                } else {
                    self.error_current("expected `=` or `:` in record field");
                    ast::RecordFieldSeparator::Missing
                };
                let value = self
                    .parse_expr_with_stop(field_stop)
                    .unwrap_or_else(|| ast::Expr::Error(self.error_node(self.pos)));

                fields.push(ast::RecordField {
                    name,
                    separator,
                    value,
                    range: self.range_from(field_start),
                });

                if !self.eat_punct(Punct::Comma) {
                    break;
                }

                if self.at_punct(Punct::RBrace) {
                    break;
                }
            }

            self.expect_punct(Punct::RBrace, "expected `}` to close record expression");
        }

        Some(ast::Expr::Record {
            fields,
            range: self.range_from(start),
        })
    }

    fn parse_inline_wasm_expr(&mut self) -> ast::Expr {
        let start = self.pos;

        self.expect_punct(
            Punct::LParen,
            "expected `(` to start inline wasm expression",
        );
        self.expect_keyword(Keyword::Wasm, "expected `wasm` in inline wasm expression");
        self.expect_punct(Punct::Colon, "expected `:` in inline wasm expression");
        let result_type = self.parse_type_expr();
        self.expect_punct(Punct::RParen, "expected `)` after inline wasm signature");
        self.expect_operator(
            Operator::FatArrow,
            "expected `=>` in inline wasm expression",
        );

        let body = if let Some(sexpr) = self.parse_sexpr() {
            Some(sexpr)
        } else {
            self.error_current("expected S-expression body in inline wasm expression");
            None
        };

        ast::Expr::InlineWasm {
            result_type,
            body,
            range: self.range_from(start),
        }
    }

    fn atom_starts(&self) -> bool {
        self.literal_starts()
            || self.at_ident_token()
            || self.at_keyword(Keyword::Root)
            || self.at_keyword(Keyword::Bundle)
            || self.at_punct(Punct::LParen)
            || self.at_punct(Punct::LBracket)
            || self.at_punct(Punct::LBrace)
    }

    fn literal_starts(&self) -> bool {
        matches!(
            self.current_kind(),
            TokenKind::IntegerLiteral
                | TokenKind::NaturalLiteral
                | TokenKind::RealLiteral
                | TokenKind::StringLiteral
                | TokenKind::GlyphLiteral
                | TokenKind::FormatStringLiteral
        ) || matches!(
            self.current_keyword(),
            Some(Keyword::True) | Some(Keyword::False)
        )
    }

    fn is_inline_wasm_expr_start(&self) -> bool {
        self.at_punct(Punct::LParen)
            && matches!(self.peek_kind(1), Some(TokenKind::Keyword(Keyword::Wasm)))
            && self.peek_punct(2, Punct::Colon)
    }

    fn make_binary_expr(
        &self,
        op: ast::BinaryOperator,
        lhs: ast::Expr,
        rhs: ast::Expr,
    ) -> ast::Expr {
        let range = self.merge_ranges(lhs.range(), rhs.range());
        ast::Expr::Binary {
            op,
            lhs: Box::new(lhs),
            rhs: Box::new(rhs),
            range,
        }
    }

    fn eat_cmp_operator(&mut self) -> Option<ast::BinaryOperator> {
        let op = match self.current_kind() {
            TokenKind::Operator(Operator::EqualEqual) => ast::BinaryOperator::Equal,
            TokenKind::Operator(Operator::BangEqual) => ast::BinaryOperator::NotEqual,
            TokenKind::Operator(Operator::Less) => ast::BinaryOperator::Less,
            TokenKind::Operator(Operator::LessEqual) => ast::BinaryOperator::LessEqual,
            TokenKind::Operator(Operator::Greater) => ast::BinaryOperator::Greater,
            TokenKind::Operator(Operator::GreaterEqual) => ast::BinaryOperator::GreaterEqual,
            _ => return None,
        };
        self.bump();
        Some(op)
    }

    fn at_cmp_operator(&self) -> bool {
        matches!(
            self.current_kind(),
            TokenKind::Operator(Operator::EqualEqual)
                | TokenKind::Operator(Operator::BangEqual)
                | TokenKind::Operator(Operator::Less)
                | TokenKind::Operator(Operator::LessEqual)
                | TokenKind::Operator(Operator::Greater)
                | TokenKind::Operator(Operator::GreaterEqual)
        )
    }

    fn should_stop_expr(&self, stop: ExprStop) -> bool {
        self.at_eof()
            || (stop.stop_on_in && self.at_keyword(Keyword::In))
            || (stop.stop_on_then && self.at_keyword(Keyword::Then))
            || (stop.stop_on_else && self.at_keyword(Keyword::Else))
            || (stop.stop_on_with && self.at_keyword(Keyword::With))
            || (stop.stop_on_pipe && self.at_punct(Punct::Pipe))
            || (stop.stop_on_comma && self.at_punct(Punct::Comma))
            || (stop.stop_on_rparen && self.at_punct(Punct::RParen))
            || (stop.stop_on_rbracket && self.at_punct(Punct::RBracket))
            || (stop.stop_on_rbrace && self.at_punct(Punct::RBrace))
            || self.reached_expr_boundary(stop.boundary)
    }

    fn reached_expr_boundary(&self, boundary: ExprBoundary) -> bool {
        match boundary {
            ExprBoundary::Statement => {
                if self.at_keyword(Keyword::Bundle)
                    && matches!(
                        self.peek_kind(1),
                        Some(TokenKind::Operator(Operator::PathSep))
                    )
                {
                    false
                } else {
                    self.current_keyword()
                        .is_some_and(is_statement_boundary_keyword)
                }
            }
            ExprBoundary::ImplItem => self.current_keyword().is_some_and(|keyword| {
                matches!(keyword, Keyword::Let | Keyword::Type | Keyword::End)
            }),
        }
    }

    fn parse_sexpr(&mut self) -> Option<ast::SExpr> {
        if !self.at_punct(Punct::LParen) {
            return None;
        }

        let start = self.pos;
        self.bump();

        let mut items = Vec::new();
        while !self.at_eof() && !self.at_punct(Punct::RParen) {
            if let Some(item) = self.parse_sexpr_item() {
                items.push(item);
            } else {
                let error_start = self.pos;
                self.error_current("expected S-expression item");
                self.bump();
                items.push(ast::SExpr::Error(self.error_node(error_start)));
            }
        }

        self.expect_punct(Punct::RParen, "expected `)` to close S-expression");
        Some(ast::SExpr::List {
            items,
            range: self.range_from(start),
        })
    }

    fn parse_sexpr_item(&mut self) -> Option<ast::SExpr> {
        if self.at_punct(Punct::LParen) {
            return self.parse_sexpr();
        }

        if let Some(path) = self.parse_sexpr_path() {
            return Some(path);
        }

        if let Some(identifier) = self.parse_sexpr_ident() {
            return Some(identifier);
        }

        let kind = match self.current_kind() {
            TokenKind::StringLiteral => Some(ast::SExprAtomKind::String),
            TokenKind::IntegerLiteral => Some(ast::SExprAtomKind::Integer),
            TokenKind::NaturalLiteral => Some(ast::SExprAtomKind::Natural),
            TokenKind::RealLiteral => Some(ast::SExprAtomKind::Real),
            _ => None,
        };

        if let Some(kind) = kind {
            let range = self.current_range();
            self.bump();
            return Some(ast::SExpr::Atom { kind, range });
        }

        if self.at_keyword(Keyword::True) {
            let range = self.current_range();
            self.bump();
            return Some(ast::SExpr::Atom {
                kind: ast::SExprAtomKind::BoolTrue,
                range,
            });
        }

        if self.at_keyword(Keyword::False) {
            let range = self.current_range();
            self.bump();
            return Some(ast::SExpr::Atom {
                kind: ast::SExprAtomKind::BoolFalse,
                range,
            });
        }

        None
    }

    fn sexpr_list_items<'s>(&self, sexpr: &'s ast::SExpr) -> Option<&'s [ast::SExpr]> {
        match sexpr {
            ast::SExpr::List { items, .. } => Some(items),
            _ => None,
        }
    }

    fn parse_sexpr_path(&mut self) -> Option<ast::SExpr> {
        let checkpoint = self.pos;
        let start = self.pos;

        if !self.eat_punct(Punct::Dollar) {
            return None;
        }

        if !self.eat_sexpr_ident_segment() {
            self.pos = checkpoint;
            return None;
        }

        if !self.eat_operator(Operator::PathSep) {
            self.pos = checkpoint;
            return None;
        }

        if !self.eat_sexpr_ident_segment() {
            self.pos = checkpoint;
            return None;
        }

        while self.eat_operator(Operator::PathSep) {
            if !self.eat_sexpr_ident_segment() {
                self.pos = checkpoint;
                return None;
            }
        }

        Some(ast::SExpr::Atom {
            kind: ast::SExprAtomKind::Path,
            range: self.range_from(start),
        })
    }

    fn parse_sexpr_ident(&mut self) -> Option<ast::SExpr> {
        let checkpoint = self.pos;
        let start = self.pos;

        self.eat_punct(Punct::Dollar);

        if !self.eat_sexpr_ident_segment() {
            self.pos = checkpoint;
            return None;
        }

        while self.eat_punct(Punct::Dot) {
            if !self.eat_sexpr_ident_segment() {
                self.pos = checkpoint;
                return None;
            }
        }

        Some(ast::SExpr::Atom {
            kind: ast::SExprAtomKind::Ident,
            range: self.range_from(start),
        })
    }

    fn eat_sexpr_ident_segment(&mut self) -> bool {
        if self.eat_ident_node().is_some() {
            return true;
        }

        if matches!(
            self.current_keyword(),
            Some(keyword) if !matches!(keyword, Keyword::True | Keyword::False)
        ) {
            self.bump();
            return true;
        }

        false
    }

    fn expect_ident_or_path_node(&mut self, context: &str) -> Option<ast::NameRef> {
        if let Some(name) = self.parse_ident_or_path() {
            Some(name)
        } else {
            self.error_current(format!("expected {context}"));
            None
        }
    }

    fn expect_use_target_node(&mut self, context: &str) -> Option<ast::NameRef> {
        if let Some(target) = self.parse_use_target() {
            Some(target)
        } else {
            self.error_current(format!(
                "expected identifier, path, or `bundle` in {context}"
            ));
            None
        }
    }

    fn parse_use_target(&mut self) -> Option<ast::NameRef> {
        if self.at_keyword(Keyword::Bundle)
            && !matches!(
                self.peek_kind(1),
                Some(TokenKind::Operator(Operator::PathSep))
            )
        {
            let start = self.pos;
            self.bump();
            return Some(ast::NameRef::Path(ast::Path {
                root: ast::PathRoot::Bundle,
                segments: Vec::new(),
                range: self.range_from(start),
            }));
        }

        self.parse_ident_or_path()
    }

    fn parse_ident_or_path(&mut self) -> Option<ast::NameRef> {
        let checkpoint = self.pos;
        if let Some(path) = self.eat_path() {
            return Some(ast::NameRef::Path(path));
        }
        self.pos = checkpoint;

        self.eat_ident_node().map(ast::NameRef::Identifier)
    }

    fn eat_path(&mut self) -> Option<ast::Path> {
        let checkpoint = self.pos;
        let start = self.pos;

        let root = if self.eat_keyword(Keyword::Root) {
            ast::PathRoot::Root
        } else if self.eat_keyword(Keyword::Bundle) {
            ast::PathRoot::Bundle
        } else {
            ast::PathRoot::Relative
        };

        let mut segments = Vec::new();
        if root == ast::PathRoot::Relative {
            let first = self.eat_ident_node()?;
            segments.push(first);
        }

        if !self.eat_operator(Operator::PathSep) {
            self.pos = checkpoint;
            return None;
        }

        if let Some(segment) = self.eat_ident_node() {
            segments.push(segment);
        } else {
            self.pos = checkpoint;
            return None;
        }

        while self.eat_operator(Operator::PathSep) {
            if let Some(segment) = self.eat_ident_node() {
                segments.push(segment);
            } else {
                self.pos = checkpoint;
                return None;
            }
        }

        Some(ast::Path {
            root,
            segments,
            range: self.range_from(start),
        })
    }

    fn recover_statement(&mut self) {
        let start = self.pos;
        self.recover_until(|parser| {
            parser.at_eof() || parser.is_statement_start() || parser.at_keyword(Keyword::End)
        });
        if self.pos == start && !self.at_eof() {
            self.bump();
        }
    }

    fn recover_trait_item(&mut self) {
        let start = self.pos;
        self.recover_until(|parser| {
            parser.at_eof()
                || parser.at_keyword(Keyword::End)
                || parser.at_keyword(Keyword::Let)
                || parser.at_keyword(Keyword::Type)
        });
        if self.pos == start && !self.at_eof() {
            self.bump();
        }
    }

    fn recover_impl_item(&mut self) {
        let start = self.pos;
        self.recover_until(|parser| {
            parser.at_eof()
                || parser.at_keyword(Keyword::End)
                || parser.at_keyword(Keyword::Let)
                || parser.at_keyword(Keyword::Type)
        });
        if self.pos == start && !self.at_eof() {
            self.bump();
        }
    }

    fn recover_until(&mut self, mut stop: impl FnMut(&Self) -> bool) {
        while !stop(self) {
            self.bump();
        }
    }

    fn is_statement_start(&self) -> bool {
        self.current_keyword()
            .is_some_and(is_statement_start_keyword)
    }

    fn at_ident_token(&self) -> bool {
        matches!(
            self.current_kind(),
            TokenKind::Ident | TokenKind::BracketedIdent
        )
    }

    fn identifier_text(&self, identifier: &ast::Identifier) -> Option<String> {
        identifier.range.text(self.source.contents(self.db))
    }

    fn eat_ident_node(&mut self) -> Option<ast::Identifier> {
        let (kind, range) = match self.current_kind() {
            TokenKind::Ident => (ast::IdentifierKind::Bare, self.current_range()),
            TokenKind::BracketedIdent => (ast::IdentifierKind::Bracketed, self.current_range()),
            _ => return None,
        };
        self.bump();
        Some(ast::Identifier { kind, range })
    }

    fn expect_ident_node(&mut self, context: &str) -> Option<ast::Identifier> {
        if let Some(identifier) = self.eat_ident_node() {
            Some(identifier)
        } else {
            self.error_current(format!("expected identifier for {context}"));
            None
        }
    }

    fn eat_string_literal_node(&mut self) -> Option<ast::Literal> {
        if matches!(self.current_kind(), TokenKind::StringLiteral) {
            let range = self.current_range();
            self.bump();
            Some(ast::Literal {
                kind: ast::LiteralKind::String,
                range,
            })
        } else {
            None
        }
    }

    fn at_keyword(&self, keyword: Keyword) -> bool {
        matches!(self.current_kind(), TokenKind::Keyword(current) if current == keyword)
    }

    fn current_keyword(&self) -> Option<Keyword> {
        match self.current_kind() {
            TokenKind::Keyword(keyword) => Some(keyword),
            _ => None,
        }
    }

    fn eat_keyword(&mut self, keyword: Keyword) -> bool {
        if self.at_keyword(keyword) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_keyword(&mut self, keyword: Keyword, message: &str) -> bool {
        if self.eat_keyword(keyword) {
            true
        } else {
            self.error_current(message);
            false
        }
    }

    fn at_operator(&self, operator: Operator) -> bool {
        matches!(self.current_kind(), TokenKind::Operator(current) if current == operator)
    }

    fn eat_operator(&mut self, operator: Operator) -> bool {
        if self.at_operator(operator) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_operator(&mut self, operator: Operator, message: &str) -> bool {
        if self.eat_operator(operator) {
            true
        } else {
            self.error_current(message);
            false
        }
    }

    fn at_punct(&self, punct: Punct) -> bool {
        matches!(self.current_kind(), TokenKind::Punct(current) if current == punct)
    }

    fn eat_punct(&mut self, punct: Punct) -> bool {
        if self.at_punct(punct) {
            self.bump();
            true
        } else {
            false
        }
    }

    fn expect_punct(&mut self, punct: Punct, message: &str) -> bool {
        if self.eat_punct(punct) {
            true
        } else {
            self.error_current(message);
            false
        }
    }

    fn peek_punct(&self, offset: usize, punct: Punct) -> bool {
        matches!(self.peek_kind(offset), Some(TokenKind::Punct(current)) if current == punct)
    }

    fn current_kind(&self) -> TokenKind {
        self.current_token().kind
    }

    fn peek_kind(&self, offset: usize) -> Option<TokenKind> {
        self.tokens.get(self.pos + offset).map(|token| token.kind)
    }

    fn current_token(&self) -> &Token {
        self.tokens
            .get(self.pos)
            .or_else(|| self.tokens.last())
            .expect("lexer must emit EOF token")
    }

    fn current_range(&self) -> TextRange {
        if self.at_eof() {
            TextRange::empty(self.source, self.source_len)
        } else {
            self.current_token().range
        }
    }

    fn at_eof(&self) -> bool {
        matches!(self.current_kind(), TokenKind::EndOfFile)
    }

    fn bump(&mut self) {
        if !self.at_eof() {
            self.pos += 1;
        }
    }

    fn error_current(&mut self, message: impl Into<String>) {
        self.error_at(self.current_range(), message);
    }

    fn error_at(&mut self, range: TextRange, message: impl Into<String>) {
        Diag(Diagnostic::error(range, message)).accumulate(self.db);
    }

    fn warn_at(&mut self, range: TextRange, message: impl Into<String>) {
        Diag(Diagnostic::warning(range, message)).accumulate(self.db);
    }

    fn error_node(&self, start_pos: usize) -> ast::ErrorNode {
        ast::ErrorNode::new(self.range_from(start_pos))
    }

    fn range_from(&self, start_pos: usize) -> TextRange {
        let start = self.position_start(start_pos);
        let end = if self.pos > start_pos {
            self.tokens
                .get(self.pos - 1)
                .and_then(|token| token.range.end())
                .unwrap_or(self.source_len)
        } else {
            start
        };
        TextRange::from_bounds(self.source, start, end)
    }

    fn position_start(&self, pos: usize) -> TextSize {
        self.tokens
            .get(pos)
            .and_then(|token| token.range.start())
            .unwrap_or(self.source_len)
    }

    fn merge_ranges(&self, left: TextRange, right: TextRange) -> TextRange {
        match (left.source(), left.start(), right.source(), right.end()) {
            (Some(left_source), Some(start), Some(right_source), Some(end))
                if left_source == right_source =>
            {
                TextRange::from_bounds(left_source, start, end)
            }
            _ => TextRange::generated(),
        }
    }
}

fn is_statement_start_keyword(keyword: Keyword) -> bool {
    matches!(
        keyword,
        Keyword::Bundle
            | Keyword::Module
            | Keyword::Let
            | Keyword::Do
            | Keyword::Use
            | Keyword::Type
            | Keyword::Trait
            | Keyword::Impl
            | Keyword::Wasm
    )
}

fn is_statement_boundary_keyword(keyword: Keyword) -> bool {
    is_statement_start_keyword(keyword) || keyword == Keyword::End
}

fn default_bundle_version() -> Version {
    Version::new(0, 0, 0)
}

fn decode_string_literal(raw: &str) -> Option<String> {
    let content = raw.strip_prefix('"')?.strip_suffix('"')?;
    let mut out = String::new();
    let mut chars = content.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }

        let escape = chars.next()?;
        match escape {
            'n' => out.push('\n'),
            't' => out.push('\t'),
            'r' => out.push('\r'),
            '\\' => out.push('\\'),
            '"' => out.push('"'),
            '\'' => out.push('\''),
            'u' => {
                if chars.next()? != '{' {
                    return None;
                }

                let mut digits = String::new();
                while let Some(peek) = chars.peek().copied() {
                    if peek == '}' {
                        break;
                    }
                    digits.push(peek);
                    chars.next();
                }

                if chars.next()? != '}' || digits.is_empty() {
                    return None;
                }

                let value = u32::from_str_radix(&digits, 16).ok()?;
                out.push(char::from_u32(value)?);
            }
            _ => return None,
        }
    }

    Some(out)
}
