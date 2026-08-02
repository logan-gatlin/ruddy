//! The lexer's output, flat.
//!
//! This list is also what colours the editor, so a token the lexer got wrong is
//! visible as a miscoloured word before any panel is opened.

use ruddy::token::Kind;

use crate::{
    stage::{Cx, Ids, Spec},
    wire::{Node, Stage},
};

pub fn build(spec: &Spec, cx: &Cx) -> Stage {
    let Some(tokens) = cx.tokens else {
        return crate::stage::skipped(spec, "lexing did not run");
    };

    let mut ids = Ids::default();
    let nodes: Vec<Node> = tokens
        .iter()
        .map(|token| {
            Node::new(ids.next(), label(&token.tracked), token.tracked.to_string())
                .at(token.span)
                .field(
                    "bytes",
                    format!("{}..{}", token.span.start, token.span.end()),
                )
                // Underscored fields are for the page, not for the table: this
                // one is the class the editor paints the token with.
                .field("_class", class(&token.tracked))
        })
        .collect();

    let summary = match nodes.len() {
        1 => "1 token".to_string(),
        n => format!("{n} tokens"),
    };
    Stage {
        micros: cx.micros.lex,
        nodes,
        debug: format!("{tokens:#?}"),
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
        Kind::Trait => "Trait",
        Kind::For => "For",
        Kind::Impl => "Impl",
        Kind::End => "End",
        Kind::With => "With",
        Kind::Fn => "Fn",
        Kind::Equal => "Equal",
        Kind::FatArrow => "FatArrow",
        Kind::Arrow => "Arrow",
        Kind::Colon => "Colon",
        Kind::Comma => "Comma",
        Kind::Dot => "Dot",
        Kind::LeftBrace => "LeftBrace",
        Kind::RightBrace => "RightBrace",
        Kind::LeftParen => "LeftParen",
        Kind::RightParen => "RightParen",
        Kind::Identifier(_) => "Identifier",
        Kind::Natural(_) => "Natural",
    }
}

/// The CSS class the editor paints this token with. Keywords, punctuation,
/// identifiers and literals are the only distinctions the surface syntax
/// supports so far.
pub fn class(kind: &Kind) -> &'static str {
    match kind {
        Kind::Identifier(_) => "ident",
        Kind::Natural(_) => "number",
        Kind::Equal
        | Kind::FatArrow
        | Kind::Arrow
        | Kind::Colon
        | Kind::Comma
        | Kind::Dot
        | Kind::LeftBrace
        | Kind::RightBrace
        | Kind::LeftParen
        | Kind::RightParen => "punct",
        _ => "keyword",
    }
}
