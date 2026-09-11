//! Rendering an artifact's metadata as the source it was written as.
//!
//! The artifact is span-free and names nothing by symbol, so unlike
//! [`ir`](super::ir) this printer needs no mint; and unlike [`ast`](super::ast)
//! it has no written spelling to keep, so it prints the canonical one the
//! lowered tree's printer prints. It exists so the artifact tab can show a
//! declaration's metadata in the language rather than in the artifact's own
//! text, which the tab already shows whole.

use std::fmt;

use ruddy::artifact::Data;

use crate::print::{
    Grouped, Prec, string, tuple_field_order, write_struct, write_tag, write_tuple,
};

/// A published value, ready to print.
struct Shown<'a>(&'a Data);

/// `@key`, and its value after a space when it is anything but unit — the
/// empty struct, which a bare attribute and a written `()` both became.
struct Attribute<'a> {
    key: &'a str,
    value: &'a Data,
}

fn unit(data: &Data) -> bool {
    matches!(data, Data::Struct(fields) if fields.is_empty())
}

impl fmt::Display for Attribute<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "@{}", self.key)?;
        match unit(self.value) {
            true => Ok(()),
            false => write!(f, " {}", Shown(self.value)),
        }
    }
}

impl Grouped for Shown<'_> {
    fn prec(&self) -> Prec {
        match self.0 {
            Data::Tag { payload, .. } if !unit(payload) => Prec::Apply,
            Data::Tag { .. } => Prec::Tag,
            _ => Prec::Atom,
        }
    }
}

impl fmt::Display for Shown<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            Data::Natural(value) => write!(f, "{value}n"),
            Data::Integer(value) => write!(f, "{value}i"),
            Data::Fixed(value) => write!(f, "{value}"),
            Data::Real(value) => write!(f, "{value}"),
            Data::String(value) => f.write_str(&string(value)),
            Data::Bool(value) => write!(f, "{value}"),
            Data::Array(items) => {
                f.write_str("[")?;
                for (index, item) in items.iter().enumerate() {
                    if index != 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{}", Shown(item))?;
                }
                f.write_str("]")
            }
            Data::Struct(fields) => {
                if fields.is_empty() {
                    f.write_str("()")
                } else if let Some(order) = tuple_field_order(fields.keys().map(String::as_str)) {
                    write_tuple(
                        f,
                        order.into_iter().map(|insertion| Shown(&fields[insertion])),
                    )
                } else {
                    write_struct(
                        f,
                        fields.iter().map(|(name, value)| (name, Shown(value))),
                        None,
                    )
                }
            }
            Data::Tag { name, payload } => write_tag(
                f,
                name,
                None,
                (!unit(payload)).then(|| Shown(payload.as_ref())),
            ),
        }
    }
}

/// Render one entry of a declaration's published metadata, `@key value`.
pub fn attribute<'a>(key: &'a str, value: &'a Data) -> impl fmt::Display + 'a {
    Attribute { key, value }
}
