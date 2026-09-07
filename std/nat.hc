extern add : fn(Nat, Nat) -> Nat = "(left, right) => left + right"
extern subtract : fn(Nat, Nat) -> Nat = "(left, right) => Math.max(0, left - right)"
extern multiply : fn(Nat, Nat) -> Nat = "(left, right) => left * right"
extern divide : fn(Nat, Nat) -> Nat = "(left, right) => Math.trunc(left / right)"
extern remainder : fn(Nat, Nat) -> Nat = "(left, right) => left % right"

extern equal : fn(Nat, Nat) -> Boolean = "(left, right) => left === right"
extern not_equal : fn(Nat, Nat) -> Boolean = "(left, right) => left !== right"
extern less_than : fn(Nat, Nat) -> Boolean = "(left, right) => left < right"
extern less_than_or_equal : fn(Nat, Nat) -> Boolean = "(left, right) => left <= right"
extern greater_than : fn(Nat, Nat) -> Boolean = "(left, right) => left > right"
extern greater_than_or_equal : fn(Nat, Nat) -> Boolean = "(left, right) => left >= right"

extern min : fn(Nat, Nat) -> Nat = "Math.min"
extern max : fn(Nat, Nat) -> Nat = "Math.max"
extern clamp : fn(Nat, Nat, Nat) -> Nat = "(value, minimum, maximum) => Math.min(Math.max(value, minimum), maximum)"
let is_zero: Nat -> Boolean = fn value => match value with | 0n => true | _ => false end

extern from_int : fn(Int) -> Nat = "value => Math.max(0, value)"
extern from_real : fn(Real) -> Nat = "value => Math.max(0, Math.trunc(value))"

let compare : Nat -> Nat -> Ordering = fn left right =>
  if less_than left right then #Less
  else if greater_than left right then #Greater
  else #Equal
  end

-- Nat8: arithmetic wraps modulo 2^8; division truncates toward zero.
let min_value8 : Nat8 = 0n8
let max_value8 : Nat8 = 255n8
extern add8 : fn(Nat8, Nat8) -> Nat8 = "(left, right) => ((left + right) & 255)"
extern subtract8 : fn(Nat8, Nat8) -> Nat8 = "(left, right) => ((left - right) & 255)"
extern multiply8 : fn(Nat8, Nat8) -> Nat8 = "(left, right) => ((left * right) & 255)"
extern divide8 : fn(Nat8, Nat8) -> Nat8 = "(left, right) => { if (right === 0) throw new RangeError(\"division by zero\"); return ((Math.trunc(left / right)) & 255); }"
extern remainder8 : fn(Nat8, Nat8) -> Nat8 = "(left, right) => { if (right === 0) throw new RangeError(\"division by zero\"); return ((left % right) & 255); }"
extern equal8 : fn(Nat8, Nat8) -> Boolean = "(left, right) => left === right"
extern not_equal8 : fn(Nat8, Nat8) -> Boolean = "(left, right) => left !== right"
extern less_than8 : fn(Nat8, Nat8) -> Boolean = "(left, right) => left < right"
extern less_than_or_equal8 : fn(Nat8, Nat8) -> Boolean = "(left, right) => left <= right"
extern greater_than8 : fn(Nat8, Nat8) -> Boolean = "(left, right) => left > right"
extern greater_than_or_equal8 : fn(Nat8, Nat8) -> Boolean = "(left, right) => left >= right"
extern min8 : fn(Nat8, Nat8) -> Nat8 = "(left, right) => left < right ? left : right"
extern max8 : fn(Nat8, Nat8) -> Nat8 = "(left, right) => left > right ? left : right"
extern clamp8 : fn(Nat8, Nat8, Nat8) -> Nat8 = "(value, minimum, maximum) => value < minimum ? minimum : value > maximum ? maximum : value"
extern is_zero8 : fn(Nat8) -> Boolean = "value => value === 0"
extern from_nat8 : fn(Nat) -> Nat8 = "value => { if (!Number.isSafeInteger(value)) throw new RangeError(\"expected a safe integer\"); return ((value) & 255); }"
extern from_int8 : fn(Int) -> Nat8 = "value => { if (!Number.isSafeInteger(value)) throw new RangeError(\"expected a safe integer\"); return ((value) & 255); }"
extern to_string8 : fn(Nat8) -> String = "value => String(value)"
extern to_nat8 : fn(Nat8) -> Nat = "value => { const result = value; if (!Number.isSafeInteger(result) || result < 0) throw new RangeError(\"integer out of range\"); return result; }"
let compare8 : Nat8 -> Nat8 -> Ordering = fn left right =>
  if less_than8 left right then #Less
  else if greater_than8 left right then #Greater
  else #Equal
  end

