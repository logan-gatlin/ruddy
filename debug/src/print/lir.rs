//! Rendering the lowered instruction stream.
//!
//! Unlike [`ast`](super::ast) and [`ir`](super::ir), this prints no surface
//! syntax: LIR has none, and never will. What it has is one canonical listing —
//! a section per function and per global, one instruction per line as
//! `%N: rep = op operands`, child blocks indented under the instruction that
//! owns them, and terminators written bare. That listing lives here, in the
//! debugger, for the reason every other printer does: turning a compiler
//! structure into something a person reads is a debugging concern, and the
//! compiler crate does not do it.
//!
//! The LIR tab and the tests both read this module, so there is one format
//! rather than two that could drift. [`arms`] is why: the tree the tab builds
//! and the text this file writes walk the same child blocks in the same order.

use std::{collections::HashSet, fmt::Write};

use ruddy::{
    ir::{Literal, Program},
    lir::{
        Block, Callee, End, Extern, FieldKey, Function, Global, Instr, Op, Output, Rep, Terminator,
    },
    types::{EffectId, Shape},
};

/// The structural effect keys that are internal to one lowered program.
///
/// An encoded identity has no intrinsic meaning in an LIR struct field: quoted
/// source fields may resemble one. Only generated evidence operations naming
/// keys minted for effects hide the opaque interface component.
#[derive(Default)]
pub struct Labels {
    effects: HashSet<String>,
}

impl Labels {
    pub fn new(program: &Program) -> Self {
        let effects = program
            .effect_ids
            .values()
            .filter_map(|effect| match effect {
                EffectId::Structural { .. } => Some(effect.row_key()),
                EffectId::Pending(_) => None,
            })
            .collect();
        Self { effects }
    }

    fn named<'a>(&self, name: &'a str, generated: bool) -> &'a str {
        match generated && self.effects.contains(name) {
            true => EffectId::parse_row_key(name).map_or(name, |(name, _)| name),
            false => name,
        }
    }

    fn field<'a>(&self, field: &'a FieldKey, generated: bool) -> &'a str {
        match field {
            FieldKey::Named(name) => self.named(name, generated),
            FieldKey::UnnamedOperation => "<unnamed>",
        }
    }
}

/// How far one level of nesting indents. An arm label sits one level under its
/// instruction and the arm's block one level under that, which is what makes a
/// decision tree readable as a tree.
const STEP: usize = 2;

/// The whole listing: imports, functions, then initialized globals, one blank
/// line apart. Imports lead because they have no initializer block; functions
/// precede globals because a global may refer to a lifted wrapper above it.
pub fn program(output: &Output, labels: &Labels) -> String {
    let mut out = String::new();
    let mut first = true;
    for external in &output.externs {
        if !first {
            out.push('\n');
        }
        first = false;
        let _ = writeln!(out, "{}", extern_header(external));
    }
    for function in &output.functions {
        if !first {
            out.push('\n');
        }
        first = false;
        let _ = writeln!(out, "{}", signature(function));
        block(output, labels, &function.body, STEP, &mut out);
    }
    for global in &output.globals {
        if !first {
            out.push('\n');
        }
        first = false;
        let _ = writeln!(out, "{}", header(global));
        block(output, labels, &global.body, STEP, &mut out);
    }
    out
}

/// A target-provided declaration. It has no initializer block: a backend
/// imports the value from this target expression.
pub fn extern_header(external: &Extern) -> String {
    format!(
        "extern {}: {} = {}",
        external.name,
        rep(external.rep),
        super::string(&external.target)
    )
}

/// A function's header line: its name and its parameters, each with its
/// representation.
pub fn signature(function: &Function) -> String {
    let params: Vec<String> = function
        .params
        .iter()
        .map(|param| format!("%{}: {}", param.temp, rep(param.rep)))
        .collect();
    format!("fn {}({}):", function.name, params.join(", "))
}

/// A global's header line.
pub fn header(global: &Global) -> String {
    format!("global {}:", global.name)
}

/// One instruction, without its child blocks: the temp it assigns, how that temp
/// is held, and what it does. A block-valued instruction ends in the `:` its
/// blocks hang under.
pub fn instruction(output: &Output, labels: &Labels, instr: &Instr) -> String {
    format!(
        "%{}: {} = {}",
        instr.temp,
        rep(instr.rep),
        operation(output, labels, instr.span.is_generated(), &instr.op)
    )
}

