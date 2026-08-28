//! Deterministic JavaScript generation for linked Ruddy artifacts.
//!
//! This crate deliberately performs no I/O. Drivers compile and link an artifact,
//! call [`generate`], and decide where (or whether) to install the resulting ESM.

use ruddy::artifact::{
    Artifact, Block, Callee, End, FieldKey, Instr, Literal, Op, PrimCase, Rep, TagCase,
};
use std::{
    collections::{BTreeMap, HashSet},
    error, fmt,
};

/// A JavaScript generation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The backend only accepts a linked root artifact.
    Unlinked,
    /// A header value has no corresponding runtime global or extern.
    UnresolvedPublicValue(String),
    /// Two public names cannot occupy the same export tree.
    ExportTreeConflict(String),
    /// An operation refers outside the artifact function table.
    InvalidFunctionReference(u64),
    /// An operation refers to an unknown qualified global.
    InvalidGlobalReference(String),
    /// Compiler-produced input uses a construct this target cannot represent.
    TargetLimitation(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unlinked => f.write_str("the JavaScript backend requires a linked artifact"),
            Self::UnresolvedPublicValue(name) => {
                write!(f, "public value `{name}` has no runtime definition")
            }
            Self::ExportTreeConflict(path) => {
                write!(f, "conflicting JavaScript export path `{path}`")
            }
            Self::InvalidFunctionReference(index) => {
                write!(f, "invalid function reference {index}")
            }
            Self::InvalidGlobalReference(name) => write!(f, "invalid global reference `{name}`"),
            Self::TargetLimitation(message) => write!(f, "JavaScript target limitation: {message}"),
        }
    }
}

impl error::Error for Error {}

#[derive(Default)]
struct ExportNode {
    value: Option<String>,
    children: BTreeMap<String, ExportNode>,
}

/// Generate a complete modern ECMAScript module from a linked artifact.
pub fn generate(artifact: &Artifact) -> Result<String, Error> {
    Generator::new(artifact)?.generate()
}

struct Generator<'a> {
    artifact: &'a Artifact,
    runtime_names: HashSet<&'a str>,
    exports: ExportNode,
}

impl<'a> Generator<'a> {
    fn new(artifact: &'a Artifact) -> Result<Self, Error> {
        if !artifact.header.dependencies.is_empty() {
            return Err(Error::Unlinked);
        }
        let mut runtime_names = HashSet::new();
        for external in &artifact.lir.externs {
            if external.target.is_empty() || external.target.iter().any(String::is_empty) {
                return Err(Error::TargetLimitation(format!(
                    "extern `{}` has an empty target path or segment",
                    external.name
                )));
            }
            if !runtime_names.insert(external.name.as_str()) {
                return Err(Error::InvalidGlobalReference(external.name.clone()));
            }
        }
        for global in &artifact.lir.globals {
            if !runtime_names.insert(global.name.as_str()) {
                return Err(Error::InvalidGlobalReference(global.name.clone()));
            }
        }

        let prefix = format!(
            "{}@{}::",
            artifact.header.identity.name, artifact.header.identity.version
        );
        let mut exports = ExportNode::default();
        for value in &artifact.header.values {
            if !runtime_names.contains(value.name.as_str()) {
                return Err(Error::UnresolvedPublicValue(value.name.clone()));
            }
            let Some(relative) = value.name.strip_prefix(&prefix) else {
                return Err(Error::UnresolvedPublicValue(value.name.clone()));
            };
            let path: Vec<_> = relative.split("::").collect();
            if path.is_empty() || path.iter().any(|part| part.is_empty()) {
                return Err(Error::ExportTreeConflict(relative.to_owned()));
            }
            insert_export(&mut exports, &path, &value.name)?;
        }

        let generator = Self {
            artifact,
            runtime_names,
            exports,
        };
        generator.validate()?;
        Ok(generator)
    }

