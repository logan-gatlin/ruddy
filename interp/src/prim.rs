//! The portable primitive table: every contract the standard library
//! declares as `$prim.<module>.<name>`, and the array primitives, implemented
//! over the interpreter's values.
//!
//! `src/backend/primitives.js` is the contract, so this reads as a
//! translation of that table rather than as the Rust anyone would write from
//! scratch: `nat.subtract` saturates at zero because `Math.max(0, left -
//! right)` does, `real.round` sends a tie towards positive infinity because
//! `Math.round` does, `real.min` propagates NaN and orders negative zero
//! first because `Math.min` does, and every `str` index and length counts
//! Unicode scalar values because the JavaScript counts them one at a time.
//!
//! A few contracts are bound to the target's integer domains rather than to
//! JavaScript's doubles — reading a `Nat` or an `Int` back from bytes, and
//! converting a fixed width to one of them — and those are the only ones the
//! domains reach. Everything else means the same under any target.
//!
//! The contract stops short of the last bit for one family. ECMAScript
//! leaves `Math.sin` and its relatives to the implementation, so `real.sin`,
//! `cos`, `exp`, `log`, `atan2`, `asinh`, `acosh` and `atanh` are Rust's and
//! agree with node only to within an ulp, differing on a few arguments in
//! every thousand. Nothing may assert that a result of one of them is spelled
//! the same on both backends; the rest of `real`, including `sqrt`, `power`,
//! the roundings, `min` and `max`, does agree bit for bit.
//!
//! One departure from the JavaScript is deliberate: text built by
//! `str.repeat`, `str.pad_start` or `str.pad_end` is refused once it passes
//! what a JavaScript string can hold, which is a contract failure there and
//! would be exhausted memory here.

use ruddy::types::{Bounds, Domains, FixedInt};

use crate::{number, render, value::Value};

/// What a contract failure reports when a divisor is zero.
const DIVISION: &str = "division by zero";
/// What a contract failure reports when an integer leaves its domain.
const RANGE: &str = "integer out of range";
/// The longest text a repeating or padding primitive will build, in scalar
/// values. JavaScript refuses a longer string as well.
const LONGEST: u64 = 1 << 29;

/// How many arguments a primitive takes, or none for an unknown target.
pub fn arity(target: &str) -> Option<usize> {
    if let Some(rest) = target.strip_prefix("$prim.") {
        let (module, name) = rest.split_once('.')?;
        return match module {
            "binary" => binary_arity(name),
            "int" => integral_arity(true, name),
            "json" => json_arity(name),
            "nat" => integral_arity(false, name),
            "real" => real_arity(name),
            "str" => text_arity(name),
            _ => None,
        };
    }
    match target {
        "$arrayLen" | "$arrayPop" => Some(1),
        "$arrayGet" | "$arrayPush" | "$arrayConcat" | "$arrayPrepend" => Some(2),
        "$arraySet" | "$arraySlice" => Some(3),
        _ => None,
    }
}

/// Run a primitive on exactly its arity of arguments.
pub fn call(target: &str, args: &[Value], domains: Domains) -> Result<Value, String> {
    let Some(count) = arity(target) else {
        return Err(unknown(target));
    };
    if args.len() != count {
        return Err(format!(
            "`{target}` takes {count} arguments and was given {}",
            args.len()
        ));
    }
    let Some(rest) = target.strip_prefix("$prim.") else {
        return arrays(target, args);
    };
    let (module, name) = rest.split_once('.').ok_or_else(|| unknown(target))?;
    match module {
        "binary" => binary(name, args, domains),
        "int" => integral(true, name, args, domains),
        "json" => json(name, args),
        "nat" => integral(false, name, args, domains),
        "real" => real(name, args),
        "str" => text(name, args),
        _ => Err(unknown(target)),
    }
}

/// One value wrapped into a fixed width, the way `BigInt.asIntN` and
/// `BigInt.asUintN` wrap one: the low bits kept, and the highest of them
/// read as a sign where the width has one.
pub fn wrap(kind: FixedInt, value: i128) -> i128 {
    let span = 1i128 << kind.bits();
    let low = value & (span - 1);
    if low > kind.max() { low - span } else { low }
}

fn unknown(target: &str) -> String {
    format!("no implementation for `{target}`")
}

