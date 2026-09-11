//! Deterministic JavaScript generation for linked Ruddy artifacts.
//!
//! This backend deliberately performs no I/O. Drivers compile and link an artifact,
//! call [`generate`], and decide where (or whether) to install the resulting ESM.

use crate::artifact::{
    Artifact, Block, Callee, Edge, End, FieldKey, Instr, Literal, Op, Rep, Test,
};
use esparse::Item;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    error, fmt,
};

/// A JavaScript generation failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    Entry(crate::entry::Error),
    Export {
        name: String,
        message: String,
    },
    ExportType {
        name: String,
        message: String,
    },
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
    /// The complete generated ECMAScript module failed validation.
    InvalidJavaScript(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Entry(error) => error.fmt(f),
            Self::Export { name, message } | Self::ExportType { name, message } => {
                write!(f, "export `{name}` is not callable by the host: {message}")
            }
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
            Self::InvalidJavaScript(diagnostic) => {
                write!(f, "generated JavaScript is invalid: {diagnostic}")
            }
        }
    }
}

impl error::Error for Error {}

#[derive(Default)]
struct ExportNode {
    descriptor: Option<crate::backend::host::NativeTemplate>,
    adapter: Option<crate::externs::Callback>,
    value: Option<String>,
    children: BTreeMap<String, ExportNode>,
}

/// Platform handlers available to host calls into the root bundle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Platform {
    #[default]
    Node,
    Web,
}

/// Generate a complete modern ECMAScript module from a linked artifact.
pub fn generate(artifact: &Artifact) -> Result<String, Error> {
    generate_for_platform(artifact, Platform::Node)
}

/// Generate root host exports using only the selected platform's handlers.
pub fn generate_for_platform(artifact: &Artifact, platform: Platform) -> Result<String, Error> {
    if !artifact.header().dependencies.is_empty() {
        return Err(Error::Unlinked);
    }
    let mut prepared = None;
    let mut entry = None;
    if artifact.header().kind == crate::artifact::Kind::Executable {
        if platform == Platform::Web {
            return Err(Error::Export {
                name: "main".into(),
                message: "web executables are not supported yet".into(),
            });
        }
        let adapter = entry_adapter(artifact)?;
        entry = adapter
            .header()
            .values
            .iter()
            .find(|value| value.name.ends_with("::invoke"))
            .map(|value| value.name.clone());
        let mut library = artifact.clone();
        library.header.kind = crate::artifact::Kind::Library;
        let mut linked =
            crate::link::link(&[library, adapter]).expect("entry adapter links to its root");
        linked.header = artifact.header().clone();
        prepared = Some(linked);
    }
    let mut exports = HashMap::new();
    let root = prepared.as_ref().unwrap_or(artifact);
    if let Some(host) = super::host::compile(root, &[], platform)? {
        let mut library = root.clone();
        library.header.kind = crate::artifact::Kind::Library;
        let mut linked =
            crate::link::link(&[library, host.artifact]).expect("host adapter links to its root");
        linked.header = artifact.header().clone();
        exports = host.exports;
        prepared = Some(linked);
    }
    Generator::new(prepared.as_ref().unwrap_or(artifact), &exports, platform)?
        .generate_with_entry(entry.as_deref())
}

/// Validate only the root's public host interface; dependencies retain effects.
pub fn check_exports(
    artifact: &Artifact,
    dependencies: &[&Artifact],
    platform: Platform,
) -> Result<(), Error> {
    super::host::compile(artifact, dependencies, platform).map(|_| ())
}

/// Check Node's root effect contract without emitting JavaScript.
pub fn check_entry(artifact: &Artifact, dependencies: &[&Artifact]) -> Result<(), Error> {
    if artifact.header().kind == crate::artifact::Kind::Executable {
        crate::entry::wrapper(artifact, dependencies, NODE_ENTRY).map_err(Error::Entry)?;
    }
    Ok(())
}

fn entry_adapter(artifact: &Artifact) -> Result<Artifact, Error> {
    crate::entry::wrapper(artifact, &[], NODE_ENTRY).map_err(Error::Entry)
}

// The platform ABI is structural. These declarations describe the runtime's
// supported interfaces independently of a project's choice of std bundle.
const NODE_ENTRY: &str = concat!(
    include_str!("node-platform.rud"),
    include_str!("web-platform.rud"),
    "let with_platform: (() -> 'a + !IO + !Exit + !FileSystem + !Process + !Path + !Http + !Host + !Immediate) -> 'a + !Immediate = fn body => handle body () with\n",
    include_str!("node-handler.rud"),
    include_str!("web-handler.rud"),
    "end\n",
    include_str!("node-entry.rud")
);

struct Generator<'a> {
    artifact: &'a Artifact,
    runtime_names: HashSet<&'a str>,
    extern_names: HashSet<&'a str>,
    exports: ExportNode,
    platform_exports: bool,
    platform: Platform,
}

