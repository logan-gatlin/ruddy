//! Tests for the reference interpreter's primitive contracts.
//!
//! `src/backend/primitives.js` is what these check the interpreter against:
//! every expectation here is what the JavaScript backend produces for the
//! same arguments, except where a contract is bound to the target's integer
//! domains instead, which the JavaScript reads out of `$domains`.

use std::{fs, path::Path};

use ruddy::types::{Domains, FixedInt};
use ruddy_interp::{Value, number, prim, render};

/// Run a primitive under the default domains, which must not fail.
fn call(target: &str, args: &[Value]) -> Value {
    under(Domains::Js53, target, args)
}

/// Run a primitive under named domains, which must not fail.
fn under(domains: Domains, target: &str, args: &[Value]) -> Value {
    prim::call(target, args, domains).unwrap_or_else(|error| panic!("{target}: {error}"))
}

/// The message a primitive fails with.
fn fails(target: &str, args: &[Value]) -> String {
    prim::call(target, args, Domains::Js53)
        .err()
        .unwrap_or_else(|| panic!("{target} was expected to fail"))
}

/// A primitive's result as JSON, for the values made of other values.
fn json(target: &str, args: &[Value]) -> String {
    render::json(&call(target, args))
}

fn nat(value: Value) -> u64 {
    value.as_nat().expect("a natural number")
}

fn int(value: Value) -> i64 {
    value.as_int().expect("an integer")
}

fn real(value: Value) -> f64 {
    value.as_real().expect("a real number")
}

fn text(value: Value) -> String {
    value.as_str().expect("a string").to_string()
}

fn truth(value: Value) -> bool {
    value.as_bool().expect("a boolean")
}

fn width(value: Value) -> (FixedInt, i128) {
    match value {
        Value::Fixed(kind, raw) => (kind, raw),
        other => panic!("expected a fixed-width integer, found {}", other.kind()),
    }
}

fn nat8(byte: u8) -> Value {
    Value::Fixed(FixedInt::Nat8, i128::from(byte))
}

/// The eight little-endian bytes of a sixty-four bit number, as an argument.
fn bytes(raw: u64) -> Value {
    Value::array(raw.to_le_bytes().into_iter().map(nat8).collect())
}

#[test]
fn every_primitive_the_backend_declares_has_an_arity() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the workspace root");
    let table = fs::read_to_string(root.join("src/backend/primitives.js"))
        .expect("the portable primitive table");
    let mut module = String::new();
    let mut targets = Vec::new();
    for line in table.lines() {
        if let Some(name) = line
            .strip_prefix("  ")
            .and_then(|rest| rest.strip_suffix(": {"))
        {
            module = name.to_string();
        } else if let Some(name) = line
            .strip_prefix("    ")
            .and_then(|rest| rest.strip_suffix(": ("))
        {
            targets.push(format!("$prim.{module}.{name}"));
        }
    }
    assert_eq!(targets.len(), 239, "the table's size changed");
    for target in &targets {
        let arity = prim::arity(target)
            .unwrap_or_else(|| panic!("{target} has no implementation in the interpreter"));
        assert!((1..=3).contains(&arity), "{target} takes {arity} arguments");
    }
    assert_eq!(prim::arity("$arrayLen"), Some(1));
    assert_eq!(prim::arity("$arrayPop"), Some(1));
    assert_eq!(prim::arity("$arrayGet"), Some(2));
    assert_eq!(prim::arity("$arrayPush"), Some(2));
    assert_eq!(prim::arity("$arrayConcat"), Some(2));
    assert_eq!(prim::arity("$arrayPrepend"), Some(2));
    assert_eq!(prim::arity("$arraySet"), Some(3));
    assert_eq!(prim::arity("$arraySlice"), Some(3));
}

#[test]
fn a_target_outside_the_table_is_not_a_primitive() {
    for target in [
        "$prim.str.slurp",
        "$prim.nat.negate",
        "$prim.nat.negate8",
        "$prim.int.to_nat8",
        "$prim.int.to_string",
        "$prim.real.frobnicate",
        "$prim.magic.add",
        "$prim",
        "$prim.str",
        "$arrayZip",
        "console.log",
    ] {
        assert_eq!(prim::arity(target), None, "{target}");
        let message = prim::call(target, &[], Domains::Js53).unwrap_err();
        assert_eq!(message, format!("no implementation for `{target}`"));
    }
    let message = prim::call("$prim.nat.add", &[Value::Nat(1)], Domains::Js53).unwrap_err();
    assert_eq!(message, "`$prim.nat.add` takes 2 arguments and was given 1");
    assert_eq!(
        fails("$prim.str.len", &[Value::Nat(1)]),
        "expected a string, found a natural number"
    );
}