    fn validate(&self) -> Result<(), Error> {
        let mut pending: Vec<&Block> = self
            .artifact
            .lir
            .functions
            .iter()
            .map(|f| &f.body)
            .chain(self.artifact.lir.globals.iter().map(|g| &g.body))
            .collect();
        while let Some(block) = pending.pop() {
            for instr in &block.instrs {
                match &instr.op {
                    Op::Closure { func, .. }
                    | Op::Call {
                        callee: Callee::Direct(func),
                        ..
                    } if *func >= self.artifact.lir.functions.len() as u64 => {
                        return Err(Error::InvalidFunctionReference(*func));
                    }
                    Op::Global { target } if !self.runtime_names.contains(target.as_str()) => {
                        return Err(Error::InvalidGlobalReference(target.clone()));
                    }
                    Op::Catch { body, .. } => pending.push(body),
                    Op::SwitchTag {
                        cases, fallback, ..
                    } => {
                        pending.extend(cases.iter().map(|case| &case.block));
                        pending.extend(fallback.as_deref());
                    }
                    Op::SwitchPrim {
                        cases, fallback, ..
                    } => {
                        pending.extend(cases.iter().map(|case| &case.block));
                        pending.extend(fallback.as_deref());
                    }
                    Op::SwitchPresence {
                        present, absent, ..
                    } => {
                        pending.push(present);
                        pending.push(absent);
                    }
                    Op::SwitchRest { none, some, .. } => {
                        pending.push(none);
                        pending.push(some);
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn generate(&self) -> Result<String, Error> {
        let mut out = String::new();
        out.push_str("// Generated by Ruddy.\n");
        out.push_str(RUNTIME);
        out.push_str("const $f = [\n");
        for function in &self.artifact.lir.functions {
            out.push_str("  function(");
            for (i, param) in function.params.iter().enumerate() {
                if i != 0 {
                    out.push_str(", ");
                }
                out.push_str(&temp(param.temp));
            }
            out.push_str(") { return ");
            self.block(&function.body, &mut out, 2)?;
            out.push_str("; },\n");
        }
        out.push_str("];\nconst $g = Object.create(null);\n");
        for external in &self.artifact.lir.externs {
            out.push_str("$g[");
            string(&external.name, &mut out);
            out.push_str("] = $extern([");
            for (i, segment) in external.target.iter().enumerate() {
                if i != 0 {
                    out.push_str(", ");
                }
                string(segment, &mut out);
            }
            out.push_str("]);\n");
        }
        for global in &self.artifact.lir.globals {
            out.push_str("$g[");
            string(&global.name, &mut out);
            out.push_str("] = ");
            self.block(&global.body, &mut out, 0)?;
            out.push_str(";\n");
        }

        let mut bindings = Vec::new();
        for (name, node) in &self.exports.children {
            let binding = format!("$e{}", bindings.len());
            emit_export_value(node, &binding, &mut out);
            bindings.push((binding, name));
        }
        if bindings.is_empty() {
            out.push_str("export {};\n");
        } else {
            out.push_str("export { ");
            for (i, (binding, name)) in bindings.iter().enumerate() {
                if i != 0 {
                    out.push_str(", ");
                }
                out.push_str(binding);
                out.push_str(" as ");
                export_name(name, &mut out);
            }
            out.push_str(" };\n");
        }
        Ok(out)
    }

    fn block(&self, block: &Block, out: &mut String, depth: usize) -> Result<(), Error> {
        out.push_str("(function() {\n");
        for instr in &block.instrs {
            indent(out, depth + 1);
            out.push_str("const ");
            out.push_str(&temp(instr.temp));
            out.push_str(" = ");
            self.op(instr, out, depth + 1)?;
            out.push_str(";\n");
        }
        indent(out, depth + 1);
        match block.end {
            End::Ret(value) | End::Yield(value) => {
                out.push_str("return ");
                out.push_str(&temp(value));
                out.push(';');
            }
            End::Throw { tag, value } => {
                out.push_str("throw $effectValue(");
                out.push_str(&temp(tag));
                out.push_str(", ");
                out.push_str(&temp(value));
                out.push_str(");");
            }
        }
        out.push('\n');
        indent(out, depth);
        out.push_str("})()");
        Ok(())
    }

    fn op(&self, instr: &Instr, out: &mut String, depth: usize) -> Result<(), Error> {
        let v = |n| temp(n);
        match &instr.op {
            Op::Const(lit) => literal(lit, out),
            Op::Neg(a) => {
                out.push_str("(-");
                out.push_str(&v(*a));
                out.push(')');
            }
            Op::Not(a) => {
                out.push_str("(!");
                out.push_str(&v(*a));
                out.push(')');
            }
            Op::And { left, right } => binary(out, &v(*left), "&&", &v(*right)),
            Op::Or { left, right } => binary(out, &v(*left), "||", &v(*right)),
            Op::Xor { left, right } => {
                out.push_str("Boolean(");
                binary(out, &v(*left), "!==", &v(*right));
                out.push(')');
            }
            Op::Add { left, right } => binary(out, &v(*left), "+", &v(*right)),
            Op::Sub { left, right } if instr.rep == Rep::Nat => {
                out.push_str("Math.max(0, ");
                out.push_str(&v(*left));
                out.push_str(" - ");
                out.push_str(&v(*right));
                out.push(')');
            }
            Op::Sub { left, right } => binary(out, &v(*left), "-", &v(*right)),
            Op::Mul { left, right } => binary(out, &v(*left), "*", &v(*right)),
            Op::Div { left, right } if matches!(instr.rep, Rep::Nat | Rep::Int) => {
                out.push_str("Math.trunc(");
                out.push_str(&v(*left));
                out.push_str(" / ");
                out.push_str(&v(*right));
                out.push(')');
            }
            Op::Div { left, right } => binary(out, &v(*left), "/", &v(*right)),
            Op::Struct(fields) => {
                out.push_str("$record([");
                for (i, (key, value)) in fields.iter().enumerate() {
                    if i != 0 {
                        out.push_str(", ");
                    }
                    out.push('[');
                    field_key(key, out);
                    out.push_str(", ");
                    out.push_str(&v(*value));
                    out.push(']');
                }
                out.push_str("])");
            }
            Op::Merge(values) => {
                out.push_str("Object.assign(Object.create(null)");
                for value in values {
                    out.push_str(", ");
                    out.push_str(&v(*value));
                }
                out.push(')');
            }
            Op::Project { base, field } => {
                out.push_str(&v(*base));
                out.push('[');
                field_key(field, out);
                out.push(']');
            }
            Op::Tag { name, payload } => {
                out.push_str("$sum(");
                string(name, out);
                out.push_str(", ");
                if let Some(payload) = payload {
                    out.push_str(&v(*payload));
                } else {
                    out.push_str("undefined");
                }
                out.push(')');
            }
            Op::Payload(value) => {
                out.push_str(&v(*value));
                out.push_str("[$payload]");
            }
            Op::Closure { func, captures } => {
                out.push_str("(...$a) => $f[");
                out.push_str(&func.to_string());
                out.push_str("](");
                for (i, capture) in captures.iter().enumerate() {
                    if i != 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&v(*capture));
                }
                if !captures.is_empty() {
                    out.push_str(", ");
                }
                out.push_str("...$a)");
            }
            Op::Call { callee, args } => {
                match callee {
                    Callee::Direct(i) => {
                        out.push_str("$f[");
                        out.push_str(&i.to_string());
                        out.push(']');
                    }
                    Callee::Indirect(t) => out.push_str(&v(*t)),
                }
                out.push('(');
                comma_temps(args, out);
                out.push(')');
            }
            Op::Global { target } => {
                out.push_str("$g[");
                string(target, out);
                out.push(']');
            }
            Op::NewTag => out.push_str("Symbol()"),
            Op::Catch { tag, body } => {
                out.push_str("$catch(");
                out.push_str(&v(*tag));
                out.push_str(", () => ");
                self.block(body, out, depth)?;
                out.push(')');
            }
            Op::SwitchTag {
                on,
                cases,
                fallback,
            } => self.switch_tag(*on, cases, fallback.as_deref(), out, depth)?,
            Op::SwitchPrim {
                on,
                cases,
                fallback,
            } => self.switch_prim(*on, cases, fallback.as_deref(), out, depth)?,
            Op::SwitchPresence {
                on,
                field,
                present,
                absent,
            } => {
                out.push_str("($own(");
                out.push_str(&v(*on));
                out.push_str(", ");
                string(field, out);
                out.push_str(") ? ");
                self.block(present, out, depth)?;
                out.push_str(" : ");
                self.block(absent, out, depth)?;
                out.push(')');
            }
            Op::SwitchRest {
                on,
                fields,
                none,
                some,
            } => {
                out.push_str("($hasRest(");
                out.push_str(&v(*on));
                out.push_str(", [");
                for (i, field) in fields.iter().enumerate() {
                    if i != 0 {
                        out.push_str(", ");
                    }
                    string(field, out);
                }
                out.push_str("]) ? ");
                self.block(some, out, depth)?;
                out.push_str(" : ");
                self.block(none, out, depth)?;
                out.push(')');
            }
        }
        Ok(())
    }

    fn switch_tag(
        &self,
        on: u32,
        cases: &[TagCase],
        fallback: Option<&Block>,
        out: &mut String,
        depth: usize,
    ) -> Result<(), Error> {
        out.push_str("(function() { switch (");
        out.push_str(&temp(on));
        out.push_str("[$tag]) {");
        for case in cases {
            out.push_str(" case ");
            string(&case.name, out);
            out.push_str(": return ");
            self.block(&case.block, out, depth + 1)?;
            out.push(';');
        }
        out.push_str(" default: return ");
        if let Some(fallback) = fallback {
            self.block(fallback, out, depth + 1)?;
        } else {
            out.push_str("$unreachable()");
        }
        out.push_str("; } })()");
        Ok(())
    }

    fn switch_prim(
        &self,
        on: u32,
        cases: &[PrimCase],
        fallback: Option<&Block>,
        out: &mut String,
        depth: usize,
    ) -> Result<(), Error> {
        out.push_str("(function() {");
        for case in cases {
            out.push_str(" if (");
            let integer = matches!(case.value, Literal::Natural(_) | Literal::Integer(_));
            if !integer {
                out.push_str("$same(");
            }
            out.push_str(&temp(on));
            out.push_str(if integer { " === " } else { ", " });
            literal(&case.value, out);
            if !integer {
                out.push(')');
            }
            out.push_str(") return ");
            self.block(&case.block, out, depth + 1)?;
            out.push(';');
        }
        out.push_str(" return ");
        if let Some(fallback) = fallback {
            self.block(fallback, out, depth + 1)?;
        } else {
            out.push_str("$unreachable()");
        }
        out.push_str("; })()");
        Ok(())
    }
}

fn insert_export(node: &mut ExportNode, path: &[&str], qualified: &str) -> Result<(), Error> {
    let mut at = node;
    for (i, segment) in path.iter().enumerate() {
        if at.value.is_some() {
            return Err(Error::ExportTreeConflict(path[..i].join("::")));
        }
        at = at.children.entry((*segment).to_owned()).or_default();
    }
    if at.value.is_some() || !at.children.is_empty() {
        return Err(Error::ExportTreeConflict(path.join("::")));
    }
    at.value = Some(qualified.to_owned());
    Ok(())
}

fn emit_export_value(node: &ExportNode, binding: &str, out: &mut String) {
    out.push_str("const ");
    out.push_str(binding);
    out.push_str(" = ");
    if let Some(value) = &node.value {
        out.push_str("$g[");
        string(value, out);
        out.push(']');
    } else {
        out.push_str("$namespace([");
        for (i, (name, child)) in node.children.iter().enumerate() {
            if i != 0 {
                out.push_str(", ");
            }
            out.push('[');
            string(name, out);
            out.push_str(", ");
            emit_export_expression(child, out);
            out.push(']');
        }
        out.push_str("])");
    }
    out.push_str(";\n");
}

fn emit_export_expression(node: &ExportNode, out: &mut String) {
    if let Some(value) = &node.value {
        out.push_str("$g[");
        string(value, out);
        out.push(']');
    } else {
        out.push_str("$namespace([");
        for (i, (name, child)) in node.children.iter().enumerate() {
            if i != 0 {
                out.push_str(", ");
            }
            out.push('[');
            string(name, out);
            out.push_str(", ");
            emit_export_expression(child, out);
            out.push(']');
        }
        out.push_str("])");
    }
}

const RUNTIME: &str = r#"const $unnamed = Symbol("unnamed operation");
const $tag = Symbol("sum tag");
const $payload = Symbol("sum payload");
const $effect = Symbol("effect envelope");
const $dv = new DataView(new ArrayBuffer(8));
const $real = $bits => { $dv.setBigUint64(0, BigInt("0x" + $bits)); return $dv.getFloat64(0); };
const $record = $entries => { const $o = Object.create(null); for (const [$k, $v] of $entries) $o[$k] = $v; return $o; };
const $sum = ($name, $value) => { const $o = Object.create(null); $o[$tag] = $name; $o[$payload] = $value; return $o; };
const $namespace = $entries => Object.freeze($record($entries));
const $own = ($o, $k) => Object.prototype.hasOwnProperty.call($o, $k);
const $hasRest = ($o, $known) => Reflect.ownKeys($o).some($k => typeof $k !== "string" || !$known.includes($k));
const $same = Object.is;
const $effectValue = ($identity, $value) => ({ [$effect]: true, identity: $identity, value: $value });
const $catch = ($identity, $body) => { try { return $body(); } catch ($e) { if ($e !== null && typeof $e === "object" && $e[$effect] === true && $e.identity === $identity) return $e.value; throw $e; } };
const $extern = $path => { let $owner = globalThis; let $value = globalThis; for (const $part of $path) { $owner = $value; if ($value == null || !($part in Object($value))) throw new Error("missing Ruddy extern: " + $path.join(".")); $value = $value[$part]; } return typeof $value === "function" ? $value.bind($owner) : $value; };
const $unreachable = () => { throw new Error("unreachable Ruddy LIR branch"); };
"#;

fn temp(value: u32) -> String {
    format!("$v{value}")
}
fn indent(out: &mut String, depth: usize) {
    for _ in 0..depth {
        out.push_str("  ");
    }
}
fn binary(out: &mut String, left: &str, op: &str, right: &str) {
    out.push('(');
    out.push_str(left);
    out.push(' ');
    out.push_str(op);
    out.push(' ');
    out.push_str(right);
    out.push(')');
}
fn comma_temps(values: &[u32], out: &mut String) {
    for (i, value) in values.iter().enumerate() {
        if i != 0 {
            out.push_str(", ");
        }
        out.push_str(&temp(*value));
    }
}
fn field_key(key: &FieldKey, out: &mut String) {
    match key {
        FieldKey::Named(name) => string(name, out),
        FieldKey::UnnamedOperation => out.push_str("$unnamed"),
    }
}

fn literal(value: &Literal, out: &mut String) {
    match value {
        Literal::Natural(value) => out.push_str(&value.to_string()),
        Literal::Integer(value) => out.push_str(&value.to_string()),
        Literal::Real(bits) => {
            out.push_str("$real(\"");
            out.push_str(&format!("{bits:016x}"));
            out.push_str("\")");
        }
        Literal::String(value) => string(value, out),
        Literal::Boolean(value) => out.push_str(if *value { "true" } else { "false" }),
    }
}

fn string(value: &str, out: &mut String) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            ch if ch <= '\u{1f}' || ch == '\u{7f}' => {
                use fmt::Write;
                write!(out, "\\u{:04x}", ch as u32).expect("writing String cannot fail");
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

fn export_name(name: &str, out: &mut String) {
    let mut chars = name.chars();
    let simple = chars
        .next()
        .is_some_and(|c| c == '_' || c == '$' || c.is_ascii_alphabetic())
        && chars.all(|c| c == '_' || c == '$' || c.is_ascii_alphanumeric());
    if simple {
        out.push_str(name);
    } else {
        string(name, out);
    }
}