fn binary_arity(name: &str) -> Option<usize> {
    matches!(
        name,
        "real_to_bits"
            | "real_from_bits"
            | "utf8_encode"
            | "utf8_decode"
            | "nat64_to_bytes"
            | "nat64_from_bytes"
            | "int64_to_bytes"
            | "int64_from_bytes"
            | "nat_to_bytes"
            | "int_to_bytes"
            | "nat_from_bytes"
            | "int_from_bytes"
            | "nat32_to_bytes"
            | "nat32_from_bytes"
    )
    .then_some(1)
}

fn json_arity(name: &str) -> Option<usize> {
    matches!(
        name,
        "char_code"
            | "char_from_code"
            | "quote"
            | "real_value"
            | "real_token"
            | "nat_of_digits"
            | "int_of_digits"
            | "nat64_of_digits"
            | "int64_of_digits"
    )
    .then_some(1)
}

/// The arity of one name of the `int` module, or of the `nat` module. The two
/// hold the same families, but only the signed one negates and takes an
/// absolute value, and each converts to the integer type it belongs to.
fn integral_arity(signed: bool, name: &str) -> Option<usize> {
    if !signed && name == "xor64" {
        return Some(2);
    }
    let (stem, width) = widthed(name);
    let fixed = width.is_some();
    match (stem, fixed, signed) {
        (
            "add"
            | "subtract"
            | "multiply"
            | "divide"
            | "remainder"
            | "ordering_less_than"
            | "ordering_greater_than"
            | "min"
            | "max",
            _,
            _,
        ) => Some(2),
        ("clamp", _, _) => Some(3),
        ("negate" | "abs", _, true) => Some(1),
        ("from_real", false, _) => Some(1),
        ("from_nat", false, true) | ("from_int", false, false) => Some(1),
        ("from_nat" | "from_int" | "is_zero" | "to_string", true, _) => Some(1),
        ("to_int", true, true) | ("to_nat", true, false) => Some(1),
        _ => None,
    }
}

fn real_arity(name: &str) -> Option<usize> {
    match name {
        "remainder"
        | "power"
        | "ordering_equal"
        | "ordering_less_than"
        | "ordering_greater_than"
        | "min"
        | "max"
        | "atan2" => Some(2),
        "clamp" => Some(3),
        "abs" | "sqrt" | "floor" | "ceil" | "round" | "truncate" | "is_nan" | "is_finite"
        | "is_integer" | "from_nat" | "from_int" | "sin" | "cos" | "tan" | "asin" | "acos"
        | "atan" | "sinh" | "cosh" | "tanh" | "asinh" | "acosh" | "atanh" | "exp" | "expm1"
        | "log" | "log1p" | "log2" | "log10" => Some(1),
        _ => None,
    }
}

fn text_arity(name: &str) -> Option<usize> {
    match name {
        "len" | "utf8_len" | "from_nat" | "from_int" | "from_real" | "trim" | "trim_start"
        | "trim_end" | "to_lowercase" | "to_uppercase" | "reverse" => Some(1),
        "concat"
        | "ordering_less_than"
        | "ordering_greater_than"
        | "contains"
        | "starts_with"
        | "ends_with"
        | "char_at"
        | "index_of"
        | "repeat" => Some(2),
        "slice" | "replace_first" | "replace_all" | "pad_start" | "pad_end" => Some(3),
        _ => None,
    }
}

/// A name's family and the width it belongs to, where it names one: `add64`
/// is `add` at sixty-four bits, and `log2` is a family of its own.
fn widthed(name: &str) -> (&str, Option<u32>) {
    for (suffix, width) in [("64", 64u32), ("32", 32), ("16", 16), ("8", 8)] {
        if let Some(stem) = name.strip_suffix(suffix) {
            return (stem, Some(width));
        }
    }
    (name, None)
}

fn kind_of(signed: bool, bits: u32) -> FixedInt {
    match (signed, bits) {
        (false, 8) => FixedInt::Nat8,
        (false, 16) => FixedInt::Nat16,
        (false, 32) => FixedInt::Nat32,
        (false, _) => FixedInt::Nat64,
        (true, 8) => FixedInt::Int8,
        (true, 16) => FixedInt::Int16,
        (true, 32) => FixedInt::Int32,
        (true, _) => FixedInt::Int64,
    }
}