impl<'a> Generator<'a> {
    fn new(
        artifact: &'a Artifact,
        host_exports: &HashMap<String, String>,
        platform: Platform,
    ) -> Result<Self, Error> {
        if !artifact.header().dependencies.is_empty() {
            return Err(Error::Unlinked);
        }
        let mut runtime_names = HashSet::new();
        for external in &artifact.lir().externs {
            runtime_names.insert(external.name.as_str());
        }
        let extern_names = artifact
            .lir
            .externs
            .iter()
            .map(|external| external.name.as_str())
            .collect();
        for global in &artifact.lir().globals {
            if artifact
                .lir
                .globals
                .iter()
                .filter(|candidate| candidate.name == global.name)
                .count()
                > 1
                || !runtime_names.insert(global.name.as_str())
                    && !artifact
                        .lir
                        .externs
                        .iter()
                        .any(|external| external.name == global.name)
            {
                return Err(Error::InvalidGlobalReference(global.name.clone()));
            }
        }

        let prefix = format!(
            "{}@{}::",
            artifact.header().identity.name,
            artifact.header().identity.version
        );
        let mut exports = ExportNode::default();
        let declarations = artifact
            .header()
            .types
            .iter()
            .map(|ty| (ty.name.as_str(), ty))
            .collect();
        for value in artifact
            .header()
            .values
            .iter()
            .filter(|_| artifact.header().kind != crate::artifact::Kind::Executable)
        {
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
            insert_export(
                &mut exports,
                &path,
                host_exports.get(&value.name).unwrap_or(&value.name),
                super::host::native_descriptor(&value.scheme.body, &declarations).map_err(
                    |message| Error::ExportType {
                        name: value.name.clone(),
                        message,
                    },
                )?,
                artifact
                    .lir()
                    .globals
                    .iter()
                    .find(|g| g.name == value.name)
                    .and_then(|g| g.adapter),
            )?;
        }

        let generator = Self {
            artifact,
            runtime_names,
            extern_names,
            exports,
            platform_exports: !host_exports.is_empty() && platform == Platform::Node,
            platform,
        };
        generator.validate()?;
        Ok(generator)
    }

