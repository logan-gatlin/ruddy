//! Rendering portable CPS control flow. Functions have explicit identities,
//! parameterized blocks, saved continuation environments and terminal transfers.
//! Source spans remain on instructions and transfers for debugger navigation.

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

/// Indentation between function, block and instruction rows.
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
    for (id, function) in output.functions.iter().enumerate() {
        if !first {
            out.push('\n');
        }
        first = false;
        let _ = writeln!(out, "{}", function_header(id, function));
        for (id, body) in function.blocks.iter().enumerate() {
            let _ = writeln!(out, "  {}", block_header(id, body));
            block(output, labels, body, STEP * 2, &mut out);
        }
    }
    for global in &output.globals {
        if !first {
            out.push('\n');
        }
        first = false;
        let _ = writeln!(out, "{}", header(global));
        let _ = writeln!(
            out,
            "  initializer f{} ({})",
            global.initializer, output.functions[global.initializer].name
        );
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
    format!(
        "fn {}({}; %{}: cont) entry b{}:",
        function.name,
        params.join(", "),
        function.continuation,
        function.entry
    )
}

/// A code-table identity and its independently compiled suspension summary.
pub fn function_header(id: usize, function: &Function) -> String {
    let signature = signature(function);
    format!(
        "{} [f{id}, {:?}]:",
        signature.trim_end_matches(':'),
        function.suspension
    )
}

/// A global's header line.
pub fn header(global: &Global) -> String {
    format!("global {}:", global.name)
}

/// One value instruction: its temporary, representation and operands.
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
        Op::Array(_) => "array",
        Op::Merge(_) => "merge",
        Op::Concat(_) => "concat",
        Op::Project { .. } => "project",
        Op::Tag { .. } => "tag",
        Op::Payload(_) => "payload",
        Op::Nth { .. } => "nth",
        Op::NthBack { .. } => "nth_back",
        Op::Slice { .. } => "slice",
        Op::Closure { .. } => "closure",
        Op::Continuation { .. } => "continuation",
        Op::Extern { .. } => "extern",
        Op::Global { .. } => "global",
        Op::Callback { .. } => "callback",
        Op::NewTag => "new_tag",
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
        Rep::Array => "array",
        Rep::Sum => "sum",
        Rep::Fn => "fn",
        Rep::Any => "any",
        Rep::Cont => "cont",
        Rep::Handler => "handler",
    }
}

/// A terminator, written bare — a block ends with one and it assigns nothing.
pub fn terminator(output: &Output, end: &Terminator) -> String {
    match &end.kind {
        End::Continue {
            continuation,
            value,
        } => format!("continue %{continuation}, %{value}"),
        End::Jump(e) => format!("jump {}", edge(e)),
        End::Branch { test, yes, no } => {
            let (kind, on, value) = match test {
                ruddy::lir::Test::Tag { on, name } => ("tag", on, super::label(Shape::Sum, name)),
                ruddy::lir::Test::Literal { on, value } => ("prim", on, literal(value)),
                ruddy::lir::Test::Presence { on, field } => ("presence", on, super::string(field)),
                ruddy::lir::Test::Rest { on, fields } => ("rest", on, format!("{fields:?}")),
                ruddy::lir::Test::Length { on, length } => ("len", on, length.to_string()),
            };
            format!(
                "branch_{kind} %{on}, {value} => {}, otherwise {}",
                edge(yes),
                edge(no)
            )
        }
        End::Call {
            callee,
            args,
            continuation,
        } => format!(
            "call {}, {} -> %{continuation}{}",
            match callee {
                Callee::Direct(f) => output.functions[*f].name.clone(),
                Callee::Indirect(t) => format!("%{t}"),
            },
            temps(args),
            match callee {
                Callee::Direct(f) => format!(" [f{f}]"),
                _ => String::new(),
            }
        ),
        End::RawCall {
            callee,
            args,
            continuation,
            completion,
        } => format!(
            "raw_call {completion:?} %{callee}, {} -> %{continuation}",
            temps(args)
        ),
        End::Enter {
            tag,
            body,
            continuation,
        } => format!("enter %{tag}, {}, %{continuation}", edge(body)),
        End::Leave { tag, value } => format!("leave %{tag}, %{value}"),
        End::Abort { tag, value } => format!("abort %{tag}, %{value}"),
        End::Unreachable => "unreachable".into(),
    }
}
fn temps(values: &[u32]) -> String {
    values
        .iter()
        .map(|v| format!("%{v}"))
        .collect::<Vec<_>>()
        .join(", ")
}
fn edge(e: &ruddy::lir::Edge) -> String {
    format!("b{}({})", e.block, temps(&e.args))
}
pub fn block_header(id: usize, b: &Block) -> String {
    format!(
        "b{id}({}){}:",
        b.params
            .iter()
            .map(|p| format!("%{}: {}", p.temp, rep(p.rep)))
            .collect::<Vec<_>>()
            .join(", "),
        if b.result.is_some() {
            " continuation"
        } else {
            ""
        }
    )
}
pub fn end_label(end: &Terminator) -> &'static str {
    match end.kind {
        End::Continue { .. } => "continue",
        End::Jump(_) => "jump",
        End::Branch { .. } => "branch",
        End::Call { .. } => "call",
        End::RawCall { .. } => "raw_call",
        End::Enter { .. } => "enter",
        End::Leave { .. } => "leave",
        End::Abort { .. } => "abort",
        End::Unreachable => "unreachable",
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
    }
    let _ = writeln!(
        out,
        "{:indent$}{}",
        "",
        terminator(output, &block.end),
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
        Op::Array(values) => {
            let values: Vec<String> = values.iter().map(|temp| format!("%{temp}")).collect();
            format!("array [{}]", values.join(", "))
        }
        Op::Merge(records) => {
            let laid: Vec<String> = records.iter().map(|temp| format!("%{temp}")).collect();
            format!("merge {}", laid.join(", "))
        }
        Op::Concat(arrays) => {
            let joined: Vec<String> = arrays.iter().map(|temp| format!("%{temp}")).collect();
            format!("concat {}", joined.join(", "))
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
        Op::Nth { base, index } => format!("nth %{base}, {index}"),
        Op::NthBack { base, index } => format!("nth_back %{base}, {index}"),
        Op::Slice { base, start, drop } => format!("slice %{base}, {start}, {drop}"),
        Op::Closure { func, captures } => {
            let held: Vec<String> = captures.iter().map(|temp| format!("%{temp}")).collect();
            format!(
                "closure {}, [{}]",
                output.functions[*func].name,
                held.join(", ")
            )
        }
        Op::Continuation { code, captures } => format!(
            "continuation {}:b{}, [{}] [f{}]",
            output.functions[code.function].name,
            code.block,
            temps(captures),
            code.function
        ),
        Op::Extern { name, .. } => format!("extern {name}"),
        Op::Global { name, .. } => format!("global {name}"),
        Op::Callback { value, mode } => format!("callback {mode:?} %{}", value),
        Op::NewTag => "new_tag".to_string(),
    }
}
