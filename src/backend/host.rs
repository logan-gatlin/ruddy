//! Root-only host adapters. Dependency interfaces and internal global reads
//! retain their original evidence convention. Ordinary inference checks each
//! generated call under the platform handlers and permits only Immediate to remain.

use std::{collections::HashMap, fmt, sync::Arc};

use crate::artifact::{Artifact, DeclaredType, Presence, Rest, Row, Type};

use super::js::{Error, Platform};

pub(super) struct Adapters {
    pub artifact: Artifact,
    pub exports: HashMap<String, String>,
}

/// Which way a value crosses the JavaScript boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Direction {
    ToJs,
    FromJs,
}

/// A conversion shape for one boundary crossing: the finite graph of the
/// type under the host's policy, and the fields the host may leave out. It
/// can inspect optional fields without claiming an exact runtime identity
/// for an abstract presence package, which is why it is not a descriptor.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeTemplate {
    pub descriptor: crate::reification::Descriptor,
    pub optional_fields: crate::reification::OptionalFields,
}

impl NativeTemplate {
    pub fn template(
        ty: &Arc<crate::types::Ty>,
        aliases: &indexmap::IndexMap<crate::symbol::Symbol, crate::types::Scheme>,
    ) -> Result<(Self, Vec<u32>), String> {
        let (root, graph) = crate::ir::representation_graph(ty, aliases);
        let (descriptor, optional_fields, parameters) =
            crate::reification::Descriptor::from_graph_policy(root, &graph, true)?;
        Ok((
            Self {
                descriptor,
                optional_fields,
            },
            parameters,
        ))
    }

    pub fn validate(&self, parameters: usize) -> Result<(), &'static str> {
        self.descriptor.validate(parameters)?;
        for (index, optional) in &self.optional_fields {
            let Some(crate::reification::Node::Struct(fields)) =
                self.descriptor.nodes.get(*index as usize)
            else {
                return Err("native optional fields require a record shape");
            };
            if optional
                .iter()
                .any(|name| !fields.iter().any(|(field, _)| field == name))
            {
                return Err("native optional field is absent from its record shape");
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
struct View<'a> {
    ty: &'a Type,
    args: Arc<Vec<View<'a>>>,
}

struct Quoted<'a>(&'a str);

impl fmt::Display for Quoted<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        crate::ui::write_string(f, self.0)
    }
}

impl<'a> View<'a> {
    fn child(&self, ty: &'a Type) -> Self {
        Self {
            ty,
            args: self.args.clone(),
        }
    }
}

type Node = usize;