#[test]
fn target_width_naturals_saturate_at_zero_and_divide_towards_it() {
    let pair = |left: u64, right: u64| [Value::Nat(left), Value::Nat(right)];
    // Subtraction saturating at zero is the standard library's own contract,
    // not a target's accident, so it is pinned. Overflow and a zero divisor
    // are undefined for a target-width natural number and nothing here says
    // what they do: the two backends are free to differ, and do.
    assert_eq!(nat(call("$prim.nat.subtract", &pair(3, 5))), 0);
    assert_eq!(nat(call("$prim.nat.subtract", &pair(5, 3))), 2);
    assert_eq!(nat(call("$prim.nat.divide", &pair(7, 2))), 3);
    assert_eq!(nat(call("$prim.nat.remainder", &pair(7, 2))), 1);
    assert!(truth(call("$prim.nat.ordering_less_than", &pair(1, 2))));
    assert!(!truth(call("$prim.nat.ordering_greater_than", &pair(1, 2))));
    assert_eq!(nat(call("$prim.nat.min", &pair(1, 2))), 1);
    assert_eq!(nat(call("$prim.nat.max", &pair(1, 2))), 2);
    assert_eq!(
        nat(call(
            "$prim.nat.clamp",
            &[Value::Nat(9), Value::Nat(1), Value::Nat(3)]
        )),
        3
    );
    // The target's own width clamps by `Math.min` of `Math.max`, so a value
    // below crossed bounds comes out at the upper one.
    assert_eq!(
        nat(call(
            "$prim.nat.clamp",
            &[Value::Nat(0), Value::Nat(5), Value::Nat(3)]
        )),
        3
    );
    assert_eq!(nat(call("$prim.nat.from_int", &[Value::Int(-4)])), 0);
    assert_eq!(nat(call("$prim.nat.from_int", &[Value::Int(4)])), 4);
    assert_eq!(nat(call("$prim.nat.from_real", &[Value::Real(2.9)])), 2);
    assert_eq!(nat(call("$prim.nat.from_real", &[Value::Real(-2.9)])), 0);
    assert_eq!(
        nat(call("$prim.nat.from_real", &[Value::Real(f64::NAN)])),
        0
    );
    assert_eq!(
        nat(call("$prim.nat.from_real", &[Value::Real(1e30)])),
        u64::MAX
    );
}

#[test]
fn target_width_integers_truncate_towards_zero() {
    let pair = |left: i64, right: i64| [Value::Int(left), Value::Int(right)];
    // Truncation towards zero is defined and pinned. Overflow, a zero
    // divisor, and negating the least integer are undefined for a
    // target-width integer, so nothing here says what they do.
    assert_eq!(int(call("$prim.int.divide", &pair(-7, 2))), -3);
    assert_eq!(int(call("$prim.int.remainder", &pair(-7, 2))), -1);
    assert_eq!(int(call("$prim.int.negate", &[Value::Int(5)])), -5);
    assert_eq!(int(call("$prim.int.abs", &[Value::Int(-5)])), 5);
    assert_eq!(
        int(call(
            "$prim.int.clamp",
            &[Value::Int(-9), Value::Int(-3), Value::Int(3)]
        )),
        -3
    );
    assert_eq!(int(call("$prim.int.min", &pair(-1, 2))), -1);
    assert_eq!(int(call("$prim.int.max", &pair(-1, 2))), 2);
    assert_eq!(int(call("$prim.int.from_nat", &[Value::Nat(4)])), 4);
    assert_eq!(
        fails("$prim.int.from_nat", &[Value::Nat(u64::MAX)]),
        "integer out of range"
    );
    assert_eq!(int(call("$prim.int.from_real", &[Value::Real(-2.9)])), -2);
    // An integer has one zero, on every target, and a real number that
    // truncates to zero from below reaches that one.
    assert_eq!(int(call("$prim.int.from_real", &[Value::Real(-0.5)])), 0);
}