fn binary(name: &str, args: &[Value], domains: Domains) -> Result<Value, String> {
    Ok(match name {
        "real_to_bits" => Value::Fixed(FixedInt::Nat64, i128::from(args[0].as_real()?.to_bits())),
        "real_from_bits" => Value::Real(f64::from_bits(raw(args[0].as_integer()?))),
        "utf8_encode" => bytes_of(args[0].as_str()?.bytes()),
        "utf8_decode" => match String::from_utf8(bytes(&args[0])?) {
            Ok(text) => Value::some(Value::string(&text)),
            Err(_) => Value::none(),
        },
        "nat64_to_bytes" | "int64_to_bytes" | "nat_to_bytes" | "int_to_bytes" => {
            bytes_of(raw(args[0].as_integer()?).to_le_bytes())
        }
        "nat32_to_bytes" => bytes_of((raw(args[0].as_integer()?) as u32).to_le_bytes()),
        "nat64_from_bytes" => Value::Fixed(
            FixedInt::Nat64,
            i128::from(little_endian(&bytes(&args[0])?, 8)),
        ),
        "int64_from_bytes" => Value::Fixed(
            FixedInt::Int64,
            i128::from(little_endian(&bytes(&args[0])?, 8) as i64),
        ),
        "nat32_from_bytes" => Value::Nat(little_endian(&bytes(&args[0])?, 4)),
        "nat_from_bytes" => {
            let value = i128::from(little_endian(&bytes(&args[0])?, 8));
            within(domains.nat(), value, Value::Nat(value as u64))
        }
        "int_from_bytes" => {
            let value = i128::from(little_endian(&bytes(&args[0])?, 8) as i64);
            within(domains.int(), value, Value::Int(value as i64))
        }
        _ => return Err(unknown(&format!("$prim.binary.{name}"))),
    })
}

fn json(name: &str, args: &[Value]) -> Result<Value, String> {
    Ok(match name {
        // The first UTF-16 code unit of the text, which is a high surrogate
        // where the first scalar value is outside the basic plane.
        "char_code" => Value::Nat(u64::from(args[0].as_str()?.chars().next().map_or(
            0,
            |ch| {
                let point = u32::from(ch);
                if point > 0xffff {
                    0xd800 + ((point - 0x10000) >> 10)
                } else {
                    point
                }
            },
        ))),
        "char_from_code" => {
            let point = u32::try_from(args[0].as_integer()?).unwrap_or(u32::MAX);
            Value::string(&char::from_u32(point).unwrap_or('\u{fffd}').to_string())
        }
        "quote" => {
            let mut out = String::new();
            render::string(args[0].as_str()?, &mut out);
            Value::string(&out)
        }
        "real_value" => Value::Real(number::value(args[0].as_str()?)),
        "real_token" => {
            let value = args[0].as_real()?;
            if value == 0.0 && value.is_sign_negative() {
                Value::string("-0.0")
            } else {
                Value::string(&number::to_string(value))
            }
        }
        "nat_of_digits" => Value::Nat(number::natural(args[0].as_str()?)),
        "int_of_digits" => Value::Int(number::integer(args[0].as_str()?)),
        "nat64_of_digits" => Value::Fixed(
            FixedInt::Nat64,
            i128::from(number::sixty_four(args[0].as_str()?)),
        ),
        "int64_of_digits" => Value::Fixed(
            FixedInt::Int64,
            i128::from(number::sixty_four(args[0].as_str()?) as i64),
        ),
        _ => return Err(unknown(&format!("$prim.json.{name}"))),
    })
}

/// One name of the `int` module, or of the `nat` module: a fixed width where
/// the name carries one, and the target's own width otherwise.
fn integral(signed: bool, name: &str, args: &[Value], domains: Domains) -> Result<Value, String> {
    match widthed(name) {
        (stem, Some(bits)) => fixed(kind_of(signed, bits), stem, args, domains),
        (stem, None) if signed => integers(stem, args),
        (stem, None) => naturals(stem, args),
    }
}