/// Build the native root contract from the portable interface, including
/// incoming and nested callable positions. Effects are handled by Graph's
/// generated wrappers; they never become runtime type identities here.
pub(super) fn native_descriptor(
    root: &Type,
    declarations: &HashMap<&str, &DeclaredType>,
) -> Result<NativeTemplate, String> {
    use crate::reification::{Descriptor, Node as Runtime};
    let mut nodes = vec![Runtime::ForeignValue];
    let mut optional_fields =
        std::collections::BTreeMap::<u32, std::collections::BTreeSet<String>>::new();
    let mut work = vec![(
        0,
        View {
            ty: root,
            args: Arc::default(),
        },
        String::from("value"),
        false,
    )];
    let mut aliases = HashMap::new();
    let projections = forwarding(declarations);
    while let Some((id, view, path, incoming)) = work.pop() {
        let view = resolved(view);
        let mut child = |view, position: String, incoming| {
            let index = nodes.len();
            nodes.push(Runtime::ForeignValue);
            work.push((index, view, position, incoming));
            index as u32
        };
        nodes[id] = match view.ty {
            Type::Nat => Runtime::Nat,
            Type::Int => Runtime::Int,
            Type::Fixed(kind) => Runtime::Fixed(*kind),
            Type::Real => Runtime::Real,
            Type::String => Runtime::String,
            Type::Bool => Runtime::Bool,
            Type::ForeignValue => Runtime::ForeignValue,
            Type::Package(inner) => Runtime::Alias(child(view.child(inner), path, incoming)),
            // A hidden type crosses the boundary as a sealed package: the
            // host holds it and hands it back, and cannot make one.
            Type::Hidden { body, .. } => Runtime::Hidden(child(view.child(body), path, incoming)),
            Type::HiddenVar { .. } => Runtime::HiddenBound(0),
            Type::Arrow(from, to, row) => {
                let effects = fields(row, view.clone(), declarations);
                if incoming
                    && (!matches!(effects.rest, Rest::Closed)
                        || effects
                            .labels
                            .iter()
                            .any(|(_, presence, _)| **presence != Presence::Absent))
                {
                    return Err(format!(
                        "{path} has an effectful callback contract that cannot be supplied by a native JavaScript caller"
                    ));
                }
                let argument = child(view.child(from), format!("{path} argument"), true);
                let result = child(view.child(to), format!("{path} result"), incoming);
                // A host callable performs nothing: its row is empty.
                let effects = nodes.len() as u32;
                nodes.push(Runtime::Effects(Vec::new()));
                Runtime::Arrow([argument, result, effects])
            }
            Type::Array(inner) => Runtime::Array(child(
                view.child(inner),
                format!("{path} element"),
                incoming,
            )),
            Type::Mirror(inner) => {
                Runtime::Mirror(child(view.child(inner), format!("{path} mirror"), incoming))
            }
            Type::Named { name, args } => {
                let declaration = declarations
                    .get(name.as_str())
                    .ok_or_else(|| format!("cannot inspect exported type `{name}`"))?;
                let arguments: Vec<_> = args.iter().map(|arg| view.child(arg)).collect();
                let state = (
                    name.clone(),
                    incoming,
                    arguments
                        .iter()
                        .map(|arg| key(arg.clone(), declarations, &projections))
                        .collect::<Vec<_>>(),
                );
                Runtime::Alias(if let Some(&index) = aliases.get(&state) {
                    index
                } else {
                    aliases.insert(state, id as u32);
                    child(
                        View {
                            ty: &declaration.scheme.body,
                            args: Arc::new(arguments),
                        },
                        path,
                        incoming,
                    )
                })
            }
            Type::Struct(row) | Type::Sum(row) => {
                let fields = fields(row, view.clone(), declarations);
                if !matches!(fields.rest, Rest::Closed) {
                    return Err(format!(
                        "{path} needs runtime information for unresolved fields or cases; export a concrete type or ForeignValue wrapper"
                    ));
                }
                let mut fields: Vec<_> = fields
                    .labels
                    .into_iter()
                    .filter(|(_, presence, _)| **presence != Presence::Absent)
                    .map(|(name, presence, ty)| {
                        if matches!(view.ty, Type::Struct(_))
                            && !matches!(presence, Presence::Present | Presence::Absent)
                        {
                            optional_fields
                                .entry(id as u32)
                                .or_default()
                                .insert(name.to_owned());
                        }
                        (
                            name.to_owned(),
                            child(ty, format!("{path}.{name}"), incoming),
                        )
                    })
                    .collect();
                fields.sort_by(|a, b| a.0.cmp(&b.0));
                if matches!(view.ty, Type::Struct(_)) {
                    Runtime::Struct(fields)
                } else {
                    Runtime::Sum(fields)
                }
            }
            _ => {
                return Err(format!(
                    "{path} needs runtime type information unavailable to JavaScript; export a concrete instantiation or a ForeignValue wrapper"
                ));
            }
        };
    }
    Ok(NativeTemplate {
        descriptor: Descriptor { nodes },
        optional_fields,
    })
}

#[derive(Clone)]
enum Plan {
    Value,
    Alias {
        name: String,
        inner: Node,
    },
    Function {
        effectful: bool,
        handlers: String,
        residual_effects: &'static str,
        result: Node,
    },
    Struct(Vec<(String, bool, Node)>, bool),
    Sum(Vec<(String, bool, Node)>, bool),
    Array(Node),
}