#[test]
fn fixed_width_arithmetic_wraps_at_the_edges() {
    let of = |kind: FixedInt, value: i128| Value::Fixed(kind, value);
    assert_eq!(
        width(call(
            "$prim.int.add8",
            &[of(FixedInt::Int8, 127), of(FixedInt::Int8, 1)]
        )),
        (FixedInt::Int8, -128)
    );
    assert_eq!(
        width(call(
            "$prim.nat.subtract8",
            &[of(FixedInt::Nat8, 0), of(FixedInt::Nat8, 1)]
        ))
        .1,
        255
    );
    assert_eq!(
        width(call(
            "$prim.int.negate64",
            &[of(FixedInt::Int64, i128::from(i64::MIN))]
        ))
        .1,
        i128::from(i64::MIN)
    );
    assert_eq!(
        width(call(
            "$prim.int.abs64",
            &[of(FixedInt::Int64, i128::from(i64::MIN))]
        ))
        .1,
        i128::from(i64::MIN)
    );
    assert_eq!(
        width(call(
            "$prim.nat.multiply64",
            &[
                of(FixedInt::Nat64, i128::from(u64::MAX)),
                of(FixedInt::Nat64, i128::from(u64::MAX))
            ]
        ))
        .1,
        1
    );
    assert_eq!(
        width(call(
            "$prim.nat.multiply32",
            &[of(FixedInt::Nat32, 65536), of(FixedInt::Nat32, 65536)]
        ))
        .1,
        0
    );
    assert_eq!(
        width(call(
            "$prim.int.divide8",
            &[of(FixedInt::Int8, -128), of(FixedInt::Int8, -1)]
        ))
        .1,
        -128
    );
    assert_eq!(
        width(call(
            "$prim.int.remainder16",
            &[of(FixedInt::Int16, -7), of(FixedInt::Int16, 2)]
        ))
        .1,
        -1
    );
    for target in ["$prim.nat.divide8", "$prim.int.remainder32"] {
        let kind = if target.contains("nat") {
            FixedInt::Nat8
        } else {
            FixedInt::Int32
        };
        assert_eq!(
            fails(target, &[of(kind, 1), of(kind, 0)]),
            "division by zero"
        );
    }
    assert!(truth(call(
        "$prim.int.ordering_less_than8",
        &[of(FixedInt::Int8, -1), of(FixedInt::Int8, 0)]
    )));
    assert!(truth(call(
        "$prim.nat.is_zero16",
        &[of(FixedInt::Nat16, 0)]
    )));
    assert_eq!(
        width(call(
            "$prim.int.clamp8",
            &[
                of(FixedInt::Int8, 9),
                of(FixedInt::Int8, -3),
                of(FixedInt::Int8, 3)
            ]
        ))
        .1,
        3
    );
    // Where the bounds cross, a fixed width keeps the value below the lower
    // bound at that bound, which is not what `Math.min` of `Math.max` gives.
    assert_eq!(
        width(call(
            "$prim.nat.clamp8",
            &[
                of(FixedInt::Nat8, 0),
                of(FixedInt::Nat8, 5),
                of(FixedInt::Nat8, 3)
            ]
        ))
        .1,
        5
    );
    assert_eq!(
        width(call(
            "$prim.nat.min8",
            &[of(FixedInt::Nat8, 9), of(FixedInt::Nat8, 3)]
        ))
        .1,
        3
    );
    assert_eq!(
        width(call(
            "$prim.nat.max8",
            &[of(FixedInt::Nat8, 9), of(FixedInt::Nat8, 3)]
        ))
        .1,
        9
    );
    assert_eq!(
        text(call(
            "$prim.nat.to_string64",
            &[of(FixedInt::Nat64, i128::from(u64::MAX))]
        )),
        "18446744073709551615"
    );
    assert_eq!(
        text(call("$prim.int.to_string8", &[of(FixedInt::Int8, -128)])),
        "-128"
    );
    // A `Nat` or an `Int` narrows into a width by wrapping, both ways round.
    assert_eq!(
        width(call("$prim.nat.from_int8", &[Value::Int(-1)])),
        (FixedInt::Nat8, 255)
    );
    assert_eq!(
        width(call("$prim.int.from_nat8", &[Value::Nat(255)])),
        (FixedInt::Int8, -1)
    );
    assert_eq!(
        width(call("$prim.nat.from_nat32", &[Value::Nat(1 << 33)])),
        (FixedInt::Nat32, 0)
    );
    assert_eq!(
        width(call("$prim.int.from_int16", &[Value::Int(32768)])),
        (FixedInt::Int16, -32768)
    );
}

#[test]
fn widening_out_of_a_fixed_width_follows_the_target_domains() {
    let wide = || Value::Fixed(FixedInt::Nat64, 1 << 40);
    assert_eq!(
        nat(under(Domains::Bits64, "$prim.nat.to_nat64", &[wide()])),
        1 << 40
    );
    assert_eq!(
        nat(under(Domains::Js53, "$prim.nat.to_nat64", &[wide()])),
        1 << 40
    );
    assert_eq!(
        prim::call("$prim.nat.to_nat64", &[wide()], Domains::Bits32).unwrap_err(),
        "integer out of range"
    );
    let deep = || Value::Fixed(FixedInt::Int64, -(1i128 << 40));
    assert_eq!(
        int(under(Domains::Bits64, "$prim.int.to_int64", &[deep()])),
        -(1i64 << 40)
    );
    assert_eq!(
        prim::call("$prim.int.to_int64", &[deep()], Domains::Bits32).unwrap_err(),
        "integer out of range"
    );
    // A narrow width is inside every domain.
    assert_eq!(
        nat(under(
            Domains::Bits32,
            "$prim.nat.to_nat8",
            &[Value::Fixed(FixedInt::Nat8, 200)]
        )),
        200
    );
    assert_eq!(
        int(under(
            Domains::Bits32,
            "$prim.int.to_int16",
            &[Value::Fixed(FixedInt::Int16, -200)]
        )),
        -200
    );
    // The fifty-three bit domain is narrower than sixty-four bits.
    let huge = || Value::Fixed(FixedInt::Nat64, i128::from(u64::MAX));
    assert_eq!(
        prim::call("$prim.nat.to_nat64", &[huge()], Domains::Js53).unwrap_err(),
        "integer out of range"
    );
    assert_eq!(
        nat(under(Domains::Bits64, "$prim.nat.to_nat64", &[huge()])),
        u64::MAX
    );
}

