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

use std::fmt::Write;

use ruddy::lir::{Block, Callee, End, Function, Global, Instr, Op, Output, Rep, Terminator};

/// How far one level of nesting indents. An arm label sits one level under its
/// instruction and the arm's block one level under that, which is what makes a
/// decision tree readable as a tree.
const STEP: usize = 2;

/// The whole listing: every function, then every global, one blank line apart.
///
/// Functions first because they are what a global refers to — a definition whose
/// value is a `fn` has a global holding nothing but the closure of a wrapper
/// printed above it.
pub fn program(output: &Output) -> String {
    let mut out = String::new();
    let mut first = true;
    for function in &output.functions {
        if !first {
            out.push('\n');
        }
        first = false;
        let _ = writeln!(out, "{}", signature(function));
        block(output, &function.body, STEP, &mut out);
    }
    for global in &output.globals {
        if !first {
            out.push('\n');
        }
        first = false;
        let _ = writeln!(out, "{}", header(global));
        block(output, &global.body, STEP, &mut out);
    }
    out
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
pub fn instruction(output: &Output, instr: &Instr) -> String {
    format!(
        "%{}: {} = {}",
        instr.temp,
        rep(instr.rep),
        operation(output, &instr.op)
    )
}

/// The opcode alone, which is what a row of the tab is labelled with.
pub fn opcode(op: &Op) -> &'static str {
    match op {
        Op::Const(_) | Op::ConstInt(_) | Op::ConstReal(_) => "const",
        Op::Struct(_) => "struct",
        Op::Merge(_) => "merge",
        Op::Project { .. } => "project",
        Op::Tag { .. } => "tag",
        Op::Payload(_) => "payload",
        Op::Closure { .. } => "closure",
        Op::Call { .. } => "call",
        Op::Global { .. } => "global",
        Op::NewTag => "new_tag",
        Op::Catch { .. } => "catch",
        Op::SwitchTag { .. } => "switch_tag",
        Op::SwitchNat { .. } => "switch_nat",
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
            .map(|case| (Some(format!("#{}", case.name)), &case.block))
            .chain(
                fallback
                    .iter()
                    .map(|block| (Some("else".to_string()), &**block)),
            )
            .collect(),
        Op::SwitchNat {
            cases, fallback, ..
        } => cases
            .iter()
            .map(|case| (Some(case.value.to_string()), &case.block))
            .chain(std::iter::once((Some("else".to_string()), &**fallback)))
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
fn block(output: &Output, block: &Block, indent: usize, out: &mut String) {
    for instr in &block.instrs {
        let _ = writeln!(
            out,
            "{:indent$}{}",
            "",
            instruction(output, instr),
            indent = indent
        );
        for (label, child) in arms(&instr.op) {
            match label {
                Some(label) => {
                    let _ = writeln!(out, "{:indent$}{label} =>", "", indent = indent + STEP);
                    self::block(output, child, indent + STEP * 2, out);
                }
                None => self::block(output, child, indent + STEP, out),
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

/// Hide the internal structural interface when an effect label reaches LIR
/// evidence. Ordinary field names never contain this separator.
fn effect_name(name: &str) -> &str {
    name.split('\u{1f}').next().unwrap_or(name)
}

/// What one instruction does, with its operands.
fn operation(output: &Output, op: &Op) -> String {
    match op {
        Op::Const(value) => format!("const {value}n"),
        Op::ConstInt(value) => format!("const {value}i"),
        Op::ConstReal(value) => format!("const {value}"),
        Op::Struct(fields) if fields.is_empty() => "struct {}".to_string(),
        Op::Struct(fields) => {
            let entries: Vec<String> = fields
                .iter()
                .map(|(name, temp)| format!("{}: %{temp}", effect_name(name)))
                .collect();
            format!("struct {{ {} }}", entries.join(", "))
        }
        Op::Merge(records) => {
            let laid: Vec<String> = records.iter().map(|temp| format!("%{temp}")).collect();
            format!("merge {}", laid.join(", "))
        }
        Op::Project { base, field } => format!("project %{base}, {:?}", effect_name(field)),
        Op::Tag {
            name,
            payload: None,
        } => format!("tag #{name}"),
        Op::Tag {
            name,
            payload: Some(temp),
        } => format!("tag #{name}, %{temp}"),
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
        Op::Global { name, .. } => format!("global {name}"),
        Op::NewTag => "new_tag".to_string(),
        Op::Catch { tag, .. } => format!("catch %{tag}:"),
        Op::SwitchTag { on, .. } => format!("switch_tag %{on}:"),
        Op::SwitchNat { on, .. } => format!("switch_nat %{on}:"),
        Op::SwitchPresence { on, field, .. } => {
            format!("switch_presence %{on}, {:?}:", effect_name(field))
        }
        Op::SwitchRest { on, fields, .. } => {
            let names: Vec<String> = fields
                .iter()
                .map(|name| format!("{:?}", effect_name(name)))
                .collect();
            format!("switch_rest %{on}, [{}]:", names.join(", "))
        }
    }
}