impl Plan {
    fn open(&self) -> bool {
        match self {
            Self::Struct(fields, open) => *open || fields.iter().any(|(_, present, _)| !present),
            Self::Sum(fields, open) => *open || fields.iter().any(|(_, present, _)| !present),
            _ => false,
        }
    }

    fn children(&self) -> Vec<Node> {
        match self {
            Self::Value => Vec::new(),
            Self::Alias { inner, .. } | Self::Array(inner) => vec![*inner],
            Self::Function { result, .. } => vec![*result],
            Self::Struct(fields, _) => fields.iter().map(|(_, _, inner)| *inner).collect(),
            Self::Sum(fields, _) => fields.iter().map(|(_, _, inner)| *inner).collect(),
        }
    }
}

/// The graph retains alias applications as back edges. Only outgoing positions
/// are traversed; incoming callback requirements are checked by inference.
struct Graph {
    nodes: Vec<Plan>,
    needed: Vec<bool>,
    component: Vec<usize>,
    cyclic: Vec<bool>,
}

/// Resolve alias parameters without expanding recursive structural aliases.
fn resolved(mut view: View<'_>) -> View<'_> {
    while let Type::Bound(index) = view.ty {
        let Some(arg) = view.args.get(*index as usize) else {
            break;
        };
        view = arg.clone();
    }
    view
}

/// Forwarding declarations select a type argument without adding structure.
/// Compute their projections to a fixed point, including chains such as
/// Second a b = Id b, without unfolding recursive structural declarations.
fn forwarding<'a>(declarations: &HashMap<&'a str, &'a DeclaredType>) -> HashMap<&'a str, usize> {
    let mut projections = HashMap::new();
    loop {
        let before = projections.len();
        for (&name, declaration) in declarations {
            if projections.contains_key(name) {
                continue;
            }
            let mut ty = &declaration.scheme.body;
            loop {
                match ty {
                    Type::Bound(index) => {
                        projections.insert(name, *index as usize);
                        break;
                    }
                    Type::Named { name, args } => {
                        let Some(&index) = projections.get(name.as_str()) else {
                            break;
                        };
                        let Some(arg) = args.get(index) else {
                            break;
                        };
                        ty = arg;
                    }
                    _ => break,
                }
            }
        }
        if projections.len() == before {
            return projections;
        }
    }
}

struct Fields<'a> {
    labels: Vec<(&'a str, &'a Presence, View<'a>)>,
    rest: &'a Rest,
}

/// Splice row arguments just as unfolding a source alias does. A row parameter
/// may carry callable fields, or supply the effects on a recursive arrow.
fn fields<'a>(
    mut row: &'a Row,
    mut view: View<'a>,
    declarations: &HashMap<&str, &'a DeclaredType>,
) -> Fields<'a> {
    let mut labels = Vec::new();
    loop {
        labels.extend(
            row.labels
                .iter()
                .map(|(name, field)| (name.as_str(), &field.presence, view.child(&field.ty))),
        );
        match &row.rest {
            Rest::More(more) => row = more,
            Rest::Bound(index) if (*index as usize) < view.args.len() => {
                view = resolved(view.args[*index as usize].clone());
                while let Type::Named { name, args } = view.ty {
                    let Some(declaration) = declarations.get(name.as_str()) else {
                        break;
                    };
                    view = resolved(View {
                        ty: &declaration.scheme.body,
                        args: Arc::new(args.iter().map(|arg| view.child(arg)).collect()),
                    });
                }
                match view.ty {
                    Type::Struct(more) | Type::Sum(more) | Type::Arrow(_, _, more) => row = more,
                    _ => {
                        return Fields {
                            labels,
                            rest: &row.rest,
                        };
                    }
                }
            }
            _ => {
                return Fields {
                    labels,
                    rest: &row.rest,
                };
            }
        }
    }
}