#[test]
fn bytes_and_bits_carry_numbers_whole() {
    assert_eq!(
        width(call("$prim.binary.real_to_bits", &[Value::Real(1.0)])),
        (FixedInt::Nat64, 4607182418800017408)
    );
    assert_eq!(
        real(call(
            "$prim.binary.real_from_bits",
            &[Value::Fixed(FixedInt::Nat64, 4607182418800017408)]
        )),
        1.0
    );
    assert!(
        real(call(
            "$prim.binary.real_from_bits",
            &[Value::Fixed(
                FixedInt::Nat64,
                i128::from(f64::NAN.to_bits())
            )]
        ))
        .is_nan()
    );
    assert_eq!(
        json(
            "$prim.binary.utf8_encode",
            &[Value::string("h\u{e9}\u{1d11e}")]
        ),
        "[104,195,169,240,157,132,158]"
    );
    assert_eq!(
        json(
            "$prim.binary.utf8_decode",
            &[Value::array(
                [104, 195, 169].into_iter().map(nat8).collect()
            )]
        ),
        "{\"tag\":\"Some\",\"value\":\"h\u{e9}\"}"
    );
    assert_eq!(
        json(
            "$prim.binary.utf8_decode",
            &[Value::array([0xff, 0xfe].into_iter().map(nat8).collect())]
        ),
        "{\"tag\":\"None\"}"
    );
    assert_eq!(
        json(
            "$prim.binary.nat64_to_bytes",
            &[Value::Fixed(FixedInt::Nat64, 1)]
        ),
        "[1,0,0,0,0,0,0,0]"
    );
    assert_eq!(
        json("$prim.binary.int_to_bytes", &[Value::Int(-1)]),
        "[255,255,255,255,255,255,255,255]"
    );
    assert_eq!(
        json("$prim.binary.nat_to_bytes", &[Value::Nat(258)]),
        "[2,1,0,0,0,0,0,0]"
    );
    assert_eq!(
        json("$prim.binary.nat32_to_bytes", &[Value::Nat(0x01020304)]),
        "[4,3,2,1]"
    );
    assert_eq!(
        nat(call("$prim.binary.nat32_from_bytes", &[bytes(0x01020304)])),
        0x01020304
    );
    assert_eq!(
        width(call("$prim.binary.nat64_from_bytes", &[bytes(u64::MAX)])).1,
        i128::from(u64::MAX)
    );
    assert_eq!(
        width(call("$prim.binary.int64_from_bytes", &[bytes(u64::MAX)])).1,
        -1
    );
    // Reading a `Nat` or an `Int` back is the one place bytes meet a domain.
    let wide = || bytes(1 << 40);
    assert_eq!(
        render::json(&under(
            Domains::Bits64,
            "$prim.binary.nat_from_bytes",
            &[wide()]
        )),
        "{\"tag\":\"Some\",\"value\":1099511627776}"
    );
    assert_eq!(
        render::json(&under(
            Domains::Js53,
            "$prim.binary.nat_from_bytes",
            &[wide()]
        )),
        "{\"tag\":\"Some\",\"value\":1099511627776}"
    );
    assert_eq!(
        render::json(&under(
            Domains::Bits32,
            "$prim.binary.nat_from_bytes",
            &[wide()]
        )),
        "{\"tag\":\"None\"}"
    );
    let negative = || bytes((-(1i64 << 40)) as u64);
    assert_eq!(
        render::json(&under(
            Domains::Bits64,
            "$prim.binary.int_from_bytes",
            &[negative()]
        )),
        "{\"tag\":\"Some\",\"value\":-1099511627776}"
    );
    assert_eq!(
        render::json(&under(
            Domains::Bits32,
            "$prim.binary.int_from_bytes",
            &[negative()]
        )),
        "{\"tag\":\"None\"}"
    );
    assert_eq!(
        render::json(&under(
            Domains::Bits32,
            "$prim.binary.int_from_bytes",
            &[bytes(u64::MAX)]
        )),
        "{\"tag\":\"Some\",\"value\":-1}"
    );
}

