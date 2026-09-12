//! Validation of the private test interface during ordinary compilation.

use std::sync::OnceLock;

use crate::{
    inference, ir, parse,
    symbol::{Bundle, Mint, Version},
    token,
    tracking::FileID,
    types::{Presence, Rest, Row, Ty},
};

fn assertion_key() -> &'static str {
    static KEY: OnceLock<String> = OnceLock::new();
    KEY.get_or_init(|| {
        // Ask the same structural canonicalizer used by user declarations;
        // the platform contract must not depend on a particular std bundle.
        let parsed =
            parse::parse(token::lex("effect Assert = String -> ()", FileID::GENERATED).tokens);
        let mut mint =
            Mint::new(Bundle::new("ruddy-test-contract", Version::new(0, 0, 0)).unwrap());
        let output = ir::build(&mut mint, parsed.stmts);
        output
            .program
            .effect_ids
            .values()
            .next()
            .expect("Assert has an identity")
            .row_key()
    })
}

fn closed_row(row: &Row, accepts: impl Fn(&str, &crate::types::RowField) -> bool + Copy) -> bool {
    row.labels
        .iter()
        .all(|(name, field)| field.presence == Presence::Absent || accepts(name, field))
        && match &row.rest {
            Rest::Closed => true,
            Rest::More(more) => closed_row(more, accepts),
            _ => false,
        }
}

fn unit(ty: &std::sync::Arc<Ty>, semantics: &inference::Semantics) -> bool {
    matches!(&*inference::unfold(semantics.aliases(), ty), Ty::Struct(row) if closed_row(row, |_, _| false))
}

pub(crate) fn review(program: &ir::Program, semantics: &inference::Semantics) -> Vec<ir::Error> {
    program.terms.iter().filter(|(_, decl)| decl.metadata.contains_key("test"))
        .filter_map(|(symbol, decl)| {
            let scheme = semantics.schemes().get(symbol)?;
            let body = inference::unfold(semantics.aliases(), scheme.body());
            let valid = scheme.count() == 0 && matches!(&*body,
                Ty::Arrow(from, to, effects) if unit(from, semantics) && unit(to, semantics)
                    && closed_row(effects, |name, field| name == assertion_key() && field.presence == Presence::Present));
            (!valid).then_some(ir::Error { at: decl.name_at, kind: ir::ErrorKind::InvalidTestSignature })
        }).collect()
}