/// The `nat` module at the target's own width. Addition and multiplication
/// wrap, subtraction saturates at zero, and division truncates.
fn naturals(name: &str, args: &[Value]) -> Result<Value, String> {
    // The conversions take something other than a natural number, so they
    // come before one is read out of the first argument.
    if matches!(name, "from_int" | "from_real") {
        return Ok(Value::Nat(match name {
            "from_int" => u64::try_from(args[0].as_integer()?.max(0)).unwrap_or(u64::MAX),
            _ => args[0].as_real()? as u64,
        }));
    }
    let left = natural(&args[0])?;
    Ok(match name {
        "add" => Value::Nat(left.wrapping_add(natural(&args[1])?)),
        "subtract" => Value::Nat(left.saturating_sub(natural(&args[1])?)),
        "multiply" => Value::Nat(left.wrapping_mul(natural(&args[1])?)),
        "divide" => Value::Nat(left / divisor(natural(&args[1])?)?),
        "remainder" => Value::Nat(left % divisor(natural(&args[1])?)?),
        "ordering_less_than" => Value::Bool(left < natural(&args[1])?),
        "ordering_greater_than" => Value::Bool(left > natural(&args[1])?),
        "min" => Value::Nat(left.min(natural(&args[1])?)),
        "max" => Value::Nat(left.max(natural(&args[1])?)),
        "clamp" => Value::Nat(left.max(natural(&args[1])?).min(natural(&args[2])?)),
        _ => return Err(unknown(&format!("$prim.nat.{name}"))),
    })
}

/// The `int` module at the target's own width. Everything but division wraps.
fn integers(name: &str, args: &[Value]) -> Result<Value, String> {
    // As in `naturals`, the conversions read their argument their own way.
    if matches!(name, "from_nat" | "from_real") {
        return Ok(Value::Int(match name {
            "from_nat" => i64::try_from(args[0].as_integer()?).map_err(|_| RANGE.to_string())?,
            _ => args[0].as_real()? as i64,
        }));
    }
    let left = integer(&args[0])?;
    Ok(match name {
        "add" => Value::Int(left.wrapping_add(integer(&args[1])?)),
        "subtract" => Value::Int(left.wrapping_sub(integer(&args[1])?)),
        "multiply" => Value::Int(left.wrapping_mul(integer(&args[1])?)),
        "divide" => Value::Int(left.wrapping_div(divisor(integer(&args[1])?)?)),
        "remainder" => Value::Int(left.wrapping_rem(divisor(integer(&args[1])?)?)),
        "negate" => Value::Int(left.wrapping_neg()),
        "abs" => Value::Int(left.wrapping_abs()),
        "ordering_less_than" => Value::Bool(left < integer(&args[1])?),
        "ordering_greater_than" => Value::Bool(left > integer(&args[1])?),
        "min" => Value::Int(left.min(integer(&args[1])?)),
        "max" => Value::Int(left.max(integer(&args[1])?)),
        "clamp" => Value::Int(left.max(integer(&args[1])?).min(integer(&args[2])?)),
        _ => return Err(unknown(&format!("$prim.int.{name}"))),
    })
}

/// One name of either integer module at a fixed width. Every result is
/// wrapped back into the width, which is what the JavaScript's shifts and
/// `BigInt.asIntN` do; only leaving the width for the target's own integers
/// is checked, and that against the target's domain.
fn fixed(kind: FixedInt, name: &str, args: &[Value], domains: Domains) -> Result<Value, String> {
    let left = args[0].as_integer()?;
    let signed = kind.signed();
    let whole = |value: i128| Value::Fixed(kind, wrap(kind, value));
    Ok(match (name, signed) {
        ("xor", false) if kind == FixedInt::Nat64 => whole(left ^ args[1].as_integer()?),
        ("add", _) => whole(left + args[1].as_integer()?),
        ("subtract", _) => whole(left - args[1].as_integer()?),
        ("multiply", _) => whole(left.wrapping_mul(args[1].as_integer()?)),
        ("divide", _) => whole(left / divisor(args[1].as_integer()?)?),
        ("remainder", _) => whole(left % divisor(args[1].as_integer()?)?),
        ("negate", true) => whole(-left),
        ("abs", true) => whole(if left < 0 { -left } else { left }),
        ("ordering_less_than", _) => Value::Bool(left < args[1].as_integer()?),
        ("ordering_greater_than", _) => Value::Bool(left > args[1].as_integer()?),
        ("min", _) => whole(left.min(args[1].as_integer()?)),
        ("max", _) => whole(left.max(args[1].as_integer()?)),
        // A fixed width clamps by comparing against each bound in turn,
        // where the target's own widths clamp by `Math.min` of `Math.max`.
        // The two part ways only where the bounds cross.
        ("clamp", _) => {
            let (low, high) = (args[1].as_integer()?, args[2].as_integer()?);
            whole(if left < low {
                low
            } else if left > high {
                high
            } else {
                left
            })
        }
        ("is_zero", _) => Value::Bool(left == 0),
        ("to_string", _) => Value::string(&left.to_string()),
        ("from_nat" | "from_int", _) => whole(left),
        ("to_int", true) if holds(domains.int(), left) => Value::Int(left as i64),
        ("to_nat", false) if holds(domains.nat(), left) => Value::Nat(left as u64),
        ("to_int", true) | ("to_nat", false) => return Err(RANGE.to_string()),
        _ => {
            let module = if signed { "int" } else { "nat" };
            return Err(unknown(&format!("$prim.{module}.{name}{}", kind.bits())));
        }
    })
}