// State keys retain instantiated arguments, including effect rows and presence
// identities. Only forwarding aliases are erased: unfolding every named body
// here would expand shared recursive arguments into exponentially large trees.
fn key(
    view: View<'_>,
    declarations: &HashMap<&str, &DeclaredType>,
    projections: &HashMap<&str, usize>,
) -> String {
    enum Work<'a> {
        Type(View<'a>),
        Text(String),
    }
    let mut work = vec![Work::Type(view)];
    let mut out = String::new();
    while let Some(item) = work.pop() {
        let view = match item {
            Work::Text(text) => {
                out.push_str(&text);
                continue;
            }
            Work::Type(view) => resolved(view),
        };
        match view.ty {
            Type::Named { name, args } => {
                if let Some(&index) = projections.get(name.as_str()) {
                    work.push(Work::Type(view.child(&args[index])));
                } else {
                    out.push_str(&format!("N{name:?}("));
                    work.push(Work::Text(")".into()));
                    for arg in args.iter().rev() {
                        work.push(Work::Type(view.child(arg)));
                    }
                }
            }
            Type::Arrow(_, _, _) | Type::Struct(_) | Type::Sum(_) => {
                let row = match view.ty {
                    Type::Arrow(input, result, row) => {
                        out.push_str("F(");
                        work.push(Work::Type(view.child(result)));
                        work.push(Work::Type(view.child(input)));
                        row
                    }
                    Type::Struct(row) => {
                        out.push_str("R(");
                        row
                    }
                    Type::Sum(row) => {
                        out.push_str("S(");
                        row
                    }
                    _ => unreachable!(),
                };
                let fields = fields(row, view.clone(), declarations);
                out.push_str(&format!("{:?};", fields.rest));
                work.push(Work::Text(")".into()));
                for (name, presence, ty) in fields.labels.into_iter().rev() {
                    work.push(Work::Type(ty));
                    work.push(Work::Text(format!("{name:?}:{presence:?};")));
                }
            }
            Type::Array(inner) | Type::Package(inner) | Type::Mirror(inner) => {
                out.push_str(match view.ty {
                    Type::Array(_) => "A(",
                    Type::Mirror(_) => "M(",
                    _ => "P(",
                });
                work.push(Work::Text(")".into()));
                work.push(Work::Type(view.child(inner)));
            }
            _ => out.push_str(&format!("{:?};", view.ty)),
        }
    }
    out
}