#[test]
fn real_numbers_follow_the_javascript_math_object() {
    let pair = |left: f64, right: f64| [Value::Real(left), Value::Real(right)];
    assert_eq!(real(call("$prim.real.remainder", &pair(-5.5, 2.0))), -1.5);
    assert_eq!(real(call("$prim.real.power", &pair(2.0, 10.0))), 1024.0);
    // `Math.pow` is not `powf` where the base is one and the exponent is not
    // a number or is infinite.
    for arguments in [
        pair(1.0, f64::NAN),
        pair(1.0, f64::INFINITY),
        pair(-1.0, f64::NEG_INFINITY),
        pair(2.0, f64::NAN),
    ] {
        assert!(real(call("$prim.real.power", &arguments)).is_nan());
    }
    assert!(truth(call(
        "$prim.real.ordering_equal",
        &pair(f64::NAN, f64::NAN)
    )));
    assert!(!truth(call("$prim.real.ordering_equal", &pair(-0.0, 0.0))));
    assert!(truth(call("$prim.real.ordering_equal", &pair(1.5, 1.5))));
    assert!(!truth(call(
        "$prim.real.ordering_less_than",
        &pair(f64::NAN, 1.0)
    )));
    assert!(real(call("$prim.real.min", &pair(f64::NAN, 1.0))).is_nan());
    assert!(real(call("$prim.real.max", &pair(1.0, f64::NAN))).is_nan());
    let least = real(call("$prim.real.min", &pair(0.0, -0.0)));
    assert!(least == 0.0 && least.is_sign_negative());
    let greatest = real(call("$prim.real.max", &pair(-0.0, 0.0)));
    assert!(greatest == 0.0 && greatest.is_sign_positive());
    assert_eq!(
        real(call(
            "$prim.real.clamp",
            &[Value::Real(9.0), Value::Real(1.0), Value::Real(3.0)]
        )),
        3.0
    );
    // `Math.round` sends a tie towards positive infinity, and the number just
    // below one half is not a tie at all.
    assert_eq!(int(call("$prim.real.round", &[Value::Real(2.5)])), 3);
    assert_eq!(int(call("$prim.real.round", &[Value::Real(-2.5)])), -2);
    assert_eq!(int(call("$prim.real.round", &[Value::Real(-2.6)])), -3);
    assert_eq!(
        int(call(
            "$prim.real.round",
            &[Value::Real(0.499_999_999_999_999_94)]
        )),
        0
    );
    assert_eq!(int(call("$prim.real.floor", &[Value::Real(-1.5)])), -2);
    assert_eq!(int(call("$prim.real.ceil", &[Value::Real(-1.5)])), -1);
    assert_eq!(int(call("$prim.real.truncate", &[Value::Real(-1.5)])), -1);
    assert!(truth(call("$prim.real.is_nan", &[Value::Real(f64::NAN)])));
    assert!(!truth(call(
        "$prim.real.is_finite",
        &[Value::Real(f64::INFINITY)]
    )));
    assert!(truth(call("$prim.real.is_integer", &[Value::Real(2.0)])));
    assert!(!truth(call("$prim.real.is_integer", &[Value::Real(2.5)])));
    assert!(!truth(call(
        "$prim.real.is_integer",
        &[Value::Real(f64::INFINITY)]
    )));
    assert_eq!(real(call("$prim.real.abs", &[Value::Real(-2.5)])), 2.5);
    assert_eq!(real(call("$prim.real.sqrt", &[Value::Real(9.0)])), 3.0);
    assert_eq!(real(call("$prim.real.log2", &[Value::Real(8.0)])), 3.0);
    assert_eq!(real(call("$prim.real.log10", &[Value::Real(1000.0)])), 3.0);
    assert_eq!(
        real(call("$prim.real.log", &[Value::Real(std::f64::consts::E)])),
        1.0
    );
    assert_eq!(real(call("$prim.real.exp", &[Value::Real(0.0)])), 1.0);
    assert_eq!(real(call("$prim.real.expm1", &[Value::Real(0.0)])), 0.0);
    assert_eq!(real(call("$prim.real.log1p", &[Value::Real(0.0)])), 0.0);
    assert_eq!(real(call("$prim.real.sin", &[Value::Real(0.0)])), 0.0);
    assert_eq!(real(call("$prim.real.cos", &[Value::Real(0.0)])), 1.0);
    assert_eq!(real(call("$prim.real.tan", &[Value::Real(0.0)])), 0.0);
    assert_eq!(real(call("$prim.real.asin", &[Value::Real(0.0)])), 0.0);
    assert_eq!(real(call("$prim.real.acos", &[Value::Real(1.0)])), 0.0);
    assert_eq!(real(call("$prim.real.atan", &[Value::Real(0.0)])), 0.0);
    assert_eq!(
        real(call("$prim.real.atan2", &pair(0.0, -1.0))),
        std::f64::consts::PI
    );
    assert_eq!(real(call("$prim.real.sinh", &[Value::Real(0.0)])), 0.0);
    assert_eq!(real(call("$prim.real.cosh", &[Value::Real(0.0)])), 1.0);
    assert_eq!(real(call("$prim.real.tanh", &[Value::Real(0.0)])), 0.0);
    assert_eq!(real(call("$prim.real.asinh", &[Value::Real(0.0)])), 0.0);
    assert_eq!(real(call("$prim.real.acosh", &[Value::Real(1.0)])), 0.0);
    assert_eq!(real(call("$prim.real.atanh", &[Value::Real(0.0)])), 0.0);
    // A `Nat` or an `Int` argument widens into a real number.
    assert_eq!(real(call("$prim.real.from_nat", &[Value::Nat(7)])), 7.0);
    assert_eq!(real(call("$prim.real.from_int", &[Value::Int(-7)])), -7.0);
}