fn real(name: &str, args: &[Value]) -> Result<Value, String> {
    let value = args[0].as_real()?;
    Ok(match name {
        "remainder" => Value::Real(value % args[1].as_real()?),
        "abs" => Value::Real(value.abs()),
        "sqrt" => Value::Real(value.sqrt()),
        "power" => Value::Real(power(value, args[1].as_real()?)),
        "ordering_equal" => Value::Bool(same(value, args[1].as_real()?)),
        "ordering_less_than" => Value::Bool(value < args[1].as_real()?),
        "ordering_greater_than" => Value::Bool(value > args[1].as_real()?),
        "min" => Value::Real(least(value, args[1].as_real()?)),
        "max" => Value::Real(greatest(value, args[1].as_real()?)),
        "clamp" => Value::Real(least(
            greatest(value, args[1].as_real()?),
            args[2].as_real()?,
        )),
        "floor" => Value::Int(value.floor() as i64),
        "ceil" => Value::Int(value.ceil() as i64),
        "round" => Value::Int(round(value) as i64),
        "truncate" => Value::Int(value.trunc() as i64),
        "is_nan" => Value::Bool(value.is_nan()),
        "is_finite" => Value::Bool(value.is_finite()),
        "is_integer" => Value::Bool(value.is_finite() && value.fract() == 0.0),
        "from_nat" | "from_int" => Value::Real(value),
        "sin" => Value::Real(value.sin()),
        "cos" => Value::Real(value.cos()),
        "tan" => Value::Real(value.tan()),
        "asin" => Value::Real(value.asin()),
        "acos" => Value::Real(value.acos()),
        "atan" => Value::Real(value.atan()),
        "atan2" => Value::Real(value.atan2(args[1].as_real()?)),
        "sinh" => Value::Real(value.sinh()),
        "cosh" => Value::Real(value.cosh()),
        "tanh" => Value::Real(value.tanh()),
        "asinh" => Value::Real(value.asinh()),
        "acosh" => Value::Real(value.acosh()),
        "atanh" => Value::Real(value.atanh()),
        "exp" => Value::Real(value.exp()),
        "expm1" => Value::Real(value.exp_m1()),
        "log" => Value::Real(value.ln()),
        "log1p" => Value::Real(value.ln_1p()),
        "log2" => Value::Real(value.log2()),
        "log10" => Value::Real(value.log10()),
        _ => return Err(unknown(&format!("$prim.real.{name}"))),
    })
}

