//! Every symbol the mint has handed out, with its mangled name.
//!
//! Each row also demangles its own mangled name and checks that what comes back
//! describes the symbol it started from. That makes the panel a continuous
//! round-trip test of the mangling scheme: break it while editing `symbol.rs`
//! and the offending rows turn red as you type.

use std::{collections::HashMap, time::Instant};

use ruddy::{
    symbol::{LOCAL_SEGMENT, Mint, Namespace, Symbol, demangle},
    tracking::FileID,
};

use crate::{
    stage::{Cx, Spec, plural},
    wire::{Node, Stage},
};

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let Some(mint) = cx.mint else {
        return crate::stage::skipped(spec, "lowering did not run");
    };

    let started = Instant::now();
    let mut nodes = Vec::new();
    let mut broken = 0;
    let mut modules = 0;
    let declared = declarations(cx);

    for (i, symbol) in mint.symbols().enumerate() {
        let mangled = mint.mangle(symbol);
        let path = mint.path(symbol).to_string();
        let check = round_trip(mint, symbol, &mangled, &path);
        if check.is_err() {
            broken += 1;
        }
        if mint.namespace(symbol) == Namespace::Modules {
            modules += 1;
        }

        let mut node = Node::new(i as u32, mint.name(symbol), path)
            .symbol(i as u32)
            .field("namespace", mint.namespace(symbol).to_string())
            .field(
                "scope",
                match mint.is_local(symbol) {
                    true => "local",
                    false => "global",
                },
            )
            // Where the symbol sits in the module tree, and which file put it
            // there. Both are what a bundle of several files added: a name is
            // no longer enough to say where a symbol came from.
            .field("module", enclosing(mint, symbol))
            .field(
                "file",
                declared
                    .get(&symbol)
                    .map(|&at| crate::stage::file_label(cx, at))
                    .unwrap_or_default(),
            )
            .field("mangled", mangled)
            .field(
                "demangle",
                match &check {
                    Ok(()) => "ok".to_string(),
                    Err(why) => why.clone(),
                },
            );
        if check.is_err() {
            node = node.error();
        }
        nodes.push(node);
    }

    let counted = format!(
        "{} · {} · {}",
        plural(nodes.len(), "symbol"),
        plural(cx.files.len(), "file"),
        plural(modules, "module")
    );
    let summary = match broken {
        0 => counted,
        n => format!("{counted} · {n} demangle mismatch"),
    };

    let status = match broken {
        0 => cx.status(),
        _ => crate::wire::Status::Partial,
    };
    Stage {
        micros: Some(started.elapsed().as_micros() as u64),
        nodes,
        debug: format!("{mint:#?}"),
        ..spec.stage(status, summary)
    }
}

/// The module path a symbol sits in, outermost first — `A::B` for something
/// declared in `B` inside `A` — or a dash for one at the top level of the
/// bundle, which is a real position in the tree rather than a missing one.
fn enclosing(mint: &Mint, symbol: Symbol) -> String {
    let mut chain = Vec::new();
    let mut at = mint.parent(symbol);
    while let Some(module) = at {
        chain.push(mint.name(module.symbol()).to_string());
        at = mint.parent(module.symbol());
    }
    match chain.is_empty() {
        true => "—".to_string(),
        false => {
            chain.reverse();
            chain.join("::")
        }
    }
}

/// Which file each global was declared in, read off the span its name was
/// written at.
///
/// Globals only: a local is written inside a definition rather than declared,
/// so there is no [`Decl`](ruddy::ir::Decl) with its name on it and the column
/// is left blank.
fn declarations(cx: &Cx) -> HashMap<Symbol, FileID> {
    let Some(program) = cx.program else {
        return HashMap::new();
    };
    let terms = program.terms.iter().map(|(s, d)| (*s, d.name_span.file_id));
    let types = program.types.iter().map(|(s, d)| (*s, d.name_span.file_id));
    let effects = program
        .effects
        .iter()
        .map(|(s, d)| (*s, d.name_span.file_id));
    terms.chain(types).chain(effects).collect()
}

/// Demangle a symbol's mangled name and check it describes the same symbol.
///
/// The mint does not expose disambiguators, so the comparison is over what it
/// does expose: the bundle, the path of names, and the namespace of the symbol
/// itself. That is enough to catch a mangling or demangling that loses, adds,
/// or reorders a component.
///
/// The rebuilt path puts a component carrying a disambiguator under
/// [`LOCAL_SEGMENT`], the way [`Mint::path`] does — which makes this check the
/// stricter one of the two: the mint only reports local-ness for the symbol
/// itself, while a rebuilt path disagrees if the mangling dropped or invented a
/// disambiguator anywhere along the chain.
fn round_trip(mint: &Mint, symbol: Symbol, mangled: &str, path: &str) -> Result<(), String> {
    let Some(demangled) = demangle(mangled) else {
        return Err("did not demangle".to_string());
    };
    if demangled.bundle != *mint.bundle() {
        return Err(format!("bundle became {}", demangled.bundle));
    }
    let Some(last) = demangled.path.last() else {
        return Err("empty path".to_string());
    };
    if last.namespace != mint.namespace(symbol) {
        return Err(format!("namespace became {}", last.namespace));
    }
    if last.disambiguator.is_some() != mint.is_local(symbol) {
        return Err("scope changed".to_string());
    }
    let mut rebuilt = mint.bundle().name().to_string();
    for component in &demangled.path {
        if component.disambiguator.is_some() {
            rebuilt.push_str("::");
            rebuilt.push_str(LOCAL_SEGMENT);
        }
        rebuilt.push_str("::");
        rebuilt.push_str(&component.name);
    }
    match rebuilt == path {
        true => Ok(()),
        false => Err(format!("path became {rebuilt}")),
    }
}

/// Index every symbol the mint holds, in mint order. This is both the row order
/// of this panel and the id space that `Node::symbol` points into.
pub fn index(mint: &Mint) -> HashMap<Symbol, u32> {
    mint.symbols()
        .enumerate()
        .map(|(i, symbol)| (symbol, i as u32))
        .collect()
}