impl Graph {
    fn build(
        root: &Type,
        declarations: &HashMap<&str, &DeclaredType>,
        platform: Platform,
    ) -> Result<Self, String> {
        let mut nodes = vec![Plan::Value];
        let mut work = vec![(
            0,
            View {
                ty: root,
                args: Arc::default(),
            },
        )];
        let mut aliases = HashMap::new();
        let projections = forwarding(declarations);
        while let Some((id, mut view)) = work.pop() {
            loop {
                match view.ty {
                    Type::Package(inner) => view = view.child(inner),
                    Type::Bound(index) if (*index as usize) < view.args.len() => {
                        view = view.args[*index as usize].clone()
                    }
                    _ => break,
                }
            }
            let mut child = |view| {
                let id = nodes.len();
                nodes.push(Plan::Value);
                work.push((id, view));
                id
            };
            let plan = match view.ty {
                Type::Named { name, args } => {
                    let Some(decl) = declarations.get(name.as_str()) else {
                        return Err(format!("cannot inspect exported type `{name}`"));
                    };
                    let arguments: Vec<_> = args.iter().map(|arg| view.child(arg)).collect();
                    let key = (
                        name.clone(),
                        arguments
                            .iter()
                            .map(|arg| key(arg.clone(), declarations, &projections))
                            .collect::<Vec<_>>(),
                    );
                    let inner = if let Some(&existing) = aliases.get(&key) {
                        existing
                    } else {
                        // Reserve before expanding: recursive children refer to this state.
                        aliases.insert(key, id);
                        child(View {
                            ty: &decl.scheme.body,
                            args: Arc::new(arguments),
                        })
                    };
                    Plan::Alias {
                        name: name.clone(),
                        inner,
                    }
                }
                Type::Arrow(_, result, row) => {
                    let fields = fields(row, view.clone(), declarations);
                    let open = !matches!(fields.rest, Rest::Closed);
                    let platform_handlers = match platform {
                        Platform::Node => concat!(
                            include_str!("node-handler.rud"),
                            include_str!("web-handler.rud")
                        ),
                        Platform::Web => include_str!("web-handler.rud"),
                    };
                    let handlers = platform_handlers
                        .lines()
                        .filter(|line| {
                            open || fields.labels.iter().any(|(label, presence, _)| {
                                let name = crate::types::EffectId::parse_row_key(label)
                                    .map_or(*label, |(name, _)| name);
                                // Named operations use `!IO.write`; unnamed ones use
                                // `!Exit code`. Require either delimiter after the name.
                                **presence != Presence::Absent
                                    && [format!("| !{name}."), format!("| !{name} ")]
                                        .iter()
                                        .any(|prefix| line.trim_start().starts_with(prefix))
                            })
                        })
                        .map(|line| format!("{line}\n"))
                        .collect();
                    Plan::Function {
                        handlers,
                        // Select by name here, then let ordinary inference below
                        // verify the complete zero-operation structural interface.
                        residual_effects: if open
                            || fields.labels.iter().any(|(label, presence, _)| {
                                let name = crate::types::EffectId::parse_row_key(label)
                                    .map_or(*label, |(name, _)| name);
                                **presence != Presence::Absent && name == "Immediate"
                            }) {
                            " + !Immediate"
                        } else {
                            ""
                        },
                        effectful: fields
                            .labels
                            .iter()
                            .any(|(_, presence, _)| **presence != Presence::Absent)
                            || !matches!(fields.rest, Rest::Closed),
                        result: child(view.child(result)),
                    }
                }
                Type::Array(inner) => Plan::Array(child(view.child(inner))),
                // A mirror crosses as the opaque value it is.
                Type::Mirror(_) => Plan::Value,
                Type::Struct(row) | Type::Sum(row) => {
                    let fields = fields(row, view.clone(), declarations);
                    let open = !matches!(fields.rest, Rest::Closed);
                    let fields: Vec<_> = fields
                        .labels
                        .into_iter()
                        .filter(|(_, presence, _)| **presence != Presence::Absent)
                        .map(|(name, presence, ty)| {
                            (name.to_string(), *presence == Presence::Present, child(ty))
                        })
                        .collect();
                    if matches!(view.ty, Type::Struct(_)) {
                        Plan::Struct(fields, open)
                    } else {
                        Plan::Sum(fields, open)
                    }
                }
                _ => Plan::Value,
            };
            nodes[id] = plan;
        }
        let mut parents = vec![Vec::new(); nodes.len()];
        for (id, node) in nodes.iter().enumerate() {
            for child in node.children() {
                parents[child].push(id);
            }
        }
        let mut needed = vec![false; nodes.len()];
        let mut pending: Vec<_> = nodes
            .iter()
            .enumerate()
            .filter_map(|(id, node)| {
                matches!(
                    node,
                    Plan::Function {
                        effectful: true,
                        ..
                    }
                )
                .then_some(id)
            })
            .collect();
        while let Some(id) = pending.pop() {
            if !needed[id] {
                needed[id] = true;
                pending.extend(&parents[id]);
            }
        }
        // Iterative Kosaraju traversal. SCCs determine where explicit recursive
        // host types are necessary; everything outside an SCC is a parameter.
        let mut seen = vec![false; nodes.len()];
        let mut order = Vec::new();
        let mut stack = vec![(0, false)];
        while let Some((id, finish)) = stack.pop() {
            if finish {
                order.push(id);
            } else if !seen[id] {
                seen[id] = true;
                stack.push((id, true));
                stack.extend(nodes[id].children().into_iter().map(|child| (child, false)));
            }
        }
        let mut component = vec![usize::MAX; nodes.len()];
        let mut cyclic = Vec::new();
        for root in order.into_iter().rev() {
            if component[root] != usize::MAX {
                continue;
            }
            let group = cyclic.len();
            let mut size = 0;
            let mut stack = vec![root];
            while let Some(id) = stack.pop() {
                if component[id] != usize::MAX {
                    continue;
                }
                component[id] = group;
                size += 1;
                stack.extend(&parents[id]);
            }
            cyclic.push(size > 1 || nodes[root].children().contains(&root));
        }
        Ok(Self {
            nodes,
            needed,
            component,
            cyclic,
        })
    }