/// One name of the `str` module. Every index and length here counts Unicode
/// scalar values, never UTF-16 code units and never bytes.
fn text(name: &str, args: &[Value]) -> Result<Value, String> {
    match name {
        "from_nat" | "from_int" => return Ok(Value::string(&args[0].as_integer()?.to_string())),
        "from_real" => return Ok(Value::string(&number::to_string(args[0].as_real()?))),
        _ => {}
    }
    let value = args[0].as_str()?;
    Ok(match name {
        "concat" => Value::string(&format!("{value}{}", args[1].as_str()?)),
        "len" => Value::Nat(value.chars().count() as u64),
        // A Rust string is already UTF-8, so its own length is the count.
        "utf8_len" => Value::Nat(value.len() as u64),
        "ordering_less_than" => Value::Bool(value < args[1].as_str()?),
        "ordering_greater_than" => Value::Bool(value > args[1].as_str()?),
        "contains" => Value::Bool(value.contains(args[1].as_str()?)),
        "starts_with" => Value::Bool(value.starts_with(args[1].as_str()?)),
        "ends_with" => Value::Bool(value.ends_with(args[1].as_str()?)),
        "char_at" => {
            let mut out = String::new();
            out.extend(value.chars().nth(place(&args[1])?));
            Value::string(&out)
        }
        "index_of" => Value::Int(match value.find(args[1].as_str()?) {
            Some(at) => i64::try_from(value[..at].chars().count()).unwrap_or(i64::MAX),
            None => -1,
        }),
        "slice" => {
            let scalars: Vec<char> = value.chars().collect();
            let start = place(&args[1])?.min(scalars.len());
            let end = place(&args[2])?.min(scalars.len());
            Value::string(&scalars[start..end.max(start)].iter().collect::<String>())
        }
        "trim" => Value::string(value.trim_matches(number::space)),
        "trim_start" => Value::string(value.trim_start_matches(number::space)),
        "trim_end" => Value::string(value.trim_end_matches(number::space)),
        "to_lowercase" => Value::string(&value.to_lowercase()),
        "to_uppercase" => Value::string(&value.to_uppercase()),
        "repeat" => {
            let count = natural(&args[1])?;
            let scalars = value.chars().count() as u64;
            if scalars.saturating_mul(count) > LONGEST {
                return Err("the text is too long".to_string());
            }
            Value::string(&value.repeat(count as usize))
        }
        // An empty search matches at every scalar boundary; Rust's own
        // `replace` matches at every char boundary, which is the same thing
        // here, and `replacen` with one matches once at the front.
        "replace_first" => Value::string(&value.replacen(args[1].as_str()?, args[2].as_str()?, 1)),
        "replace_all" => Value::string(&value.replace(args[1].as_str()?, args[2].as_str()?)),
        "pad_start" => {
            let padding = padding(value, &args[1], &args[2])?;
            Value::string(&format!("{padding}{value}"))
        }
        "pad_end" => {
            let padding = padding(value, &args[1], &args[2])?;
            Value::string(&format!("{value}{padding}"))
        }
        "reverse" => Value::string(&value.chars().rev().collect::<String>()),
        _ => return Err(unknown(&format!("$prim.str.{name}"))),
    })
}

/// The array primitives, which are not part of the table: the backend has
/// them in its own runtime, over its own representation of an array.
fn arrays(target: &str, args: &[Value]) -> Result<Value, String> {
    let items = args[0].as_array()?;
    Ok(match target {
        "$arrayLen" => Value::Nat(items.len() as u64),
        "$arrayGet" => match items.get(place(&args[1])?) {
            Some(item) => Value::some(item.clone()),
            None => Value::none(),
        },
        "$arraySet" => {
            let at = place(&args[1])?;
            if at >= items.len() {
                Value::none()
            } else {
                let mut changed = items.as_ref().clone();
                changed[at] = args[2].clone();
                Value::some(Value::array(changed))
            }
        }
        "$arrayPush" => {
            let mut changed = items.as_ref().clone();
            changed.push(args[1].clone());
            Value::array(changed)
        }
        "$arrayConcat" => {
            let mut changed = items.as_ref().clone();
            changed.extend(args[1].as_array()?.iter().cloned());
            Value::array(changed)
        }
        "$arraySlice" => {
            let (from, to) = (place(&args[1])?, place(&args[2])?);
            if from > to || to > items.len() {
                Value::none()
            } else {
                Value::some(Value::array(items[from..to].to_vec()))
            }
        }
        "$arrayPrepend" => {
            let mut changed = Vec::with_capacity(items.len() + 1);
            changed.push(args[1].clone());
            changed.extend(items.iter().cloned());
            Value::array(changed)
        }
        "$arrayPop" => match items.split_last() {
            Some((last, rest)) => {
                Value::some(Value::pair(last.clone(), Value::array(rest.to_vec())))
            }
            None => Value::none(),
        },
        _ => return Err(unknown(target)),
    })
}

/// One argument as a natural number of the target's width. An integer of
/// another width is read as the same number, as it would be in JavaScript,
/// and anything but a number is refused.
fn natural(value: &Value) -> Result<u64, String> {
    u64::try_from(value.as_integer()?).map_err(|_| RANGE.to_string())
}

/// One argument as a signed integer of the target's width.
fn integer(value: &Value) -> Result<i64, String> {
    i64::try_from(value.as_integer()?).map_err(|_| RANGE.to_string())
}

