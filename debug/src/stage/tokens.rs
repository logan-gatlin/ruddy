//! The lexer's output, flat.
//!
//! This list is also what colours the editor, so a token the lexer got wrong is
//! visible as a miscoloured word before any panel is opened.

use ruddy::token::Kind;

use crate::{
    stage::{Cx, Ids, Spec, plural},
    wire::{Node, Stage},
};

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let Some(loaded) = cx.bundle else {
        return crate::stage::skipped(spec, "lexing did not run");
    };

    // One row per file, with that file's tokens under it: a bundle is several
    // streams rather than one, and a flat list would run the last token of one
    // file into the first of the next with nothing to say where the seam was.
    let mut ids = Ids::default();
    let mut counted = 0;
    let nodes: Vec<Node> = loaded
        .loaded
        .iter()
        .map(|file| {
            counted += file.tokens.len();
            Node::new(
                ids.next(),
                "File",
                format!("{} · {}", file.path, plural(file.tokens.len(), "token")),
            )
            .children(file.tokens.iter().map(|token| {
                Node::new(ids.next(), label(&token.tracked), token.tracked.to_string())
                    .at(token.span)
                    .field(
                        "bytes",
                        format!("{}..{}", token.span.start, token.span.end()),
                    )
                    // Underscored fields are for the page, not for the table:
                    // this one is the class the editor paints the token with.
                    .field("_class", class(&token.tracked))
            }))
        })
        .collect();

    let summary = format!(
        "{} · {}",
        plural(counted, "token"),
        plural(nodes.len(), "file")
    );
    Stage {
        micros: Some(cx.micros.lex),
        nodes,
        debug: format!("{:#?}", loaded.loaded),
        ..spec.stage(cx.status(), summary)
    }
}

/// The variant name, which is what the panel groups and filters on. Kept
/// separate from `Display`, which renders the token's spelling instead.
pub fn label(kind: &Kind) -> &'static str {
    match kind {
        Kind::Let => "Let",
        Kind::In => "In",
        Kind::Type => "Type",
        Kind::End => "End",
        Kind::With => "With",
        Kind::Match => "Match",
        Kind::Fn => "Fn",
        Kind::Effect => "Effect",
        Kind::Handle => "Handle",
        Kind::Raise => "Raise",
        Kind::Bundle => "Bundle",
        Kind::Module => "Module",
        Kind::Equal => "Equal",
        Kind::FatArrow => "FatArrow",
        Kind::Arrow => "Arrow",
        Kind::Colon => "Colon",
        Kind::ColonColon => "ColonColon",
        Kind::Comma => "Comma",
        Kind::Semicolon => "Semicolon",
        Kind::Dot => "Dot",
        Kind::DotDot => "DotDot",
        Kind::Plus => "Plus",
        Kind::NotEqual => "NotEqual",
        Kind::Backslash => "Backslash",
        Kind::Pipe => "Pipe",
        Kind::Tag(_) => "Tag",
        Kind::EffectLabel(_) => "EffectLabel",
        Kind::Variable(_) => "Variable",
        Kind::LeftBrace => "LeftBrace",
        Kind::RightBrace => "RightBrace",
        Kind::LeftParen => "LeftParen",
        Kind::RightParen => "RightParen",
        Kind::Identifier(_) => "Identifier",
        Kind::Underscore => "Underscore",
        Kind::Natural(_) => "Natural",
    }
}

/// The CSS class the editor paints this token with. Keywords, punctuation,
/// identifiers, literals and tags are the only distinctions the surface syntax
/// supports so far.
///
/// A tag has a class of its own rather than sharing the identifier's: it names
/// a case rather than a definition, nothing ever resolves it, and a reader
/// scanning a sum wants to see where the cases are. An effect has a third,
/// because it is neither: it resolves to a declaration, like a name, and wears
/// a sigil, like a tag.
pub fn class(kind: &Kind) -> &'static str {
    match kind {
        Kind::Identifier(_) => "ident",
        Kind::Natural(_) => "number",
        Kind::Tag(_) => "tag",
        Kind::EffectLabel(_) => "effect",
        Kind::Variable(_) => "variable",
        Kind::Equal
        | Kind::FatArrow
        | Kind::Arrow
        | Kind::Colon
        | Kind::ColonColon
        | Kind::Comma
        | Kind::Dot
        | Kind::DotDot
        | Kind::NotEqual
        | Kind::Backslash
        | Kind::Plus
        | Kind::Pipe
        | Kind::LeftBrace
        | Kind::RightBrace
        | Kind::LeftParen
        | Kind::RightParen => "punct",
        _ => "keyword",
    }
}
