-- Metadata on every kind of top-level definition, in every literal form the
-- language admits. None of it reaches the program: the values below compute
-- exactly what they would without a single attribute.

@doc "Adds two naturals."
@since 2n
@stable
extern add : fn(Nat, Nat) -> Nat = "(left, right) => left + right"

@shape (1n, "two")
@fields { first: "Nat", second: "String" }
type Pair = { first: Nat, second: String }

@level #Debug
@retired #Removed 3n
effect Log = Nat -> ()

@owner { team: "core", "since when": 1.5, 0: true }
module Inline =
  @inner [1i, 0.5, "x"]
  let doubled = fn n => add n n
end

@file_backed
@notes \\ A module whose body is another file.
       \\ The attribute rides on the declaration here.
module Notes

@k () @j {} @flags [#A, #B ()]
let pair = { first: add 1n 2n, second: "three" }

@k let (left, right) = (Inline::doubled 4n, Notes::greeting)

@test
let logged : () -> Nat + !Log = fn _ => do
  let _ = !Log 1n
  return 7n
end

@handled
let quiet = handle logged () with
  | !Log n => ()
  | return v => v
end

@discarded "nothing is bound here"
let _ = add 1n 1n