/// One argument as a position in text or in an array. A position past
/// everything the interpreter can hold is past the end, which every caller
/// already handles.
fn place(value: &Value) -> Result<usize, String> {
    Ok(usize::try_from(natural(value)?).unwrap_or(usize::MAX))
}

/// A divisor, or the contract failure a zero one is.
fn divisor<T: Default + PartialEq>(value: T) -> Result<T, String> {
    if value == T::default() {
        return Err(DIVISION.to_string());
    }
    Ok(value)
}

/// The low sixty-four bits of a value, as two's complement where it is
/// negative: what `BigInt.asUintN(64, value)` reads.
fn raw(value: i128) -> u64 {
    value as u64
}

/// A little-endian number of up to `count` bytes. Bytes past the count are
/// left out, as the JavaScript leaves them out by never reading them.
fn little_endian(bytes: &[u8], count: usize) -> u64 {
    let mut value: u64 = 0;
    for (index, byte) in bytes.iter().take(count).enumerate() {
        value |= u64::from(*byte) << (index * 8);
    }
    value
}

fn bytes(value: &Value) -> Result<Vec<u8>, String> {
    value
        .as_array()?
        .iter()
        .map(|item| Ok(raw(item.as_integer()?) as u8))
        .collect()
}

fn bytes_of(bytes: impl IntoIterator<Item = u8>) -> Value {
    Value::array(
        bytes
            .into_iter()
            .map(|byte| Value::Fixed(FixedInt::Nat8, i128::from(byte)))
            .collect(),
    )
}

/// Whether a value is within an integer domain.
fn holds(bounds: Bounds, value: i128) -> bool {
    let low: i128 = bounds.min.parse().expect("a domain's bounds are decimal");
    let high: i128 = bounds.max.parse().expect("a domain's bounds are decimal");
    (low..=high).contains(&value)
}

/// A value where it is within an integer domain, and nothing where it is not.
fn within(bounds: Bounds, value: i128, held: Value) -> Value {
    if holds(bounds, value) {
        Value::some(held)
    } else {
        Value::none()
    }
}

/// The padding one of the padding primitives puts on: the fill's scalar
/// values over and over until the text reaches the length, and nothing at all
/// where the fill is empty.
fn padding(value: &str, length: &Value, fill: &Value) -> Result<String, String> {
    let length = natural(length)?;
    if length > LONGEST {
        return Err("the text is too long".to_string());
    }
    let fill: Vec<char> = fill.as_str()?.chars().collect();
    let mut count = value.chars().count() as u64;
    let mut out = String::new();
    if !fill.is_empty() {
        let mut at = 0;
        while count < length {
            out.push(fill[at % fill.len()]);
            at += 1;
            count += 1;
        }
    }
    Ok(out)
}

/// `Math.pow`, which is not Rust's `powf` where the base is one and the
/// exponent is not a number or is infinite.
fn power(base: f64, exponent: f64) -> f64 {
    if exponent.is_nan() {
        return f64::NAN;
    }
    if base.abs() == 1.0 && exponent.is_infinite() {
        return f64::NAN;
    }
    base.powf(exponent)
}

/// `Object.is` for two numbers: NaN is itself, and the two zeros differ.
fn same(left: f64, right: f64) -> bool {
    if left.is_nan() || right.is_nan() {
        return left.is_nan() && right.is_nan();
    }
    left == right && left.is_sign_negative() == right.is_sign_negative()
}

/// `Math.min`, which is not Rust's `min`: NaN spreads, and negative zero is
/// below zero.
fn least(left: f64, right: f64) -> f64 {
    if left.is_nan() || right.is_nan() {
        f64::NAN
    } else if left < right || (left == right && left.is_sign_negative()) {
        left
    } else {
        right
    }
}

/// `Math.max`, the same way round.
fn greatest(left: f64, right: f64) -> f64 {
    if left.is_nan() || right.is_nan() {
        f64::NAN
    } else if left > right || (left == right && right.is_sign_negative()) {
        left
    } else {
        right
    }
}

/// `Math.round`, which sends a tie towards positive infinity where Rust's
/// `round` sends one away from zero.
fn round(value: f64) -> f64 {
    if !value.is_finite() {
        return value;
    }
    let below = value.floor();
    if value - below >= 0.5 {
        below + 1.0
    } else {
        below
    }
}
