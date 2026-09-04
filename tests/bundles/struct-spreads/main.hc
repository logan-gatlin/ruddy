extern mark : fn(Nat) -> Nat = "host.mark"
extern mark_base : fn({ a: Nat, b: Nat }) -> { a: Nat, b: Nat } = "host.markBase"

let base = { a: 1n, b: "b", c: true }
let copy = { ..base }
let extended = { d: 2n, ..base }
let replaced = { a: 5n, ..base }
let retyped = { a: "text", ..base }
let several = { a: 3n, c: false, e: 9n, ..base }
let pick = fn v => v
let computed = { d: 4n, ..pick base }
let from_unit = { a: 1n, ..() }
let from_empty = { ..{} }
let pair = (1n, 2n)
let from_pair = { 1: "second", ..pair }
let set_a = fn v => { a: 7n, ..v }
let added = set_a { b: 1n }
let overwritten = set_a { a: "old", b: 2n }
let read_back = fn v => (v.a, v.b, v.c)
let read_added = fn v => (v.d, v.e)
let traced = fn v => { a: mark 10n, c: mark 30n, ..mark_base v }