/// The opcode alone, which is what a row of the tab is labelled with.
pub fn opcode(op: &Op) -> &'static str {
    match op {
        Op::Const(_) => "const",
        Op::Neg(_) => "neg",
        Op::Not(_) => "not",
        Op::And { .. } => "and",
        Op::Or { .. } => "or",
        Op::Xor { .. } => "xor",
        Op::Add { .. } => "add",
        Op::Sub { .. } => "sub",
        Op::Mul { .. } => "mul",
        Op::Div { .. } => "div",
        Op::Struct(_) => "struct",
        Op::Merge(_) => "merge",
        Op::Project { .. } => "project",
        Op::Tag { .. } => "tag",
        Op::Payload(_) => "payload",
        Op::Closure { .. } => "closure",
        Op::Call { .. } => "call",
        Op::RawCall { .. } => "raw_call",
        Op::Extern { .. } => "extern",
        Op::Global { .. } => "global",
        Op::NewTag => "new_tag",
        Op::Catch { .. } => "catch",
        Op::SwitchTag { .. } => "switch_tag",
        Op::SwitchPrim { .. } => "switch_prim",
        Op::SwitchPresence { .. } => "switch_presence",
        Op::SwitchRest { .. } => "switch_rest",
    }
}

/// How a temp is held, as the one word the listing writes.
pub fn rep(rep: Rep) -> &'static str {
    match rep {
        Rep::Nat => "nat",
        Rep::Int => "int64",
        Rep::Real => "real64",
        Rep::String => "string",
        Rep::Boolean => "boolean",
        Rep::Unit => "unit",
        Rep::Struct => "struct",
        Rep::Sum => "sum",
        Rep::Fn => "fn",
        Rep::Any => "any",
    }
}

/// A terminator, written bare — a block ends with one and it assigns nothing.
pub fn terminator(end: &Terminator) -> String {
    match &end.kind {
        End::Ret(temp) => format!("ret %{temp}"),
        End::Yield(temp) => format!("yield %{temp}"),
        End::Throw { tag, value } => format!("throw %{tag}, %{value}"),
    }
}

/// Which of the three a terminator is, for the row that shows it.
pub fn end_label(end: &Terminator) -> &'static str {
    match end.kind {
        End::Ret(_) => "ret",
        End::Yield(_) => "yield",
        End::Throw { .. } => "throw",
    }
}

/// The blocks one instruction owns, each with the answer that selects it — or
/// `None` for a block nothing selects, which is a `catch`'s single body.
///
/// The one place the shape of a block-valued instruction is written down. The
/// listing and the tab both walk it, so a new dispatch cannot reach one of them
/// and not the other.
pub fn arms(op: &Op) -> Vec<(Option<String>, &Block)> {
    match op {
        Op::Catch { body, .. } => vec![(None, body)],
        Op::SwitchTag {
            cases, fallback, ..
        } => cases
            .iter()
            .map(|case| {
                (
                    Some(crate::print::label(Shape::Sum, &case.name)),
                    &case.block,
                )
            })
            .chain(
                fallback
                    .iter()
                    .map(|block| (Some("else".to_string()), &**block)),
            )
            .collect(),
        Op::SwitchPrim {
            cases, fallback, ..
        } => cases
            .iter()
            .map(|case| (Some(literal(&case.value)), &case.block))
            .chain(
                fallback
                    .iter()
                    .map(|block| (Some("else".to_string()), &**block)),
            )
            .collect(),
        Op::SwitchPresence {
            present, absent, ..
        } => vec![
            (Some("present".to_string()), &**present),
            (Some("absent".to_string()), &**absent),
        ],
        Op::SwitchRest { none, some, .. } => vec![
            (Some("none".to_string()), &**none),
            (Some("some".to_string()), &**some),
        ],
        _ => Vec::new(),
    }
}

/// One block, indented: its instructions, then the terminator that ends it.
fn block(output: &Output, labels: &Labels, block: &Block, indent: usize, out: &mut String) {
    for instr in &block.instrs {
        let _ = writeln!(
            out,
            "{:indent$}{}",
            "",
            instruction(output, labels, instr),
            indent = indent
        );
        for (label, child) in arms(&instr.op) {
            match label {
                Some(label) => {
                    let _ = writeln!(out, "{:indent$}{label} =>", "", indent = indent + STEP);
                    self::block(output, labels, child, indent + STEP * 2, out);
                }
                None => self::block(output, labels, child, indent + STEP, out),
            }
        }
    }
    let _ = writeln!(
        out,
        "{:indent$}{}",
        "",
        terminator(&block.end),
        indent = indent
    );
}

