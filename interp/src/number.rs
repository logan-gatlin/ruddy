//! Number formatting and parsing under the contracts the standard library's
//! JSON and text modules assume.
//!
//! ECMAScript is the contract, not Rust: a real number is spelled the way
//! `Number.prototype.toString` spells it, text is read the way `Number(text)`
//! reads it, and a run of decimal digits is read the way `Number` and
//! `BigInt` read one. Rust's own `to_string` and `parse` agree on the digits
//! but not on the layout around them, so the layout is written out here.

/// Whether a character is ECMAScript `WhiteSpace` or a `LineTerminator`:
/// what `String.prototype.trim` strips and what `Number(text)` ignores at
/// either end. It is not Rust's `char::is_whitespace`, which counts U+0085
/// and does not count U+FEFF.
pub(crate) fn space(ch: char) -> bool {
    matches!(
        ch,
        '\u{9}'..='\u{d}'
            | '\u{20}'
            | '\u{a0}'
            | '\u{1680}'
            | '\u{2000}'..='\u{200a}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{202f}'
            | '\u{205f}'
            | '\u{3000}'
            | '\u{feff}'
    )
}

/// A real number as text, the way ECMAScript's `Number.prototype.toString`
/// spells it.
///
/// The digits are the shortest run that reads back as the same number, and
/// of two such runs the closer one, and of two equally close runs the one
/// whose last digit is even. Rust's exponential formatting gives a shortest
/// run but breaks a tie the other way, so only its length is taken from Rust
/// and the digits themselves from rounding the number to that length, which
/// is round-half-to-even. Rounding is over every run of that length rather
/// than over the ones that read back, so it can step off the end of them
/// where a binade begins and the runs that read back all lie above the
/// number; Rust's run is the one to keep there.
///
/// What ECMAScript settles beyond the digits is where the decimal point
/// goes. A number is written plainly while its point sits within twenty-one
/// places of the digits and no more than six zeros of the other side of it,
/// and in exponential form with a signed exponent otherwise.
pub fn to_string(value: f64) -> String {
    if value.is_nan() {
        return "NaN".to_string();
    }
    // Both zeros are `0`, and a negative number is its magnitude with a sign,
    // so everything below this reads as a positive number.
    if value == 0.0 {
        return "0".to_string();
    }
    if value < 0.0 {
        return format!("-{}", to_string(-value));
    }
    if value.is_infinite() {
        return "Infinity".to_string();
    }
    // A shortest run that reads back as this number, and the number rounded
    // to that many digits, which settles a tie the way ECMAScript settles it.
    // The rounded run is the one to write unless it no longer reads back as
    // this number, which leaves the run Rust wrote.
    let shortest = format!("{value:e}");
    let (digits, exponent) = split(&shortest);
    let rounded = format!("{value:.*e}", digits.len() - 1);
    let (digits, exponent) = if rounded.parse().is_ok_and(|read: f64| read == value) {
        split(&rounded)
    } else {
        (digits, exponent)
    };
    // Rounding can carry, as ninety-nine rounds to one hundred, and leave a
    // zero on the end that no shortest run has.
    let digits = digits.trim_end_matches('0');
    // `count` is how many digits there are and `point` is where the decimal
    // point falls among them, counted from the left and possibly outside them.
    let count = i32::try_from(digits.len()).expect("a number has few digits");
    let point = exponent + 1;
    if count <= point && point <= 21 {
        format!("{digits}{}", "0".repeat((point - count) as usize))
    } else if 0 < point && point <= 21 {
        let at = point as usize;
        format!("{}.{}", &digits[..at], &digits[at..])
    } else if -6 < point && point <= 0 {
        format!("0.{}{digits}", "0".repeat((-point) as usize))
    } else if count == 1 {
        format!("{digits}e{}", exponential(point - 1))
    } else {
        format!(
            "{}.{}e{}",
            &digits[..1],
            &digits[1..],
            exponential(point - 1)
        )
    }
}

/// The number ECMAScript's `Number(text)` reads from text: nothing is zero,
/// text that is not a number at all is NaN, and a number too large for a
/// double is infinite.
///
/// The hexadecimal, octal and binary spellings ECMAScript also accepts are
/// deliberately left out: the only text that reaches this is a JSON number
/// token, which has none of them.
pub fn value(text: &str) -> f64 {
    let text = text.trim_matches(space);
    if text.is_empty() {
        return 0.0;
    }
    let body = text.strip_prefix(['+', '-']).unwrap_or(text);
    if body == "Infinity" {
        return if text.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    // Rust reads `inf` and `nan` as numbers where ECMAScript reads neither.
    if body
        .chars()
        .any(|ch| ch.is_ascii_alphabetic() && ch != 'e' && ch != 'E')
    {
        return f64::NAN;
    }
    text.parse().unwrap_or(f64::NAN)
}

/// The value of a run of decimal digits as a natural number, saturating
/// rather than wrapping: text the scanner accepted may still spell a number
/// larger than the type holds.
pub fn natural(text: &str) -> u64 {
    let (negative, magnitude, _) = decimal(text);
    if negative {
        0
    } else {
        u64::try_from(magnitude).unwrap_or(u64::MAX)
    }
}

/// The value of a run of decimal digits, with an optional sign, as a signed
/// integer, saturating at either end.
pub fn integer(text: &str) -> i64 {
    let (negative, magnitude, _) = decimal(text);
    if negative {
        magnitude
            .try_into()
            .ok()
            .and_then(|magnitude: i128| i64::try_from(-magnitude).ok())
            .unwrap_or(i64::MIN)
    } else {
        i64::try_from(magnitude).unwrap_or(i64::MAX)
    }
}

/// The value of a run of decimal digits, with an optional sign, taken modulo
/// two to the sixty-fourth: what `BigInt.asUintN(64, BigInt(text))` gives,
/// since the caller has already checked the range the digits belong to.
pub fn sixty_four(text: &str) -> u64 {
    let (negative, _, wrapped) = decimal(text);
    if negative {
        wrapped.wrapping_neg()
    } else {
        wrapped
    }
}

/// The digits and the exponent of a number Rust wrote in exponential form,
/// with the decimal point taken out of the digits: the exponent already says
/// where it belongs.
fn split(formatted: &str) -> (String, i32) {
    let (mantissa, exponent) = formatted
        .split_once('e')
        .expect("Rust writes an exponent for every finite number");
    let digits = mantissa.chars().filter(|ch| *ch != '.').collect();
    let exponent = exponent
        .parse()
        .expect("Rust writes the exponent in decimal");
    (digits, exponent)
}

/// A signed exponent, which ECMAScript always writes with its sign.
fn exponential(exponent: i32) -> String {
    if exponent < 0 {
        format!("-{}", exponent.unsigned_abs())
    } else {
        format!("+{exponent}")
    }
}

/// A decimal literal's sign, its magnitude saturated at the widest integer
/// there is, and its magnitude modulo two to the sixty-fourth. Reading stops
/// at the first character that is not a digit.
fn decimal(text: &str) -> (bool, u128, u64) {
    let negative = text.starts_with('-');
    let body = text.strip_prefix(['+', '-']).unwrap_or(text);
    let mut magnitude: u128 = 0;
    let mut wrapped: u64 = 0;
    for ch in body.chars() {
        let Some(digit) = ch.to_digit(10) else { break };
        magnitude = magnitude
            .saturating_mul(10)
            .saturating_add(u128::from(digit));
        wrapped = wrapped.wrapping_mul(10).wrapping_add(u64::from(digit));
    }
    (negative, magnitude, wrapped)
}
