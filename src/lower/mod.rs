pub mod ir;

mod lowerer;
mod query;
mod resolver_registry;
mod wasm;

use std::collections::HashMap;

use crate::parser::ast;
use crate::reporting::TextRange;
use crate::resolver::ResolverDispatch;
use crate::{engine::Source, resolver::ResolverToken};

pub use query::{lower_diagnostics, lower_diagnostics_fs, lower_source, lower_text, lower_text_fs};

pub const PATH_SEP: &str = "::";

#[salsa::interned(debug)]
struct InternedString<'db> {
    #[returns(ref)]
    pub text: String,
}

#[salsa::interned(debug)]
struct ModuleRequest<'db> {
    module_path: InternedString<'db>,
    source_canon: InternedString<'db>,
    file_root_path: InternedString<'db>,
    root_module_path: InternedString<'db>,
    root_source_canon: InternedString<'db>,
    resolver: ResolverToken<'db>,
    root_source: Option<Source>,
}

#[derive(Debug, Clone, PartialEq, salsa::Update)]
struct ModuleLoweringResult<'db> {
    module: ir::LoweredModule,
    children: Vec<ModuleRequest<'db>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Namespace {
    Module,
    Type,
    Trait,
    Term,
    Constructor,
}

impl std::fmt::Display for Namespace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            match self {
                Namespace::Module => "module",
                Namespace::Type => "type",
                Namespace::Trait => "trait",
                Namespace::Term => "term",
                Namespace::Constructor => "constructor",
            }
        )
    }
}

#[derive(Default)]
struct ScopeState {
    modules: HashMap<String, ir::QualifiedName>,
    types: HashMap<String, ir::QualifiedName>,
    traits: HashMap<String, ir::QualifiedName>,
    terms: HashMap<String, ir::QualifiedName>,
    constructors: HashMap<String, ir::QualifiedName>,
}

impl ScopeState {
    fn named_paths(map: &HashMap<String, ir::QualifiedName>) -> Vec<ir::NamedPath> {
        let mut entries = map
            .iter()
            .map(|(name, path)| ir::NamedPath {
                name: name.clone(),
                path: path.clone(),
            })
            .collect::<Vec<_>>();
        entries.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.path.text().cmp(&b.path.text())));
        entries
    }

    fn exports(&self) -> ir::ModuleExports {
        ir::ModuleExports {
            modules: Self::named_paths(&self.modules),
            types: Self::named_paths(&self.types),
            traits: Self::named_paths(&self.traits),
            terms: Self::named_paths(&self.terms),
            constructors: Self::named_paths(&self.constructors),
        }
    }
}

#[derive(Clone)]
struct ScopedMap<V: Copy> {
    values: HashMap<String, V>,
    history: Vec<ScopeChange<V>>,
    scope_starts: Vec<usize>,
}

#[derive(Clone)]
enum ScopeChange<V: Copy> {
    Inserted { name: String },
    Shadowed { name: String, previous: V },
}

impl<V: Copy> ScopedMap<V> {
    fn new() -> Self {
        Self {
            values: HashMap::new(),
            history: Vec::new(),
            scope_starts: vec![0],
        }
    }

    fn push_scope(&mut self) {
        self.scope_starts.push(self.history.len());
    }

    fn pop_scope(&mut self) {
        if self.scope_starts.len() <= 1 {
            return;
        }

        let scope_start = self.scope_starts.pop().unwrap_or(0);
        self.rollback_to(scope_start);
    }

    fn bind(&mut self, name: String, value: V) {
        let previous = self.values.insert(name.clone(), value);
        match previous {
            Some(previous) => self.history.push(ScopeChange::Shadowed { name, previous }),
            None => self.history.push(ScopeChange::Inserted { name }),
        }
    }

    fn lookup(&self, name: &str) -> Option<V> {
        self.values.get(name).copied()
    }

    fn rollback_to(&mut self, target_len: usize) {
        while self.history.len() > target_len {
            let Some(change) = self.history.pop() else {
                break;
            };
            match change {
                ScopeChange::Inserted { name } => {
                    self.values.remove(&name);
                }
                ScopeChange::Shadowed { name, previous } => {
                    self.values.insert(name, previous);
                }
            }
        }
    }
}