-- Nat16: arithmetic wraps modulo 2^16; division truncates toward zero.
let min_value16 : Nat16 = 0n16
let max_value16 : Nat16 = 65535n16
extern add16 : fn(Nat16, Nat16) -> Nat16 = "(left, right) => ((left + right) & 65535)"
extern subtract16 : fn(Nat16, Nat16) -> Nat16 = "(left, right) => ((left - right) & 65535)"
extern multiply16 : fn(Nat16, Nat16) -> Nat16 = "(left, right) => ((left * right) & 65535)"
extern divide16 : fn(Nat16, Nat16) -> Nat16 = "(left, right) => { if (right === 0) throw new RangeError(\"division by zero\"); return ((Math.trunc(left / right)) & 65535); }"
extern remainder16 : fn(Nat16, Nat16) -> Nat16 = "(left, right) => { if (right === 0) throw new RangeError(\"division by zero\"); return ((left % right) & 65535); }"
extern equal16 : fn(Nat16, Nat16) -> Boolean = "(left, right) => left === right"
extern not_equal16 : fn(Nat16, Nat16) -> Boolean = "(left, right) => left !== right"
extern less_than16 : fn(Nat16, Nat16) -> Boolean = "(left, right) => left < right"
extern less_than_or_equal16 : fn(Nat16, Nat16) -> Boolean = "(left, right) => left <= right"
extern greater_than16 : fn(Nat16, Nat16) -> Boolean = "(left, right) => left > right"
extern greater_than_or_equal16 : fn(Nat16, Nat16) -> Boolean = "(left, right) => left >= right"
extern min16 : fn(Nat16, Nat16) -> Nat16 = "(left, right) => left < right ? left : right"
extern max16 : fn(Nat16, Nat16) -> Nat16 = "(left, right) => left > right ? left : right"
extern clamp16 : fn(Nat16, Nat16, Nat16) -> Nat16 = "(value, minimum, maximum) => value < minimum ? minimum : value > maximum ? maximum : value"
extern is_zero16 : fn(Nat16) -> Boolean = "value => value === 0"
extern from_nat16 : fn(Nat) -> Nat16 = "value => { if (!Number.isSafeInteger(value)) throw new RangeError(\"expected a safe integer\"); return ((value) & 65535); }"
extern from_int16 : fn(Int) -> Nat16 = "value => { if (!Number.isSafeInteger(value)) throw new RangeError(\"expected a safe integer\"); return ((value) & 65535); }"
extern to_string16 : fn(Nat16) -> String = "value => String(value)"
extern to_nat16 : fn(Nat16) -> Nat = "value => { const result = value; if (!Number.isSafeInteger(result) || result < 0) throw new RangeError(\"integer out of range\"); return result; }"
let compare16 : Nat16 -> Nat16 -> Ordering = fn left right =>
  if less_than16 left right then #Less
  else if greater_than16 left right then #Greater
  else #Equal
  end