/// Render a primitive literal exactly as source syntax would spell it.
/// Debug formatting keeps string delimiters and escapes intact.
fn literal(value: &Literal) -> String {
    match value {
        Literal::Natural(value) => format!("{value}n"),
        Literal::Integer(value) => format!("{value}i"),
        Literal::Real(value) => value.to_string(),
        Literal::String(value) => crate::print::string(value),
        Literal::Boolean(value) => value.to_string(),
    }
}

/// What one instruction does, with its operands.
fn operation(output: &Output, labels: &Labels, generated: bool, op: &Op) -> String {
    match op {
        Op::Const(value) => format!("const {}", literal(value)),
        Op::Neg(value) => format!("neg %{value}"),
        Op::Not(value) => format!("not %{value}"),
        Op::And { left, right } => format!("and %{left}, %{right}"),
        Op::Or { left, right } => format!("or %{left}, %{right}"),
        Op::Xor { left, right } => format!("xor %{left}, %{right}"),
        Op::Add { left, right } => format!("add %{left}, %{right}"),
        Op::Sub { left, right } => format!("sub %{left}, %{right}"),
        Op::Mul { left, right } => format!("mul %{left}, %{right}"),
        Op::Div { left, right } => format!("div %{left}, %{right}"),
        Op::Struct(fields) if fields.is_empty() => "struct {}".to_string(),
        Op::Struct(fields) => {
            let entries: Vec<String> = fields
                .iter()
                .map(|(field, temp)| {
                    let name = match field {
                        FieldKey::Named(_) => {
                            crate::print::label(Shape::Struct, labels.field(field, generated))
                        }
                        FieldKey::UnnamedOperation => "<unnamed>".to_string(),
                    };
                    format!("{name}: %{temp}")
                })
                .collect();
            format!("struct {{ {} }}", entries.join(", "))
        }
        Op::Merge(records) => {
            let laid: Vec<String> = records.iter().map(|temp| format!("%{temp}")).collect();
            format!("merge {}", laid.join(", "))
        }
        Op::Project { base, field } => {
            format!("project %{base}, {:?}", labels.field(field, generated))
        }
        Op::Tag {
            name,
            payload: None,
        } => format!("tag {}", crate::print::label(Shape::Sum, name)),
        Op::Tag {
            name,
            payload: Some(temp),
        } => format!("tag {}, %{temp}", crate::print::label(Shape::Sum, name)),
        Op::Payload(temp) => format!("payload %{temp}"),
        Op::Closure { func, captures } => {
            let held: Vec<String> = captures.iter().map(|temp| format!("%{temp}")).collect();
            format!(
                "closure {}, [{}]",
                output.functions[*func].name,
                held.join(", ")
            )
        }
        Op::Call { callee, args } => {
            let callee = match callee {
                Callee::Direct(func) => output.functions[*func].name.clone(),
                Callee::Indirect(temp) => format!("%{temp}"),
            };
            let mut written = vec![callee];
            written.extend(args.iter().map(|temp| format!("%{temp}")));
            format!("call {}", written.join(", "))
        }
        Op::RawCall { callee, args } => {
            let mut written = vec![format!("%{callee}")];
            written.extend(args.iter().map(|temp| format!("%{temp}")));
            format!("raw_call {}", written.join(", "))
        }
        Op::Extern { name, .. } => format!("extern {name}"),
        Op::Global { name, .. } => format!("global {name}"),
        Op::NewTag => "new_tag".to_string(),
        Op::Catch { tag, .. } => format!("catch %{tag}:"),
        Op::SwitchTag { on, .. } => format!("switch_tag %{on}:"),
        Op::SwitchPrim { on, .. } => format!("switch_prim %{on}:"),
        Op::SwitchPresence { on, field, .. } => {
            format!(
                "switch_presence %{on}, {:?}:",
                labels.named(field, generated)
            )
        }
        Op::SwitchRest { on, fields, .. } => {
            let names: Vec<String> = fields
                .iter()
                .map(|name| format!("{:?}", labels.named(name, generated)))
                .collect();
            format!("switch_rest %{on}, [{}]:", names.join(", "))
        }
    }
}