#[test]
fn text_counts_unicode_scalar_values() {
    // `a`, a musical symbol outside the basic plane, and `b`.
    let score = || Value::string("a\u{1d11e}b");
    assert_eq!(nat(call("$prim.str.len", &[score()])), 3);
    assert_eq!(
        text(call("$prim.str.char_at", &[score(), Value::Nat(1)])),
        "\u{1d11e}"
    );
    assert_eq!(
        text(call("$prim.str.char_at", &[score(), Value::Nat(9)])),
        ""
    );
    assert_eq!(
        int(call("$prim.str.index_of", &[score(), Value::string("b")])),
        2
    );
    assert_eq!(
        int(call("$prim.str.index_of", &[score(), Value::string("z")])),
        -1
    );
    assert_eq!(
        text(call(
            "$prim.str.slice",
            &[score(), Value::Nat(1), Value::Nat(3)]
        )),
        "\u{1d11e}b"
    );
    assert_eq!(
        text(call(
            "$prim.str.slice",
            &[score(), Value::Nat(2), Value::Nat(99)]
        )),
        "b"
    );
    assert_eq!(
        text(call(
            "$prim.str.slice",
            &[score(), Value::Nat(2), Value::Nat(1)]
        )),
        ""
    );
    assert_eq!(text(call("$prim.str.reverse", &[score()])), "b\u{1d11e}a");
    assert!(truth(call(
        "$prim.str.contains",
        &[score(), Value::string("\u{1d11e}")]
    )));
    assert!(truth(call(
        "$prim.str.starts_with",
        &[score(), Value::string("a\u{1d11e}")]
    )));
    assert!(truth(call(
        "$prim.str.ends_with",
        &[score(), Value::string("\u{1d11e}b")]
    )));
    assert!(truth(call(
        "$prim.str.ordering_less_than",
        &[Value::string("Z"), Value::string("a")]
    )));
    // Scalar values order the comparison, not the UTF-16 code units the
    // JavaScript would have compared: U+FFFF is below the musical symbol,
    // where its high surrogate U+D834 would not be.
    assert!(truth(call(
        "$prim.str.ordering_less_than",
        &[Value::string("\u{ffff}"), Value::string("\u{1d11e}")]
    )));
    assert!(truth(call(
        "$prim.str.ordering_greater_than",
        &[Value::string("\u{1d11e}"), Value::string("\u{ffff}")]
    )));
    assert!(truth(call(
        "$prim.str.ordering_greater_than",
        &[score(), Value::string("a\u{1d11e}")]
    )));
    assert_eq!(
        text(call("$prim.str.concat", &[score(), Value::string("!")])),
        "a\u{1d11e}b!"
    );
    assert_eq!(
        text(call(
            "$prim.str.repeat",
            &[Value::string("\u{1d11e}"), Value::Nat(3)]
        )),
        "\u{1d11e}\u{1d11e}\u{1d11e}"
    );
    assert_eq!(
        text(call(
            "$prim.str.repeat",
            &[Value::string("ab"), Value::Nat(0)]
        )),
        ""
    );
    // Padding repeats the fill one scalar value at a time, and an empty fill
    // pads nothing at all.
    assert_eq!(
        text(call(
            "$prim.str.pad_start",
            &[
                Value::string("\u{1d11e}"),
                Value::Nat(4),
                Value::string("xy")
            ]
        )),
        "xyx\u{1d11e}"
    );
    assert_eq!(
        text(call(
            "$prim.str.pad_end",
            &[
                Value::string("a"),
                Value::Nat(3),
                Value::string("\u{1d11e}")
            ]
        )),
        "a\u{1d11e}\u{1d11e}"
    );
    assert_eq!(
        text(call(
            "$prim.str.pad_start",
            &[score(), Value::Nat(9), Value::string("")]
        )),
        "a\u{1d11e}b"
    );
    assert_eq!(
        text(call(
            "$prim.str.pad_start",
            &[score(), Value::Nat(1), Value::string("x")]
        )),
        "a\u{1d11e}b"
    );
    // Trimming strips exactly ECMAScript's whitespace: U+FEFF is whitespace
    // where Rust says it is not, and U+0085 is not where Rust says it is.
    let padded = || Value::string("\u{feff}\u{2028} \u{85}a\u{85} \u{3000}");
    assert_eq!(text(call("$prim.str.trim", &[padded()])), "\u{85}a\u{85}");
    assert_eq!(
        text(call("$prim.str.trim_start", &[padded()])),
        "\u{85}a\u{85} \u{3000}"
    );
    assert_eq!(
        text(call("$prim.str.trim_end", &[padded()])),
        "\u{feff}\u{2028} \u{85}a\u{85}"
    );
    assert_eq!(
        text(call(
            "$prim.str.to_uppercase",
            &[Value::string("stra\u{df}e")]
        )),
        "STRASSE"
    );
    assert_eq!(
        text(call(
            "$prim.str.to_lowercase",
            &[Value::string("\u{c9}T\u{c9}")]
        )),
        "\u{e9}t\u{e9}"
    );
    // Replacement is of a plain substring: a dollar sign in the replacement
    // is text, not a pattern.
    assert_eq!(
        text(call(
            "$prim.str.replace_first",
            &[
                Value::string("a-a"),
                Value::string("a"),
                Value::string("$&")
            ]
        )),
        "$&-a"
    );
    assert_eq!(
        text(call(
            "$prim.str.replace_all",
            &[
                Value::string("a\u{1d11e}a"),
                Value::string("a"),
                Value::string("b")
            ]
        )),
        "b\u{1d11e}b"
    );
    assert_eq!(text(call("$prim.str.from_nat", &[Value::Nat(42)])), "42");
    assert_eq!(text(call("$prim.str.from_int", &[Value::Int(-42)])), "-42");
    assert_eq!(
        text(call("$prim.str.from_real", &[Value::Real(0.1 + 0.2)])),
        "0.30000000000000004"
    );
    assert_eq!(
        text(call("$prim.str.from_real", &[Value::Real(1e21)])),
        "1e+21"
    );
}

