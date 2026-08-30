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