    fn validate(&self) -> Result<(), Error> {
        for external in &self.artifact.lir().externs {
            validate_extern_target(&external.target)?;
        }

        for block in self.artifact.lir().functions.iter().flat_map(|f| &f.blocks) {
            for instr in &block.instrs {
                match &instr.op {
                    Op::Closure { func, .. }
                        if *func >= self.artifact.lir().functions.len() as u64 =>
                    {
                        return Err(Error::InvalidFunctionReference(*func));
                    }
                    Op::Extern { target } if !self.extern_names.contains(target.as_str()) => {
                        return Err(Error::InvalidGlobalReference(target.clone()));
                    }
                    Op::Global { target, .. } if !self.runtime_names.contains(target.as_str()) => {
                        return Err(Error::InvalidGlobalReference(target.clone()));
                    }
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn generate_with_entry(&self, entry: Option<&str>) -> Result<String, Error> {
        let mut out = String::new();
        out.push_str("// Generated by Ruddy.\n");
        out.push_str(RUNTIME);
        out.push_str(CPS_RUNTIME);
        // The domains the program bound `Nat` and `Int` to, for conversion
        // at the foreign boundary and for what a mirror describes.
        let domains = self.artifact.header().domains;
        // `min` and `max` are the exact bounds as text, which is what a
        // mirror describes. `low` and `high` are the numbers a check
        // compares against, so they are clamped to what this target holds
        // exactly: a domain wider than the safe integers cannot be checked
        // numerically here, and rounding one outwards would admit a value
        // outside it.
        let bounds = |bounds: crate::types::Bounds| {
            let safe = 9007199254740991i128;
            let clamp = |text: &str| {
                text.parse::<i128>()
                    .unwrap_or(0)
                    .clamp(-safe, safe)
                    .to_string()
            };
            format!(
                "{{ bits: {}, signed: {}, min: {:?}, max: {:?}, low: {}, high: {} }}",
                bounds.bits,
                bounds.signed,
                bounds.min,
                bounds.max,
                clamp(bounds.min),
                clamp(bounds.max)
            )
        };
        out.push_str(&format!(
            "const $domains = {{ nat: {}, int: {} }};\n",
            bounds(domains.nat()),
            bounds(domains.int())
        ));
        out.push_str(include_str!("type-runtime.js"));
        out.push_str(include_str!("primitives.js"));
        out.push_str(include_str!("web-apis.js"));
        if self.platform == Platform::Node {
            out.push_str(include_str!("node-apis.js"));
        }
        if entry.is_some() || self.platform_exports {
            out.push_str(include_str!("node-fs.js"));
        }
        out.push_str("const $f = [\n");
        for (id, function) in self.artifact.lir().functions.iter().enumerate() {
            out.push_str(&format!(
                "{{ arity: {}, suspends: {}, entry: ($ctx",
                function.params.len(),
                function.suspension == crate::lir::Suspension::MaySuspend
            ));
            for p in &function.params {
                out.push_str(", ");
                out.push_str(&temp(p.temp));
            }
            out.push_str(", ");
            out.push_str(&temp(function.continuation));
            out.push_str(") => ");
            let entry = Edge {
                block: function.entry,
                args: function.blocks[function.entry as usize]
                    .params
                    .iter()
                    .map(|p| p.temp)
                    .collect(),
            };
            emit_edge(id, &entry, &mut out);
            out.push_str(", blocks: [\n");
            for block in &function.blocks {
                self.block(id, block, &mut out)?;
                out.push_str(",\n");
            }
            out.push_str("] },\n");
        }
        out.push_str("];\nconst $h = Object.create(null);\nconst $g = Object.create(null);\n");
        for external in &self.artifact.lir().externs {
            out.push_str("$h[");
            string(&external.name, &mut out);
            out.push_str("] = (\n");
            out.push_str(&external.target);
            out.push_str("\n);\n$g[");
            string(&external.name, &mut out);
            out.push_str("] = $h[");
            string(&external.name, &mut out);
            out.push_str("];\n");
        }
        out.push_str("const $initializers = [");
        for global in &self.artifact.lir().globals {
            out.push('[');
            string(&global.name, &mut out);
            out.push_str(", ");
            out.push_str(&global.initializer.to_string());
            out.push_str("],");
        }
        out.push_str("];\nconst $ready = $initialize($initializers);\nif ($ready) await $ready;\n");

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
        if let Some(entry) = entry {
            out.push_str("const $entryResult = $start($g[");
            string(entry, &mut out);
            out.push_str("], [$record([])], null);\nif ($entryResult.pending) await $promise($entryResult);\nif ($entryResult.failed) throw $entryResult.error;\nconst $exitCode = Math.min(255, $entryResult.value);\nlet $pendingWrites = 2;\nconst $drained = () => { if (--$pendingWrites === 0) process.exit($exitCode); };\nprocess.stdout.write(\"\", $drained);\nprocess.stderr.write(\"\", $drained);\n");
        }
        validate_module(&out)?;
        Ok(out)
    }

    fn block(&self, function: usize, block: &Block, out: &mut String) -> Result<(), Error> {
        out.push_str("($ctx");
        for p in &block.params {
            out.push_str(", ");
            out.push_str(&temp(p.temp));
        }
        out.push_str(") => {\n");
        for i in &block.instrs {
            out.push_str("const ");
            out.push_str(&temp(i.temp));
            out.push_str(" = ");
            self.op(i, out)?;
            out.push_str(";\n");
        }
        out.push_str("return ");
        emit_end(function, &block.end, out);
        out.push_str(";\n}");
        Ok(())
    }

    fn op(&self, instr: &Instr, out: &mut String) -> Result<(), Error> {
        let v = |n| temp(n);
        match &instr.op {
            Op::Callback { value, mode } => {
                out.push_str("$callback(");
                out.push_str(&v(*value));
                out.push_str(", ");
                string(callback_protocol(*mode), out);
                out.push(')');
            }
            Op::Const(lit) => literal(lit, out),
            Op::Convert {
                descriptor,
                value,
                direction,
            } => {
                out.push_str("$convertType(");
                out.push_str(&v(*descriptor));
                out.push_str(", ");
                out.push_str(&v(*value));
                out.push_str(match direction {
                    crate::backend::host::Direction::ToJs => ", true)",
                    crate::backend::host::Direction::FromJs => ", false)",
                });
            }
            Op::TypeProjection { descriptor, path } => {
                out.push_str("$projectType(");
                out.push_str(&v(*descriptor));
                out.push_str(", ");
                out.push_str(&serde_json::to_string(path).expect("descriptor path JSON"));
                out.push(')');
            }
            Op::TypeDescriptor {
                template,
                arguments,
            } => {
                out.push_str("$instantiateType(");
                out.push_str(
                    &serde_json::to_string(template).expect("a descriptor is serializable"),
                );
                out.push_str(", [");
                for (index, argument) in arguments.iter().enumerate() {
                    if index != 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&v(*argument));
                }
                out.push_str("])");
            }
            Op::NativePlan {
                template,
                arguments,
            } => {
                out.push_str("$nativePlan(");
                out.push_str(
                    &serde_json::to_string(template).expect("a descriptor is serializable"),
                );
                out.push_str(", [");
                for (index, argument) in arguments.iter().enumerate() {
                    if index != 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&v(*argument));
                }
                out.push_str("])");
            }
            Op::Reflect {
                kind,
                descriptor,
                value,
            } => {
                out.push_str(match kind {
                    crate::reification::Intrinsic::Decode => "$ffiDecode(",
                    crate::reification::Intrinsic::Encode => "$ffiEncode(",
                    crate::reification::Intrinsic::Mirror => "$mirror(",
                    crate::reification::Intrinsic::TypeOf => "$typeOf(",
                    crate::reification::Intrinsic::Describe => "$describe(",
                    crate::reification::Intrinsic::Same => "$sameMirror(",
                    crate::reification::Intrinsic::Shape => "$shape(",
                });
                out.push_str(&v(*descriptor));
                out.push_str(", ");
                out.push_str(&v(*value));
                out.push(')');
            }
            Op::Neg(a) => {
                out.push_str("(-");
                out.push_str(&v(*a));
                out.push(')');
            }
            Op::Allocate(a) => {
                out.push_str("({ value: ");
                out.push_str(&v(*a));
                out.push_str(" })");
            }
            Op::Read(a) => {
                out.push_str(&v(*a));
                out.push_str(".value");
            }
            Op::Write { left, right } => {
                binary(out, &format!("{}.value", v(*left)), "=", &v(*right));
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
            Op::Array(values) => {
                out.push_str("$array([");
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&v(*value));
                }
                out.push_str("])");
            }
            Op::Concat(values) => {
                out.push_str("$arrayJoin([");
                for (index, value) in values.iter().enumerate() {
                    if index != 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&v(*value));
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
            Op::Nth { base, index } => {
                out.push_str("$arrayNth(");
                out.push_str(&v(*base));
                out.push_str(", ");
                out.push_str(&index.to_string());
                out.push(')');
            }
            Op::NthBack { base, index } => {
                out.push_str("$arrayNth(");
                out.push_str(&v(*base));
                out.push_str(", ");
                out.push_str(&v(*base));
                out.push_str(".size - ");
                out.push_str(&(index + 1).to_string());
                out.push(')');
            }
            Op::Slice { base, start, drop } => {
                out.push_str("$arrayCut(");
                out.push_str(&v(*base));
                out.push_str(", ");
                out.push_str(&start.to_string());
                out.push_str(", ");
                out.push_str(&drop.to_string());
                out.push(')');
            }
            Op::Closure { func, captures } => {
                out.push_str("$closure(");
                out.push_str(&func.to_string());
                out.push_str(", [");
                comma_temps(captures, out);
                out.push_str("])");
            }
            Op::Continuation { code, captures } => {
                out.push_str("$continuation(");
                out.push_str(&code.function.to_string());
                out.push_str(", ");
                out.push_str(&code.block.to_string());
                out.push_str(", [");
                comma_temps(captures, out);
                out.push_str("], $ctx)");
            }
            Op::Extern { target } => {
                out.push_str("$h[");
                string(target, out);
                out.push(']');
            }
            Op::Global { target, .. } => {
                out.push_str("$g[");
                string(target, out);
                out.push(']');
            }
            Op::NewTag => out.push_str("({ active: false })"),
        }
        Ok(())
    }
}

fn emit_edge(function: usize, edge: &Edge, out: &mut String) {
    out.push_str("$step(");
    out.push_str(&function.to_string());
    out.push_str(", ");
    out.push_str(&edge.block.to_string());
    out.push_str(", [");
    comma_temps(&edge.args, out);
    out.push_str("], $ctx)");
}
fn emit_end(function: usize, end: &End, out: &mut String) {
    match end {
        End::Continue {
            continuation,
            value,
        } => {
            out.push_str("$resume(");
            out.push_str(&temp(*continuation));
            out.push_str(", ");
            out.push_str(&temp(*value));
            out.push(')');
        }
        End::Jump(edge) => emit_edge(function, edge, out),
        End::Branch { test, yes, no } => {
            out.push('(');
            emit_test(test, out);
            out.push_str(" ? ");
            emit_edge(function, yes, out);
            out.push_str(" : ");
            emit_edge(function, no, out);
            out.push(')');
        }
        End::Call {
            callee,
            args,
            continuation,
        } => {
            out.push_str("$call(");
            match callee {
                Callee::Direct(f) => out.push_str(&format!("$closure({f}, [])")),
                Callee::Indirect(t) => out.push_str(&temp(*t)),
            };
            out.push_str(", [");
            comma_temps(args, out);
            out.push_str("], ");
            out.push_str(&temp(*continuation));
            out.push_str(", $ctx)");
        }
        End::RawCall {
            callee,
            args,
            continuation,
            completion,
        } => {
            out.push_str("$raw(");
            out.push_str(&temp(*callee));
            out.push_str(", [");
            comma_temps(args, out);
            out.push_str("], ");
            out.push_str(&temp(*continuation));
            out.push_str(", $ctx, ");
            string(completion_protocol(*completion), out);
            out.push(')');
        }
        End::Enter {
            tag,
            body,
            continuation,
        } => {
            out.push_str("$enter(");
            out.push_str(&temp(*tag));
            out.push_str(", ");
            out.push_str(&temp(*continuation));
            out.push_str(", ");
            emit_edge(function, body, out);
            out.push(')');
        }
        End::Leave { tag, value } | End::Abort { tag, value } => {
            out.push_str("$leave(");
            out.push_str(&temp(*tag));
            out.push_str(", ");
            out.push_str(&temp(*value));
            out.push_str(", $ctx)");
        }
        End::Unreachable => out.push_str("$unreachable()"),
    }
}
fn emit_test(test: &Test, out: &mut String) {
    match test {
        Test::Tag { on, name } => {
            out.push_str(&temp(*on));
            out.push_str("[$tag] === ");
            string(name, out);
        }
        Test::Literal {
            on,
            value: value @ Literal::Real(_),
        } => {
            out.push_str("$same(");
            out.push_str(&temp(*on));
            out.push_str(", ");
            literal(value, out);
            out.push(')');
        }
        Test::Literal { on, value } => {
            out.push_str(&temp(*on));
            out.push_str(" === ");
            literal(value, out);
        }
        Test::Presence { on, field } => {
            out.push_str("$own(");
            out.push_str(&temp(*on));
            out.push_str(", ");
            string(field, out);
            out.push(')');
        }
        Test::Rest { on, fields } => {
            out.push_str("!$hasRest(");
            out.push_str(&temp(*on));
            out.push_str(", [");
            for (i, f) in fields.iter().enumerate() {
                if i != 0 {
                    out.push(',');
                }
                string(f, out);
            }
            out.push_str("])");
        }
        Test::Length { on, length } => {
            out.push_str(&temp(*on));
            out.push_str(".size === ");
            out.push_str(&length.to_string());
        }
    }
}
const CPS_RUNTIME: &str = include_str!("cps-runtime.js");

fn insert_export(
    node: &mut ExportNode,
    path: &[&str],
    qualified: &str,
    descriptor: crate::backend::host::NativeTemplate,
    adapter: Option<crate::externs::Callback>,
) -> Result<(), Error> {
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
    at.adapter = adapter;
    at.descriptor = Some(descriptor);
    Ok(())
}

fn emit_export_value(node: &ExportNode, binding: &str, out: &mut String) {
    out.push_str("const ");
    out.push_str(binding);
    out.push_str(" = ");
    emit_export_expression(node, out);
    out.push_str(";\n");
}

fn emit_export_expression(node: &ExportNode, out: &mut String) {
    if let Some(value) = &node.value {
        out.push_str("$convertType($nativePlan(");
        out.push_str(
            &serde_json::to_string(node.descriptor.as_ref().expect("reviewed native export"))
                .expect("descriptor JSON"),
        );
        out.push_str(", []), ");
        out.push_str(if node.adapter.is_some() {
            "$callback($g["
        } else {
            "$export($g["
        });
        string(value, out);
        out.push(']');
        if let Some(mode) = node.adapter {
            out.push_str(", ");
            string(callback_protocol(mode), out);
            out.push_str(", false");
        }
        out.push_str("), true, 0, true, true)");
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
const $dv = new DataView(new ArrayBuffer(8));
const $real = $bits => { $dv.setBigUint64(0, BigInt("0x" + $bits)); return $dv.getFloat64(0); };
const $record = $entries => { const $o = Object.create(null); for (const [$k, $v] of $entries) $o[$k] = $v; return $o; };
const $sum = ($name, $value) => { const $o = Object.create(null); $o[$tag] = $name; $o[$payload] = $value; return $o; };
// ── Arrays ──────────────────────────────────────────────────────────────────
// A relaxed-radix-balanced tree after Bagwell and Rompf (2011). A vector is
// `{ size, shift, root, tail }`: `tail` holds the last one to thirty-two
// elements as a plain frozen array, and `root` is a node at level `shift`
// holding everything before them. A node is `{ items, sizes }`: `items` its
// children — nodes, or at level five the leaves, which are frozen arrays of
// values — and `sizes` the cumulative count of elements under each child, so
// that a child need not be full for the one holding an index to be found.
// Concatenation rebalances to at most two extra search steps per level, which
// is what keeps every walk logarithmic.
const $B = 32;
const $BITS = 5;
const $EXTRA = 2;
const $node = ($items, $sizes) => Object.freeze({ items: Object.freeze($items), sizes: Object.freeze($sizes) });
const $emptyNode = $node([], []);
const $vector = ($size, $shift, $root, $tail) => Object.freeze({ size: $size, shift: $shift, root: $root, tail: Object.freeze($tail) });
const $emptyArray = $vector(0, $BITS, $emptyNode, []);
const $count = $n => $n.sizes.length === 0 ? 0 : $n.sizes[$n.sizes.length - 1];
const $slots = ($child, $level) => $level === 0 ? $child.length : $child.items.length;
const $below = ($child, $level) => $level === 0 ? $child.length : $count($child);
// A node at `level` over these children, its size table computed.
const $mk = ($items, $level) => {
  const $sizes = [];
  let $sum = 0;
  for (const $child of $items) { $sum += $below($child, $level - $BITS); $sizes.push($sum); }
  return $node($items, $sizes);
};
// Which child of a node at `level` holds relative index `i`, and how many
// elements sit before that child. The radix guess is never past the answer,
// since no child holds more than its full share.
const $find = ($n, $level, $i) => {
  let $idx = Math.floor($i / 2 ** $level);
  while ($n.sizes[$idx] <= $i) $idx += 1;
  return [$idx, $idx === 0 ? 0 : $n.sizes[$idx - 1]];
};
const $arrayNth = ($a, $i) => {
  const $off = $a.size - $a.tail.length;
  if ($i >= $off) return $a.tail[$i - $off];
  let $n = $a.root;
  for (let $level = $a.shift; $level > 0; $level -= $BITS) {
    const [$idx, $before] = $find($n, $level, $i);
    $n = $n.items[$idx];
    $i -= $before;
  }
  return $n[$i];
};
const $assoc = ($n, $level, $i, $v) => {
  if ($level === 0) { const $leaf = $n.slice(); $leaf[$i] = $v; return Object.freeze($leaf); }
  const [$idx, $before] = $find($n, $level, $i);
  const $items = $n.items.slice();
  $items[$idx] = $assoc($n.items[$idx], $level - $BITS, $i - $before, $v);
  return $node($items, $n.sizes);
};
const $path = ($level, $leaf) => $level === 0 ? $leaf : $mk([$path($level - $BITS, $leaf)], $level);
// A leaf appended under a node at `level`, or null where there is no room.
const $pushLeaf = ($n, $level, $leaf) => {
  const $len = $n.items.length;
  if ($level > $BITS && $len > 0) {
    const $last = $pushLeaf($n.items[$len - 1], $level - $BITS, $leaf);
    if ($last !== null) { const $items = $n.items.slice(); $items[$len - 1] = $last; return $mk($items, $level); }
  }
  if ($len >= $B) return null;
  return $mk([...$n.items, $path($level - $BITS, $leaf)], $level);
};
// The tree with a leaf appended, grown a level where the root is full.
const $withLeaf = ($root, $shift, $leaf) => {
  const $grown = $pushLeaf($root, $shift, $leaf);
  if ($grown !== null) return [$grown, $shift];
  return [$mk([$root, $path($shift, $leaf)], $shift + $BITS), $shift + $BITS];
};
// The rightmost leaf taken out of a tree: the tree without it, and the leaf.
const $popLeaf = ($n, $level) => {
  const $len = $n.items.length;
  if ($level === $BITS) return [$mk($n.items.slice(0, -1), $level), $n.items[$len - 1]];
  const [$child, $leaf] = $popLeaf($n.items[$len - 1], $level - $BITS);
  const $items = $child.items.length === 0 ? $n.items.slice(0, -1) : [...$n.items.slice(0, -1), $child];
  return [$mk($items, $level), $leaf];
};
// A root with one child is that child, a level down; an empty one is empty.
const $shrink = ($root, $shift) => {
  while ($shift > $BITS && $root.items.length === 1) { $root = $root.items[0]; $shift -= $BITS; }
  return $root.items.length === 0 ? [$emptyNode, $BITS] : [$root, $shift];
};
// The first `to` elements under a node, `to` being at least one.
const $cut = ($n, $level, $to) => {
  if ($level === 0) return Object.freeze($n.slice(0, $to));
  const [$idx, $before] = $find($n, $level, $to - 1);
  return $mk([...$n.items.slice(0, $idx), $cut($n.items[$idx], $level - $BITS, $to - $before)], $level);
};
// Everything after the first `from` elements under a node, `from` being less
// than the count.
const $chop = ($n, $level, $from) => {
  if ($level === 0) return Object.freeze($n.slice($from));
  const [$idx, $before] = $find($n, $level, $from);
  const $child = $from === $before ? $n.items[$idx] : $chop($n.items[$idx], $level - $BITS, $from - $before);
  return $mk([$child, ...$n.items.slice($idx + 1)], $level);
};
const $take = ($a, $to) => {
  if ($to >= $a.size) return $a;
  if ($to === 0) return $emptyArray;
  const $off = $a.size - $a.tail.length;
  if ($to > $off) return $vector($to, $a.shift, $a.root, $a.tail.slice(0, $to - $off));
  const [$root, $leaf] = $popLeaf($cut($a.root, $a.shift, $to), $a.shift);
  const [$shrunk, $shift] = $shrink($root, $a.shift);
  return $vector($to, $shift, $shrunk, $leaf);
};
const $drop = ($a, $from) => {
  if ($from === 0) return $a;
  if ($from >= $a.size) return $emptyArray;
  const $off = $a.size - $a.tail.length;
  if ($from >= $off) return $vector($a.size - $from, $BITS, $emptyNode, $a.tail.slice($from - $off));
  const [$root, $shift] = $shrink($chop($a.root, $a.shift, $from), $a.shift);
  return $vector($a.size - $from, $shift, $root, $a.tail);
};
const $arrayCut = ($a, $from, $dropBack) => $drop($take($a, $a.size - $dropBack), $from);
const $arrayValues = $a => {
  const $out = [];
  const $walk = ($n, $level) => {
    if ($level === 0) { for (const $v of $n) $out.push($v); return; }
    for (const $child of $n.items) $walk($child, $level - $BITS);
  };
  $walk($a.root, $a.shift);
  for (const $v of $a.tail) $out.push($v);
  return $out;
};
// The children of two nodes redistributed so that no more than `$EXTRA`
// children beyond the fewest that could hold their contents remain — the
// invariant that bounds the search steps at a level — returned as a node one
// level up holding the one or two nodes they fill.
const $rebalance = ($l, $mid, $r, $level) => {
  const $all = [
    ...($l === null ? [] : $mid === null ? $l.items : $l.items.slice(0, -1)),
    ...($mid === null ? [] : $mid.items),
    ...($r === null ? [] : $mid === null ? $r.items : $r.items.slice(1)),
  ];
  const $sub = $level - $BITS;
  const $plan = $all.map($child => $slots($child, $sub));
  let $n = $plan.length;
  const $total = $plan.reduce(($x, $y) => $x + $y, 0);
  const $optimal = Math.ceil($total / $B);
  let $i = 0;
  while ($n > $optimal + $EXTRA) {
    while ($plan[$i] >= $B) $i += 1;
    let $remaining = $plan[$i];
    let $j = $i;
    while ($remaining > 0) {
      const $fill = Math.min($remaining + $plan[$j + 1], $B);
      $plan[$j] = $fill;
      $remaining = $remaining + $plan[$j + 1] - $fill;
      $j += 1;
    }
    for (let $k = $j; $k < $n - 1; $k += 1) $plan[$k] = $plan[$k + 1];
    $n -= 1;
    $i = $j - 1;
  }
  const $merged = [];
  let $src = 0;
  let $off = 0;
  for (let $k = 0; $k < $n; $k += 1) {
    const $want = $plan[$k];
    if ($off === 0 && $slots($all[$src], $sub) === $want) { $merged.push($all[$src]); $src += 1; continue; }
    const $grand = [];
    while ($grand.length < $want) {
      const $from = $sub === 0 ? $all[$src] : $all[$src].items;
      const $taken = Math.min($want - $grand.length, $from.length - $off);
      for (let $t = 0; $t < $taken; $t += 1) $grand.push($from[$off + $t]);
      $off += $taken;
      if ($off === $from.length) { $src += 1; $off = 0; }
    }
    $merged.push($sub === 0 ? Object.freeze($grand) : $mk($grand, $sub));
  }
  const $nodes = $merged.length <= $B ? [$mk($merged, $level)] : [$mk($merged.slice(0, $B), $level), $mk($merged.slice($B), $level)];
  return $mk($nodes, $level + $BITS);
};
// Two trees joined, as a node one level above the taller holding the one or
// two nodes the join fills.
const $mergeAt = ($l, $ls, $r, $rs) => {
  if ($ls > $rs) return $rebalance($l, $mergeAt($l.items[$l.items.length - 1], $ls - $BITS, $r, $rs), null, $ls);
  if ($ls < $rs) return $rebalance(null, $mergeAt($l, $ls, $r.items[0], $rs - $BITS), $r, $rs);
  if ($ls === $BITS) return $rebalance($l, null, $r, $ls);
  return $rebalance($l, $mergeAt($l.items[$l.items.length - 1], $ls - $BITS, $r.items[0], $rs - $BITS), $r, $ls);
};
const $join = ($a, $b) => {
  if ($a.size === 0) return $b;
  if ($b.size === 0) return $a;
  if ($b.size <= $B) { let $out = $a; for (const $v of $arrayValues($b)) $out = $push($out, $v); return $out; }
  const [$lroot, $lshift] = $withLeaf($a.root, $a.shift, $a.tail);
  const [$rroot, $rshift] = $withLeaf($b.root, $b.shift, $b.tail);
  const $top = $mergeAt($lroot, $lshift, $rroot, $rshift);
  const $shift = Math.max($lshift, $rshift) + $BITS;
  const [$root, $leaf] = $popLeaf($top, $shift);
  const [$shrunk, $final] = $shrink($root, $shift);
  return $vector($a.size + $b.size, $final, $shrunk, $leaf);
};
const $push = ($a, $v) => {
  if ($a.tail.length < $B) return $vector($a.size + 1, $a.shift, $a.root, [...$a.tail, $v]);
  const [$root, $shift] = $withLeaf($a.root, $a.shift, $a.tail);
  return $vector($a.size + 1, $shift, $root, [$v]);
};
const $array = $values => {
  const $size = $values.length;
  if ($size === 0) return $emptyArray;
  const $tailStart = Math.floor(($size - 1) / $B) * $B;
  const $tail = $values.slice($tailStart);
  let $level = [];
  for (let $i = 0; $i < $tailStart; $i += $B) $level.push(Object.freeze($values.slice($i, $i + $B)));
  let $shift = $BITS;
  if ($level.length === 0) return $vector($size, $shift, $emptyNode, $tail);
  for (;;) {
    const $next = [];
    for (let $i = 0; $i < $level.length; $i += $B) $next.push($mk($level.slice($i, $i + $B), $shift));
    if ($next.length === 1) return $vector($size, $shift, $next[0], $tail);
    $level = $next;
    $shift += $BITS;
  }
};
const $arrayJoin = $pieces => $pieces.reduce($join, $emptyArray);
const $arrayLen = $a => $a.size;
const $arrayGet = $a => $i => $i < $a.size
  ? $sum("Some", $arrayNth($a, $i))
  : $sum("None", undefined);
const $arraySet = $a => $i => $v => {
  if ($i >= $a.size) return $sum("None", undefined);
  const $off = $a.size - $a.tail.length;
  if ($i >= $off) { const $tail = $a.tail.slice(); $tail[$i - $off] = $v; return $sum("Some", $vector($a.size, $a.shift, $a.root, $tail)); }
  return $sum("Some", $vector($a.size, $a.shift, $assoc($a.root, $a.shift, $i, $v), $a.tail));
};
const $arrayPush = $a => $v => $push($a, $v);
const $arrayConcat = $a => $b => $join($a, $b);
const $arraySlice = $a => $from => $to => $from > $to || $to > $a.size
  ? $sum("None", undefined)
  : $sum("Some", $drop($take($a, $to), $from));
const $arrayPrepend = $a => $v => $join($array([$v]), $a);
const $arrayPop = $a => $a.size === 0
  ? $sum("None", undefined)
  : $sum("Some", $record([["0", $arrayNth($a, $a.size - 1)], ["1", $take($a, $a.size - 1)]]));
const $namespace = $entries => Object.freeze($record($entries));
const $own = ($o, $k) => Object.prototype.hasOwnProperty.call($o, $k);
const $hasRest = ($o, $known) => Reflect.ownKeys($o).some($k => typeof $k !== "string" || !$known.includes($k));
const $same = Object.is;
const $unreachable = () => { throw new Error("unreachable Ruddy LIR branch"); };
"#;

fn validate_extern_target(target: &str) -> Result<(), Error> {
    // Parsing the target in its own module prevents it from closing the generated
    // initializer and introducing statements or module declarations. The newlines
    // keep a trailing line comment from consuming either closing delimiter.
    let source = format!("export default (\n{target}\n);\n");
    let module = validate_module(&source)?;
    if !matches!(module.items(), [Item::ExportDefault]) {
        return Err(Error::InvalidJavaScript(
            "an extern target must be exactly one expression".to_string(),
        ));
    }
    Ok(())
}

/// `source` as an ECMAScript module, or its first syntax error. The parser
/// keeps its nesting off the native stack, so this runs on whichever thread
/// called the backend, however deep the generated program is.
fn validate_module(source: &str) -> Result<esparse::Module, Error> {
    esparse::parse_module(source).map_err(|error| Error::InvalidJavaScript(error.to_string()))
}

fn temp(value: u32) -> String {
    format!("$v{value}")
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
        Literal::Fixed(value) => {
            out.push_str(&value.value().to_string());
            if value.kind().bits() == 64 {
                out.push('n');
            }
        }
        Literal::Real(bits) => {
            out.push_str("$real(\"");
            out.push_str(&format!("{bits:016x}"));
            out.push_str("\")");
        }
        Literal::String(value) => string(value, out),
        Literal::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
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

// These are runtime ABI names, deliberately independent of Debug rendering.
fn callback_protocol(mode: crate::externs::Callback) -> &'static str {
    use crate::externs::Callback;
    match mode {
        Callback::Sync => "Sync",
        Callback::Promise => "Promise",
    }
}
fn completion_protocol(mode: crate::externs::Completion) -> &'static str {
    use crate::externs::Completion;
    match mode {
        Completion::Immediate => "Immediate",
        Completion::Promise => "Promise",
    }
}

#[cfg(test)]
mod tests {
    use super::{Error, validate_extern_target, validate_module};

    #[test]
    fn validates_exactly_one_extern_expression() {
        for target in ["host.value", "({ value: 1 })", "host.value // trailing"] {
            validate_extern_target(target).unwrap();
        }
        for target in [
            "0); export const injected = 1; (2",
            "0); import 'injected'; (2",
            "0); sideEffect(); (2",
        ] {
            assert!(
                matches!(
                    validate_extern_target(target),
                    Err(Error::InvalidJavaScript(_))
                ),
                "accepted structural breakout: {target}"
            );
        }
    }

    #[test]
    fn validates_complete_ecmascript_modules() {
        validate_module("const value = (() => 42)(); export { value };").unwrap();
    }

    #[test]
    fn reports_parser_diagnostics_for_invalid_modules() {
        let error = validate_module("const value = (); export { value };").unwrap_err();
        assert!(
            matches!(error, Error::InvalidJavaScript(ref diagnostic) if !diagnostic.is_empty())
        );
        assert!(
            error
                .to_string()
                .starts_with("generated JavaScript is invalid: ")
        );
    }
}