#[test]
fn json_reads_and_writes_the_tokens_the_scanner_produces() {
    assert_eq!(
        nat(call("$prim.json.char_code", &[Value::string("\u{1d11e}")])),
        0xd834
    );
    assert_eq!(nat(call("$prim.json.char_code", &[Value::string("A")])), 65);
    assert_eq!(nat(call("$prim.json.char_code", &[Value::string("")])), 0);
    assert_eq!(
        text(call("$prim.json.char_from_code", &[Value::Nat(97)])),
        "a"
    );
    assert_eq!(
        text(call("$prim.json.char_from_code", &[Value::Nat(0x1d11e)])),
        "\u{1d11e}"
    );
    // A lone surrogate is not a scalar value, so it reads as the replacement.
    assert_eq!(
        text(call("$prim.json.char_from_code", &[Value::Nat(0xd834)])),
        "\u{fffd}"
    );
    assert_eq!(
        text(call(
            "$prim.json.quote",
            &[Value::string("a\"b\\c\n\u{1}\u{1d11e}")]
        )),
        "\"a\\\"b\\\\c\\n\\u0001\u{1d11e}\""
    );
    assert_eq!(
        real(call("$prim.json.real_value", &[Value::string("-1.5e2")])),
        -150.0
    );
    assert_eq!(
        real(call("$prim.json.real_value", &[Value::string("1e400")])),
        f64::INFINITY
    );
    assert_eq!(
        real(call("$prim.json.real_value", &[Value::string("")])),
        0.0
    );
    assert!(real(call("$prim.json.real_value", &[Value::string("nope")])).is_nan());
    assert_eq!(
        text(call("$prim.json.real_token", &[Value::Real(-0.0)])),
        "-0.0"
    );
    assert_eq!(
        text(call("$prim.json.real_token", &[Value::Real(0.0)])),
        "0"
    );
    assert_eq!(
        text(call("$prim.json.real_token", &[Value::Real(1.5)])),
        "1.5"
    );
    assert_eq!(
        nat(call("$prim.json.nat_of_digits", &[Value::string("00123")])),
        123
    );
    assert_eq!(
        nat(call(
            "$prim.json.nat_of_digits",
            &[Value::string("99999999999999999999999")]
        )),
        u64::MAX
    );
    assert_eq!(
        int(call("$prim.json.int_of_digits", &[Value::string("-42")])),
        -42
    );
    assert_eq!(
        int(call(
            "$prim.json.int_of_digits",
            &[Value::string("-99999999999999999999999")]
        )),
        i64::MIN
    );
    assert_eq!(
        width(call(
            "$prim.json.nat64_of_digits",
            &[Value::string("18446744073709551615")]
        )),
        (FixedInt::Nat64, i128::from(u64::MAX))
    );
    assert_eq!(
        width(call(
            "$prim.json.int64_of_digits",
            &[Value::string("-9223372036854775808")]
        )),
        (FixedInt::Int64, i128::from(i64::MIN))
    );
}