    fn helper(&self, id: Node) -> bool {
        self.needed[id]
            && self.cyclic[self.component[id]]
            && matches!(self.nodes[id], Plan::Alias { .. })
    }

    fn parameters(&self, group: usize) -> Vec<String> {
        let mut params = Vec::new();
        for (id, node) in self.nodes.iter().enumerate() {
            if self.component[id] != group {
                continue;
            }
            if matches!(node, Plan::Function { .. }) {
                params.push(format!("input_{id}"));
            }
            if node.open() {
                params.push(format!("rest_{id}"));
            }
            for child in node.children() {
                if self.component[child] != group && !params.contains(&format!("value_{child}")) {
                    params.push(format!("value_{child}"));
                }
            }
        }
        params
    }

    fn host_type(&self, id: Node, group: usize, prefix: &str, expand: bool) -> String {
        if self.component[id] != group {
            return format!("'value_{id}");
        }
        match &self.nodes[id] {
            Plan::Alias { inner, .. } if expand => self.host_type(*inner, group, prefix, false),
            Plan::Alias { .. } => format!(
                "{prefix}Type_{id}{}",
                self.parameters(group)
                    .iter()
                    .map(|p| format!(" '{p}"))
                    .collect::<String>()
            ),
            Plan::Function {
                result,
                residual_effects,
                ..
            } => format!(
                "('input_{id} -> {}{residual_effects})",
                self.host_type(*result, group, prefix, false)
            ),
            Plan::Array(inner) => format!("[{}]", self.host_type(*inner, group, prefix, false)),
            Plan::Struct(fields, _) => {
                let mut fields: Vec<_> = fields
                    .iter()
                    .filter(|(_, present, _)| *present)
                    .map(|(name, _, child)| {
                        format!(
                            "{}: {}",
                            Quoted(name),
                            self.host_type(*child, group, prefix, false)
                        )
                    })
                    .collect();
                if self.nodes[id].open() {
                    fields.push(format!("..'rest_{id}"));
                }
                format!("{{ {} }}", fields.join(", "))
            }
            Plan::Sum(fields, _) => {
                let mut fields: Vec<_> = fields
                    .iter()
                    .filter(|(_, present, _)| *present)
                    .map(|(name, _, child)| {
                        format!(
                            "#{} {}",
                            Quoted(name),
                            self.host_type(*child, group, prefix, false)
                        )
                    })
                    .collect();
                if self.nodes[id].open() {
                    fields.push(format!("..'rest_{id}"));
                }
                format!("(| {})", fields.join(" | "))
            }
            Plan::Value => unreachable!("a value leaf cannot be in a cyclic component"),
        }
    }