pub fn bake_string(raw: &str) -> Option<String> {
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

struct ExprEnv {
    locals: ScopedMap<ir::LocalId>,
    opened_modules: Vec<ir::QualifiedName>,
    module_aliases: HashMap<String, ir::QualifiedName>,
}

impl ExprEnv {
    fn new(
        opened_modules: impl IntoIterator<Item = ir::QualifiedName>,
        module_aliases: HashMap<String, ir::QualifiedName>,
    ) -> Self {
        Self {
            locals: ScopedMap::new(),
            opened_modules: opened_modules.into_iter().collect(),
            module_aliases,
        }
    }

    fn push_scope(&mut self) {
        self.locals.push_scope();
    }

    fn pop_scope(&mut self) {
        self.locals.pop_scope();
    }

    fn bind_local(&mut self, name: String, local: ir::LocalId) {
        self.locals.bind(name, local);
    }

    fn lookup_local(&self, name: &str) -> Option<ir::LocalId> {
        self.locals.lookup(name)
    }
}

#[derive(Clone)]
struct TypeEnv {
    locals: ScopedMap<ir::LocalId>,
}

impl TypeEnv {
    fn new() -> Self {
        Self {
            locals: ScopedMap::new(),
        }
    }

    fn push_scope(&mut self) {
        self.locals.push_scope();
    }

    fn pop_scope(&mut self) {
        self.locals.pop_scope();
    }

    fn bind_local(&mut self, name: String, local: ir::LocalId) {
        self.locals.bind(name, local);
    }

    fn lookup_local(&self, name: &str) -> Option<ir::LocalId> {
        self.locals.lookup(name)
    }
}

struct LookupContext<'a> {
    opened_modules: &'a [ir::QualifiedName],
    module_aliases: &'a HashMap<String, ir::QualifiedName>,
}

#[derive(Default, Clone)]
struct WasmModuleScope {
    functions: HashMap<String, u32>,
    globals: HashMap<String, u32>,
    function_decl_indices: HashMap<TextRange, u32>,
    global_decl_indices: HashMap<TextRange, u32>,
}

#[derive(Default, Clone)]
struct WasmLocalScope {
    names: HashMap<String, u32>,
    next_index: u32,
}

impl WasmLocalScope {
    fn bind(&mut self, name: String, range: TextRange) -> ir::WasmBinding {
        let index = self.next_index;
        self.next_index = self.next_index.saturating_add(1);
        self.names.entry(name.clone()).or_insert(index);
        ir::WasmBinding { name, index, range }
    }

    fn insert_name_if_missing(&mut self, name: String, index: u32) {
        self.names.entry(name).or_insert(index);
    }

    fn lookup(&self, name: &str) -> Option<u32> {
        self.names.get(name).copied()
    }
}

struct WasmIndexResolution {
    index: u32,
    symbol: Option<ir::WasmResolvedSymbol>,
}

struct WasmStreamCursor<'a> {
    items: &'a [ast::SExpr],
    pos: usize,
}

impl<'a> WasmStreamCursor<'a> {
    fn new(items: &'a [ast::SExpr]) -> Self {
        Self { items, pos: 0 }
    }

    fn is_eof(&self) -> bool {
        self.pos >= self.items.len()
    }

    fn peek(&self) -> Option<&'a ast::SExpr> {
        self.items.get(self.pos)
    }

    fn next(&mut self) -> Option<&'a ast::SExpr> {
        let item = self.items.get(self.pos)?;
        self.pos += 1;
        Some(item)
    }

    fn mark(&self) -> usize {
        self.pos
    }

    fn reset(&mut self, mark: usize) {
        self.pos = mark;
    }
}

struct ModuleLowerer<'db> {
    db: &'db dyn salsa::Database,
    request: ModuleRequest<'db>,
    source_canon: String,
    source_contents: String,
    module_path: Vec<String>,
    bundle_name: String,
    resolver: ResolverDispatch,
    scope: ScopeState,
    opened_modules: Vec<ir::QualifiedName>,
    module_aliases: HashMap<String, ir::QualifiedName>,
    module_requests: HashMap<String, ModuleRequest<'db>>,
    children: Vec<ModuleRequest<'db>>,
    wasm_scope: WasmModuleScope,
    next_local_id: u32,
}

#[cfg(test)]
mod tests;
