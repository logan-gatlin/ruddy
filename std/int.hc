extern add : fn(Int, Int) -> Int = "(left, right) => left + right"
extern subtract : fn(Int, Int) -> Int = "(left, right) => left - right"
extern multiply : fn(Int, Int) -> Int = "(left, right) => left * right"
extern divide : fn(Int, Int) -> Int = "(left, right) => Math.trunc(left / right)"
extern remainder : fn(Int, Int) -> Int = "(left, right) => left % right"
extern negate : fn(Int) -> Int = "value => -value"
extern abs : fn(Int) -> Int = "Math.abs"

extern equal : fn(Int, Int) -> Boolean = "(left, right) => left === right"
extern not_equal : fn(Int, Int) -> Boolean = "(left, right) => left !== right"
extern less_than : fn(Int, Int) -> Boolean = "(left, right) => left < right"
extern less_than_or_equal : fn(Int, Int) -> Boolean = "(left, right) => left <= right"
extern greater_than : fn(Int, Int) -> Boolean = "(left, right) => left > right"
extern greater_than_or_equal : fn(Int, Int) -> Boolean = "(left, right) => left >= right"

extern min : fn(Int, Int) -> Int = "Math.min"
extern max : fn(Int, Int) -> Int = "Math.max"
extern clamp : fn(Int, Int, Int) -> Int = "(value, minimum, maximum) => Math.min(Math.max(value, minimum), maximum)"
let is_zero: Int -> Boolean = fn value => match value with | 0i => true | _ => false end

extern from_nat : fn(Nat) -> Int = "value => value"
extern from_real : fn(Real) -> Int = "Math.trunc"

let compare : Int -> Int -> Ordering = fn left right =>
  if less_than left right then #Less
  else if greater_than left right then #Greater
  else #Equal
  end