-- Nat32: arithmetic wraps modulo 2^32; division truncates toward zero.
let min_value32 : Nat32 = 0n32
let max_value32 : Nat32 = 4294967295n32
extern add32 : fn(Nat32, Nat32) -> Nat32 = "(left, right) => ((left + right) >>> 0)"
extern subtract32 : fn(Nat32, Nat32) -> Nat32 = "(left, right) => ((left - right) >>> 0)"
extern multiply32 : fn(Nat32, Nat32) -> Nat32 = "(left, right) => ((Math.imul(left, right)) >>> 0)"
extern divide32 : fn(Nat32, Nat32) -> Nat32 = "(left, right) => { if (right === 0) throw new RangeError(\"division by zero\"); return ((Math.trunc(left / right)) >>> 0); }"
extern remainder32 : fn(Nat32, Nat32) -> Nat32 = "(left, right) => { if (right === 0) throw new RangeError(\"division by zero\"); return ((left % right) >>> 0); }"
extern equal32 : fn(Nat32, Nat32) -> Boolean = "(left, right) => left === right"
extern not_equal32 : fn(Nat32, Nat32) -> Boolean = "(left, right) => left !== right"
extern less_than32 : fn(Nat32, Nat32) -> Boolean = "(left, right) => left < right"
extern less_than_or_equal32 : fn(Nat32, Nat32) -> Boolean = "(left, right) => left <= right"
extern greater_than32 : fn(Nat32, Nat32) -> Boolean = "(left, right) => left > right"
extern greater_than_or_equal32 : fn(Nat32, Nat32) -> Boolean = "(left, right) => left >= right"
extern min32 : fn(Nat32, Nat32) -> Nat32 = "(left, right) => left < right ? left : right"
extern max32 : fn(Nat32, Nat32) -> Nat32 = "(left, right) => left > right ? left : right"
extern clamp32 : fn(Nat32, Nat32, Nat32) -> Nat32 = "(value, minimum, maximum) => value < minimum ? minimum : value > maximum ? maximum : value"
extern is_zero32 : fn(Nat32) -> Boolean = "value => value === 0"
extern from_nat32 : fn(Nat) -> Nat32 = "value => { if (!Number.isSafeInteger(value)) throw new RangeError(\"expected a safe integer\"); return ((value) >>> 0); }"
extern from_int32 : fn(Int) -> Nat32 = "value => { if (!Number.isSafeInteger(value)) throw new RangeError(\"expected a safe integer\"); return ((value) >>> 0); }"
extern to_string32 : fn(Nat32) -> String = "value => String(value)"
extern to_nat32 : fn(Nat32) -> Nat = "value => { const result = value; if (!Number.isSafeInteger(result) || result < 0) throw new RangeError(\"integer out of range\"); return result; }"
let compare32 : Nat32 -> Nat32 -> Ordering = fn left right =>
  if less_than32 left right then #Less
  else if greater_than32 left right then #Greater
  else #Equal
  end

-- Nat64: arithmetic wraps modulo 2^64; division truncates toward zero.
let min_value64 : Nat64 = 0n64
let max_value64 : Nat64 = 18446744073709551615n64
extern add64 : fn(Nat64, Nat64) -> Nat64 = "(left, right) => BigInt.asUintN(64, left + right)"
extern subtract64 : fn(Nat64, Nat64) -> Nat64 = "(left, right) => BigInt.asUintN(64, left - right)"
extern multiply64 : fn(Nat64, Nat64) -> Nat64 = "(left, right) => BigInt.asUintN(64, left * right)"
extern divide64 : fn(Nat64, Nat64) -> Nat64 = "(left, right) => BigInt.asUintN(64, left / right)"
extern remainder64 : fn(Nat64, Nat64) -> Nat64 = "(left, right) => BigInt.asUintN(64, left % right)"
extern equal64 : fn(Nat64, Nat64) -> Boolean = "(left, right) => left === right"
extern not_equal64 : fn(Nat64, Nat64) -> Boolean = "(left, right) => left !== right"
extern less_than64 : fn(Nat64, Nat64) -> Boolean = "(left, right) => left < right"
extern less_than_or_equal64 : fn(Nat64, Nat64) -> Boolean = "(left, right) => left <= right"
extern greater_than64 : fn(Nat64, Nat64) -> Boolean = "(left, right) => left > right"
extern greater_than_or_equal64 : fn(Nat64, Nat64) -> Boolean = "(left, right) => left >= right"
extern min64 : fn(Nat64, Nat64) -> Nat64 = "(left, right) => left < right ? left : right"
extern max64 : fn(Nat64, Nat64) -> Nat64 = "(left, right) => left > right ? left : right"
extern clamp64 : fn(Nat64, Nat64, Nat64) -> Nat64 = "(value, minimum, maximum) => value < minimum ? minimum : value > maximum ? maximum : value"
extern is_zero64 : fn(Nat64) -> Boolean = "value => value === 0n"
extern from_nat64 : fn(Nat) -> Nat64 = "value => { if (!Number.isSafeInteger(value)) throw new RangeError(\"expected a safe integer\"); return BigInt.asUintN(64, BigInt(value)); }"
extern from_int64 : fn(Int) -> Nat64 = "value => { if (!Number.isSafeInteger(value)) throw new RangeError(\"expected a safe integer\"); return BigInt.asUintN(64, BigInt(value)); }"
extern to_string64 : fn(Nat64) -> String = "value => String(value)"
extern to_nat64 : fn(Nat64) -> Nat = "value => { const result = Number(value); if (!Number.isSafeInteger(result) || result < 0) throw new RangeError(\"integer out of range\"); return result; }"
let compare64 : Nat64 -> Nat64 -> Ordering = fn left right =>
  if less_than64 left right then #Less
  else if greater_than64 left right then #Greater
  else #Equal
  end