#[test]
fn arrays_are_read_and_rebuilt_whole() {
    let items = || Value::array(vec![Value::Nat(1), Value::Nat(2), Value::Nat(3)]);
    assert_eq!(nat(call("$arrayLen", &[items()])), 3);
    assert_eq!(nat(call("$arrayLen", &[Value::array(Vec::new())])), 0);
    assert_eq!(
        json("$arrayGet", &[items(), Value::Nat(1)]),
        "{\"tag\":\"Some\",\"value\":2}"
    );
    assert_eq!(
        json("$arrayGet", &[items(), Value::Nat(3)]),
        "{\"tag\":\"None\"}"
    );
    assert_eq!(
        json("$arraySet", &[items(), Value::Nat(1), Value::Nat(9)]),
        "{\"tag\":\"Some\",\"value\":[1,9,3]}"
    );
    assert_eq!(
        json("$arraySet", &[items(), Value::Nat(3), Value::Nat(9)]),
        "{\"tag\":\"None\"}"
    );
    assert_eq!(json("$arrayPush", &[items(), Value::Nat(4)]), "[1,2,3,4]");
    assert_eq!(
        json("$arrayPrepend", &[items(), Value::Nat(0)]),
        "[0,1,2,3]"
    );
    assert_eq!(
        json(
            "$arrayConcat",
            &[items(), Value::array(vec![Value::Nat(4)])]
        ),
        "[1,2,3,4]"
    );
    assert_eq!(
        json("$arraySlice", &[items(), Value::Nat(1), Value::Nat(3)]),
        "{\"tag\":\"Some\",\"value\":[2,3]}"
    );
    assert_eq!(
        json("$arraySlice", &[items(), Value::Nat(2), Value::Nat(1)]),
        "{\"tag\":\"None\"}"
    );
    assert_eq!(
        json("$arraySlice", &[items(), Value::Nat(0), Value::Nat(9)]),
        "{\"tag\":\"None\"}"
    );
    assert_eq!(
        json("$arrayPop", &[items()]),
        "{\"tag\":\"Some\",\"value\":{\"0\":3,\"1\":[1,2]}}"
    );
    assert_eq!(
        json("$arrayPop", &[Value::array(Vec::new())]),
        "{\"tag\":\"None\"}"
    );
    assert_eq!(
        fails("$arrayLen", &[Value::Nat(1)]),
        "expected an array, found a natural number"
    );
}

#[test]
fn real_numbers_are_spelled_the_way_javascript_spells_them() {
    for (value, spelling) in [
        (f64::NAN, "NaN"),
        (f64::INFINITY, "Infinity"),
        (f64::NEG_INFINITY, "-Infinity"),
        (0.0, "0"),
        (-0.0, "0"),
        (100.0, "100"),
        (1.5, "1.5"),
        (-2.0, "-2"),
        (0.1 + 0.2, "0.30000000000000004"),
        (1e21, "1e+21"),
        (1e20, "100000000000000000000"),
        (123_456_789_012_345_680_000.0, "123456789012345680000"),
        (1e-7, "1e-7"),
        (0.000_001, "0.000001"),
        (5e-324, "5e-324"),
        (f64::MAX, "1.7976931348623157e+308"),
        (-1.5e-9, "-1.5e-9"),
        (1e-6, "0.000001"),
    ] {
        assert_eq!(number::to_string(value), spelling, "{value:?}");
    }
}

#[test]
fn shortest_digits_are_settled_the_way_javascript_settles_them() {
    for (value, spelling) in [
        // These two are exactly halfway between two shortest runs, written
        // as quarters and eighths so that the halfway point is plain, and
        // the even run wins. Rust's own formatting picks the odd one and
        // spells them `...27.3` and `...04.13`.
        (8_725_981_186_952_109.0 / 4.0, "2181495296738027.2"),
        (602_012_437_563_233.0 / 8.0, "75251554695404.12"),
        (-8_725_981_186_952_109.0 / 4.0, "-2181495296738027.2"),
        // A power of two begins a binade, so every run that reads back as it
        // lies above it and rounding to the shortest length steps below all
        // of them. The run that reads back is the one to write.
        (2.0f64.powi(-24), "5.960464477539063e-8"),
        (2.0f64.powi(-44), "5.684341886080802e-14"),
        (2.0f64.powi(-1007), "7.291122019556398e-304"),
        // Ordinary numbers, whose shortest run is nowhere near a tie.
        (2.0f64.powi(-24) * 1e10, "596.0464477539062"),
        (4.35, "4.35"),
        (1.005, "1.005"),
    ] {
        assert_eq!(number::to_string(value), spelling, "{value:?}");
    }
}