    fn expression(
        &self,
        node: Node,
        value: &str,
        prefix: &str,
        next: &mut usize,
        expand: bool,
    ) -> String {
        if !self.needed[node] {
            return value.into();
        }
        if self.helper(node) && !expand {
            return format!("({prefix}adapt_{node} ({value}))");
        }
        let id = *next;
        *next += 1;
        match &self.nodes[node] {
            Plan::Value => value.into(),
            Plan::Alias { inner, .. } => self.expression(*inner, value, prefix, next, false),
            Plan::Function {
                effectful,
                handlers,
                residual_effects,
                result,
            } => {
                let call = format!("{value} arg_{id}");
                let call = if *effectful && !handlers.is_empty() {
                    format!("handle {call} with\n{handlers}end")
                } else {
                    call
                };
                let result = self.expression(*result, &format!("value_{id}"), prefix, next, false);
                format!(
                    "(do let function_{id}: _ -> _{residual_effects} = fn arg_{id} => do let value_{id} = {call} return {result} end return function_{id} end)"
                )
            }
            Plan::Struct(fields, _) => {
                let mut body = format!("do let record_{id} = {value}\n");
                let mut previous = format!("record_{id}");
                for (field, present, inner) in fields {
                    if !self.needed[*inner] {
                        continue;
                    }
                    let field = Quoted(field).to_string();
                    let target = format!("record_{}", *next);
                    *next += 1;
                    let projected = format!("{previous}.{field}");
                    let adapted = self.expression(*inner, &projected, prefix, next, false);
                    let updated = format!("{{ {field}: {adapted}, ..{previous} }}");
                    let updated = if *present {
                        updated
                    } else {
                        format!(
                            "match {previous} with | {{ {field}: _, .. }} => {updated} | _ => {previous} end"
                        )
                    };
                    body.push_str(&format!("let {target} = {updated}\n"));
                    previous = target;
                }
                format!("({body}return {previous} end)")
            }
            Plan::Sum(cases, _) => {
                let mut body = format!("(match {value} with\n");
                for (tag, _, inner) in cases {
                    let tag = Quoted(tag);
                    let inner =
                        self.expression(*inner, &format!("payload_{id}"), prefix, next, false);
                    body.push_str(&format!("| #{tag} payload_{id} => #{tag} {inner}\n"));
                }
                format!("{body}end)")
            }
            Plan::Array(inner) => {
                let inner = self.expression(*inner, &format!("item_{id}"), prefix, next, false);
                format!(
                    "(do let array_{id} = {value}\nlet map_{id}: Nat -> [_] -> [_] = fn index_{id} output_{id} => match host_get array_{id} index_{id} with\n| #Some item_{id} => map_{id} (host_next index_{id}) (host_push output_{id} {inner})\n| #None => output_{id}\nend\nreturn map_{id} 0n [] end)"
                )
            }
        }
    }

    fn definitions(&self, prefix: &str, raw_types: &HashMap<&str, (String, String)>) -> String {
        let mut source = String::new();
        for (id, node) in self.nodes.iter().enumerate() {
            if !self.helper(id) {
                continue;
            }
            let group = self.component[id];
            let params = self.parameters(group);
            let ty = self.host_type(id, group, prefix, true);
            source.push_str(&format!(
                "type {prefix}Type_{id}{} = {ty}\n",
                params.iter().map(|p| format!(" '{p}")).collect::<String>()
            ));
            let Plan::Alias { name, .. } = node else {
                unreachable!()
            };
            let (raw, arguments) = &raw_types[name.as_str()];
            let holes = params
                .iter()
                .map(|param| {
                    if let Some(id) = param.strip_prefix("rest_") {
                        match self.nodes[id.parse::<usize>().expect("generated node index")] {
                            Plan::Struct(..) => " { .. }",
                            Plan::Sum(..) => " (| ..)",
                            _ => unreachable!("only row nodes have a rest parameter"),
                        }
                    } else {
                        " _"
                    }
                })
                .collect::<String>();
            let body = self.expression(id, "original", prefix, &mut 0, true);
            source.push_str(&format!("let {prefix}adapt_{id}: (program::{raw}{}) -> ({prefix}Type_{id}{}) = fn original => {body}\n", arguments, holes));
        }
        source
    }
}

