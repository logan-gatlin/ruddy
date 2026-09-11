//! Values as text, for the command line and for comparing runs.

use crate::value::{Key, Value};

/// A value as JSON text where it has one: records as objects, arrays as
/// arrays, sums as objects with a `tag` and, when there is one, a `value`,
/// fixed-width integers of sixty-four bits as decimal strings, and
/// everything else as a description in a string.
pub fn json(value: &Value) -> String {
    let mut out = String::new();
    write(value, &mut out);
    out
}

fn write(value: &Value, out: &mut String) {
    match value {
        Value::Absent => out.push_str("null"),
        Value::Nat(value) => out.push_str(&value.to_string()),
        Value::Int(value) => out.push_str(&value.to_string()),
        Value::Fixed(kind, value) if kind.bits() == 64 => {
            out.push('"');
            out.push_str(&value.to_string());
            out.push('"');
        }
        Value::Fixed(_, value) => out.push_str(&value.to_string()),
        Value::Real(value) if value.is_finite() => out.push_str(&crate::number::to_string(*value)),
        Value::Real(_) => out.push_str("null"),
        Value::Str(value) => string(value, out),
        Value::Bool(value) => out.push_str(if *value { "true" } else { "false" }),
        Value::Record(record) => {
            out.push('{');
            for (index, (key, value)) in record.iter().enumerate() {
                if index != 0 {
                    out.push(',');
                }
                match key {
                    Key::Named(name) => string(name, out),
                    Key::Unnamed => out.push_str("\"\""),
                }
                out.push(':');
                write(value, out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index != 0 {
                    out.push(',');
                }
                write(item, out);
            }
            out.push(']');
        }
        Value::Sum(sum) => {
            out.push_str("{\"tag\":");
            string(&sum.tag, out);
            if let Some(payload) = &sum.payload {
                out.push_str(",\"value\":");
                write(payload, out);
            }
            out.push('}');
        }
        // A sealed package shows nothing of what it holds, which is the
        // whole of its contract at a boundary.
        Value::Package(_) => out.push_str("{}"),
        other => string(&format!("<{}>", other.kind()), out),
    }
}

/// A JSON string literal, escaped the way `JSON.stringify` escapes one.
pub fn string(value: &str, out: &mut String) {
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
            ch if ch < '\u{20}' => {
                use std::fmt::Write;
                write!(out, "\\u{:04x}", ch as u32).expect("writing a String cannot fail");
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}