const ARRAY_HELPERS: &str = r#"
type HostOption 'a = #Some 'a | #None
extern host_get: ['a] -> Nat -> HostOption 'a = "$arrayGet"
extern host_push: ['a] -> 'a -> ['a] = "$arrayPush"
extern host_next: Nat -> Nat = "n => n + 1"
"#;

pub(super) fn compile(
    root: &Artifact,
    dependencies: &[&Artifact],
    platform: Platform,
) -> Result<Option<Adapters>, Error> {
    if root.header().kind == crate::artifact::Kind::Executable {
        return Ok(None);
    }
    let declarations: HashMap<_, _> = dependencies
        .iter()
        .copied()
        .chain(std::iter::once(root))
        .flat_map(|artifact| artifact.header().types.iter())
        .map(|decl| (decl.name.as_str(), decl))
        .collect();
    let prefix = format!(
        "{}@{}::",
        root.header().identity.name,
        root.header().identity.version
    );
    // Publish aliases only in the temporary compilation view. In particular,
    // private and transitive source types remain private in the real artifact.
    let mut interface = root.clone();
    let mut raw_types = HashMap::new();
    let mut namespace = String::from("RuddyHost");
    while interface
        .header
        .types
        .iter()
        .any(|ty| ty.name.starts_with(&format!("{prefix}{namespace}")))
    {
        namespace.push('_');
    }
    let mut names: Vec<_> = declarations.keys().copied().collect();
    names.sort_unstable();
    for (index, name) in names.into_iter().enumerate() {
        let declaration = declarations[name];
        let local = format!("{namespace}_{index}");
        let mut raw = declaration.clone();
        raw.name = format!("{prefix}{local}");
        raw.exported = true;
        let arguments = raw
            .params
            .iter()
            .map(|param| match param.sense {
                crate::artifact::Sense::Effects => " (..)",
                crate::artifact::Sense::Row => " { .. }",
                crate::artifact::Sense::Type | crate::artifact::Sense::Region => " _",
            })
            .collect::<String>();
        raw_types.insert(name, (local, arguments));
        interface.header.types.push(raw);
    }
    let mut definitions = Vec::new();
    for (index, value) in root.header().values.iter().enumerate() {
        if !value.scheme.representations.is_empty() {
            return Err(Error::ExportType { name: value.name.clone(), message: "this value requires runtime type information from its caller; export a concrete instantiation or a ForeignValue wrapper".into() });
        }
        native_descriptor(&value.scheme.body, &declarations).map_err(|message| {
            Error::ExportType {
                name: value.name.clone(),
                message,
            }
        })?;
        let graph =
            Graph::build(&value.scheme.body, &declarations, platform).map_err(|message| {
                Error::Export {
                    name: value.name.clone(),
                    message,
                }
            })?;
        if !graph.needed[0] {
            continue;
        }
        let Some(name) = value.name.strip_prefix(&prefix) else {
            return Err(Error::UnresolvedPublicValue(value.name.clone()));
        };
        let helper_prefix = format!("Host_{index}_");
        let mut source = graph.definitions(&helper_prefix, &raw_types);
        source.push_str(&format!(
            "let host_{index} = {}\n",
            graph.expression(
                0,
                &format!("program::{name}"),
                &helper_prefix,
                &mut 0,
                false
            )
        ));
        definitions.push((value.name.clone(), format!("host_{index}"), source));
    }
    if definitions.is_empty() {
        return Ok(None);
    }
    let prelude = match platform {
        Platform::Node => include_str!("node-platform.rud"),
        Platform::Web => "effect Immediate\n",
    };
    let web = include_str!("web-platform.rud");
    let prelude = format!("{prelude}\n{web}\n{ARRAY_HELPERS}");
    let source = format!(
        "{prelude}\n{}",
        definitions
            .iter()
            .map(|(_, _, source)| source.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    );
    let adapter =
        crate::entry::compile_adapter(&interface, dependencies, &source).map_err(|error| {
            // Recheck individually only on failure so the diagnostic names the
            // public export, rather than a generated helper or the entire bundle.
            for (name, _, definition) in &definitions {
                if let Err(error) = crate::entry::compile_adapter(
                    &interface,
                    dependencies,
                    &format!("{prelude}\n{definition}"),
                ) {
                    return Error::Export {
                        name: name.clone(),
                        message: error.join("; "),
                    };
                }
            }
            Error::Export {
                name: root.header().identity.name.clone(),
                message: error.join("; "),
            }
        })?;
    let exports = definitions
        .into_iter()
        .map(|(original, local, _)| {
            let qualified = format!(
                "{}@{}::{local}",
                adapter.header().identity.name,
                adapter.header().identity.version
            );
            (original, qualified)
        })
        .collect();
    Ok(Some(Adapters {
        artifact: adapter,
        exports,
    }))
}
